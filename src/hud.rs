//! Canvas2D arayüz katmanı.
//!
//! Tüm arayüz (nişangah, kısayol çubuğu, can/açlık/nefes göstergeleri,
//! bildirimler, envanter + çalışma tezgahı ekranı) WebGL tuvalinin üstündeki
//! ikinci bir tuval üzerine çizilir. Yerleşim her karede hesaplanır ve aynı
//! dikdörtgenler hem çizim hem de fare isabet testi için kullanılır.

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
    pub width: f64,
    pub height: f64,
    toast: Option<(String, f64)>,
    pub layout: Option<Layout>,
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
fn make_atlas_canvas(document: &web_sys::Document) -> Result<HtmlCanvasElement, String> {
    let canvas = document
        .create_element("canvas")
        .map_err(|_| "tuval oluşturulamadı")?
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| "tuval dönüştürülemedi")?;
    canvas.set_width(ATLAS_W as u32);
    canvas.set_height(ATLAS_H as u32);
    let ctx = canvas
        .get_context("2d")
        .map_err(|_| "2d bağlamı alınamadı")?
        .ok_or("2d bağlamı yok")?
        .dyn_into::<CanvasRenderingContext2d>()
        .map_err(|_| "2d bağlamı dönüştürülemedi")?;

    let pixels = crate::atlas::build_atlas();
    let clamped = wasm_bindgen::Clamped(pixels.as_slice());
    let img = ImageData::new_with_u8_clamped_array_and_sh(clamped, ATLAS_W as u32, ATLAS_H as u32)
        .map_err(|_| "ImageData oluşturulamadı")?;
    ctx.put_image_data(&img, 0.0, 0.0)
        .map_err(|_| "atlas yazılamadı")?;
    Ok(canvas)
}

impl Hud {
    pub fn new(canvas: &HtmlCanvasElement, document: &web_sys::Document) -> Result<Self, String> {
        let ctx = canvas
            .get_context("2d")
            .map_err(|_| "2d bağlamı alınamadı")?
            .ok_or("2d bağlamı yok")?
            .dyn_into::<CanvasRenderingContext2d>()
            .map_err(|_| "2d bağlamı dönüştürülemedi")?;
        let atlas = make_atlas_canvas(document)?;
        Ok(Self {
            ctx,
            atlas,
            width: 1.0,
            height: 1.0,
            toast: None,
            layout: None,
        })
    }

