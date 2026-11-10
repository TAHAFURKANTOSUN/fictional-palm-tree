//! Uygulama katmanı: oyun döngüsü, girdi, chunk akışı ve başlatma.
//!
//! Yalnızca `wasm32` hedefinde derlenir.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    Document, HtmlCanvasElement, HtmlElement, KeyboardEvent, MouseEvent, Performance, WheelEvent,
    Window,
};

use crate::blocks::{item_def, item_name, APPLE, CAR, FISH, FISHING_ROD, TORCH};
use crate::crafting::{self, FillError, Grid};
use crate::effects::{BoxInst, Mining, Particles};
use crate::fishing::{find_water, Fishing, Phase, CAST_RANGE};
use crate::vehicle::{Car, Controls};
use crate::gl::{FrameEffects, Renderer};
use crate::hud::{DebugInfo, Hud, HudFrame};
use crate::inventory::{Inventory, Stack, HOTBAR_SIZE, MAX_STACK};
use crate::mesh::{self, MeshBuffers};
use crate::pacing::QualityGovernor;
use crate::player::{Player, REACH};
use crate::raycast::raycast_voxel;
use crate::sky::{clock_hours, sky_state, DAY_LENGTH, START_TIME};
use crate::survival::{Event, GameMode, Survival, TickInput};
use crate::world::{ckey, stream_plan, World, CHUNK_SIZE, KEEP_MARGIN};

const FOV_Y: f32 = 76.0_f32.to_radians();
const MIN_RENDER_DIST: i32 = 3;
const MAX_RENDER_DIST: i32 = 16;
/// Chunk akışına kare başına ayrılan süre. Kare zaten geç kalıyorsa
/// (≈45 FPS altı) bütçe kısılır: akış biraz yavaşlar ama kare düşmez.
const WORK_BUDGET_MS: f64 = 2.5;
const WORK_BUDGET_LATE_MS: f64 = 1.0;
const LATE_FRAME_S: f64 = 0.022;
/// Kare başına boşaltılan en fazla chunk (GPU nesnesi silme tek kareye yığılmasın).
const UNLOADS_PER_FRAME: usize = 32;
/// Yerde hızlanma/yavaşlama katsayısı (1/sn): hız hedefe ≈50 ms'de yaklaşır.
const GROUND_ACCEL: f64 = 20.0;
/// Meşale modellerinin çizildiği en uzak mesafe (blok); arabalar iki katı.
const PROP_DRAW_DIST: f64 = 48.0;
const SENSITIVITY: f64 = 0.0022;
/// Ölçüm panelindeki sayıların (konum, çizim sayısı…) yenilenme aralığı.
/// Her karede yenilenseydi arayüz de her karede yeniden çizilirdi.
const DEBUG_SAMPLE_INTERVAL: f64 = 0.25;
/// Yaratıcı modda sol tık basılı tutulurken art arda kırma aralığı.
const CREATIVE_BREAK_INTERVAL: f64 = 0.22;

/// Yüzme (H6): suda yerçekimi yerine yumuşak batma, `Space` ile yukarı çıkış.
const SWIM_SPEED: f64 = 2.6;
const SWIM_SPRINT_SPEED: f64 = 4.0;
/// `Space` basılıyken hedeflenen dikey hız (yukarı).
const SWIM_UP: f64 = 3.6;
/// Hiçbir tuşa basılmazken hedeflenen dikey hız (yavaş batma).
const SWIM_SINK: f64 = 1.5;

struct Game {
    world: World,
    player: Player,
    inv: Inventory,
    survival: Survival,
    grid: Grid,
    held: Option<Stack>,

    renderer: Renderer,
    hud: Hud,
    window: Window,
    document: Document,
    perf: Option<Performance>,
    /// Mesh üretiminin yeniden kullanılan tamponları (chunk başına ayırma yok).
    mesh_buf: MeshBuffers,

    /// Sol tık basılı mı? (kazma)
    mining_held: bool,
    mining: Mining,
    creative_break_cd: f64,
    particles: Particles,
    /// Parçacık renkleri için doku atlasının pikselleri.
    atlas_pixels: Vec<u8>,

    keys: HashMap<String, bool>,
    mouse: (f64, f64),
    pointer_locked: bool,
    dragging: bool,
    drag_button: i16,
    drag_moved: f64,
    last_mouse: (f64, f64),

    last_space_tap: f64,
    jumped: bool,
    broke_block: bool,
    moved_dist: f64,
    sprinting: bool,

    inv_open: bool,
    hud_visible: bool,
    render_dist: i32,
    force_restream: bool,
    last_pcx: i32,
    last_pcz: i32,

    /// Boşaltılmayı bekleyen chunk'lar (karelere yayılır).
    doomed: Vec<(i32, i32)>,
    /// Son karenin gerçek süresi (sn); akış bütçesini belirler.
    last_frame_dt: f64,
    /// Yakındaki meşalelerin konumları (çizim ve ışık için önbellek).
    torches: Vec<[i32; 3]>,
    torches_dirty: bool,
    /// Meşale listesinin en son sıralandığı kamera hücresi.
    torch_sort_cell: Option<[i32; 3]>,
    fishing: Option<Fishing>,
    cars: Vec<Car>,
    /// Sürülen arabanın `cars` içindeki sırası.
    driving: Option<usize>,
    /// Kare başına yeniden doldurulan çizim listeleri.
    fx_boxes: Vec<BoxInst>,
    fx_lights: Vec<[f32; 4]>,
    /// Koşarken görüş açısının yumuşakça genişlemesi (0..1).
    fov_kick: f32,
    /// Yürüme salınımının fazı ve olta savurma animasyonu (1 → 0).
    walk_phase: f64,
    rod_swing: f64,
    /// Günün zamanı, 0..1 (0 = gün doğumu).
    day_time: f64,
    governor: QualityGovernor,
    render_scale: f32,
    gov_time: f64,
    gov_frames: u32,
    plan_dist: i32,
    plan_data: Vec<(i32, i32, i32)>,
    plan_mesh: Vec<(i32, i32, i32)>,
    want_data: Vec<(i32, i32, i32)>,
    want_mesh: Vec<(i32, i32, i32)>,
    data_idx: usize,
    mesh_idx: usize,

    booting: bool,
    boot_total: usize,
    boot_done: usize,

    death_timer: f64,
    last_time: f64,
    fps_accum: f64,
    fps_frames: u32,
    fps: u32,
    debug: DebugInfo,
    debug_timer: f64,
    time: f32,
    started: bool,
}

impl Game {
    /// Milisaniye cinsinden zaman. `Performance` nesnesi bir kez alınıp
    /// saklanır: chunk akışı bunu bütçe denetimi için döngü içinde çağırır.
    fn now_ms(&self) -> f64 {
        self.perf.as_ref().map(|p| p.now()).unwrap_or(0.0)
    }

    fn reach(&self) -> f64 {
        self.render_dist as f64 * CHUNK_SIZE as f64
    }

    fn set_render_dist(&mut self, v: i32) {
        let nv = v.clamp(MIN_RENDER_DIST, MAX_RENDER_DIST);
        if nv == self.render_dist {
            return;
        }
        self.render_dist = nv;
        self.force_restream = true;
        let msg = format!("Görüş mesafesi: {nv} bölge");
        self.hud.toast(msg);
    }

    // ---------------------------------------------------------------
    // Chunk akışı
    // ---------------------------------------------------------------

    fn rebuild_want(&mut self, pcx: i32, pcz: i32) {
        // Sıralı ofset listeleri yalnızca görüş mesafesi değişince hesaplanır;
        // chunk sınırı her geçildiğinde yalnızca ötelenir.
        if self.plan_dist != self.render_dist {
            self.plan_dist = self.render_dist;
            stream_plan(0, 0, self.render_dist, &mut self.plan_data, &mut self.plan_mesh);
        }
        self.want_data.clear();
        self.want_data.extend(self.plan_data.iter().map(|&(x, z, d)| (pcx + x, pcz + z, d)));
        self.want_mesh.clear();
        self.want_mesh.extend(self.plan_mesh.iter().map(|&(x, z, d)| (pcx + x, pcz + z, d)));
        self.data_idx = 0;
        self.mesh_idx = 0;
    }

    /// Chunk'ın mesh'ini üretip GPU'ya yükler. Chunk'ın kendisi ya da 8
    /// komşusundan biri yüklü değilse `false` döner.
    fn mesh_chunk(&mut self, cx: i32, cz: i32) -> bool {
        if !mesh::build_chunk_mesh_into(&self.world, cx, cz, &mut self.mesh_buf) {
            return false;
        }
        let key = ckey(cx, cz);
        if let Some(ch) = self.world.chunks.get_mut(&key) {
            ch.meshed = true;
        }
        self.renderer.upload_chunk(key, cx, cz, &self.mesh_buf);
        true
    }

    fn cleanup_chunks(&mut self, pcx: i32, pcz: i32) {
        let keep = (self.render_dist + KEEP_MARGIN) as i64;
        let k2 = keep * keep;
        self.doomed = self
            .world
            .chunks
            .values()
            .filter(|ch| {
                // `i64`: çok uzağa ışınlanınca (≥ 46341 chunk) fark karesi
                // `i32`'yi taşar, sarılan değer küçük çıkar ve eski
                // chunk'lar hiç boşaltılmazdı.
                let dx = ch.cx as i64 - pcx as i64;
                let dz = ch.cz as i64 - pcz as i64;
                dx * dx + dz * dz > k2
            })
            .map(|ch| (ch.cx, ch.cz))
            .collect();
    }

    /// Bekleyen boşaltmaların bir kısmını yapar. Oyuncu bu arada geri
    /// döndüyse yeniden menzile giren chunk'lar atlanır.
    fn unload_some(&mut self, pcx: i32, pcz: i32) {
        let keep = (self.render_dist + KEEP_MARGIN) as i64;
        for _ in 0..UNLOADS_PER_FRAME {
            let Some((cx, cz)) = self.doomed.pop() else { return };
            let dx = cx as i64 - pcx as i64;
            let dz = cz as i64 - pcz as i64;
            if dx * dx + dz * dz > keep * keep {
                let key = ckey(cx, cz);
                self.world.unload_chunk(key);
                self.renderer.remove_chunk(key);
            }
        }
    }

