//! Uygulama katmanı: oyun döngüsü, girdi, chunk akışı ve başlatma.
//!
//! Yalnızca `wasm32` hedefinde derlenir.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, HtmlCanvasElement, HtmlElement, KeyboardEvent, MouseEvent, WheelEvent, Window};

use crate::blocks::{item_name, APPLE};
use crate::crafting::{self, Grid};
use crate::gl::Renderer;
use crate::hud::{DebugInfo, Hud};
use crate::inventory::{Inventory, Stack, HOTBAR_SIZE, MAX_STACK};
use crate::mesh;
use crate::player::{Player, REACH};
use crate::raycast::raycast_voxel;
use crate::survival::{Event, GameMode, Survival, TickInput};
use crate::world::{ckey, World, CHUNK_SIZE};

const FOV_Y: f32 = 76.0_f32.to_radians();
const MIN_RENDER_DIST: i32 = 3;
const MAX_RENDER_DIST: i32 = 16;
const KEEP_MARGIN: i32 = 2;
const WORK_BUDGET_MS: f64 = 4.0;
const SENSITIVITY: f64 = 0.0022;

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
    time: f32,
    started: bool,
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

impl Game {
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
        self.want_data.clear();
        self.want_mesh.clear();
        let r = self.render_dist;
        let rd = r + 1;
        for dz in -rd..=rd {
            for dx in -rd..=rd {
                let d2 = dx * dx + dz * dz;
                if d2 > rd * rd {
                    continue;
                }
                self.want_data.push((pcx + dx, pcz + dz, d2));
                if d2 <= r * r {
                    self.want_mesh.push((pcx + dx, pcz + dz, d2));
                }
            }
        }
        self.want_data.sort_by_key(|e| e.2);
        self.want_mesh.sort_by_key(|e| e.2);
        self.data_idx = 0;
        self.mesh_idx = 0;
    }

    fn cleanup_chunks(&mut self, pcx: i32, pcz: i32) {
        let keep = self.render_dist + KEEP_MARGIN;
        let k2 = keep * keep;
        let doomed: Vec<u64> = self
            .world
            .chunks
            .values()
            .filter(|ch| {
                let dx = ch.cx - pcx;
                let dz = ch.cz - pcz;
                dx * dx + dz * dz > k2
            })
            .map(|ch| ckey(ch.cx, ch.cz))
            .collect();
        for key in doomed {
            self.world.unload_chunk(key);
            self.renderer.remove_chunk(key);
        }
    }

    /// Kare başına sınırlı iş: veri üretimi ve mesh kurulumu.
    fn update_chunks(&mut self) {
        let pcx = (self.player.pos[0] / CHUNK_SIZE as f64).floor() as i32;
        let pcz = (self.player.pos[2] / CHUNK_SIZE as f64).floor() as i32;

        if pcx != self.last_pcx || pcz != self.last_pcz || self.force_restream {
            self.last_pcx = pcx;
            self.last_pcz = pcz;
            self.force_restream = false;
            self.rebuild_want(pcx, pcz);
            self.cleanup_chunks(pcx, pcz);
        }

        let t0 = now_ms();

        // 1) Veri üretimi (görüş + 1 halka)
        let mut it = 0;
        while self.data_idx < self.want_data.len() {
            let (cx, cz, _) = self.want_data[self.data_idx];
            self.data_idx += 1;
            if !self.world.chunks.contains_key(&ckey(cx, cz)) {
                self.world.generate_chunk(cx, cz);
            }
            it += 1;
            if it & 7 == 0 && now_ms() - t0 > WORK_BUDGET_MS {
                return;
            }
        }

        // 2) Mesh üretimi — 8 komşusu hazır olanlar
        let mut it = 0;
        while self.mesh_idx < self.want_mesh.len() {
            let (cx, cz, _) = self.want_mesh[self.mesh_idx];
            let key = ckey(cx, cz);
            let needs_mesh = self
                .world
                .chunks
                .get(&key)
                .map(|c| !c.meshed)
                .unwrap_or(false);
            if needs_mesh {
                if let Some((solid, liquid)) = mesh::build_chunk_mesh(&self.world, cx, cz) {
                    if let Some(ch) = self.world.chunks.get_mut(&key) {
                        ch.meshed = true;
                        ch.faces = solid.face_count() + liquid.face_count();
                    }
                    self.renderer.upload_chunk(key, &solid, &liquid);
                } else {
                    return; // komşu eksik: sonraki karede tekrar dene
                }
            }
            self.mesh_idx += 1;
            it += 1;
            if it & 3 == 0 && now_ms() - t0 > WORK_BUDGET_MS {
                return;
            }
        }
    }

    /// Başlatma sırasında tüm chunk'lar bitene kadar çalışır.
    fn boot_step(&mut self) -> bool {
        let t0 = now_ms();
        while self.data_idx < self.want_data.len() && now_ms() - t0 < 12.0 {
            let (cx, cz, _) = self.want_data[self.data_idx];
            self.data_idx += 1;
            self.world.generate_chunk(cx, cz);
            self.boot_done += 1;
        }
        while self.mesh_idx < self.want_mesh.len() && now_ms() - t0 < 12.0 {
            let (cx, cz, _) = self.want_mesh[self.mesh_idx];
            let key = ckey(cx, cz);
            if self.world.chunks.get(&key).map(|c| !c.meshed).unwrap_or(false) {
                if let Some((solid, liquid)) = mesh::build_chunk_mesh(&self.world, cx, cz) {
                    if let Some(ch) = self.world.chunks.get_mut(&key) {
                        ch.meshed = true;
                        ch.faces = solid.face_count() + liquid.face_count();
                    }
                    self.renderer.upload_chunk(key, &solid, &liquid);
                }
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
        self.remesh_around(hit.x, hit.y, hit.z);
        if !self.survival.is_creative() {
            let msg = format!("+{}", item_name(id as u16));
            self.hud.toast(msg);
        }
    }

    fn right_click(&mut self) {
        if self.inv.selected_stack().map(|s| s.id) == Some(APPLE) {
            self.survival.try_eat(&mut self.inv);
            return;
        }
        self.place_block();
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
        self.remesh_around(tx, ty, tz);

        if !self.survival.is_creative() {
            self.inv.consume_selected();
        }
    }

    fn remesh_around(&mut self, x: i32, _y: i32, z: i32) {
        let cx = x >> 4;
        let cz = z >> 4;
        let mut targets = vec![(cx, cz)];
        if x & 15 == 0 {
            targets.push((cx - 1, cz));
        }
        if x & 15 == 15 {
            targets.push((cx + 1, cz));
        }
        if z & 15 == 0 {
            targets.push((cx, cz - 1));
        }
        if z & 15 == 15 {
            targets.push((cx, cz + 1));
        }
        for (tx, tz) in targets {
            let key = ckey(tx, tz);
            if let Some((solid, liquid)) = mesh::build_chunk_mesh(&self.world, tx, tz) {
                if let Some(ch) = self.world.chunks.get_mut(&key) {
                    ch.meshed = true;
                    ch.faces = solid.face_count() + liquid.face_count();
                }
                self.renderer.upload_chunk(key, &solid, &liquid);
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
        let dt = ((now - self.last_time) / 1000.0).clamp(0.0, 0.05);
        self.last_time = now;
        self.time += dt as f32;

        if self.booting {
            self.boot_step();
            self.hud.tick(dt);
            self.present(dt);
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

        self.update_chunks();
        self.update_player(dt);
        self.update_survival(dt);
        self.drain_events();
        self.hud.tick(dt);
        self.present(dt);
    }

    fn update_player(&mut self, dt: f64) {
        if self.survival.dead {
            self.player.vel = [0.0; 3];
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

        let speed = if self.player.flying {
            if sprinting { 30.0 } else { 14.0 }
        } else if sprinting && (k("KeyW") || k("ArrowUp")) {
            7.8
        } else if k("ShiftLeft") || k("ShiftRight") {
            2.4
        } else {
            5.2
        };

        let blend = (dt * 12.0).min(1.0);
        if self.player.flying {
            self.player.vel[0] += (mx * speed - self.player.vel[0]) * blend;
            self.player.vel[2] += (mz * speed - self.player.vel[2]) * blend;
        } else {
            self.player.vel[0] = mx * speed;
            self.player.vel[2] = mz * speed;
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
        self.player.move_with_collision(&self.world, v[0] * dt, v[1] * dt, v[2] * dt);

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

        self.moved_dist = (v[0] * dt).hypot(v[2] * dt);
        self.sprinting = sprinting;
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

    fn present(&mut self, _dt: f64) {
        let (w, h) = (self.hud.width, self.hud.height);
        let aspect = if h > 0.0 { (w / h) as f32 } else { 1.0 };
        let reach = self.reach() as f32;
        let cam = [
            self.player.pos[0] as f32,
            (self.player.pos[1] + self.player.eye_height) as f32,
            self.player.pos[2] as f32,
        ];

        self.renderer.render(
            cam,
            self.player.yaw as f32,
            self.player.pitch as f32,
            FOV_Y,
            aspect,
            reach + 64.0,
            reach * 0.55,
            reach - 2.0,
            self.time,
        );

        self.fps_accum += _dt;
        self.fps_frames += 1;
        if self.fps_accum >= 0.4 {
            self.fps = (self.fps_frames as f64 / self.fps_accum).round() as u32;
            self.fps_accum = 0.0;
            self.fps_frames = 0;
        }

        let hand = match self.inv.selected_stack() {
            Some(s) => format!("{} ×{}", item_name(s.id), s.count),
            None => "Boş".to_string(),
        };

        let debug = DebugInfo {
            fps: self.fps,
            pos: self.player.pos,
            chunks: self.world.chunks.len(),
            draws: self.renderer.stats_draws,
            tris: self.renderer.stats_tris,
            render_dist: self.render_dist,
            mode_label: self.survival.mode.label().to_string(),
            fly_label: if self.player.flying { "Uçuş" } else { "Yürüyüş" }.to_string(),
            hand,
        };

        let inv_open = self.inv_open;
        let inv = &self.inv;
        let grid = &self.grid;
        let survival = &self.survival;
        let held = self.held;
        let mouse = self.mouse;
        let hud_visible = self.hud_visible;
        self.hud
            .draw(inv_open, inv, grid, survival, held, mouse, hud_visible, Some(debug));
    }

    // ---------------------------------------------------------------
    // Girdi
    // ---------------------------------------------------------------

    fn on_key_down(&mut self, e: &KeyboardEvent) {
        let code = e.code();

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
            let now = now_ms();
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
            self.break_block();
        } else if button == 2 {
            self.right_click();
        }
    }

    fn on_mouse_up(&mut self, e: &MouseEvent) {
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
        let _ = e;
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
                    if let Err(missing) = crafting::auto_fill(&mut self.grid, &mut self.inv, rec) {
                        let msg = format!("Yetersiz malzeme: {}", item_name(missing));
                        self.hud.toast(msg);
                    }
                }
            }
            "result" => {
                if let Some((id, n)) = crafting::take_result(&mut self.grid) {
                    match self.held {
                        Some(ref mut h) if h.id == id && h.count as u32 + n as u32 <= MAX_STACK as u32 => {
                            h.count += n;
                        }
                        None => self.held = Some(Stack::new(id, n)),
                        _ => {}
                    }
                }
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
                "\"chunks\":{},\"fps\":{},\"draws\":{},\"tris\":{},",
                "\"selected\":{},\"held\":{},",
                "\"pos\":[{:.2},{:.2},{:.2}],\"yaw\":{:.3},\"pitch\":{:.3},",
                "\"flying\":{},\"onGround\":{},\"renderDist\":{},",
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

pub fn debug_add_item(id: u16, n: u32) {
    with_game(|g| {
        g.inv.add(id, n);
    });
}

pub fn debug_count(id: u16) -> u32 {
    with_game(|g| g.inv.count(id)).unwrap_or(0)
}

// -------------------------------------------------------------------
// Başlatma
// -------------------------------------------------------------------

fn resize_canvases(game: &Game) {
    let (w, h) = (
        game.window.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(800.0),
        game.window.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(600.0),
    );
    for id in ["gl", "hud"] {
        if let Some(el) = game.document.get_element_by_id(id) {
            if let Some(c) = el.dyn_ref::<HtmlCanvasElement>() {
                c.set_width(w as u32);
                c.set_height(h as u32);
            }
        }
    }
    game.renderer.resize(w as i32, h as i32);
}

/// Uygulamayı başlatır. `index.html` içinden `start()` olarak çağrılır
/// (dışa aktarım `lib.rs` içindedir).
pub fn start() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("pencere yok")?;
    let document = window.document().ok_or("doküman yok")?;

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
        want_data: Vec::new(),
        want_mesh: Vec::new(),
        data_idx: 0,
        mesh_idx: 0,
        booting: true,
        boot_total: 0,
        boot_done: 0,
        death_timer: 0.0,
        last_time: now_ms(),
        fps_accum: 0.0,
        fps_frames: 0,
        fps: 0,
        time: 0.0,
        started: false,
        moved_dist: 0.0,
        sprinting: false,
    };

    game.rebuild_want(pcx, pcz);
    game.boot_total = game.want_data.len() + game.want_mesh.len();
    resize_canvases(&game);

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
            resize_canvases(&game);
            let (w, h) = (game.hud.width, game.hud.height);
            game.hud.resize(w, h);
        });
        window.add_event_listener_with_callback("resize", cb.as_ref().unchecked_ref())?;
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
            game.last_time = now_ms();
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
        let w = g.window.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(800.0);
        let h = g.window.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(600.0);
        g.hud.resize(w, h);
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
