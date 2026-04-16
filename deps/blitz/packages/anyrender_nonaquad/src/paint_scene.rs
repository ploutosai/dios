//! Implementation of `anyrender::PaintScene` using nona + nonaquad.

use anyrender::{CustomPaint, Glyph, NormalizedCoord, Paint, PaintRef, PaintScene};
use kurbo::{Affine, Rect, Shape, Stroke};
use nona::{
    Color as NonaColor, Context as NonaContext, Gradient as NonaGradient, ImagePattern,
    LineCap as NonaLineCap, LineJoin as NonaLineJoin,
};
use peniko::{BlendMode, Color, Fill, FontData, StyleRef};
use std::collections::HashMap;

use crate::convert::{affine_to_nona, build_nona_path, kurbo_rect_to_nona, peniko_color_to_nona};
use crate::custom_paint::get_custom_paint_source;

fn transform_rect_bbox(transform: Affine, rect: Rect) -> Rect {
    let p0 = transform * rect.origin();
    let p1 = transform * kurbo::Point::new(rect.x1, rect.y0);
    let p2 = transform * kurbo::Point::new(rect.x0, rect.y1);
    let p3 = transform * kurbo::Point::new(rect.x1, rect.y1);

    let min_x = p0.x.min(p1.x).min(p2.x).min(p3.x);
    let min_y = p0.y.min(p1.y).min(p2.y).min(p3.y);
    let max_x = p0.x.max(p1.x).max(p2.x).max(p3.x);
    let max_y = p0.y.max(p1.y).max(p2.y).max(p3.y);

    Rect::new(min_x, min_y, max_x, max_y)
}

/// A PaintScene implementation backed by nona's 2D drawing context.
///
/// This struct borrows a nona Context and a nona Renderer (via nonaquad)
/// and translates anyrender drawing commands into nona drawing calls.
///
/// The caller is responsible for managing the nona frame lifecycle
/// (begin_frame/end_frame) around paint scene usage.
pub struct NonaquadScenePainter<'a, R: nona::Renderer> {
    pub ctx: &'a mut NonaContext,
    pub renderer: &'a mut R,
    layer_depth: u32,
    /// Maps font data pointer (address of the Arc'd data) to nona FontId.
    /// This allows us to register fonts lazily on first use.
    font_registry: &'a mut HashMap<usize, nona::FontId>,
    custom_images: &'a mut HashMap<u64, CustomImageState>,
}

pub struct CustomImageState {
    pub image_id: nona::ImageId,
    pub width: u32,
    pub height: u32,
}

impl<'a, R: nona::Renderer> NonaquadScenePainter<'a, R> {
    pub fn new(
        ctx: &'a mut NonaContext,
        renderer: &'a mut R,
        font_registry: &'a mut HashMap<usize, nona::FontId>,
        custom_images: &'a mut HashMap<u64, CustomImageState>,
    ) -> Self {
        // Disable shape anti-aliasing to avoid visible seams between adjacent fills
        ctx.shape_antialias(false);
        Self {
            ctx,
            renderer,
            layer_depth: 0,
            font_registry,
            custom_images,
        }
    }

