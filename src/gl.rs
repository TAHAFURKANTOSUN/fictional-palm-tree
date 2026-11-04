//! WebGL2 çizici.
//!
//! Rust ↔ GPU sınırı bilerek ince tutulur; her kare ve her yükleme mümkün
//! olan en az JS çağrısı, en az bayt ve sıfır JS-tarafı ayırmayla yapılır.
//!
//! # Geometri: tek arena
//!
//! * **Tek köşe tamponu.** Tüm chunk'ların geometrisi tek bir büyük VBO'da
//!   durur; arena 2 KB'lık sayfalara bölünür ve her chunk'a bitişik bir sayfa
//!   aralığı verilir ([`crate::alloc::PageAlloc`]). Tek VAO vardır, chunk
//!   başına GL nesnesi yoktur.
//! * **Paketlenmiş köşe** — 8 bayt/köşe ([`mesh::Vertex`]). UV
//!   gölgelendiricide hesaplanır.
//! * **Paylaşılan indeks deseni.** IBO tüm arenayı kapsayan dörtgen
//!   desenidir (`4q, 4q+1, 4q+2, 4q, 4q+2, 4q+3`); bir chunk'ın yüzleri,
//!   desenin o chunk'a denk gelen diliminden çizilir — temel köşe
//!   (base vertex) gerekmez.
//! * **Chunk orijini dokudan.** Köşe gölgelendiricisi `gl_VertexID`'den
//!   sayfayı bulur ve orijini küçük bir tamsayı dokusundan okur. Çizim başına
//!   hiçbir uniform ya da VAO değişikliği yoktur.
//! * **Kopyasız yükleme** — baytlar wasm belleğinin görünümü olarak
//!   doğrudan `bufferSubData`'ya gider.
//!
//! # Kare
//!
//! * **`WEBGL_multi_draw`** varsa her geçiş (opak / oyuk / sıvı) **tek**
//!   çağrıdır. Yoksa (ör. Firefox) görünen her chunk için tek
//!   `drawElements` (eskiden `bindVertexArray` + çizim).
//! * **Yön elemesi.** Yüzler normal yönüne göre gruplanmıştır; kameraya
//!   sırtını dönen gruplar hiç gönderilmez (multi-draw'da bedava).
//! * **Opak / oyuk ayrımı.** Saydam pikseli olmayan her şey `discard`'sız
//!   gölgelendiriciyle çizilir; GPU erken derinlik testini tam kullanır.
//! * **Kare verisi UBO'da** — görünüm-izdüşüm, kamera, sis tek
//!   `bufferSubData` ile tüm programlara gider.
//! * **Sıra:** opak (önden arkaya) → oyuk → gökyüzü (yalnızca boş
//!   piksellerde) → sıvı (arkadan öne).

use std::collections::HashMap;

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as GL, WebGlBuffer, WebGlProgram, WebGlShader,
    WebGlTexture, WebGlUniformLocation, WebGlVertexArrayObject, WebglMultiDraw,
};

use crate::alloc::PageAlloc;
use crate::atlas::{ATLAS_H, ATLAS_W, TILE};
use crate::mat4::{self, Frustum};
use crate::mesh::{visible_groups, MeshData, QUAD_PATTERN, VERTEX_BYTES};
use crate::world::CHUNK_SIZE;

const STRIDE: i32 = VERTEX_BYTES as i32;

/// Sayfa: 64 dörtgen = 256 köşe = 2 KB.
const PAGE_SHIFT: u32 = 8;
const PAGE_VERTS: u32 = 1 << PAGE_SHIFT;
const PAGE_QUADS: u32 = PAGE_VERTS / 4;
const PAGE_BYTES: usize = PAGE_VERTS as usize * VERTEX_BYTES;
/// İndeks deseninde dörtgen başına bayt (6 × u32).
const QUAD_INDEX_BYTES: u32 = QUAD_PATTERN.len() as u32 * 4;

/// Orijin dokusu genişliği (sayfa): `MAX_TEXTURE_SIZE ≥ 2048` güvencesinin altında.
const ORIGIN_SHIFT: u32 = 10;
const ORIGIN_W: u32 = 1 << ORIGIN_SHIFT;

