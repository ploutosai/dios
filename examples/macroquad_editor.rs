use dios::embedded::macroquad::Editor;
use macroquad::prelude::*;

fn build_material(
    fragment_shader: &str,
    pipeline_params: PipelineParams,
) -> Result<Material, String> {
    load_material(
        ShaderSource::Glsl {
            vertex: DEFAULT_VERTEX_SHADER,
            fragment: fragment_shader,
        },
        MaterialParams {
            pipeline_params,
            ..Default::default()
        },
    )
    .map_err(|err| format!("{err:#?}"))
}

fn draw_multiline_text(text: &str, x: f32, y: f32, font_size: f32, color: Color, max_lines: usize) {
    for (i, line) in text.lines().take(max_lines).enumerate() {
        draw_text(line, x, y + i as f32 * (font_size + 4.0), font_size, color);
    }
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Dios embedded in macroquad".to_string(),
        window_width: 1280,
        window_height: 720,
        high_dpi: true,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let mut editor = Editor::new("fragment.glsl", DEFAULT_FRAGMENT_SHADER);

    let ferris = load_texture("examples/ferris.png").await.unwrap();
    let pipeline_params = PipelineParams {
        depth_write: true,
        depth_test: Comparison::LessOrEqual,
        ..Default::default()
    };
    let mut material =
        build_material(DEFAULT_FRAGMENT_SHADER, pipeline_params).expect("default shader");
    let mut error: Option<String> = None;

    loop {
        editor.update();
        if let Some(fragment_shader) = editor.take_changed_text() {
            match build_material(&fragment_shader, pipeline_params) {
                Ok(new_material) => {
                    material = new_material;
                    error = None;
                }
                Err(err) => {
                    error = Some(err);
                }
            }
        }

        clear_background(Color::new(0.06, 0.065, 0.09, 1.0));

        let scene_x = (screen_width() * 0.52) as i32;
        let scene_w = (screen_width() as i32 - scene_x).max(1);
        let scene_h = screen_height() as i32;
        let t = get_time() as f32;
        let camera = Camera3D {
            position: vec3(-15.0 + t.sin() * 2.0, 15.0,-5.0),
            up: vec3(0.0, 1.0, 0.0),
            target: vec3(0.0, 5.0, -5.0),
            ..Default::default()
        };

        set_camera(&camera);
        draw_grid(
            20,
            1.0,
            Color::new(0.45, 0.45, 0.55, 0.75),
            Color::new(0.65, 0.65, 0.75, 0.75),
        );
        gl_use_material(&material);
        draw_sphere(vec3(0.0, 6.0, 0.0), 5.0, Some(&ferris), WHITE);
        gl_use_default_material();

        set_default_camera();
        let label_x = scene_x as f32 + 24.0;
        draw_text(
            "macroquad shader preview",
            label_x,
            36.0,
            28.0,
            Color::new(0.9, 0.9, 0.95, 1.0),
        );
        draw_text(
            "Edit fragment.glsl in the embedded Dios editor.",
            label_x,
            68.0,
            20.0,
            Color::new(0.72, 0.74, 0.82, 1.0),
        );
        if let Some(error) = &error {
            draw_text("shader compile error:", label_x, 108.0, 20.0, RED);
            draw_multiline_text(
                error,
                label_x,
                136.0,
                16.0,
                Color::new(1.0, 0.55, 0.55, 1.0),
                18,
            );
        }

        editor.draw(100.0, 100.0, screen_width() * 0.48, screen_height() - 200.0);
        next_frame().await;
    }
}

const DEFAULT_FRAGMENT_SHADER: &str = r#"#version 100
precision lowp float;

varying vec2 uv;

uniform sampler2D Texture;

void main() {
    vec4 tex = texture2D(Texture, uv);
    vec3 tint = vec3(0.65 + 0.35 * uv.x, 0.75, 1.0 - 0.35 * uv.y);
    gl_FragColor = vec4(tex.rgb * tint, tex.a);
}
"#;

const DEFAULT_VERTEX_SHADER: &str = r#"#version 100
precision lowp float;

attribute vec3 position;
attribute vec2 texcoord;

varying vec2 uv;

uniform mat4 Model;
uniform mat4 Projection;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    uv = texcoord;
}
"#;
