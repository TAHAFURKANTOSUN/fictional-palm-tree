//! Motor çekirdeği için basit, bağımlılıksız performans ölçümü.
//!
//! Tarayıcı olmadan chunk üretimi ve mesh kurulumu sürelerini ölçer. Paylaşılan
//! makinelerdeki gürültüyü azaltmak için her aşama birkaç kez çalıştırılır ve
//! **en iyi** tur raporlanır.
//!
//! ```bash
//! cargo run --release --example bench
//! ```

use std::time::Instant;

use voxelcraft_rs::mesh::{build_chunk_mesh_into, MeshData};
use voxelcraft_rs::world::World;

const R: i32 = 8; // oyunun varsayılan görüş mesafesi
const ROUNDS: usize = 15;
const SEED: i32 = 20261012;

fn generate(world: &mut World) -> usize {
    let mut n = 0;
    for cz in -(R + 1)..=(R + 1) {
        for cx in -(R + 1)..=(R + 1) {
            world.generate_chunk(cx, cz);
            n += 1;
        }
    }
    n
}

fn main() {
    // 1) Veri üretimi: görüş + 1 halka (her turda yeni dünya).
    let mut gen_best = f64::MAX;
    let mut gen = 0;
    let mut world = World::new(SEED);
    for _ in 0..ROUNDS {
        world = World::new(SEED);
        let t = Instant::now();
        gen = generate(&mut world);
        gen_best = gen_best.min(t.elapsed().as_secs_f64() * 1000.0);
    }

    // 2) Mesh kurulumu. Oyun döngüsü gibi tamponlar yeniden kullanılır.
    let (mut s, mut l) = (MeshData::default(), MeshData::default());
    let mut mesh_best = f64::MAX;
    let (mut faces, mut verts, mut meshed) = (0u64, 0u64, 0usize);
    for _ in 0..ROUNDS {
        (faces, verts, meshed) = (0, 0, 0);
        let t = Instant::now();
        for cz in -R..=R {
            for cx in -R..=R {
                if build_chunk_mesh_into(&world, cx, cz, &mut s, &mut l) {
                    faces += (s.face_count() + l.face_count()) as u64;
                    verts += (s.vertex_count() + l.vertex_count()) as u64;
                    meshed += 1;
                }
            }
        }
        mesh_best = mesh_best.min(t.elapsed().as_secs_f64() * 1000.0);
    }

    println!(
        "chunk üretimi : {gen} chunk, {gen_best:8.2} ms  ({:.3} ms/chunk)",
        gen_best / gen as f64
    );
    println!(
        "mesh kurulumu : {meshed} chunk, {mesh_best:8.2} ms  ({:.3} ms/chunk)",
        mesh_best / meshed as f64
    );
    println!("geometri      : {faces} yüz, {verts} köşe");
}
