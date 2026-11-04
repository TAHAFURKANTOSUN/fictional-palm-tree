//! WebGL2 çizici.
//!
//! Rust ↔ GPU sınırı bilerek ince tutulur; her kare ve her yükleme mümkün
//! olan en az JS çağrısı, en az bayt ve sıfır JS-tarafı ayırmayla yapılır:
//!
//! * **Paketlenmiş köşe** — 8 bayt/köşe ([`mesh::Vertex`]): chunk-yerel konum
//!   (`u8×3`, tamsayı öznitelik) + döşeme/köşe kodu (`u8`) + renk (`unorm8×4`).
//!   UV gölgelendiricide hesaplanır; gölgelendiricinin hiç okumadığı normal
//!   gönderilmez. (Eskiden 44 bayt/köşe + chunk başına u32 indeks tamponuydu.)
//! * **Kopyasız yükleme** — köşe baytları wasm belleğinin bir görünümü olarak
//!   (`&[u8]`) doğrudan `bufferData`'ya gider; JS tarafında ara
//!   `Float32Array`/`Uint32Array` ayrılmaz.
//! * **Paylaşılan indeks tamponu** — her yüz 4 ardışık köşe olduğundan tüm
//!   chunk'lar aynı dörtgen indeks tamponunu kullanır (u16; 65 536 köşeyi aşan
//!   nadir chunk'lar için büyüyebilen u32).
//! * **Chunk orijini köşe tamponunun başlığında** — her VBO'nun ilk 8 baytı
//!   chunk'ın dünya orijinidir ve `divisor = 1` ile okunur; çizim başına ek
//!   `uniform` çağrısı gerekmez (çizim = `bindVertexArray` + `drawElements`).
//! * **Kameraya göre konum** — büyük koordinatlarda (±50 000) titreme olmaz.
//! * **Tampon havuzu** — kaldırılan chunk'ların VAO/VBO'ları yeniden kullanılır;
//!   remesh yeni GL nesnesi oluşturmaz.
//! * **Sabit durum bir kez** — derinlik/cull/blend/doku/örnekleyici/sis rengi
//!   başlangıçta ayarlanır; kare başına yalnızca değişen değerler gönderilir.
//! * **Sıra:** katı (önden arkaya, erken-z) → gökyüzü (yalnızca boş kalan
//!   piksellerde; pahalı bulut gürültüsü artık tüm ekranda çalışmaz) →
//!   sıvı (arkadan öne, doğru saydamlık).
//! * **Kutu elemesi** — görüş konisi testi küre yerine eksen hizalı kutuyla.

use std::collections::HashMap;

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as GL, WebGlBuffer, WebGlProgram, WebGlShader,
    WebGlTexture, WebGlUniformLocation, WebGlVertexArrayObject,
};

use crate::atlas::{ATLAS_H, ATLAS_W, TILE};
use crate::mat4::{self, Frustum};
use crate::mesh::{MeshData, QUAD_PATTERN, VERTEX_BYTES};
use crate::world::CHUNK_SIZE;

/// VBO başlığı: chunk orijini `(x, z)` — iki `f32`.
const HEADER_BYTES: usize = 8;
const STRIDE: i32 = VERTEX_BYTES as i32;

/// u16 indeksle çizilebilecek en fazla dörtgen (65 536 köşe).
const QUADS_U16: u32 = 1 << 14;

/// Havuzda tutulacak en fazla boş katman (VAO + VBO).
const POOL_MAX: usize = 48;

/// Konum öznitelikleri.
const A_LOCAL: u32 = 0;
const A_COLOR: u32 = 1;
const A_ORIGIN: u32 = 2;

