//! Silahlar: dürbünsüz tabanca ve dürbünlü tüfek. Saf kurallar (atış aralığı,
//! geri tepme, yakınlaştırma); çizimi ve ışın izlemeyi `app` yapar.

use crate::blocks::{GLASS, LEAVES, PISTOL, SNIPER, TORCH};

pub struct Weapon {
    /// İki atış arasındaki en kısa süre (saniye).
    pub interval: f64,
    /// Bir canlıya verdiği hasar.
    pub damage: i32,
    /// Merminin menzili (blok).
    pub range: f64,
    /// Atışta kameranın yukarı sıçraması (radyan).
    pub kick: f64,
    /// Dürbün: tam yakınlaştırmada görüş açısı çarpanı (1 = dürbün yok).
    pub zoom_fov: f32,
}

const PISTOL_STATS: Weapon = Weapon { interval: 0.22, damage: 5, range: 70.0, kick: 0.014, zoom_fov: 1.0 };
const SNIPER_STATS: Weapon = Weapon { interval: 1.1, damage: 20, range: 150.0, kick: 0.04, zoom_fov: 0.22 };

/// Eşya bir silahsa özelliklerini döndürür.
pub fn weapon(id: u16) -> Option<&'static Weapon> {
    match id {
        PISTOL => Some(&PISTOL_STATS),
        SNIPER => Some(&SNIPER_STATS),
        _ => None,
    }
}

/// Mermi bu bloğu kırar mı? (Yalnızca kırılgan bloklar; taş, toprak vb. sağlam kalır.)
pub fn shatters(block: u8) -> bool {
    matches!(block, GLASS | LEAVES | TORCH)
}

/// Eldeki silahın anlık durumu.
#[derive(Default)]
pub struct Gun {
    cooldown: f64,
    /// Geri tepme animasyonu, 1 → 0.
    pub recoil: f64,
    /// Namlu alevi, 1 → 0 (çok kısa).
    pub flash: f64,
    /// Yakınlaştırma, 0 (kapalı) … 1 (tam).
    pub zoom: f64,
}

impl Gun {
    /// Zamanı ilerletir. `aiming`: dürbünlü silah elde ve nişan tuşu basılı.
    pub fn tick(&mut self, dt: f64, aiming: bool) {
        self.cooldown = (self.cooldown - dt).max(0.0);
        self.recoil = (self.recoil - dt * 6.0).max(0.0);
        self.flash = (self.flash - dt * 16.0).max(0.0);
        let target = if aiming { 1.0 } else { 0.0 };
        self.zoom += (target - self.zoom) * (dt * 14.0).min(1.0);
        if (self.zoom - target).abs() < 0.002 {
            self.zoom = target;
        }
    }

    /// Tetik çekildi. Silah hazırsa ateş eder ve `true` döner.
    pub fn try_fire(&mut self, w: &Weapon) -> bool {
        if self.cooldown > 0.0 {
            return false;
        }
        self.cooldown = w.interval;
        self.recoil = 1.0;
        self.flash = 1.0;
        true
    }

    /// Dürbün görüntüsü (siyah çerçeve) gösterilecek kadar yakınlaşıldı mı?
    pub fn scoped(&self) -> bool {
        self.zoom > 0.6
    }

    /// Görüş açısı çarpanı: 1 (normal) … `zoom_fov` (tam yakın).
    pub fn fov_scale(&self, w: Option<&Weapon>) -> f32 {
        let z = w.map(|w| w.zoom_fov).unwrap_or(1.0);
        1.0 + (z - 1.0) * self.zoom as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{APPLE, STONE};

    #[test]
    fn only_guns_are_weapons_and_only_the_rifle_zooms() {
        assert!(weapon(APPLE).is_none());
        assert_eq!(weapon(PISTOL).unwrap().zoom_fov, 1.0, "tabancada dürbün yok");
        assert!(weapon(SNIPER).unwrap().zoom_fov < 0.5, "tüfek yakınlaştırmalı");
        assert!(weapon(SNIPER).unwrap().range > weapon(PISTOL).unwrap().range);
    }

    #[test]
    fn fire_rate_is_limited_by_the_interval() {
        let w = weapon(PISTOL).unwrap();
        let mut g = Gun::default();
        assert!(g.try_fire(w));
        assert!(!g.try_fire(w), "aralık dolmadan ikinci atış olmamalı");
        assert!(g.recoil == 1.0 && g.flash == 1.0);
        let mut shots = 1;
        for _ in 0..100 {
            g.tick(0.01, false);
            shots += g.try_fire(w) as u32;
        }
        // 1 saniyede 0,22 sn aralıkla: ilk atış + 4.
        assert_eq!(shots, 5);
    }

    #[test]
    fn recoil_and_flash_fade_out() {
        let mut g = Gun::default();
        g.try_fire(weapon(SNIPER).unwrap());
        g.tick(0.05, false);
        assert!(g.flash < 0.3 && g.recoil > 0.5, "alev geri tepmeden çok daha kısa sürmeli");
        for _ in 0..30 {
            g.tick(0.05, false);
        }
        assert_eq!((g.recoil, g.flash), (0.0, 0.0));
    }

    #[test]
    fn zoom_eases_in_and_out() {
        let sniper = weapon(SNIPER);
        let mut g = Gun::default();
        assert_eq!(g.fov_scale(sniper), 1.0);
        g.tick(0.016, true);
        assert!(g.zoom > 0.0 && g.zoom < 1.0 && !g.scoped(), "yakınlaşma anlık olmamalı");
        for _ in 0..60 {
            g.tick(0.016, true);
        }
        assert!(g.scoped() && g.zoom == 1.0);
        assert!((g.fov_scale(sniper) - 0.22).abs() < 1e-6);
        assert_eq!(g.fov_scale(weapon(PISTOL)), 1.0, "tabanca görüş açısını değiştirmez");
        for _ in 0..60 {
            g.tick(0.016, false);
        }
        assert_eq!(g.zoom, 0.0);
    }

    #[test]
    fn bullets_only_shatter_fragile_blocks() {
        assert!(shatters(GLASS) && shatters(LEAVES) && shatters(TORCH));
        assert!(!shatters(STONE));
    }
}
