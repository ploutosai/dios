use anyrender::PaintScene;
use anyrender_nonaquad::NonaquadScenePainter;
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene as paint_blitz_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use kurbo::BezPath;
use linebender_resource_handle::Blob;
use miniquad::{self, conf::Conf, EventHandler, PassAction};
use nona::{Color, Context, Point, Renderer as _};
use nonaquad::nvgimpl;

const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/LiberationMono-Regular.ttf");

const BLITZ_HTML_BODY_RECT: &str = r#"
<!DOCTYPE html><html><head><style>
body { background: #23283b; margin: 0; }
.card { width: 220px; height: 90px; background: #2b3147; }
</style></head><body><div class="card"></div></body></html>
"#;

const PANEL_W: u32 = 260;
const PANEL_H: u32 = 180;
const CARD_W: f64 = 220.0;
const CARD_H: f64 = 90.0;
const BG: (u8, u8, u8) = (35, 40, 59);
const CARD: (u8, u8, u8) = (43, 49, 71);

struct App {
    ctx: Box<dyn miniquad::RenderingBackend>,
    renderer: nvgimpl::Renderer,
    canvas: Context,
    blitz_doc: HtmlDocument,
}

fn perimeter_path(width: f64, height: f64) -> BezPath {
    let mut path = BezPath::new();
    path.move_to((0.0, 0.0));
    path.line_to((0.0, 0.0));
    path.line_to((width, 0.0));
    path.line_to((width, 0.0));

    path.move_to((width, 0.0));
    path.line_to((width, 0.0));
    path.line_to((width, height));
    path.line_to((width, height));

    path.move_to((width, height));
    path.line_to((width, height));
    path.line_to((0.0, height));
    path.line_to((0.0, height));

    path.move_to((0.0, height));
    path.line_to((0.0, height));
    path.line_to((0.0, 0.0));
    path.line_to((0.0, 0.0));
    path
}

fn draw_static_body_rect_like(scene: &mut impl PaintScene, offset: kurbo::Affine) {
    let viewport_rect = kurbo::Rect::new(0.0, 0.0, PANEL_W as f64, PANEL_H as f64);
    let body_rect = kurbo::Rect::new(0.0, 0.0, PANEL_W as f64, 90.0);
    let card_rect = kurbo::Rect::new(0.0, 0.0, CARD_W, CARD_H);
    let body_outline = perimeter_path(PANEL_W as f64, 90.0);
    let card_outline = perimeter_path(CARD_W, CARD_H);
    let bg = peniko::Color::from_rgb8(BG.0, BG.1, BG.2);
    let card = peniko::Color::from_rgb8(CARD.0, CARD.1, CARD.2);
    let black = peniko::Color::from_rgb8(0, 0, 0);

    // Static transcription of the logged Blitz `body-rect` command stream.
    scene.fill(peniko::Fill::NonZero, offset, bg, None, &viewport_rect);

    scene.push_clip_layer(offset, &body_rect);
    scene.pop_layer();

    scene.fill(peniko::Fill::NonZero, offset, black, None, &body_outline);
    scene.fill(peniko::Fill::NonZero, offset, bg, None, &body_rect);

    scene.push_clip_layer(offset, &body_rect);
    scene.pop_layer();

    scene.fill(peniko::Fill::NonZero, offset, black, None, &body_outline);
    scene.fill(peniko::Fill::NonZero, offset, card, None, &card_rect);

    scene.push_clip_layer(offset, &card_rect);
    scene.pop_layer();

    scene.fill(peniko::Fill::NonZero, offset, black, None, &card_outline);
}

impl App {
    fn new() -> Self {
        let mut ctx = miniquad::window::new_rendering_backend();
        let mut renderer = nvgimpl::Renderer::create(&mut *ctx).expect("create renderer");
        let mut canvas =
            Context::create(&mut renderer.with_context(&mut *ctx)).expect("create context");

        canvas
            .create_font("mono", FONT_DATA.to_vec())
            .expect("register bundled font");

        let mut blitz_doc = HtmlDocument::from_html(
            BLITZ_HTML_BODY_RECT,
            DocumentConfig {
                font_ctx: Some({
                    let mut font_ctx = blitz_dom::FontContext::default();
                    font_ctx.collection.register_fonts(
                        Blob::new(std::sync::Arc::new(blitz_dom::BULLET_FONT) as _),
                        None,
                    );
                    font_ctx
                        .collection
                        .register_fonts(Blob::new(std::sync::Arc::new(FONT_DATA) as _), None);
                    font_ctx
                }),
                ..Default::default()
            },
        );
        blitz_doc.set_viewport(Viewport::new(PANEL_W, PANEL_H, 1.0, ColorScheme::Light));

        Self {
            ctx,
            renderer,
            canvas,
            blitz_doc,
        }
    }
}

impl EventHandler for App {
    fn update(&mut self) {}

    fn draw(&mut self) {
        let (w, h) = miniquad::window::screen_size();
        let dpi = miniquad::window::dpi_scale();

        self.ctx.begin_default_pass(PassAction::Clear {
            color: Some((0.10, 0.11, 0.16, 1.0)),
            depth: Some(1.0),
            stencil: Some(0),
        });
        self.ctx.end_render_pass();

        {
            let mut rc = self.renderer.with_context(&mut *self.ctx);
            let _ = rc.viewport(nona::Extent::new(w, h), dpi);
        }

        {
            let mut rc = self.renderer.with_context(&mut *self.ctx);
            let _ = self
                .canvas
                .begin_frame(&mut rc, Some(Color::rgba(0.10, 0.11, 0.16, 1.0)));
        }

        {
            let mut rc = self.renderer.with_context(&mut *self.ctx);
            self.canvas.font("mono");
            self.canvas.font_size(20.0);
            self.canvas.fill_paint(Color::rgb(0.80, 0.84, 0.96));
            let _ = self.canvas.text(
                &mut rc,
                Point::new(24.0, 34.0),
                &format!("blitz vs direct nona | size=({w:.0}, {h:.0}) dpi={dpi:.2}"),
            );

            let mut font_registry = std::collections::HashMap::new();
            let mut scene =
                NonaquadScenePainter::new(&mut self.canvas, &mut rc, &mut font_registry);

            let left_x = 24u32;
            let right_x = 324u32;
            let panel_y = 60u32;

            self.blitz_doc
                .set_viewport(Viewport::new(PANEL_W, PANEL_H, 1.0, ColorScheme::Light));
            self.blitz_doc.resolve(0.0);

            scene.push_clip_layer(
                kurbo::Affine::translate((left_x as f64, panel_y as f64)),
                &kurbo::Rect::new(0.0, 0.0, PANEL_W as f64, PANEL_H as f64),
            );
            paint_blitz_scene(
                &mut scene,
                &self.blitz_doc,
                1.0,
                PANEL_W,
                PANEL_H,
                left_x,
                panel_y,
            );
            scene.pop_layer();

            draw_static_body_rect_like(
                &mut scene,
                kurbo::Affine::translate((right_x as f64, panel_y as f64)),
            );

            self.canvas.font_size(16.0);
            self.canvas.fill_paint(Color::rgb(0.88, 0.90, 0.94));
            let _ = self
                .canvas
                .text(&mut rc, Point::new(left_x as f32, 255.0), "blitz body-rect");
            let _ = self.canvas.text(
                &mut rc,
                Point::new(right_x as f32, 255.0),
                "direct static commands",
            );
        }

        {
            let mut rc = self.renderer.with_context(&mut *self.ctx);
            let _ = self.canvas.end_frame(&mut rc);
        }

        self.ctx.commit_frame();
    }
}

fn main() {
    miniquad::start(
        Conf {
            window_title: "Nona Web Repro".to_string(),
            window_width: 620,
            window_height: 320,
            high_dpi: true,
            ..Default::default()
        },
        || Box::new(App::new()),
    );
}
