//! WebGL2 çizici.
//!
//! Chunk başına tek bir interleaved vertex buffer (konum + normal + uv + renk)
//! ve iki katman (katı / sıvı) tutulur. Kare başına önce görüş konisi (frustum)
//! elemesi yapılır, sonra katı ve sıvı katmanlar sırayla çizilir.

use std::collections::HashMap;

use wasm_bindgen::JsCast;
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as GL, WebGlBuffer, WebGlProgram, WebGlShader,
    WebGlTexture, WebGlUniformLocation, WebGlVertexArrayObject,
};

use crate::atlas::{ATLAS_H, ATLAS_W};
use crate::mat4::{self, Frustum};
use crate::mesh::MeshData;

/// Vertex başına `f32` sayısı: konum(3) + normal(3) + uv(2) + renk(3).
const FLOATS_PER_VERTEX: usize = 11;
const STRIDE: i32 = (FLOATS_PER_VERTEX * 4) as i32;

const MAIN_VS: &str = r#"#version 300 es
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_nor;
layout(location = 2) in vec2 a_uv;
layout(location = 3) in vec3 a_col;
uniform mat4 u_viewProj;
uniform vec3 u_camPos;
out vec2 v_uv;
out vec3 v_col;
out float v_dist;
void main() {
  gl_Position = u_viewProj * vec4(a_pos, 1.0);
  v_uv = a_uv;
  v_col = a_col;
  v_dist = distance(a_pos, u_camPos);
}
"#;

const MAIN_FS: &str = r#"#version 300 es
precision mediump float;
in vec2 v_uv;
in vec3 v_col;
in float v_dist;
uniform sampler2D u_atlas;
uniform vec3 u_fogColor;
uniform float u_fogNear;
uniform float u_fogFar;
uniform float u_opacity;
uniform float u_alphaTest;
out vec4 fragColor;
void main() {
  vec4 t = texture(u_atlas, v_uv);
  if (t.a < u_alphaTest) discard;
  vec3 c = t.rgb * v_col;
  float f = clamp((v_dist - u_fogNear) / max(u_fogFar - u_fogNear, 0.001), 0.0, 1.0);
  fragColor = vec4(mix(c, u_fogColor, f), t.a * u_opacity);
}
"#;

const SKY_VS: &str = r#"#version 300 es
layout(location = 0) in vec2 a_xy;
out vec2 v_ndc;
void main() { v_ndc = a_xy; gl_Position = vec4(a_xy, 1.0, 1.0); }
"#;

/// Gökyüzü tek bir tam ekran üçgenle çizilir; gradyan ve bulutlar fragment
/// gölgelendiricisinde ışın yönünden hesaplanır (ek geometri gerekmez).
const SKY_FS: &str = r#"#version 300 es
precision highp float;
in vec2 v_ndc;
uniform vec3 u_fwd;
uniform vec3 u_right;
uniform vec3 u_up;
uniform float u_tanHalfFov;
uniform float u_aspect;
uniform vec3 u_top;
uniform vec3 u_horizon;
uniform float u_time;
out vec4 fragColor;

float hash(vec2 p) {
  p = fract(p * vec2(123.34, 345.45));
  p += dot(p, p + 34.345);
  return fract(p.x * p.y);
}
float vnoise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  float a = hash(i);
  float b = hash(i + vec2(1.0, 0.0));
  float c = hash(i + vec2(0.0, 1.0));
  float d = hash(i + vec2(1.0, 1.0));
  return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}
float fbm(vec2 p) {
  float s = 0.0;
  float a = 0.5;
  for (int i = 0; i < 4; i++) { s += vnoise(p) * a; p *= 2.03; a *= 0.5; }
  return s;
}

