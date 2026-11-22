//! The app mark without its icon tile. Only the colored artwork rotates.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt as _, ElementId, IntoElement, ParentElement as _, Pixels, RenderImage,
    Styled as _, canvas, div, svg, white,
};
use resvg::tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

use crate::theme;

pub const SPIN_DURATION: Duration = Duration::from_millis(740);
const FRAME_COUNT: usize = 90;
const TEXTURE_SIZE: u32 = 96;
static ROTOR: LazyLock<Option<Arc<RenderImage>>> = LazyLock::new(|| {
    let source = include_bytes!("../assets/app/windmill.svg");
    let tree = resvg::usvg::Tree::from_data(source, &resvg::usvg::Options::default())
        .inspect_err(|error| eprintln!("cannot load the windmill artwork: {error}"))
        .ok()?;
    let mut base = Pixmap::new(TEXTURE_SIZE, TEXTURE_SIZE)?;
    let scale = TEXTURE_SIZE as f32 / tree.size().width();
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut base.as_mut(),
    );
    let center = TEXTURE_SIZE as f32 / 2.0;
    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };
    let frames = (0..FRAME_COUNT)
        .map(|index| {
            let mut frame = Pixmap::new(TEXTURE_SIZE, TEXTURE_SIZE)?;
            frame.draw_pixmap(
                0,
                0,
                base.as_ref(),
                &paint,
                Transform::from_rotate_at(
                    index as f32 * 360.0 / FRAME_COUNT as f32,
                    center,
                    center,
                ),
                None,
            );
            let mut pixels = frame.take();
            // Tiny-skia gives premultiplied RGBA, GPUI uploads straight BGRA.
            for pixel in pixels.chunks_exact_mut(4) {
                let alpha = u16::from(pixel[3]);
                if alpha != 0 {
                    for channel in &mut pixel[..3] {
                        *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha) as u8;
                    }
                    pixel.swap(0, 2);
                }
            }
            image::RgbaImage::from_raw(TEXTURE_SIZE, TEXTURE_SIZE, pixels).map(image::Frame::new)
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Arc::new(RenderImage::new(frames)))
});

pub fn mark(size: Pixels, spin: Option<ElementId>) -> impl IntoElement {
    let rotor = |progress: f32| {
        let index = (progress * FRAME_COUNT as f32).round() as usize % FRAME_COUNT;
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                if let Some(image) = ROTOR.as_ref() {
                    let _ = window.paint_image(
                        bounds,
                        bounds,
                        Default::default(),
                        image.clone(),
                        index,
                        false,
                    );
                }
            },
        )
        .absolute()
        .inset_0()
    };
    let layer = match spin {
        Some(id) => rotor(0.0)
            .with_animation(
                id,
                Animation::new(SPIN_DURATION).with_easing(theme::curve(theme::MOVE_CURVE)),
                move |_, progress| rotor(progress),
            )
            .into_any_element(),
        None => rotor(0.0).into_any_element(),
    };
    div().relative().flex_none().size(size).child(layer).child(
        svg()
            .path("app/windmill-terminal.svg")
            .text_color(white())
            .absolute()
            .inset_0(),
    )
}
