//! One ordered GPU worker overlaps software/hardware drawing with CPU interpretation.
//! Bounded batches preserve texture feedback, blending and transfer ordering.
use super::{
    AntiAliasing, ColorDepth, DisplayFrame, DisplaySettings, RenderScale,
    rasterizer::{RasterCommand, Rasterizer},
    renderer_engine::Engine,
};
use std::{
    cell::RefCell,
    io,
    str::FromStr,
    sync::{Arc, OnceLock, mpsc},
    thread,
};

const BATCH_SIZE: usize = 128;
const QUEUED_BATCHES: usize = 2;

/// An ordered scanout command captures pixels before subsequent guest draws.
/// CPU execution can continue; only a consumer of this field waits for pixels.
pub(super) struct FrameReadback {
    native_dimensions: (usize, usize),
    dimensions: (usize, usize),
    ready: OnceLock<DisplayFrame>,
    pending: RefCell<Option<mpsc::Receiver<Arc<[u8]>>>>,
}

impl FrameReadback {
    pub(super) fn get(&self) -> &DisplayFrame {
        self.ready.get_or_init(|| DisplayFrame {
            native_dimensions: self.native_dimensions,
            dimensions: self.dimensions,
            rgb: self
                .pending
                .borrow_mut()
                .take()
                .expect("Unresolved scanout has a reply channel")
                .recv()
                .expect("GPU worker failed while capturing scanout"),
        })
    }
}

impl Clone for FrameReadback {
    fn clone(&self) -> Self {
        self.get().clone().into()
    }
}

impl From<DisplayFrame> for FrameReadback {
    fn from(frame: DisplayFrame) -> Self {
        Self {
            native_dimensions: frame.native_dimensions,
            dimensions: frame.dimensions,
            ready: OnceLock::from(frame),
            pending: RefCell::new(None),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RendererMode {
    #[default]
    Serial,
    Threaded,
    Hardware,
    Dx12,
    Vulkan,
    OpenGl,
    Metal,
}

impl RendererMode {
    /// Stable identifier used by CLI arguments and saved preferences.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Serial => "serial",
            Self::Threaded => "threaded",
            Self::Hardware => "hardware",
            Self::Dx12 => "dx12",
            Self::Vulkan => "vulkan",
            Self::OpenGl => "opengl",
            Self::Metal => "metal",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Serial => "Serial",
            Self::Threaded => "Threaded",
            Self::Hardware => "Hardware (automatic)",
            Self::Dx12 => "DirectX 12",
            Self::Vulkan => "Vulkan",
            Self::OpenGl => "OpenGL",
            Self::Metal => "Metal",
        }
    }

    pub const fn is_hardware(self) -> bool {
        !matches!(self, Self::Serial | Self::Threaded)
    }
}

impl FromStr for RendererMode {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "serial" => Ok(Self::Serial),
            "threaded" => Ok(Self::Threaded),
            "hardware" => Ok(Self::Hardware),
            "dx12" => Ok(Self::Dx12),
            "vulkan" => Ok(Self::Vulkan),
            "opengl" => Ok(Self::OpenGl),
            "metal" => Ok(Self::Metal),
            _ => {
                Err("--renderer expects serial, threaded, hardware, dx12, vulkan, opengl, or metal")
            }
        }
    }
}

pub(super) struct Renderer {
    backend: RefCell<Backend>,
    inherited_error: Option<String>,
    pub(super) preferred_anti_aliasing: AntiAliasing,
}

enum Backend {
    Serial(Rasterizer),
    Threaded(Worker),
}

impl Default for Renderer {
    fn default() -> Self {
        Self::from_raster(Rasterizer::default())
    }
}

impl Clone for Renderer {
    // Machine checkpoints contain independent data, never shared GPU workers.
    // The session restores its selected execution mode after reset/loading.
    fn clone(&self) -> Self {
        let raster = self.snapshot();
        Self {
            inherited_error: self.error(),
            ..Self::from_raster(raster)
        }
    }
}

impl Renderer {
    pub(super) fn from_raster(raster: Rasterizer) -> Self {
        Self {
            backend: RefCell::new(Backend::Serial(raster)),
            inherited_error: None,
            preferred_anti_aliasing: AntiAliasing::Off,
        }
    }

