//! Canvas2D arayüz katmanı.
//!
//! Tüm arayüz (nişangah, kısayol çubuğu, can/açlık/nefes göstergeleri,
//! bildirimler, envanter + çalışma tezgahı ekranı) WebGL tuvalinin üstündeki
//! ikinci bir tuval üzerine çizilir. Yerleşim yalnızca pencere boyutu
//! değişince hesaplanır ve aynı dikdörtgenler hem çizim hem de fare isabet
//! testi için kullanılır.
//!
//! # Rust → Canvas2D sınırı
//!
//! Her Canvas2D çağrısı wasm → JS geçişidir ve stil atamaları her seferinde
//! bir dize çözer. Bu yüzden:
//!
//! * **Değişmeyen kare çizilmez.** Ekrana çizilecek her şeyin (yuvalar, can
//!   durumları, bildirim, ölçüm metni…) özeti alınır; özet aynıysa tuval
//!   olduğu gibi kalır. Yalnızca ölçüm paneli değiştiyse yalnızca o bölge
//!   yeniden çizilir.
//! * **Can/açlık/nefes ikonları önceden çizilir.** Her ikon eskiden ~40
//!   `fillStyle` + `fillRect` çağrısıydı; şimdi hazır bir sprite'tan tek
//!   `drawImage`.
//! * **Stil önbelleği.** Aynı `fillStyle`/`font`/hizalama tekrar atanmaz.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};

use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

use crate::atlas::{ATLAS_H, ATLAS_W, TILE};
use crate::blocks::*;
use crate::crafting::{self, Grid};
use crate::inventory::{Inventory, Stack, HOTBAR_SIZE};
use crate::survival::{Survival, MAX_AIR};

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

/// Bir kare için hesaplanan tüm tıklanabilir alanlar.
pub struct Layout {
    pub hotbar: Vec<Rect>,
    pub craft: Vec<Rect>,
    pub craft_result: Rect,
    pub inv_main: Vec<Rect>,
    pub inv_hotbar: Vec<Rect>,
    pub recipes: Vec<Rect>,
    pub clear_btn: Rect,
    pub close_btn: Rect,
    pub panel: Rect,
    pub vitals_hearts: Rect,
    pub vitals_hunger: Rect,
    pub vitals_air: Rect,
    /// Bölüm başlıkları için ayrılan satır yüksekliği.
    pub label_row: f64,
}

pub struct Hud {
    ctx: CanvasRenderingContext2d,
    atlas: HtmlCanvasElement,
    /// Önceden çizilmiş can/açlık/nefes ikonları (bkz. [`make_vitals_sprites`]).
    sprites: HtmlCanvasElement,
    pub width: f64,
    pub height: f64,
    toast: Option<(String, f64)>,
    pub layout: Option<Layout>,
    /// Son çizilen karenin özeti (ölçüm paneli hariç) ve ölçüm panelinin özeti.
    drawn_main: Option<u64>,
    drawn_debug: Option<u64>,
    /// Ölçüm paneli satırları (yeniden kullanılan dizeler).
    debug_lines: [String; DEBUG_LINES],
    style: RefCell<StyleCache>,
}

/// Bağlamda son atanan stiller. Tuval boyutu değişince bağlam durumu
/// sıfırlandığı için önbellek de sıfırlanır.
struct StyleCache {
    fill: String,
    stroke: String,
    font: String,
    align: &'static str,
    baseline: &'static str,
    line_width: f64,
}

impl Default for StyleCache {
    fn default() -> Self {
        Self {
            fill: String::new(),
            stroke: String::new(),
            font: String::new(),
            align: "",
            baseline: "",
            line_width: f64::NAN,
        }
    }
}

const DEBUG_LINES: usize = 8;
const DEBUG_KEYS: [&str; DEBUG_LINES] = ["FPS", "Konum", "Bölge", "Mesafe", "Çizim", "Mod", "Oyun", "El"];
/// Ölçüm panelinin kapladığı bölge (kısmi yeniden çizim için).
const DEBUG_RECT: Rect = Rect { x: 0.0, y: 0.0, w: 340.0, h: 16.0 + DEBUG_LINES as f64 * 17.0 + 4.0 };

/// Can/açlık/nefes ikon boyutu ve sprite hücresi (taşma payıyla).
const VITAL_ICON: f64 = 18.0;
const SPRITE_W: f64 = 19.0;
const SPRITE_H: f64 = 17.0;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Vital {
    Heart = 0,
    Food = 1,
    Air = 2,
}

/// İkonun doluluk durumu (sprite sütunu).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Fill {
    Full = 0,
    Half = 1,
    Empty = 2,
}

impl Fill {
    fn of(value: f64) -> Self {
        if value >= 1.0 {
            Fill::Full
        } else if value > 0.0 {
            Fill::Half
        } else {
            Fill::Empty
        }
    }
}

