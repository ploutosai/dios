//! Minimal example: nona + nonaquad shapes directly on miniquad.
//! Tests anti-aliasing behavior, including glyph outlines via skrifa.

use miniquad::{self, conf::Conf, EventHandler, PassAction};
use nona::{Color, Context, Point, Rect, Renderer as _, Solidity};
use nonaquad::nvgimpl;

struct App {
    ctx: Box<dyn miniquad::RenderingBackend>,
    nona_renderer: nvgimpl::Renderer,
    nona_ctx: Context,
    font_data: Vec<u8>,
}

// Same glyph path extraction as in anyrender_nonaquad
enum PathCmd {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CurveTo(f32, f32, f32, f32, f32, f32),
    Close,
}

struct GlyphPen {
    commands: Vec<PathCmd>,
}

impl GlyphPen {
    fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }
}

impl skrifa::outline::OutlinePen for GlyphPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(PathCmd::MoveTo(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(PathCmd::LineTo(x, y));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.commands.push(PathCmd::QuadTo(cx0, cy0, x, y));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.commands
            .push(PathCmd::CurveTo(cx0, cy0, cx1, cy1, x, y));
    }
    fn close(&mut self) {
        self.commands.push(PathCmd::Close);
    }
}

fn compute_subpath_signed_area(commands: &[PathCmd]) -> f32 {
    let mut area: f32 = 0.0;
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    let (mut fx, mut fy) = (0.0f32, 0.0f32);
    for cmd in commands {
        match cmd {
            PathCmd::MoveTo(x, y) => {
                fx = *x;
                fy = *y;
                cx = *x;
                cy = *y;
            }
            PathCmd::LineTo(x, y) => {
                area += cx * *y - *x * cy;
                cx = *x;
                cy = *y;
            }
            PathCmd::QuadTo(_, _, x, y) | PathCmd::CurveTo(_, _, _, _, x, y) => {
                area += cx * *y - *x * cy;
                cx = *x;
                cy = *y;
            }
            PathCmd::Close => {
                area += cx * fy - fx * cy;
                cx = fx;
                cy = fy;
            }
        }
    }
    area * 0.5
}

fn draw_glyph_path(nc: &mut Context, rc: &mut impl nona::Renderer, commands: &[PathCmd]) {
    // Split into sub-paths
    let mut subpath_starts: Vec<usize> = vec![0];
    for (i, cmd) in commands.iter().enumerate() {
        if matches!(cmd, PathCmd::MoveTo(_, _)) && i > 0 {
            subpath_starts.push(i);
        }
    }

    nc.begin_path();
    for (sp_idx, &start) in subpath_starts.iter().enumerate() {
        let end = subpath_starts
            .get(sp_idx + 1)
            .copied()
            .unwrap_or(commands.len());
        let subpath = &commands[start..end];
        let area = compute_subpath_signed_area(subpath);

        for cmd in subpath {
            match cmd {
                PathCmd::MoveTo(x, y) => {
                    nc.move_to(Point::new(*x, *y));
                    if area < 0.0 {
                        nc.path_solidity(Solidity::Hole);
                    }
                }
                PathCmd::LineTo(x, y) => nc.line_to(Point::new(*x, *y)),
                PathCmd::QuadTo(cx, cy, x, y) => {
                    nc.quad_to(Point::new(*cx, *cy), Point::new(*x, *y))
                }
                PathCmd::CurveTo(c1x, c1y, c2x, c2y, x, y) => nc.bezier_to(
                    Point::new(*c1x, *c1y),
                    Point::new(*c2x, *c2y),
                    Point::new(*x, *y),
                ),
                PathCmd::Close => nc.close_path(),
            }
        }
    }
    let _ = nc.fill(rc);
}

