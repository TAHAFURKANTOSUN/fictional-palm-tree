//! Determinist gürültü fonksiyonları.
//!
//! Orijinal JavaScript motoruyla **aynı** sonucu üretmesi için tüm aritmetik
//! `f64` üzerinde ve `Math.imul` semantiği (`wrapping_mul`) korunarak yazıldı.
//! Bu sayede dünya düzeni iki sürümde de aynı karakterde kalır.

/// 32 bit tamsayı hash'i → `[0,1)` aralığında `f64`.
#[inline]
pub fn hash2f(x: i32, y: i32, s: i32) -> f64 {
    let mut h = x
        .wrapping_mul(374_761_393)
        ^ y.wrapping_mul(668_265_263)
        ^ s.wrapping_mul(1_274_126_177);
    h = (h ^ ((h as u32) >> 13) as i32).wrapping_mul(1_274_126_177);
    h ^= ((h as u32) >> 16) as i32;
    (h as u32) as f64 / 4_294_967_296.0
}

/// Üç boyutlu hash — ağaç yerleşimi ve yüz varyasyonu için.
#[inline]
pub fn hash3f(x: i32, y: i32, z: i32) -> f64 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(z.wrapping_mul(1_274_126_177));
    h = (h ^ ((h as u32) >> 13) as i32).wrapping_mul(1_274_126_177);
    h ^= ((h as u32) >> 16) as i32;
    (h as u32) as f64 / 4_294_967_296.0
}

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[inline]
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn grad(h: u8, x: f64, y: f64) -> f64 {
    match h & 3 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        _ => -x - y,
    }
}

/// Klasik 2B Perlin gürültüsü (permütasyon tablosu LCG ile karıştırılır).
pub struct Perlin {
    perm: [u8; 512],
}

impl Perlin {
    pub fn new(seed: u32) -> Self {
        let mut p = [0u8; 256];
        for (i, slot) in p.iter_mut().enumerate() {
            *slot = i as u8;
        }

        // `seed >>> 0 || 1` — sıfır tohumu 1'e çevrilir.
        let mut s = if seed == 0 { 1 } else { seed };
        let mut rnd = move || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            s as f64 / 4_294_967_296.0
        };

        for i in (1..256).rev() {
            let j = (rnd() * (i as f64 + 1.0)).floor() as usize;
            p.swap(i, j);
        }

        let mut perm = [0u8; 512];
        for i in 0..512 {
            perm[i] = p[i & 255];
        }
        Self { perm }
    }

    /// Tek oktav gürültü. Çıktı yaklaşık `[-0.72, 0.72]` aralığındadır ve
    /// **normalize edilmez** — sonsuz dünyada süreklilik bunu gerektirir.
    pub fn noise2(&self, x: f64, y: f64) -> f64 {
        let fx = x.floor();
        let fy = y.floor();
        let xi = (fx as i32 & 255) as usize;
        let yi = (fy as i32 & 255) as usize;
        let xf = x - fx;
        let yf = y - fy;

        let u = fade(xf);
        let v = fade(yf);

        let aa = self.perm[self.perm[xi] as usize + yi] as u8;
        let ab = self.perm[self.perm[xi] as usize + yi + 1] as u8;
        let ba = self.perm[self.perm[xi + 1] as usize + yi] as u8;
        let bb = self.perm[self.perm[xi + 1] as usize + yi + 1] as u8;

        let x1 = lerp(grad(aa, xf, yf), grad(ba, xf - 1.0, yf), u);
        let x2 = lerp(grad(ab, xf, yf - 1.0), grad(bb, xf - 1.0, yf - 1.0), u);
        lerp(x1, x2, v) * 0.72
    }

    /// Katmanlı (fractal) gürültü — her oktavda genlik yarıya, frekans ikiye.
    pub fn fbm(&self, x: f64, y: f64, oct: u32) -> f64 {
        let mut amp = 1.0;
        let mut freq = 1.0;
        let mut sum = 0.0;
        let mut norm = 0.0;
        for _ in 0..oct {
            sum += self.noise2(x * freq, y * freq) * amp;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perlin_is_deterministic() {
        let a = Perlin::new(12345);
        let b = Perlin::new(12345);
        for i in 0..200 {
            let x = i as f64 * 0.37;
            assert_eq!(a.noise2(x, -x), b.noise2(x, -x));
        }
    }

    #[test]
    fn different_seeds_differ() {
        let a = Perlin::new(1);
        let b = Perlin::new(2);
        let mut diff = 0;
        for i in 0..100 {
            let x = i as f64 * 0.11;
            if (a.noise2(x, x) - b.noise2(x, x)).abs() > 1e-9 {
                diff += 1;
            }
        }
        assert!(diff > 80, "farklı tohumlar benzer çıktı ({diff}/100)");
    }

    #[test]
    fn noise_stays_in_range() {
        let p = Perlin::new(999);
        for i in 0..5000 {
            let x = (i % 100) as f64 * 1.7 - 80.0;
            let y = (i / 100) as f64 * 2.3 - 60.0;
            let v = p.noise2(x, y);
            assert!(v.abs() <= 0.75, "gürültü aralık dışı: {v}");
        }
    }

    #[test]
    fn noise_is_continuous_across_integer_boundaries() {
        // Chunk sınırı sürekliliğinin temeli: komşu örnekler arasında sıçrama olmamalı.
        let p = Perlin::new(4242);
        let mut max_jump = 0.0f64;
        for i in 0..2000 {
            let x = i as f64 * 0.05;
            let d = (p.noise2(x, 3.0) - p.noise2(x + 0.001, 3.0)).abs();
            if d > max_jump {
                max_jump = d;
            }
        }
        assert!(max_jump < 0.01, "gürültüde sıçrama var: {max_jump}");
    }

    #[test]
    fn fbm_is_bounded() {
        let p = Perlin::new(7);
        for i in 0..2000 {
            let x = i as f64 * 0.31;
            let v = p.fbm(x, x * 0.5, 4);
            assert!(v.abs() <= 0.75, "fbm aralık dışı: {v}");
        }
    }

    #[test]
    fn hash_is_in_unit_interval() {
        for i in -50..50 {
            for j in -50..50 {
                let v = hash2f(i, j, 13);
                assert!((0.0..1.0).contains(&v), "hash aralık dışı: {v}");
                let v3 = hash3f(i, j, 5);
                assert!((0.0..1.0).contains(&v3));
            }
        }
    }

    #[test]
    fn zero_seed_is_replaced() {
        // `seed >>> 0 || 1` davranışı: sıfır tohum çökmeye yol açmamalı.
        let p = Perlin::new(0);
        let v = p.noise2(1.5, 2.5);
        assert!(v.is_finite());
    }
}
