use glam::{Mat4, Vec4};
use miniquad::graphics::*;
use nona::{renderer::*, NonaError};
use slab::Slab;

enum ShaderType {
    FillGradient,
    FillImage,
    Simple,
    Image,
}

#[derive(PartialEq, Eq, Debug)]
enum CallType {
    Fill,
    ConvexFill,
    Stroke,
    Triangles,
}

/// Color and Alpha blend states
struct Blend {
    pub color: BlendState,
    pub alpha: BlendState,
}

impl From<CompositeOperationState> for Blend {
    fn from(state: CompositeOperationState) -> Self {
        Blend {
            color: BlendState::new(
                Equation::Add,
                convert_blend_factor(state.src_rgb),
                convert_blend_factor(state.dst_rgb),
            ),
            alpha: BlendState::new(
                Equation::Add,
                convert_blend_factor(state.src_alpha),
                convert_blend_factor(state.dst_alpha),
            ),
        }
    }
}

struct Call {
    call_type: CallType,
    image: Option<usize>,
    path_offset: usize,
    path_count: usize,
    triangle_offset: usize,
    triangle_count: usize,
    uniform_offset: usize,
    blend_func: Blend,
}

struct Texture {
    tex: TextureId,
    format: TextureFormat,
    flags: ImageFlags,
}

struct GLPath {
    fill_offset: usize,
    fill_count: usize,
    stroke_offset: usize,
    stroke_count: usize,
}

pub struct Renderer {
    shader: ShaderId,
    textures: Slab<Texture>,
    view: Extent,
    bindings: Bindings,
    vertex_buffer_capacity: usize,
    index_buffer_capacity: usize,
    calls: Vec<Call>,
    paths: Vec<GLPath>,
    vertexes: Vec<Vertex>,
    indices: Vec<u16>,
    uniforms: Vec<shader::Uniforms>,
}

pub struct RendererCtx<'a> {
    renderer: &'a mut Renderer,
    ctx: &'a mut (dyn miniquad::RenderingBackend + 'a),
}

impl Renderer {
    pub fn with_context<'a>(
        &'a mut self,
        ctx: &'a mut (dyn miniquad::RenderingBackend + 'a),
    ) -> RendererCtx<'a> {
        RendererCtx {
            renderer: self,
            ctx,
        }
    }
}

mod shader {
    use miniquad::*;

    pub const VERTEX: &str = include_str!("shader.vert");
    pub const FRAGMENT: &str = include_str!("shader.frag");

    pub const ATTRIBUTES: &[VertexAttribute] = &[
        VertexAttribute::new("vertex", VertexFormat::Float2),
        VertexAttribute::new("tcoord", VertexFormat::Float2),
    ];
    pub fn meta() -> ShaderMeta {
        ShaderMeta {
            images: vec!["tex".to_string()],
            uniforms: UniformBlockLayout {
                uniforms: vec![
                    UniformDesc::new("viewSize", UniformType::Float2),
                    UniformDesc::new("scissorMat", UniformType::Mat4),
                    UniformDesc::new("paintMat", UniformType::Mat4),
                    UniformDesc::new("innerCol", UniformType::Float4),
                    UniformDesc::new("outerCol", UniformType::Float4),
                    UniformDesc::new("scissorExt", UniformType::Float2),
                    UniformDesc::new("scissorScale", UniformType::Float2),
                    UniformDesc::new("extent", UniformType::Float2),
                    UniformDesc::new("radius", UniformType::Float1),
                    UniformDesc::new("feather", UniformType::Float1),
                    UniformDesc::new("strokeMult", UniformType::Float1),
                    UniformDesc::new("strokeThr", UniformType::Float1),
                    UniformDesc::new("texType", UniformType::Int1),
                    UniformDesc::new("type", UniformType::Int1),
                ],
            },
        }
    }

    #[derive(Default)]
    #[repr(C)]
    pub struct Uniforms {
        pub view_size: (f32, f32),
        pub scissor_mat: glam::Mat4,
        pub paint_mat: glam::Mat4,
        pub inner_col: (f32, f32, f32, f32),
        pub outer_col: (f32, f32, f32, f32),
        pub scissor_ext: (f32, f32),
        pub scissor_scale: (f32, f32),
        pub extent: (f32, f32),
        pub radius: f32,
        pub feather: f32,
        pub stroke_mult: f32,
        pub stroke_thr: f32,
        pub tex_type: i32,
        pub type_: i32,
    }
}

const MAX_VERTICES: usize = 21845; // u16.max / 3 due to index buffer limitations
const MAX_INDICES: usize = u16::max_value() as usize;

