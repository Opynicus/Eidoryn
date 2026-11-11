use super::{Desktop, components::button};
use eidoryn_psx::{
    systems::ps1::runner::Snapshot,
    video_filter::{CrtFilter, OverlayKey},
};
use gpui::{
    Bounds, Context, Corners, IntoElement, RenderImage, canvas, point, prelude::*, px, size,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

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
    // RenderImage expects BGRA bytes, even though image::Frame uses an RGBA container.
    let mut pixels = Vec::with_capacity(width * height * 4);
    for rgb in snapshot.rgb.as_chunks::<3>().0 {
        pixels.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
    }
    let buffer = image::RgbaImage::from_raw(width as u32, height as u32, pixels)
        .expect("Snapshot RGB dimensions match its framebuffer");
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

impl Desktop {
    pub(super) fn toggle_crt(&mut self, cx: &mut Context<Self>) {
        self.crt.filter = self.crt.filter.next();
        cx.notify();
    }

    pub(super) fn crt_filter(&self) -> CrtFilter {
        self.crt.filter
    }

    pub(super) fn set_crt_filter(&mut self, filter: CrtFilter, cx: &mut Context<Self>) {
        self.crt.filter = filter;
        cx.notify();
    }

    pub(super) fn crt_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        button(
            &self.themes.active().palette,
            "crt-filter",
            format!("CRT  {}", crt_label(self.crt.filter)),
            self.crt.filter != CrtFilter::Off,
        )
        .on_click(cx.listener(|view, _, _, cx| view.toggle_crt(cx)))
    }

    pub(super) fn video(&self) -> impl IntoElement {
        let image = Arc::clone(&self.image);
        let filter = self.crt.filter;
        let cache = Rc::clone(&self.crt.overlay);
        let lines = self.snapshot.dimensions.1;
        // PS1 modes have different pixel aspect ratios, but all target a 4:3 TV.
        // Stretching by the raw 256/320/512/640-wide framebuffer ratio distorts games.
        let aspect = 4.0 / 3.0;
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let width = bounds.size.width.min(bounds.size.height * aspect);
                let height = width / aspect;
                let origin = bounds.origin
                    + point(
                        (bounds.size.width - width) / 2.0,
                        (bounds.size.height - height) / 2.0,
                    );
                let bounds = Bounds::new(origin, size(width, height));
                let corners = Corners::all(if filter == CrtFilter::Off {
                    px(0.0)
                } else {
                    height * 0.035
                });
                if let Err(error) = window.paint_image(bounds, corners, image, 0, false) {
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
                    && let Err(error) =
                        window.paint_image(bounds, corners, Arc::clone(&overlay.image), 0, false)
                {
                    eprintln!("GPUI CRT overlay: {error}");
                }
            },
        )
        .size_full()
    }
}

pub(super) fn crt_label(filter: CrtFilter) -> &'static str {
    match filter {
        CrtFilter::Off => "关闭",
        CrtFilter::Soft => "柔和",
        CrtFilter::Classic => "经典",
    }
}
