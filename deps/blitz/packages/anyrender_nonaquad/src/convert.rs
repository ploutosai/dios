//! Conversion utilities between anyrender/kurbo/peniko types and nona types.

use kurbo::{Affine, PathEl, Shape};
use nona::{Color as NonaColor, Rect as NonaRect, Transform as NonaTransform};
use peniko::Color;

/// Convert a peniko/color AlphaColor<Srgb> to a nona Color.
pub fn peniko_color_to_nona(color: Color) -> NonaColor {
    let [r, g, b, a] = color.components;
    NonaColor::rgba(r, g, b, a)
}

/// Convert a kurbo Affine to a nona Transform.
pub fn affine_to_nona(affine: Affine) -> NonaTransform {
    let c = affine.as_coeffs();
    NonaTransform([
        c[0] as f32,
        c[1] as f32,
        c[2] as f32,
        c[3] as f32,
        c[4] as f32,
        c[5] as f32,
    ])
}

/// Build nona path commands from a kurbo Shape on a nona Context.
pub fn build_nona_path(ctx: &mut nona::Context, shape: &impl Shape) {
    ctx.begin_path();
    for el in shape.path_elements(0.25) {
        match el {
            PathEl::MoveTo(p) => ctx.move_to((p.x as f32, p.y as f32)),
            PathEl::LineTo(p) => ctx.line_to((p.x as f32, p.y as f32)),
            PathEl::QuadTo(c, p) => ctx.quad_to((c.x as f32, c.y as f32), (p.x as f32, p.y as f32)),
            PathEl::CurveTo(c1, c2, p) => ctx.bezier_to(
                (c1.x as f32, c1.y as f32),
                (c2.x as f32, c2.y as f32),
                (p.x as f32, p.y as f32),
            ),
            PathEl::ClosePath => ctx.close_path(),
        }
    }
}

/// Convert a kurbo Rect to a nona Rect.
pub fn kurbo_rect_to_nona(rect: kurbo::Rect) -> NonaRect {
    NonaRect::new(
        (rect.x0 as f32, rect.y0 as f32).into(),
        ((rect.x1 - rect.x0) as f32, (rect.y1 - rect.y0) as f32).into(),
    )
}