impl Renderer {
    pub fn create(ctx: &mut (dyn miniquad::RenderingBackend + '_)) -> Result<Renderer, NonaError> {
        let shader = ctx
            .new_shader(
                ShaderSource::Glsl {
                    vertex: shader::VERTEX,
                    fragment: shader::FRAGMENT,
                },
                shader::meta(),
            )
            .map_err(|error: ShaderError| NonaError::Shader(error.to_string()))?;

        let vertex_buffer = ctx.new_buffer(
            BufferType::VertexBuffer,
            BufferUsage::Stream,
            BufferSource::empty::<Vertex>(MAX_VERTICES),
        );
        let index_buffer = ctx.new_buffer(
            BufferType::IndexBuffer,
            BufferUsage::Stream,
            BufferSource::empty::<u16>(MAX_INDICES),
        );

        let pixels: [u8; 4 * 4 * 4] = [
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00,
            0x00, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0xFF,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0xFF, 0xFF,
            0xFF, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ];
        let temp_texture = ctx.new_texture_from_rgba8(4, 4, &pixels);

        let bindings = Bindings {
            vertex_buffers: vec![vertex_buffer],
            index_buffer,
            images: vec![temp_texture], // TODO: set and use image only if needed
        };

        Ok(Renderer {
            shader,
            bindings,
            vertex_buffer_capacity: MAX_VERTICES,
            index_buffer_capacity: MAX_INDICES,
            textures: Default::default(),
            view: Default::default(),
            calls: Default::default(),
            paths: Default::default(),
            vertexes: Default::default(),
            indices: Default::default(),
            uniforms: Default::default(),
        })
    }

    /// Create a pipeline with the given blend and additional params.
    fn make_pipeline(
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        shader: ShaderId,
        blend: &Blend,
        stencil_test: Option<StencilState>,
        cull_face: CullFace,
        color_write: (bool, bool, bool, bool),
    ) -> Pipeline {
        ctx.new_pipeline(
            &[BufferLayout::default()],
            shader::ATTRIBUTES,
            shader,
            PipelineParams {
                cull_face,
                front_face_order: FrontFaceOrder::CounterClockwise,
                depth_write: false,
                color_blend: Some(blend.color),
                alpha_blend: Some(blend.alpha),
                stencil_test,
                color_write,
                ..Default::default()
            },
        )
    }

    fn set_uniforms(
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        uniforms: &shader::Uniforms,
        _img: Option<usize>,
    ) {
        ctx.apply_uniforms(UniformsSource::table(uniforms));
    }

    fn do_fill(
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        shader: ShaderId,
        blend: &Blend,
        call: &Call,
        paths: &[GLPath],
        bindings: &mut Bindings,
        index_buffer_capacity: &mut usize,
        indices: &mut Vec<u16>,
        uniforms: &shader::Uniforms,
        uniforms_next: &shader::Uniforms,
        temp_pipelines: &mut Vec<Pipeline>,
    ) {
        indices.clear();
        // TODO: test!!!

        // Phase 1: Stencil fill (no color write, dual-sided stencil, no culling)
        let p1 = Self::make_pipeline(
            ctx,
            shader,
            blend,
            Some(StencilState {
                front: StencilFaceState {
                    fail_op: StencilOp::Keep,
                    depth_fail_op: StencilOp::Keep,
                    pass_op: StencilOp::IncrementWrap,
                    test_func: CompareFunc::Always,
                    test_ref: 0,
                    test_mask: 0xff,
                    write_mask: 0xff,
                },
                back: StencilFaceState {
                    fail_op: StencilOp::Keep,
                    depth_fail_op: StencilOp::Keep,
                    pass_op: StencilOp::DecrementWrap,
                    test_func: CompareFunc::Always,
                    test_ref: 0,
                    test_mask: 0xff,
                    write_mask: 0xff,
                },
            }),
            CullFace::Nothing,
            (false, false, false, false),
        );
        ctx.apply_pipeline(&p1);
        temp_pipelines.push(p1);

        Self::set_uniforms(ctx, uniforms, call.image);

        for path in paths {
            Self::add_triangle_fan(indices, path.fill_offset as u16, path.fill_count as u16);
        }

        // draw
        Self::ensure_index_buffer_capacity(ctx, bindings, index_buffer_capacity, indices.len());
        ctx.buffer_update(bindings.index_buffer, BufferSource::slice(indices));
        ctx.apply_bindings(bindings);
        ctx.draw(0, indices.len() as i32, 1);
        indices.clear();

        // Phase 2: Anti-alias pass (stencil equal to 0, back-face culling, color write)
        let p2 = Self::make_pipeline(
            ctx,
            shader,
            blend,
            Some(StencilState {
                front: StencilFaceState {
                    fail_op: StencilOp::Keep,
                    depth_fail_op: StencilOp::Keep,
                    pass_op: StencilOp::Keep,
                    test_func: CompareFunc::Equal,
                    test_ref: 0,
                    test_mask: 0xff,
                    write_mask: 0xff,
                },
                back: StencilFaceState {
                    fail_op: StencilOp::Keep,
                    depth_fail_op: StencilOp::Keep,
                    pass_op: StencilOp::Keep,
                    test_func: CompareFunc::Equal,
                    test_ref: 0,
                    test_mask: 0xff,
                    write_mask: 0xff,
                },
            }),
            CullFace::Back,
            (true, true, true, true),
        );
        ctx.apply_pipeline(&p2);
        temp_pipelines.push(p2);

        Self::set_uniforms(ctx, uniforms_next, call.image);

        for path in paths {
            Self::add_triangle_strip(indices, path.stroke_offset as u16, path.stroke_count as u16);
        }
        Self::ensure_index_buffer_capacity(ctx, bindings, index_buffer_capacity, indices.len());
        ctx.buffer_update(bindings.index_buffer, BufferSource::slice(indices));
        ctx.apply_bindings(bindings);
        ctx.draw(0, indices.len() as i32, 1);

        indices.clear();

        // Phase 3: Clear stencil (stencil not-equal, zero ops)
        let p3 = Self::make_pipeline(
            ctx,
            shader,
            blend,
            Some(StencilState {
                front: StencilFaceState {
                    fail_op: StencilOp::Zero,
                    depth_fail_op: StencilOp::Zero,
                    pass_op: StencilOp::Zero,
                    test_func: CompareFunc::NotEqual,
                    test_ref: 0,
                    test_mask: 0xff,
                    write_mask: 0xff,
                },
                back: StencilFaceState {
                    fail_op: StencilOp::Zero,
                    depth_fail_op: StencilOp::Zero,
                    pass_op: StencilOp::Zero,
                    test_func: CompareFunc::NotEqual,
                    test_ref: 0,
                    test_mask: 0xff,
                    write_mask: 0xff,
                },
            }),
            CullFace::Back,
            (true, true, true, true),
        );
        ctx.apply_pipeline(&p3);
        temp_pipelines.push(p3);

        Self::add_triangle_strip(
            indices,
            call.triangle_offset as u16,
            call.triangle_count as u16,
        );
        Self::ensure_index_buffer_capacity(ctx, bindings, index_buffer_capacity, indices.len());
        ctx.buffer_update(bindings.index_buffer, BufferSource::slice(indices));
        ctx.apply_bindings(bindings);
        ctx.draw(0, indices.len() as i32, 1);
    }

    // from https://www.khronos.org/opengl/wiki/Primitive:
    // GL_TRIANGLE_FAN:
    // Indices:     0 1 2 3 4 5 ... (6 total indices)
    // Triangles:  {0 1 2}
    //             {0} {2 3}
    //             {0}   {3 4}
    //             {0}     {4 5}    (4 total triangles)
    //
    // GL_TRIANGLES:
    // Indices:     0 1 2 3 4 5 ...
    // Triangles:  {0 1 2}
    //                   {3 4 5}
    /// Adds indices to convert from GL_TRIANGLE_FAN to GL_TRIANGLES
    #[inline]
    fn add_triangle_fan(indices: &mut Vec<u16>, first_vertex_index: u16, index_count: u16) {
        if index_count < 3 {
            return;
        }

        let start_index = first_vertex_index;
        for i in first_vertex_index..first_vertex_index + index_count - 2 {
            indices.push(start_index);
            indices.push(i + 1);
            indices.push(i + 2);
        }
    }

    // from https://www.khronos.org/opengl/wiki/Primitive:
    // GL_TRIANGLES:
    // Indices:     0 1 2 3 4 5 ... (6 total indices)
    // Triangles:  {0 1 2}
    //                   {3 4 5}    (2 total indices)
    /// Adds indices to draw GL_TRIANGLES
    #[inline]
    fn add_triangles(indices: &mut Vec<u16>, first_vertex_index: u16, index_count: u16) {
        // TODO: test!
        for i in (first_vertex_index..first_vertex_index + index_count).step_by(3) {
            indices.push(i);
            indices.push(i + 1);
            indices.push(i + 2);
        }
    }

    // from https://www.khronos.org/opengl/wiki/Primitive:
    // GL_TRIANGLE_STRIP:
    // Indices:     0 1 2 3 4 5 ... (6 total indices)
    // Triangles:  {0 1 2}
    //               {1 2 3}  drawing order is (2 1 3) to maintain proper winding
    //                 {2 3 4}
    //                   {3 4 5}  drawing order is (4 3 5) to maintain proper winding (4 total triangles)
    //
    // GL_TRIANGLES:
    // Indices:     0 1 2 3 4 5 ...
    // Triangles:  {0 1 2}
    //                   {3 4 5}
    /// Adds indices to convert from GL_TRIANGLE_STRIP to GL_TRIANGLES
    #[inline]
    fn add_triangle_strip(indices: &mut Vec<u16>, first_vertex_index: u16, index_count: u16) {
        if index_count < 3 {
            return;
        }
        let mut draw_order_winding = true; // true to draw in straight (0 1 2) order; false to draw in (1 0 2) order to maintain proper winding
        for i in first_vertex_index..first_vertex_index + index_count - 2 {
            if draw_order_winding {
                indices.push(i);
                indices.push(i + 1);
            } else {
                indices.push(i + 1);
                indices.push(i);
            }
            draw_order_winding = !draw_order_winding;
            indices.push(i + 2);
        }
    }

    fn do_convex_fill(
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        call: &Call,
        paths: &[GLPath],
        bindings: &mut Bindings,
        index_buffer_capacity: &mut usize,
        indices: &mut Vec<u16>,
        uniforms: &shader::Uniforms,
    ) {
        indices.clear();
        Self::set_uniforms(ctx, uniforms, call.image);

        // convert all fans and strips into single draw call
        // more info: https://gamedev.stackexchange.com/questions/133208/difference-in-gldrawarrays-and-gldrawelements
        for path in paths {
            // draw TRIANGLE_FAN from path.fill_offset with path.fill_count, same as
            // glDrawArrays(GL_TRIANGLE_FAN, path.fill_offset, path.fill_count); // note: count is "number of indices to render"
            Self::add_triangle_fan(indices, path.fill_offset as u16, path.fill_count as u16);

            if path.stroke_count > 0 {
                // draw TRIANGLE_STRIP from path.stroke_offset with path.stroke_count, same as
                // glDrawArrays(GL_TRIANGLE_STRIP,path.stroke_offset, path.stroke_count);
                Self::add_triangle_strip(
                    indices,
                    path.stroke_offset as u16,
                    path.stroke_count as u16,
                );
            }
        }

        Self::ensure_index_buffer_capacity(ctx, bindings, index_buffer_capacity, indices.len());
        ctx.buffer_update(bindings.index_buffer, BufferSource::slice(indices));
        ctx.apply_bindings(bindings);
        ctx.draw(0, indices.len() as i32, 1);
    }

    fn do_stroke(
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        call: &Call,
        paths: &[GLPath],
        bindings: &mut Bindings,
        index_buffer_capacity: &mut usize,
        indices: &mut Vec<u16>,
        uniforms: &shader::Uniforms,
        uniforms_next: &shader::Uniforms,
    ) {
        indices.clear();

        // TODO glEnable(GL_STENCIL_TEST);

        // TODO glStencilMask(0xff);
        // TODO glStencilFunc(GL_EQUAL, 0x0, 0xff);
        // TODO glStencilOp(GL_KEEP, GL_KEEP, GL_INCR);

        // self.set_uniforms(call.uniform_offset + 1, call.image);
        Self::set_uniforms(ctx, uniforms_next, call.image);
        for path in paths {
            // glDrawArrays(GL_TRIANGLE_STRIP, path.stroke_offset as i32, path.stroke_count as i32);
            Self::add_triangle_strip(indices, path.stroke_offset as u16, path.stroke_count as u16);
        }
        Self::ensure_index_buffer_capacity(ctx, bindings, index_buffer_capacity, indices.len());
        ctx.buffer_update(bindings.index_buffer, BufferSource::slice(indices));
        ctx.apply_bindings(bindings);
        ctx.draw(0, indices.len() as i32, 1);

        // self.set_uniforms(call.uniform_offset, call.image);
        Self::set_uniforms(ctx, uniforms, call.image);
        // TODO glStencilFunc(GL_EQUAL, 0x0, 0xff);
        // TODO glStencilOp(GL_KEEP, GL_KEEP, GL_KEEP);
        ctx.draw(0, indices.len() as i32, 1);

        // TODO glColorMask(GL_FALSE, GL_FALSE, GL_FALSE, GL_FALSE);
        // TODO glStencilFunc(GL_ALWAYS, 0x0, 0xff);
        // TODO glStencilOp(GL_ZERO, GL_ZERO, GL_ZERO);
        // ctx.draw(0, indices.len() as i32, 1); TODO: uncomment once above TODOs are done
        // TODO glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);

        // TODO glDisable(GL_STENCIL_TEST);
    }

    fn do_triangles(
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        call: &Call,
        bindings: &mut Bindings,
        index_buffer_capacity: &mut usize,
        indices: &mut Vec<u16>,
        uniforms: &shader::Uniforms,
    ) {
        indices.clear();
        Self::set_uniforms(ctx, uniforms, call.image);

        // draw TRIANGLES from call.triangle_offset with call.triangle_count, same as
        // glDrawArrays(GL_TRIANGLES, call.triangle_offset as i32, call.triangle_count as i32); // note: triangle_count is "number of indices to render", not number of triangles
        Self::add_triangles(
            indices,
            call.triangle_offset as u16,
            call.triangle_count as u16,
        );

        Self::ensure_index_buffer_capacity(ctx, bindings, index_buffer_capacity, indices.len());
        ctx.buffer_update(bindings.index_buffer, BufferSource::slice(indices));
        ctx.apply_bindings(bindings);
        ctx.draw(0, indices.len() as i32, 1);
    }

    fn convert_paint(
        &self,
        paint: &Paint,
        scissor: &Scissor,
        width: f32,
        fringe: f32,
        stroke_thr: f32,
    ) -> shader::Uniforms {
        let mut frag = shader::Uniforms {
            view_size: Default::default(),
            scissor_mat: Mat4::ZERO,
            paint_mat: Default::default(),
            inner_col: premul_color(paint.inner_color).into_tuple(),
            outer_col: premul_color(paint.outer_color).into_tuple(),
            scissor_ext: Default::default(),
            scissor_scale: Default::default(),
            extent: Default::default(),
            radius: 0.0,
            feather: 0.0,
            stroke_mult: 0.0,
            stroke_thr,
            tex_type: 0,
            type_: 0,
        };

        if scissor.extent.width < -0.5 || scissor.extent.height < -0.5 {
            frag.scissor_ext = (1.0, 1.0);
            frag.scissor_scale = (1.0, 1.0);
        } else {
            frag.scissor_mat = xform_to_4x4(scissor.xform.inverse());
            frag.scissor_ext = (scissor.extent.width, scissor.extent.height);
            frag.scissor_scale = (
                (scissor.xform.0[0] * scissor.xform.0[0] + scissor.xform.0[2] * scissor.xform.0[2])
                    .sqrt()
                    / fringe,
                (scissor.xform.0[1] * scissor.xform.0[1] + scissor.xform.0[3] * scissor.xform.0[3])
                    .sqrt()
                    / fringe,
            );
        }

        frag.extent = (paint.extent.width, paint.extent.height);
        frag.stroke_mult = (width * 0.5 + fringe * 0.5) / fringe;

        let mut invxform = Transform::default();

        if let Some(img) = paint.image {
            if let Some(texture) = self.textures.get(img) {
                if texture.flags.contains(ImageFlags::FLIPY) {
                    let m1 = Transform::translate(0.0, frag.extent.1 * 0.5) * paint.xform;
                    let m2 = Transform::scale(1.0, -1.0) * m1;
                    let m1 = Transform::translate(0.0, -frag.extent.1 * 0.5) * m2;
                    invxform = m1.inverse();
                } else {
                    invxform = paint.xform.inverse();
                };

                frag.type_ = ShaderType::FillImage as i32;
                match texture.format {
                    TextureFormat::RGBA8 => {
                        frag.tex_type = if texture.flags.contains(ImageFlags::PREMULTIPLIED) {
                            0
                        } else {
                            1
                        }
                    }
                    TextureFormat::Alpha => frag.tex_type = 2,
                    _ => todo!("Unsupported texture type"),
                }
            }
        } else {
            frag.type_ = ShaderType::FillGradient as i32;
            frag.radius = paint.radius;
            frag.feather = paint.feather;
            invxform = paint.xform.inverse();
        }

        frag.paint_mat = xform_to_4x4(invxform);

        frag
    }

    fn append_uniforms(&mut self, uniforms: shader::Uniforms) {
        self.uniforms.push(uniforms);
    }
}

trait IntoTuple4<T> {
    fn into_tuple(self) -> (T, T, T, T);
}

impl IntoTuple4<f32> for Color {
    fn into_tuple(self) -> (f32, f32, f32, f32) {
        (self.r, self.g, self.b, self.a)
    }
}

impl renderer::Renderer for RendererCtx<'_> {
    fn edge_antialias(&self) -> bool {
        self.renderer.edge_antialias()
    }