const MAIN_VS: &str = r#"
layout(location = 0) in uvec4 a_local;  // chunk-yerel x, y, z ; döşeme*4 + köşe
layout(location = 1) in vec4 a_col;     // unorm8 renk
layout(location = 2) in vec2 a_origin;  // chunk orijini (x, z), divisor = 1
uniform mat4 u_viewProj;                // ötelemesiz (kameraya göre)
uniform vec3 u_camPos;
out vec2 v_uv;
out vec3 v_col;
out float v_dist;
void main() {
  vec3 rel = vec3(a_origin.x - u_camPos.x, -u_camPos.y, a_origin.y - u_camPos.z)
           + vec3(a_local.xyz);
  gl_Position = u_viewProj * vec4(rel, 1.0);

  // UV: döşemenin yarım piksel içeriden kenarları (komşu döşemeye taşmasın).
  float tile = float(a_local.w >> 2u);
  vec2 corner = vec2(float(a_local.w & 1u), float((a_local.w >> 1u) & 1u));
  v_uv = vec2(
    (tile * TILE_PX + 0.5 + corner.x * (TILE_PX - 1.0)) / ATLAS_W_PX,
    (0.5 + corner.y * (TILE_PX - 1.0)) / ATLAS_H_PX);

  v_col = a_col.rgb;
  v_dist = length(rel);
}
"#;

const MAIN_FS: &str = r#"
precision mediump float;
in vec2 v_uv;
in vec3 v_col;
in float v_dist;
uniform sampler2D u_atlas;
uniform vec3 u_fogColor;
uniform vec2 u_fog;        // yakın, uzak
uniform vec2 u_alpha;      // opaklık, alfa eşiği
out vec4 fragColor;
void main() {
  vec4 t = texture(u_atlas, v_uv);
  if (t.a < u_alpha.y) discard;
  vec3 c = t.rgb * v_col;
  float f = clamp((v_dist - u_fog.x) / max(u_fog.y - u_fog.x, 0.001), 0.0, 1.0);
  fragColor = vec4(mix(c, u_fogColor, f), t.a * u_alpha.x);
}
"#;

const SKY_VS: &str = r#"
layout(location = 0) in vec2 a_xy;
out vec2 v_ndc;
void main() { v_ndc = a_xy; gl_Position = vec4(a_xy, 1.0, 1.0); }
"#;

/// Gökyüzü tek bir tam ekran üçgenle çizilir; gradyan ve bulutlar fragment
/// gölgelendiricisinde ışın yönünden hesaplanır (ek geometri gerekmez).
/// Derinliği 1.0'dır: katı dünyadan **sonra** çizilir ve yalnızca hiçbir
/// geometrinin kaplamadığı piksellerde çalışır.
const SKY_FS: &str = r#"
precision highp float;
in vec2 v_ndc;
uniform mat3 u_basis;      // sütunlar: sağ, yukarı, ileri
uniform vec3 u_params;     // tan(fov/2), en-boy oranı, zaman
uniform vec3 u_top;
uniform vec3 u_horizon;
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
  vec3 dir = normalize(u_basis * vec3(
    v_ndc.x * u_params.x * u_params.y,
    v_ndc.y * u_params.x,
    1.0));

  float h = clamp(dir.y * 1.55 + 0.12, 0.0, 1.0);
  vec3 sky = mix(u_horizon, u_top, pow(h, 0.8));

  if (dir.y > 0.03) {
    float t = u_params.z;
    vec2 cp = dir.xz / max(dir.y, 0.03) * 0.32 + vec2(t * 0.006, t * 0.0022);
    float n = fbm(cp * 1.7);
    float cl = smoothstep(0.50, 0.80, n) * smoothstep(0.03, 0.28, dir.y);
    sky = mix(sky, vec3(1.0), cl * 0.82);
  }

  fragColor = vec4(sky, 1.0);
}
"#;

const FOG_COLOR: [f32; 3] = [0.81, 0.90, 0.98];
const SKY_TOP: [f32; 3] = [0.35, 0.65, 0.94];
const SKY_HORIZON: [f32; 3] = [0.81, 0.90, 0.98];

