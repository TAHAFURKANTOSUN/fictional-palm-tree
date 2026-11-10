//! Blok kırma: kazma ilerlemesi ve kırılınca saçılan parçacıklar.
//!
//! Saf çekirdektedir (tarayıcı API'si yok): kurallar `cargo test` ile
//! doğrulanır, çizimi `gl` modülü yapar.

use crate::atlas::{ATLAS_W, TILE};
use crate::blocks::*;
use crate::world::World;

/// Bloğu elle kırmanın sürdüğü saniye. Kırılamayan bloklar için `None`.
pub fn break_time(id: u8) -> Option<f64> {
    match id {
        AIR | WATER | BEDROCK => None,
        TORCH => Some(0.08),
        LEAVES => Some(0.25),
        GLASS => Some(0.35),
        GRASS | DIRT | SAND => Some(0.55),
        LOG | PLANKS | CRAFT_TABLE => Some(0.9),
        STONE | COBBLE | BRICK => Some(1.3),
        _ => Some(0.6),
    }
}

/// Basılı tutulan sol tıkın kazma durumu.
#[derive(Default)]
pub struct Mining {
    /// Kazılmakta olan blok.
    pub target: Option<[i32; 3]>,
    /// 0 → 1; 1'e ulaşınca blok kırılır.
    pub progress: f64,
}

impl Mining {
    pub fn reset(&mut self) {
        self.target = None;
        self.progress = 0.0;
    }

    /// Çatlak kaplamasının çizileceği blok ve ilerlemesi.
    pub fn crack(&self) -> Option<([i32; 3], f32)> {
        self.target.filter(|_| self.progress > 0.0).map(|t| (t, self.progress as f32))
    }

    /// `target` bloğunu `dt` kadar kazar. Hedef değiştiyse ilerleme sıfırdan
    /// başlar. Blok kırıldıysa `true` döner (durumu çağıran sıfırlar).
    pub fn advance(&mut self, target: [i32; 3], id: u8, dt: f64) -> bool {
        let Some(time) = break_time(id) else {
            self.reset();
            return false;
        };
        if self.target != Some(target) {
            self.target = Some(target);
            self.progress = 0.0;
        }
        self.progress += dt / time;
        self.progress >= 1.0
    }
}

/// Chunk mesh'i dışında çizilen düz renkli kutu (meşale, araba, şamandıra).
#[derive(Clone, Copy, Debug)]
pub struct BoxInst {
    pub center: [f64; 3],
    /// Yarı boyutlar (x, y, z).
    pub half: [f32; 3],
    /// Dikey eksen etrafında dönüş (oyuncunun `yaw` kuralıyla).
    pub yaw: f32,
    /// Yerel X ekseni etrafında dönüş, `yaw`'dan önce uygulanır: pozitif
    /// değer kutunun ön (−Z) ucunu yukarı kaldırır. Tekerlek dönüşü, olta
    /// kamışı ve misina bununla yönlendirilir.
    pub pitch: f32,
    pub color: [u8; 3],
    /// Kendi ışığını yayar: gece kararmaz (alev).
    pub glow: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    /// Kalan ömür (saniye).
    pub life: f64,
    /// Dünya birimiyle kenar uzunluğu.
    pub size: f32,
    pub color: [u8; 3],
    /// Yerçekimi çarpanı: 1 düşen moloz, 0 süzülen duman, negatif yükselen kıvılcım.
    pub gravity: f32,
}

const MAX_PARTICLES: usize = 512;
const PARTICLES_PER_BREAK: usize = 22;
const PARTICLE_GRAVITY: f64 = 22.0;

/// Kırılan bloklardan saçılan parçacıklar.
pub struct Particles {
    pub list: Vec<Particle>,
    rng: u32,
}

impl Default for Particles {
    fn default() -> Self {
        Self { list: Vec::new(), rng: 0x2545_f491 }
    }
}

