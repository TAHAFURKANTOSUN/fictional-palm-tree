//! Olta ile balık tutma. Saf kurallar; rastgelelik dışarıdan verilir.

use crate::blocks::{is_solid, WATER};
use crate::world::World;

/// Oltanın atılabileceği en uzak mesafe (blok).
pub const CAST_RANGE: f64 = 14.0;
/// Balık vurduktan sonra çekmek için tanınan süre (saniye).
pub const BITE_WINDOW: f64 = 1.4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Balık bekleniyor; kalan süre.
    Waiting(f64),
    /// Balık vurdu; çekmek için kalan süre.
    Biting(f64),
}

pub struct Fishing {
    /// Şamandıranın su yüzeyindeki konumu.
    pub bobber: [f64; 3],
    pub phase: Phase,
    pub age: f64,
}

impl Fishing {
    pub fn new(bobber: [f64; 3], wait: f64) -> Self {
        Self { bobber, phase: Phase::Waiting(wait), age: 0.0 }
    }

    /// Zamanı ilerletir. Balık **bu adımda** vurduysa `true` döner. Kaçan
    /// balıktan sonra `next_wait` saniye yeniden beklenir.
    pub fn tick(&mut self, dt: f64, next_wait: f64) -> bool {
        self.age += dt;
        match &mut self.phase {
            Phase::Waiting(t) => {
                *t -= dt;
                if *t <= 0.0 {
                    self.phase = Phase::Biting(BITE_WINDOW);
                    return true;
                }
            }
            Phase::Biting(t) => {
                *t -= dt;
                if *t <= 0.0 {
                    self.phase = Phase::Waiting(next_wait);
                }
            }
        }
        false
    }

    /// Olta çekildi: balık vurmuşken çekildiyse yakalanır.
    pub fn reel(&self) -> bool {
        matches!(self.phase, Phase::Biting(_))
    }

    /// Şamandıranın dikey salınımı; balık vurunca suya gömülür.
    pub fn bob_offset(&self) -> f64 {
        match self.phase {
            Phase::Waiting(_) => (self.age * 2.2).sin() * 0.04,
            Phase::Biting(_) => -0.22 + (self.age * 18.0).sin() * 0.05,
        }
    }
}

/// Bakış doğrultusunda ilk su yüzeyini bulur (şamandıranın konacağı nokta).
/// Önce katı bir bloğa çarpılırsa ya da menzilde su yoksa `None`.
pub fn find_water(world: &World, eye: [f64; 3], dir: [f64; 3], range: f64) -> Option<[f64; 3]> {
    let step = 0.2;
    let mut t = step;
    while t <= range {
        let p = [eye[0] + dir[0] * t, eye[1] + dir[1] * t, eye[2] + dir[2] * t];
        let (bx, mut by, bz) = (p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
        let b = world.get_block(bx, by, bz);
        if b == WATER {
            // Su altına bakılıyorsa şamandıra yine yüzeye çıkar.
            while world.get_block(bx, by + 1, bz) == WATER {
                by += 1;
            }
            return Some([p[0], by as f64 + 0.9, p[2]]);
        }
        if is_solid(b) {
            return None;
        }
        t += step;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{AIR, STONE};

    #[test]
    fn fish_bites_after_the_wait_and_can_be_caught() {
        let mut f = Fishing::new([0.0; 3], 3.0);
        let mut bites = 0;
        for _ in 0..29 {
            bites += f.tick(0.1, 5.0) as u32;
        }
        assert_eq!(bites, 0, "erken vurdu");
        assert!(!f.reel(), "beklerken çekince balık gelmemeli");
        bites += f.tick(0.2, 5.0) as u32;
        assert_eq!(bites, 1);
        assert!(f.reel(), "vurmuşken çekince yakalanmalı");
        assert!(f.bob_offset() < -0.1, "şamandıra suya gömülmeli");
    }

    #[test]
    fn a_missed_bite_goes_back_to_waiting() {
        let mut f = Fishing::new([0.0; 3], 0.1);
        assert!(f.tick(0.2, 4.0));
        for _ in 0..20 {
            f.tick(0.1, 4.0);
        }
        assert!(!f.reel(), "süre dolunca balık kaçmalı");
        assert!(matches!(f.phase, Phase::Waiting(t) if t > 3.0));
    }

    fn pond() -> World {
        let mut w = World::new(20261012);
        for dz in -1..=1 {
            for dx in -1..=1 {
                w.generate_chunk(dx, dz);
            }
        }
        for x in -8..8 {
            for z in -8..8 {
                for y in 0..=20 {
                    w.set_block_raw(x, y, z, STONE);
                }
                for y in 21..40 {
                    w.set_block_raw(x, y, z, AIR);
                }
            }
        }
        // 3..=6 arası iki blok derinliğinde havuz.
        for x in 3..=6 {
            for z in -2..=2 {
                w.set_block_raw(x, 20, z, WATER);
                w.set_block_raw(x, 19, z, WATER);
            }
        }
        w
    }

    #[test]
    fn casting_finds_the_water_surface() {
        let w = pond();
        let eye = [0.5, 22.6, 0.5];
        let d = [0.86, -0.5, 0.0];
        let p = find_water(&w, eye, d, CAST_RANGE).expect("su bulunamadı");
        assert!((3.0..7.0).contains(&p[0]));
        assert!((p[1] - 20.9).abs() < 1e-9, "şamandıra su yüzeyinde olmalı: {}", p[1]);
    }

    #[test]
    fn casting_at_land_or_sky_finds_nothing() {
        let w = pond();
        let eye = [0.5, 22.6, 0.5];
        assert!(find_water(&w, eye, [-0.86, -0.5, 0.0], CAST_RANGE).is_none(), "karaya atılmamalı");
        assert!(find_water(&w, eye, [0.0, 1.0, 0.0], CAST_RANGE).is_none());
        assert!(find_water(&w, eye, [0.86, -0.5, 0.0], 2.0).is_none(), "menzil dışı");
    }
}
