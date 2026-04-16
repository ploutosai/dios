//! Anyrender backend using nonaquad (NanoVG-style rendering on miniquad).
//!
//! This crate implements `anyrender::PaintScene` and `anyrender::WindowRenderer`
//! using the `nona` 2D drawing library and `nonaquad` miniquad GPU backend.

mod paint_scene;
mod window_renderer;
mod convert;
mod custom_paint;

pub use paint_scene::NonaquadScenePainter;
pub use window_renderer::NonaquadWindowRenderer;
pub use custom_paint::{
    CustomPaintSource, CustomPaintTexture, get_custom_paint_source, register_custom_paint_source,
    unregister_custom_paint_source,
};