/// Başlangıç arenası: 2048 sayfa (4 MB köşe). Gerekirse ikiye katlanır.
const INITIAL_PAGES: u32 = 2048;
/// İndeks deseni büyütülürken kullanılan parça boyu (geçici bellek sınırı).
const INDEX_FILL_QUADS: u32 = 1 << 14;

/// UBO düzeni (std140): mat4 + vec4 kamera + vec4 sis = 96 bayt.
const FRAME_FLOATS: usize = 24;

const A_LOCAL: u32 = 0;
const A_COLOR: u32 = 1;

/// Katmanlar.
const OPAQUE: usize = 0;
const CUTOUT: usize = 1;
const LIQUID: usize = 2;

const COMMON: &str = r#"
layout(std140) uniform Frame {
  highp mat4 u_viewProj;  // ötelemesiz (kameraya göre)
  highp vec4 u_camPos;    // xyz
  highp vec4 u_fog;       // yakın, uzak
};
"#;

const MAIN_VS: &str = r#"
layout(location = 0) in uvec4 a_local;  // chunk-yerel x, y, z ; döşeme*4 + köşe
layout(location = 1) in vec4 a_col;     // unorm8 renk
uniform highp isampler2D u_origins;     // sayfa → chunk orijini (x, z)
out vec2 v_uv;
out vec3 v_col;
out float v_dist;
void main() {
  int page = gl_VertexID >> PAGE_SHIFT;
  ivec2 o = texelFetch(u_origins, ivec2(page & (ORIGIN_W - 1), page >> ORIGIN_SHIFT), 0).xy;
  vec3 rel = vec3(float(o.x) - u_camPos.x, -u_camPos.y, float(o.y) - u_camPos.z)
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

/// Opak ve oyuk/sıvı programları aynı kaynaktan; `CUTOUT` tanımlı değilse
/// `discard` yoktur.
const MAIN_FS: &str = r#"
precision mediump float;
in vec2 v_uv;
in vec3 v_col;
in float v_dist;
uniform sampler2D u_atlas;
uniform vec3 u_fogColor;
#ifdef CUTOUT
uniform vec2 u_alpha;      // opaklık, alfa eşiği
#endif
out vec4 fragColor;
void main() {
  vec4 t = texture(u_atlas, v_uv);
#ifdef CUTOUT
  if (t.a < u_alpha.y) discard;
  float a = t.a * u_alpha.x;
#else
  float a = 1.0;
#endif
  vec3 c = t.rgb * v_col;
  float f = clamp((v_dist - u_fog.x) / max(u_fog.y - u_fog.x, 0.001), 0.0, 1.0);
  fragColor = vec4(mix(c, u_fogColor, f), a);
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

/// Bir chunk katmanının arenadaki yeri ve eleme bilgisi.
struct Layer {
    /// Arenadaki ilk dörtgen (mutlak).
    start: u32,
    groups: [u32; 6],
    spans: [[u8; 2]; 6],
    /// Chunk-yerel kapsayıcı kutu.
    bmin: [f32; 3],
    bmax: [f32; 3],
}

struct GpuChunk {
    key: u64,
    /// Dünya orijini (x, z).
    origin: [f64; 2],
    /// Arenadaki sayfa aralığı.
    first_page: u32,
    pages: u32,
    layers: [Option<Layer>; 3],
}

/// Bir çizim aralığı: (sıralama anahtarı, ilk dörtgen, dörtgen sayısı).
type Range = (f32, u32, u32);

pub struct Renderer {
    gl: GL,
    multi: Option<WebglMultiDraw>,

    prog_opaque: WebGlProgram,
    prog_cutout: WebGlProgram,
    prog_sky: WebGlProgram,
    u_alpha: Option<WebGlUniformLocation>,
    alpha_sent: [f32; 2],
    s_basis: Option<WebGlUniformLocation>,
    s_params: Option<WebGlUniformLocation>,

    world_vao: WebGlVertexArrayObject,
    sky_vao: WebGlVertexArrayObject,
    /// TEXTURE0'a kalıcı olarak bağlı atlas (tutamaç canlı kalsın).
    _atlas: WebGlTexture,
    /// Kare verisi (UNIFORM_BUFFER'a kalıcı olarak bağlı).
    _ubo: WebGlBuffer,

    // --- arena ---
    vbo: WebGlBuffer,
    /// İndeks deseni (dünya VAO'suna bağlı; tutamaç canlı kalsın).
    _ibo: WebGlBuffer,
    /// TEXTURE1: sayfa → chunk orijini (RG32I).
    origins: WebGlTexture,
    pages: PageAlloc,
    /// Orijin dokusunun CPU kopyası (sayfa başına x, z).
    page_origin: Vec<i32>,

    chunks: Vec<GpuChunk>,
    slot: HashMap<u64, usize>,

    /// Yeniden kullanılan tamponlar (kararlı durumda ayırma yok).
    upload: Vec<u8>,
    lists: [Vec<Range>; 3],
    counts: Vec<i32>,
    offsets: Vec<i32>,

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

/// Gölgelendiricilere ortak başlığı (sürüm + sabitler + `defines`) ekleyip bağlar.
fn link(gl: &GL, vs_src: &str, fs_src: &str, defines: &str) -> Result<WebGlProgram, String> {
    let header = format!(
        "#version 300 es\n{defines}#define TILE_PX {TILE}.0\n#define ATLAS_W_PX {ATLAS_W}.0\n\
         #define ATLAS_H_PX {ATLAS_H}.0\n#define PAGE_SHIFT {PAGE_SHIFT}\n\
         #define ORIGIN_SHIFT {ORIGIN_SHIFT}\n#define ORIGIN_W {ORIGIN_W}\n"
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

/// `[from, to)` dörtgenlerinin indeks deseni, küçük-sonlu u32 baytlar.
fn quad_index_bytes(from: u32, to: u32, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(((to - from) * QUAD_INDEX_BYTES) as usize);
    for q in from..to {
        for i in QUAD_PATTERN {
            out.extend_from_slice(&(q * 4 + i).to_le_bytes());
        }
    }
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

        // Eklenti arayüzleri tarayıcıda global olarak tanımlı değildir
        // (`instanceof WEBGL_multi_draw` yok); `dyn_into` hep başarısız olur.
        let multi = gl
            .get_extension("WEBGL_multi_draw")
            .ok()
            .flatten()
            .map(|e| e.unchecked_into::<WebglMultiDraw>());

        let prog_opaque = link(&gl, &(COMMON.to_owned() + MAIN_VS), &(COMMON.to_owned() + MAIN_FS), "")?;
        let prog_cutout = link(
            &gl,
            &(COMMON.to_owned() + MAIN_VS),
            &(COMMON.to_owned() + MAIN_FS),
            "#define CUTOUT 1\n",
        )?;
        let prog_sky = link(&gl, SKY_VS, SKY_FS, "")?;

        // --- doku atlası (TEXTURE0'a kalıcı olarak bağlı) ---
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

        // --- sayfa → orijin dokusu (TEXTURE1'e kalıcı olarak bağlı) ---
        let origins = gl.create_texture().ok_or("orijin dokusu oluşturulamadı")?;
        gl.active_texture(GL::TEXTURE1);
        gl.bind_texture(GL::TEXTURE_2D, Some(&origins));
        // Tamsayı dokular yalnızca NEAREST ile tamdır.
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::NEAREST as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::NEAREST as i32);

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

        // --- dünya: tek VAO + arena ---
        let world_vao = gl.create_vertex_array().ok_or("VAO oluşturulamadı")?;
        let vbo = gl.create_buffer().ok_or("arena oluşturulamadı")?;
        let ibo = gl.create_buffer().ok_or("indeks tamponu oluşturulamadı")?;
        gl.bind_vertex_array(Some(&world_vao));
        gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(&ibo));
        gl.enable_vertex_attrib_array(A_LOCAL);
        gl.enable_vertex_attrib_array(A_COLOR);
        gl.bind_vertex_array(None);
        gl.bind_buffer(GL::ARRAY_BUFFER, None);

        // --- kare verisi: UBO, bağlama noktası 0 ---
        let ubo = gl.create_buffer().ok_or("UBO oluşturulamadı")?;
        gl.bind_buffer_base(GL::UNIFORM_BUFFER, 0, Some(&ubo));
        gl.buffer_data_with_i32(GL::UNIFORM_BUFFER, (FRAME_FLOATS * 4) as i32, GL::DYNAMIC_DRAW);

        // --- sabit boru hattı durumu: bir kez ---
        gl.enable(GL::DEPTH_TEST);
        gl.depth_func(GL::LEQUAL);
        gl.enable(GL::CULL_FACE);
        gl.cull_face(GL::BACK);
        gl.blend_func(GL::SRC_ALPHA, GL::ONE_MINUS_SRC_ALPHA);
        gl.clear_color(SKY_HORIZON[0], SKY_HORIZON[1], SKY_HORIZON[2], 1.0);

        // --- sabit uniform'lar: bir kez ---
        for prog in [&prog_opaque, &prog_cutout] {
            gl.use_program(Some(prog));
            let block = gl.get_uniform_block_index(prog, "Frame");
            gl.uniform_block_binding(prog, block, 0);
            let u = |name: &str| gl.get_uniform_location(prog, name);
            gl.uniform1i(u("u_atlas").as_ref(), 0);
            gl.uniform1i(u("u_origins").as_ref(), 1);
            gl.uniform3fv_with_f32_array(u("u_fogColor").as_ref(), &FOG_COLOR);
        }
        let u_alpha = gl.get_uniform_location(&prog_cutout, "u_alpha");

        gl.use_program(Some(&prog_sky));
        let su = |name: &str| gl.get_uniform_location(&prog_sky, name);
        gl.uniform3fv_with_f32_array(su("u_top").as_ref(), &SKY_TOP);
        gl.uniform3fv_with_f32_array(su("u_horizon").as_ref(), &SKY_HORIZON);
        let s_basis = su("u_basis");
        let s_params = su("u_params");

        let mut r = Self {
            gl,
            multi,
            prog_opaque,
            prog_cutout,
            prog_sky,
            u_alpha,
            alpha_sent: [f32::NAN; 2],
            s_basis,
            s_params,
            world_vao,
            sky_vao,
            _atlas: atlas,
            _ubo: ubo,
            vbo,
            _ibo: ibo,
            origins,
            pages: PageAlloc::new(0),
            page_origin: Vec::new(),
            chunks: Vec::new(),
            slot: HashMap::new(),
            upload: Vec::new(),
            lists: Default::default(),
            counts: Vec::new(),
            offsets: Vec::new(),
            stats_draws: 0,
            stats_tris: 0,
        };
        r.grow(INITIAL_PAGES)?;
        Ok(r)
    }

    /// `WEBGL_multi_draw` kullanılıyor mu?
    pub fn multi_draw(&self) -> bool {
        self.multi.is_some()
    }

    // ---------------------------------------------------------------
    // Arena
    // ---------------------------------------------------------------

    /// Arenayı en az `min_pages` sayfaya büyütür (ikiye katlayarak). Eski
    /// köşeler GPU içinde kopyalanır; indeks deseni ve orijin dokusu
    /// genişletilir.
    fn grow(&mut self, min_pages: u32) -> Result<(), String> {
        let old = self.pages.capacity();
        if min_pages <= old {
            return Ok(());
        }
        let new = min_pages.max(old * 2).next_multiple_of(ORIGIN_W);
        let gl = &self.gl;

        // 1) Köşe tamponu: yenisini ayır, eskiyi GPU içinde kopyala.
        let vbo = gl.create_buffer().ok_or("arena büyütülemedi")?;
        gl.bind_buffer(GL::COPY_WRITE_BUFFER, Some(&vbo));
        gl.buffer_data_with_i32(GL::COPY_WRITE_BUFFER, new as i32 * PAGE_BYTES as i32, GL::DYNAMIC_DRAW);
        if old > 0 {
            gl.bind_buffer(GL::COPY_READ_BUFFER, Some(&self.vbo));
            gl.copy_buffer_sub_data_with_i32_and_i32_and_i32(
                GL::COPY_READ_BUFFER,
                GL::COPY_WRITE_BUFFER,
                0,
                0,
                old as i32 * PAGE_BYTES as i32,
            );
            gl.bind_buffer(GL::COPY_READ_BUFFER, None);
        }
        gl.bind_buffer(GL::COPY_WRITE_BUFFER, None);
        gl.delete_buffer(Some(&self.vbo));
        self.vbo = vbo;

        // 2) VAO öznitelikleri yeni tampona; indeks deseni yeni kapasiteye.
        gl.bind_vertex_array(Some(&self.world_vao));
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.vbo));
        gl.vertex_attrib_i_pointer_with_i32(A_LOCAL, 4, GL::UNSIGNED_BYTE, STRIDE, 0);
        gl.vertex_attrib_pointer_with_i32(A_COLOR, 4, GL::UNSIGNED_BYTE, true, STRIDE, 4);
        gl.bind_buffer(GL::ARRAY_BUFFER, None);
        let quads = new * PAGE_QUADS;
        gl.buffer_data_with_i32(GL::ELEMENT_ARRAY_BUFFER, (quads * QUAD_INDEX_BYTES) as i32, GL::STATIC_DRAW);
        let mut q = 0;
        while q < quads {
            let end = (q + INDEX_FILL_QUADS).min(quads);
            quad_index_bytes(q, end, &mut self.upload);
            gl.buffer_sub_data_with_i32_and_u8_array(
                GL::ELEMENT_ARRAY_BUFFER,
                (q * QUAD_INDEX_BYTES) as i32,
                &self.upload,
            );
            q = end;
        }
        gl.bind_vertex_array(None);

        // 3) Orijin dokusu.
        self.page_origin.resize(new as usize * 2, 0);
        gl.active_texture(GL::TEXTURE1);
        gl.bind_texture(GL::TEXTURE_2D, Some(&self.origins));
        let data = js_sys::Int32Array::from(&self.page_origin[..]);
        gl.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_array_buffer_view(
            GL::TEXTURE_2D,
            0,
            GL::RG32I as i32,
            ORIGIN_W as i32,
            (new / ORIGIN_W) as i32,
            0,
            GL::RG_INTEGER,
            GL::INT,
            Some(&data),
        )
        .map_err(|_| "orijin dokusu büyütülemedi")?;

        self.pages.grow(new);
        Ok(())
    }

    /// `[first, first + n)` sayfalarının orijinini yazar (dokuda en fazla
    /// iki satıra yayılır).
    fn write_origins(&mut self, first: u32, n: u32, origin: [f64; 2]) {
        for p in first..first + n {
            self.page_origin[p as usize * 2] = origin[0] as i32;
            self.page_origin[p as usize * 2 + 1] = origin[1] as i32;
        }
        let gl = &self.gl;
        gl.active_texture(GL::TEXTURE1);
        gl.bind_texture(GL::TEXTURE_2D, Some(&self.origins));
        let mut p = first;
        while p < first + n {
            let (x, y) = (p % ORIGIN_W, p / ORIGIN_W);
            let w = (ORIGIN_W - x).min(first + n - p);
            let data = js_sys::Int32Array::from(&self.page_origin[p as usize * 2..(p + w) as usize * 2]);
            let _ = gl.tex_sub_image_2d_with_i32_and_i32_and_u32_and_type_and_opt_array_buffer_view(
                GL::TEXTURE_2D,
                0,
                x as i32,
                y as i32,
                w as i32,
                1,
                GL::RG_INTEGER,
                GL::INT,
                Some(&data),
            );
            p += w;
        }
    }

    // ---------------------------------------------------------------
    // Chunk'lar
    // ---------------------------------------------------------------

    /// Chunk'ı GPU'ya yükler. Var olan sayfa aralığı yetiyorsa yerinde
    /// güncellenir; üç katman tek `bufferSubData` ile gider.
    pub fn upload_chunk(
        &mut self,
        key: u64,
        cx: i32,
        cz: i32,
        opaque: &MeshData,
        cutout: &MeshData,
        liquid: &MeshData,
    ) {
        let meshes = [opaque, cutout, liquid];
        let quads: u32 = meshes.iter().map(|m| m.face_count()).sum();
        if quads == 0 {
            self.remove_chunk(key);
            return;
        }
        let need = quads.div_ceil(PAGE_QUADS);
        let origin = [(cx * CHUNK_SIZE) as f64, (cz * CHUNK_SIZE) as f64];

        // --- sayfa aralığı: yerinde kullan, kuyruğu bırak ya da yeniden ayır ---
        let existing = self.slot.get(&key).map(|&i| (self.chunks[i].first_page, self.chunks[i].pages));
        let first = match existing {
            Some((first, have)) if have >= need => {
                self.pages.free(first + need, have - need);
                first
            }
            other => {
                if let Some((first, have)) = other {
                    self.pages.free(first, have);
                }
                let first = match self.pages.alloc(need) {
                    Some(p) => p,
                    None => {
                        let cap = self.pages.capacity();
                        if self.grow(cap + need).is_err() {
                            return;
                        }
                        match self.pages.alloc(need) {
                            Some(p) => p,
                            None => return,
                        }
                    }
                };
                self.write_origins(first, need, origin);
                first
            }
        };

        // --- katmanlar: arka arkaya, tek yükleme ---
        self.upload.clear();
        let mut layers: [Option<Layer>; 3] = [None, None, None];
        let mut q = first * PAGE_QUADS;
        for (slot, m) in layers.iter_mut().zip(meshes) {
            if let Some((bmin, bmax)) = m.local_bounds() {
                *slot = Some(Layer {
                    start: q,
                    groups: m.groups,
                    spans: m.plane_spans(),
                    bmin: bmin.map(f32::from),
                    bmax: bmax.map(f32::from),
                });
                q += m.face_count();
                self.upload.extend_from_slice(m.as_bytes());
            }
        }
        self.gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.vbo));
        self.gl.buffer_sub_data_with_i32_and_u8_array(
            GL::ARRAY_BUFFER,
            first as i32 * PAGE_BYTES as i32,
            &self.upload,
        );
        self.gl.bind_buffer(GL::ARRAY_BUFFER, None);

        let chunk = GpuChunk { key, origin, first_page: first, pages: need, layers };
        match self.slot.get(&key) {
            Some(&i) => self.chunks[i] = chunk,
            None => {
                self.slot.insert(key, self.chunks.len());
                self.chunks.push(chunk);
            }
        }
    }

    pub fn remove_chunk(&mut self, key: u64) {
        let Some(i) = self.slot.remove(&key) else { return };
        let c = self.chunks.swap_remove(i);
        if let Some(moved) = self.chunks.get(i) {
            self.slot.insert(moved.key, i);
        }
        self.pages.free(c.first_page, c.pages);
    }

    pub fn resize(&self, w: i32, h: i32) {
        self.gl.viewport(0, 0, w, h);
    }

    // ---------------------------------------------------------------
    // Kare
    // ---------------------------------------------------------------

    /// Bir geçişin aralıklarını çizer: multi-draw ile tek çağrı, yoksa
    /// aralık başına tek `drawElements`.
    fn draw_list(&mut self, layer: usize) {
        let Self { gl, multi, lists, counts, offsets, stats_draws, stats_tris, .. } = self;
        let list = &lists[layer];
        if list.is_empty() {
            return;
        }
        counts.clear();
        offsets.clear();
        for &(_, start, quads) in list {
            counts.push((quads * 6) as i32);
            offsets.push((start * QUAD_INDEX_BYTES) as i32);
            *stats_tris += quads * 2;
        }
        *stats_draws += list.len() as u32;
        match multi {
            Some(m) => m.multi_draw_elements_webgl_with_i32_array_and_i32_array(
                GL::TRIANGLES,
                counts,
                0,
                GL::UNSIGNED_INT,
                offsets,
                0,
                list.len() as i32,
            ),
            None => {
                for (&c, &o) in counts.iter().zip(offsets.iter()) {
                    gl.draw_elements_with_i32(GL::TRIANGLES, c, GL::UNSIGNED_INT, o);
                }
            }
        }
    }

    fn set_alpha(&mut self, opacity: f32, threshold: f32) {
        if self.alpha_sent != [opacity, threshold] {
            self.gl.uniform2f(self.u_alpha.as_ref(), opacity, threshold);
            self.alpha_sent = [opacity, threshold];
        }
    }

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

        // --- görünürlük, yön elemesi, sıralama (tamamen Rust'ta, ayırmasız) ---
        let split_groups = self.multi.is_some();
        let Self { chunks, lists, .. } = self;
        for l in lists.iter_mut() {
            l.clear();
        }
        for c in chunks.iter() {
            let rel = [c.origin[0] - cam[0], -cam[1], c.origin[1] - cam[2]];
            let cam_local = [-rel[0] as f32, -rel[1] as f32, -rel[2] as f32];
            for (li, layer) in c.layers.iter().enumerate() {
                let Some(l) = layer else { continue };
                let mn = [0, 1, 2].map(|a| (rel[a] + l.bmin[a] as f64) as f32);
                let mx = [0, 1, 2].map(|a| (rel[a] + l.bmax[a] as f64) as f32);
                if !frustum.aabb_visible(mn, mx) {
                    continue;
                }
                let d2 = (0..3).map(|a| ((mn[a] + mx[a]) * 0.5).powi(2)).sum::<f32>();
                // Sıvı arkadan öne çizilir.
                let key = if li == LIQUID { -d2 } else { d2 };
                let list = &mut lists[li];
                if !split_groups {
                    let quads: u32 = l.groups.iter().sum();
                    list.push((key, l.start, quads));
                    continue;
                }
                // Görünen yön gruplarını bitişik aralıklara birleştir.
                let mask = visible_groups(cam_local, &l.spans, &l.groups);
                let mut q = l.start;
                let mut open: Option<(u32, u32)> = None;
                for f in 0..6 {
                    let n = l.groups[f];
                    if mask & (1 << f) != 0 {
                        open = Some(match open {
                            Some((s, len)) => (s, len + n),
                            None => (q, n),
                        });
                    } else if let Some((s, len)) = open.take() {
                        list.push((key, s, len));
                    }
                    q += n;
                }
                if let Some((s, len)) = open {
                    list.push((key, s, len));
                }
            }
        }
        // Opak/oyuk önden arkaya (erken-z), sıvı arkadan öne (anahtar ters).
        for l in lists.iter_mut() {
            l.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        }

        // --- kare verisi: tek UBO yüklemesi ---
        let mut frame = [0f32; FRAME_FLOATS];
        frame[..16].copy_from_slice(&view_proj);
        frame[16..19].copy_from_slice(&[cam[0] as f32, cam[1] as f32, cam[2] as f32]);
        frame[20] = fog_near;
        frame[21] = fog_far;
        let mut bytes = [0u8; FRAME_FLOATS * 4];
        for (dst, v) in bytes.chunks_exact_mut(4).zip(frame) {
            dst.copy_from_slice(&v.to_le_bytes());
        }
        let gl = &self.gl;
        gl.buffer_sub_data_with_i32_and_u8_array(GL::UNIFORM_BUFFER, 0, &bytes);

        gl.clear(GL::COLOR_BUFFER_BIT | GL::DEPTH_BUFFER_BIT);
        self.stats_draws = 0;
        self.stats_tris = 0;

        // --- 1) opak (discard yok) ---
        gl.bind_vertex_array(Some(&self.world_vao));
        gl.use_program(Some(&self.prog_opaque));
        self.draw_list(OPAQUE);

        // --- 2) oyuk (cam): alfa testi ---
        if !self.lists[CUTOUT].is_empty() {
            self.gl.use_program(Some(&self.prog_cutout));
            self.set_alpha(1.0, 0.5);
            self.draw_list(CUTOUT);
        }

        // --- 3) gökyüzü: yalnızca geometrinin kaplamadığı pikseller ---
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
        let gl = &self.gl;
        gl.depth_mask(false);
        gl.use_program(Some(&self.prog_sky));
        gl.uniform_matrix3fv_with_f32_array(self.s_basis.as_ref(), false, &basis);
        gl.uniform3f(self.s_params.as_ref(), (fov_y * 0.5).tan(), aspect, time);
        gl.bind_vertex_array(Some(&self.sky_vao));
        gl.draw_arrays(GL::TRIANGLES, 0, 3);
        gl.depth_mask(true);

        // --- 4) sıvı (saydam, arkadan öne) ---
        if !self.lists[LIQUID].is_empty() {
            let gl = &self.gl;
            gl.bind_vertex_array(Some(&self.world_vao));
            gl.use_program(Some(&self.prog_cutout));
            gl.enable(GL::BLEND);
            self.set_alpha(0.82, 0.01);
            self.draw_list(LIQUID);
            self.gl.disable(GL::BLEND);
        }
    }
}