    pub(super) fn rewind_heap_bytes(&self) -> usize {
        let Backend::Serial(raster) = &*self.backend.borrow() else {
            unreachable!("rewind snapshots contain only serial renderers");
        };
        raster.vram.capacity() * size_of::<u16>()
            + self.inherited_error.as_ref().map_or(0, String::capacity)
    }

    pub(super) fn mode(&self) -> RendererMode {
        match &*self.backend.borrow() {
            Backend::Serial(_) => RendererMode::Serial,
            Backend::Threaded(worker) => worker.mode,
        }
    }

    pub(super) fn device_name(&self) -> String {
        match &*self.backend.borrow() {
            Backend::Serial(_) => "CPU software rasterizer".into(),
            Backend::Threaded(worker) => worker.device_name.clone(),
        }
    }

    pub(super) fn error(&self) -> Option<String> {
        if self.inherited_error.is_some() {
            return self.inherited_error.clone();
        }
        match &*self.backend.borrow() {
            Backend::Serial(_) => None,
            Backend::Threaded(worker) => worker.error.get().cloned(),
        }
    }

    pub(super) fn scale(&self) -> RenderScale {
        match &*self.backend.borrow() {
            Backend::Serial(_) => RenderScale::Native,
            Backend::Threaded(worker) => worker.scale,
        }
    }

    pub(super) fn take_presentation_warning(&self) -> Option<String> {
        match &*self.backend.borrow() {
            Backend::Serial(_) => None,
            Backend::Threaded(worker) => worker.warnings.try_recv().ok(),
        }
    }

    pub(super) fn color_depth(&self) -> ColorDepth {
        match &*self.backend.borrow() {
            Backend::Serial(_) => ColorDepth::Native,
            Backend::Threaded(worker) => worker.color_depth,
        }
    }

    pub(super) fn set_mode(
        &mut self,
        mode: RendererMode,
        scale: RenderScale,
        color_depth: ColorDepth,
    ) -> io::Result<()> {
        let (scale, color_depth) = if mode.is_hardware() {
            (scale, color_depth)
        } else {
            (RenderScale::Native, ColorDepth::Native)
        };
        if let Some(error) = self.error() {
            return Err(io::Error::other(error));
        }
        if self.mode() == mode && self.scale() == scale && self.color_depth() == color_depth {
            return Ok(());
        }
        // Initialize before touching the old session. A missing device, shader
        // error or failed initial upload must leave its VRAM/mode intact.
        let worker = if mode == RendererMode::Serial {
            None
        } else {
            Some(Worker::spawn(mode, scale, color_depth)?)
        };
        let raster = self.snapshot();
        if let Some(error) = self.error() {
            return Err(io::Error::other(error));
        }
        *self.backend.get_mut() = if let Some(mut worker) = worker {
            worker
                .request(|reply| Message::Start(raster, reply))
                .map_err(io::Error::other)?;
            Backend::Threaded(worker)
        } else {
            Backend::Serial(raster)
        };
        Ok(())
    }

    pub(super) fn submit(&mut self, command: RasterCommand) {
        match self.backend.get_mut() {
            Backend::Serial(raster) => raster.execute(command),
            Backend::Threaded(worker) => {
                worker.pending.push(command);
                if worker.pending.len() == BATCH_SIZE {
                    worker.flush();
                }
            }
        }
    }

    pub(super) fn read(&self, indices: [Option<usize>; 2]) -> [u32; 2] {
        match &mut *self.backend.borrow_mut() {
            Backend::Serial(raster) => indices.map(|i| i.map_or(0, |i| u32::from(raster.vram[i]))),
            Backend::Threaded(worker) => worker.request(|reply| Message::Read(indices, reply)),
        }
    }

    pub(super) fn display(&self, display: DisplaySettings) -> Vec<u8> {
        match &mut *self.backend.borrow_mut() {
            Backend::Serial(raster) => raster.display_rgb(display),
            Backend::Threaded(worker) => worker.request(|reply| Message::Display(display, reply)),
        }
    }

