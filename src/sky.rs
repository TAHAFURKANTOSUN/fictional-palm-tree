//! Gündüz–gece döngüsü: günün saatinden gökyüzü renkleri, güneş yönü ve
//! ortam ışığı. Saf hesaplamadır; çizimi `gl` modülü yapar.

/// Tam bir günün gerçek süresi (saniye).
pub const DAY_LENGTH: f64 = 720.0;
/// Günün gündüz geçen kısmı; gece daha kısadır ki oyun uzun süre karanlık kalmasın.
const DAY_FRACTION: f64 = 0.7;
/// Oyunun başladığı an (sabah).
pub const START_TIME: f64 = 0.10;
/// Gece yarısındaki ortam ışığı: karanlık ama oynanabilir.
const NIGHT_LIGHT: f32 = 0.28;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyState {
    /// Güneşe doğru birim vektör (gece ufkun altındadır; ay tam karşısındadır).
    pub sun_dir: [f32; 3],
    pub top: [f32; 3],
    pub horizon: [f32; 3],
    /// Dünyanın ışık çarpanı (`NIGHT_LIGHT`..1).
    pub light: f32,
    /// Yıldızların görünürlüğü (0 gündüz, 1 gece).
    pub stars: f32,
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Güneşin yörünge açısı: 0 gün doğumu, π/2 öğle, π gün batımı, 3π/2 gece yarısı.
fn sun_angle(time: f64) -> f64 {
    let t = time.rem_euclid(1.0);
    let pi = std::f64::consts::PI;
    if t < DAY_FRACTION {
        pi * t / DAY_FRACTION
    } else {
        pi + pi * (t - DAY_FRACTION) / (1.0 - DAY_FRACTION)
    }
}

/// Saat cinsinden günün zamanı (0..24); gün doğumu 06:00, gün batımı 18:00.
pub fn clock_hours(time: f64) -> f64 {
    (6.0 + sun_angle(time) / std::f64::consts::PI * 12.0).rem_euclid(24.0)
}

/// `time` ∈ [0,1) için gökyüzü durumu (0 = gün doğumu).
pub fn sky_state(time: f64) -> SkyState {
    let a = sun_angle(time);
    let (x, y, z) = (a.cos() as f32, a.sin() as f32, 0.35f32);
    let len = (x * x + y * y + z * z).sqrt();
    let sun_dir = [x / len, y / len, z / len];

    // Güneş ufkun biraz altındayken alacakaranlık sürer.
    let day = smoothstep(-0.12, 0.22, sun_dir[1]);
    let mut top = mix([0.012, 0.02, 0.06], [0.35, 0.65, 0.94], day);
    let mut horizon = mix([0.04, 0.06, 0.13], [0.81, 0.90, 0.98], day);

    // Gün doğumu / batımı: ufuk turuncuya, tepe hafif mora döner.
    let glow = (-(sun_dir[1] / 0.2).powi(2)).exp();
    horizon = mix(horizon, [0.98, 0.56, 0.30], glow * 0.6);
    top = mix(top, [0.36, 0.32, 0.55], glow * 0.3);

    SkyState {
        sun_dir,
        top,
        horizon,
        light: NIGHT_LIGHT + (1.0 - NIGHT_LIGHT) * day,
        stars: 1.0 - day,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_is_up_by_day_and_down_by_night() {
        let noon = sky_state(DAY_FRACTION / 2.0);
        assert!(noon.sun_dir[1] > 0.9, "öğlen güneş tepede olmalı");
        assert!(noon.light > 0.99 && noon.stars < 0.01);

        let midnight = sky_state(DAY_FRACTION + (1.0 - DAY_FRACTION) / 2.0);
        assert!(midnight.sun_dir[1] < -0.9);
        assert!((midnight.light - NIGHT_LIGHT).abs() < 1e-3 && midnight.stars > 0.99);
        assert!(midnight.top[2] < 0.1, "gece gökyüzü karanlık olmalı");
    }

    #[test]
    fn game_starts_in_daylight() {
        let s = sky_state(START_TIME);
        assert!(s.light > 0.95, "oyun aydınlıkta başlamalı: {}", s.light);
    }

    #[test]
    fn sunset_tints_the_horizon_orange() {
        let s = sky_state(DAY_FRACTION);
        assert!(s.horizon[0] > s.horizon[2], "gün batımında ufuk sıcak renkte olmalı: {:?}", s.horizon);
        let noon = sky_state(DAY_FRACTION / 2.0);
        assert!(noon.horizon[2] > noon.horizon[0], "öğlen ufuk mavi olmalı");
    }

    #[test]
    fn cycle_is_continuous_and_bounded() {
        let mut prev = sky_state(0.0);
        for i in 1..=2000 {
            let s = sky_state(i as f64 / 2000.0);
            assert!((NIGHT_LIGHT - 1e-4..=1.0001).contains(&s.light));
            assert!((s.light - prev.light).abs() < 0.02, "ışık sıçradı (t={})", i as f64 / 2000.0);
            for c in 0..3 {
                assert!((s.top[c] - prev.top[c]).abs() < 0.03 && (0.0..=1.0).contains(&s.top[c]));
                assert!((s.horizon[c] - prev.horizon[c]).abs() < 0.03);
            }
            let l: f32 = s.sun_dir.iter().map(|v| v * v).sum();
            assert!((l - 1.0).abs() < 1e-4);
            prev = s;
        }
        assert_eq!(sky_state(0.25), sky_state(1.25), "döngü 1'de sarmalı");
    }

    #[test]
    fn clock_matches_the_sun() {
        assert!((clock_hours(0.0) - 6.0).abs() < 1e-9);
        assert!((clock_hours(DAY_FRACTION / 2.0) - 12.0).abs() < 1e-9);
        assert!((clock_hours(DAY_FRACTION) - 18.0).abs() < 1e-9);
        assert!((clock_hours(DAY_FRACTION + (1.0 - DAY_FRACTION) / 2.0) - 0.0).abs() < 1e-9);
    }
}
