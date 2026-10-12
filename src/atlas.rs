//! Tamamen prosedürel doku atlası.
//!
//! 16×16 piksellik 16 döşeme tek bir 256×16 RGBA tamponuna çizilir ve WebGL'e
//! tek doku olarak yüklenir. Hiçbir görsel dosya kullanılmaz.

use crate::noise::hash2f;

pub const TILE: i32 = 16;
pub const ATLAS_COLS: i32 = 16;
pub const ATLAS_W: usize = (TILE * ATLAS_COLS) as usize;
pub const ATLAS_H: usize = TILE as usize;

#[inline]
fn clamp255(v: f64) -> u8 {
    if v < 0.0 {
        0
    } else if v > 255.0 {
        255
    } else {
        (v + 0.5).floor() as u8
    }
}

/// Temel rengi `t ∈ [0,1]` gürültüsüyle `amt` kadar aydınlatır/koyulaştırır.
#[inline]
fn shade(base: [f64; 3], t: f64, amt: f64) -> [u8; 4] {
    let f = 1.0 + (t - 0.5) * 2.0 * amt;
    [clamp255(base[0] * f), clamp255(base[1] * f), clamp255(base[2] * f), 255]
}

/// `index` numaralı döşemeyi atlasa çizer.
fn paint(atlas: &mut [u8], index: u8, f: impl Fn(i32, i32) -> [u8; 4]) {
    let x_off = index as usize * TILE as usize;
    for y in 0..TILE {
        for x in 0..TILE {
            let c = f(x, y);
            let o = (y as usize * ATLAS_W + x_off + x as usize) * 4;
            atlas[o..o + 4].copy_from_slice(&c);
        }
    }
}

