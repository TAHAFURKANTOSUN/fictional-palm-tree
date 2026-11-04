//! Küçük 4×4 matris kütüphanesi (sütun-major, WebGL kuralı).
//!
//! Harici bir matematik crate'i yerine yalnızca ihtiyaç duyulan işlemler
//! yazıldı: perspektif, kamera görünümü ve görüş konisi (frustum) elemesi.

pub type Mat4 = [f32; 16];

pub fn identity() -> Mat4 {
    [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
}

/// `a * b` (önce `b`, sonra `a` uygulanır).
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut o = [0.0f32; 16];
    for c in 0..4 {
        for r in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + r] * b[c * 4 + k];
            }
            o[c * 4 + r] = s;
        }
    }
    o
}

pub fn perspective(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fovy_rad * 0.5).tan();
    let nf = 1.0 / (near - far);
    [
        f / aspect, 0.0, 0.0, 0.0,
        0.0, f, 0.0, 0.0,
        0.0, 0.0, (far + near) * nf, -1.0,
        0.0, 0.0, 2.0 * far * near * nf, 0.0,
    ]
}

pub fn translate(x: f32, y: f32, z: f32) -> Mat4 {
    [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, x, y, z, 1.0]
}

pub fn rot_x(a: f32) -> Mat4 {
    let (s, c) = a.sin_cos();
    [1.0, 0.0, 0.0, 0.0, 0.0, c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0]
}

pub fn rot_y(a: f32) -> Mat4 {
    let (s, c) = a.sin_cos();
    [c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0]
}

/// Kamera görünüm matrisi: `R_x(-pitch) * R_y(-yaw) * T(-pos)`.
///
/// `YXZ` Euler sırasının tersidir; bakış yönü `player::Player::look_dir` ile
/// tutarlıdır.
pub fn view(yaw: f32, pitch: f32, pos: [f32; 3]) -> Mat4 {
    let t = translate(-pos[0], -pos[1], -pos[2]);
    mul(&view_rotation(yaw, pitch), &t)
}

/// Öteleme içermeyen görünüm matrisi: `R_x(-pitch) * R_y(-yaw)`.
///
/// **Kameraya göre** çizimde kullanılır: köşeler gölgelendiricide önce kamera
/// konumundan çıkarılır, böylece büyük dünya koordinatlarında (±50 000) f32
/// hassasiyet kaybı olmaz.
pub fn view_rotation(yaw: f32, pitch: f32) -> Mat4 {
    mul(&rot_x(-pitch), &rot_y(-yaw))
}

/// Görüş konisi — Gribb–Hartmann yöntemiyle düzlem çıkarımı.
pub struct Frustum {
    planes: [[f32; 4]; 6],
}