/// Ekrandaki 10'ar ikonun durumu — hem çizim hem de kare özeti bundan çıkar.
fn vital_states(s: &Survival) -> ([Fill; 10], [Fill; 10], Option<[Fill; 10]>) {
    let hearts = std::array::from_fn(|i| Fill::of((s.health as f64 - i as f64 * 2.0) / 2.0));
    let food = std::array::from_fn(|i| Fill::of((s.hunger as f64 - i as f64 * 2.0) / 2.0));
    // Nefes yalnızca su altındayken görünür.
    let air = (s.air < MAX_AIR - 0.01).then(|| {
        let step = MAX_AIR / 10.0;
        std::array::from_fn(|i| Fill::of((s.air - i as f64 * step) / step))
    });
    (hearts, food, air)
}

/// 0..1 değeri 1/255 adımlarla nicemlenir: özet ve çizim aynı değeri kullanır.
fn quantize(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Piksel sanatı ikonlar (9×8). `1` ana renk, `2` koyu kenar.
const HEART_PX: [&str; 8] = [
    "022000220", "211202112", "211121112", "211111112",
    "021111120", "002111200", "000212000", "000020000",
];
const FOOD_PX: [&str; 8] = [
    "000002220", "000021112", "000211112", "002111120",
    "021111200", "221122000", "212212000", "020002000",
];
const AIR_PX: [&str; 8] = [
    "000111000", "011111100", "111111110", "111111110",
    "111111110", "111111110", "011111100", "000111000",
];

/// Atlası bir `ImageData`'dan çevrimdışı tuvale kopyalar; böylece ikonlar
/// `drawImage` ile hızlıca çizilebilir.
fn offscreen_canvas(
    document: &web_sys::Document,
    w: u32,
    h: u32,
) -> Result<(HtmlCanvasElement, CanvasRenderingContext2d), String> {
    let canvas = document
        .create_element("canvas")
        .map_err(|_| "tuval oluşturulamadı")?
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| "tuval dönüştürülemedi")?;
    canvas.set_width(w);
    canvas.set_height(h);
    let ctx = canvas
        .get_context("2d")
        .map_err(|_| "2d bağlamı alınamadı")?
        .ok_or("2d bağlamı yok")?
        .dyn_into::<CanvasRenderingContext2d>()
        .map_err(|_| "2d bağlamı dönüştürülemedi")?;
    Ok((canvas, ctx))
}

/// Atlası bir `ImageData`'dan çevrimdışı tuvale kopyalar; böylece ikonlar
/// `drawImage` ile hızlıca çizilebilir. Pikseller GL çiziciyle paylaşılır
/// (atlas yalnızca bir kez üretilir).
fn make_atlas_canvas(document: &web_sys::Document, pixels: &[u8]) -> Result<HtmlCanvasElement, String> {
    let (canvas, ctx) = offscreen_canvas(document, ATLAS_W as u32, ATLAS_H as u32)?;
    let clamped = wasm_bindgen::Clamped(pixels);
    let img = ImageData::new_with_u8_clamped_array_and_sh(clamped, ATLAS_W as u32, ATLAS_H as u32)
        .map_err(|_| "ImageData oluşturulamadı")?;
    ctx.put_image_data(&img, 0.0, 0.0)
        .map_err(|_| "atlas yazılamadı")?;
    Ok(canvas)
}

/// Can/açlık/nefes ikonlarını tüm doluluk durumlarıyla (dolu, yarım, boş)
/// bir kez çizer: satır = [`Vital`], sütun = [`Fill`].
fn make_vitals_sprites(document: &web_sys::Document) -> Result<HtmlCanvasElement, String> {
    let (canvas, ctx) = offscreen_canvas(document, (SPRITE_W * 3.0) as u32, (SPRITE_H * 3.0) as u32)?;
    let kinds: [(&[&str; 8], &str, &str); 3] = [
        (&HEART_PX, "#e2453e", "#5d1714"),
        (&FOOD_PX, "#c8763a", "#6b3a15"),
        (&AIR_PX, "#9fd8f5", "#3d7fa6"),
    ];
    let (empty_fill, empty_dark) = ("#3c3c3c", "#232323");
    let px = VITAL_ICON / 9.0;
    for (row, (bmp, fill, dark)) in kinds.iter().enumerate() {
        for (col, value) in [1.0, 0.5, 0.0].into_iter().enumerate() {
            let (x, y) = (col as f64 * SPRITE_W, row as f64 * SPRITE_H);
            let half = value > 0.0 && value < 1.0;
            for (ry, line) in bmp.iter().enumerate() {
                for (rx, ch) in line.chars().enumerate() {
                    if ch == '0' {
                        continue;
                    }
                    let empty = value <= 0.0 || (half && rx >= 5);
                    let color = match (empty, ch == '2') {
                        (true, true) => empty_dark,
                        (true, false) => empty_fill,
                        (false, true) => *dark,
                        (false, false) => *fill,
                    };
                    ctx.set_fill_style_str(color);
                    ctx.fill_rect(x + rx as f64 * px, y + ry as f64 * px, px + 0.4, px + 0.4);
                }
            }
        }
    }
    Ok(canvas)
}

