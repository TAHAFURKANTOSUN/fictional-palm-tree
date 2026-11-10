//! Kare hızını koruyan uyarlanabilir çözünürlük.
//!
//! Kare hızı GPU yüzünden düşüyorsa (dolgu hızı), 3B tuvalin çözünürlüğünü
//! düşürmek en doğrudan çaredir. Ama düşük FPS'in her nedeni bu değildir:
//! ekran 30 Hz olabilir, tarayıcı pil tasarrufu için kareleri kısabilir. Bu
//! yüzden her düşürme bir **denemedir**: FPS belirgin biçimde artmazsa geri
//! alınır ve bir daha denenmez.

/// Çözünürlük kademeleri (tuval kenarı çarpanı).
pub const SCALES: [f32; 4] = [1.0, 0.85, 0.7, 0.55];

/// Bu FPS'in altı "düşüş" sayılır.
const LOW_FPS: f64 = 50.0;
/// Bu FPS'in üstü, kaliteyi geri yükseltmeyi denemek için yeterlidir.
const HIGH_FPS: f64 = 58.0;
/// Düşürmenin işe yaradığını kabul etmek için gereken en az kazanç.
const MIN_GAIN: f64 = 1.12;

#[derive(Default)]
pub struct QualityGovernor {
    level: usize,
    low_streak: u32,
    high_streak: u32,
    /// Deneme sürüyorsa: düşürmeden önceki FPS.
    trial_from: Option<f64>,
    /// Değişiklikten sonraki ilk örnek geçiş karelerini içerir; atlanır.
    settle: bool,
    /// Daha aşağı inmek fayda etmedi: bu kademeden aşağısı denenmez.
    floor: Option<usize>,
    /// Geri yükseltmeden önce beklenen iyi örnek sayısı; her başarısız
    /// yükseltmede iki katına çıkar (gidip gelme olmasın).
    raise_after: u32,
}

impl QualityGovernor {
    pub fn scale(&self) -> f32 {
        SCALES[self.level]
    }

    /// Yaklaşık saniyede bir, o aralığın ortalama FPS'i ile çağrılır.
    /// Çözünürlük kademesi değiştiyse yeni çarpanı döndürür.
    pub fn sample(&mut self, fps: f64) -> Option<f32> {
        if self.raise_after == 0 {
            self.raise_after = 30;
        }
        if self.settle {
            self.settle = false;
            return None;
        }

        if let Some(before) = self.trial_from.take() {
            if fps < before * MIN_GAIN {
                // İşe yaramadı: darboğaz çözünürlük değil. Geri al, kilitle.
                self.level -= 1;
                self.floor = Some(self.level);
                self.low_streak = 0;
                self.settle = true;
                return Some(self.scale());
            }
        }

        if fps < LOW_FPS {
            self.high_streak = 0;
            self.low_streak += 1;
            let can_lower = self.level + 1 < SCALES.len() && self.floor != Some(self.level);
            if self.low_streak >= 2 && can_lower {
                self.low_streak = 0;
                self.trial_from = Some(fps);
                self.level += 1;
                self.settle = true;
                return Some(self.scale());
            }
        } else {
            self.low_streak = 0;
            if fps >= HIGH_FPS && self.level > 0 {
                self.high_streak += 1;
                if self.high_streak >= self.raise_after {
                    self.high_streak = 0;
                    self.raise_after = (self.raise_after * 2).min(480);
                    self.level -= 1;
                    self.settle = true;
                    return Some(self.scale());
                }
            } else {
                self.high_streak = 0;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FPS'i piksel sayısıyla ters orantılı olan, dolgu hızına bağlı bir GPU.
    fn gpu_bound(base: f64, g: &QualityGovernor) -> f64 {
        (base / (g.scale() as f64).powi(2)).min(60.0)
    }

    #[test]
    fn steady_frame_rate_never_changes_quality() {
        let mut g = QualityGovernor::default();
        for _ in 0..200 {
            assert_eq!(g.sample(60.0), None);
        }
        assert_eq!(g.scale(), 1.0);
    }

    #[test]
    fn a_single_dip_is_ignored() {
        let mut g = QualityGovernor::default();
        assert_eq!(g.sample(30.0), None);
        for _ in 0..20 {
            assert_eq!(g.sample(60.0), None);
        }
    }

    #[test]
    fn gpu_bound_drop_lowers_resolution_until_smooth() {
        let mut g = QualityGovernor::default();
        for _ in 0..20 {
            let fps = gpu_bound(34.0, &g);
            g.sample(fps);
        }
        assert!(g.scale() < 1.0, "çözünürlük düşmeliydi");
        assert!(gpu_bound(34.0, &g) >= LOW_FPS, "akıcı kareye ulaşılmalı: {}", gpu_bound(34.0, &g));
    }

    #[test]
    fn capped_display_reverts_and_stops_trying() {
        // 30 Hz ekran: çözünürlük ne olursa olsun 30 FPS.
        let mut g = QualityGovernor::default();
        let mut changes = 0;
        for _ in 0..300 {
            if g.sample(30.0).is_some() {
                changes += 1;
            }
        }
        assert_eq!(g.scale(), 1.0, "faydasız düşürme geri alınmalı");
        assert_eq!(changes, 2, "bir deneme + geri alma, sonrası yok");
    }

    #[test]
    fn quality_returns_when_load_disappears_without_flapping() {
        let mut g = QualityGovernor::default();
        for _ in 0..20 {
            let fps = gpu_bound(34.0, &g);
            g.sample(fps);
        }
        assert!(g.scale() < 1.0);
        // Yük kalktı: yeterince uzun süre akıcıysa tam çözünürlüğe döner.
        for _ in 0..400 {
            g.sample(60.0);
        }
        assert_eq!(g.scale(), 1.0);

        // Yük sürerken ise sürekli gidip gelmez: yükseltme denemeleri seyrekleşir.
        let mut g = QualityGovernor::default();
        let mut changes = 0;
        for _ in 0..600 {
            let fps = gpu_bound(34.0, &g);
            if g.sample(fps).is_some() {
                changes += 1;
            }
        }
        assert!(changes <= 14, "çok sık kademe değişimi: {changes}");
    }
}
