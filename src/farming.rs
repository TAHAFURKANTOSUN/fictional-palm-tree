//! Bitkiler ve tarım: toprağı çapalama, tohum ekme, suya yakınlık ile büyüme,
//! kemik ile gübreleme, hasat ganimetleri ve 3B bitki modelleri.
//!
//! Büyüyen ekinlerin konumları [`crate::protocol::Position`] (64 bitlik
//! paketlenmiş blok koordinatı) ile saklanır ve tarayıcıya kaydedilir.

use crate::blocks::{
    is_crop, AIR, DANDELION, DIRT, FARMLAND, GRASS, POPPY, SEEDS, SUGAR_CANE, TALL_GRASS, WATER,
    WHEAT, WHEAT_0, WHEAT_1, WHEAT_2, WHEAT_3,
};
use crate::effects::BoxInst;
use crate::protocol::{Error, Position, Reader, Result as ProtoResult, Writer};
use crate::world::{ckey, World};

/// Kuru toprakta bir büyüme evresinin süresi (saniye).
/// Suya yakın (`is_hydrated`) sürülmüş toprakta **iki kat hızlı** büyür.
pub const STAGE_TIME: f64 = 18.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CropPlot {
    pub pos: Position,
    pub timer: f64,
}

#[derive(Default)]
pub struct Farming {
    pub plots: Vec<CropPlot>,
}