impl App {
    fn new() -> Self {
        let mut ctx = miniquad::window::new_rendering_backend();
        let mut nona_renderer =
            nvgimpl::Renderer::create(&mut *ctx).expect("Failed to create nonaquad renderer");
        let mut nona_ctx = Context::create(&mut nona_renderer.with_context(&mut *ctx))
            .expect("Failed to create nona context");

        // Load a system font
        let font_data = std::fs::read("/usr/share/fonts/noto/NotoSans-Bold.ttf")
            .or_else(|_| std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"))
            .or_else(|_| std::fs::read("/usr/share/fonts/TTF/DejaVuSans.ttf"))
            .expect("Could not find a system font");

        // Register font with nona's built-in text system
        nona_ctx
            .create_font("sans", font_data.clone())
            .expect("Failed to create nona font");

        Self {
            ctx,
            nona_renderer,
            nona_ctx,
            font_data,
        }
    }

    fn draw_text_at(
        nc: &mut Context,
        rc: &mut impl nona::Renderer,
        font_data: &[u8],
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
        aa: bool,
    ) {
        let font_ref = skrifa::FontRef::new(font_data).expect("bad font");
        use skrifa::MetadataProvider;
        let outlines = font_ref.outline_glyphs();
        let charmap = font_ref.charmap();
        let metrics = font_ref.metrics(
            skrifa::instance::Size::new(font_size),
            skrifa::instance::LocationRef::default(),
        );
        let glyph_metrics = font_ref.glyph_metrics(
            skrifa::instance::Size::new(font_size),
            skrifa::instance::LocationRef::default(),
        );

        nc.shape_antialias(aa);
        let mut cursor_x = x;

        for ch in text.chars() {
            let glyph_id = charmap.map(ch).unwrap_or_default();
            if let Some(outline_glyph) = outlines.get(glyph_id) {
                let mut pen = GlyphPen::new();
                let settings = skrifa::outline::DrawSettings::unhinted(
                    skrifa::instance::Size::new(font_size),
                    skrifa::instance::LocationRef::default(),
                );
                let _ = outline_glyph.draw(settings, &mut pen);

                nc.save();
                nc.translate(cursor_x, y);
                nc.scale(1.0, -1.0);
                nc.fill_paint(Color::rgb(1.0, 1.0, 1.0));
                draw_glyph_path(nc, rc, &pen.commands);
                nc.restore();
            }

            let advance = glyph_metrics
                .advance_width(glyph_id)
                .unwrap_or(font_size * 0.5);
            cursor_x += advance;
        }
    }

    /// Draw text using skrifa for glyph mapping + nona's native glyph rendering
    fn draw_text_native_glyphs(
        nc: &mut Context,
        rc: &mut impl nona::Renderer,
        font_data: &[u8],
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
    ) {
        let font_ref = skrifa::FontRef::new(font_data).expect("bad font");
        use skrifa::MetadataProvider;
        let charmap = font_ref.charmap();
        let glyph_metrics = font_ref.glyph_metrics(
            skrifa::instance::Size::new(font_size),
            skrifa::instance::LocationRef::default(),
        );

        // Build glyph positions using skrifa for character→glyph mapping and advance widths
        let mut glyph_positions = Vec::new();
        let mut cursor_x = x;
        for ch in text.chars() {
            let glyph_id = charmap.map(ch).unwrap_or_default();
            glyph_positions.push(nona::GlyphPosition {
                glyph_id: glyph_id.to_u32() as u16,
                x: cursor_x,
                y,
            });
            let advance = glyph_metrics
                .advance_width(glyph_id)
                .unwrap_or(font_size * 0.5);
            cursor_x += advance;
        }

        nc.font("sans");
        nc.font_size(font_size);
        nc.fill_paint(Color::rgb(1.0, 1.0, 1.0));
        let _ = nc.draw_glyphs_by_id(rc, &glyph_positions);
    }
}

impl EventHandler for App {
    fn update(&mut self) {}

    fn draw(&mut self) {
        let (w, h) = miniquad::window::screen_size();
        let ctx = &mut *self.ctx;
        let nr = &mut self.nona_renderer;
        let nc = &mut self.nona_ctx;

        ctx.begin_default_pass(PassAction::Clear {
            color: Some((0.12, 0.12, 0.18, 1.0)),
            depth: Some(1.0),
            stencil: Some(0),
        });
        ctx.end_render_pass();

        {
            let mut rc = nr.with_context(ctx);
            let _ = rc.viewport(nona::Extent::new(w, h), 1.0);
        }
        {
            let mut rc = nr.with_context(ctx);
            let _ = nc.begin_frame(&mut rc, Some(Color::rgba(0.12, 0.12, 0.18, 1.0)));
        }

        {
            let mut rc = nr.with_context(ctx);
            let font_data = &self.font_data;

            // === Left column: skrifa outline glyphs ===

            // Row 1: outline glyphs AA on - large
            App::draw_text_at(
                nc,
                &mut rc,
                font_data,
                "Outline AA on 48px",
                20.0,
                60.0,
                48.0,
                true,
            );

            // Row 2: outline glyphs AA off - large
            App::draw_text_at(
                nc,
                &mut rc,
                font_data,
                "Outline AA off 48px",
                20.0,
                130.0,
                48.0,
                false,
            );

            // Row 3: outline glyphs AA on - small
            App::draw_text_at(
                nc,
                &mut rc,
                font_data,
                "Outline small AA on 16px",
                20.0,
                180.0,
                16.0,
                true,
            );

            // Row 4: outline glyphs AA off - small
            App::draw_text_at(
                nc,
                &mut rc,
                font_data,
                "Outline small AA off 16px",
                20.0,
                210.0,
                16.0,
                false,
            );

            // === Right column: nona built-in text ===

            nc.font("sans");
            nc.fill_paint(Color::rgb(1.0, 1.0, 1.0));

            nc.font_size(48.0);
            let _ = nc.text(&mut rc, Point::new(20.0, 310.0), "Nona native 48px");

            nc.font_size(24.0);
            let _ = nc.text(&mut rc, Point::new(20.0, 360.0), "Nona native 24px");

            nc.font_size(16.0);
            let _ = nc.text(
                &mut rc,
                Point::new(20.0, 400.0),
                "Nona native: The quick brown fox jumps over the lazy dog 16px",
            );

            nc.font_size(14.0);
            let _ = nc.text(
                &mut rc,
                Point::new(20.0, 430.0),
                "Nona native 14px - small text quality test",
            );

            // === Row: nona draw_glyphs_by_id (skrifa mapping + nona rendering) ===

            App::draw_text_native_glyphs(
                nc,
                &mut rc,
                font_data,
                "draw_glyphs_by_id 48px",
                20.0,
                490.0,
                48.0,
            );
            App::draw_text_native_glyphs(
                nc,
                &mut rc,
                font_data,
                "draw_glyphs_by_id 24px",
                20.0,
                530.0,
                24.0,
            );
            App::draw_text_native_glyphs(
                nc,
                &mut rc,
                font_data,
                "draw_glyphs_by_id: The quick brown fox 16px",
                20.0,
                560.0,
                16.0,
            );
        }

        {
            let mut rc = nr.with_context(ctx);
            let _ = nc.end_frame(&mut rc);
        }

        ctx.commit_frame();
    }
}

fn main() {
    let conf = Conf {
        window_title: "Nona Glyph Test".to_string(),
        window_width: 900,
        window_height: 600,
        ..Default::default()
    };

    miniquad::start(conf, move || Box::new(App::new()));
}