    pub(super) fn capture(&self, display: DisplaySettings) -> FrameReadback {
        match &mut *self.backend.borrow_mut() {
            Backend::Serial(raster) => DisplayFrame {
                native_dimensions: display.dimensions,
                dimensions: display.dimensions,
                rgb: raster.display_rgb(display).into(),
            }
            .into(),
            Backend::Threaded(worker) => FrameReadback {
                native_dimensions: display.dimensions,
                dimensions: display.output_dimensions(),
                ready: OnceLock::new(),
                pending: RefCell::new(Some(
                    worker.request_async(|reply| Message::Capture(display, reply)),
                )),
            },
        }
    }

    pub(super) fn snapshot(&self) -> Rasterizer {
        match &mut *self.backend.borrow_mut() {
            Backend::Serial(raster) => raster.clone(),
            Backend::Threaded(worker) => worker.request(Message::Snapshot),
        }
    }

    pub(super) fn synchronize(&self) {
        if let Backend::Threaded(worker) = &mut *self.backend.borrow_mut() {
            worker.request(Message::Fence);
        }
    }

    pub(super) fn dispatch_pending(&mut self) {
        if let Backend::Threaded(worker) = self.backend.get_mut() {
            worker.flush();
        }
    }

    pub(super) fn vram_mut(&mut self) -> &mut [u16] {
        self.set_mode(
            RendererMode::Serial,
            RenderScale::Native,
            ColorDepth::Native,
        )
        .expect("Cannot edit VRAM after a renderer failure");
        match self.backend.get_mut() {
            Backend::Serial(raster) => &mut raster.vram,
            Backend::Threaded(_) => unreachable!("renderer was stopped"),
        }
    }
}

enum Message {
    Start(Rasterizer, mpsc::SyncSender<Result<(), String>>),
    Batch(Vec<RasterCommand>),
    Read([Option<usize>; 2], mpsc::SyncSender<[u32; 2]>),
    Display(DisplaySettings, mpsc::SyncSender<Vec<u8>>),
    Capture(DisplaySettings, mpsc::SyncSender<Arc<[u8]>>),
    Snapshot(mpsc::SyncSender<Rasterizer>),
    Fence(mpsc::SyncSender<()>),
    Shutdown,
}

struct Worker {
    scale: RenderScale,
    color_depth: ColorDepth,
    mode: RendererMode,
    device_name: String,
    error: Arc<OnceLock<String>>,
    warnings: mpsc::Receiver<String>,
    sender: mpsc::SyncSender<Message>,
    pending: Vec<RasterCommand>,
    recycled: mpsc::Receiver<Vec<RasterCommand>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Worker {
    fn spawn(mode: RendererMode, scale: RenderScale, color_depth: ColorDepth) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(QUEUED_BATCHES);
        let (recycle, recycled) = mpsc::channel();
        let (ready, initialized) = mpsc::sync_channel(1);
        let (warning, warnings) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("eidoryn-gpu".into())
            .spawn(move || {
                let (mut engine, status) = match Engine::new(mode, scale, color_depth) {
                    Ok(initialized) => initialized,
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return;
                    }
                };
                if ready
                    .send(Ok((engine.name(), Arc::clone(&status))))
                    .is_err()
                {
                    return;
                }
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Start(raster, reply) => {
                            let result = engine.start(raster);
                            if let Err(error) = &result {
                                let _ = status.set(error.clone());
                            }
                            let _ = reply.send(result);
                        }
                        Message::Batch(mut batch) => {
                            for command in batch.drain(..) {
                                if status.get().is_none() {
                                    resolve(engine.execute(command), &status, || ());
                                }
                            }
                            let _ = recycle.send(batch);
                        }
                        Message::Read(indices, reply) => {
                            let _ = reply.send(resolve(engine.read(indices), &status, || [0; 2]));
                        }
                        Message::Display(display, reply) => {
                            let _ = reply.send(resolve(engine.display(display), &status, || {
                                let (width, height) = display.output_dimensions();
                                vec![0; width * height * 3]
                            }));
                        }
                        Message::Capture(display, reply) => {
                            let rgb = resolve(engine.capture(display), &status, || {
                                let (width, height) = display.output_dimensions();
                                std::iter::repeat_n(0, width * height * 3).collect()
                            });
                            if let Some(message) = engine.take_presentation_warning() {
                                let _ = warning.try_send(message);
                            }
                            let _ = reply.send(rgb);
                        }
                        Message::Snapshot(reply) => {
                            let _ = reply.send(resolve(
                                engine.snapshot(),
                                &status,
                                Rasterizer::default,
                            ));
                        }
                        Message::Fence(reply) => {
                            resolve(engine.synchronize(), &status, || ());
                            let _ = reply.send(());
                        }
                        Message::Shutdown => {
                            resolve(engine.synchronize(), &status, || ());
                            return;
                        }
                    }
                }
            })?;
        let (device_name, error) = match initialized.recv() {
            Ok(Ok(initialized)) => initialized,
            result => {
                let _ = thread.join();
                return Err(io::Error::other(match result {
                    Ok(Err(error)) => error,
                    _ => "GPU initialization worker stopped".into(),
                }));
            }
        };
        Ok(Self {
            scale,
            color_depth,
            mode,
            device_name,
            error,
            warnings,
            sender,
            pending: Vec::with_capacity(BATCH_SIZE),
            recycled,
            thread: Some(thread),
        })
    }

    fn send(&self, message: Message) {
        self.sender
            .send(message)
            .expect("GPU worker stopped unexpectedly");
    }

    fn flush(&mut self) {
        if !self.pending.is_empty() {
            let empty = self
                .recycled
                .try_recv()
                .unwrap_or_else(|_| Vec::with_capacity(BATCH_SIZE));
            let batch = std::mem::replace(&mut self.pending, empty);
            self.send(Message::Batch(batch));
        }
    }

    fn request_async<T>(
        &mut self,
        message: impl FnOnce(mpsc::SyncSender<T>) -> Message,
    ) -> mpsc::Receiver<T> {
        self.flush();
        let (reply, receiver) = mpsc::sync_channel(1);
        self.send(message(reply));
        receiver
    }

    fn request<T>(&mut self, message: impl FnOnce(mpsc::SyncSender<T>) -> Message) -> T {
        self.request_async(message)
            .recv()
            .expect("GPU worker failed before its synchronization barrier")
    }
}

