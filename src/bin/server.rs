//! VoxelCraft çok oyunculu sunucu ve HTTP web sunucusu ikilisi.
//!
//! Kullanım:
//! ```text
//! cargo run --release --bin server -- [port] [seed]
//! ```

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use std::env;
    use std::path::Path;
    use voxelcraft_rs::net_server::run_server;

    let args: Vec<String> = env::args().collect();
    let port: u16 = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);
    let seed: i32 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20261012);

    if let Err(e) = run_server(port, seed, Path::new(".")) {
        eprintln!("Sunucu hatası: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}