    /// Kare başına sınırlı iş: veri üretimi ve mesh kurulumu.
    fn update_chunks(&mut self) {
        let pcx = (self.player.pos[0] / CHUNK_SIZE as f64).floor() as i32;
        let pcz = (self.player.pos[2] / CHUNK_SIZE as f64).floor() as i32;
        // Süre, sınır geçişindeki liste yenilemeyi de kapsar: o karede akışa
        // daha az iş kalır.
        let t0 = self.now_ms();

        if pcx != self.last_pcx || pcz != self.last_pcz || self.force_restream {
            self.last_pcx = pcx;
            self.last_pcz = pcz;
            self.force_restream = false;
            self.rebuild_want(pcx, pcz);
            self.cleanup_chunks(pcx, pcz);
            self.torches_dirty = true;
        }

        self.unload_some(pcx, pcz);

        let budget = if self.last_frame_dt > LATE_FRAME_S { WORK_BUDGET_LATE_MS } else { WORK_BUDGET_MS };

        // 1) Veri üretimi: mesh'lenecek chunk'lar ve onların tüm komşuları.
        //    Bütçe her gerçek işten sonra denetlenir (taşma en fazla bir chunk).
        while self.data_idx < self.want_data.len() {
            let (cx, cz, _) = self.want_data[self.data_idx];
            self.data_idx += 1;
            if !self.world.chunks.contains_key(&ckey(cx, cz)) {
                self.world.generate_chunk(cx, cz);
                if self.now_ms() - t0 > budget {
                    return;
                }
            }
        }

        // 2) Mesh üretimi. Veri aşaması bittiği için her chunk'ın 8 komşusu
        //    hazırdır; yine de biri eksikse o chunk atlanır (sıradakileri
        //    bekletmez) ve bir sonraki akış yenilemesinde tekrar denenir.
        while self.mesh_idx < self.want_mesh.len() {
            let (cx, cz, _) = self.want_mesh[self.mesh_idx];
            self.mesh_idx += 1;
            let needs_mesh = self
                .world
                .chunks
                .get(&ckey(cx, cz))
                .map(|c| !c.meshed)
                .unwrap_or(false);
            if needs_mesh {
                self.mesh_chunk(cx, cz);
                if self.now_ms() - t0 > budget {
                    return;
                }
            }
        }
    }

    /// Başlatma sırasında tüm chunk'lar bitene kadar çalışır.
    fn boot_step(&mut self) -> bool {
        let t0 = self.now_ms();
        while self.data_idx < self.want_data.len() && self.now_ms() - t0 < 12.0 {
            let (cx, cz, _) = self.want_data[self.data_idx];
            self.data_idx += 1;
            self.world.generate_chunk(cx, cz);
            self.boot_done += 1;
        }
        while self.mesh_idx < self.want_mesh.len() && self.now_ms() - t0 < 12.0 {
            let (cx, cz, _) = self.want_mesh[self.mesh_idx];
            if self.world.chunks.get(&ckey(cx, cz)).map(|c| !c.meshed).unwrap_or(false) {
                self.mesh_chunk(cx, cz);
            }
            self.mesh_idx += 1;
            self.boot_done += 1;
        }

        let pct = 8.0 + (self.boot_done as f64 / self.boot_total.max(1) as f64) * 90.0;
        if let Some(el) = self.document.get_element_by_id("barFill") {
            if let Some(el) = el.dyn_ref::<HtmlElement>() {
                let _ = el.style().set_property("width", &format!("{pct:.1}%"));
            }
        }
        if let Some(el) = self.document.get_element_by_id("loadingText") {
            el.set_text_content(Some(&format!(
                "Dünya oluşturuluyor… {}/{} bölge",
                self.boot_done, self.boot_total
            )));
        }

        if self.data_idx >= self.want_data.len() && self.mesh_idx >= self.want_mesh.len() {
            if let Some(el) = self.document.get_element_by_id("loading") {
                let _ = el.class_list().add_1("hide");
            }
            if let Some(el) = self.document.get_element_by_id("overlay") {
                let _ = el.class_list().remove_1("hide");
            }
            self.booting = false;
            return true;
        }
        false
    }

    // ---------------------------------------------------------------
    // Oyun kuralları
    // ---------------------------------------------------------------

    fn target(&self) -> Option<crate::raycast::Hit> {
        let eye = self.player.eye();
        let d = self.player.look_dir();
        raycast_voxel(&self.world, eye[0], eye[1], eye[2], d[0], d[1], d[2], REACH)
    }

    fn break_block(&mut self) {
        let Some(hit) = self.target() else { return };
        if hit.y <= 0 {
            self.hud.toast("Ana kaya kırılamaz");
            return;
        }
        let id = self.world.get_block(hit.x, hit.y, hit.z);
        if id == crate::blocks::AIR {
            return;
        }

        if !self.survival.is_creative() {
            if self.inv.add(id as u16, 1) > 0 {
                self.hud.toast("Envanter dolu!");
                return;
            }
            self.broke_block = true;
            if id == crate::blocks::LEAVES && js_random() < 0.14 {
                self.inv.add(APPLE, 1);
            }
        }

        self.world.set_block_raw(hit.x, hit.y, hit.z, crate::blocks::AIR);
        self.torches_dirty = true;
        self.remesh_around(hit.x, hit.y, hit.z);
        self.particles.spawn_break(hit.x, hit.y, hit.z, id, &self.atlas_pixels);
        if !self.survival.is_creative() {
            let msg = format!("+{}", item_name(id as u16));
            self.hud.toast(msg);
        }
    }

    /// Sol tık basıldı: kazma başlar. Hayatta kalmada blok, sertliğine göre
    /// basılı tutuldukça kırılır; yaratıcıda hemen (ve basılı tutulursa
    /// aralıklarla) kırılır.
    fn start_mining(&mut self) {
        self.mining_held = true;
        self.creative_break_cd = 0.0;
        if self.target().is_some_and(|h| h.y <= 0) {
            self.hud.toast("Ana kaya kırılamaz");
        }
    }

    fn stop_mining(&mut self) {
        self.mining_held = false;
        self.mining.reset();
    }

    fn update_mining(&mut self, dt: f64) {
        if !self.mining_held || self.inv_open || self.survival.dead {
            self.mining.reset();
            return;
        }
        let Some(hit) = self.target().filter(|h| h.y > 0) else {
            self.mining.reset();
            return;
        };
        if self.survival.is_creative() {
            self.mining.reset();
            self.creative_break_cd -= dt;
            if self.creative_break_cd <= 0.0 {
                self.creative_break_cd = CREATIVE_BREAK_INTERVAL;
                self.break_block();
            }
            return;
        }
        let id = self.world.get_block(hit.x, hit.y, hit.z);
        if self.mining.advance([hit.x, hit.y, hit.z], id, dt) {
            self.mining.reset();
            self.break_block();
        }
    }

    fn right_click(&mut self) {
        let Some(id) = self.inv.selected_stack().map(|s| s.id) else {
            self.place_block();
            return;
        };
        if item_def(id).is_some_and(|d| d.food > 0) {
            self.survival.try_eat(&mut self.inv);
        } else if id == FISHING_ROD {
            self.use_rod();
        } else if id == CAR {
            self.place_car();
        } else {
            self.place_block();
        }
    }

    // ---------------------------------------------------------------
    // Meşale, olta, araba
    // ---------------------------------------------------------------

    /// Oyuncunun çevresindeki (±4 chunk) meşaleleri oyuncu düzenlemelerinden
    /// toplar. Meşaleler yalnızca oyuncu tarafından konur, bu yüzden düzenleme
    /// kayıtlarına bakmak yeterlidir; blok değişince ya da chunk sınırı
    /// geçilince yenilenir.
    fn refresh_torches(&mut self) {
        self.torches_dirty = false;
        self.torch_sort_cell = None;
        self.torches.clear();
        let size = CHUNK_SIZE as usize;
        for dz in -4..=4 {
            for dx in -4..=4 {
                let (cx, cz) = (self.last_pcx + dx, self.last_pcz + dz);
                let Some(edits) = self.world.edits.get(&ckey(cx, cz)) else { continue };
                for (&idx, &v) in edits {
                    if v == TORCH {
                        self.torches.push([
                            cx * CHUNK_SIZE + (idx % size) as i32,
                            (idx / (size * size)) as i32,
                            cz * CHUNK_SIZE + (idx / size % size) as i32,
                        ]);
                    }
                }
            }
        }
    }

    /// Olta: suya bakarken sağ tık atar; balık vurunca tekrar sağ tık çeker.
    fn use_rod(&mut self) {
        if let Some(f) = self.fishing.take() {
            self.rod_swing = 1.0;
            for _ in 0..6 {
                self.particles.puff(f.bobber, [0.0, 2.0, 0.0], 1.6, [150, 196, 240], 0.4, 0.05, 1.0);
            }
            if f.reel() {
                if self.inv.add(FISH, 1) > 0 {
                    self.hud.toast("Envanter dolu!");
                } else {
                    self.hud.toast("+Balık");
                }
            } else {
                self.hud.toast("Olta boş çekildi");
            }
            return;
        }
        let (eye, dir) = (self.player.eye(), self.player.look_dir());
        match find_water(&self.world, eye, dir, CAST_RANGE) {
            Some(bobber) => {
                let from = self.rod_pose(0.0).1;
                self.fishing = Some(Fishing::new(from, bobber, 2.5 + js_random() * 5.0));
                self.rod_swing = 1.0;
                self.hud.toast("Olta atıldı — balık vurunca sağ tıkla");
            }
            None => self.hud.toast("Oltayı atmak için suya bak"),
        }
    }

    fn update_fishing(&mut self, dt: f64) {
        let Some(f) = self.fishing.as_mut() else { return };
        let p = self.player.pos;
        let far = (f.bobber[0] - p[0]).hypot(f.bobber[2] - p[2]) > CAST_RANGE + 6.0;
        if far || self.inv.selected_stack().map(|s| s.id) != Some(FISHING_ROD) {
            self.fishing = None;
            return;
        }
        let was_flying = f.in_flight();
        let bit = f.tick(dt, 2.5 + js_random() * 5.0);
        let landed = was_flying && !f.in_flight();
        let b = f.bobber;
        if bit {
            self.hud.toast("Balık vurdu! Sağ tıkla");
        }
        // Şamandıra suya değince ve balık vurunca su sıçrar.
        if landed || bit {
            for _ in 0..if bit { 14 } else { 8 } {
                self.particles.puff([b[0], b[1] + 0.05, b[2]], [0.0, 2.6, 0.0], 2.2, [150, 196, 240], 0.45, 0.06, 1.0);
            }
        }
    }