/// Atlası sıfırdan üretir; çıktı `ATLAS_W * ATLAS_H * 4` baytlık RGBA'dır.
pub fn build_atlas() -> Vec<u8> {
    let mut a = vec![0u8; ATLAS_W * ATLAS_H * 4];

    // 0 — çim üstü
    paint(&mut a, crate::blocks::T_GRASS_TOP, |x, y| {
        shade([98.0, 163.0, 56.0], hash2f(x, y, 161), 0.18)
    });

    // 1 — çim yanı: üstte çim şeridi, altta toprak
    paint(&mut a, crate::blocks::T_GRASS_SIDE, |x, y| {
        let n = hash2f(x, y, 171);
        let boundary = 3 + (hash2f(x, 0, 172) * 2.6).floor() as i32;
        if y < boundary {
            shade([98.0, 163.0, 56.0], n, 0.18)
        } else {
            shade([134.0, 96.0, 67.0], n, 0.18)
        }
    });

    // 2 — toprak
    paint(&mut a, crate::blocks::T_DIRT, |x, y| {
        shade([134.0, 96.0, 67.0], hash2f(x, y, 181), 0.18)
    });

    // 3 — taş
    paint(&mut a, crate::blocks::T_STONE, |x, y| {
        shade([126.0, 126.0, 128.0], hash2f(x, y, 191), 0.14)
    });

    // 4 — parke taşı: 4×4 lekeler + derz karartması
    paint(&mut a, crate::blocks::T_COBBLE, |x, y| {
        let blob = hash2f(x / 4, y / 4, 71);
        let n = hash2f(x, y, 72);
        let mut v = 92.0 + blob * 58.0;
        v *= 1.0 + (n - 0.5) * 0.30;
        if x % 4 == 0 || y % 4 == 0 {
            v *= 0.70;
        }
        [clamp255(v), clamp255(v), clamp255(v * 1.03), 255]
    });

    // 5 — kum
    paint(&mut a, crate::blocks::T_SAND, |x, y| {
        shade([222.0, 208.0, 164.0], hash2f(x, y, 151), 0.12)
    });

    // 6 — kütük yanı: dikey damarlar
    paint(&mut a, crate::blocks::T_LOG_SIDE, |x, y| {
        let streak = hash2f(x / 2, y / 8, 121);
        let n = hash2f(x, y, 122);
        let f = 0.80 + streak * 0.34 + (n - 0.5) * 0.12;
        [clamp255(112.0 * f), clamp255(86.0 * f), clamp255(52.0 * f), 255]
    });

    // 7 — kütük üstü: yıllık halkalar
    paint(&mut a, crate::blocks::T_LOG_TOP, |x, y| {
        let dx = x as f64 - 7.5;
        let dy = y as f64 - 7.5;
        let r = (dx * dx + dy * dy).sqrt();
        let ring = 0.5 + 0.5 * (r * 2.6).sin();
        let n = hash2f(x, y, 111);
        let f = 0.76 + ring * 0.30 + (n - 0.5) * 0.08;
        [clamp255(158.0 * f), clamp255(128.0 * f), clamp255(84.0 * f), 255]
    });

    // 8 — yaprak: yoğun gürültü + koyu delikler
    paint(&mut a, crate::blocks::T_LEAVES, |x, y| {
        let n = hash2f(x, y, 141);
        let n2 = hash2f(x, y, 142);
        let mut f = 0.70 + n * 0.52;
        if n2 > 0.88 {
            f *= 0.58;
        }
        [clamp255(58.0 * f), clamp255(112.0 * f), clamp255(40.0 * f), 255]
    });

    // 9 — tahta: 4 piksel satırlar, kaydırılmış dikey ekler
    paint(&mut a, crate::blocks::T_PLANKS, |x, y| {
        let row = y / 4;
        let seam_y = if y % 4 == 0 { 0.78 } else { 1.0 };
        let seam_x = if (x + row * 5) % 8 == 0 { 0.82 } else { 1.0 };
        let s = shade([176.0, 138.0, 86.0], hash2f(x, y, 101), 0.12);
        let k = seam_y * seam_x;
        [
            clamp255(s[0] as f64 * k),
            clamp255(s[1] as f64 * k),
            clamp255(s[2] as f64 * k),
            255,
        ]
    });

    // 10 — tuğla: iki sıralı, kaydırılmış harç çizgileri
    paint(&mut a, crate::blocks::T_BRICK, |x, y| {
        let row = y / 4;
        let offset = (row % 2) * 4;
        if y % 4 == 0 || (x + offset) % 8 == 0 {
            return [186, 178, 170, 255];
        }
        shade([152.0, 66.0, 52.0], hash2f(x, y, 91), 0.14)
    });

    // 11 — cam: saydam iç, açık çerçeve, iki parlama lekesi
    paint(&mut a, crate::blocks::T_GLASS, |x, y| {
        let border = x == 0 || y == 0 || x == 15 || y == 15;
        let corner = (x == 0 || x == 15) && (y == 0 || y == 15);
        if corner {
            return [238, 248, 255, 205];
        }
        if border {
            return [212, 232, 246, 240];
        }
        let sparkle = (x == 3 && y == 4)
            || (x == 4 && y == 3)
            || (x == 4 && y == 4)
            || (x == 10 && y == 11)
            || (x == 11 && y == 10)
            || (x == 11 && y == 11);
        if sparkle {
            return [255, 255, 255, 190];
        }
        [0, 0, 0, 0]
    });

    // 12 — su
    paint(&mut a, crate::blocks::T_WATER, |x, y| {
        let n = hash2f(x, y, 131);
        let f = 0.92 + (n - 0.5) * 0.16;
        [
            clamp255(62.0 * f),
            clamp255(118.0 * f),
            clamp255(205.0 * f),
            255,
        ]
    });

    // 13 — çalışma tezgahı üstü: ızgara
    paint(&mut a, crate::blocks::T_CRAFT_TOP, |x, y| {
        let base = shade([176.0, 138.0, 86.0], hash2f(x, y, 201), 0.12);
        let mut k = 1.0;
        if x == 0 || y == 0 || x == 15 || y == 15 {
            k = 0.70;
        } else if x == 5 || x == 10 || y == 5 || y == 10 {
            k = 0.52;
        }
        [
            clamp255(base[0] as f64 * k),
            clamp255(base[1] as f64 * k),
            clamp255(base[2] as f64 * k),
            255,
        ]
    });

    // 14 — çalışma tezgahı yanı: alet izleri
    paint(&mut a, crate::blocks::T_CRAFT_SIDE, |x, y| {
        let base = shade([176.0, 138.0, 86.0], hash2f(x, y, 202), 0.12);
        let mut k = 1.0;
        if y == 3 {
            k = 0.52;
        } else if y < 3 {
            k = 0.76;
        } else if (5..=12).contains(&y) && (x == 3 || x == 12) {
            k = 0.60;
        } else if (8..=11).contains(&y) && (6..=9).contains(&x) {
            k = 0.58;
        }
        [
            clamp255(base[0] as f64 * k),
            clamp255(base[1] as f64 * k),
            clamp255(base[2] as f64 * k),
            255,
        ]
    });

    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_has_expected_dimensions() {
        let a = build_atlas();
        assert_eq!(a.len(), 256 * 16 * 4);
    }

    #[test]
    fn every_tile_has_visible_pixels() {
        let a = build_atlas();
        for tile in 0..15u8 {
            // Cam kasıtlı olarak çoğunlukla saydamdır; eşiği ona göre düşürüyoruz.
            let min = if tile == crate::blocks::T_GLASS { 60 } else { 200 };
            let mut opaque = 0;
            let x_off = tile as usize * TILE as usize;
            for y in 0..TILE as usize {
                for x in 0..TILE as usize {
                    let o = (y * ATLAS_W + x_off + x) * 4;
                    if a[o + 3] > 0 {
                        opaque += 1;
                    }
                }
            }
            assert!(opaque >= min, "döşeme {tile} boş görünüyor ({opaque} piksel)");
        }
    }

    #[test]
    fn glass_is_mostly_transparent() {
        let a = build_atlas();
        let x_off = crate::blocks::T_GLASS as usize * TILE as usize;
        let mut transparent = 0;
        for y in 0..TILE as usize {
            for x in 0..TILE as usize {
                let o = (y * ATLAS_W + x_off + x) * 4;
                if a[o + 3] == 0 {
                    transparent += 1;
                }
            }
        }
        assert!(transparent > 150, "cam yeterince saydam değil: {transparent}");
    }

    #[test]
    fn water_is_opaque_and_blue() {
        let a = build_atlas();
        let x_off = crate::blocks::T_WATER as usize * TILE as usize;
        let o = (5 * ATLAS_W + x_off + 5) * 4;
        assert_eq!(a[o + 3], 255);
        assert!(a[o + 2] > a[o], "su mavi baskın olmalı");
    }

    #[test]
    fn atlas_is_deterministic() {
        assert_eq!(build_atlas(), build_atlas());
    }

    #[test]
    fn grass_side_has_green_top_and_brown_bottom() {
        let a = build_atlas();
        let x_off = crate::blocks::T_GRASS_SIDE as usize * TILE as usize;
        let top = (0 * ATLAS_W + x_off + 5) * 4;
        let bottom = (15 * ATLAS_W + x_off + 5) * 4;
        assert!(a[top + 1] > a[top + 0], "çim yanının üstü yeşil olmalı");
        assert!(a[bottom + 0] > a[bottom + 1], "çim yanının altı kahverengi olmalı");
    }
}