void main() {
  vec3 dir = normalize(u_fwd
    + u_right * v_ndc.x * u_tanHalfFov * u_aspect
    + u_up * v_ndc.y * u_tanHalfFov);

  float h = clamp(dir.y * 1.55 + 0.12, 0.0, 1.0);
  vec3 sky = mix(u_horizon, u_top, pow(h, 0.8));

  if (dir.y > 0.03) {
    vec2 cp = dir.xz / max(dir.y, 0.03) * 0.32 + vec2(u_time * 0.006, u_time * 0.0022);
    float n = fbm(cp * 1.7);
    float cl = smoothstep(0.50, 0.80, n) * smoothstep(0.03, 0.28, dir.y);
    sky = mix(sky, vec3(1.0), cl * 0.82);
  }

  fragColor = vec4(sky, 1.0);
}
"#;

struct GpuLayer {
    vao: WebGlVertexArrayObject,
    vbo: WebGlBuffer,
    ibo: WebGlBuffer,
    count: i32,
}

struct GpuChunk {
    solid: Option<GpuLayer>,
    liquid: Option<GpuLayer>,
    center: [f32; 3],
    radius: f32,
}

pub struct Renderer {
    gl: GL,
    prog: WebGlProgram,
    sky_prog: WebGlProgram,
    atlas: WebGlTexture,
    sky_vao: WebGlVertexArrayObject,

    u_view_proj: Option<WebGlUniformLocation>,
    u_cam_pos: Option<WebGlUniformLocation>,
    u_fog_color: Option<WebGlUniformLocation>,
    u_fog_near: Option<WebGlUniformLocation>,
    u_fog_far: Option<WebGlUniformLocation>,
    u_opacity: Option<WebGlUniformLocation>,
    u_alpha_test: Option<WebGlUniformLocation>,

    s_fwd: Option<WebGlUniformLocation>,
    s_right: Option<WebGlUniformLocation>,
    s_up: Option<WebGlUniformLocation>,
    s_tan: Option<WebGlUniformLocation>,
    s_aspect: Option<WebGlUniformLocation>,
    s_top: Option<WebGlUniformLocation>,
    s_horizon: Option<WebGlUniformLocation>,
    s_time: Option<WebGlUniformLocation>,

    chunks: HashMap<u64, GpuChunk>,
    pub fog_color: [f32; 3],
    pub sky_top: [f32; 3],
    pub sky_horizon: [f32; 3],
    pub stats_draws: u32,
    pub stats_tris: u32,
}

fn compile(gl: &GL, kind: u32, src: &str) -> Result<WebGlShader, String> {
    let sh = gl.create_shader(kind).ok_or("shader oluşturulamadı")?;
    gl.shader_source(&sh, src);
    gl.compile_shader(&sh);
    let ok = gl
        .get_shader_parameter(&sh, GL::COMPILE_STATUS)
        .as_bool()
        .unwrap_or(false);
    if !ok {
        let log = gl.get_shader_info_log(&sh).unwrap_or_default();
        return Err(format!("shader derlenemedi: {log}"));
    }
    Ok(sh)
}

fn link(gl: &GL, vs_src: &str, fs_src: &str) -> Result<WebGlProgram, String> {
    let vs = compile(gl, GL::VERTEX_SHADER, vs_src)?;
    let fs = compile(gl, GL::FRAGMENT_SHADER, fs_src)?;
    let prog = gl.create_program().ok_or("program oluşturulamadı")?;
    gl.attach_shader(&prog, &vs);
    gl.attach_shader(&prog, &fs);
    gl.link_program(&prog);
    let ok = gl
        .get_program_parameter(&prog, GL::LINK_STATUS)
        .as_bool()
        .unwrap_or(false);
    if !ok {
        let log = gl.get_program_info_log(&prog).unwrap_or_default();
        return Err(format!("program bağlanamadı: {log}"));
    }
    gl.delete_shader(Some(&vs));
    gl.delete_shader(Some(&fs));
    Ok(prog)
}

fn interleave(m: &MeshData) -> Vec<f32> {
    let n = m.pos.len() / 3;
    let mut v = Vec::with_capacity(n * FLOATS_PER_VERTEX);
    for i in 0..n {
        v.extend_from_slice(&m.pos[i * 3..i * 3 + 3]);
        v.extend_from_slice(&m.nor[i * 3..i * 3 + 3]);
        v.extend_from_slice(&m.uv[i * 2..i * 2 + 2]);
        v.extend_from_slice(&m.col[i * 3..i * 3 + 3]);
    }
    v
}