/// GPU'daki bir chunk katmanı (katı ya da sıvı).
struct GpuLayer {
    vao: WebGlVertexArrayObject,
    vbo: WebGlBuffer,
    /// Çizilecek indeks sayısı (dörtgen × 6).
    count: i32,
    /// VAO'ya bağlı paylaşılan indeks tamponunun türü.
    index_type: u32,
    /// Chunk-yerel kapsayıcı kutu.
    bmin: [f32; 3],
    bmax: [f32; 3],
}

struct GpuChunk {
    key: u64,
    /// Dünya orijini (x, z).
    origin: [f64; 2],
    solid: Option<GpuLayer>,
    liquid: Option<GpuLayer>,
}

pub struct Renderer {
    gl: GL,
    prog: WebGlProgram,
    sky_prog: WebGlProgram,
    sky_vao: WebGlVertexArrayObject,
    /// TEXTURE0'a kalıcı olarak bağlı atlas (tutamaç canlı kalsın).
    _atlas: WebGlTexture,

    u_view_proj: Option<WebGlUniformLocation>,
    u_cam_pos: Option<WebGlUniformLocation>,
    u_fog: Option<WebGlUniformLocation>,
    u_alpha: Option<WebGlUniformLocation>,
    s_basis: Option<WebGlUniformLocation>,
    s_params: Option<WebGlUniformLocation>,
    /// Son gönderilen sis aralığı (değişmedikçe yeniden gönderilmez).
    fog_sent: [f32; 2],

    /// Paylaşılan dörtgen indeks tamponları.
    ibo16: WebGlBuffer,
    ibo32: Option<WebGlBuffer>,
    ibo32_quads: u32,

    /// Yoğun dizi: kare başına doğrudan gezilir (HashMap gezintisi yok).
    chunks: Vec<GpuChunk>,
    slot: HashMap<u64, usize>,
    pool: Vec<GpuLayer>,

    /// Yükleme için yeniden kullanılan bayt tamponu (başlık + köşeler).
    upload: Vec<u8>,
    /// Kare başına yeniden kullanılan görünür liste: (uzaklık², chunk indeksi).
    vis_solid: Vec<(f32, u32)>,
    vis_liquid: Vec<(f32, u32)>,

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

/// Gölgelendiricilere ortak başlığı (sürüm + atlas sabitleri) ekleyip bağlar.
fn link(gl: &GL, vs_src: &str, fs_src: &str) -> Result<WebGlProgram, String> {
    let header = format!(
        "#version 300 es\n#define TILE_PX {TILE}.0\n#define ATLAS_W_PX {ATLAS_W}.0\n#define ATLAS_H_PX {ATLAS_H}.0\n"
    );
    let vs = compile(gl, GL::VERTEX_SHADER, &(header.clone() + vs_src))?;
    let fs = compile(gl, GL::FRAGMENT_SHADER, &(header + fs_src))?;
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

/// `quads` dörtgenlik indeks deseni, küçük-sonlu (little-endian) baytlar.
fn quad_index_bytes(quads: u32, wide: bool) -> Vec<u8> {
    let per = if wide { 4 } else { 2 };
    let mut out = Vec::with_capacity(quads as usize * QUAD_PATTERN.len() * per);
    for q in 0..quads {
        for i in QUAD_PATTERN {
            let v = q * 4 + i;
            if wide {
                out.extend_from_slice(&v.to_le_bytes());
            } else {
                out.extend_from_slice(&(v as u16).to_le_bytes());
            }
        }
    }
    out
}

impl Renderer {
    pub fn new(canvas: &HtmlCanvasElement, atlas_pixels: &[u8]) -> Result<Self, String> {
        // `alpha: false`: tuval opak bileştirilir (sayfa arka planıyla karışma
        // ve bileştirme maliyeti yok). `powerPreference`: çift GPU'lu
        // dizüstülerde ayrık GPU tercih edilir.
        let attrs = js_sys::Object::new();
        let set = |k: &str, v: JsValue| js_sys::Reflect::set(&attrs, &JsValue::from_str(k), &v);
        set("alpha", JsValue::FALSE).map_err(|_| "bağlam ayarı yapılamadı")?;
        set("powerPreference", JsValue::from_str("high-performance"))
            .map_err(|_| "bağlam ayarı yapılamadı")?;

        let gl = canvas
            .get_context_with_context_options("webgl2", &attrs)
            .map_err(|_| "webgl2 bağlamı alınamadı")?
            .ok_or("tarayıcı WebGL2 desteklemiyor")?
            .dyn_into::<GL>()
            .map_err(|_| "bağlam WebGL2'ye çevrilemedi")?;

        let prog = link(&gl, MAIN_VS, MAIN_FS)?;
        let sky_prog = link(&gl, SKY_VS, SKY_FS)?;

        // --- doku atlası (tek doku, TEXTURE0'a kalıcı olarak bağlı) ---
        let atlas = gl.create_texture().ok_or("doku oluşturulamadı")?;
        gl.active_texture(GL::TEXTURE0);
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
            Some(atlas_pixels),
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
        let tri_bytes: Vec<u8> = tri.iter().flat_map(|v| v.to_le_bytes()).collect();
        gl.buffer_data_with_u8_array(GL::ARRAY_BUFFER, &tri_bytes, GL::STATIC_DRAW);
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 2, GL::FLOAT, false, 0, 0);
        gl.bind_vertex_array(None);

        // --- paylaşılan u16 dörtgen indeks tamponu ---
        let ibo16 = gl.create_buffer().ok_or("indeks tamponu oluşturulamadı")?;
        gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(&ibo16));
        gl.buffer_data_with_u8_array(
            GL::ELEMENT_ARRAY_BUFFER,
            &quad_index_bytes(QUADS_U16, false),
            GL::STATIC_DRAW,
        );
        gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, None);
        gl.bind_buffer(GL::ARRAY_BUFFER, None);