    fn view_size(&self) -> (f32, f32) {
        self.renderer.view_size()
    }

    fn device_pixel_ratio(&self) -> f32 {
        self.renderer.device_pixel_ratio()
    }

    fn create_texture(
        &mut self,
        texture_type: TextureType,
        width: usize,
        height: usize,
        flags: ImageFlags,
        data: Option<&[u8]>,
    ) -> Result<ImageId, NonaError> {
        self.renderer
            .create_texture(self.ctx, texture_type, width, height, flags, data)
    }

    fn delete_texture(&mut self, img: ImageId) -> Result<(), NonaError> {
        self.renderer.delete_texture(self.ctx, img)
    }

    fn update_texture(
        &mut self,
        img: ImageId,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        data: &[u8],
    ) -> Result<(), NonaError> {
        self.renderer
            .update_texture(self.ctx, img, x, y, width, height, data)
    }

    fn texture_size(&self, img: ImageId) -> Result<(usize, usize), NonaError> {
        self.renderer.texture_size(self.ctx, img)
    }

    fn viewport(&mut self, extent: Extent, device_pixel_ratio: f32) -> Result<(), NonaError> {
        self.renderer.viewport(extent, device_pixel_ratio)
    }

    fn clear_screen(&mut self, color: Color) {
        self.renderer.clear_screen(self.ctx, color)
    }