impl Hud {
    pub fn new(
        canvas: &HtmlCanvasElement,
        document: &web_sys::Document,
        atlas_pixels: &[u8],
    ) -> Result<Self, String> {
        let ctx = canvas
            .get_context("2d")
            .map_err(|_| "2d bağlamı alınamadı")?
            .ok_or("2d bağlamı yok")?
            .dyn_into::<CanvasRenderingContext2d>()
            .map_err(|_| "2d bağlamı dönüştürülemedi")?;
        let atlas = make_atlas_canvas(document, atlas_pixels)?;
        let sprites = make_vitals_sprites(document)?;
        let mut hud = Self {
            ctx,
            atlas,
            sprites,
            width: 1.0,
            height: 1.0,
            toast: None,
            layout: None,
            drawn_main: None,
            drawn_debug: None,
            debug_lines: Default::default(),
            style: RefCell::new(StyleCache::default()),
        };
        hud.resize(1.0, 1.0);
        Ok(hud)
    }

    /// Tuval boyutu değiştikten **sonra** çağrılır: yerleşim yeniden
    /// hesaplanır; tuval temizlendiği ve bağlam durumu sıfırlandığı için
    /// sonraki karede her şey yeniden çizilir.
    pub fn resize(&mut self, w: f64, h: f64) {
        self.width = w;
        self.height = h;
        self.layout = Some(self.compute_layout());
        self.drawn_main = None;
        self.drawn_debug = None;
        *self.style.borrow_mut() = StyleCache::default();
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), 1.6));
    }

    pub fn tick(&mut self, dt: f64) {
        if let Some((_, t)) = self.toast.as_mut() {
            *t -= dt;
            if *t <= 0.0 {
                self.toast = None;
            }
        }
    }

    fn rounded(&self, r: Rect, radius: f64) {
        let ctx = &self.ctx;
        ctx.begin_path();
        let _ = ctx.round_rect_with_f64(r.x, r.y, r.w, r.h, radius);
    }

    // --- önbellekli stil atamaları (aynı değer tekrar gönderilmez) ---

    fn fill(&self, color: &str) {
        let mut st = self.style.borrow_mut();
        if st.fill != color {
            self.ctx.set_fill_style_str(color);
            st.fill.clear();
            st.fill.push_str(color);
        }
    }

    fn stroke(&self, color: &str) {
        let mut st = self.style.borrow_mut();
        if st.stroke != color {
            self.ctx.set_stroke_style_str(color);
            st.stroke.clear();
            st.stroke.push_str(color);
        }
    }

    fn line_width(&self, w: f64) {
        let mut st = self.style.borrow_mut();
        if st.line_width != w {
            self.ctx.set_line_width(w);
            st.line_width = w;
        }
    }

    fn font(&self, font: &str) {
        let mut st = self.style.borrow_mut();
        if st.font != font {
            self.ctx.set_font(font);
            st.font.clear();
            st.font.push_str(font);
        }
    }

    fn text_layout(&self, align: &'static str, baseline: &'static str) {
        let mut st = self.style.borrow_mut();
        if st.align != align {
            self.ctx.set_text_align(align);
            st.align = align;
        }
        if st.baseline != baseline {
            self.ctx.set_text_baseline(baseline);
            st.baseline = baseline;
        }
    }

    // ---------------------------------------------------------------
    // Yerleşim
    // ---------------------------------------------------------------

    pub fn compute_layout(&self) -> Layout {
        let (w, h) = (self.width, self.height);
        let gap = 5.0;
        let slot = 52.0;

        // --- Kısayol çubuğu ---
        let total = HOTBAR_SIZE as f64 * slot + (HOTBAR_SIZE as f64 - 1.0) * gap;
        let hb_x = (w - total) / 2.0;
        let hb_y = h - slot - 18.0;
        let hotbar: Vec<Rect> = (0..HOTBAR_SIZE)
            .map(|i| Rect { x: hb_x + i as f64 * (slot + gap), y: hb_y, w: slot, h: slot })
            .collect();

        // --- Can / açlık / nefes ---
        let icon = VITAL_ICON;
        let bar_w = 10.0 * (icon + 1.0);
        let vitals_hearts = Rect { x: hb_x, y: hb_y - 24.0, w: bar_w, h: icon };
        let vitals_air = Rect { x: hb_x, y: hb_y - 24.0 - icon - 3.0, w: bar_w, h: icon };
        let vitals_hunger = Rect { x: hb_x + total - bar_w, y: hb_y - 24.0, w: bar_w, h: icon };

        // --- Envanter paneli ---
        // Yuva boyutu, pencerenin yüksekliğine göre en büyük sığan değere
        // indirilir; böylece panel her ekranda taşmadan tam görünür.
        let pad = 18.0;
        let header = 48.0;
        let label_row = 18.0;
        let row_gap = 4.0;
        let rows = crafting::RECIPES.len() as f64;
        let fixed_no_craft =
            pad * 2.0 + header + 22.0 + 16.0 + 16.0 + 30.0 + 2.0 * label_row;

        let mut s = 32.0f64;
        for cand in (32..=52).rev() {
            let sc = cand as f64;
            let gc = (sc * 0.1).max(4.0);
            let craft_h = 3.0 * sc + 2.0 * gc;
            let fixed = fixed_no_craft + 4.0 * (sc + gc);
            let avail = (h - 20.0) - fixed - craft_h;
            let rh = (((avail - label_row) / rows) - row_gap).clamp(20.0, 34.0);
            let list_h = label_row + rows * (rh + row_gap);
            if fixed + craft_h.max(list_h) <= h - 20.0 {
                s = sc;
                break;
            }
        }
        let g = (s * 0.1).max(4.0);
        let grid_w = 3.0 * s + 2.0 * g;
        let craft_h = 3.0 * s + 2.0 * g;
        let fixed = fixed_no_craft + 4.0 * (s + g);
        let avail = (h - 20.0) - fixed - craft_h;
        let row_h = (((avail - label_row) / rows) - row_gap).clamp(20.0, 34.0);
        let list_h = label_row + rows * (row_h + row_gap);
        let craft_area_h = craft_h.max(list_h);

        let list_w = 300.0;
        let body_w = grid_w + 14.0 + 28.0 + s + 18.0 + list_w;
        let inv_w = 9.0 * s + 8.0 * g;
        let content_w = body_w.max(inv_w);

        let panel_w = (content_w + pad * 2.0).min(w - 20.0);
        let panel_h = (fixed + craft_area_h).min(h - 20.0);
        let px = (w - panel_w) / 2.0;
        let py = (h - panel_h) / 2.0;
        let panel = Rect { x: px, y: py, w: panel_w, h: panel_h };

        let inner_x = px + pad;
        let mut y = py + pad + header;

        let craft: Vec<Rect> = (0..9)
            .map(|i| {
                let cx = (i % 3) as f64;
                let cy = (i / 3) as f64;
                Rect { x: inner_x + cx * (s + g), y: y + cy * (s + g), w: s, h: s }
            })
            .collect();
        let arrow_x = inner_x + grid_w + 14.0;
        let craft_result = Rect { x: arrow_x + 28.0, y: y + s + g, w: s, h: s };

        let list_x = arrow_x + 28.0 + s + 18.0;
        let recipes: Vec<Rect> = (0..crafting::RECIPES.len())
            .map(|i| Rect {
                x: list_x,
                y: y + label_row + i as f64 * (row_h + row_gap),
                w: (px + panel_w - pad - list_x).max(140.0),
                h: row_h,
            })
            .collect();

        y += craft_area_h + 22.0;

        let inv_main: Vec<Rect> = (0..27)
            .map(|i| {
                let cx = (i % 9) as f64;
                let cy = (i / 9) as f64;
                Rect { x: inner_x + cx * (s + g), y: y + label_row + cy * (s + g), w: s, h: s }
            })
            .collect();

        y += label_row + 3.0 * (s + g) + 16.0;

        let inv_hotbar: Vec<Rect> = (0..9)
            .map(|i| Rect { x: inner_x + i as f64 * (s + g), y: y + label_row, w: s, h: s })
            .collect();

        y += label_row + (s + g);

        let btn_y = y + 16.0;
        let close_btn = Rect { x: px + panel_w - pad - 108.0, y: btn_y, w: 108.0, h: 30.0 };
        let clear_btn = Rect { x: close_btn.x - 12.0 - 130.0, y: btn_y, w: 130.0, h: 30.0 };

        Layout {
            hotbar,
            craft,
            craft_result,
            inv_main,
            inv_hotbar,
            recipes,
            clear_btn,
            close_btn,
            panel,
            vitals_hearts,
            vitals_hunger,
            vitals_air,
            label_row,
        }
    }

    // ---------------------------------------------------------------
    // Çizim yardımcıları
    // ---------------------------------------------------------------

    fn draw_icon(&self, id: u16, x: f64, y: f64, size: f64) {
        let ctx = &self.ctx;
        let Some(def) = item_def(id) else { return };
        match def.icon {
            Icon::Tile(tile) => {
                let _ = ctx.draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                    &self.atlas,
                    (tile as f64) * TILE as f64,
                    0.0,
                    TILE as f64,
                    TILE as f64,
                    x,
                    y,
                    size,
                    size,
                );
            }
            Icon::Cubuk => {
                // Çapraz ahşap çubuk.
                let step = size / 8.0;
                for i in 0..8 {
                    self.fill(if i % 2 == 0 { "#b98f52" } else { "#7d5c2f" });
                    ctx.fill_rect(
                        x + size * 0.18 + i as f64 * step,
                        y + size * 0.78 - i as f64 * step,
                        step * 1.2,
                        step * 1.2,
                    );
                }
            }
            Icon::Elma => {
                // Sap + yaprak + gövde.
                self.fill("#7a4a1c");
                ctx.fill_rect(x + size * 0.46, y + size * 0.06, size * 0.12, size * 0.18);
                self.fill("#4f9c3c");
                ctx.fill_rect(x + size * 0.58, y + size * 0.12, size * 0.26, size * 0.12);
                self.fill("#b8352c");
                ctx.begin_path();
                let _ = ctx.arc(x + size * 0.36, y + size * 0.62, size * 0.28, 0.0, std::f64::consts::TAU);
                let _ = ctx.arc(x + size * 0.66, y + size * 0.62, size * 0.28, 0.0, std::f64::consts::TAU);
                ctx.fill();
                ctx.fill_rect(x + size * 0.32, y + size * 0.34, size * 0.38, size * 0.56);
                self.fill("#e2564a");
                ctx.fill_rect(x + size * 0.26, y + size * 0.40, size * 0.16, size * 0.26);
            }
        }
    }

    fn slot(&self, r: Rect, stack: Option<Stack>, highlight: bool, index_label: Option<usize>) {
        let ctx = &self.ctx;
        self.fill(if highlight {
            "rgba(255,255,255,0.20)"
        } else if stack.is_some() {
            "rgba(255,255,255,0.13)"
        } else {
            "rgba(255,255,255,0.07)"
        });
        // Aynı yol hem doldurulur hem çizilir (iki kez kurmaya gerek yok).
        self.rounded(r, 7.0);
        ctx.fill();

        self.stroke(if highlight { "#ffffff" } else { "rgba(255,255,255,0.22)" });
        self.line_width(if highlight { 2.5 } else { 2.0 });
        ctx.stroke();

        if let Some(label) = index_label {
            self.fill("rgba(255,255,255,0.75)");
            self.font("10px 'Segoe UI', sans-serif");
            self.text_layout("left", "top");
            let _ = ctx.fill_text(DIGITS[(label + 1) % 10], r.x + 5.0, r.y + 3.0);
        }

        if let Some(s) = stack {
            let pad = r.w * 0.14;
            self.draw_icon(s.id, r.x + pad, r.y + pad, r.w - pad * 2.0);
            if s.count > 1 {
                self.fill("#ffffff");
                self.font("700 12.5px 'Segoe UI', sans-serif");
                self.text_layout("right", "bottom");
                let _ = ctx.fill_text(&s.count.to_string(), r.x + r.w - 4.0, r.y + r.h - 2.0);
            }
        }
    }

    fn label(&self, text: &str, x: f64, y: f64, size: f64, color: &str, align: &'static str) {
        self.fill(color);
        let mut font = String::with_capacity(40);
        let _ = write!(font, "600 {size}px 'Segoe UI', sans-serif");
        self.font(&font);
        self.text_layout(align, "middle");
        let _ = self.ctx.fill_text(text, x, y);
    }

    fn panel_box(&self, r: Rect, fill: &str, stroke: &str, radius: f64) {
        let ctx = &self.ctx;
        self.fill(fill);
        self.rounded(r, radius);
        ctx.fill();
        self.stroke(stroke);
        self.line_width(1.0);
        ctx.stroke();
    }

    fn button(&self, r: Rect, text: &str, primary: bool) {
        self.panel_box(
            r,
            if primary { "rgba(140,220,90,0.85)" } else { "rgba(255,255,255,0.09)" },
            if primary { "rgba(140,220,90,1)" } else { "rgba(255,255,255,0.16)" },
            8.0,
        );
        self.label(
            text,
            r.x + r.w / 2.0,
            r.y + r.h / 2.0 + 1.0,
            12.5,
            if primary { "#0d1a06" } else { "#dfe7f2" },
            "center",
        );
    }

    /// Önceden çizilmiş ikonu tek `drawImage` ile kopyalar (1:1, tam piksel).
    fn vital_icon(&self, kind: Vital, fill: Fill, x: f64, y: f64) {
        let _ = self
            .ctx
            .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                &self.sprites,
                fill as u8 as f64 * SPRITE_W,
                kind as u8 as f64 * SPRITE_H,
                SPRITE_W,
                SPRITE_H,
                x.round(),
                y.round(),
                SPRITE_W,
                SPRITE_H,
            );
    }

    fn draw_vitals(&self, s: &Survival, l: &Layout) {
        if !s.vitals_visible() {
            return;
        }
        let per = l.vitals_hearts.h + 1.0;
        let (hearts, food, air) = vital_states(s);
        for i in 0..10 {
            let dx = i as f64 * per;
            self.vital_icon(Vital::Heart, hearts[i], l.vitals_hearts.x + dx, l.vitals_hearts.y);
            self.vital_icon(Vital::Food, food[i], l.vitals_hunger.x + dx, l.vitals_hunger.y);
            if let Some(air) = &air {
                self.vital_icon(Vital::Air, air[i], l.vitals_air.x + dx, l.vitals_air.y);
            }
        }
    }

    fn draw_crosshair(&self) {
        let ctx = &self.ctx;
        let (cx, cy) = (self.width / 2.0, self.height / 2.0);
        self.fill("rgba(255,255,255,0.9)");
        ctx.fill_rect(cx - 1.0, cy - 10.0, 2.0, 7.0);
        ctx.fill_rect(cx - 1.0, cy + 3.0, 2.0, 7.0);
        ctx.fill_rect(cx - 10.0, cy - 1.0, 7.0, 2.0);
        ctx.fill_rect(cx + 3.0, cy - 1.0, 7.0, 2.0);
    }

    /// Bildirimin nicemlenmiş saydamlığı (özet ve çizim aynı değeri kullanır).
    fn toast_alpha(&self) -> Option<(&str, u8)> {
        let (text, life) = self.toast.as_ref()?;
        Some((text.as_str(), quantize(life / 0.4)))
    }

    fn draw_toast(&self) {
        let Some((text, alpha)) = self.toast_alpha() else { return };
        let alpha = alpha as f64 / 255.0;
        let ctx = &self.ctx;
        let w = text.chars().count() as f64 * 8.0 + 34.0;
        let h = 30.0;
        let x = (self.width - w) / 2.0;
        let y = self.height - 152.0;
        ctx.set_global_alpha(alpha);
        self.panel_box(
            Rect { x, y, w, h },
            "rgba(10,12,18,0.68)",
            "rgba(255,255,255,0.16)",
            15.0,
        );
        self.label(text, x + w / 2.0, y + h / 2.0 + 1.0, 13.5, "#ffffff", "center");
        ctx.set_global_alpha(1.0);
    }

    /// Ölçüm paneli satırlarını yeniden kullanılan dizelere yazar ve özetini
    /// döndürür (ayırma yok; metin aynıysa panel yeniden çizilmez).
    fn format_debug(&mut self, d: &DebugInfo) -> u64 {
        let l = &mut self.debug_lines;
        for line in l.iter_mut() {
            line.clear();
        }
        let _ = write!(l[0], "{}", d.fps);
        let _ = write!(l[1], "{:.0}, {:.0}, {:.0}", d.pos[0], d.pos[1], d.pos[2]);
        let _ = write!(l[2], "{} yüklü", d.chunks);
        let _ = write!(l[3], "{} bölge", d.render_dist);
        let _ = write!(l[4], "{} · {:.0}k üçgen", d.draws, d.tris as f64 / 1000.0);
        l[5].push_str(d.fly_label);
        l[6].push_str(d.mode_label);
        match d.hand {
            Some(s) => {
                let _ = write!(l[7], "{} ×{}", item_name(s.id), s.count);
            }
            None => l[7].push_str("Boş"),
        }
        let mut h = DefaultHasher::new();
        l.hash(&mut h);
        h.finish()
    }

    /// Sağ üstteki ölçüm paneli.
    fn draw_debug(&self) {
        let ctx = &self.ctx;
        let x = 12.0;
        let mut y = 16.0;
        for (k, v) in DEBUG_KEYS.iter().zip(self.debug_lines.iter()) {
            ctx.set_global_alpha(0.6);
            self.label(k, x, y, 12.5, "#ffffff", "left");
            ctx.set_global_alpha(1.0);
            self.label(v, x + 62.0, y, 12.5, "#ffffff", "left");
            y += 17.0;
        }
    }

    // ---------------------------------------------------------------
    // Ana çizim
    // ---------------------------------------------------------------

    /// Ölçüm paneli dışında ekrana çizilecek her şeyin özeti.
    #[allow(clippy::too_many_arguments)]
    fn main_key(
        &self,
        inv_open: bool,
        inv: &Inventory,
        grid: &Grid,
        survival: &Survival,
        held: Option<Stack>,
        mouse: (f64, f64),
        hud_visible: bool,
    ) -> u64 {
        let mut h = DefaultHasher::new();
        self.width.to_bits().hash(&mut h);
        self.height.to_bits().hash(&mut h);
        (inv_open, hud_visible, survival.dead).hash(&mut h);
        inv.slots.hash(&mut h);
        inv.selected.hash(&mut h);
        grid.hash(&mut h);
        survival.vitals_visible().hash(&mut h);
        vital_states(survival).hash(&mut h);
        quantize(survival.hurt_flash * 0.6).hash(&mut h);
        self.toast_alpha().hash(&mut h);
        held.hash(&mut h);
        if held.is_some() {
            // Taşınan yığın fareyi izler.
            mouse.0.to_bits().hash(&mut h);
            mouse.1.to_bits().hash(&mut h);
        }
        h.finish()
    }

    /// Arayüzü çizer — yalnızca ekranda bir şey değiştiyse.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        inv_open: bool,
        inv: &Inventory,
        grid: &Grid,
        survival: &Survival,
        held: Option<Stack>,
        mouse: (f64, f64),
        hud_visible: bool,
        debug: Option<DebugInfo>,
    ) {
        let show_debug = !inv_open && hud_visible && debug.is_some();
        let debug_key = match &debug {
            Some(d) if show_debug => Some(self.format_debug(d)),
            _ => None,
        };
        let main_key = self.main_key(inv_open, inv, grid, survival, held, mouse, hud_visible);

        if self.drawn_main == Some(main_key) {
            if self.drawn_debug == debug_key {
                return; // hiçbir şey değişmedi: tuval olduğu gibi kalır
            }
            // Yalnızca ölçüm paneli değişti. Panel bölgesinin altında başka
            // bir şey yoksa yalnızca o bölge yeniden çizilir.
            let overlays = survival.dead || quantize(survival.hurt_flash * 0.6) > 0;
            let room = self.height > 420.0 && self.width > DEBUG_RECT.w + 20.0;
            // Fareyi izleyen yığın panelin üstünde olabilir; o zaman tam çizim.
            if show_debug && !overlays && room && held.is_none() && self.drawn_debug.is_some() {
                let r = DEBUG_RECT;
                self.ctx.clear_rect(r.x, r.y, r.w, r.h);
                self.draw_debug();
                self.drawn_debug = debug_key;
                return;
            }
        }
        self.drawn_main = Some(main_key);
        self.drawn_debug = debug_key;

        let ctx = &self.ctx;
        ctx.clear_rect(0.0, 0.0, self.width, self.height);
        let Some(l) = self.layout.as_ref() else { return };

        // Hasar aldığında kırmızı vinyet.
        let hurt = quantize(survival.hurt_flash * 0.6);
        if hurt > 0 {
            let a = hurt as f64 / 255.0;
            let g = ctx
                .create_radial_gradient(
                    self.width / 2.0,
                    self.height / 2.0,
                    self.width.min(self.height) * 0.2,
                    self.width / 2.0,
                    self.height / 2.0,
                    self.width.max(self.height) * 0.75,
                )
                .ok();
            if let Some(g) = g {
                let _ = g.add_color_stop(0.0, "rgba(180,16,16,0)");
                let _ = g.add_color_stop(1.0, &format!("rgba(180,16,16,{a})"));
                ctx.set_fill_style_canvas_gradient(&g);
                self.style.borrow_mut().fill.clear(); // önbellek artık geçersiz
                ctx.fill_rect(0.0, 0.0, self.width, self.height);
            }
        }

        if inv_open {
            self.draw_inventory(inv, grid, l);
        } else {
            if hud_visible {
                self.draw_crosshair();
                for (i, r) in l.hotbar.iter().enumerate() {
                    self.slot(*r, inv.slots[i], i == inv.selected, Some(i));
                }
                self.draw_vitals(survival, l);
                if show_debug {
                    self.draw_debug();
                }
            }
            self.draw_toast();
        }

        if survival.dead {
            self.fill("rgba(72,0,0,0.55)");
            ctx.fill_rect(0.0, 0.0, self.width, self.height);
            self.label(
                "ÖLDÜN",
                self.width / 2.0,
                self.height / 2.0,
                46.0,
                "#ffe0e0",
                "center",
            );
        }

        // Fareyle taşınan yığın
        if let Some(s) = held {
            self.draw_icon(s.id, mouse.0 - 20.0, mouse.1 - 20.0, 40.0);
            if s.count > 1 {
                self.label(
                    &s.count.to_string(),
                    mouse.0 + 16.0,
                    mouse.1 + 16.0,
                    13.0,
                    "#ffffff",
                    "right",
                );
            }
        }
    }

    fn draw_inventory(&self, inv: &Inventory, grid: &Grid, l: &Layout) {
        let ctx = &self.ctx;
        self.fill("rgba(6,8,12,0.58)");
        ctx.fill_rect(0.0, 0.0, self.width, self.height);

        self.panel_box(l.panel, "rgba(18,22,31,0.97)", "rgba(255,255,255,0.14)", 14.0);

        let pad = 18.0;
        self.label(
            "Envanter & Çalışma Tezgahı",
            l.panel.x + pad,
            l.panel.y + pad + 12.0,
            16.0,
            "#eaf0f8",
            "left",
        );
        self.label(
            "Sol tık: al / bırak   ·   Sağ tık: yarım al / tek bırak   ·   E ile kapat",
            l.panel.x + pad,
            l.panel.y + pad + 32.0,
            11.5,
            "rgba(234,240,248,0.55)",
            "left",
        );

        // Tezgah ızgarası
        for (i, r) in l.craft.iter().enumerate() {
            self.slot(*r, grid[i], false, None);
        }
        self.label(
            "➜",
            l.craft_result.x - 14.0,
            l.craft_result.y + l.craft_result.h / 2.0,
            20.0,
            "rgba(234,240,248,0.5)",
            "center",
        );
        let out = crafting::craft_output(grid);
        self.slot(l.craft_result, out.map(|(id, n)| Stack::new(id, n)), out.is_some(), None);

        // Tarifler
        self.label(
            "TARİFLER — tıkla, malzemeler tezgaha yerleşir",
            l.recipes[0].x,
            l.recipes[0].y - l.label_row / 2.0,
            11.5,
            "rgba(234,240,248,0.55)",
            "left",
        );
        for (i, r) in l.recipes.iter().enumerate() {
            let Some(rec) = crafting::RECIPES.get(i) else { continue };
            let ok = crafting::can_craft(inv, rec);
            self.panel_box(
                *r,
                if ok { "rgba(140,220,90,0.13)" } else { "rgba(255,255,255,0.05)" },
                if ok { "rgba(140,220,90,0.45)" } else { "rgba(255,255,255,0.08)" },
                7.0,
            );
            let icon = Rect { x: r.x + 5.0, y: r.y + (r.h - 24.0) / 2.0, w: 24.0, h: 24.0 };
            self.draw_icon(rec.out.0, icon.x, icon.y, icon.w);
            self.label(
                &format!("×{}", rec.out.1),
                icon.x + icon.w + 2.0,
                icon.y + icon.h - 2.0,
                10.0,
                "#ffffff",
                "left",
            );
            self.label(
                item_name(rec.out.0),
                icon.x + icon.w + 18.0,
                r.y + r.h / 2.0,
                12.5,
                "#eaf0f8",
                "left",
            );
            // Girdiler
            let needs = crafting::recipe_needs(rec);
            let mut ix = r.x + r.w - 8.0;
            for (id, n) in needs.iter().rev() {
                self.label(
                    &format!("×{n}"),
                    ix,
                    r.y + r.h / 2.0,
                    10.5,
                    "rgba(234,240,248,0.7)",
                    "right",
                );
                ix -= 26.0;
                self.draw_icon(*id, ix - 18.0, r.y + (r.h - 18.0) / 2.0, 18.0);
                ix -= 24.0;
            }
        }

        // Sırt çantası + kısayol
        self.label(
            "SIRT ÇANTASI",
            l.inv_main[0].x,
            l.inv_main[0].y - l.label_row / 2.0,
            11.5,
            "rgba(234,240,248,0.55)",
            "left",
        );
        for (i, r) in l.inv_main.iter().enumerate() {
            self.slot(*r, inv.slots[i + HOTBAR_SIZE], false, None);
        }
        self.label(
            "KISAYOL ÇUBUĞU",
            l.inv_hotbar[0].x,
            l.inv_hotbar[0].y - l.label_row / 2.0,
            11.5,
            "rgba(234,240,248,0.55)",
            "left",
        );
        for (i, r) in l.inv_hotbar.iter().enumerate() {
            self.slot(*r, inv.slots[i], i == inv.selected, Some(i));
        }

        self.button(l.clear_btn, "Tezgahı boşalt", false);
        self.button(l.close_btn, "Kapat (E)", true);
    }

    // ---------------------------------------------------------------
    // İsabet testi
    // ---------------------------------------------------------------

    /// Tıklanan hedefi döndürür: `("hotbar"|"main"|"craft"|"result"|"recipe"|"btn", index)`.
    pub fn hit(&self, mx: f64, my: f64, inv_open: bool) -> Option<(&'static str, usize)> {
        let l = self.layout.as_ref()?;
        if inv_open {
            if l.close_btn.contains(mx, my) {
                return Some(("close", 0));
            }
            if l.clear_btn.contains(mx, my) {
                return Some(("clear", 0));
            }
            for (i, r) in l.recipes.iter().enumerate() {
                if r.contains(mx, my) {
                    return Some(("recipe", i));
                }
            }
            if l.craft_result.contains(mx, my) {
                return Some(("result", 0));
            }
            for (i, r) in l.craft.iter().enumerate() {
                if r.contains(mx, my) {
                    return Some(("craft", i));
                }
            }
            for (i, r) in l.inv_main.iter().enumerate() {
                if r.contains(mx, my) {
                    return Some(("main", i));
                }
            }
            for (i, r) in l.inv_hotbar.iter().enumerate() {
                if r.contains(mx, my) {
                    return Some(("hotbar", i));
                }
            }
            None
        } else {
            for (i, r) in l.hotbar.iter().enumerate() {
                if r.contains(mx, my) {
                    return Some(("hotbar", i));
                }
            }
            None
        }
    }
}

/// HUD'a aktarılan ölçüm bilgileri (kare başına dize ayırmadan).
pub struct DebugInfo {
    pub fps: u32,
    pub pos: [f64; 3],
    pub chunks: usize,
    pub draws: u32,
    pub tris: u32,
    pub render_dist: i32,
    pub mode_label: &'static str,
    pub fly_label: &'static str,
    pub hand: Option<Stack>,
}

/// Kısayol numaraları (kare başına `to_string` ayırması olmasın).
const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