impl Particles {
    /// `[0, 1)` aralığında sözde rastgele sayı (xorshift).
    fn rand(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f64 / 4_294_967_296.0
    }

    /// `(x, y, z)` bloğu kırıldı: bloğun dokusundan renk alan parçacıklar
    /// saçar. `atlas`, `atlas::build_atlas` çıktısıdır.
    pub fn spawn_break(&mut self, x: i32, y: i32, z: i32, id: u8, atlas: &[u8]) {
        let tile = block_def(id).side as usize;
        for _ in 0..PARTICLES_PER_BREAK {
            if self.list.len() >= MAX_PARTICLES {
                self.list.remove(0);
            }
            // Dokudan rastgele bir piksel; saydam pikseller (cam) atlanır.
            let mut color = [200, 200, 200];
            for _ in 0..6 {
                let px = (self.rand() * TILE as f64) as usize;
                let py = (self.rand() * TILE as f64) as usize;
                let o = (py * ATLAS_W + tile * TILE as usize + px) * 4;
                if atlas.get(o + 3).copied().unwrap_or(0) > 128 {
                    color = [atlas[o], atlas[o + 1], atlas[o + 2]];
                    break;
                }
            }
            let pos = [
                x as f64 + 0.15 + self.rand() * 0.7,
                y as f64 + 0.15 + self.rand() * 0.7,
                z as f64 + 0.15 + self.rand() * 0.7,
            ];
            let vel = [
                (self.rand() - 0.5) * 4.0,
                1.5 + self.rand() * 3.5,
                (self.rand() - 0.5) * 4.0,
            ];
            let life = 0.45 + self.rand() * 0.45;
            let size = 0.07 + self.rand() as f32 * 0.07;
            self.list.push(Particle { pos, vel, life, size, color, gravity: 1.0 });
        }
    }

    /// Tek bir serbest parçacık (duman, kıvılcım, su damlası) ekler; hıza
    /// küçük bir rastgele sapma katılır.
    pub fn puff(&mut self, pos: [f64; 3], vel: [f64; 3], spread: f64, color: [u8; 3], life: f64, size: f32, gravity: f32) {
        if self.list.len() >= MAX_PARTICLES {
            self.list.remove(0);
        }
        let vel = [
            vel[0] + (self.rand() - 0.5) * spread,
            vel[1] + (self.rand() - 0.5) * spread,
            vel[2] + (self.rand() - 0.5) * spread,
        ];
        let life = life * (0.7 + self.rand() * 0.6);
        self.list.push(Particle { pos, vel, life, size, color, gravity });
    }

    /// `rate` (1/sn) sıklığında olan bir olay bu `dt` adımında gerçekleşti mi?
    pub fn chance(&mut self, rate: f64, dt: f64) -> bool {
        self.rand() < rate * dt
    }

