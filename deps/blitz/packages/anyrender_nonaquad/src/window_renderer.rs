//! Implementation of `anyrender::WindowRenderer` using nonaquad/miniquad.

use anyrender::{WindowHandle, WindowRenderer};
use miniquad::window;
use nona::Color as NonaColor;
use nona::Renderer as NonaRenderer;
use nonaquad::nvgimpl;
use std::collections::HashMap;
use std::sync::Arc;

use crate::NonaquadScenePainter;
use crate::paint_scene::CustomImageState;

/// A WindowRenderer backed by nonaquad (NanoVG on miniquad).
pub struct NonaquadWindowRenderer {
    owned_ctx: Option<Box<dyn miniquad::RenderingBackend>>,
    nona_renderer: Option<nvgimpl::Renderer>,
    nona_ctx: Option<nona::Context>,
    width: u32,
    height: u32,
    is_active: bool,
    clear_color: NonaColor,
    /// Maps font data pointer to nona FontId for lazy font registration.
    font_registry: HashMap<usize, nona::FontId>,
    custom_images: HashMap<u64, CustomImageState>,
}

impl NonaquadWindowRenderer {
    /// Create a new NonaquadWindowRenderer (uninitialized).
    pub fn new() -> Self {
        Self {
            owned_ctx: None,
            nona_renderer: None,
            nona_ctx: None,
            width: 0,
            height: 0,
            is_active: false,
            clear_color: NonaColor::rgb(1.0, 1.0, 1.0),
            font_registry: HashMap::new(),
            custom_images: HashMap::new(),
        }
    }

    /// Create and initialize immediately, creating its own rendering backend.
    pub fn new_active(width: u32, height: u32) -> Self {
        let mut s = Self::new();
        let ctx = window::new_rendering_backend();
        s.init_with_ctx(ctx, width, height);
        s
    }

    /// Create and initialize using an external rendering backend (e.g. macroquad's).
    /// The context is borrowed for each render call via `render_with_ctx`.
    pub fn new_active_shared(
        ctx: &mut dyn miniquad::RenderingBackend,
        width: u32,
        height: u32,
    ) -> Self {
        let mut s = Self::new();
        s.init_nona(ctx, width, height);
        s
    }

    /// Set the clear/background color.
    pub fn set_clear_color(&mut self, color: NonaColor) {
        self.clear_color = color;
    }

    /// Initialize with an owned context.
    fn init_with_ctx(
        &mut self,
        mut ctx: Box<dyn miniquad::RenderingBackend>,
        width: u32,
        height: u32,
    ) {
        self.init_nona(&mut *ctx, width, height);
        self.owned_ctx = Some(ctx);
    }

    /// Initialize nona renderer and context using a borrowed backend.
    fn init_nona(&mut self, ctx: &mut dyn miniquad::RenderingBackend, width: u32, height: u32) {
        let mut nona_renderer =
            nvgimpl::Renderer::create(ctx).expect("Failed to create nonaquad renderer");
        let nona_ctx = nona::Context::create(&mut nona_renderer.with_context(ctx))
            .expect("Failed to create nona context");

        self.nona_renderer = Some(nona_renderer);
        self.nona_ctx = Some(nona_ctx);
        self.width = width;
        self.height = height;
        self.is_active = true;
    }

    /// Render using an external rendering backend (embedded mode).
    /// Does not clear the screen or commit the frame.
    pub fn render_with_ctx<F>(&mut self, ctx: &mut dyn miniquad::RenderingBackend, draw_fn: F)
    where
        F: FnOnce(&mut NonaquadScenePainter<'_, nvgimpl::RendererCtx<'_>>),
    {
        let nona_renderer = self
            .nona_renderer
            .as_mut()
            .expect("render_with_ctx called without renderer");
        let nona_ctx = self
            .nona_ctx
            .as_mut()
            .expect("render_with_ctx called without nona context");

        let dpr = 1.0;
        let width = self.width as f32;
        let height = self.height as f32;

        // Begin nona frame — no clear, the host app manages the background
        {
            let mut renderer_ctx = nona_renderer.with_context(ctx);
            let _ = renderer_ctx.viewport(nona::Extent::new(width, height), dpr);
        }

        {
            let mut renderer_ctx = nona_renderer.with_context(ctx);
            let _ = nona_ctx.begin_frame(&mut renderer_ctx, None);
        }

        // Paint
        {
            let mut renderer_ctx = nona_renderer.with_context(ctx);
            let mut painter =
                NonaquadScenePainter::new(
                    nona_ctx,
                    &mut renderer_ctx,
                    &mut self.font_registry,
                    &mut self.custom_images,
                );
            draw_fn(&mut painter);
        }

        // End nona frame
        {
            let mut renderer_ctx = nona_renderer.with_context(ctx);
            let _ = nona_ctx.end_frame(&mut renderer_ctx);
        }
    }
}

impl Default for NonaquadWindowRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowRenderer for NonaquadWindowRenderer {
    type ScenePainter<'a> = NonaquadScenePainter<'a, nvgimpl::RendererCtx<'a>>;

    fn resume(&mut self, _window: Arc<dyn WindowHandle>, width: u32, height: u32) {
        if !self.is_active {
            let ctx = window::new_rendering_backend();
            self.init_with_ctx(ctx, width, height);
        } else {
            self.width = width;
            self.height = height;
        }
    }

    fn suspend(&mut self) {
        self.is_active = false;
        self.nona_ctx = None;
        self.nona_renderer = None;
        self.owned_ctx = None;
    }

    fn is_active(&self) -> bool {
        self.is_active
    }

    fn set_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    fn render<F: FnOnce(&mut Self::ScenePainter<'_>)>(&mut self, draw_fn: F) {
        let font_registry = &mut self.font_registry;
        let ctx = self
            .owned_ctx
            .as_deref_mut()
            .expect("render called without owned context — use render_with_ctx for embedded mode");
        let nona_renderer = self
            .nona_renderer
            .as_mut()
            .expect("render called without renderer");
        let nona_ctx = self
            .nona_ctx
            .as_mut()
            .expect("render called without nona context");

        let dpr = 1.0;
        let width = self.width as f32;
        let height = self.height as f32;

        // Begin miniquad render pass
        ctx.begin_default_pass(miniquad::PassAction::Clear {
            color: Some((
                self.clear_color.r,
                self.clear_color.g,
                self.clear_color.b,
                self.clear_color.a,
            )),
            depth: Some(1.0),
            stencil: Some(0),
        });
        ctx.end_render_pass();

        // Begin nona frame
        {
            let mut renderer_ctx = nona_renderer.with_context(ctx);
            let _ = renderer_ctx.viewport(nona::Extent::new(width, height), dpr);
        }

        {
            let mut renderer_ctx = nona_renderer.with_context(ctx);
            let _ = nona_ctx.begin_frame(&mut renderer_ctx, Some(self.clear_color));
        }

        // Paint
        {
            let mut renderer_ctx = nona_renderer.with_context(ctx);
            let mut painter = NonaquadScenePainter::new(
                nona_ctx,
                &mut renderer_ctx,
                font_registry,
                &mut self.custom_images,
            );
            draw_fn(&mut painter);
        }

        // End nona frame
        {
            let mut renderer_ctx = nona_renderer.with_context(ctx);
            let _ = nona_ctx.end_frame(&mut renderer_ctx);
        }

        // Present
        ctx.commit_frame();
    }
}