    fn custom_paint_to_nona(&mut self, custom: &CustomPaint) -> Option<(nona::Paint, Rect)> {
        let source = get_custom_paint_source(custom.source_id)?;
        let frame = source.frame(custom.width, custom.height, custom.scale)?;
        if frame.width == 0 || frame.height == 0 {
            return None;
        }

        let box_width = custom.width as f64;
        let box_height = custom.height as f64;
        let frame_width = frame.width as f64;
        let frame_height = frame.height as f64;
        let scale = (box_width / frame_width).min(box_height / frame_height);
        let draw_width = (frame_width * scale) as f32;
        let draw_height = (frame_height * scale) as f32;
        let offset_x = ((box_width - draw_width as f64) * 0.5) as f32;
        let offset_y = ((box_height - draw_height as f64) * 0.5) as f32;
        let draw_rect = Rect::from_origin_size(
            (offset_x as f64, offset_y as f64),
            (draw_width as f64, draw_height as f64),
        );

        let state = match self.custom_images.get_mut(&custom.source_id) {
            Some(state) if state.width == frame.width && state.height == frame.height => {
                let _ = self.renderer.update_texture(
                    state.image_id,
                    0,
                    0,
                    frame.width as usize,
                    frame.height as usize,
                    &frame.rgba,
                );
                state
            }
            Some(state) => {
                let _ = self.renderer.delete_texture(state.image_id);
                let image_id = self
                    .renderer
                    .create_texture(
                        nona::renderer::TextureType::RGBA,
                        frame.width as usize,
                        frame.height as usize,
                        nona::ImageFlags::empty(),
                        Some(&frame.rgba),
                    )
                    .ok()?;
                *state = CustomImageState {
                    image_id,
                    width: frame.width,
                    height: frame.height,
                };
                state
            }
            None => {
                let image_id = self
                    .renderer
                    .create_texture(
                        nona::renderer::TextureType::RGBA,
                        frame.width as usize,
                        frame.height as usize,
                        nona::ImageFlags::empty(),
                        Some(&frame.rgba),
                    )
                    .ok()?;
                self.custom_images.insert(
                    custom.source_id,
                    CustomImageState {
                        image_id,
                        width: frame.width,
                        height: frame.height,
                    },
                );
                self.custom_images.get_mut(&custom.source_id).unwrap()
            }
        };

        Some((
            ImagePattern {
                // Nona's image shader computes texture coordinates as
                // `(inverse(xform) * fpos) / extent`, so the sampling extent
                // needs to match the target canvas box, not the source frame
                // dimensions. Otherwise smaller canvases only sample the
                // texture's top-left corner.
                center: nona::Point::new(offset_x, offset_y),
                size: nona::Extent::new(draw_width, draw_height),
                angle: 0.0,
                img: state.image_id,
                alpha: 1.0,
            }
            .into(),
            draw_rect,
        ))
    }

    /// Get or register a font in nona, returning its FontId.
    fn get_or_register_font(&mut self, font_data: &FontData) -> Option<nona::FontId> {
        let data_ptr = font_data.data.as_ref().as_ptr() as usize;
        if let Some(&id) = self.font_registry.get(&data_ptr) {
            return Some(id);
        }
        // Register new font
        let name = format!("font_{:x}", data_ptr);
        match self
            .ctx
            .create_font(&name, font_data.data.as_ref().to_vec())
        {
            Ok(id) => {
                self.font_registry.insert(data_ptr, id);
                Some(id)
            }
            Err(_) => None,
        }
    }

    /// Convert an anyrender PaintRef to a nona Paint and set it as fill paint on the context.
    fn set_fill_paint(&mut self, brush: PaintRef<'_>, transform: Affine) {
        let paint = self.convert_paint(brush, transform);
        self.ctx.fill_paint(paint);
    }

    /// Convert an anyrender PaintRef to a nona Paint and set it as stroke paint on the context.
    fn set_stroke_paint(&mut self, brush: PaintRef<'_>, transform: Affine) {
        let paint = self.convert_paint(brush, transform);
        self.ctx.stroke_paint(paint);
    }

    /// Convert an anyrender PaintRef into a nona Paint.
    fn convert_paint(&mut self, brush: PaintRef<'_>, _transform: Affine) -> nona::Paint {
        match brush {
            Paint::Solid(color) => peniko_color_to_nona(color).into(),
            Paint::Gradient(gradient) => self.convert_gradient(gradient),
            Paint::Image(_image_brush) => {
                // TODO: implement image brush support (requires uploading the image to nona)
                // For now, fall back to a transparent fill
                NonaColor::rgba(0.0, 0.0, 0.0, 0.0).into()
            }
            Paint::Custom(custom) => custom
                .downcast_ref::<CustomPaint>()
                .and_then(|custom| self.custom_paint_to_nona(custom).map(|(paint, _)| paint))
                .unwrap_or_else(|| NonaColor::rgba(0.0, 0.0, 0.0, 0.0).into()),
        }
    }