impl Renderer {
    pub fn new(canvas: &HtmlCanvasElement) -> Result<Self, String> {
        let gl = canvas
            .get_context("webgl2")
            .map_err(|_| "webgl2 bağlamı alınamadı")?
            .ok_or("tarayıcı WebGL2 desteklemiyor")?
            .dyn_into::<GL>()
            .map_err(|_| "bağlam WebGL2'ye çevrilemedi")?;

        let prog = link(&gl, MAIN_VS, MAIN_FS)?;
        let sky_prog = link(&gl, SKY_VS, SKY_FS)?;

        // --- doku atlası ---
        let atlas_pixels = crate::atlas::build_atlas();
        let atlas = gl.create_texture().ok_or("doku oluşturulamadı")?;
        gl.bind_texture(GL::TEXTURE_2D, Some(&atlas));
        gl.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_u8_array(
            GL::TEXTURE_2D,
            0,
            GL::RGBA as i32,
            ATLAS_W as i32,
            ATLAS_H as i32,
            0,
            GL::RGBA,
            GL::UNSIGNED_BYTE,
            Some(&atlas_pixels),
        )
        .map_err(|_| "atlas dokusu yüklenemedi")?;
        // Piksel sanatı: en yakın komşu örnekleme, mipmap yok.
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::NEAREST as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::NEAREST as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_S, GL::CLAMP_TO_EDGE as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_T, GL::CLAMP_TO_EDGE as i32);