    fn flush(&mut self) -> Result<(), NonaError> {
        self.renderer.flush(self.ctx)
    }

    fn fill(
        &mut self,
        paint: &Paint,
        composite_operation: CompositeOperationState,
        scissor: &Scissor,
        fringe: f32,
        bounds: Bounds,
        paths: &[Path],
    ) -> Result<(), NonaError> {
        self.renderer.fill(
            self.ctx,
            paint,
            composite_operation,
            scissor,
            fringe,
            bounds,
            paths,
        )
    }

    fn stroke(
        &mut self,
        paint: &Paint,
        composite_operation: CompositeOperationState,
        scissor: &Scissor,
        fringe: f32,
        stroke_width: f32,
        paths: &[Path],
    ) -> Result<(), NonaError> {
        self.renderer.stroke(
            self.ctx,
            paint,
            composite_operation,
            scissor,
            fringe,
            stroke_width,
            paths,
        )
    }

    fn triangles(
        &mut self,
        paint: &Paint,
        composite_operation: CompositeOperationState,
        scissor: &Scissor,
        vertexes: &[Vertex],
    ) -> Result<(), NonaError> {
        self.renderer
            .triangles(self.ctx, paint, composite_operation, scissor, vertexes)
    }
}

impl Renderer {
    fn edge_antialias(&self) -> bool {
        true
    }

