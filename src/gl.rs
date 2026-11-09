//! WebGL2 çizici.
//!
//! Chunk başına iki katman (katı / sıvı) ve katman başına tek bir vertex
//! tamponu tutulur; vertex biçimi `mesh` modülünde anlatılan 8 baytlık
//! paketlenmiş biçimdir. İndeksler tüm chunk'lar için ortak tek bir tampondan
//! gelir (her yüz aynı `0,1,2, 0,2,3` deseni).
//!
//! Çizim **kameraya göredir**: chunk konumları `f64` ile kameradan çıkarılır ve
//! gölgelendiriciye chunk başına bir ofset olarak verilir.
//!
//! Kare sırası: katı katman (önden arkaya) → gökyüzü (yalnızca boş kalan
//! pikseller) → sıvı katman (arkadan öne).

use std::collections::HashMap;

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as GL, WebGlBuffer, WebGlProgram, WebGlShader,
    WebGlTexture, WebGlUniformLocation, WebGlVertexArrayObject,
};

use crate::atlas::{ATLAS_COLS, ATLAS_H, ATLAS_W, TILE};
use crate::effects::Particle;
use crate::mat4::{self, Frustum};
use crate::mesh::{MeshBuffers, MeshData, CORNER_SHIFT, FACES, VERTEX_BYTES};
use crate::world::CHUNK_SIZE;

const STRIDE: i32 = VERTEX_BYTES as i32;

/// Ortak indeks tamponunun başlangıç kapasitesi (yüz sayısı). Daha büyük bir
/// chunk geldiğinde tampon büyütülür.
const MIN_QUAD_CAPACITY: u32 = 4096;

// Gölgelendiricideki sabitler (`16.0`, `15.0`, `256.0`) bu değerlere göredir.
const _: () = assert!(TILE == 16 && ATLAS_COLS == 16 && CORNER_SHIFT == 4);

/// `a_pos.w` baytı `döşeme + 16 * köşe` taşır; uv burada köşeden üretilir.
/// Döşeme kenarında komşu döşemeye taşmayı önlemek için yarım piksel içeriden
/// örneklenir.
const MAIN_VS: &str = r#"#version 300 es
layout(location = 0) in vec4 a_pos;
layout(location = 1) in vec3 a_col;
uniform mat4 u_viewProj;
uniform vec3 u_origin;
uniform vec2 u_camMod;
out vec2 v_uv;
out vec3 v_col;
out float v_dist;
out highp vec2 v_wpos;
void main() {
  vec3 p = a_pos.xyz + u_origin;
  gl_Position = u_viewProj * vec4(p, 1.0);
  // Dünya konumu (64 blokta bir saran): su parıltısı kamerayla kaymasın.
  v_wpos = p.xz + u_camMod;
  float corner = floor(a_pos.w / 16.0);
  float tile = a_pos.w - corner * 16.0;
  float cu = (corner == 1.0 || corner == 2.0) ? 1.0 : 0.0;
  float cv = corner >= 2.0 ? 1.0 : 0.0;
  v_uv = vec2((tile * 16.0 + 0.5 + cu * 15.0) / 256.0, (0.5 + cv * 15.0) / 16.0);
  v_col = a_col;
  v_dist = length(p);
}
"#;

const MAIN_FS: &str = r#"#version 300 es
precision mediump float;
in vec2 v_uv;
in vec3 v_col;
in float v_dist;
in highp vec2 v_wpos;
uniform sampler2D u_atlas;
uniform float u_water;
uniform float u_time;
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
  if (u_water > 0.5) {
    // Dalga boyları 64'ün tam bölenleri: sarma noktasında dikiş oluşmaz.
    highp vec2 w = floor(v_wpos * 8.0) / 8.0 * 0.0981748;
    float ripple = sin(w.x * 11.0 + u_time * 1.4) * sin(w.y * 7.0 - u_time * 1.1)
                 + 0.6 * sin((w.x + w.y) * 17.0 + u_time * 2.0);
    c *= 1.0 + 0.07 * ripple;
  }
  float f = clamp((v_dist - u_fogNear) / max(u_fogFar - u_fogNear, 0.001), 0.0, 1.0);
  fragColor = vec4(mix(c, u_fogColor, f), t.a * u_opacity);
}
"#;