    /// Yerçekimi ve basit çarpışma: katı bloğa giren parçacık o eksende durur.
    pub fn update(&mut self, dt: f64, world: &World) {
        let solid = |p: &[f64; 3]| {
            is_solid(world.get_block(p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32))
        };
        for p in self.list.iter_mut() {
            p.life -= dt;
            p.vel[1] -= PARTICLE_GRAVITY * p.gravity as f64 * dt;
            for a in 0..3 {
                let old = p.pos[a];
                p.pos[a] += p.vel[a] * dt;
                if solid(&p.pos) {
                    p.pos[a] = old;
                    p.vel[a] = 0.0;
                    if a == 1 {
                        // Yere değince yatay hız sürtünmeyle söner.
                        p.vel[0] *= 0.5;
                        p.vel[2] *= 0.5;
                    }
                }
            }
        }
        self.list.retain(|p| p.life > 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbreakable_blocks_have_no_break_time() {
        assert!(break_time(AIR).is_none());
        assert!(break_time(WATER).is_none());
        assert!(break_time(BEDROCK).is_none());
        for id in GRASS..=CRAFT_TABLE {
            if id != WATER && id != BEDROCK {
                assert!(break_time(id).unwrap() > 0.0, "blok {id}");
            }
        }
        assert!(break_time(LEAVES) < break_time(DIRT));
        assert!(break_time(DIRT) < break_time(STONE));
    }

    #[test]
    fn mining_takes_the_blocks_break_time() {
        let mut m = Mining::default();
        let t = break_time(STONE).unwrap();
        let mut elapsed = 0.0;
        while !m.advance([1, 2, 3], STONE, 0.05) {
            elapsed += 0.05;
            assert!(elapsed < t + 0.1, "taş zamanında kırılmadı");
            assert!(m.crack().is_some());
        }
        assert!(elapsed >= t - 0.11, "taş fazla çabuk kırıldı: {elapsed}");
    }

    #[test]
    fn switching_target_restarts_progress() {
        let mut m = Mining::default();
        m.advance([0, 5, 0], STONE, 0.6);
        let before = m.progress;
        assert!(before > 0.3);
        m.advance([1, 5, 0], STONE, 0.1);
        assert!(m.progress < before, "yeni hedefte ilerleme sıfırlanmalı");
        assert_eq!(m.target, Some([1, 5, 0]));
    }

    #[test]
    fn unbreakable_target_never_progresses() {
        let mut m = Mining::default();
        for _ in 0..100 {
            assert!(!m.advance([0, 0, 0], BEDROCK, 0.1));
        }
        assert!(m.crack().is_none());
    }

    #[test]
    fn particles_take_colour_from_the_block_and_expire() {
        let atlas = crate::atlas::build_atlas();
        let w = World::new(1);
        let mut p = Particles::default();
        p.spawn_break(0, 50, 0, GRASS, &atlas);
        assert_eq!(p.list.len(), PARTICLES_PER_BREAK);
        for q in &p.list {
            assert!((0.0..1.0).contains(&q.pos[0]) && (50.0..51.0).contains(&q.pos[1]));
        }
        // Cam çoğunlukla saydamdır; yine de görünür bir renk seçilmeli.
        p.spawn_break(0, 50, 0, GLASS, &atlas);
        assert!(p.list.iter().all(|q| q.color != [0, 0, 0]));
        for _ in 0..40 {
            p.update(0.05, &w);
        }
        assert!(p.list.is_empty(), "parçacıklar ömrü dolunca silinmeli");
    }

    #[test]
    fn sparks_rise_and_smoke_drifts() {
        let w = World::new(1);
        let mut p = Particles::default();
        p.puff([0.5, 50.0, 0.5], [0.0, 0.2, 0.0], 0.0, [255, 200, 80], 1.0, 0.04, -0.05);
        p.puff([0.5, 50.0, 0.5], [0.0, 0.0, 0.0], 0.0, [90, 90, 90], 1.0, 0.1, 1.0);
        for _ in 0..10 {
            p.update(0.05, &w);
        }
        assert!(p.list[0].pos[1] > 50.1, "kıvılcım yükselmeli");
        assert!(p.list[1].pos[1] < 49.0, "ağır parçacık düşmeli");
    }

    #[test]
    fn particles_stop_on_solid_ground_and_are_capped() {
        let atlas = crate::atlas::build_atlas();
        let mut w = World::new(20261012);
        w.generate_chunk(0, 0);
        let h = w.height_at(8, 8);
        let mut p = Particles::default();
        p.spawn_break(8, h + 2, 8, STONE, &atlas);
        for _ in 0..8 {
            p.update(0.05, &w);
        }
        for q in &p.list {
            assert!(q.pos[1] >= (h + 1) as f64 - 1e-9, "parçacık zemine girdi: {:?}", q.pos);
        }
        for _ in 0..60 {
            p.spawn_break(8, h + 2, 8, STONE, &atlas);
        }
        assert!(p.list.len() <= MAX_PARTICLES);
    }
}
