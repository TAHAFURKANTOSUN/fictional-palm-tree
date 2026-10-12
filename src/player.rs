//! Oyuncu: hareket, çarpışma ve uçuş.

use crate::blocks::{is_solid, WATER};
use crate::world::World;

pub const GRAVITY: f64 = 30.0;
pub const JUMP_SPEED: f64 = 8.6;
pub const REACH: f64 = 6.0;
/// Çarpışma çözünürlüğü: dünyada kare başına en fazla bu kadar yol alınır.
const STEP: f64 = 0.05;

#[derive(Clone, Copy)]
pub struct Player {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f64,
    pub pitch: f64,
    pub on_ground: bool,
    pub flying: bool,
    pub radius: f64,
    pub height: f64,
    pub eye_height: f64,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            pos: [0.5, 40.0, 0.5],
            vel: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            flying: false,
            radius: 0.3,
            height: 1.8,
            eye_height: 1.62,
        }
    }
}

impl Player {
    pub fn eye(&self) -> [f64; 3] {
        [self.pos[0], self.pos[1] + self.eye_height, self.pos[2]]
    }

    /// Bakış yönü. `YXZ` Euler sırası ile `(pitch, yaw, 0)` dönüşünün -Z eksenine
    /// uygulanmış hâlidir (three.js `getWorldDirection` ile aynı).
    pub fn look_dir(&self) -> [f64; 3] {
        let cp = self.pitch.cos();
        [
            -self.yaw.sin() * cp,
            self.pitch.sin(),
            -self.yaw.cos() * cp,
        ]
    }

    /// Yatay ileri/geri ve sağ/sol birim vektörleri.
    pub fn basis(&self) -> ([f64; 2], [f64; 2]) {
        let sy = self.yaw.sin();
        let cy = self.yaw.cos();
        ([-sy, -cy], [cy, -sy])
    }