    fn view_size(&self) -> (f32, f32) {
        miniquad::window::screen_size()
    }

    fn device_pixel_ratio(&self) -> f32 {
        miniquad::window::dpi_scale()
    }

    fn create_texture(
        &mut self,
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        texture_type: TextureType,
        width: usize,
        height: usize,
        flags: ImageFlags,
        data: Option<&[u8]>,
    ) -> Result<ImageId, NonaError> {
        let format = match texture_type {
            TextureType::RGBA => TextureFormat::RGBA8,
            TextureType::Alpha => TextureFormat::Alpha,
        };
        let filter = if flags.contains(ImageFlags::NEAREST) {
            FilterMode::Nearest
        } else {
            FilterMode::Linear
        };
        let zeroed;
        let source = match data {
            Some(bytes) => TextureSource::Bytes(bytes),
            None => {
                let pixel_size = match format {
                    TextureFormat::RGBA8 => 4,
                    TextureFormat::Alpha => 1,
                    _ => return Err(NonaError::Texture("unsupported empty texture format".into())),
                };
                zeroed = vec![0; width * height * pixel_size];
                TextureSource::Bytes(&zeroed)
            }
        };
        let tex = ctx.new_texture(
            TextureAccess::Static,
            source,
            TextureParams {
                format,
                wrap: TextureWrap::Clamp, // TODO: support repeatx/y/mirror
                min_filter: filter,
                mag_filter: filter,
                width: width as u32,
                height: height as u32,
                ..Default::default()
            },
        );

        // TODO: support ImageFlags::GENERATE_MIPMAPS) with/without if flags.contains(ImageFlags::NEAREST) {

        let id = self.textures.insert(Texture { tex, format, flags });
        Ok(id)
    }

