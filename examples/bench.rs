//! Yerel (tarayıcısız) ölçüm aracı: chunk üretimi, mesh üretimi ve
//! yükseklik hesabının maliyeti. Wasm derlemesine dâhil edilmez.
//!
//! Kullanım:
//!   cargo run --release --example bench
//!
//! Not: tarayıcıdaki kare süreleri için `debug_state()` içindeki `prof`
//! alanına bakın (bkz. README → Performans tasarımı).
use std::time::Instant;
use voxelcraft_rs::mesh::{build_chunk_mesh_into, MeshBuffers};
use voxelcraft_rs::world::World;

fn main() {
    let seed = 20261012;
    let mut w = World::new(seed);

    // --- Veri üretimi ---
    let coords: Vec<(i32, i32)> = (0..12)
        .flat_map(|z| (0..12).map(move |x| (x, z)))
        .collect();
    let t = Instant::now();
    for &(cx, cz) in &coords {
        w.generate_chunk(cx, cz);
    }
    let gen = t.elapsed().as_secs_f64() / coords.len() as f64;

    // --- Mesh üretimi ---
    let mut buf = MeshBuffers::default();
    let mut meshed = 0usize;
    let mut quads = 0usize;
    let t = Instant::now();
    for &(cx, cz) in &coords {
        if build_chunk_mesh_into(&w, cx, cz, &mut buf) {
            meshed += 1;
            quads += buf.solid.face_count() as usize + buf.liquid.face_count() as usize;
        }
    }
    let mesh = t.elapsed().as_secs_f64() / meshed.max(1) as f64;

    println!(
        "chunk üretimi: {:.3} ms/chunk   mesh: {:.3} ms/chunk   ({meshed} chunk, ort. {} yüz)",
        gen * 1000.0,
        mesh * 1000.0,
        quads / meshed.max(1)
    );

    // --- Yüksekliği tekrar tekrar hesaplamanın maliyeti ---
    let t = Instant::now();
    let mut acc = 0i64;
    for z in 0..64 {
        for x in 0..64 {
            acc += w.height_at(x, z) as i64;
        }
    }
    let h = t.elapsed().as_secs_f64() / (64.0 * 64.0);
    println!("height_at: {:.4} ms/çağrı (acc {acc})", h * 1000.0);
}
