//! VoxelCraft — Rust + WebAssembly sürümü.
//!
//! # Mimari
//!
//! Motor bilerek ikiye ayrılmıştır:
//!
//! * **Saf çekirdek** (`world`, `noise`, `mesh`, `atlas`, `raycast`, `player`,
//!   `inventory`, `crafting`, `survival`) — hiçbir tarayıcı API'si kullanmaz.
//!   `cargo test` ile tarayıcı olmadan doğrulanabilir.
//! * **Tarayıcı katmanı** (`app`, `gl`, `hud`, `mat4`) — yalnızca
//!   `wasm32` hedefinde derlenir; WebGL2 çizimi ve canvas arayüzünü yapar.

pub mod atlas;
pub mod blocks;
pub mod crafting;
pub mod inventory;
pub mod mat4;
pub mod mesh;
pub mod noise;
pub mod player;
pub mod raycast;
pub mod survival;
pub mod world;

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod gl;
#[cfg(target_arch = "wasm32")]
mod hud;

/// Tarayıcı giriş noktası (`index.html` içinden çağrılır).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    app::start()
}

/// Hata ayıklama / otomasyon kancası: oyun durumunu JSON olarak döndürür.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_state() -> String {
    app::debug_state_json()
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_set_health(v: i32) {
    app::debug_set_health(v)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_set_hunger(v: i32) {
    app::debug_set_hunger(v)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_damage(n: i32) {
    app::debug_damage(n)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_set_mode(creative: bool) {
    app::debug_set_mode(creative)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_set_look(yaw: f64, pitch: f64) {
    app::debug_set_look(yaw, pitch)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_break() {
    app::debug_break()
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_right_click() {
    app::debug_right_click()
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_place() -> bool {
    app::debug_place()
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_place_consumed() -> i32 {
    app::debug_place_consumed()
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_select(i: usize) {
    app::debug_select(i)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_set_open(open: bool) {
    app::debug_set_open(open)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_sim(seconds: f64) {
    app::debug_sim(seconds)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_teleport(x: f64, y: f64, z: f64) {
    app::debug_teleport(x, y, z)
}

/// Oyuncuyu deniz seviyesinin altındaki bir sütuna ışınlar (yüzme testleri).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_teleport_to_water() -> bool {
    app::debug_teleport_to_water()
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_add_item(id: u16, n: u32) {
    app::debug_add_item(id, n)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn debug_count(id: u16) -> u32 {
    app::debug_count(id)
}
