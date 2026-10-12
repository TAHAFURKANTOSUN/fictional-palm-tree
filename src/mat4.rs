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
    mul(&mul(&rot_x(-pitch), &rot_y(-yaw)), &t)
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
    fn rot_y_turns_forward_vector() {
        // yaw = 90°: ileri vektörü -X olmalı.
        let m = rot_y(std::f32::consts::FRAC_PI_2);
        let fwd = [0.0, 0.0, -1.0];
        let x = m[0] * fwd[0] + m[4] * fwd[1] + m[8] * fwd[2];
        assert!((x + 1.0).abs() < 1e-5, "beklenen -1, bulunan {x}");
    }
}