/// Gökyüzü uzak düzlemde (`z = w`) çizilir; derinlik testi sayesinde yalnızca
/// arazinin kapatmadığı piksellerde çalışır.
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

  // Güneş: keskin disk + geniş, yumuşak hale.
  float sun = max(dot(dir, normalize(vec3(0.60, 0.58, 0.55))), 0.0);
  sky += vec3(1.0, 0.82, 0.55) * pow(sun, 10.0) * 0.16;
  sky = mix(sky, vec3(1.0, 0.97, 0.86), smoothstep(0.9988, 0.9993, sun));

  if (dir.y > 0.03) {
    vec2 cp = dir.xz / max(dir.y, 0.03) * 0.32 + vec2(u_time * 0.006, u_time * 0.0022);
    float n = fbm(cp * 1.7);
    float cl = smoothstep(0.50, 0.80, n) * smoothstep(0.03, 0.28, dir.y);
    sky = mix(sky, vec3(1.0), cl * 0.82);
  }

  fragColor = vec4(sky, 1.0);
}
"#;

/// Kazılan bloğun üstüne çizilen çatlak kaplaması: bloktan bir tık büyük bir
/// küp. Desen dokudan değil, 16×16'lık "piksel" ızgarasında gürültüden
/// üretilir ve ilerleme arttıkça merkezden kenarlara yayılır.
const CRACK_VS: &str = r#"#version 300 es
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec2 a_uv;
uniform mat4 u_viewProj;
uniform vec3 u_origin;
out vec2 v_uv;
void main() {
  vec3 p = (a_pos - 0.5) * 1.004 + 0.5 + u_origin;
  gl_Position = u_viewProj * vec4(p, 1.0);
  v_uv = a_uv;
}
"#;

const CRACK_FS: &str = r#"#version 300 es
precision mediump float;
in vec2 v_uv;
uniform float u_progress;
out vec4 fragColor;
float hash(vec2 p) {
  p = fract(p * vec2(123.34, 345.45));
  p += dot(p, p + 34.345);
  return fract(p.x * p.y);
}
void main() {
  // Bakılan bloğun ince çerçevesi.
  vec2 edge = min(v_uv, 1.0 - v_uv);
  if (min(edge.x, edge.y) < 0.018) { fragColor = vec4(0.0, 0.0, 0.0, 0.55); return; }
  vec2 cell = floor(v_uv * 16.0);
  float d = length((cell + 0.5) / 16.0 - 0.5);
  float t = hash(cell) * 0.6 + d * 0.8;
  if (t > u_progress * 0.85) discard;
  fragColor = vec4(0.0, 0.0, 0.0, 0.5);
}
"#;

/// Kırılma parçacıkları: dünya boyutuna göre ölçeklenen kare noktalar.
const PARTICLE_VS: &str = r#"#version 300 es
layout(location = 0) in vec3 a_pos;
layout(location = 1) in float a_size;
layout(location = 2) in vec3 a_col;
uniform mat4 u_viewProj;
uniform float u_pixelScale;
out vec3 v_col;
void main() {
  gl_Position = u_viewProj * vec4(a_pos, 1.0);
  gl_PointSize = max(a_size * u_pixelScale / gl_Position.w, 1.0);
  v_col = a_col;
}
"#;

const PARTICLE_FS: &str = r#"#version 300 es
precision mediump float;
in vec3 v_col;
out vec4 fragColor;
void main() { fragColor = vec4(v_col, 1.0); }
"#;

/// Parçacık başına bayt: konum (3×f32) + boyut (f32) + renk (4×u8).
const PARTICLE_BYTES: usize = 20;

/// Bir karede çizilecek kırma efektleri.
pub struct FrameEffects<'a> {
    /// Bakılan blok (çerçevesi çizilir) ve kazma ilerlemesi (0..1).
    pub crack: Option<([i32; 3], f32)>,
    /// Göz suyun içinde: mavi, yakın bir sis kullanılır.
    pub underwater: bool,
    pub particles: &'a [Particle],
}

