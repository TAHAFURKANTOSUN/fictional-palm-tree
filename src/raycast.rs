//! Voxel ışın izleme (Amanatides–Woo DDA).
//!
//! Adım adım ilerleyen taramanın aksine ışın, hücre sınırlarını atlayarak
//! ilerler: blok başına sabit iş yapılır ve 6 blokluk menzil için bile
//! mikrosaniyenin altında kalır.

use crate::blocks::{AIR, WATER};
use crate::world::World;

pub struct Hit {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Vurulan yüzün normali — bloğun hangi tarafına yerleştirileceğini belirler.
    pub nx: i32,
    pub ny: i32,
    pub nz: i32,
}

/// `(ox,oy,oz)` noktasından `(dx,dy,dz)` yönünde ışın atar.
/// Su ve hava saydamdır; `max_dist` aşılırsa `None` döner.
pub fn raycast_voxel(
    world: &World,
    ox: f64,
    oy: f64,
    oz: f64,
    dx: f64,
    dy: f64,
    dz: f64,
    max_dist: f64,
) -> Option<Hit> {
    let mut x = ox.floor() as i32;
    let mut y = oy.floor() as i32;
    let mut z = oz.floor() as i32;

    let step_x = if dx > 0.0 { 1 } else { -1 };
    let step_y = if dy > 0.0 { 1 } else { -1 };
    let step_z = if dz > 0.0 { 1 } else { -1 };

    let tdx = if dx != 0.0 { (1.0 / dx).abs() } else { f64::INFINITY };
    let tdy = if dy != 0.0 { (1.0 / dy).abs() } else { f64::INFINITY };
    let tdz = if dz != 0.0 { (1.0 / dz).abs() } else { f64::INFINITY };

    let mut tmx = if dx != 0.0 {
        (if dx > 0.0 { x as f64 + 1.0 - ox } else { ox - x as f64 }) * tdx
    } else {
        f64::INFINITY
    };
    let mut tmy = if dy != 0.0 {
        (if dy > 0.0 { y as f64 + 1.0 - oy } else { oy - y as f64 }) * tdy
    } else {
        f64::INFINITY
    };
    let mut tmz = if dz != 0.0 {
        (if dz > 0.0 { z as f64 + 1.0 - oz } else { oz - z as f64 }) * tdz
    } else {
        f64::INFINITY
    };

    let (mut nx, mut ny, mut nz): (i32, i32, i32);
    let mut t;

    for _ in 0..256 {
        if tmx < tmy {
            if tmx < tmz {
                x += step_x;
                t = tmx;
                tmx += tdx;
                nx = -step_x;
                ny = 0;
                nz = 0;
            } else {
                z += step_z;
                t = tmz;
                tmz += tdz;
                nx = 0;
                ny = 0;
                nz = -step_z;
            }
        } else if tmy < tmz {
            y += step_y;
            t = tmy;
            tmy += tdy;
            nx = 0;
            ny = -step_y;
            nz = 0;
        } else {
            z += step_z;
            t = tmz;
            tmz += tdz;
            nx = 0;
            ny = 0;
            nz = -step_z;
        }

        if t > max_dist {
            return None;
        }

        let b = world.get_block(x, y, z);
        if b != AIR && b != WATER {
            return Some(Hit { x, y, z, nx, ny, nz });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{AIR, BRICK, STONE, WATER};

    /// Üst yüzeyi tam olarak `y = 20` olan düz bir test dünyası
    /// (0..=20 katı, 21.. katı değil). Böylece ışın her zaman havadan başlar.
    const GROUND_TOP: i32 = 20;

    fn flat_world() -> World {
        let mut w = World::new(20261012);
        for dz in -2..=2 {
            for dx in -2..=2 {
                w.generate_chunk(dx, dz);
            }
        }
        for x in -20..20 {
            for z in -20..20 {
                for y in 0..=GROUND_TOP {
                    w.set_block_raw(x, y, z, STONE);
                }
                for y in (GROUND_TOP + 1)..40 {
                    w.set_block_raw(x, y, z, AIR);
                }
            }
        }
        w
    }

    #[test]
    fn ray_hits_floor_when_looking_straight_down() {
        let w = flat_world();
        let hit = raycast_voxel(&w, 0.5, 25.0, 0.5, 0.0, -1.0, 0.0, 6.0).expect("zemine çarpmadı");
        assert_eq!(hit.y, GROUND_TOP, "zeminin üst bloğuna çarpmalı");
        assert_eq!(hit.ny, 1, "üst yüzün normali +Y olmalı");
    }

    #[test]
    fn ray_hits_wall_when_looking_horizontally() {
        let mut w = flat_world();
        for y in (GROUND_TOP + 1)..(GROUND_TOP + 5) {
            for z in 0..3 {
                w.set_block_raw(4, y, z, BRICK);
            }
        }
        let hit = raycast_voxel(&w, 0.5, 21.5, 0.5, 1.0, 0.0, 0.0, 6.0).expect("duvara çarpmadı");
        assert_eq!(hit.x, 4);
        assert_eq!(hit.nx, -1, "duvarın batı yüzü -X normali vermeli");
    }

    #[test]
    fn ray_returns_none_beyond_reach() {
        let w = flat_world();
        // 4 blok aşağıda zemin var ama menzil 2 blok.
        assert!(raycast_voxel(&w, 0.5, 25.0, 0.5, 0.0, -1.0, 0.0, 2.0).is_none());
    }

    #[test]
    fn ray_returns_none_when_nothing_is_in_range() {
        let w = flat_world();
        // Gökyüzüne bak: hiçbir şeye çarpmamalı.
        assert!(raycast_voxel(&w, 0.5, 25.0, 0.5, 0.0, 1.0, 0.0, 6.0).is_none());
    }

    #[test]
    fn ray_passes_through_water_and_air() {
        let mut w = flat_world();
        w.set_block_raw(2, GROUND_TOP + 1, 0, WATER);
        w.set_block_raw(4, GROUND_TOP + 1, 0, STONE);
        let hit = raycast_voxel(&w, 0.5, 21.5, 0.5, 1.0, 0.0, 0.0, 8.0).unwrap();
        assert_eq!(hit.x, 4, "ışın suyun içinde durmamalı");
    }

    #[test]
    fn diagonal_ray_hits_the_ground() {
        let w = flat_world();
        let hit = raycast_voxel(&w, 0.5, 26.0, 0.5, 0.6, -0.7, 0.4, 40.0);
        let hit = hit.expect("çapraz ışın zemine ulaşmalı");
        assert_eq!(hit.y, GROUND_TOP);
    }

    #[test]
    fn ray_terminates_without_overflow_on_axis_aligned_direction() {
        let w = flat_world();
        assert!(raycast_voxel(&w, 0.5, 100.0, 0.5, 0.0, 1.0, 0.0, 6.0).is_none());
    }

    #[test]
    fn hit_normal_points_back_at_the_camera() {
        let mut w = flat_world();
        for y in (GROUND_TOP + 1)..(GROUND_TOP + 5) {
            w.set_block_raw(6, y, 0, BRICK);
        }
        let hit = raycast_voxel(&w, 0.5, 21.5, 0.5, 1.0, 0.0, 0.0, 8.0).unwrap();
        // Yerleştirme konumu = vurulan blok + normal; bu boş olmalı.
        let px = hit.x + hit.nx;
        let py = hit.y + hit.ny;
        let pz = hit.z + hit.nz;
        assert_eq!(w.get_block(px, py, pz), AIR, "normalin gösterdiği yer boş olmalı");
    }
}