impl Frustum {
    pub fn from_view_proj(m: &Mat4) -> Self {
        // Sütun-major: m[col*4 + row]
        let row = |i: usize| [m[i], m[4 + i], m[8 + i], m[12 + i]];
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));

        let mut planes = [[0.0f32; 4]; 6];
        let combos: [[f32; 4]; 6] = [
            [r3[0] + r0[0], r3[1] + r0[1], r3[2] + r0[2], r3[3] + r0[3]], // sol
            [r3[0] - r0[0], r3[1] - r0[1], r3[2] - r0[2], r3[3] - r0[3]], // sağ
            [r3[0] + r1[0], r3[1] + r1[1], r3[2] + r1[2], r3[3] + r1[3]], // alt
            [r3[0] - r1[0], r3[1] - r1[1], r3[2] - r1[2], r3[3] - r1[3]], // üst
            [r3[0] + r2[0], r3[1] + r2[1], r3[2] + r2[2], r3[3] + r2[3]], // yakın
            [r3[0] - r2[0], r3[1] - r2[1], r3[2] - r2[2], r3[3] - r2[3]], // uzak
        ];

        for (i, p) in combos.iter().enumerate() {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            let inv = if len > 0.0 { 1.0 / len } else { 1.0 };
            planes[i] = [p[0] * inv, p[1] * inv, p[2] * inv, p[3] * inv];
        }
        Self { planes }
    }

    /// Eksen hizalı kutu koninin içinde (veya kesişiyor) mu?
    ///
    /// Her düzlem için normal yönünde en uzak köşe ("p-köşesi") sınanır.
    /// Chunk'lar ince ve yatay olduğundan küreden belirgin şekilde sıkıdır.
    pub fn aabb_visible(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        for p in self.planes.iter() {
            let x = if p[0] >= 0.0 { max[0] } else { min[0] };
            let y = if p[1] >= 0.0 { max[1] } else { min[1] };
            let z = if p[2] >= 0.0 { max[2] } else { min[2] };
            if p[0] * x + p[1] * y + p[2] * z + p[3] < 0.0 {
                return false;
            }
        }
        true
    }

    /// Kapsayıcı küre koninin içinde (veya kesişiyor) mu?
    pub fn sphere_visible(&self, center: [f32; 3], radius: f32) -> bool {
        for p in self.planes.iter() {
            let d = p[0] * center[0] + p[1] * center[1] + p[2] * center[2] + p[3];
            if d < -radius {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_neutral() {
        let i = identity();
        let m = perspective(1.0, 1.5, 0.1, 100.0);
        let r = mul(&i, &m);
        for k in 0..16 {
            assert!((r[k] - m[k]).abs() < 1e-6);
        }
    }

    #[test]
    fn view_matrix_maps_camera_position_to_origin() {
        let v = view(0.7, -0.3, [10.0, 20.0, 30.0]);
        // Kameranın kendi konumu görünüm uzayında orijinde olmalı.
        let p = [10.0, 20.0, 30.0, 1.0];
        let mut out = [0.0f32; 4];
        for r in 0..4 {
            out[r] = v[r] * p[0] + v[4 + r] * p[1] + v[8 + r] * p[2] + v[12 + r] * p[3];
        }
        for k in 0..3 {
            assert!(out[k].abs() < 1e-4, "eksen {k} sıfır değil: {}", out[k]);
        }
    }

    #[test]
    fn frustum_contains_the_point_in_front_of_the_camera() {
        let vp = mul(
            &perspective(1.2, 1.6, 0.1, 200.0),
            &view(0.0, 0.0, [0.0, 0.0, 0.0]),
        );
        let f = Frustum::from_view_proj(&vp);
        // Kamera -Z yönüne bakar; ilerideki bir nokta görünür olmalı.
        assert!(f.sphere_visible([0.0, 0.0, -10.0], 1.0));
        // Arkadaki bir nokta elenmeli.
        assert!(!f.sphere_visible([0.0, 0.0, 10.0], 1.0));
        // Çok yandaki bir nokta elenmeli.
        assert!(!f.sphere_visible([500.0, 0.0, -10.0], 1.0));
    }

    #[test]
    fn frustum_keeps_spheres_that_overlap_the_edge() {
        let vp = mul(
            &perspective(1.2, 1.6, 0.1, 200.0),
            &view(0.0, 0.0, [0.0, 0.0, 0.0]),
        );
        let f = Frustum::from_view_proj(&vp);
        // Merkez dışarıda ama küre kenara değiyor.
        let mut visible = false;
        for x in 0..200 {
            let cx = 20.0 + x as f32;
            if f.sphere_visible([cx, 0.0, -20.0], 30.0) {
                visible = true;
                break;
            }
        }
        assert!(visible, "büyük küre kenardan görünmeliydi");
    }

    #[test]
    fn aabb_culling_matches_geometry() {
        let vp = mul(
            &perspective(1.2, 1.6, 0.1, 200.0),
            &view(0.0, 0.0, [0.0, 0.0, 0.0]),
        );
        let f = Frustum::from_view_proj(&vp);
        // İleride, arkada, çok yanda.
        assert!(f.aabb_visible([-1.0, -1.0, -11.0], [1.0, 1.0, -9.0]));
        assert!(!f.aabb_visible([-1.0, -1.0, 9.0], [1.0, 1.0, 11.0]));
        assert!(!f.aabb_visible([499.0, -1.0, -11.0], [501.0, 1.0, -9.0]));
        // Kamerayı içine alan kutu her zaman görünür.
        assert!(f.aabb_visible([-8.0, -8.0, -8.0], [8.0, 8.0, 8.0]));
        // Uzak düzlemin ötesi elenir.
        assert!(!f.aabb_visible([-1.0, -1.0, -260.0], [1.0, 1.0, -250.0]));
    }

    /// Kutu testi, kutunun 8 köşesinden biri bile konideyse kutuyu elememeli
    /// (muhafazakâr olmalı) ve küre testinden asla daha gevşek olmamalı.
    #[test]
    fn aabb_is_conservative_and_tighter_than_sphere() {
        let vp = mul(
            &perspective(1.3, 1.7, 0.05, 180.0),
            &view(0.6, -0.25, [3.0, 40.0, -7.0]),
        );
        let f = Frustum::from_view_proj(&vp);
        let inside = |p: [f32; 3]| {
            let c = [
                vp[0] * p[0] + vp[4] * p[1] + vp[8] * p[2] + vp[12],
                vp[1] * p[0] + vp[5] * p[1] + vp[9] * p[2] + vp[13],
                vp[2] * p[0] + vp[6] * p[1] + vp[10] * p[2] + vp[14],
                vp[3] * p[0] + vp[7] * p[1] + vp[11] * p[2] + vp[15],
            ];
            c[3] > 0.0 && (0..3).all(|i| c[i].abs() <= c[3])
        };
        let mut culled = 0;
        let mut sphere_kept_box_culled = 0;
        for cz in -12..=12 {
            for cx in -12..=12 {
                let min = [cx as f32 * 16.0, 20.0, cz as f32 * 16.0];
                let max = [min[0] + 16.0, 46.0, min[2] + 16.0];
                let any_corner = (0..8).any(|k| {
                    inside([
                        if k & 1 == 0 { min[0] } else { max[0] },
                        if k & 2 == 0 { min[1] } else { max[1] },
                        if k & 4 == 0 { min[2] } else { max[2] },
                    ])
                });
                let boxed = f.aabb_visible(min, max);
                if any_corner {
                    assert!(boxed, "görünen kutu elendi: {min:?}");
                }
                let c = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0, (min[2] + max[2]) / 2.0];
                let r = ((8.0f32).powi(2) * 2.0 + 13.0f32.powi(2)).sqrt();
                let sphere = f.sphere_visible(c, r);
                assert!(!boxed || sphere, "kutu küreden gevşek olamaz");
                culled += !boxed as u32;
                sphere_kept_box_culled += (sphere && !boxed) as u32;
            }
        }
        assert!(culled > 0 && sphere_kept_box_culled > 0, "kutu testi daha sıkı olmalı");
    }

    #[test]
    fn view_is_rotation_after_translation() {
        let (yaw, pitch, pos) = (0.8f32, -0.4f32, [5.0f32, 31.0, -12.0]);
        let a = view(yaw, pitch, pos);
        let b = mul(&view_rotation(yaw, pitch), &translate(-pos[0], -pos[1], -pos[2]));
        for k in 0..16 {
            assert!((a[k] - b[k]).abs() < 1e-6);
        }
        // Rotasyon matrisi ötelemesizdir.
        let r = view_rotation(yaw, pitch);
        assert_eq!([r[12], r[13], r[14]], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn rot_y_turns_forward_vector() {
        // yaw = 90°: ileri vektörü -X olmalı.
        let m = rot_y(std::f32::consts::FRAC_PI_2);
        let fwd = [0.0, 0.0, -1.0];
        let x = m[0] * fwd[0] + m[4] * fwd[1] + m[8] * fwd[2];
        assert!((x + 1.0).abs() < 1e-5, "beklenen -1, bulunan {x}");
    }
}