    /// Convert a peniko Gradient into a nona Paint (via nona::Gradient).
    fn convert_gradient(&self, gradient: &peniko::Gradient) -> nona::Paint {
        // Get first and last color stops as start/end colors
        let stops = &gradient.stops.0;
        if stops.is_empty() {
            return NonaColor::rgba(0.0, 0.0, 0.0, 0.0).into();
        }

        let start_color = dynamic_color_to_nona(&stops.first().unwrap().color);
        let end_color = dynamic_color_to_nona(&stops.last().unwrap().color);

        match &gradient.kind {
            peniko::GradientKind::Linear(pos) => NonaGradient::Linear {
                start: (pos.start.x as f32, pos.start.y as f32).into(),
                end: (pos.end.x as f32, pos.end.y as f32).into(),
                start_color,
                end_color,
            }
            .into(),
            peniko::GradientKind::Radial(pos) => NonaGradient::Radial {
                center: (pos.end_center.x as f32, pos.end_center.y as f32).into(),
                in_radius: pos.start_radius,
                out_radius: pos.end_radius,
                inner_color: start_color,
                outer_color: end_color,
            }
            .into(),
            peniko::GradientKind::Sweep(_pos) => {
                // nona doesn't have sweep gradients, fall back to solid
                start_color.into()
            }
        }
    }
}

/// Convert a peniko DynamicColor to a nona Color.
fn dynamic_color_to_nona(color: &peniko::color::DynamicColor) -> NonaColor {
    // DynamicColor has components [c1, c2, c3, alpha] - convert to sRGB
    let components = color.components;
    // Assuming sRGB space (most common case)
    NonaColor::rgba(components[0], components[1], components[2], components[3])
}

impl<'a, R: nona::Renderer> PaintScene for NonaquadScenePainter<'a, R> {
    fn reset(&mut self) {
        // nona doesn't have a "reset scene" concept - the scene is drawn immediately.
        // Disable shape anti-aliasing to avoid visible seams between adjacent fills.
        self.ctx.shape_antialias(false);
    }

    fn push_layer(
        &mut self,
        _blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl Shape,
    ) {
        self.ctx.save();
        self.ctx.global_alpha(alpha);

        // Clip in transformed coordinates, but leave drawing transforms to each node.
        // `intersect_scissor` (rather than `scissor`) is essential: nested clip
        // layers must AND with the parent's clip, otherwise a child's local
        // clip (e.g. a box-shadow's shadow_clip path) overwrites the
        // ancestor's overflow:hidden scissor and the child's paint leaks
        // outside its scrolling container.
        let bbox = transform_rect_bbox(transform, clip.bounding_box());
        self.ctx.intersect_scissor(kurbo_rect_to_nona(bbox));

        self.layer_depth += 1;
    }

    fn push_clip_layer(&mut self, transform: Affine, clip: &impl Shape) {
        self.ctx.save();

        // See `push_layer` above — clip nesting must intersect, not replace.
        let bbox = transform_rect_bbox(transform, clip.bounding_box());
        self.ctx.intersect_scissor(kurbo_rect_to_nona(bbox));

        self.layer_depth += 1;
    }

    fn pop_layer(&mut self) {
        if self.layer_depth > 0 {
            self.ctx.restore();
            self.layer_depth -= 1;
        }
    }

    fn stroke<'b>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<PaintRef<'b>>,
        _brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.ctx.save();

        // Apply transform
        let t = affine_to_nona(transform);
        self.ctx.transform(t);

        // Set stroke style
        self.ctx.stroke_width(style.width as f32);

        match style.join {
            kurbo::Join::Bevel => self.ctx.line_join(NonaLineJoin::Bevel),
            kurbo::Join::Miter => self.ctx.line_join(NonaLineJoin::Miter),
            kurbo::Join::Round => self.ctx.line_join(NonaLineJoin::Round),
        }

        match style.start_cap {
            kurbo::Cap::Butt => self.ctx.line_cap(NonaLineCap::Butt),
            kurbo::Cap::Round => self.ctx.line_cap(NonaLineCap::Round),
            kurbo::Cap::Square => self.ctx.line_cap(NonaLineCap::Square),
        }