impl Farming {
    /// Sürülmüş toprağın `(x, y, z)` 4 blok çevresinde (`dx, dz ∈ -4..=4`,
    /// `dy ∈ -1..=0`) su var mı? (Minecraft'ın tarla sulama kuralı.)
    pub fn is_hydrated(world: &World, x: i32, y: i32, z: i32) -> bool {
        for dy in -1..=0 {
            for dz in -4..=4 {
                for dx in -4..=4 {
                    if world.get_block(x + dx, y + dy, z + dz) == WATER {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Çapa ile `(x, y, z)` bloğunu sürer: çim ya da toprak ise (ve üstü boş
    /// ya da ot ise) [`FARMLAND`]'e dönüştürür.
    pub fn till(world: &mut World, x: i32, y: i32, z: i32) -> bool {
        let ground = world.get_block(x, y, z);
        if ground != GRASS && ground != DIRT {
            return false;
        }
        let above = world.get_block(x, y + 1, z);
        if above != AIR && above != TALL_GRASS {
            return false;
        }
        if above == TALL_GRASS {
            world.set_block_raw(x, y + 1, z, AIR);
        }
        world.set_block_raw(x, y, z, FARMLAND)
    }

    /// `(x, y, z)` sürülmüş toprağının üstüne (`y + 1`) buğday tohumu eker.
    pub fn plant_seeds(&mut self, world: &mut World, x: i32, y: i32, z: i32) -> bool {
        if world.get_block(x, y, z) != FARMLAND || world.get_block(x, y + 1, z) != AIR {
            return false;
        }
        if !world.set_block_raw(x, y + 1, z, WHEAT_0) {
            return false;
        }
        let pos = Position { x, y: y + 1, z };
        if let Some(p) = self.plots.iter_mut().find(|p| p.pos == pos) {
            p.timer = 0.0;
        } else {
            self.plots.push(CropPlot { pos, timer: 0.0 });
        }
        true
    }

    /// Kemik (kemik tozu etkisi) ile ekini bir evre büyütür. `(x, y, z)` hem
    /// ekinin kendisi hem de altındaki [`FARMLAND`] olabilir. Büyüdüyse
    /// ekinin konumunu `Some([x, y, z])` olarak döndürür.
    pub fn fertilize(&mut self, world: &mut World, x: i32, y: i32, z: i32) -> Option<[i32; 3]> {
        let b = world.get_block(x, y, z);
        let (cx, cy, cz, id) = if matches!(b, WHEAT_0 | WHEAT_1 | WHEAT_2) {
            (x, y, z, b)
        } else if b == FARMLAND {
            let up = world.get_block(x, y + 1, z);
            if matches!(up, WHEAT_0 | WHEAT_1 | WHEAT_2) {
                (x, y + 1, z, up)
            } else {
                return None;
            }
        } else {
            return None;
        };

        let next = id + 1;
        if !world.set_block_raw(cx, cy, cz, next) {
            return None;
        }
        let pos = Position { x: cx, y: cy, z: cz };
        if next >= WHEAT_3 {
            self.plots.retain(|p| p.pos != pos);
        } else if let Some(p) = self.plots.iter_mut().find(|p| p.pos == pos) {
            p.timer = 0.0;
        } else {
            self.plots.push(CropPlot { pos, timer: 0.0 });
        }
        Some([cx, cy, cz])
    }

    /// Tüm ekili ekinleri ilerletir; evresi değişen ya da altı kırıldığı için
    /// düşen ekinlerin koordinatlarını `changed` listesine yazar.
    pub fn tick(&mut self, world: &mut World, dt: f64, changed: &mut Vec<[i32; 3]>) {
        self.plots.retain_mut(|plot| {
            let Position { x, y, z } = plot.pos;
            if !world.chunks.contains_key(&ckey(x >> 4, z >> 4)) {
                return true;
            }
            let id = world.get_block(x, y, z);
            let soil = world.get_block(x, y - 1, z);
            if soil != FARMLAND {
                if is_crop(id) {
                    world.set_block_raw(x, y, z, AIR);
                    changed.push([x, y, z]);
                }
                return false;
            }
            if !matches!(id, WHEAT_0 | WHEAT_1 | WHEAT_2) {
                return false;
            }
            let rate = if Self::is_hydrated(world, x, y - 1, z) { 2.0 } else { 1.0 };
            plot.timer += dt * rate;
            let mut cur = id;
            while plot.timer >= STAGE_TIME && cur < WHEAT_3 {
                plot.timer -= STAGE_TIME;
                cur += 1;
            }
            if cur != id {
                world.set_block_raw(x, y, z, cur);
                changed.push([x, y, z]);
            }
            cur < WHEAT_3
        });
    }

    /// Kırılan bitki ya da ekin bloğunun bıraktığı eşyalar.
    pub fn harvest_drops(block_id: u8, rand: f64) -> Vec<(u16, u32)> {
        match block_id {
            WHEAT_3 => vec![(WHEAT, 1), (SEEDS, 2)],
            WHEAT_0 | WHEAT_1 | WHEAT_2 => vec![(SEEDS, 1)],
            TALL_GRASS => {
                if rand < 0.65 {
                    vec![(SEEDS, 1)]
                } else {
                    Vec::new()
                }
            }
            POPPY => vec![(POPPY as u16, 1)],
            DANDELION => vec![(DANDELION as u16, 1)],
            SUGAR_CANE => vec![(SUGAR_CANE as u16, 1)],
            _ => Vec::new(),
        }
    }

    /// `(x, y, z)` hücresindeki bitki ya da ekinin 3B kutu parçalarını üretir.
    pub fn push_plant_boxes(x: i32, y: i32, z: i32, id: u8, time: f32, boxes: &mut Vec<BoxInst>) {
        let (bx, by, bz) = (x as f64 + 0.5, y as f64, z as f64 + 0.5);
        let phase = time * 2.2 + (x.wrapping_mul(7).wrapping_add(z.wrapping_mul(13))) as f32 * 0.37;
        let sway = phase.sin() * 0.05;

        match id {
            TALL_GRASS => {
                let blades: [(f64, f64, f32, f32, [u8; 3]); 5] = [
                    (-0.18, -0.14, 0.24, 0.2, [74, 168, 56]),
                    (0.16, -0.16, 0.30, -0.4, [62, 150, 48]),
                    (-0.14, 0.18, 0.28, 0.6, [84, 178, 62]),
                    (0.18, 0.15, 0.22, -0.7, [66, 156, 50]),
                    (0.0, 0.0, 0.34, 0.1, [78, 172, 58]),
                ];
                for (dx, dz, hh, yaw, color) in blades {
                    boxes.push(BoxInst {
                        center: [bx + dx, by + hh as f64, bz + dz],
                        half: [0.035, hh, 0.035],
                        yaw,
                        pitch: sway,
                        color,
                        glow: false,
                    });
                }
            }
            POPPY => {
                boxes.push(BoxInst {
                    center: [bx, by + 0.22, bz],
                    half: [0.035, 0.22, 0.035],
                    yaw: 0.2,
                    pitch: sway,
                    color: [56, 142, 46],
                    glow: false,
                });
                boxes.push(BoxInst {
                    center: [bx, by + 0.48, bz],
                    half: [0.13, 0.08, 0.13],
                    yaw: 0.4,
                    pitch: sway,
                    color: [218, 44, 38],
                    glow: false,
                });
                boxes.push(BoxInst {
                    center: [bx, by + 0.56, bz],
                    half: [0.05, 0.025, 0.05],
                    yaw: 0.4,
                    pitch: sway,
                    color: [38, 24, 24],
                    glow: false,
                });
            }
            DANDELION => {
                boxes.push(BoxInst {
                    center: [bx, by + 0.20, bz],
                    half: [0.035, 0.20, 0.035],
                    yaw: -0.2,
                    pitch: sway,
                    color: [56, 142, 46],
                    glow: false,
                });
                boxes.push(BoxInst {
                    center: [bx, by + 0.44, bz],
                    half: [0.12, 0.07, 0.12],
                    yaw: 0.5,
                    pitch: sway,
                    color: [242, 208, 46],
                    glow: false,
                });
                boxes.push(BoxInst {
                    center: [bx, by + 0.51, bz],
                    half: [0.07, 0.025, 0.07],
                    yaw: 0.5,
                    pitch: sway,
                    color: [255, 240, 115],
                    glow: false,
                });
            }
            SUGAR_CANE => {
                for (dx, dz, hh) in [(-0.18, -0.15, 0.46f32), (0.17, -0.12, 0.42), (-0.02, 0.18, 0.48)] {
                    boxes.push(BoxInst {
                        center: [bx + dx, by + hh as f64, bz + dz],
                        half: [0.055, hh, 0.055],
                        yaw: 0.15,
                        pitch: sway * 0.5,
                        color: [116, 198, 72],
                        glow: false,
                    });
                    boxes.push(BoxInst {
                        center: [bx + dx, by + hh as f64, bz + dz],
                        half: [0.065, 0.025, 0.065],
                        yaw: 0.15,
                        pitch: 0.0,
                        color: [82, 152, 48],
                        glow: false,
                    });
                }
            }
            WHEAT_0 | WHEAT_1 | WHEAT_2 | WHEAT_3 => {
                let stage = (id - WHEAT_0) as usize;
                let hh = [0.11f32, 0.22, 0.34, 0.44][stage];
                let stalk_col = [
                    [88, 184, 58],
                    [114, 182, 56],
                    [168, 184, 58],
                    [218, 182, 62],
                ][stage];
                for (dx, dz) in [(-0.22, -0.22), (0.22, -0.22), (-0.22, 0.22), (0.22, 0.22)] {
                    boxes.push(BoxInst {
                        center: [bx + dx, by + hh as f64, bz + dz],
                        half: [0.04, hh, 0.04],
                        yaw: 0.0,
                        pitch: sway,
                        color: stalk_col,
                        glow: false,
                    });
                    if stage >= 2 {
                        let head_col = if stage == 3 { [240, 198, 68] } else { [192, 190, 68] };
                        boxes.push(BoxInst {
                            center: [bx + dx, by + (hh * 2.0 + 0.06) as f64, bz + dz],
                            half: [0.065, 0.08, 0.065],
                            yaw: 0.25,
                            pitch: sway,
                            color: head_col,
                            glow: false,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    /// Ekili tarlaları `protocol::Writer` (`Position` + kalan süre) ile kodlar.
    pub fn encode(&self, w: &mut Writer) {
        let valid: Vec<&CropPlot> = self.plots.iter().filter(|p| p.pos.in_range()).collect();
        w.var_int(valid.len() as i32);
        for p in valid {
            w.position(p.pos);
            w.f32(p.timer as f32);
        }
    }

    /// `protocol::Reader` üzerinden ekili tarlaları çözer.
    pub fn decode(&mut self, r: &mut Reader) -> ProtoResult<()> {
        let len = r.var_int()?;
        if !(0..=4096).contains(&len) {
            return Err(Error::InvalidValue);
        }
        let mut plots = Vec::with_capacity(len as usize);
        for _ in 0..len {
            let pos = r.position()?;
            let timer = r.f32()? as f64;
            if !timer.is_finite() || timer < 0.0 {
                return Err(Error::InvalidValue);
            }
            plots.push(CropPlot { pos, timer });
        }
        self.plots = plots;
        Ok(())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode(&mut w);
        w.buf
    }

    pub fn load_bytes(&mut self, bytes: &[u8]) -> ProtoResult<()> {
        let mut r = Reader::new(bytes);
        self.decode(&mut r)?;
        r.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::STONE;

    fn test_world() -> World {
        let mut w = World::new(20261012);
        for dz in -1..=1 {
            for dx in -1..=1 {
                w.generate_chunk(dx, dz);
            }
        }
        for x in -8..=8 {
            for z in -8..=8 {
                for y in 0..20 {
                    w.set_block_raw(x, y, z, STONE);
                }
                w.set_block_raw(x, 20, z, GRASS);
                for y in 21..35 {
                    w.set_block_raw(x, y, z, AIR);
                }
            }
        }
        w
    }

    #[test]
    fn hoe_tills_grass_and_dirt_into_farmland_and_clears_tall_grass() {
        let mut w = test_world();
        w.set_block_raw(2, 21, 2, TALL_GRASS);
        assert!(Farming::till(&mut w, 2, 20, 2));
        assert_eq!(w.get_block(2, 20, 2), FARMLAND);
        assert_eq!(w.get_block(2, 21, 2), AIR, "üstteki yabani ot temizlenmeli");

        // Taş çapalanamaz; üstü dolu blok da çapalanamaz.
        w.set_block_raw(3, 20, 3, STONE);
        assert!(!Farming::till(&mut w, 3, 20, 3));
        w.set_block_raw(4, 21, 4, STONE);
        assert!(!Farming::till(&mut w, 4, 20, 4));
    }

    #[test]
    fn hydrated_farmland_grows_wheat_twice_as_fast_as_dry_farmland() {
        let mut w = test_world();
        let mut farm = Farming::default();

        // Kuru tarla: (-6, 20, -6)
        assert!(Farming::till(&mut w, -6, 20, -6));
        assert!(farm.plant_seeds(&mut w, -6, 20, -6));
        assert!(!Farming::is_hydrated(&w, -6, 20, -6));

        // Sulu tarla: (4, 20, 4), 3 blok ötesinde (7, 20, 4) su var.
        w.set_block_raw(7, 20, 4, WATER);
        assert!(Farming::till(&mut w, 4, 20, 4));
        assert!(farm.plant_seeds(&mut w, 4, 20, 4));
        assert!(Farming::is_hydrated(&w, 4, 20, 4));

        let mut changed = Vec::new();
        // 28 saniye sonra: sulu tarla 3 evre ilerleyip olgunlaşmalı (3 × 9 sn = 27 sn),
        // kuru tarla ise henüz 1. evrede (WHEAT_1, 18..36 sn) olmalı.
        for _ in 0..28 {
            farm.tick(&mut w, 1.0, &mut changed);
        }
        assert_eq!(w.get_block(4, 21, 4), WHEAT_3, "suya yakın buğday 2x hızlı olgunlaşmalı");
        assert_eq!(w.get_block(-6, 21, -6), WHEAT_1, "kuru topraktaki buğday daha yavaş büyümeli");

        // 30 saniye daha geçince kuru tarladaki buğday da olgunlaşır.
        for _ in 0..30 {
            farm.tick(&mut w, 1.0, &mut changed);
        }
        assert_eq!(w.get_block(-6, 21, -6), WHEAT_3);
        assert!(farm.plots.is_empty(), "olgunlaşan ekinler aktif zamanlayıcıdan çıkmalı");
    }

    #[test]
    fn bone_fertilizes_crop_and_harvest_drops_match_maturity() {
        let mut w = test_world();
        let mut farm = Farming::default();
        assert!(Farming::till(&mut w, 0, 20, 0));
        assert!(farm.plant_seeds(&mut w, 0, 20, 0));
        assert_eq!(w.get_block(0, 21, 0), WHEAT_0);

        // Erken kırılırsa yalnızca tohum verir.
        assert_eq!(Farming::harvest_drops(WHEAT_0, 0.1), vec![(SEEDS, 1)]);

        // Kemik ile 3 kez gübreleyince anında olgun buğdaya (WHEAT_3) dönüşür.
        assert_eq!(farm.fertilize(&mut w, 0, 21, 0), Some([0, 21, 0]));
        assert_eq!(w.get_block(0, 21, 0), WHEAT_1);
        assert_eq!(farm.fertilize(&mut w, 0, 20, 0), Some([0, 21, 0]), "altındaki toprağa tıklamak da gübrelemeli");
        assert_eq!(w.get_block(0, 21, 0), WHEAT_2);
        assert_eq!(farm.fertilize(&mut w, 0, 21, 0), Some([0, 21, 0]));
        assert_eq!(w.get_block(0, 21, 0), WHEAT_3);
        assert_eq!(farm.fertilize(&mut w, 0, 21, 0), None, "olgun buğday daha fazla gübrelenemez");

        assert_eq!(Farming::harvest_drops(WHEAT_3, 0.5), vec![(WHEAT, 1), (SEEDS, 2)]);
        assert_eq!(Farming::harvest_drops(TALL_GRASS, 0.2), vec![(SEEDS, 1)]);
        assert!(Farming::harvest_drops(TALL_GRASS, 0.9).is_empty());
    }

    #[test]
    fn farming_plots_round_trip_through_protocol_position_encoding() {
        let mut farm = Farming::default();
        farm.plots.push(CropPlot { pos: Position { x: -12, y: 24, z: 38 }, timer: 7.5 });
        farm.plots.push(CropPlot { pos: Position { x: 105, y: 31, z: -200 }, timer: 14.25 });

        let bytes = farm.to_bytes();
        let mut restored = Farming::default();
        restored.load_bytes(&bytes).expect("tarla kaydı çözülebilmeli");
        assert_eq!(restored.plots.len(), 2);
        assert_eq!(restored.plots[0].pos, Position { x: -12, y: 24, z: 38 });
        assert!((restored.plots[0].timer - 7.5).abs() < 1e-4);
        assert_eq!(restored.plots[1].pos, Position { x: 105, y: 31, z: -200 });
    }
}