        // --- gökyüzü: tam ekran üçgen ---
        let sky_vao = gl.create_vertex_array().ok_or("gökyüzü VAO oluşturulamadı")?;
        let sky_vbo = gl.create_buffer().ok_or("gökyüzü tamponu oluşturulamadı")?;
        gl.bind_vertex_array(Some(&sky_vao));
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&sky_vbo));
        let tri: [f32; 6] = [-1.0, -1.0, 3.0, -1.0, -1.0, 3.0];
        let tri_arr = js_sys::Float32Array::from(&tri[..]);
        gl.buffer_data_with_array_buffer_view(GL::ARRAY_BUFFER, &tri_arr, GL::STATIC_DRAW);
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 2, GL::FLOAT, false, 0, 0);
        gl.bind_vertex_array(None);

        let u = |name: &str| gl.get_uniform_location(&prog, name);
        let su = |name: &str| gl.get_uniform_location(&sky_prog, name);

        let r = Self {
            u_view_proj: u("u_viewProj"),
            u_cam_pos: u("u_camPos"),
            u_fog_color: u("u_fogColor"),
            u_fog_near: u("u_fogNear"),
            u_fog_far: u("u_fogFar"),
            u_opacity: u("u_opacity"),
            u_alpha_test: u("u_alphaTest"),

            s_fwd: su("u_fwd"),
            s_right: su("u_right"),
            s_up: su("u_up"),
            s_tan: su("u_tanHalfFov"),
            s_aspect: su("u_aspect"),
            s_top: su("u_top"),
            s_horizon: su("u_horizon"),
            s_time: su("u_time"),

            gl,
            prog,
            sky_prog,
            atlas,
            sky_vao,
            chunks: HashMap::new(),
            fog_color: [0.81, 0.90, 0.98],
            sky_top: [0.35, 0.65, 0.94],
            sky_horizon: [0.81, 0.90, 0.98],
            stats_draws: 0,
            stats_tris: 0,
        };
        Ok(r)
    }

    fn create_layer(&self, m: &MeshData) -> Option<GpuLayer> {
        if m.is_empty() {
            return None;
        }
        let gl = &self.gl;
        let vao = gl.create_vertex_array()?;
        let vbo = gl.create_buffer()?;
        let ibo = gl.create_buffer()?;

        gl.bind_vertex_array(Some(&vao));

        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&vbo));
        let verts = interleave(m);
        let varr = js_sys::Float32Array::from(&verts[..]);
        gl.buffer_data_with_array_buffer_view(GL::ARRAY_BUFFER, &varr, GL::STATIC_DRAW);

        gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(&ibo));
        let iarr = js_sys::Uint32Array::from(&m.idx[..]);
        gl.buffer_data_with_array_buffer_view(GL::ELEMENT_ARRAY_BUFFER, &iarr, GL::STATIC_DRAW);

        let stride = STRIDE;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 3, GL::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_with_i32(1, 3, GL::FLOAT, false, stride, 12);
        gl.enable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_with_i32(2, 2, GL::FLOAT, false, stride, 24);
        gl.enable_vertex_attrib_array(3);
        gl.vertex_attrib_pointer_with_i32(3, 3, GL::FLOAT, false, stride, 32);

        gl.bind_vertex_array(None);
        gl.bind_buffer(GL::ARRAY_BUFFER, None);
        gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, None);

        Some(GpuLayer { vao, vbo, ibo, count: m.idx.len() as i32 })
    }

    fn destroy_layer(&self, l: &GpuLayer) {
        self.gl.delete_vertex_array(Some(&l.vao));
        self.gl.delete_buffer(Some(&l.vbo));
        self.gl.delete_buffer(Some(&l.ibo));
    }

    /// Chunk'ı GPU'ya yükler; eski tamponlar serbest bırakılır.
    pub fn upload_chunk(&mut self, key: u64, solid: &MeshData, liquid: &MeshData) {
        let center = {
            let (c, r) = solid.bounding_sphere();
            if r > 0.0 {
                (c, r)
            } else {
                let (c2, r2) = liquid.bounding_sphere();
                (c2, r2)
            }
        };

        let new_solid = self.create_layer(solid);
        let new_liquid = self.create_layer(liquid);

        if let Some(old) = self.chunks.remove(&key) {
            if let Some(l) = old.solid {
                self.destroy_layer(&l);
            }
            if let Some(l) = old.liquid {
                self.destroy_layer(&l);
            }
        }

        self.chunks.insert(
            key,
            GpuChunk { solid: new_solid, liquid: new_liquid, center: center.0, radius: center.1 },
        );
    }

    pub fn remove_chunk(&mut self, key: u64) {
        if let Some(old) = self.chunks.remove(&key) {
            if let Some(l) = old.solid {
                self.destroy_layer(&l);
            }
            if let Some(l) = old.liquid {
                self.destroy_layer(&l);
            }
        }
    }

    pub fn resize(&self, w: i32, h: i32) {
        self.gl.viewport(0, 0, w, h);
    }

    /// Bir kareyi çizer.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        cam_pos: [f32; 3],
        yaw: f32,
        pitch: f32,
        fov_y: f32,
        aspect: f32,
        far: f32,
        fog_near: f32,
        fog_far: f32,
        time: f32,
    ) {
        let gl = &self.gl;
        let view = mat4::view(yaw, pitch, cam_pos);
        let proj = mat4::perspective(fov_y, aspect, 0.05, far);
        let view_proj = mat4::mul(&proj, &view);
        let frustum = Frustum::from_view_proj(&view_proj);

        gl.enable(GL::DEPTH_TEST);
        gl.depth_func(GL::LEQUAL);
        gl.enable(GL::CULL_FACE);
        gl.cull_face(GL::BACK);
        gl.clear_color(self.sky_horizon[0], self.sky_horizon[1], self.sky_horizon[2], 1.0);
        gl.clear(GL::COLOR_BUFFER_BIT | GL::DEPTH_BUFFER_BIT);

        // --- gökyüzü (derinlik yazmadan, en arkada) ---
        gl.disable(GL::DEPTH_TEST);
        gl.depth_mask(false);
        gl.use_program(Some(&self.sky_prog));
        let dir = [
            -yaw.sin() * pitch.cos(),
            pitch.sin(),
            -yaw.cos() * pitch.cos(),
        ];
        let right = [yaw.cos(), 0.0, -yaw.sin()];
        let up = [
            right[1] * dir[2] - right[2] * dir[1],
            right[2] * dir[0] - right[0] * dir[2],
            right[0] * dir[1] - right[1] * dir[0],
        ];
        gl.uniform3f(self.s_fwd.as_ref(), dir[0], dir[1], dir[2]);
        gl.uniform3f(self.s_right.as_ref(), right[0], right[1], right[2]);
        gl.uniform3f(self.s_up.as_ref(), up[0], up[1], up[2]);
        gl.uniform1f(self.s_tan.as_ref(), (fov_y * 0.5).tan());
        gl.uniform1f(self.s_aspect.as_ref(), aspect);
        gl.uniform3f(self.s_top.as_ref(), self.sky_top[0], self.sky_top[1], self.sky_top[2]);
        gl.uniform3f(
            self.s_horizon.as_ref(),
            self.sky_horizon[0],
            self.sky_horizon[1],
            self.sky_horizon[2],
        );
        gl.uniform1f(self.s_time.as_ref(), time);
        gl.bind_vertex_array(Some(&self.sky_vao));
        gl.draw_arrays(GL::TRIANGLES, 0, 3);
        gl.depth_mask(true);
        gl.enable(GL::DEPTH_TEST);

        // --- dünya ---
        gl.use_program(Some(&self.prog));
        gl.uniform_matrix4fv_with_f32_array(self.u_view_proj.as_ref(), false, &view_proj);
        gl.uniform3f(self.u_cam_pos.as_ref(), cam_pos[0], cam_pos[1], cam_pos[2]);
        gl.uniform3f(
            self.u_fog_color.as_ref(),
            self.fog_color[0],
            self.fog_color[1],
            self.fog_color[2],
        );
        gl.uniform1f(self.u_fog_near.as_ref(), fog_near);
        gl.uniform1f(self.u_fog_far.as_ref(), fog_far);
        gl.active_texture(GL::TEXTURE0);
        gl.bind_texture(GL::TEXTURE_2D, Some(&self.atlas));
        gl.uniform1i(gl.get_uniform_location(&self.prog, "u_atlas").as_ref(), 0);

        // Görüş konisi dışındaki chunk'lar hiç çizilmez.
        let visible: Vec<u64> = self
            .chunks
            .iter()
            .filter(|(_, c)| frustum.sphere_visible(c.center, c.radius))
            .map(|(k, _)| *k)
            .collect();

        let mut draws = 0u32;
        let mut tris = 0u32;

        // 1) katı katman
        gl.disable(GL::BLEND);
        gl.uniform1f(self.u_opacity.as_ref(), 1.0);
        gl.uniform1f(self.u_alpha_test.as_ref(), 0.5);
        for key in &visible {
            if let Some(l) = &self.chunks[key].solid {
                gl.bind_vertex_array(Some(&l.vao));
                gl.draw_elements_with_i32(GL::TRIANGLES, l.count, GL::UNSIGNED_INT, 0);
                draws += 1;
                tris += l.count as u32 / 3;
            }
        }

        // 2) sıvı katman (saydam)
        gl.enable(GL::BLEND);
        gl.blend_func(GL::SRC_ALPHA, GL::ONE_MINUS_SRC_ALPHA);
        gl.uniform1f(self.u_opacity.as_ref(), 0.82);
        gl.uniform1f(self.u_alpha_test.as_ref(), 0.01);
        for key in &visible {
            if let Some(l) = &self.chunks[key].liquid {
                gl.bind_vertex_array(Some(&l.vao));
                gl.draw_elements_with_i32(GL::TRIANGLES, l.count, GL::UNSIGNED_INT, 0);
                draws += 1;
                tris += l.count as u32 / 3;
            }
        }
        gl.disable(GL::BLEND);
        gl.bind_vertex_array(None);

        self.stats_draws = draws;
        self.stats_tris = tris;
    }
}
