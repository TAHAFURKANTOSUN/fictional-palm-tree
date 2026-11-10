//! Sürülebilir araba: basit bir kutu gövde, gaz/fren, direksiyon ve tek
//! blokluk basamakları tırmanma. Saf fizik; çizimi `app` kutulara çevirir.

use crate::player::{Player, GRAVITY};
use crate::world::World;

pub const MAX_SPEED: f64 = 13.0;
const REVERSE_SPEED: f64 = 4.5;
const ACCEL: f64 = 1.6;
const COAST: f64 = 0.9;
const STEER_RATE: f64 = 2.2;
/// Arabanın tek hamlede çıkabildiği yükseklik.
const STEP_HEIGHT: f64 = 1.05;

/// Sürücünün girdisi: `throttle` −1 (geri) … 1 (ileri), `steer` −1 (sağ) … 1 (sol).
#[derive(Clone, Copy, Default)]
pub struct Controls {
    pub throttle: f64,
    pub steer: f64,
}

pub struct Car {
    /// Çarpışma gövdesi (`pos` taban merkezidir).
    pub body: Player,
    pub yaw: f64,
    pub speed: f64,
    /// Tekerlek dönüşü için kat edilen yol.
    pub travelled: f64,
    /// Yumuşatılmış direksiyon açısı (−1..1); ön tekerlekler buna göre kırılır.
    pub steer: f64,
    /// Son gaz girdisi (egzoz dumanı ve gövde yatması için).
    pub throttle: f64,
}

impl Car {
    pub fn new(pos: [f64; 3], yaw: f64) -> Self {
        let body = Player { pos, radius: 0.62, height: 1.15, ..Player::default() };
        Self { body, yaw, speed: 0.0, travelled: 0.0, steer: 0.0, throttle: 0.0 }
    }

    /// İleri yönün yatay birim vektörü (oyuncunun bakış kuralıyla aynı).
    pub fn forward(&self) -> [f64; 2] {
        [-self.yaw.sin(), -self.yaw.cos()]
    }