    /// Arabayı bakılan yüzeye bırakır.
    fn place_car(&mut self) {
        let Some(hit) = self.target() else { return };
        let pos = [
            (hit.x + hit.nx) as f64 + 0.5,
            (hit.y + hit.ny) as f64 + 0.05,
            (hit.z + hit.nz) as f64 + 0.5,
        ];
        let car = Car::new(pos, self.player.yaw);
        if car.body.collides(&self.world) {
            self.hud.toast("Araba buraya sığmıyor");
            return;
        }
        self.cars.push(car);
        if !self.survival.is_creative() {
            self.inv.consume_selected();
        }
        self.hud.toast("Araba hazır — yanında R ile bin");
    }

    /// `R`: arabaya bin / in. Eğilirken (Shift) yakındaki arabayı envantere alır.
    fn toggle_drive(&mut self) {
        if let Some(i) = self.driving.take() {
            // Arabanın sağ yanına in.
            let car = &self.cars[i];
            let f = car.forward();
            let p = car.body.pos;
            self.player.pos = [p[0] - f[1] * 1.4, p[1] + 0.3, p[2] + f[0] * 1.4];
            self.player.vel = [0.0; 3];
            self.hud.toast("Arabadan indin");
            return;
        }
        let p = self.player.pos;
        let dist = |c: &Car| {
            let b = c.body.pos;
            ((b[0] - p[0]).powi(2) + (b[1] - p[1]).powi(2) + (b[2] - p[2]).powi(2)).sqrt()
        };
        let nearest = (0..self.cars.len()).min_by(|&a, &b| dist(&self.cars[a]).total_cmp(&dist(&self.cars[b])));
        let Some(i) = nearest.filter(|&i| dist(&self.cars[i]) < 5.0) else {
            self.hud.toast("Yakında araba yok");
            return;
        };
        let sneaking = ["ShiftLeft", "ShiftRight"].iter().any(|k| *self.keys.get(*k).unwrap_or(&false));
        if sneaking {
            if self.inv.add(CAR, 1) > 0 {
                self.hud.toast("Envanter dolu!");
            } else {
                self.cars.swap_remove(i);
                self.hud.toast("+Araba");
            }
            return;
        }
        self.driving = Some(i);
        self.player.flying = false;
        self.fishing = None;
        self.hud.toast("W/S gaz-fren · A/D direksiyon · R in");
    }

    /// Sürüş: tuşlar arabayı yönetir, oyuncu koltukta taşınır.
    fn update_driving(&mut self, dt: f64, i: usize) {
        let active = !self.inv_open;
        let k = |c: &str| active && *self.keys.get(c).unwrap_or(&false);
        let axis = |pos: bool, neg: bool| pos as i32 as f64 - neg as i32 as f64;
        let controls = Controls {
            throttle: axis(k("KeyW") || k("ArrowUp"), k("KeyS") || k("ArrowDown")),
            steer: axis(k("KeyA") || k("ArrowLeft"), k("KeyD") || k("ArrowRight")),
        };
        let p = self.cars[i].body.pos;
        self.world.ensure_around(p[0], p[2]);
        self.cars[i].update(&self.world, dt, Some(controls));

        let car = &self.cars[i];
        // Göz kabinin içinde kalsın diye oyuncu koltuğa "gömülür".
        self.player.pos = [car.body.pos[0], car.body.pos[1] - 0.32, car.body.pos[2]];
        self.player.vel = [0.0; 3];
        self.player.on_ground = car.body.on_ground;
        self.jumped = false;
        self.moved_dist = 0.0;
        self.sprinting = false;
        let kick = (car.speed.abs() / crate::vehicle::MAX_SPEED) as f32;
        self.fov_kick += (kick - self.fov_kick) * (dt as f32 * 4.0).min(1.0);
    }

    /// Sürülmeyen arabalar: yüklü chunk'lardakiler yerçekimiyle yerine oturur.
    fn update_parked_cars(&mut self, dt: f64) {
        for (i, car) in self.cars.iter_mut().enumerate() {
            if self.driving == Some(i) {
                continue;
            }
            let p = car.body.pos;
            let loaded = self.world.chunks.contains_key(&ckey((p[0] as i32) >> 4, (p[2] as i32) >> 4));
            if loaded {
                car.update(&self.world, dt, None);
            }
        }
    }