    pub fn resize(&mut self, w: f64, h: f64) {
        self.width = w;
        self.height = h;
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

    // ---------------------------------------------------------------
    // Yerleşim
    // ---------------------------------------------------------------

    pub fn compute_layout(&self, inv_open: bool) -> Layout {
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
        let icon = 18.0;
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

        let _ = inv_open;
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
                    ctx.set_fill_style_str(if i % 2 == 0 { "#b98f52" } else { "#7d5c2f" });
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
                ctx.set_fill_style_str("#7a4a1c");
                ctx.fill_rect(x + size * 0.46, y + size * 0.06, size * 0.12, size * 0.18);
                ctx.set_fill_style_str("#4f9c3c");
                ctx.fill_rect(x + size * 0.58, y + size * 0.12, size * 0.26, size * 0.12);
                ctx.set_fill_style_str("#b8352c");
                ctx.begin_path();
                let _ = ctx.arc(x + size * 0.36, y + size * 0.62, size * 0.28, 0.0, std::f64::consts::TAU);
                let _ = ctx.arc(x + size * 0.66, y + size * 0.62, size * 0.28, 0.0, std::f64::consts::TAU);
                ctx.fill();
                ctx.fill_rect(x + size * 0.32, y + size * 0.34, size * 0.38, size * 0.56);
                ctx.set_fill_style_str("#e2564a");
                ctx.fill_rect(x + size * 0.26, y + size * 0.40, size * 0.16, size * 0.26);
            }
        }
    }

    fn slot(&self, r: Rect, stack: Option<Stack>, highlight: bool, index_label: Option<usize>) {
        let ctx = &self.ctx;
        ctx.set_fill_style_str(if highlight {
            "rgba(255,255,255,0.20)"
        } else if stack.is_some() {
            "rgba(255,255,255,0.13)"
        } else {
            "rgba(255,255,255,0.07)"
        });
        self.rounded(r, 7.0);
        ctx.fill();

        ctx.set_stroke_style_str(if highlight { "#ffffff" } else { "rgba(255,255,255,0.22)" });
        ctx.set_line_width(if highlight { 2.5 } else { 2.0 });
        self.rounded(r, 7.0);
        ctx.stroke();

        if let Some(label) = index_label {
            ctx.set_fill_style_str("rgba(255,255,255,0.75)");
            ctx.set_font("10px 'Segoe UI', sans-serif");
            ctx.set_text_align("left");
            ctx.set_text_baseline("top");
            let _ = ctx.fill_text(&(label + 1).to_string(), r.x + 5.0, r.y + 3.0);
        }

        if let Some(s) = stack {
            let pad = r.w * 0.14;
            self.draw_icon(s.id, r.x + pad, r.y + pad, r.w - pad * 2.0);
            if s.count > 1 {
                ctx.set_fill_style_str("#ffffff");
                ctx.set_font("700 12.5px 'Segoe UI', sans-serif");
                ctx.set_text_align("right");
                ctx.set_text_baseline("bottom");
                let _ = ctx.fill_text(&s.count.to_string(), r.x + r.w - 4.0, r.y + r.h - 2.0);
            }
        }
    }

    fn label(&self, text: &str, x: f64, y: f64, size: f64, color: &str, align: &str) {
        let ctx = &self.ctx;
        ctx.set_fill_style_str(color);
        ctx.set_font(&format!("600 {size}px 'Segoe UI', sans-serif"));
        ctx.set_text_align(align);
        ctx.set_text_baseline("middle");
        let _ = ctx.fill_text(text, x, y);
    }

    fn panel_box(&self, r: Rect, fill: &str, stroke: &str, radius: f64) {
        let ctx = &self.ctx;
        ctx.set_fill_style_str(fill);
        self.rounded(r, radius);
        ctx.fill();
        ctx.set_stroke_style_str(stroke);
        ctx.set_line_width(1.0);
        self.rounded(r, radius);
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

    /// Piksel ikon çizer; `value` 0..max arasında o ikonun doluluk oranını verir.
    fn pixel_icon(
        &self,
        bmp: &[&str; 8],
        x: f64,
        y: f64,
        size: f64,
        fill: &str,
        dark: &str,
        empty_fill: &str,
        empty_dark: &str,
        value: f64,
    ) {
        let ctx = &self.ctx;
        let px = size / 9.0;
        let half = value > 0.0 && value < 1.0;
        for (ry, row) in bmp.iter().enumerate() {
            for (rx, ch) in row.chars().enumerate() {
                if ch == '0' {
                    continue;
                }
                let empty = value <= 0.0 || (half && rx >= 5);
                let color = if empty {
                    if ch == '2' { empty_dark } else { empty_fill }
                } else if ch == '2' {
                    dark
                } else {
                    fill
                };
                ctx.set_fill_style_str(color);
                ctx.fill_rect(x + rx as f64 * px, y + ry as f64 * px, px + 0.4, px + 0.4);
            }
        }
    }

    fn draw_vitals(&self, s: &Survival, l: &Layout) {
        if !s.vitals_visible() {
            return;
        }
        let icon = l.vitals_hearts.h;
        let per = icon + 1.0;

        for i in 0..10 {
            let v = (s.health as f64 - i as f64 * 2.0) / 2.0;
            self.pixel_icon(
                &HEART_PX,
                l.vitals_hearts.x + i as f64 * per,
                l.vitals_hearts.y,
                icon,
                "#e2453e",
                "#5d1714",
                "#3c3c3c",
                "#232323",
                v,
            );
        }
        for i in 0..10 {
            let v = (s.hunger as f64 - i as f64 * 2.0) / 2.0;
            self.pixel_icon(
                &FOOD_PX,
                l.vitals_hunger.x + i as f64 * per,
                l.vitals_hunger.y,
                icon,
                "#c8763a",
                "#6b3a15",
                "#3c3c3c",
                "#232323",
                v,
            );
        }
        // Nefes yalnızca su altındayken görünür.
        if s.air < MAX_AIR - 0.01 {
            for i in 0..10 {
                let v = (s.air - i as f64 * (MAX_AIR / 10.0)) / (MAX_AIR / 10.0);
                self.pixel_icon(
                    &AIR_PX,
                    l.vitals_air.x + i as f64 * per,
                    l.vitals_air.y,
                    icon,
                    "#9fd8f5",
                    "#3d7fa6",
                    "#3c3c3c",
                    "#232323",
                    v,
                );
            }
        }
    }

    fn draw_crosshair(&self) {
        let ctx = &self.ctx;
        let (cx, cy) = (self.width / 2.0, self.height / 2.0);
        ctx.set_fill_style_str("rgba(255,255,255,0.9)");
        ctx.fill_rect(cx - 1.0, cy - 10.0, 2.0, 7.0);
        ctx.fill_rect(cx - 1.0, cy + 3.0, 2.0, 7.0);
        ctx.fill_rect(cx - 10.0, cy - 1.0, 7.0, 2.0);
        ctx.fill_rect(cx + 3.0, cy - 1.0, 7.0, 2.0);
    }

    fn draw_toast(&self) {
        let Some((text, life)) = self.toast.as_ref() else { return };
        let alpha = (life / 0.4).min(1.0);
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

    /// Sağ üstteki ölçüm paneli.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_debug(
        &self,
        fps: u32,
        pos: [f64; 3],
        chunks: usize,
        draws: u32,
        tris: u32,
        render_dist: i32,
        mode_label: &str,
        fly_label: &str,
        hand: &str,
    ) {
        let ctx = &self.ctx;
        let lines = [
            ("FPS", fps.to_string()),
            (
                "Konum",
                format!("{:.0}, {:.0}, {:.0}", pos[0], pos[1], pos[2]),
            ),
            ("Bölge", format!("{chunks} yüklü")),
            ("Mesafe", format!("{render_dist} bölge")),
            ("Çizim", format!("{draws} · {:.0}k üçgen", tris as f64 / 1000.0)),
            ("Mod", fly_label.to_string()),
            ("Oyun", mode_label.to_string()),
            ("El", hand.to_string()),
        ];
        let x = 12.0;
        let mut y = 16.0;
        for (k, v) in lines.iter() {
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
        let ctx = &self.ctx;
        ctx.clear_rect(0.0, 0.0, self.width, self.height);
        let l = self.compute_layout(inv_open);

        // Hasar aldığında kırmızı vinyet.
        if survival.hurt_flash > 0.001 {
            let a = survival.hurt_flash * 0.6;
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
                ctx.fill_rect(0.0, 0.0, self.width, self.height);
            }
        }

        if inv_open {
            self.draw_inventory(inv, grid, held, mouse, &l);
        } else {
            if hud_visible {
                self.draw_crosshair();
                for (i, r) in l.hotbar.iter().enumerate() {
                    self.slot(*r, inv.slots[i], i == inv.selected, Some(i));
                }
                self.draw_vitals(survival, &l);
                if let Some(d) = &debug {
                    self.draw_debug(
                        d.fps, d.pos, d.chunks, d.draws, d.tris, d.render_dist,
                        &d.mode_label, &d.fly_label, &d.hand,
                    );
                }
            }
            self.draw_toast();
        }

        if survival.dead {
            ctx.set_fill_style_str("rgba(72,0,0,0.55)");
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

        self.layout = Some(l);
    }

    fn draw_inventory(
        &self,
        inv: &Inventory,
        grid: &Grid,
        held: Option<Stack>,
        mouse: (f64, f64),
        l: &Layout,
    ) {
        let ctx = &self.ctx;
        ctx.set_fill_style_str("rgba(6,8,12,0.58)");
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

        let _ = (held, mouse);
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

/// HUD'a aktarılan ölçüm bilgileri.
pub struct DebugInfo {
    pub fps: u32,
    pub pos: [f64; 3],
    pub chunks: usize,
    pub draws: u32,
    pub tris: u32,
    pub render_dist: i32,
    pub mode_label: String,
    pub fly_label: String,
    pub hand: String,
}