struct GpuLayer {
    vao: WebGlVertexArrayObject,
    vbo: WebGlBuffer,
    quads: i32,
}

struct GpuChunk {
    key: u64,
    cx: i32,
    cz: i32,
    solid: Option<GpuLayer>,
    liquid: Option<GpuLayer>,
    /// İki katmanın birlikte kapladığı `y` aralığı (görüş konisi elemesi).
    y_min: f32,
    y_max: f32,
}

/// Bir karede görünen chunk: kameraya uzaklığı ve kameraya göre ofseti.
struct Visible {
    dist2: f32,
    index: u32,
    origin: [f32; 3],
}

pub struct Renderer {
    gl: GL,
    prog: WebGlProgram,
    sky_prog: WebGlProgram,
    atlas: WebGlTexture,
    sky_vao: WebGlVertexArrayObject,
    /// Tüm chunk'ların paylaştığı `0,1,2, 0,2,3` indeks tamponu.
    quad_ibo: WebGlBuffer,
    quad_capacity: u32,

    crack_prog: WebGlProgram,
    crack_vao: WebGlVertexArrayObject,
    c_view_proj: Option<WebGlUniformLocation>,
    c_origin: Option<WebGlUniformLocation>,
    c_progress: Option<WebGlUniformLocation>,
    particle_prog: WebGlProgram,
    particle_vao: WebGlVertexArrayObject,
    particle_vbo: WebGlBuffer,
    p_view_proj: Option<WebGlUniformLocation>,
    p_pixel_scale: Option<WebGlUniformLocation>,
    viewport_h: f32,

    u_view_proj: Option<WebGlUniformLocation>,
    u_origin: Option<WebGlUniformLocation>,
    u_cam_mod: Option<WebGlUniformLocation>,
    u_water: Option<WebGlUniformLocation>,
    u_time: Option<WebGlUniformLocation>,
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

    /// Yoğun dizi + anahtar → indeks eşlemesi: kare başına gezinme `HashMap`
    /// araması gerektirmez.
    chunks: Vec<GpuChunk>,
    index: HashMap<u64, usize>,
    /// Kareler arasında yeniden kullanılan görünür chunk listesi.
    visible: Vec<Visible>,

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

fn destroy_layer(gl: &GL, l: &GpuLayer) {
    gl.delete_vertex_array(Some(&l.vao));
    gl.delete_buffer(Some(&l.vbo));
}

fn create_layer(gl: &GL, quad_ibo: &WebGlBuffer) -> Option<GpuLayer> {
    let vao = gl.create_vertex_array()?;
    let Some(vbo) = gl.create_buffer() else {
        gl.delete_vertex_array(Some(&vao));
        return None;
    };
    gl.bind_vertex_array(Some(&vao));
    gl.bind_buffer(GL::ARRAY_BUFFER, Some(&vbo));
    // Eleman tamponu bağlaması VAO'nun durumudur: ortak tampon bir kez bağlanır.
    gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(quad_ibo));
    // 0: konum (x, y, z) + döşeme/köşe baytı — tamsayı değerler olduğu gibi.
    gl.enable_vertex_attrib_array(0);
    gl.vertex_attrib_pointer_with_i32(0, 4, GL::UNSIGNED_BYTE, false, STRIDE, 0);
    // 1: renk — 0..255, 0..1 aralığına normalize edilir.
    gl.enable_vertex_attrib_array(1);
    gl.vertex_attrib_pointer_with_i32(1, 3, GL::UNSIGNED_BYTE, true, STRIDE, 4);
    gl.bind_vertex_array(None);
    Some(GpuLayer { vao, vbo, quads: 0 })
}

/// Katmanın geometrisini GPU'ya yazar. Var olan VAO/VBO yeniden kullanılır
/// (blok kırıp koyarken nesne üretip silmek gerekmez); mesh boşsa katman
/// silinir.
fn upload_layer(gl: &GL, quad_ibo: &WebGlBuffer, slot: &mut Option<GpuLayer>, m: &MeshData) {
    if m.is_empty() {
        if let Some(old) = slot.take() {
            destroy_layer(gl, &old);
        }
        return;
    }
    if slot.is_none() {
        *slot = create_layer(gl, quad_ibo);
    }
    let Some(layer) = slot.as_mut() else { return };
    gl.bind_buffer(GL::ARRAY_BUFFER, Some(&layer.vbo));
    // Dilim, wasm belleğine bakan bir görünüm olarak geçer: araya kopya girmez.
    gl.buffer_data_with_u8_array(GL::ARRAY_BUFFER, &m.verts, GL::STATIC_DRAW);
    layer.quads = m.face_count() as i32;
}