    /// Görüş doğrultusunun (ileri, sağ, yukarı) birim vektörleri.
    fn view_basis(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let f = self.player.look_dir();
        let r = [self.player.yaw.cos(), 0.0, -self.player.yaw.sin()];
        let u = [r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]];
        (f, r, u)
    }

    /// Gözün önünde, ekranın sağ altında bir nokta (elde tutulan eşyalar için).
    /// Yürürken hafifçe sallanır.
    fn hand_point(&self, right: f64, up: f64, fwd: f64) -> [f64; 3] {
        let (f, r, u) = self.view_basis();
        let e = self.player.eye();
        let sway = (self.walk_phase * 1.1).sin() * 0.012;
        let bob = (self.walk_phase * 2.2).sin().abs() * 0.014;
        let (right, up) = (right + sway, up - bob);
        [
            e[0] + r[0] * right + u[0] * up + f[0] * fwd,
            e[1] + r[1] * right + u[1] * up + f[1] * fwd,
            e[2] + r[2] * right + u[2] * up + f[2] * fwd,
        ]
    }

    /// Elde tutulan oltanın (merkez, uç, yön, eğim) değerleri. `swing` 0..1:
    /// savurma animasyonunda kamış geriye yatıp öne kamçılanır.
    fn rod_pose(&self, swing: f64) -> ([f64; 3], [f64; 3], f64, f64) {
        const LEN: f64 = 0.95;
        let base = self.hand_point(0.30, -0.30, 0.42);
        let yaw = self.player.yaw - 0.10;
        let pitch = (self.player.pitch + 0.55 + (swing * std::f64::consts::PI).sin() * 0.9).min(1.5);
        let d = [-yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos()];
        let at = |t: f64| [base[0] + d[0] * t, base[1] + d[1] * t, base[2] + d[2] * t];
        (at(LEN * 0.5), at(LEN), yaw, pitch)
    }

    /// Sürekli animasyonlar: yürüme salınımı, olta savurma, meşale kıvılcımı,
    /// egzoz dumanı ve tekerlek tozu.
    fn update_animations(&mut self, dt: f64) {
        if self.player.on_ground && self.driving.is_none() {
            self.walk_phase += self.moved_dist * 2.6;
        }
        self.rod_swing = (self.rod_swing - dt * 3.2).max(0.0);

        // Meşaleler: ara sıra yükselen kıvılcım ve ince duman (yakındakiler).
        let p = self.player.pos;
        for i in 0..self.torches.len() {
            let t = self.torches[i];
            let (x, y, z) = (t[0] as f64 + 0.5, t[1] as f64 + 0.72, t[2] as f64 + 0.5);
            if (x - p[0]).abs() + (z - p[2]).abs() > 28.0 {
                continue;
            }
            if self.particles.chance(5.0, dt) {
                self.particles.puff([x, y, z], [0.0, 0.7, 0.0], 0.5, [255, 196, 84], 0.55, 0.035, -0.04);
            }
            if self.particles.chance(2.0, dt) {
                self.particles.puff([x, y + 0.1, z], [0.0, 0.5, 0.0], 0.2, [70, 66, 62], 0.9, 0.05, -0.02);
            }
        }
        // Elde meşale: kıvılcımlar alevden çıkar.
        if self.driving.is_none() && self.inv.selected_stack().map(|s| s.id) == Some(TORCH as u16) && self.particles.chance(4.0, dt) {
            let tip = self.hand_point(0.36, -0.15, 0.6);
            self.particles.puff(tip, [0.0, 0.6, 0.0], 0.4, [255, 196, 84], 0.4, 0.02, -0.04);
        }

        // Arabalar: gaza basınca egzoz dumanı, hızlıyken tekerlek tozu.
        for i in 0..self.cars.len() {
            let (pos, f, speed, throttle, on_ground) = {
                let c = &self.cars[i];
                (c.body.pos, c.forward(), c.speed, c.throttle, c.body.on_ground)
            };
            let back = [pos[0] - f[0] * 1.05, pos[1] + 0.32, pos[2] - f[1] * 1.05];
            if throttle != 0.0 && self.particles.chance(16.0, dt) {
                self.particles.puff(back, [-f[0] * 1.2, 0.5, -f[1] * 1.2], 0.5, [96, 96, 100], 0.6, 0.09, -0.03);
            }
            if on_ground && speed.abs() > 5.0 && self.particles.chance(speed.abs() * 1.4, dt) {
                let color = [150, 128, 96];
                self.particles.puff([back[0], pos[1] + 0.08, back[2]], [0.0, 0.9, 0.0], 1.4, color, 0.4, 0.07, 0.5);
            }
        }
    }

    /// Bu karede çizilecek kutuları ve nokta ışıkları hazırlar.
    fn build_effects(&mut self, cam: [f64; 3]) {
        if self.torches_dirty {
            self.refresh_torches();
        }
        let mut boxes = std::mem::take(&mut self.fx_boxes);
        let mut lights = std::mem::take(&mut self.fx_lights);
        boxes.clear();
        lights.clear();
        let rel = |p: [f64; 3]| [(p[0] - cam[0]) as f32, (p[1] - cam[1]) as f32, (p[2] - cam[2]) as f32];

        // Elde meşale: kameranın kendisi ışık kaynağıdır.
        if self.inv.selected_stack().map(|s| s.id) == Some(TORCH as u16) {
            lights.push([0.0, -0.3, 0.0, 11.0]);
        }

        // Meşaleler: sap + titreyen alev; en yakınları ışık verir.
        // Sıralama yalnızca liste değiştiğinde ya da kamera başka bir bloğa
        // geçtiğinde yenilenir (her karede değil).
        let cell = [cam[0].floor() as i32, cam[1].floor() as i32, cam[2].floor() as i32];
        if self.torch_sort_cell != Some(cell) {
            self.torch_sort_cell = Some(cell);
            self.torches.sort_by(|a, b| {
                let d = |t: &[i32; 3]| (t[0] as f64 + 0.5 - cam[0]).powi(2) + (t[1] as f64 - cam[1]).powi(2) + (t[2] as f64 + 0.5 - cam[2]).powi(2);
                d(a).total_cmp(&d(b))
            });
        }
        for t in self.torches.iter() {
            let (x, y, z) = (t[0] as f64 + 0.5, t[1] as f64, t[2] as f64 + 0.5);
            // Liste yakından uzağa sıralı: menzil dışına çıkınca gerisi de dışarıdadır.
            if (x - cam[0]).powi(2) + (z - cam[2]).powi(2) > PROP_DRAW_DIST * PROP_DRAW_DIST {
                break;
            }
            let flicker = ((self.time * 9.0 + (t[0] * 7 + t[2] * 13) as f32).sin() * 0.5 + 0.5) * 0.25;
            boxes.push(BoxInst { center: [x, y + 0.28, z], half: [0.06, 0.28, 0.06], yaw: 0.0, pitch: 0.0, color: [128, 92, 52], glow: false });
            let s = 0.085 + flicker * 0.05;
            // Alev hafifçe sağa sola eğilir.
            let x = x + (self.time * 3.1 + t[0] as f32).sin() as f64 * 0.018;
            boxes.push(BoxInst { center: [x, y + 0.62 + s as f64 * 0.3, z], half: [s, s, s], yaw: 0.7, pitch: 0.0, color: [255, 190, 70], glow: true });
            boxes.push(BoxInst { center: [x, y + 0.62, z], half: [0.05, 0.07, 0.05], yaw: 0.0, pitch: 0.0, color: [255, 245, 190], glow: true });
            if lights.len() < 8 {
                let r = rel([x, y + 0.7, z]);
                lights.push([r[0], r[1], r[2], 12.5 + flicker]);
            }
        }

        // Arabalar: gövde, kabin, farlar ve dört teker.
        for (i, car) in self.cars.iter().enumerate() {
            // Sürülen arabanın kabini çizilmez: göz içinde durur, tavan görüşü kapatırdı.
            let driven = self.driving == Some(i);
            let mut p = car.body.pos;
            if (p[0] - cam[0]).powi(2) + (p[2] - cam[2]).powi(2) > (PROP_DRAW_DIST * 2.0).powi(2) {
                continue;
            }
            let yaw = car.yaw as f32;
            // Süspansiyon: hızla artan hafif zıplama; gaz/frende gövde yatar.
            let pace = (car.speed.abs() / crate::vehicle::MAX_SPEED).min(1.0);
            p[1] += (car.travelled * 2.4).sin() * 0.025 * pace;
            let lean = (car.throttle * 0.045 * (1.0 - pace * 0.6)) as f32;
            let f = car.forward();
            let right = [-f[1], f[0]];
            // Yerel (sağ, yukarı, ileri) ofsetini dünyaya çevirir.
            let at = |r: f64, u: f64, fw: f64| [p[0] + right[0] * r + f[0] * fw, p[1] + u, p[2] + right[1] * r + f[1] * fw];
            let mut add = |c: [f64; 3], half: [f32; 3], yaw: f32, pitch: f32, color: [u8; 3], glow: bool| {
                boxes.push(BoxInst { center: c, half, yaw, pitch, color, glow });
            };
            add(at(0.0, 0.52, 0.0), [0.62, 0.24, 1.0], yaw, lean, [200, 56, 44], false);
            if !driven {
                add(at(0.0, 0.96, -0.12), [0.54, 0.2, 0.5], yaw, lean, [176, 220, 240], false);
                add(at(0.0, 1.18, -0.12), [0.56, 0.03, 0.52], yaw, lean, [150, 40, 32], false);
            }
            // Tekerlekler yol kadar döner; öndekiler direksiyonla kırılır.
            let spin = (car.travelled / 0.26) as f32;
            for side in [-1.0, 1.0] {
                add(at(0.4 * side, 0.56, 1.0), [0.12, 0.08, 0.02], yaw, lean, [255, 244, 190], true);
                for end in [-0.62, 0.62] {
                    let wheel_yaw = if end > 0.0 { yaw + car.steer as f32 * 0.5 } else { yaw };
                    add(at(0.6 * side, 0.26, end), [0.1, 0.25, 0.25], wheel_yaw, spin, [28, 28, 30], false);
                    // Jant: dönüşü görünür kılan açık renkli çubuk.
                    add(at(0.66 * side, 0.26, end), [0.05, 0.2, 0.05], wheel_yaw, spin, [190, 190, 196], false);
                }
            }
        }

        let held = if self.driving.is_none() && self.hud_visible { self.inv.selected_stack().map(|s| s.id) } else { None };

        // Elde olta: kamış; atılmışsa şamandıra ve kamış ucundan ona misina.
        let mut rod_tip = None;
        if held == Some(FISHING_ROD) {
            let (center, tip, yaw, pitch) = self.rod_pose(self.rod_swing);
            boxes.push(BoxInst { center, half: [0.012, 0.012, 0.475], yaw: yaw as f32, pitch: pitch as f32, color: [166, 124, 69], glow: false });
            rod_tip = Some(tip);
        }
        if let Some(fish) = &self.fishing {
            let b = fish.bobber_now();
            boxes.push(BoxInst { center: [b[0], b[1] + 0.07, b[2]], half: [0.11, 0.09, 0.11], yaw: 0.0, pitch: 0.0, color: [222, 56, 46], glow: false });
            boxes.push(BoxInst { center: [b[0], b[1] + 0.22, b[2]], half: [0.07, 0.05, 0.07], yaw: 0.0, pitch: 0.0, color: [240, 240, 240], glow: false });
            if let Some(tip) = rod_tip {
                let top = [b[0], b[1] + 0.27, b[2]];
                let d = [top[0] - tip[0], top[1] - tip[1], top[2] - tip[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                boxes.push(BoxInst {
                    center: [(tip[0] + top[0]) * 0.5, (tip[1] + top[1]) * 0.5, (tip[2] + top[2]) * 0.5],
                    half: [0.008, 0.008, len as f32 * 0.5],
                    yaw: (-d[0]).atan2(-d[2]) as f32,
                    pitch: (d[1] / len).asin() as f32,
                    color: [236, 240, 245],
                    glow: false,
                });
            }
        }

        // Elde meşale: sap + titreyen alev.
        if held == Some(TORCH as u16) {
            let (yaw, pitch) = (self.player.yaw as f32, self.player.pitch as f32);
            let flick = (self.time * 11.0).sin() * 0.5 + 0.5;
            boxes.push(BoxInst { center: self.hand_point(0.36, -0.33, 0.6), half: [0.02, 0.13, 0.02], yaw, pitch, color: [128, 92, 52], glow: false });
            let s = 0.024 + flick * 0.008;
            boxes.push(BoxInst { center: self.hand_point(0.36, -0.18, 0.6), half: [s, s, s], yaw: yaw + 0.7, pitch, color: [255, 190, 70], glow: true });
            boxes.push(BoxInst { center: self.hand_point(0.36, -0.185, 0.6), half: [0.012, 0.02, 0.012], yaw, pitch, color: [255, 245, 190], glow: true });
        }

        self.fx_boxes = boxes;
        self.fx_lights = lights;
    }

    fn place_block(&mut self) {
        let Some(hit) = self.target() else { return };
        let Some(stack) = self.inv.selected_stack() else {
            self.hud.toast("Seçili yuvada blok yok");
            return;
        };
        let Some(def) = crate::blocks::item_def(stack.id) else { return };
        let Some(placeable) = def.placeable else {
            let msg = format!("{} yerleştirilemez", def.name);
            self.hud.toast(msg);
            return;
        };

        let tx = hit.x + hit.nx;
        let ty = hit.y + hit.ny;
        let tz = hit.z + hit.nz;
        if ty < 0 || ty >= crate::world::WORLD_HEIGHT {
            return;
        }
        let existing = self.world.get_block(tx, ty, tz);
        if existing != crate::blocks::AIR && existing != crate::blocks::WATER {
            return;
        }
        if self.player.block_intersects(tx, ty, tz) {
            return;
        }
        if !self.world.set_block_raw(tx, ty, tz, placeable) {
            return;
        }
        self.torches_dirty = true;
        self.remesh_around(tx, ty, tz);

        if !self.survival.is_creative() {
            self.inv.consume_selected();
        }
    }

    /// Değişen bloğun etkilediği chunk'ları yeniden mesh'ler.
    ///
    /// Yalnızca 4 kenar komşu yetmez: ambient occlusion çapraz komşuyu da okur
    /// (`mesh.rs` → `sc`), bu yüzden köşedeki bir blok değişince çapraz chunk'ın
    /// AO'su da tazelenmelidir; aksi hâlde dikiş izi kalır.
    fn remesh_around(&mut self, x: i32, _y: i32, z: i32) {
        let cx = x >> 4;
        let cz = z >> 4;
        let west = x & 15 == 0;
        let east = x & 15 == 15;
        let north = z & 15 == 0;
        let south = z & 15 == 15;

        let mut targets: Vec<(i32, i32)> = Vec::with_capacity(9);
        targets.push((cx, cz));
        if west {
            targets.push((cx - 1, cz));
        }
        if east {
            targets.push((cx + 1, cz));
        }
        if north {
            targets.push((cx, cz - 1));
        }
        if south {
            targets.push((cx, cz + 1));
        }
        if west && north {
            targets.push((cx - 1, cz - 1));
        }
        if west && south {
            targets.push((cx - 1, cz + 1));
        }
        if east && north {
            targets.push((cx + 1, cz - 1));
        }
        if east && south {
            targets.push((cx + 1, cz + 1));
        }

        for (tx, tz) in targets {
            if !self.mesh_chunk(tx, tz) {
                // Komşusu henüz yüklenmemiş (ör. ışınlanmanın hemen ardından):
                // eski mesh artık bayat, akış komşular gelince yeniden üretsin.
                if let Some(ch) = self.world.chunks.get_mut(&ckey(tx, tz)) {
                    ch.meshed = false;
                }
            }
        }
    }

    fn toggle_fly(&mut self, force: Option<bool>, silent: bool) {
        let on = force.unwrap_or(!self.player.flying);
        if on && !self.survival.is_creative() {
            if !silent {
                self.hud.toast("Uçuş yalnızca Yaratıcı modda (G)");
            }
            return;
        }
        self.player.flying = on;
        self.player.vel[1] = 0.0;
        if on {
            self.player.on_ground = false;
        }
        if !silent {
            self.hud.toast(if on {
                "Uçuş açık — Space yukarı, Shift aşağı, Ctrl hızlı"
            } else {
                "Uçuş kapalı"
            });
        }
    }

    fn drain_events(&mut self) {
        for ev in self.survival.take_events() {
            match ev {
                Event::Damaged { amount, cause } => {
                    let msg = format!("-{amount} can · {cause}");
                    self.hud.toast(msg);
                }
                Event::Died => {
                    self.death_timer = 1.6;
                }
                Event::Respawned => self.hud.toast("Yeniden doğdun"),
                Event::ModeChanged(m) => {
                    let msg = match m {
                        GameMode::Creative => "Yaratıcı mod — uçuş açık, bloklar sınırsız",
                        GameMode::Survival => "Hayatta Kalma — can ve açlık aktif",
                    };
                    self.hud.toast(msg);
                }
                Event::Ate => self.hud.toast("Elma yedin"),
                Event::NotHungry => self.hud.toast("Tokum — aç değilim"),
                Event::NoFood => self.hud.toast("Elinde yemek yok"),
                Event::Healed | Event::Starving => {}
            }
        }
    }

    // ---------------------------------------------------------------
    // Kare
    // ---------------------------------------------------------------

    fn frame(&mut self, now: f64) {
        // Simülasyon adımı 50 ms ile sınırlanır (takılmada oyuncu duvardan
        // geçmesin); FPS sayacı ise gerçek kare süresini kullanır. Aksi hâlde
        // 20 FPS'in altı hiç gösterilemezdi.
        let frame_dt = ((now - self.last_time) / 1000.0).max(0.0);
        let dt = frame_dt.min(0.05);
        self.last_time = now;
        self.last_frame_dt = frame_dt;
        self.time += dt as f32;
        self.update_stats(frame_dt);
        self.update_quality(frame_dt);

        if self.booting {
            self.boot_step();
            self.hud.tick(dt);
            self.present();
            return;
        }

        if self.survival.dead {
            self.death_timer -= dt;
            if self.death_timer <= 0.0 {
                let spawn = self.world.find_spawn();
                self.player.pos = spawn;
                self.player.vel = [0.0; 3];
                self.player.flying = self.survival.is_creative();
                self.survival.respawn();
                self.force_restream = true;
            }
        }

        if self.started {
            self.day_time = (self.day_time + dt / DAY_LENGTH).rem_euclid(1.0);
        }
        self.update_chunks();
        self.update_player(dt);
        self.update_parked_cars(dt);
        self.update_fishing(dt);
        self.update_animations(dt);
        self.update_mining(dt);
        self.particles.update(dt, &self.world);
        self.update_survival(dt);
        self.drain_events();
        self.hud.tick(dt);
        self.present();
    }

    /// Kare hızı düşükse 3B tuvalin çözünürlüğünü kademeli olarak ayarlar
    /// (bkz. `pacing`). Açılışta ve sekme arka plandayken örnek alınmaz.
    fn update_quality(&mut self, frame_dt: f64) {
        if self.booting || !self.started || frame_dt >= 1.0 {
            self.gov_time = 0.0;
            self.gov_frames = 0;
            return;
        }
        self.gov_time += frame_dt;
        self.gov_frames += 1;
        if self.gov_time < 1.0 {
            return;
        }
        let fps = self.gov_frames as f64 / self.gov_time;
        self.gov_time = 0.0;
        self.gov_frames = 0;
        if let Some(scale) = self.governor.sample(fps) {
            self.render_scale = scale;
            resize_canvases(self);
            let msg = format!("Çözünürlük: %{:.0}", scale * 100.0);
            self.hud.toast(msg);
        }
    }

    /// FPS sayacını ve ölçüm panelinin örneklenen sayılarını günceller.
    fn update_stats(&mut self, frame_dt: f64) {
        // Sekme arka plandayken kare gelmez; dönüşteki tek dev "kare" FPS'e
        // katılmaz (yoksa sayaç bir an 0 gösterir).
        if frame_dt < 1.0 {
            self.fps_accum += frame_dt;
            self.fps_frames += 1;
        }
        if self.fps_accum >= 0.4 {
            self.fps = (self.fps_frames as f64 / self.fps_accum).round() as u32;
            self.fps_accum = 0.0;
            self.fps_frames = 0;
        }

        self.debug_timer += frame_dt;
        if self.debug_timer >= DEBUG_SAMPLE_INTERVAL {
            self.debug_timer = 0.0;
            let p = self.player.pos;
            self.debug = DebugInfo {
                fps: self.fps,
                pos: [p[0].round() as i32, p[1].round() as i32, p[2].round() as i32],
                chunks: self.world.chunks.len(),
                draws: self.renderer.stats_draws,
                tris: self.renderer.stats_tris,
                render_dist: self.render_dist,
                clock: ((clock_hours(self.day_time) * 6.0).floor() as u32 * 10) % 1440,
            };
        }
    }

    fn update_player(&mut self, dt: f64) {
        if self.survival.dead {
            self.player.vel = [0.0; 3];
            self.driving = None;
            return;
        }
        if let Some(i) = self.driving {
            self.update_driving(dt, i);
            return;
        }

        let active = !self.inv_open && !self.survival.dead;
        let k = |c: &str| active && *self.keys.get(c).unwrap_or(&false);

        let (fwd, right) = self.player.basis();
        let mut mx = 0.0;
        let mut mz = 0.0;
        if k("KeyW") || k("ArrowUp") {
            mx += fwd[0];
            mz += fwd[1];
        }
        if k("KeyS") || k("ArrowDown") {
            mx -= fwd[0];
            mz -= fwd[1];
        }
        if k("KeyD") || k("ArrowRight") {
            mx += right[0];
            mz += right[1];
        }
        if k("KeyA") || k("ArrowLeft") {
            mx -= right[0];
            mz -= right[1];
        }
        let len = (mx * mx + mz * mz).sqrt();
        if len > 0.0 {
            mx /= len;
            mz /= len;
        }

        let want_sprint = k("ControlLeft") || k("ControlRight");
        let sprinting = want_sprint && self.survival.can_sprint();

        // Ayaklar suyun içindeyse yüzme fiziği devreye girer (H6).
        let in_water = self.player.feet_in_water(&self.world);

        let speed = if self.player.flying {
            if sprinting { 30.0 } else { 14.0 }
        } else if in_water {
            if sprinting { SWIM_SPRINT_SPEED } else { SWIM_SPEED }
        } else if sprinting && (k("KeyW") || k("ArrowUp")) {
            7.8
        } else if k("ShiftLeft") || k("ShiftRight") {
            2.4
        } else {
            5.2
        };

        let blend = (dt * 12.0).min(1.0);
        if self.player.flying || in_water {
            // Suda yatay hız da yumuşatılır (su direnci).
            let b = if self.player.flying { blend } else { (dt * 6.0).min(1.0) };
            self.player.vel[0] += (mx * speed - self.player.vel[0]) * b;
            self.player.vel[2] += (mz * speed - self.player.vel[2]) * b;
        } else {
            // Yerde hız anında değil, çok kısa bir ivmeyle değişir: yön
            // değiştirirken ve dururken kamera sıçramaz.
            let b = (dt * GROUND_ACCEL).min(1.0);
            self.player.vel[0] += (mx * speed - self.player.vel[0]) * b;
            self.player.vel[2] += (mz * speed - self.player.vel[2]) * b;
        }

        self.jumped = false;
        if self.player.flying {
            let mut vy = 0.0;
            if k("Space") {
                vy += speed * 0.78;
            }
            if k("ShiftLeft") || k("ShiftRight") {
                vy -= speed * 0.78;
            }
            self.player.vel[1] += (vy - self.player.vel[1]) * blend;
            self.player.on_ground = false;
        } else if in_water {
            // Yüzme: yerçekimi uygulanmaz, hedef hıza yumuşak geçiş yapılır.
            // `Space` yukarı çeker, bırakılınca oyuncu yavaşça batar. Eskiden
            // yerçekimi aynen işliyordu ve oyuncu dibe batıp boğuluyordu.
            let target = if k("Space") { SWIM_UP } else { -SWIM_SINK };
            self.player.vel[1] += (target - self.player.vel[1]) * (dt * 8.0).min(1.0);
            self.player.on_ground = false;
        } else {
            if k("Space") && self.player.on_ground {
                self.player.vel[1] = crate::player::JUMP_SPEED;
                self.player.on_ground = false;
                self.jumped = true;
            }
            self.player.vel[1] -= crate::player::GRAVITY * dt;
            if self.player.vel[1] < -70.0 {
                self.player.vel[1] = -70.0;
            }
        }

        self.world.ensure_around(self.player.pos[0], self.player.pos[2]);

        self.player.on_ground = false;
        let v = self.player.vel;
        let before = self.player.pos;
        self.player.move_with_collision(&self.world, v[0] * dt, v[1] * dt, v[2] * dt);
        // Gerçekten kat edilen yatay yol: duvara yürümek yorgunluk üretmez.
        self.moved_dist = (self.player.pos[0] - before[0]).hypot(self.player.pos[2] - before[2]);

        // Sıkışıp kalırsak yukarı doğru kurtar.
        let mut guard = 0;
        while self.player.collides(&self.world) && guard < 48 {
            self.player.pos[1] += 0.25;
            guard += 1;
        }

        if self.player.pos[1] < -30.0 {
            let spawn = self.world.find_spawn();
            self.player.pos = spawn;
            self.player.vel = [0.0; 3];
            self.survival.damage(2, "boşluk");
        }

        self.sprinting = sprinting;

        // Koşarken (ve gerçekten ilerlerken) görüş açısı hafifçe genişler.
        let kick = if sprinting && self.moved_dist > 0.0 { 1.0 } else { 0.0 };
        self.fov_kick += (kick - self.fov_kick) * (dt as f32 * 8.0).min(1.0);
    }

    fn update_survival(&mut self, dt: f64) {
        let input = TickInput {
            dt,
            player_y: self.player.pos[1],
            on_ground: self.player.on_ground,
            flying: self.player.flying,
            feet_in_water: self.player.feet_in_water(&self.world),
            eye_in_water: self.player.eye_in_water(&self.world),
            moved_dist: self.moved_dist,
            sprinting: self.sprinting,
            jumped: self.jumped,
            broke_block: self.broke_block,
        };
        self.broke_block = false;
        self.survival.tick(&input);
    }

    fn present(&mut self) {
        let (w, h) = (self.hud.width, self.hud.height);
        let aspect = if h > 0.0 { (w / h) as f32 } else { 1.0 };
        let reach = self.reach() as f32;
        // Kamera konumu `f64` olarak verilir; çizici chunk ofsetlerini bu
        // hassasiyetle hesaplar.
        let cam = [
            self.player.pos[0],
            self.player.pos[1] + self.player.eye_height,
            self.player.pos[2],
        ];

        self.build_effects(cam);
        let sky = sky_state(self.day_time);
        self.renderer.sky_top = sky.top;
        self.renderer.sky_horizon = sky.horizon;
        self.renderer.fog_color = sky.horizon;
        self.renderer.sun_dir = sky.sun_dir;
        self.renderer.light = sky.light;
        self.renderer.stars = sky.stars;

        // Bakılan bloğun çerçevesi (arayüz gizliyken ya da envanterde çizilmez);
        // kazılıyorsa çatlak ilerlemesiyle birlikte.
        let crack = if self.started && self.hud_visible && !self.inv_open && !self.survival.dead {
            self.target().filter(|h| h.y > 0).map(|h| {
                let block = [h.x, h.y, h.z];
                let progress = if self.mining.target == Some(block) { self.mining.progress as f32 } else { 0.0 };
                (block, progress)
            })
        } else {
            None
        };

        self.renderer.render(
            cam,
            self.player.yaw as f32,
            self.player.pitch as f32,
            FOV_Y * (1.0 + 0.07 * self.fov_kick),
            aspect,
            reach + 64.0,
            reach * 0.55,
            reach - 2.0,
            self.time,
            &FrameEffects {
                crack,
                underwater: self.player.eye_in_water(&self.world),
                lights: &self.fx_lights,
                boxes: &self.fx_boxes,
                particles: &self.particles.list,
            },
        );

        self.hud.draw(&HudFrame {
            inv_open: self.inv_open,
            hud_visible: self.hud_visible,
            inv: &self.inv,
            grid: &self.grid,
            survival: &self.survival,
            flying: self.player.flying,
            held: self.held,
            mouse: self.mouse,
            debug: self.debug,
        });
    }

    // ---------------------------------------------------------------
    // Girdi
    // ---------------------------------------------------------------

    fn on_key_down(&mut self, e: &KeyboardEvent) {
        let code = e.code();

        // Koşma tuşu Ctrl olduğu için oyun tuşları tarayıcı kısayollarına
        // dönüşür: Ctrl+S "sayfayı kaydet", Ctrl+D yer imi, Ctrl+F sayfada
        // bul, Ctrl+= yakınlaştırma… Oyun sürerken bunlar engellenir.
        // (Ctrl+W tarayıcıya ayrılmıştır; sayfa onu engelleyemez.)
        if e.ctrl_key() && self.started && is_game_key(&code) {
            e.prevent_default();
        }

        if code == "KeyE" && !e.repeat() {
            e.prevent_default();
            if self.inv_open {
                self.close_inventory();
            } else {
                self.open_inventory();
            }
            return;
        }
        if code == "Escape" && self.inv_open {
            self.close_inventory();
            return;
        }
        if self.inv_open {
            return;
        }

        if e.repeat() {
            if code == "Space" {
                e.prevent_default();
            }
            return;
        }

        self.keys.insert(code.clone(), true);

        if code == "Space" {
            e.prevent_default();
            let now = self.now_ms();
            if now - self.last_space_tap < 280.0 {
                self.toggle_fly(None, false);
            }
            self.last_space_tap = now;
        }

        if let Some(rest) = code.strip_prefix("Digit") {
            if let Ok(n) = rest.parse::<usize>() {
                if (1..=9).contains(&n) {
                    self.inv.select(n - 1);
                    let s = self.inv.selected_stack();
                    let msg = match s {
                        Some(s) => format!("{} ×{}", item_name(s.id), s.count),
                        None => "Boş yuva".to_string(),
                    };
                    self.hud.toast(msg);
                }
            }
        }

        match code.as_str() {
            "KeyF" => self.toggle_fly(None, false),
            "KeyG" => {
                self.survival.toggle_mode(&mut self.inv);
                self.toggle_fly(Some(self.survival.is_creative()), true);
                self.drain_events();
            }
            "BracketRight" | "Equal" => self.set_render_dist(self.render_dist + 1),
            "BracketLeft" | "Minus" => self.set_render_dist(self.render_dist - 1),
            "KeyH" => self.hud_visible = !self.hud_visible,
            "KeyR" => self.toggle_drive(),
            _ => {}
        }
    }

    fn on_key_up(&mut self, e: &KeyboardEvent) {
        self.keys.insert(e.code(), false);
    }

    fn on_mouse_move(&mut self, e: &MouseEvent) {
        self.mouse = (e.client_x() as f64, e.client_y() as f64);
        if self.inv_open {
            return;
        }
        let (dx, dy) = if self.pointer_locked {
            (e.movement_x() as f64, e.movement_y() as f64)
        } else if self.dragging {
            let dx = e.client_x() as f64 - self.last_mouse.0;
            let dy = e.client_y() as f64 - self.last_mouse.1;
            self.last_mouse = (e.client_x() as f64, e.client_y() as f64);
            self.drag_moved += dx.abs() + dy.abs();
            (dx, dy)
        } else {
            return;
        };
        self.player.yaw -= dx * SENSITIVITY;
        self.player.pitch -= dy * SENSITIVITY;
        let lim = std::f64::consts::FRAC_PI_2 - 0.001;
        self.player.pitch = self.player.pitch.clamp(-lim, lim);
    }

    fn on_mouse_down(&mut self, e: &MouseEvent) {
        let button = e.button();
        if self.inv_open {
            let (mx, my) = (e.client_x() as f64, e.client_y() as f64);
            self.inventory_click(mx, my, button == 2);
            return;
        }
        if !self.pointer_locked {
            if self.started {
                self.dragging = true;
                self.drag_button = button;
                self.drag_moved = 0.0;
                self.last_mouse = (e.client_x() as f64, e.client_y() as f64);
            }
            return;
        }
        if button == 0 {
            self.start_mining();
        } else if button == 2 {
            self.right_click();
        }
    }

    fn on_mouse_up(&mut self, e: &MouseEvent) {
        if e.button() == 0 {
            self.stop_mining();
        }
        if !self.pointer_locked && self.dragging {
            if self.drag_moved < 6.0 {
                if self.drag_button == 0 {
                    self.break_block();
                } else if self.drag_button == 2 {
                    self.right_click();
                }
            }
            self.dragging = false;
            self.drag_button = -1;
        }
    }

    fn on_wheel(&mut self, e: &WheelEvent) {
        if self.inv_open || !self.started {
            return;
        }
        let dir = if e.delta_y() > 0.0 { 1 } else { -1 };
        let next = (self.inv.selected as i32 + dir).rem_euclid(HOTBAR_SIZE as i32) as usize;
        self.inv.select(next);
    }

    fn open_inventory(&mut self) {
        if self.inv_open {
            return;
        }
        self.inv_open = true;
        self.stop_mining();
        self.keys.clear();
        self.dragging = false;
        if self.document.pointer_lock_element().is_some() {
            let _ = self.document.exit_pointer_lock();
        }
    }

    fn close_inventory(&mut self) {
        if !self.inv_open {
            return;
        }
        if let Some(h) = self.held.take() {
            let left = self.inv.add(h.id, h.count as u32);
            self.held = if left > 0 { Some(Stack::new(h.id, left as u8)) } else { None };
        }
        self.inv_open = false;
        if let Some(el) = self.document.get_element_by_id("gl") {
            request_pointer_lock(&el);
        }
    }

    /// Envanter ekranındaki tıklamalar (sürükle-bırak).
    fn inventory_click(&mut self, mx: f64, my: f64, right: bool) {
        let Some((kind, index)) = self.hud.hit(mx, my, true) else { return };
        match kind {
            "close" => self.close_inventory(),
            "clear" => crafting::clear_grid(&mut self.grid, &mut self.inv),
            "recipe" => {
                if let Some(rec) = crafting::RECIPES.get(index) {
                    match crafting::auto_fill(&mut self.grid, &mut self.inv, rec) {
                        Ok(()) => {}
                        Err(FillError::Missing(id)) => {
                            let msg = format!("Yetersiz malzeme: {}", item_name(id));
                            self.hud.toast(msg);
                        }
                        Err(FillError::GridNotEmpty) => {
                            self.hud.toast("Envanter dolu — tezgah boşaltılamadı");
                        }
                    }
                }
            }
            "result" => {
                // Sığmıyorsa (elde başka eşya var ya da yığın dolacak) hiçbir
                // şey tüketilmez.
                crafting::take_result_into(&mut self.grid, &mut self.held);
            }
            _ => {
                let target: Option<&mut Option<Stack>> = match kind {
                    "craft" => self.grid.get_mut(index),
                    "main" => self.inv.slots.get_mut(index + HOTBAR_SIZE),
                    "hotbar" => self.inv.slots.get_mut(index),
                    _ => None,
                };
                if let Some(slot) = target {
                    stack_click(slot, &mut self.held, right);
                }
            }
        }
    }
}

/// Oyunun kullandığı bir tuş mu? (`KeyboardEvent.code` değeri)
fn is_game_key(code: &str) -> bool {
    matches!(
        code,
        "KeyW" | "KeyA" | "KeyS" | "KeyD" | "KeyE" | "KeyF" | "KeyG" | "KeyH" | "KeyR" | "Space"
            | "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight"
            | "BracketLeft" | "BracketRight" | "Equal" | "Minus"
    ) || code.starts_with("Digit")
}

/// Sürükle-bırak kuralı: sol tık tümünü alır/bırakır, sağ tık yarım alır / tek bırakır.
fn stack_click(slot: &mut Option<Stack>, held: &mut Option<Stack>, right: bool) {
    match (held.as_mut(), slot.as_mut()) {
        (Some(h), None) => {
            if right {
                *slot = Some(Stack::new(h.id, 1));
                h.count -= 1;
                if h.count == 0 {
                    *held = None;
                }
            } else {
                *slot = Some(*h);
                *held = None;
            }
        }
        (Some(h), Some(s)) => {
            if s.id == h.id {
                let space = MAX_STACK - s.count;
                if space > 0 {
                    let add = if right { 1u8.min(space).min(h.count) } else { space.min(h.count) };
                    s.count += add;
                    h.count -= add;
                    if h.count == 0 {
                        *held = None;
                    }
                }
            } else if !right {
                std::mem::swap(h, s);
            }
        }
        (None, Some(s)) => {
            if right {
                let half = s.count.div_ceil(2);
                *held = Some(Stack::new(s.id, half));
                s.count -= half;
                if s.count == 0 {
                    *slot = None;
                }
            } else {
                *held = Some(*s);
                *slot = None;
            }
        }
        (None, None) => {}
    }
}

fn js_random() -> f64 {
    js_sys::Math::random()
}

#[wasm_bindgen]
extern "C" {
    /// `index.html` içinde tanımlı: fare kilidi isteğini dener, reddi yutar.
    #[wasm_bindgen(js_name = __requestLock)]
    fn request_lock(el: &web_sys::Element);
}

/// Fare kilidini güvenli biçimde ister (yakalanmamış Promise hatası üretmez).
fn request_pointer_lock(el: &web_sys::Element) {
    request_lock(el);
}

// -------------------------------------------------------------------
// Hata ayıklama / otomasyon kancası
// -------------------------------------------------------------------
//
// Tarayıcıdan oyun durumunu okuyup değiştirmeyi sağlar; başsız (headless)
// doğrulama testleri bunu kullanır.

thread_local! {
    static GAME: RefCell<Option<Rc<RefCell<Game>>>> = const { RefCell::new(None) };
}

/// Çalışan oyuna erişim. Oyun yoksa veya o an ödünç alınmışsa `None` döner.
fn with_game<R>(f: impl FnOnce(&mut Game) -> R) -> Option<R> {
    GAME.with(|slot| {
        let borrowed = slot.borrow();
        let rc = borrowed.as_ref()?;
        let mut game = rc.try_borrow_mut().ok()?;
        Some(f(&mut game))
    })
}

pub fn debug_state_json() -> String {
    with_game(|g| {
        let slots: Vec<String> = g
            .inv
            .slots
            .iter()
            .map(|s| match s {
                Some(s) => format!("[{},{}]", s.id, s.count),
                None => "null".to_string(),
            })
            .collect();
        format!(
            concat!(
                "{{",
                "\"health\":{},\"hunger\":{},\"air\":{:.1},",
                "\"mode\":\"{}\",\"dead\":{},\"booting\":{},\"started\":{},",
                "\"chunks\":{},\"gpuChunks\":{},\"wantMesh\":{},\"meshPending\":{},",
                "\"fps\":{},\"draws\":{},\"tris\":{},",
                "\"selected\":{},\"held\":{},",
                "\"pos\":[{:.2},{:.2},{:.2}],\"yaw\":{:.3},\"pitch\":{:.3},",
                "\"flying\":{},\"onGround\":{},\"renderDist\":{},",
                "\"hudW\":{},\"hudH\":{},\"feetInWater\":{},",
                "\"mining\":{:.2},\"particles\":{},\"renderScale\":{:.2},",
                "\"torches\":{},\"lights\":{},\"cars\":{},\"driving\":{},\"fishing\":\"{}\",",
                "\"slots\":[{}]",
                "}}"
            ),
            g.survival.health,
            g.survival.hunger,
            g.survival.air,
            g.survival.mode.label(),
            g.survival.dead,
            g.booting,
            g.started,
            g.world.chunks.len(),
            g.renderer.chunk_count(),
            g.want_mesh.len(),
            // Görüş mesafesinde olup henüz mesh'lenmemiş chunk sayısı; akış
            // bittiğinde 0 olmalıdır.
            g.want_mesh
                .iter()
                .filter(|(cx, cz, _)| !g.world.chunks.get(&ckey(*cx, *cz)).is_some_and(|c| c.meshed))
                .count(),
            g.fps,
            g.renderer.stats_draws,
            g.renderer.stats_tris,
            g.inv.selected,
            match g.held {
                Some(h) => format!("[{},{}]", h.id, h.count),
                None => "null".to_string(),
            },
            g.player.pos[0],
            g.player.pos[1],
            g.player.pos[2],
            g.player.yaw,
            g.player.pitch,
            g.player.flying,
            g.player.on_ground,
            g.render_dist,
            g.hud.width,
            g.hud.height,
            g.player.feet_in_water(&g.world),
            g.mining.progress,
            g.particles.list.len(),
            g.render_scale,
            g.torches.len(),
            g.fx_lights.len(),
            g.cars.len(),
            g.driving.is_some(),
            match g.fishing.as_ref().map(|f| f.phase) {
                None => "none",
                Some(Phase::Waiting(_)) => "waiting",
                Some(Phase::Biting(_)) => "biting",
            },
            slots.join(",")
        )
    })
    .unwrap_or_else(|| "{}".to_string())
}

pub fn debug_set_health(v: i32) {
    with_game(|g| g.survival.health = v.clamp(0, crate::survival::MAX_HEALTH));
}

pub fn debug_set_hunger(v: i32) {
    with_game(|g| g.survival.hunger = v.clamp(0, crate::survival::MAX_HUNGER));
}

pub fn debug_damage(n: i32) {
    with_game(|g| {
        // Testlerde dokunulmazlık engellemesin.
        g.survival.invuln = 0.0;
        g.survival.damage(n, "test");
        g.drain_events();
    });
}

pub fn debug_set_mode(creative: bool) {
    with_game(|g| {
        let m = if creative { GameMode::Creative } else { GameMode::Survival };
        g.survival.set_mode(m, &mut g.inv);
        g.toggle_fly(Some(creative), true);
        g.drain_events();
    });
}

pub fn debug_set_look(yaw: f64, pitch: f64) {
    with_game(|g| {
        g.player.yaw = yaw;
        g.player.pitch = pitch;
    });
}

/// Bakılan yöne blok koymayı dener; başarılıysa `true` döner.
pub fn debug_place() -> bool {
    with_game(|g| {
        let before = g.inv.selected_stack().map(|s| s.count).unwrap_or(0);
        g.place_block();
        let after = g.inv.selected_stack().map(|s| s.count).unwrap_or(0);
        // Yaratıcıda tüketim olmaz; yine de dünyanın değiştiğini anlamak için
        // hedefin boş olup olmadığına bakmıyoruz — yalnızca tüketim farkını veririz.
        before != after || g.survival.is_creative()
    })
    .unwrap_or(false)
}

/// Seçili yuvadan bir adet tüketildi mi? (yerleştirme testi için)
pub fn debug_place_consumed() -> i32 {
    with_game(|g| {
        let before = g.inv.selected_stack().map(|s| s.count).unwrap_or(0) as i32;
        g.place_block();
        let after = g.inv.selected_stack().map(|s| s.count).unwrap_or(0) as i32;
        before - after
    })
    .unwrap_or(0)
}

pub fn debug_break() {
    with_game(|g| g.break_block());
}

pub fn debug_set_time(t: f64) {
    with_game(|g| g.day_time = t.rem_euclid(1.0));
}

pub fn debug_hold_break(on: bool) {
    with_game(|g| if on { g.start_mining() } else { g.stop_mining() });
}

pub fn debug_right_click() {
    with_game(|g| g.right_click());
}

pub fn debug_select(i: usize) {
    with_game(|g| g.inv.select(i));
}

pub fn debug_set_open(open: bool) {
    with_game(|g| {
        if open {
            g.open_inventory();
        } else {
            g.close_inventory();
        }
    });
}

/// Hayatta kalma kurallarını `seconds` saniye boyunca ilerletir.
pub fn debug_sim(seconds: f64) {
    with_game(|g| {
        let steps = (seconds / 0.1).round().max(1.0) as i32;
        for _ in 0..steps {
            let input = TickInput {
                dt: 0.1,
                player_y: g.player.pos[1],
                on_ground: g.player.on_ground,
                flying: g.player.flying,
                feet_in_water: g.player.feet_in_water(&g.world),
                eye_in_water: g.player.eye_in_water(&g.world),
                moved_dist: 0.52,
                sprinting: false,
                jumped: false,
                broke_block: false,
            };
            g.survival.tick(&input);
        }
        g.drain_events();
    });
}

pub fn debug_teleport(x: f64, y: f64, z: f64) {
    with_game(|g| {
        g.player.pos = [x, y, z];
        g.player.vel = [0.0; 3];
        g.force_restream = true;
    });
}

/// Oyuncuyu deniz seviyesinin altındaki bir sütuna ışınlar; yüzme testleri
/// (H6) gerçek bir okyanus bulmak zorunda kalmasın. Uygun sütun bulunursa
/// `true` döner.
pub fn debug_teleport_to_water() -> bool {
    with_game(|g| {
        const GA: f64 = 2.399_963_23;
        for i in 1..4000 {
            let r = (i as f64).sqrt() * 9.0;
            let x = (i as f64 * GA).cos() * r;
            let z = (i as f64 * GA).sin() * r;
            let (xi, zi) = (x.round() as i32, z.round() as i32);
            if g.world.height_at(xi, zi) <= crate::world::SEA_LEVEL - 3 {
                g.player.pos = [
                    xi as f64 + 0.5,
                    (crate::world::SEA_LEVEL - 1) as f64,
                    zi as f64 + 0.5,
                ];
                g.player.vel = [0.0; 3];
                g.player.flying = false;
                g.force_restream = true;
                return true;
            }
        }
        false
    })
    .unwrap_or(false)
}

pub fn debug_add_item(id: u16, n: u32) {
    with_game(|g| {
        g.inv.add(id, n);
    });
}

/// Envanter yuvasını doğrudan ayarlar (`n == 0` yuvayı boşaltır).
pub fn debug_set_slot(slot: usize, id: u16, n: u8) {
    with_game(|g| {
        if let Some(s) = g.inv.slots.get_mut(slot) {
            *s = (n > 0).then(|| Stack::new(id, n.min(MAX_STACK)));
        }
    });
}

pub fn debug_count(id: u16) -> u32 {
    with_game(|g| g.inv.count(id)).unwrap_or(0)
}

// -------------------------------------------------------------------
// Başlatma
// -------------------------------------------------------------------

/// Pencerenin CSS piksel cinsinden iç boyutu.
fn window_size(window: &Window) -> (f64, f64) {
    let w = window.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(800.0);
    let h = window.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(600.0);
    (w, h)
}

/// Tuvalleri, GL görüntü alanını **ve HUD yerleşimini** pencere boyutuna getirir.
///
/// HUD'un `width`/`height` alanları burada güncellenmezse kısayol çubuğu,
/// envanter paneli ve fare isabet testleri eski pencere boyutuna göre
/// hesaplanır; `present()` de en-boy oranını aynı değerlerden aldığı için
/// pencere yeniden boyutlandığında 3B görüntü esner.
fn resize_canvases(game: &mut Game) {
    let (w, h) = window_size(&game.window);
    for id in ["gl", "hud"] {
        if let Some(el) = game.document.get_element_by_id(id) {
            if let Some(c) = el.dyn_ref::<HtmlCanvasElement>() {
                // 3B tuval uyarlanabilir çözünürlükte çizilir, CSS ile
                // pencereye gerilir; arayüz her zaman tam çözünürlüktedir.
                let s = if id == "gl" { game.render_scale as f64 } else { 1.0 };
                c.set_width(((w * s) as u32).max(1));
                c.set_height(((h * s) as u32).max(1));
            }
        }
    }
    let s = game.render_scale as f64;
    game.renderer.resize(((w * s) as i32).max(1), ((h * s) as i32).max(1));
    game.hud.resize(w, h);
}

/// Uygulamayı başlatır. `index.html` içinden `start()` olarak çağrılır
/// (dışa aktarım `lib.rs` içindedir).
pub fn start() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("pencere yok")?;
    let document = window.document().ok_or("doküman yok")?;
    let perf = window.performance();

    let gl_canvas = document
        .get_element_by_id("gl")
        .ok_or("gl tuvali yok")?
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| "gl tuvali dönüştürülemedi")?;
    let hud_canvas = document
        .get_element_by_id("hud")
        .ok_or("hud tuvali yok")?
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| "hud tuvali dönüştürülemedi")?;

    let renderer = Renderer::new(&gl_canvas).map_err(|e| JsValue::from_str(&e))?;
    let hud = Hud::new(&hud_canvas, &document).map_err(|e| JsValue::from_str(&e))?;

    let seed = (js_random() * 1.0e9) as i32;
    let world = World::new(seed);
    let spawn = world.find_spawn();

    let mut player = Player::default();
    player.pos = spawn;
    player.flying = false;

    let mut inv = Inventory::new();
    inv.give_starter_kit();
    inv.select(0);

    let survival = Survival::new();

    let render_dist = 8;
    let pcx = (spawn[0] / CHUNK_SIZE as f64).floor() as i32;
    let pcz = (spawn[2] / CHUNK_SIZE as f64).floor() as i32;

    let mut game = Game {
        world,
        player,
        inv,
        survival,
        grid: [None; 9],
        held: None,
        renderer,
        hud,
        window: window.clone(),
        document: document.clone(),
        last_time: perf.as_ref().map(|p| p.now()).unwrap_or(0.0),
        perf,
        mesh_buf: MeshBuffers::default(),
        mining_held: false,
        mining: Mining::default(),
        creative_break_cd: 0.0,
        particles: Particles::default(),
        atlas_pixels: crate::atlas::build_atlas(),
        keys: HashMap::new(),
        mouse: (0.0, 0.0),
        pointer_locked: false,
        dragging: false,
        drag_button: -1,
        drag_moved: 0.0,
        last_mouse: (0.0, 0.0),
        last_space_tap: -1000.0,
        jumped: false,
        broke_block: false,
        inv_open: false,
        hud_visible: true,
        render_dist,
        force_restream: false,
        last_pcx: pcx,
        last_pcz: pcz,
        doomed: Vec::new(),
        last_frame_dt: 0.0,
        torches: Vec::new(),
        torches_dirty: true,
        torch_sort_cell: None,
        fishing: None,
        cars: Vec::new(),
        driving: None,
        fx_boxes: Vec::new(),
        fx_lights: Vec::new(),
        fov_kick: 0.0,
        walk_phase: 0.0,
        rod_swing: 0.0,
        day_time: START_TIME,
        governor: QualityGovernor::default(),
        render_scale: 1.0,
        gov_time: 0.0,
        gov_frames: 0,
        plan_dist: -1,
        plan_data: Vec::new(),
        plan_mesh: Vec::new(),
        want_data: Vec::new(),
        want_mesh: Vec::new(),
        data_idx: 0,
        mesh_idx: 0,
        booting: true,
        boot_total: 0,
        boot_done: 0,
        death_timer: 0.0,
        fps_accum: 0.0,
        fps_frames: 0,
        fps: 0,
        debug: DebugInfo::default(),
        // İlk karede hemen örneklensin.
        debug_timer: DEBUG_SAMPLE_INTERVAL,
        time: 0.0,
        started: false,
        moved_dist: 0.0,
        sprinting: false,
    };

    game.rebuild_want(pcx, pcz);
    game.boot_total = game.want_data.len() + game.want_mesh.len();
    resize_canvases(&mut game);

    let game = Rc::new(RefCell::new(game));
    GAME.with(|slot| *slot.borrow_mut() = Some(game.clone()));

    // --- olay dinleyicileri ---
    {
        let g = game.clone();
        let cb = Closure::<dyn FnMut(KeyboardEvent)>::new(move |e: KeyboardEvent| {
            g.borrow_mut().on_key_down(&e);
        });
        window.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let g = game.clone();
        let cb = Closure::<dyn FnMut(KeyboardEvent)>::new(move |e: KeyboardEvent| {
            g.borrow_mut().on_key_up(&e);
        });
        window.add_event_listener_with_callback("keyup", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let g = game.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |e: MouseEvent| {
            g.borrow_mut().on_mouse_move(&e);
        });
        window.add_event_listener_with_callback("mousemove", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let g = game.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |e: MouseEvent| {
            g.borrow_mut().on_mouse_down(&e);
        });
        window.add_event_listener_with_callback("mousedown", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let g = game.clone();
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |e: MouseEvent| {
            g.borrow_mut().on_mouse_up(&e);
        });
        window.add_event_listener_with_callback("mouseup", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let g = game.clone();
        let cb = Closure::<dyn FnMut(WheelEvent)>::new(move |e: WheelEvent| {
            g.borrow_mut().on_wheel(&e);
        });
        window.add_event_listener_with_callback("wheel", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let g = game.clone();
        let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            let mut game = g.borrow_mut();
            resize_canvases(&mut game);
        });
        window.add_event_listener_with_callback("resize", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        // Pencere odağı kaybedince (Alt+Tab, başka sekme) basılı tuşların
        // `keyup` olayı hiç gelmez; tuşlar temizlenmezse oyuncu geri
        // dönüldüğünde kendi kendine yürümeye devam eder.
        let g = game.clone();
        let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            if let Ok(mut game) = g.try_borrow_mut() {
                game.keys.clear();
                game.stop_mining();
                game.dragging = false;
                game.drag_button = -1;
            }
        });
        window.add_event_listener_with_callback("blur", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        let g = game.clone();
        let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            let locked = g
                .borrow()
                .document
                .pointer_lock_element()
                .is_some();
            let mut game = g.borrow_mut();
            game.pointer_locked = locked;
            if !locked {
                game.stop_mining();
                game.dragging = false;
                game.drag_button = -1;
            }
            if let Some(el) = game.document.get_element_by_id("overlay") {
                if locked {
                    let _ = el.class_list().add_1("hide");
                } else if !game.inv_open {
                    let _ = el.class_list().remove_1("hide");
                }
            }
        });
        document.add_event_listener_with_callback("pointerlockchange", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    {
        // Başlat düğmesi
        let g = game.clone();
        let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            let g2 = g.borrow();
            if let Some(el) = g2.document.get_element_by_id("gl") {
                request_pointer_lock(&el);
            }
            drop(g2);
            let mut game = g.borrow_mut();
            game.started = true;
            game.last_time = game.now_ms();
            if let Some(el) = game.document.get_element_by_id("overlay") {
                let _ = el.class_list().add_1("hide");
            }
            game.hud.toast("Hayatta Kalma — G ile Yaratıcı moda geç");
        });
        if let Some(btn) = document.get_element_by_id("startBtn") {
            btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        }
        cb.forget();
    }
    {
        let cb = Closure::<dyn FnMut(MouseEvent)>::new(move |e: MouseEvent| {
            e.prevent_default();
        });
        document.add_event_listener_with_callback("contextmenu", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }

    // --- ilk boyutlandırma ---
    {
        let mut g = game.borrow_mut();
        resize_canvases(&mut g);
    }

    // --- oyun döngüsü ---
    let slot: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
    {
        let slot2 = slot.clone();
        let g = game.clone();
        let win = window.clone();
        *slot.borrow_mut() = Some(Closure::<dyn FnMut(f64)>::new(move |t: f64| {
            {
                let mut game = g.borrow_mut();
                game.frame(t);
            }
            if let Some(cb) = slot2.borrow().as_ref() {
                let _ = win.request_animation_frame(cb.as_ref().unchecked_ref());
            }
        }));
    }
    if let Some(cb) = slot.borrow().as_ref() {
        window.request_animation_frame(cb.as_ref().unchecked_ref())?;
    }
    std::mem::forget(slot);

    Ok(())
}