    fn delete_texture(
        &mut self,
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        img: ImageId,
    ) -> Result<(), NonaError> {
        if let Some(texture) = self.textures.get(img) {
            ctx.delete_texture(texture.tex);
            self.textures.remove(img);
            Ok(())
        } else {
            Err(NonaError::Texture(format!("texture '{}' not found", img)))
        }
    }

    fn update_texture(
        &mut self,
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        img: ImageId,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        data: &[u8],
    ) -> Result<(), NonaError> {
        if let Some(texture) = self.textures.get(img) {
            ctx.texture_update_part(texture.tex, x as _, y as _, width as _, height as _, data);
            Ok(())
        } else {
            Err(NonaError::Texture(format!("texture '{}' not found", img)))
        }
    }

    fn texture_size(
        &self,
        ctx: &(dyn miniquad::RenderingBackend + '_),
        img: ImageId,
    ) -> Result<(usize, usize), NonaError> {
        if let Some(texture) = self.textures.get(img) {
            let (w, h) = ctx.texture_size(texture.tex);
            Ok((w as usize, h as usize))
        } else {
            Err(NonaError::Texture(format!("texture '{}' not found", img)))
        }
    }

    fn viewport(&mut self, extent: Extent, _device_pixel_ratio: f32) -> Result<(), NonaError> {
        self.view = extent;
        Ok(())
    }

    fn clear_screen(&mut self, ctx: &mut (dyn miniquad::RenderingBackend + '_), color: Color) {
        ctx.clear(Some((color.r, color.g, color.b, color.a)), None, None);
    }

    fn ensure_buffer_capacity(
        &mut self,
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        vertex_count: usize,
        index_count: usize,
    ) {
        if vertex_count > self.vertex_buffer_capacity {
            let new_capacity = vertex_count.next_power_of_two().max(MAX_VERTICES);
            let old = self.bindings.vertex_buffers[0];
            self.bindings.vertex_buffers[0] = ctx.new_buffer(
                BufferType::VertexBuffer,
                BufferUsage::Stream,
                BufferSource::empty::<Vertex>(new_capacity),
            );
            ctx.delete_buffer(old);
            self.vertex_buffer_capacity = new_capacity;
        }

        if index_count > self.index_buffer_capacity {
            let new_capacity = index_count.next_power_of_two().max(MAX_INDICES);
            let old = self.bindings.index_buffer;
            self.bindings.index_buffer = ctx.new_buffer(
                BufferType::IndexBuffer,
                BufferUsage::Stream,
                BufferSource::empty::<u16>(new_capacity),
            );
            ctx.delete_buffer(old);
            self.index_buffer_capacity = new_capacity;
        }
    }

    fn ensure_index_buffer_capacity(
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        bindings: &mut Bindings,
        index_buffer_capacity: &mut usize,
        index_count: usize,
    ) {
        if index_count > *index_buffer_capacity {
            let new_capacity = index_count.next_power_of_two().max(MAX_INDICES);
            let old = bindings.index_buffer;
            bindings.index_buffer = ctx.new_buffer(
                BufferType::IndexBuffer,
                BufferUsage::Stream,
                BufferSource::empty::<u16>(new_capacity),
            );
            ctx.delete_buffer(old);
            *index_buffer_capacity = new_capacity;
        }
    }

    fn flush(&mut self, ctx: &mut (dyn miniquad::RenderingBackend + '_)) -> Result<(), NonaError> {
        if self.calls.is_empty() {
            self.vertexes.clear();
            self.paths.clear();
            self.calls.clear();
            self.uniforms.clear();

            return Ok(());
        }
        ctx.begin_default_pass(PassAction::Nothing);

        self.ensure_buffer_capacity(ctx, self.vertexes.len(), 0);

        // Update vertex buffer
        ctx.buffer_update(
            self.bindings.vertex_buffers[0],
            BufferSource::slice(&self.vertexes),
        );

        let mut temp_pipelines: Vec<Pipeline> = Vec::new();
        let calls = &self.calls[..];

        for call in calls {
            let call: &Call = call; // added to make rust-analyzer type inferrence work
            let blend = &call.blend_func;

            // update view size for the uniforms that may be in use
            self.uniforms[call.uniform_offset].view_size = miniquad::window::screen_size();
            if self.uniforms.len() > call.uniform_offset + 1 {
                self.uniforms[call.uniform_offset + 1].view_size = miniquad::window::screen_size();
            }
            let uniforms: &shader::Uniforms = &self.uniforms[call.uniform_offset];
            if let Some(image_index) = call.image {
                self.bindings.images[0] = self.textures[image_index].tex;
            }

            match call.call_type {
                CallType::Fill => {
                    // TODO: test!
                    let paths = &self.paths[call.path_offset..call.path_offset + call.path_count];

                    let uniforms_next: &shader::Uniforms = &self.uniforms[call.uniform_offset + 1];

                    Self::do_fill(
                        ctx,
                        self.shader,
                        blend,
                        call,
                        paths,
                        &mut self.bindings,
                        &mut self.index_buffer_capacity,
                        &mut self.indices,
                        uniforms,
                        uniforms_next,
                        &mut temp_pipelines,
                    );
                }
                CallType::ConvexFill => {
                    let pipeline = Self::make_pipeline(
                        ctx,
                        self.shader,
                        blend,
                        None,
                        CullFace::Back,
                        (true, true, true, true),
                    );
                    ctx.apply_pipeline(&pipeline);
                    temp_pipelines.push(pipeline);
                    ctx.apply_bindings(&self.bindings);

                    let paths = &self.paths[call.path_offset..call.path_offset + call.path_count];

                    Self::do_convex_fill(
                        ctx,
                        call,
                        paths,
                        &mut self.bindings,
                        &mut self.index_buffer_capacity,
                        &mut self.indices,
                        uniforms,
                    );
                }
                CallType::Stroke => {
                    let pipeline = Self::make_pipeline(
                        ctx,
                        self.shader,
                        blend,
                        None,
                        CullFace::Back,
                        (true, true, true, true),
                    );
                    ctx.apply_pipeline(&pipeline);
                    temp_pipelines.push(pipeline);
                    ctx.apply_bindings(&self.bindings);

                    let paths = &self.paths[call.path_offset..call.path_offset + call.path_count];
                    let uniforms_next: &shader::Uniforms = &self.uniforms[call.uniform_offset + 1];

                    Self::do_stroke(
                        ctx,
                        call,
                        paths,
                        &mut self.bindings,
                        &mut self.index_buffer_capacity,
                        &mut self.indices,
                        uniforms,
                        uniforms_next,
                    );
                }
                CallType::Triangles => {
                    let pipeline = Self::make_pipeline(
                        ctx,
                        self.shader,
                        blend,
                        None,
                        CullFace::Back,
                        (true, true, true, true),
                    );
                    ctx.apply_pipeline(&pipeline);
                    temp_pipelines.push(pipeline);
                    ctx.apply_bindings(&self.bindings);

                    Self::do_triangles(
                        ctx,
                        call,
                        &mut self.bindings,
                        &mut self.index_buffer_capacity,
                        &mut self.indices,
                        uniforms,
                    );
                }
            }
        }

        ctx.end_render_pass();

        // Clean up temporary pipelines
        for pipeline in temp_pipelines {
            ctx.delete_pipeline(pipeline);
        }

        self.vertexes.clear();
        self.paths.clear();
        self.calls.clear();
        self.uniforms.clear();
        Ok(())
    }

    fn fill(
        &mut self,
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        paint: &Paint,
        composite_operation: CompositeOperationState,
        scissor: &Scissor,
        fringe: f32,
        bounds: Bounds,
        paths: &[Path],
    ) -> Result<(), NonaError> {
        let mut new_vertex_count = self.vertexes.len();
        for path in paths {
            new_vertex_count += path.get_fill().len();
            new_vertex_count += path.get_stroke().len();
        }

        let call_type = if paths.len() == 1 && paths[0].convex {
            CallType::ConvexFill
        } else {
            CallType::Fill
        };

        if call_type == CallType::Fill {
            new_vertex_count += 4;
        }

        // if GPU overflow
        if new_vertex_count >= MAX_VERTICES {
            self.flush(ctx)?;
        }

        let mut call = Call {
            call_type,
            image: paint.image,
            path_offset: self.paths.len(),
            path_count: paths.len(),
            triangle_offset: 0,
            triangle_count: 4,
            uniform_offset: 0,
            blend_func: composite_operation.into(),
        };

        let mut offset = self.vertexes.len();
        for path in paths {
            let fill = path.get_fill();
            let mut gl_path = GLPath {
                fill_offset: 0,
                fill_count: 0,
                stroke_offset: 0,
                stroke_count: 0,
            };

            if !fill.is_empty() {
                gl_path.fill_offset = offset;
                gl_path.fill_count = fill.len();
                self.vertexes.extend(fill);
                offset += fill.len();
            }

            let stroke = path.get_stroke();
            if !stroke.is_empty() {
                gl_path.stroke_offset = offset;
                gl_path.stroke_count = stroke.len();
                self.vertexes.extend(stroke);
                offset += stroke.len();
            }

            self.paths.push(gl_path);
        }

        if call.call_type == CallType::Fill {
            call.triangle_offset = offset;
            self.vertexes
                .push(Vertex::new(bounds.max.x, bounds.max.y, 0.5, 1.0));
            self.vertexes
                .push(Vertex::new(bounds.max.x, bounds.min.y, 0.5, 1.0));
            self.vertexes
                .push(Vertex::new(bounds.min.x, bounds.max.y, 0.5, 1.0));
            self.vertexes
                .push(Vertex::new(bounds.min.x, bounds.min.y, 0.5, 1.0));

            call.uniform_offset = self.uniforms.len();
            self.append_uniforms(shader::Uniforms {
                stroke_thr: -1.0,
                type_: ShaderType::Simple as i32,
                ..shader::Uniforms::default()
            });
            self.append_uniforms(self.convert_paint(paint, scissor, fringe, fringe, -1.0));
        } else {
            call.uniform_offset = self.uniforms.len();
            self.append_uniforms(self.convert_paint(paint, scissor, fringe, fringe, -1.0));
        }

        self.calls.push(call);
        Ok(())
    }

    fn stroke(
        &mut self,
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        paint: &Paint,
        composite_operation: CompositeOperationState,
        scissor: &Scissor,
        fringe: f32,
        stroke_width: f32,
        paths: &[Path],
    ) -> Result<(), NonaError> {
        let mut new_vertex_count = self.vertexes.len();
        for path in paths {
            new_vertex_count += path.get_stroke().len();
        }

        // if GPU overflow
        if new_vertex_count >= MAX_VERTICES {
            self.flush(ctx)?;
        }

        let mut call = Call {
            call_type: CallType::Stroke,
            image: paint.image,
            path_offset: self.paths.len(),
            path_count: paths.len(),
            triangle_offset: 0,
            triangle_count: 0,
            uniform_offset: 0,
            blend_func: composite_operation.into(),
        };

        let mut offset = self.vertexes.len();
        for path in paths {
            let mut gl_path = GLPath {
                fill_offset: 0,
                fill_count: 0,
                stroke_offset: 0,
                stroke_count: 0,
            };

            let stroke = path.get_stroke();
            if !stroke.is_empty() {
                gl_path.stroke_offset = offset;
                gl_path.stroke_count = stroke.len();
                self.vertexes.extend(stroke);
                offset += stroke.len();
                self.paths.push(gl_path);
            }
        }

        call.uniform_offset = self.uniforms.len();
        self.append_uniforms(self.convert_paint(paint, scissor, stroke_width, fringe, -1.0));
        self.append_uniforms(self.convert_paint(
            paint,
            scissor,
            stroke_width,
            fringe,
            1.0 - 0.5 / 255.0,
        ));

        self.calls.push(call);
        Ok(())
    }

    fn triangles(
        &mut self,
        ctx: &mut (dyn miniquad::RenderingBackend + '_),
        paint: &Paint,
        composite_operation: CompositeOperationState,
        scissor: &Scissor,
        vertexes: &[Vertex],
    ) -> Result<(), NonaError> {
        let mut new_vertex_count = self.vertexes.len();
        new_vertex_count += vertexes.len();

        // if GPU overflow
        if new_vertex_count >= MAX_VERTICES {
            self.flush(ctx)?;
        }

        let call = Call {
            call_type: CallType::Triangles,
            image: paint.image,
            path_offset: 0,
            path_count: 0,
            triangle_offset: self.vertexes.len(),
            triangle_count: vertexes.len(),
            uniform_offset: self.uniforms.len(),
            blend_func: composite_operation.into(),
        };

        self.calls.push(call);
        self.vertexes.extend(vertexes);

        let mut uniforms = self.convert_paint(paint, scissor, 1.0, 1.0, -1.0);
        uniforms.type_ = ShaderType::Image as i32;
        self.append_uniforms(uniforms);
        Ok(())
    }
}

fn convert_blend_factor(factor: nona::BlendFactor) -> miniquad::BlendFactor {
    match factor {
        nona::BlendFactor::Zero => miniquad::BlendFactor::Zero,
        nona::BlendFactor::One => miniquad::BlendFactor::One,

        nona::BlendFactor::SrcColor => miniquad::BlendFactor::Value(BlendValue::SourceColor),
        nona::BlendFactor::OneMinusSrcColor => {
            miniquad::BlendFactor::OneMinusValue(BlendValue::SourceColor)
        }
        nona::BlendFactor::DstColor => miniquad::BlendFactor::Value(BlendValue::DestinationColor),
        nona::BlendFactor::OneMinusDstColor => {
            miniquad::BlendFactor::OneMinusValue(BlendValue::DestinationColor)
        }

        nona::BlendFactor::SrcAlpha => miniquad::BlendFactor::Value(BlendValue::SourceAlpha),
        nona::BlendFactor::OneMinusSrcAlpha => {
            miniquad::BlendFactor::OneMinusValue(BlendValue::SourceAlpha)
        }
        nona::BlendFactor::DstAlpha => miniquad::BlendFactor::Value(BlendValue::DestinationAlpha),
        nona::BlendFactor::OneMinusDstAlpha => {
            miniquad::BlendFactor::OneMinusValue(BlendValue::DestinationAlpha)
        }

        nona::BlendFactor::SrcAlphaSaturate => miniquad::BlendFactor::SourceAlphaSaturate,
    }
}

#[inline]
fn premul_color(color: Color) -> Color {
    Color {
        r: color.r * color.a,
        g: color.g * color.a,
        b: color.b * color.a,
        a: color.a,
    }
}

#[inline]
fn _xform_to_3x4(xform: Transform) -> [f32; 12] {
    // 3 col 4 rows
    let mut m = [0f32; 12];
    let t = &xform.0;
    m[0] = t[0];
    m[1] = t[1];
    m[2] = 0.0;
    m[3] = 0.0;
    m[4] = t[2];
    m[5] = t[3];
    m[6] = 0.0;
    m[7] = 0.0;
    m[8] = t[4];
    m[9] = t[5];
    m[10] = 1.0;
    m[11] = 0.0;
    m
}

#[inline]
fn xform_to_4x4(xform: Transform) -> Mat4 {
    let t = &xform.0;

    Mat4::from_cols(
        Vec4::new(t[0], t[1], 0.0, 0.0),
        Vec4::new(t[2], t[3], 0.0, 0.0),
        Vec4::new(t[4], t[5], 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 0.0),
    )
}