impl Renderer {
    pub fn new(canvas: &HtmlCanvasElement) -> Result<Self, String> {
        // Tuval her karede tümüyle opak çizilir; alfa kanalı istenmezse
        // tarayıcı tuvali sayfayla harmanlamak zorunda kalmaz. (Alfa açıkken
        // yarı saydam su, sayfanın arka plan rengini de içeri sızdırıyordu.)
        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &JsValue::from_str("alpha"), &JsValue::FALSE)
            .map_err(|_| "bağlam seçenekleri kurulamadı")?;
        let gl = canvas
            .get_context_with_context_options("webgl2", &options)
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

        let quad_ibo = gl.create_buffer().ok_or("indeks tamponu oluşturulamadı")?;

        // --- çatlak kaplaması: birim küp (konum + uv), 36 köşe ---
        let crack_prog = link(&gl, CRACK_VS, CRACK_FS)?;
        let crack_vao = gl.create_vertex_array().ok_or("çatlak VAO oluşturulamadı")?;
        let crack_vbo = gl.create_buffer().ok_or("çatlak tamponu oluşturulamadı")?;
        let mut cube: Vec<f32> = Vec::with_capacity(36 * 5);
        for face in FACES.iter() {
            for corner in [0usize, 1, 2, 0, 2, 3] {
                let p = face.verts[corner];
                let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]][corner];
                cube.extend_from_slice(&[p[0] as f32, p[1] as f32, p[2] as f32, uv[0], uv[1]]);
            }
        }
        gl.bind_vertex_array(Some(&crack_vao));
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&crack_vbo));
        let cube_arr = js_sys::Float32Array::from(&cube[..]);
        gl.buffer_data_with_array_buffer_view(GL::ARRAY_BUFFER, &cube_arr, GL::STATIC_DRAW);
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 3, GL::FLOAT, false, 20, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_with_i32(1, 2, GL::FLOAT, false, 20, 12);

        // --- parçacıklar: her karede yeniden doldurulan nokta tamponu ---
        let particle_prog = link(&gl, PARTICLE_VS, PARTICLE_FS)?;
        let particle_vao = gl.create_vertex_array().ok_or("parçacık VAO oluşturulamadı")?;
        let particle_vbo = gl.create_buffer().ok_or("parçacık tamponu oluşturulamadı")?;
        gl.bind_vertex_array(Some(&particle_vao));
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&particle_vbo));
        let stride = PARTICLE_BYTES as i32;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 3, GL::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_with_i32(1, 1, GL::FLOAT, false, stride, 12);
        gl.enable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_with_i32(2, 3, GL::UNSIGNED_BYTE, true, stride, 16);
        gl.bind_vertex_array(None);

        // Atlas her zaman 0. doku biriminde durur; örnekleyici bir kez ayarlanır.
        gl.use_program(Some(&prog));
        gl.uniform1i(gl.get_uniform_location(&prog, "u_atlas").as_ref(), 0);

        let u = |name: &str| gl.get_uniform_location(&prog, name);
        let su = |name: &str| gl.get_uniform_location(&sky_prog, name);

        let mut r = Self {
            u_view_proj: u("u_viewProj"),
            u_origin: u("u_origin"),
            u_cam_mod: u("u_camMod"),
            u_water: u("u_water"),
            u_time: u("u_time"),
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

            c_view_proj: gl.get_uniform_location(&crack_prog, "u_viewProj"),
            c_origin: gl.get_uniform_location(&crack_prog, "u_origin"),
            c_progress: gl.get_uniform_location(&crack_prog, "u_progress"),
            p_view_proj: gl.get_uniform_location(&particle_prog, "u_viewProj"),
            p_pixel_scale: gl.get_uniform_location(&particle_prog, "u_pixelScale"),
            crack_prog,
            crack_vao,
            particle_prog,
            particle_vao,
            particle_vbo,
            viewport_h: 1.0,

            gl,
            prog,
            sky_prog,
            atlas,
            sky_vao,
            quad_ibo,
            quad_capacity: 0,
            chunks: Vec::new(),
            index: HashMap::new(),
            visible: Vec::new(),
            fog_color: [0.81, 0.90, 0.98],
            sky_top: [0.35, 0.65, 0.94],
            sky_horizon: [0.81, 0.90, 0.98],
            stats_draws: 0,
            stats_tris: 0,
        };
        r.ensure_quad_capacity(MIN_QUAD_CAPACITY);
        Ok(r)
    }

    /// Ortak indeks tamponunu en az `quads` yüzü çizebilecek kadar büyütür.
    /// VAO'lar tampon *nesnesine* bağlı olduğundan içeriğin yeniden
    /// tanımlanması onları geçersiz kılmaz.
    fn ensure_quad_capacity(&mut self, quads: u32) {
        if quads <= self.quad_capacity {
            return;
        }
        let capacity = quads.next_power_of_two().max(MIN_QUAD_CAPACITY);
        let mut indices: Vec<u32> = Vec::with_capacity(capacity as usize * 6);
        for q in 0..capacity {
            let b = q * 4;
            indices.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
        let gl = &self.gl;
        // Eleman tamponu bağlaması etkin VAO'ya yazılır; bir chunk'ınkini
        // bozmamak için varsayılan VAO'ya geçilir.
        gl.bind_vertex_array(None);
        gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(&self.quad_ibo));
        let view = js_sys::Uint32Array::from(&indices[..]);
        gl.buffer_data_with_array_buffer_view(GL::ELEMENT_ARRAY_BUFFER, &view, GL::STATIC_DRAW);
        self.quad_capacity = capacity;
    }

    /// Chunk'ın mesh'ini GPU'ya yükler (ilk kez ya da güncelleme).
    pub fn upload_chunk(&mut self, key: u64, cx: i32, cz: i32, mesh: &MeshBuffers) {
        let Some((y_min, y_max)) = mesh.y_bounds() else {
            // Tümüyle boş mesh: çizilecek bir şey yok.
            self.remove_chunk(key);
            return;
        };
        self.ensure_quad_capacity(mesh.solid.face_count().max(mesh.liquid.face_count()));

        let slot = match self.index.get(&key) {
            Some(&i) => i,
            None => {
                self.chunks.push(GpuChunk { key, cx, cz, solid: None, liquid: None, y_min: 0.0, y_max: 0.0 });
                self.index.insert(key, self.chunks.len() - 1);
                self.chunks.len() - 1
            }
        };
        let chunk = &mut self.chunks[slot];
        chunk.y_min = y_min as f32;
        chunk.y_max = y_max as f32;
        upload_layer(&self.gl, &self.quad_ibo, &mut chunk.solid, &mesh.solid);
        upload_layer(&self.gl, &self.quad_ibo, &mut chunk.liquid, &mesh.liquid);
        self.gl.bind_buffer(GL::ARRAY_BUFFER, None);
    }

    pub fn remove_chunk(&mut self, key: u64) {
        let Some(slot) = self.index.remove(&key) else { return };
        let old = self.chunks.swap_remove(slot);
        // `swap_remove` son elemanı boşalan yere taşır; indeksini güncelle.
        if let Some(moved) = self.chunks.get(slot) {
            self.index.insert(moved.key, slot);
        }
        for layer in [&old.solid, &old.liquid].into_iter().flatten() {
            destroy_layer(&self.gl, layer);
        }
    }

    /// GPU'da mesh'i bulunan chunk sayısı.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn resize(&mut self, w: i32, h: i32) {
        self.gl.viewport(0, 0, w, h);
        self.viewport_h = h.max(1) as f32;
    }

    /// Bir kareyi çizer.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        cam_pos: [f64; 3],
        yaw: f32,
        pitch: f32,
        fov_y: f32,
        aspect: f32,
        far: f32,
        fog_near: f32,
        fog_far: f32,
        time: f32,
        fx: &FrameEffects,
    ) {
        // Kamera orijinde: matriste öteleme yok, chunk ofsetleri aşağıda.
        let view = mat4::view_rotation(yaw, pitch);
        let proj = mat4::perspective(fov_y, aspect, 0.05, far);
        let view_proj = mat4::mul(&proj, &view);
        let frustum = Frustum::from_view_proj(&view_proj);

        // --- görünür chunk'lar: görüş konisi elemesi + uzaklığa göre sıralama ---
        let size = CHUNK_SIZE as f64;
        let mut visible = std::mem::take(&mut self.visible);
        visible.clear();
        for (i, c) in self.chunks.iter().enumerate() {
            // Çıkarma `f64` ile yapılır; sonuç küçük olduğundan `f32`'ye
            // çevrilince hassasiyet kaybolmaz.
            let origin = [
                (c.cx as f64 * size - cam_pos[0]) as f32,
                -cam_pos[1] as f32,
                (c.cz as f64 * size - cam_pos[2]) as f32,
            ];
            let min = [origin[0], origin[1] + c.y_min, origin[2]];
            let max = [origin[0] + size as f32, origin[1] + c.y_max, origin[2] + size as f32];
            if frustum.aabb_visible(min, max) {
                let half = size as f32 * 0.5;
                let (dx, dz) = (origin[0] + half, origin[2] + half);
                visible.push(Visible { dist2: dx * dx + dz * dz, index: i as u32, origin });
            }
        }
        visible.sort_unstable_by(|a, b| a.dist2.total_cmp(&b.dist2));

        // Su altında: gökyüzü yerine su rengi, sis yakın ve mavi.
        let water_color = [0.10, 0.32, 0.58];
        let (fog_color, fog_near, fog_far) =
            if fx.underwater { (water_color, 0.0, fog_far.min(28.0)) } else { (self.fog_color, fog_near, fog_far) };
        let clear = if fx.underwater { water_color } else { self.sky_horizon };

        let gl = &self.gl;
        gl.enable(GL::DEPTH_TEST);
        gl.depth_func(GL::LEQUAL);
        gl.depth_mask(true);
        gl.enable(GL::CULL_FACE);
        gl.cull_face(GL::BACK);
        gl.clear_color(clear[0], clear[1], clear[2], 1.0);
        gl.clear(GL::COLOR_BUFFER_BIT | GL::DEPTH_BUFFER_BIT);

        gl.use_program(Some(&self.prog));
        gl.uniform_matrix4fv_with_f32_array(self.u_view_proj.as_ref(), false, &view_proj);
        gl.uniform3f(self.u_fog_color.as_ref(), fog_color[0], fog_color[1], fog_color[2]);
        gl.uniform2f(
            self.u_cam_mod.as_ref(),
            cam_pos[0].rem_euclid(64.0) as f32,
            cam_pos[2].rem_euclid(64.0) as f32,
        );
        gl.uniform1f(self.u_time.as_ref(), time);
        gl.uniform1f(self.u_water.as_ref(), 0.0);
        gl.uniform1f(self.u_fog_near.as_ref(), fog_near);
        gl.uniform1f(self.u_fog_far.as_ref(), fog_far);
        gl.active_texture(GL::TEXTURE0);
        gl.bind_texture(GL::TEXTURE_2D, Some(&self.atlas));

        let mut draws = 0u32;
        let mut tris = 0u32;
        let mut draw = |layer: &GpuLayer, origin: &[f32; 3]| {
            gl.uniform3f(self.u_origin.as_ref(), origin[0], origin[1], origin[2]);
            gl.bind_vertex_array(Some(&layer.vao));
            gl.draw_elements_with_i32(GL::TRIANGLES, layer.quads * 6, GL::UNSIGNED_INT, 0);
            draws += 1;
            tris += layer.quads as u32 * 2;
        };

        // 1) Katı katman, önden arkaya: yakındaki arazi derinliği önce yazar,
        //    arkasında kalan pikseller gölgelendirilmeden elenir.
        gl.disable(GL::BLEND);
        gl.uniform1f(self.u_opacity.as_ref(), 1.0);
        gl.uniform1f(self.u_alpha_test.as_ref(), 0.5);
        for v in visible.iter() {
            if let Some(layer) = &self.chunks[v.index as usize].solid {
                draw(layer, &v.origin);
            }
        }

        // 2) Gökyüzü, uzak düzlemde ve derinlik yazmadan. Araziden sonra
        //    çizildiği için bulut gürültüsü yalnızca açık kalan piksellerde
        //    hesaplanır (derinlik tamponu 1.0 olan yerler, `LEQUAL` ile geçer).
        gl.depth_mask(false);
        gl.use_program(Some(&self.sky_prog));
        let sky_vertices = if fx.underwater { 0 } else { 3 };
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
        gl.draw_arrays(GL::TRIANGLES, 0, sky_vertices);
        gl.depth_mask(true);

        // 2b) Kırma efektleri: parçacıklar opak, çatlak kaplaması harmanlı.
        //     Sudan önce çizilirler ki su altındaki blokta suyun arkasında kalsınlar.
        if !fx.particles.is_empty() {
            let mut bytes: Vec<u8> = Vec::with_capacity(fx.particles.len() * PARTICLE_BYTES);
            for p in fx.particles {
                for a in 0..3 {
                    bytes.extend_from_slice(&((p.pos[a] - cam_pos[a]) as f32).to_le_bytes());
                }
                bytes.extend_from_slice(&p.size.to_le_bytes());
                bytes.extend_from_slice(&[p.color[0], p.color[1], p.color[2], 255]);
            }
            gl.use_program(Some(&self.particle_prog));
            gl.uniform_matrix4fv_with_f32_array(self.p_view_proj.as_ref(), false, &view_proj);
            // Dünya birimi → piksel: ekran yüksekliği / (2·tan(fov/2)).
            let scale = self.viewport_h / (2.0 * (fov_y * 0.5).tan());
            gl.uniform1f(self.p_pixel_scale.as_ref(), scale);
            gl.bind_vertex_array(Some(&self.particle_vao));
            gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.particle_vbo));
            gl.buffer_data_with_u8_array(GL::ARRAY_BUFFER, &bytes, GL::DYNAMIC_DRAW);
            gl.draw_arrays(GL::POINTS, 0, fx.particles.len() as i32);
            gl.bind_buffer(GL::ARRAY_BUFFER, None);
        }
        if let Some((block, progress)) = fx.crack {
            gl.use_program(Some(&self.crack_prog));
            gl.uniform_matrix4fv_with_f32_array(self.c_view_proj.as_ref(), false, &view_proj);
            gl.uniform3f(
                self.c_origin.as_ref(),
                (block[0] as f64 - cam_pos[0]) as f32,
                (block[1] as f64 - cam_pos[1]) as f32,
                (block[2] as f64 - cam_pos[2]) as f32,
            );
            gl.uniform1f(self.c_progress.as_ref(), progress.clamp(0.0, 1.0));
            gl.enable(GL::BLEND);
            gl.blend_func(GL::SRC_ALPHA, GL::ONE_MINUS_SRC_ALPHA);
            gl.depth_mask(false);
            gl.bind_vertex_array(Some(&self.crack_vao));
            gl.draw_arrays(GL::TRIANGLES, 0, 36);
            gl.depth_mask(true);
        }

        // 3) Sıvı katman (saydam), arkadan öne: harmanlama doğru sırayla olur.
        gl.use_program(Some(&self.prog));
        gl.enable(GL::BLEND);
        gl.blend_func(GL::SRC_ALPHA, GL::ONE_MINUS_SRC_ALPHA);
        gl.uniform1f(self.u_opacity.as_ref(), 0.82);
        gl.uniform1f(self.u_water.as_ref(), 1.0);
        gl.uniform1f(self.u_alpha_test.as_ref(), 0.01);
        for v in visible.iter().rev() {
            if let Some(layer) = &self.chunks[v.index as usize].liquid {
                draw(layer, &v.origin);
            }
        }
        gl.disable(GL::BLEND);
        gl.bind_vertex_array(None);

        self.stats_draws = draws;
        self.stats_tris = tris;
        self.visible = visible;
    }
}