    /// AABB ile blok çarpışması.
    pub fn collides(&self, world: &World) -> bool {
        let p = self.pos;
        let r = self.radius;
        let h = self.height;

        let min_x = (p[0] - r).floor() as i32;
        let max_x = (p[0] + r).floor() as i32;
        let min_y = p[1].floor() as i32;
        let max_y = (p[1] + h - 0.0001).floor() as i32;
        let min_z = (p[2] - r).floor() as i32;
        let max_z = (p[2] + r).floor() as i32;

        for y in min_y..=max_y {
            for z in min_z..=max_z {
                for x in min_x..=max_x {
                    if is_solid(world.get_block(x, y, z)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Eksen bazında ayrı ayrı ilerletir; çarpışmada o eksende geri alır.
    /// Küçük adımlara bölündüğü için yüksek hızda bile blok içinden geçilmez.
    pub fn move_with_collision(&mut self, world: &World, dx: f64, dy: f64, dz: f64) {
        let longest = dx.abs().max(dy.abs()).max(dz.abs());
        let steps = (longest / STEP).ceil().max(1.0) as i32;
        let sx = dx / steps as f64;
        let sy = dy / steps as f64;
        let sz = dz / steps as f64;

        for _ in 0..steps {
            if sx != 0.0 {
                self.pos[0] += sx;
                if self.collides(world) {
                    self.pos[0] -= sx;
                    self.vel[0] = 0.0;
                }
            }
            if sz != 0.0 {
                self.pos[2] += sz;
                if self.collides(world) {
                    self.pos[2] -= sz;
                    self.vel[2] = 0.0;
                }
            }
            if sy != 0.0 {
                self.pos[1] += sy;
                if self.collides(world) {
                    self.pos[1] -= sy;
                    if sy < 0.0 {
                        self.on_ground = true;
                    }
                    self.vel[1] = 0.0;
                }
            }
        }
    }

    /// Ayakların bulunduğu blok su mu?
    pub fn feet_in_water(&self, world: &World) -> bool {
        world.get_block(
            self.pos[0].floor() as i32,
            (self.pos[1] + 0.6).floor() as i32,
            self.pos[2].floor() as i32,
        ) == WATER
    }

    /// Göz hizası suyun içinde mi? (boğulma testi)
    pub fn eye_in_water(&self, world: &World) -> bool {
        let e = self.eye();
        world.get_block(e[0].floor() as i32, e[1].floor() as i32, e[2].floor() as i32) == WATER
    }

    /// Verilen blok oyuncunun gövdesine giriyor mu? (yerleştirme engeli)
    pub fn block_intersects(&self, bx: i32, by: i32, bz: i32) -> bool {
        let p = self.pos;
        let r = self.radius;
        (bx as f64) < p[0] + r
            && (bx as f64) + 1.0 > p[0] - r
            && (by as f64) < p[1] + self.height
            && (by as f64) + 1.0 > p[1]
            && (bz as f64) < p[2] + r
            && (bz as f64) + 1.0 > p[2] - r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{AIR, STONE};

    /// Zemin yüzeyi `y = 20` olan düz bir test dünyası.
    fn flat() -> World {
        let mut w = World::new(20261012);
        for dz in -1..=1 {
            for dx in -1..=1 {
                w.generate_chunk(dx, dz);
            }
        }
        for x in -10..10 {
            for z in -10..10 {
                for y in 0..=20 {
                    w.set_block_raw(x, y, z, STONE);
                }
                for y in 21..40 {
                    w.set_block_raw(x, y, z, AIR);
                }
            }
        }
        w
    }

    #[test]
    fn player_rests_on_the_ground() {
        let w = flat();
        let mut p = Player::default();
        p.pos = [0.5, 25.0, 0.5];
        for _ in 0..200 {
            p.on_ground = false;
            p.vel[1] -= GRAVITY * 0.016;
            let v = p.vel;
            p.move_with_collision(&w, v[0] * 0.016, v[1] * 0.016, v[2] * 0.016);
        }
        assert!(p.on_ground, "oyuncu yere inemedi");
        assert!((p.pos[1] - 21.0).abs() < 0.05, "beklenen y≈21, bulunan {}", p.pos[1]);
    }

    #[test]
    fn player_cannot_walk_through_a_wall() {
        let mut w = flat();
        for y in 21..24 {
            for z in -3..3 {
                w.set_block_raw(3, y, z, STONE);
            }
        }
        let mut p = Player::default();
        p.pos = [0.5, 21.0, 0.5];
        for _ in 0..400 {
            p.move_with_collision(&w, 0.05, 0.0, 0.0);
        }
        assert!(p.pos[0] < 3.0, "oyuncu duvarın içinden geçti: {}", p.pos[0]);
    }

    #[test]
    fn fast_movement_does_not_tunnel_through_blocks() {
        let mut w = flat();
        for y in 21..24 {
            w.set_block_raw(4, y, 0, STONE);
        }
        let mut p = Player::default();
        p.pos = [0.5, 21.0, 0.5];
        // Tek karede 8 blok: adımlama olmadan tünelleme olurdu.
        p.move_with_collision(&w, 8.0, 0.0, 0.0);
        assert!(p.pos[0] < 4.0, "hızlı hareket bloktan geçti: {}", p.pos[0]);
    }

    #[test]
    fn flying_player_does_not_fall() {
        let w = flat();
        let mut p = Player::default();
        p.flying = true;
        p.pos = [0.5, 30.0, 0.5];
        let y0 = p.pos[1];
        // Uçuşta dikey hız yerçekiminden etkilenmez; motor bunu uygulamaz.
        p.move_with_collision(&w, 0.0, 0.0, 0.0);
        assert_eq!(p.pos[1], y0);
    }

    #[test]
    fn look_direction_matches_yxz_euler_convention() {
        let mut p = Player::default();
        p.yaw = 0.0;
        p.pitch = 0.0;
        let d = p.look_dir();
        assert!((d[0]).abs() < 1e-9 && (d[1]).abs() < 1e-9 && (d[2] + 1.0).abs() < 1e-9);

        p.pitch = -std::f64::consts::FRAC_PI_2; // tam aşağı
        let d = p.look_dir();
        assert!(d[1] < -0.99, "aşağı bakışta Y negatif olmalı: {d:?}");

        p.yaw = std::f64::consts::FRAC_PI_2; // +X yönü
        p.pitch = 0.0;
        let d = p.look_dir();
        assert!(d[0] < -0.99, "yaw=90° batıya bakmalı: {d:?}");
    }

    #[test]
    fn water_detection_uses_feet_and_eye() {
        let mut w = flat();
        for y in 21..23 {
            w.set_block_raw(0, y, 0, WATER);
        }
        let mut p = Player::default();
        p.pos = [0.5, 21.0, 0.5];
        assert!(p.feet_in_water(&w));
        assert!(p.eye_in_water(&w));
        p.pos = [0.5, 30.0, 0.5];
        assert!(!p.feet_in_water(&w));
        assert!(!p.eye_in_water(&w));
    }

    #[test]
    fn block_intersection_detects_placement_inside_player() {
        let mut p = Player::default();
        p.pos = [0.5, 21.0, 0.5];
        assert!(p.block_intersects(0, 21, 0), "ayak hizasındaki blok engellenmeli");
        assert!(!p.block_intersects(3, 21, 0), "uzaktaki blok engellenmemeli");
    }
}