        // --- sabit boru hattı durumu: bir kez ---
        gl.enable(GL::DEPTH_TEST);
        gl.depth_func(GL::LEQUAL);
        gl.enable(GL::CULL_FACE);
        gl.cull_face(GL::BACK);
        gl.blend_func(GL::SRC_ALPHA, GL::ONE_MINUS_SRC_ALPHA);
        gl.clear_color(SKY_HORIZON[0], SKY_HORIZON[1], SKY_HORIZON[2], 1.0);

        // --- sabit uniform'lar: bir kez ---
        gl.use_program(Some(&sky_prog));
        let su = |name: &str| gl.get_uniform_location(&sky_prog, name);
        gl.uniform3fv_with_f32_array(su("u_top").as_ref(), &SKY_TOP);
        gl.uniform3fv_with_f32_array(su("u_horizon").as_ref(), &SKY_HORIZON);
        let s_basis = su("u_basis");
        let s_params = su("u_params");

        gl.use_program(Some(&prog));
        let u = |name: &str| gl.get_uniform_location(&prog, name);
        gl.uniform1i(u("u_atlas").as_ref(), 0);
        gl.uniform3fv_with_f32_array(u("u_fogColor").as_ref(), &FOG_COLOR);
        let u_view_proj = u("u_viewProj");
        let u_cam_pos = u("u_camPos");
        let u_fog = u("u_fog");
        let u_alpha = u("u_alpha");