    /// Bir fizik adımı. `controls` yoksa araba boştadır ve yavaşlayarak durur.
    pub fn update(&mut self, world: &World, dt: f64, controls: Option<Controls>) {
        let c = controls.unwrap_or_default();
        self.throttle = c.throttle;
        self.steer += (c.steer - self.steer) * (dt * 9.0).min(1.0);
        if c.throttle != 0.0 {
            let target = if c.throttle > 0.0 { MAX_SPEED } else { -REVERSE_SPEED } * c.throttle.abs();
            self.speed += (target - self.speed) * (dt * ACCEL).min(1.0);
        } else {
            self.speed -= self.speed * (dt * COAST).min(1.0);
            if self.speed.abs() < 0.05 {
                self.speed = 0.0;
            }
        }
        if self.body.feet_in_water(world) {
            self.speed -= self.speed * (dt * 3.0).min(1.0);
        }
        // Direksiyon yalnızca hareket hâlinde etkilidir; geri giderken ters döner.
        self.yaw += c.steer * STEER_RATE * dt * (self.speed / 4.0).clamp(-1.0, 1.0);

        let f = self.forward();
        let (dx, dz) = (f[0] * self.speed * dt, f[1] * self.speed * dt);
        let want = dx.hypot(dz);
        let start = self.body.pos;
        self.body.move_with_collision(world, dx, 0.0, dz);
        let moved = (self.body.pos[0] - start[0]).hypot(self.body.pos[2] - start[2]);

        if want > 1e-6 && moved < want * 0.6 {
            // Önü kapalı: tek blokluk basamaksa üstüne çık, değilse dur.
            let blocked = self.body.pos;
            self.body.pos = [start[0], start[1] + STEP_HEIGHT, start[2]];
            let mut climbed = false;
            if self.body.on_ground && !self.body.collides(world) {
                self.body.move_with_collision(world, dx, 0.0, dz);
                let up = (self.body.pos[0] - start[0]).hypot(self.body.pos[2] - start[2]);
                climbed = up > moved + want * 0.3;
            }
            if !climbed {
                self.body.pos = blocked;
                self.speed *= 0.3;
            }
        }
        self.travelled += (self.body.pos[0] - start[0]).hypot(self.body.pos[2] - start[2]) * self.speed.signum();

        self.body.vel[1] = (self.body.vel[1] - GRAVITY * dt).max(-60.0);
        self.body.on_ground = false;
        let vy = self.body.vel[1];
        self.body.move_with_collision(world, 0.0, vy * dt, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{AIR, STONE};

    /// Zemin yüzeyi `y = 21` olan düz dünya.
    fn flat() -> World {
        let mut w = World::new(20261012);
        for dz in -2..=2 {
            for dx in -2..=2 {
                w.generate_chunk(dx, dz);
            }
        }
        for x in -30..30 {
            for z in -30..30 {
                for y in 0..=20 {
                    w.set_block_raw(x, y, z, STONE);
                }
                for y in 21..45 {
                    w.set_block_raw(x, y, z, AIR);
                }
            }
        }
        w
    }

    const GAS: Option<Controls> = Some(Controls { throttle: 1.0, steer: 0.0 });

    fn drive(car: &mut Car, w: &World, seconds: f64, c: Option<Controls>) {
        for _ in 0..(seconds / 0.016) as usize {
            car.update(w, 0.016, c);
        }
    }

    #[test]
    fn car_settles_on_the_ground_and_coasts_to_a_stop() {
        let w = flat();
        let mut car = Car::new([0.5, 24.0, 0.5], 0.0);
        drive(&mut car, &w, 1.5, None);
        assert!(car.body.on_ground && (car.body.pos[1] - 21.0).abs() < 0.06, "y={}", car.body.pos[1]);
        drive(&mut car, &w, 2.0, GAS);
        assert!(car.speed > MAX_SPEED * 0.8);
        drive(&mut car, &w, 8.0, None);
        assert_eq!(car.speed, 0.0, "gaz bırakılınca durmalı");
    }

    #[test]
    fn car_drives_forward_and_steers() {
        let w = flat();
        // yaw = 0 → ileri -Z.
        let mut car = Car::new([0.5, 21.0, 20.5], 0.0);
        drive(&mut car, &w, 2.0, GAS);
        assert!(car.body.pos[2] < 12.0, "ileri gitmedi: z={}", car.body.pos[2]);
        assert!((car.body.pos[0] - 0.5).abs() < 1e-6);
        let yaw = car.yaw;
        let wheel = car.travelled;
        drive(&mut car, &w, 0.5, Some(Controls { throttle: 1.0, steer: 1.0 }));
        assert!(car.yaw > yaw + 0.5, "direksiyon çalışmadı");
        assert!(car.steer > 0.9, "ön tekerlekler kırılmalı");
        assert!(car.travelled > wheel + 3.0, "tekerlekler yol kadar dönmeli");
        drive(&mut car, &w, 0.5, GAS);
        assert!(car.steer.abs() < 0.05, "direksiyon bırakılınca düzelmeli");
    }

    #[test]
    fn car_climbs_single_steps_but_not_walls() {
        let mut w = flat();
        // z = 10'da bir blokluk basamak (tüm genişlikte).
        for x in -30..30 {
            for z in -30..=10 {
                w.set_block_raw(x, 21, z, STONE);
            }
        }
        // z = 0'da, basamağın üstünde üç blokluk duvar.
        for x in -30..30 {
            for y in 22..=24 {
                w.set_block_raw(x, y, 0, STONE);
            }
        }
        let mut car = Car::new([0.5, 21.0, 20.5], 0.0);
        drive(&mut car, &w, 5.0, GAS);
        assert!(car.body.pos[2] < 9.0, "basamağı çıkamadı: z={}", car.body.pos[2]);
        assert!((car.body.pos[1] - 22.0).abs() < 0.1, "basamağın üstünde olmalı: y={}", car.body.pos[1]);
        assert!(car.body.pos[2] > 1.0, "duvarın içinden geçti: z={}", car.body.pos[2]);
        assert!(car.body.pos[1] < 22.2, "duvara tırmandı: y={}", car.body.pos[1]);
    }
}
