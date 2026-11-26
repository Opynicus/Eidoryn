use super::{Desktop, components::button};
use eidoryn_nds::{SCREEN_HEIGHT, SCREEN_WIDTH};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::shortcuts::ShortcutAction;
use eidoryn_psx::{
    systems::ps1::runner::Snapshot,
    video_filter::{CrtFilter, OverlayKey},
};
use gpui::{
    Bounds, Context, Corners, IntoElement, Pixels, RenderImage, Window, canvas, point, prelude::*,
    px, size,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub(super) struct VideoImage {
    pub(super) image: Arc<RenderImage>,
    pub(super) dirty: bool,
}

struct CachedOverlay {
    key: OverlayKey,
    image: Arc<RenderImage>,
}

pub(super) struct CrtState {
    filter: CrtFilter,
    // Only the UI thread accesses the cache; canvas closures share its lifetime.
    overlay: Rc<RefCell<Option<CachedOverlay>>>,
}

impl CrtState {
    pub(super) fn new(filter: CrtFilter) -> Self {
        Self {
            filter,
            overlay: Rc::default(),
        }
    }
}

pub(super) fn frame_image(snapshot: &Snapshot) -> Arc<RenderImage> {
    let (width, height) = snapshot.dimensions;
    rgb_image(&snapshot.rgb, width, height)
}

fn rgb_image(rgb: &[u8], width: usize, height: usize) -> Arc<RenderImage> {
    // RenderImage expects BGRA bytes, even though image::Frame uses an RGBA container.
    let pixels = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&[r, g, b]| [b, g, r, 255])
        .collect::<Vec<_>>()
        .into_flattened();
    let buffer = image::RgbaImage::from_raw(width as u32, height as u32, pixels)
        .expect("Snapshot RGB dimensions match its framebuffer");
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

impl Desktop {
    /// Convert only the latest visible frame. Debugger pages do not use video
    /// or CRT textures, so release those images until the display returns.
    pub(super) fn sync_video(&mut self, visible: bool, window: &mut Window) {
        if !visible
            || self.is_nds() && self.nds.snapshot.rgb.is_empty()
            || self.is_gba() && self.gba.snapshot.rgb.is_empty()
        {
            if let Some(old) = self.image.take() {
                let _ = window.drop_image(old.image);
            }
            if let Some(old) = self.crt.overlay.borrow_mut().take() {
                let _ = window.drop_image(old.image);
            }
            return;
        }
        if self.image.as_ref().is_some_and(|cached| !cached.dirty) {
            return;
        }
        let next = VideoImage {
            image: if self.is_nds() {
                rgb_image(&self.nds.snapshot.rgb, SCREEN_WIDTH, 2 * SCREEN_HEIGHT)
            } else if self.is_gba() {
                rgb_image(
                    &self.gba.snapshot.rgb,
                    eidoryn_gba::WIDTH,
                    eidoryn_gba::HEIGHT,
                )
            } else {
                frame_image(&self.snapshot)
            },
            dirty: false,
        };
        if let Some(old) = self.image.replace(next) {
            let _ = window.drop_image(old.image);
        }
    }

    pub(super) fn toggle_crt(&mut self, cx: &mut Context<Self>) {
        self.set_crt_filter(self.crt.filter.next(), cx);
    }

    pub(super) fn crt_filter(&self) -> CrtFilter {
        self.crt.filter
    }

    pub(super) fn set_crt_filter(&mut self, filter: CrtFilter, cx: &mut Context<Self>) {
        self.crt.filter = filter;
        filter.id().clone_into(&mut self.preferences.crt);
        self.save_preferences();
        cx.notify();
    }

    pub(super) fn crt_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        button(
            &self.fonts,
            &self.themes.active().palette,
            "crt-filter",
            self.shortcut_label(
                format!("CRT  {}", crt_label(self.language, self.crt.filter)),
                ShortcutAction::Crt,
                None,
            ),
            self.crt.filter != CrtFilter::Off,
        )
        .on_click(cx.listener(|view, _, _, cx| view.toggle_crt(cx)))
    }

    pub(super) fn video_aspect(&self) -> f32 {
        if self.is_nds() {
            SCREEN_WIDTH as f32 / (2 * SCREEN_HEIGHT) as f32
        } else if self.is_gba() {
            1.5
        } else {
            4.0 / 3.0
        }
    }

    pub(super) fn video(&self, cx: &Context<Self>) -> impl IntoElement {
        let image = self.image.as_ref().map(|cached| Arc::clone(&cached.image));
        let filter = self.crt.filter;
        let cache = Rc::clone(&self.crt.overlay);
        let aspect = self.video_aspect();
        let touchscreen = self.is_nds()
            && (self.page == super::WorkspacePage::Game || self.presentation.shows_video());
        let screen = touchscreen.then(|| Rc::clone(&self.nds.screen));
        let lines = if self.is_nds() {
            2 * SCREEN_HEIGHT
        } else if self.is_gba() {
            eidoryn_gba::HEIGHT
        } else {
            self.snapshot.native_dimensions.1
        };
        let canvas = canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let Some(image) = image else {
                    return;
                };
                let bounds = fit_video(bounds, aspect);
                if let Some(screen) = screen {
                    screen.set(Some(bounds));
                }
                let gpui::Size { width, height } = bounds.size;
                let corners = Corners::all(if filter == CrtFilter::Off {
                    px(0.0)
                } else {
                    height * 0.035
                });
                if let Err(error) = window.paint_image(bounds, bounds, corners, image, 0, false) {
                    eprintln!("GPUI frame: {error}");
                }
                let scale = window.scale_factor();
                let key = OverlayKey::new(
                    (f32::from(width) * scale).round() as u32,
                    (f32::from(height) * scale).round() as u32,
                    lines,
                    filter,
                );
                let mut cached = cache.borrow_mut();
                if cached.as_ref().map(|overlay| overlay.key) != key {
                    if let Some(old) = cached.take() {
                        let _ = window.drop_image(old.image);
                    }
                    if let Some(key) = key {
                        let (width, height) = key.dimensions();
                        let buffer = image::RgbaImage::from_raw(width, height, key.bgra())
                            .expect("CRT mask dimensions match its BGRA data");
                        *cached = Some(CachedOverlay {
                            key,
                            image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
                        });
                    }
                }
                if let Some(overlay) = cached.as_ref()
                    && let Err(error) = window.paint_image(
                        bounds,
                        bounds,
                        corners,
                        Arc::clone(&overlay.image),
                        0,
                        false,
                    )
                {
                    eprintln!("GPUI CRT overlay: {error}");
                }
            },
        )
        .size_full();
        if touchscreen {
            Self::nds_touchscreen(canvas, cx).into_any_element()
        } else {
            canvas.into_any_element()
        }
    }
}

/// Center the guest display and preserve its aspect ratio in logical pixels.
pub(super) fn fit_video(viewport: Bounds<Pixels>, aspect: f32) -> Bounds<Pixels> {
    let width = viewport.size.width.min(viewport.size.height * aspect);
    let height = width / aspect;
    let origin = viewport.origin
        + point(
            (viewport.size.width - width) / 2.0,
            (viewport.size.height - height) / 2.0,
        );
    Bounds::new(origin, size(width, height))
}

pub(super) fn crt_label(language: eidoryn_psx::i18n::Language, filter: CrtFilter) -> &'static str {
    match filter {
        CrtFilter::Off => language.text(Text::Off),
        CrtFilter::Soft => language.text(Text::Soft),
        CrtFilter::Classic => language.text(Text::Classic),
    }
}