        Ok(Self {
            gl,
            prog,
            sky_prog,
            sky_vao,
            _atlas: atlas,
            u_view_proj,
            u_cam_pos,
            u_fog,
            u_alpha,
            s_basis,
            s_params,
            fog_sent: [f32::NAN; 2],
            ibo16,
            ibo32: None,
            ibo32_quads: 0,
            chunks: Vec::new(),
            slot: HashMap::new(),
            pool: Vec::new(),
            upload: Vec::new(),
            vis_solid: Vec::new(),
            vis_liquid: Vec::new(),
            stats_draws: 0,
            stats_tris: 0,
        })
    }

    // ---------------------------------------------------------------
    // Tamponlar
    // ---------------------------------------------------------------

    /// Havuzdan bir katman alır ya da yenisini kurar. Öznitelik düzeni VAO'da
    /// saklandığı için havuzdan gelen katman yalnızca veri yüklemesi ister.
    fn acquire_layer(&mut self) -> Option<GpuLayer> {
        if let Some(l) = self.pool.pop() {
            return Some(l);
        }
        let gl = &self.gl;
        let vao = gl.create_vertex_array()?;
        let vbo = gl.create_buffer()?;
        gl.bind_vertex_array(Some(&vao));
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&vbo));

        let h = HEADER_BYTES as i32;
        gl.enable_vertex_attrib_array(A_LOCAL);
        gl.vertex_attrib_i_pointer_with_i32(A_LOCAL, 4, GL::UNSIGNED_BYTE, STRIDE, h);
        gl.enable_vertex_attrib_array(A_COLOR);
        gl.vertex_attrib_pointer_with_i32(A_COLOR, 4, GL::UNSIGNED_BYTE, true, STRIDE, h + 4);
        // Orijin: başlıktaki tek kayıt; örnek (instance) başına okunur.
        gl.enable_vertex_attrib_array(A_ORIGIN);
        gl.vertex_attrib_pointer_with_i32(A_ORIGIN, 2, GL::FLOAT, false, 0, 0);
        gl.vertex_attrib_divisor(A_ORIGIN, 1);

        gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(&self.ibo16));
        gl.bind_vertex_array(None);
        gl.bind_buffer(GL::ARRAY_BUFFER, None);

        Some(GpuLayer {
            vao,
            vbo,
            count: 0,
            index_type: GL::UNSIGNED_SHORT,
            bmin: [0.0; 3],
            bmax: [0.0; 3],
        })
    }

    fn release_layer(&mut self, l: GpuLayer) {
        if self.pool.len() < POOL_MAX {
            self.pool.push(l);
        } else {
            self.gl.delete_vertex_array(Some(&l.vao));
            self.gl.delete_buffer(Some(&l.vbo));
        }
    }

    /// u32 indeks tamponunu en az `quads` dörtgene büyütür. Aynı tampon
    /// nesnesi korunur; ona bağlı VAO'lar geçerli kalır.
    fn ensure_ibo32(&mut self, quads: u32) -> Option<WebGlBuffer> {
        if self.ibo32.is_none() {
            self.ibo32 = Some(self.gl.create_buffer()?);
        }
        let ibo = self.ibo32.clone()?;
        if self.ibo32_quads < quads {
            let n = quads.next_power_of_two().max(QUADS_U16 * 2);
            let gl = &self.gl;
            // VAO bağlıyken ELEMENT_ARRAY_BUFFER bağlamak o VAO'yu değiştirir.
            gl.bind_vertex_array(None);
            gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(&ibo));
            gl.buffer_data_with_u8_array(
                GL::ELEMENT_ARRAY_BUFFER,
                &quad_index_bytes(n, true),
                GL::STATIC_DRAW,
            );
            gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, None);
            self.ibo32_quads = n;
        }
        Some(ibo)
    }

    /// Bir katmanı mesh verisiyle günceller. Boş mesh katmanı havuza döndürür.
    fn write_layer(
        &mut self,
        existing: Option<GpuLayer>,
        m: &MeshData,
        origin: [f64; 2],
    ) -> Option<GpuLayer> {
        let Some((bmin, bmax)) = m.local_bounds() else {
            if let Some(l) = existing {
                self.release_layer(l);
            }
            return None;
        };
        let mut layer = match existing {
            Some(l) => l,
            None => self.acquire_layer()?,
        };

        // Başlık + köşeler tek tampona; `&[u8]` wasm belleğinin bir görünümü
        // olarak geçer (JS'te kopya ya da ayırma yok).
        self.upload.clear();
        self.upload.extend_from_slice(&(origin[0] as f32).to_le_bytes());
        self.upload.extend_from_slice(&(origin[1] as f32).to_le_bytes());
        self.upload.extend_from_slice(m.as_bytes());
        self.gl.bind_buffer(GL::ARRAY_BUFFER, Some(&layer.vbo));
        self.gl.buffer_data_with_u8_array(GL::ARRAY_BUFFER, &self.upload, GL::STATIC_DRAW);
        self.gl.bind_buffer(GL::ARRAY_BUFFER, None);

        let quads = m.face_count();
        let wide = quads > QUADS_U16;
        let ibo = if wide { Some(self.ensure_ibo32(quads)?) } else { None };
        let want = if wide { GL::UNSIGNED_INT } else { GL::UNSIGNED_SHORT };
        if layer.index_type != want {
            self.gl.bind_vertex_array(Some(&layer.vao));
            self.gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(ibo.as_ref().unwrap_or(&self.ibo16)));
            self.gl.bind_vertex_array(None);
            layer.index_type = want;
        }

        layer.count = m.index_count() as i32;
        layer.bmin = bmin.map(f32::from);
        layer.bmax = bmax.map(f32::from);
        Some(layer)
    }

    /// Chunk'ı GPU'ya yükler. Mevcut chunk'ın tamponları yerinde güncellenir.
    pub fn upload_chunk(&mut self, key: u64, cx: i32, cz: i32, solid: &MeshData, liquid: &MeshData) {
        let origin = [(cx * CHUNK_SIZE) as f64, (cz * CHUNK_SIZE) as f64];
        let idx = match self.slot.get(&key) {
            Some(&i) => i,
            None => {
                self.chunks.push(GpuChunk { key, origin, solid: None, liquid: None });
                self.slot.insert(key, self.chunks.len() - 1);
                self.chunks.len() - 1
            }
        };
        self.chunks[idx].origin = origin;

        let old = self.chunks[idx].solid.take();
        let new = self.write_layer(old, solid, origin);
        self.chunks[idx].solid = new;

        let old = self.chunks[idx].liquid.take();
        let new = self.write_layer(old, liquid, origin);
        self.chunks[idx].liquid = new;
    }

    pub fn remove_chunk(&mut self, key: u64) {
        let Some(i) = self.slot.remove(&key) else { return };
        let c = self.chunks.swap_remove(i);
        if let Some(moved) = self.chunks.get(i) {
            self.slot.insert(moved.key, i);
        }
        for l in [c.solid, c.liquid].into_iter().flatten() {
            self.release_layer(l);
        }
    }

    pub fn resize(&self, w: i32, h: i32) {
        self.gl.viewport(0, 0, w, h);
    }

    // ---------------------------------------------------------------
    // Kare
    // ---------------------------------------------------------------

    /// Bir kareyi çizer. `cam` dünya koordinatıdır (f64: eleme ve kameraya
    /// göre konum hassas hesaplanır).
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        cam: [f64; 3],
        yaw: f32,
        pitch: f32,
        fov_y: f32,
        aspect: f32,
        far: f32,
        fog_near: f32,
        fog_far: f32,
        time: f32,
    ) {
        let view_proj = mat4::mul(
            &mat4::perspective(fov_y, aspect, 0.05, far),
            &mat4::view_rotation(yaw, pitch),
        );
        let frustum = Frustum::from_view_proj(&view_proj);

        // --- görünürlük + sıralama (tamamen Rust'ta, ayırmasız) ---
        let Self { chunks, vis_solid, vis_liquid, .. } = self;
        vis_solid.clear();
        vis_liquid.clear();
        for (i, c) in chunks.iter().enumerate() {
            let rel = [c.origin[0] - cam[0], -cam[1], c.origin[1] - cam[2]];
            for (layer, list) in [(&c.solid, &mut *vis_solid), (&c.liquid, &mut *vis_liquid)] {
                let Some(l) = layer else { continue };
                let mn = [0, 1, 2].map(|a| (rel[a] + l.bmin[a] as f64) as f32);
                let mx = [0, 1, 2].map(|a| (rel[a] + l.bmax[a] as f64) as f32);
                if frustum.aabb_visible(mn, mx) {
                    let d2 = (0..3).map(|a| ((mn[a] + mx[a]) * 0.5).powi(2)).sum::<f32>();
                    list.push((d2, i as u32));
                }
            }
        }
        // Katı: önden arkaya (erken-z ile örtülen pikseller gölgelenmez).
        vis_solid.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        // Sıvı: arkadan öne (saydamlık doğru karışsın).
        vis_liquid.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));

        let gl = &self.gl;
        gl.clear(GL::COLOR_BUFFER_BIT | GL::DEPTH_BUFFER_BIT);

        let mut draws = 0u32;
        let mut tris = 0u32;
        let draw = |l: &GpuLayer, draws: &mut u32, tris: &mut u32| {
            gl.bind_vertex_array(Some(&l.vao));
            gl.draw_elements_instanced_with_i32(GL::TRIANGLES, l.count, l.index_type, 0, 1);
            *draws += 1;
            *tris += l.count as u32 / 3;
        };

        // --- 1) katı katman ---
        gl.use_program(Some(&self.prog));
        gl.uniform_matrix4fv_with_f32_array(self.u_view_proj.as_ref(), false, &view_proj);
        gl.uniform3f(self.u_cam_pos.as_ref(), cam[0] as f32, cam[1] as f32, cam[2] as f32);
        if self.fog_sent != [fog_near, fog_far] {
            gl.uniform2f(self.u_fog.as_ref(), fog_near, fog_far);
            self.fog_sent = [fog_near, fog_far];
        }
        gl.uniform2f(self.u_alpha.as_ref(), 1.0, 0.5);
        for &(_, i) in &self.vis_solid {
            if let Some(l) = &self.chunks[i as usize].solid {
                draw(l, &mut draws, &mut tris);
            }
        }

        // --- 2) gökyüzü: yalnızca geometrinin kaplamadığı pikseller ---
        // Derinlik 1.0 + LEQUAL → katı dünyanın arkasında kalan pikseller
        // erken-z ile atlanır.
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let fwd = [-sy * cp, sp, -cy * cp];
        let right = [cy, 0.0, -sy];
        let up = [
            right[1] * fwd[2] - right[2] * fwd[1],
            right[2] * fwd[0] - right[0] * fwd[2],
            right[0] * fwd[1] - right[1] * fwd[0],
        ];
        let basis = [right[0], right[1], right[2], up[0], up[1], up[2], fwd[0], fwd[1], fwd[2]];
        gl.depth_mask(false);
        gl.use_program(Some(&self.sky_prog));
        gl.uniform_matrix3fv_with_f32_array(self.s_basis.as_ref(), false, &basis);
        gl.uniform3f(self.s_params.as_ref(), (fov_y * 0.5).tan(), aspect, time);
        gl.bind_vertex_array(Some(&self.sky_vao));
        gl.draw_arrays(GL::TRIANGLES, 0, 3);
        gl.depth_mask(true);

        // --- 3) sıvı katman (saydam) ---
        if !self.vis_liquid.is_empty() {
            gl.use_program(Some(&self.prog));
            gl.enable(GL::BLEND);
            gl.uniform2f(self.u_alpha.as_ref(), 0.82, 0.01);
            for &(_, i) in &self.vis_liquid {
                if let Some(l) = &self.chunks[i as usize].liquid {
                    draw(l, &mut draws, &mut tris);
                }
            }
            gl.disable(GL::BLEND);
        }
        gl.bind_vertex_array(None);

        self.stats_draws = draws;
        self.stats_tris = tris;
    }
}
