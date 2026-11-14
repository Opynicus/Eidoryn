//! Worker-owned rendering engines. Device failures are returned to the worker,
//! which latches the error for the emulation loop and drains pending replies.
use super::{
    DisplaySettings, RendererMode,
    rasterizer::{RasterCommand, Rasterizer},
};
use std::{
    io,
    sync::{Arc, OnceLock},
};

pub(super) type ErrorLatch = Arc<OnceLock<String>>;

/// A backend is available only after its physical adapter, device, shaders and
/// resource allocation have been initialized successfully on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RendererAvailability {
    pub mode: RendererMode,
    pub device: Option<String>,
    pub reason: Option<String>,
}

/// Probe each explicit hardware backend. This initializes graphics drivers and
/// can take time; desktop callers should run it in the background and cache it.
/// Automatic mode is available when any backend in its platform order is ready.
pub fn hardware_backends() -> Vec<RendererAvailability> {
    [
        RendererMode::Dx12,
        RendererMode::Vulkan,
        RendererMode::OpenGl,
        RendererMode::Metal,
    ]
    .into_iter()
    .map(|mode| match Engine::new(mode) {
        Ok((engine, _)) => RendererAvailability {
            mode,
            device: Some(engine.name()),
            reason: None,
        },
        Err(error) => RendererAvailability {
            mode,
            device: None,
            reason: Some(error.to_string()),
        },
    })
    .collect()
}

fn hardware_preference() -> &'static [RendererMode] {
    use RendererMode::{Dx12, Metal, OpenGl, Vulkan};
    if cfg!(target_os = "windows") {
        &[Dx12, Vulkan, OpenGl]
    } else if cfg!(target_os = "macos") {
        &[Metal, Vulkan, OpenGl]
    } else {
        &[Vulkan, OpenGl]
    }
}

fn unsupported_backend(mode: RendererMode) -> Option<String> {
    let compiled = match mode {
        RendererMode::Dx12 if !cfg!(target_os = "windows") => {
            return Some("DirectX 12 requires Windows".into());
        }
        RendererMode::Metal if !cfg!(target_os = "macos") => {
            return Some("Metal requires macOS".into());
        }
        RendererMode::Dx12 => cfg!(feature = "dx12"),
        RendererMode::Metal => cfg!(feature = "metal"),
        RendererMode::Vulkan => cfg!(feature = "vulkan"),
        RendererMode::OpenGl => cfg!(feature = "opengl"),
        _ => return Some("An explicit hardware backend must be selected".into()),
    };
    (!compiled).then(|| {
        format!(
            "{} support is not compiled in; build with --features {}",
            mode.label(),
            mode.id()
        )
    })
}

fn select_hardware<T>(
    candidates: &[RendererMode],
    initialize: impl FnMut(RendererMode, &mut ErrorLatch) -> io::Result<T>,
) -> io::Result<(T, ErrorLatch)> {
    select_candidates(
        candidates
            .iter()
            .map(|&mode| (mode.label().to_owned(), mode)),
        initialize,
    )
}

pub(super) fn select_candidates<C, T>(
    candidates: impl IntoIterator<Item = (String, C)>,
    mut initialize: impl FnMut(C, &mut ErrorLatch) -> io::Result<T>,
) -> io::Result<(T, ErrorLatch)> {
    let mut failures = Vec::new();
    for (name, candidate) in candidates {
        // Error callbacks from a failed device must never poison the next one.
        // An API may replace this provisional latch with its winning adapter's.
        let mut error = Arc::new(OnceLock::new());
        match initialize(candidate, &mut error) {
            Ok(engine) if error.get().is_none() => return Ok((engine, error)),
            Ok(_) => failures.push(format!("{name}: {}", error.get().unwrap())),
            Err(failure) => failures.push(format!("{name}: {failure}")),
        }
    }
    Err(io::Error::other(format!(
        "Hardware renderer initialization failed: {}",
        failures.join("; ")
    )))
}

pub(super) enum Engine {
    Software(Rasterizer),
    #[cfg(any(
        feature = "dx12",
        feature = "vulkan",
        feature = "opengl",
        feature = "metal"
    ))]
    Hardware(Box<super::compute::ComputeRenderer>),
}