        self.ctx.miter_limit(style.miter_limit as f32);

        // Set paint
        let brush = brush.into();
        self.set_stroke_paint(brush, transform);

        // Build path and stroke
        build_nona_path(self.ctx, shape);
        let _ = self.ctx.stroke(self.renderer);

        self.ctx.restore();
    }

    fn fill<'b>(
        &mut self,
        _style: Fill,
        transform: Affine,
        brush: impl Into<PaintRef<'b>>,
        _brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.ctx.save();

        // Apply transform
        let t = affine_to_nona(transform);
        self.ctx.transform(t);

        // Set paint
        let brush = brush.into();
        if let Paint::Custom(custom) = brush {
            if let Some(custom) = custom.downcast_ref::<CustomPaint>() {
                if let Some((paint, draw_rect)) = self.custom_paint_to_nona(custom) {
                    self.ctx.fill_paint(paint);
                    build_nona_path(self.ctx, &draw_rect);
                    let _ = self.ctx.fill(self.renderer);
                    self.ctx.restore();
                    return;
                }
            }
        }
        self.set_fill_paint(brush, transform);

        // Build path and fill
        build_nona_path(self.ctx, shape);
        let _ = self.ctx.fill(self.renderer);

        self.ctx.restore();
    }

    fn draw_glyphs<'b, 's: 'b>(
        &'s mut self,
        font: &'b FontData,
        font_size: f32,
        _hint: bool,
        _normalized_coords: &'b [NormalizedCoord],
        _style: impl Into<StyleRef<'b>>,
        brush: impl Into<PaintRef<'b>>,
        _brush_alpha: f32,
        transform: Affine,
        _glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = Glyph>,
    ) {
        // Register font with nona if needed, then use atlas-based glyph rendering
        let font_id = match self.get_or_register_font(font) {
            Some(id) => id,
            None => return,
        };

        self.ctx.save();

        let t = affine_to_nona(transform);
        self.ctx.transform(t);

        let brush = brush.into();
        self.set_fill_paint(brush, transform);

        // Set up font for nona
        self.ctx.fontid(font_id);
        self.ctx.font_size(font_size);

        // Collect glyphs into GlyphPosition vec
        let glyph_positions: Vec<nona::GlyphPosition> = glyphs
            .map(|g| nona::GlyphPosition {
                glyph_id: g.id as u16,
                x: g.x,
                y: g.y,
            })
            .collect();

        if !glyph_positions.is_empty() {
            let _ = self.ctx.draw_glyphs_by_id(self.renderer, &glyph_positions);
        }

        self.ctx.restore();
    }

    fn draw_box_shadow(
        &mut self,
        transform: Affine,
        rect: Rect,
        brush: Color,
        radius: f64,
        std_dev: f64,
    ) {
        self.ctx.save();

        let t = affine_to_nona(transform);
        self.ctx.transform(t);

        // Use nona's box gradient for box shadows
        let nona_color = peniko_color_to_nona(brush);
        let transparent = NonaColor::rgba(nona_color.r, nona_color.g, nona_color.b, 0.0);

        let shadow_paint: nona::Paint = NonaGradient::Box {
            rect: kurbo_rect_to_nona(rect),
            radius: radius as f32,
            feather: std_dev as f32,
            inner_color: nona_color,
            outer_color: transparent,
        }
        .into();

        self.ctx.fill_paint(shadow_paint);

        // Draw a rect larger than the shadow bounds to cover the feathered region
        let expand = std_dev * 3.0;
        self.ctx.begin_path();
        self.ctx.rect(nona::Rect::new(
            ((rect.x0 - expand) as f32, (rect.y0 - expand) as f32).into(),
            (
                (rect.width() + expand * 2.0) as f32,
                (rect.height() + expand * 2.0) as f32,
            )
                .into(),
        ));

        let _ = self.ctx.fill(self.renderer);

        self.ctx.restore();
    }
}