fn resolve<T>(
    result: Result<T, String>,
    error: &OnceLock<String>,
    fallback: impl FnOnce() -> T,
) -> T {
    match result {
        Ok(value) => value,
        Err(message) => {
            let _ = error.set(message);
            fallback()
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            // No readback is needed for a discarded machine, but accepted draw
            // commands are drained in order before reclaiming the worker.
            if !self.pending.is_empty() {
                let _ = self
                    .sender
                    .send(Message::Batch(std::mem::take(&mut self.pending)));
            }
            let _ = self.sender.send(Message::Shutdown);
            if thread.join().is_err() {
                eprintln!("Eidoryn GPU worker failed during shutdown");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EmuError, machine::Machine};

    #[test]
    fn completed_field_keeps_the_render_workers_pixel_allocation() {
        let pixels: Arc<[u8]> = Arc::from([255, 0, 0, 0, 0, 127]);
        let (send, receive) = mpsc::sync_channel(1);
        let field = FrameReadback {
            native_dimensions: (2, 1),
            dimensions: (2, 1),
            ready: OnceLock::new(),
            pending: RefCell::new(Some(receive)),
        };
        send.send(Arc::clone(&pixels)).unwrap();
        assert!(Arc::ptr_eq(&field.get().rgb, &pixels));
        assert!(Arc::ptr_eq(&field.clone().get().rgb, &pixels));
    }

    #[test]
    fn asynchronous_failure_halts_execution_survives_clone_and_drains_replies() {
        let mut machine = Machine::demo();
        machine
            .bus
            .gpu
            .set_renderer(RendererMode::Threaded)
            .unwrap();
        {
            let Backend::Threaded(worker) = &*machine.bus.gpu.renderer.backend.borrow() else {
                unreachable!()
            };
            worker.error.set("injected device failure".into()).unwrap();
        }
        // Cloning a failed backend must not turn an invalid readback into an
        // apparently healthy machine. Pending RPCs and shutdown still drain.
        let mut copy = machine.clone();
        let expected = EmuError::GpuBackend("injected device failure".into());
        assert_eq!(copy.run_slice(), Err(expected.clone()));
        assert_eq!(machine.run_slice(), Err(expected));
        assert!(machine.bus.gpu.set_renderer(RendererMode::Serial).is_err());
        machine.bus.gpu.synchronize_renderer();
        assert!(machine.bus.gpu.renderer_error().is_some());
    }
}