#[cfg_attr(
    not(any(
        feature = "dx12",
        feature = "vulkan",
        feature = "opengl",
        feature = "metal"
    )),
    expect(
        clippy::unnecessary_wraps,
        reason = "Hardware operations can fail; keep the same renderer API in software-only builds"
    )
)]
impl Engine {
    pub(super) fn new(mode: RendererMode) -> io::Result<(Self, ErrorLatch)> {
        if !mode.is_hardware() {
            return Ok((
                Self::Software(Rasterizer::empty()),
                Arc::new(OnceLock::new()),
            ));
        }
        let candidates = if mode == RendererMode::Hardware {
            hardware_preference()
        } else {
            std::slice::from_ref(&mode)
        };
        select_hardware(candidates, Self::hardware)
    }

    fn hardware(mode: RendererMode, error: &mut ErrorLatch) -> io::Result<Self> {
        if let Some(reason) = unsupported_backend(mode) {
            return Err(io::Error::other(reason));
        }
        #[cfg(any(
            feature = "dx12",
            feature = "vulkan",
            feature = "opengl",
            feature = "metal"
        ))]
        {
            let (gpu, selected_error) = super::compute::ComputeRenderer::new(mode)?;
            *error = selected_error;
            Ok(Self::Hardware(Box::new(gpu)))
        }
        #[cfg(not(any(
            feature = "dx12",
            feature = "vulkan",
            feature = "opengl",
            feature = "metal"
        )))]
        {
            let _ = error;
            unreachable!("All hardware modes were rejected as not compiled")
        }
    }

    pub(super) fn name(&self) -> String {
        match self {
            Self::Software(_) => "CPU software rasterizer".into(),
            #[cfg(any(
                feature = "dx12",
                feature = "vulkan",
                feature = "opengl",
                feature = "metal"
            ))]
            Self::Hardware(gpu) => gpu.name.clone(),
        }
    }

    pub(super) fn start(&mut self, raster: Rasterizer) -> Result<(), String> {
        match self {
            Self::Software(current) => *current = raster,
            #[cfg(any(
                feature = "dx12",
                feature = "vulkan",
                feature = "opengl",
                feature = "metal"
            ))]
            Self::Hardware(gpu) => {
                gpu.start(raster)?;
                gpu.synchronize()?;
            }
        }
        Ok(())
    }

    pub(super) fn execute(&mut self, command: RasterCommand) -> Result<(), String> {
        match self {
            Self::Software(raster) => {
                raster.execute(command);
                Ok(())
            }
            #[cfg(any(
                feature = "dx12",
                feature = "vulkan",
                feature = "opengl",
                feature = "metal"
            ))]
            Self::Hardware(gpu) => gpu.execute(command),
        }
    }

    pub(super) fn read(&mut self, indices: [Option<usize>; 2]) -> Result<[u32; 2], String> {
        match self {
            Self::Software(raster) => {
                Ok(indices.map(|i| i.map_or(0, |i| u32::from(raster.vram[i]))))
            }
            #[cfg(any(
                feature = "dx12",
                feature = "vulkan",
                feature = "opengl",
                feature = "metal"
            ))]
            Self::Hardware(gpu) => gpu.read(indices),
        }
    }

    pub(super) fn display(&mut self, display: DisplaySettings) -> Result<Vec<u8>, String> {
        match self {
            Self::Software(raster) => Ok(raster.display_rgb(display)),
            #[cfg(any(
                feature = "dx12",
                feature = "vulkan",
                feature = "opengl",
                feature = "metal"
            ))]
            Self::Hardware(gpu) => gpu.display(display),
        }
    }

    pub(super) fn snapshot(&mut self) -> Result<Rasterizer, String> {
        match self {
            Self::Software(raster) => Ok(raster.clone()),
            #[cfg(any(
                feature = "dx12",
                feature = "vulkan",
                feature = "opengl",
                feature = "metal"
            ))]
            Self::Hardware(gpu) => gpu.snapshot(),
        }
    }

    pub(super) fn synchronize(&mut self) -> Result<(), String> {
        match self {
            Self::Software(_) => Ok(()),
            #[cfg(any(
                feature = "dx12",
                feature = "vulkan",
                feature = "opengl",
                feature = "metal"
            ))]
            Self::Hardware(gpu) => gpu.synchronize(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_preference_uses_the_native_platform_backend_first() {
        let modes = hardware_preference();
        if cfg!(target_os = "windows") {
            assert_eq!(
                modes,
                [
                    RendererMode::Dx12,
                    RendererMode::Vulkan,
                    RendererMode::OpenGl
                ]
            );
        } else if cfg!(target_os = "macos") {
            assert_eq!(
                modes,
                [
                    RendererMode::Metal,
                    RendererMode::Vulkan,
                    RendererMode::OpenGl
                ]
            );
        } else {
            assert_eq!(modes, [RendererMode::Vulkan, RendererMode::OpenGl]);
        }
    }

    #[test]
    fn fallback_isolates_failed_device_callbacks_and_keeps_the_successful_latch() {
        let mut failed_device = None;
        let mut selected_device = None;
        let candidates = [
            RendererMode::Dx12,
            RendererMode::Vulkan,
            RendererMode::OpenGl,
        ];
        let (selected, error) = select_hardware(&candidates, |mode, latch| match mode {
            RendererMode::Dx12 => {
                failed_device = Some(Arc::clone(latch));
                Err(io::Error::other("injected device creation failure"))
            }
            RendererMode::Vulkan => {
                selected_device = Some(Arc::clone(latch));
                Ok(mode)
            }
            _ => panic!("A successful backend must end the search"),
        })
        .unwrap();
        assert_eq!(selected, RendererMode::Vulkan);
        failed_device
            .unwrap()
            .set("late callback from failed device".into())
            .unwrap();
        assert!(error.get().is_none());
        selected_device
            .unwrap()
            .set("selected device was lost".into())
            .unwrap();
        assert_eq!(
            error.get().map(String::as_str),
            Some("selected device was lost")
        );
    }

    #[test]
    fn initialization_error_callback_prevents_selecting_a_broken_device() {
        let (mode, error) = select_hardware(
            &[RendererMode::Vulkan, RendererMode::OpenGl],
            |mode, error| {
                if mode == RendererMode::Vulkan {
                    error.set("shader initialization failed".into()).unwrap();
                }
                Ok(mode)
            },
        )
        .unwrap();
        assert_eq!(mode, RendererMode::OpenGl);
        assert!(error.get().is_none());
    }

    #[test]
    fn an_explicit_backend_failure_does_not_select_a_different_backend() {
        let mut tried = Vec::new();
        let result = select_hardware(&[RendererMode::Dx12], |mode, _| {
            tried.push(mode);
            Err::<(), _>(io::Error::other("driver unavailable"))
        });
        assert_eq!(tried, [RendererMode::Dx12]);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("driver unavailable")
        );
    }

    #[test]
    fn adapter_fallback_keeps_the_api_and_publishes_only_the_winning_device_latch() {
        let mut failed_adapter = None;
        let mut working_adapter = None;
        let (selected, error) = select_hardware(
            &[RendererMode::Dx12, RendererMode::Vulkan],
            |mode, backend_error| {
                assert_eq!(mode, RendererMode::Dx12, "Try all GPUs before changing API");
                let (adapter, selected_error) = select_candidates(
                    [("Discrete GPU".into(), 0), ("Integrated GPU".into(), 1)],
                    |adapter, adapter_error| {
                        if adapter == 0 {
                            failed_adapter = Some(Arc::clone(adapter_error));
                            return Err(io::Error::other("injected allocation failure"));
                        }
                        working_adapter = Some(Arc::clone(adapter_error));
                        Ok(adapter)
                    },
                )?;
                *backend_error = selected_error;
                Ok((mode, adapter))
            },
        )
        .unwrap();
        assert_eq!(selected, (RendererMode::Dx12, 1));
        failed_adapter
            .unwrap()
            .set("late callback from rejected discrete GPU".into())
            .unwrap();
        assert!(error.get().is_none());
        let working_adapter = working_adapter.unwrap();
        assert!(Arc::ptr_eq(&working_adapter, &error));
        working_adapter.set("selected GPU lost".into()).unwrap();
        assert_eq!(error.get().map(String::as_str), Some("selected GPU lost"));
    }
}
