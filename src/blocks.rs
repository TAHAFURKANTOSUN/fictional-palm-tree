//! Blok ve eşya tanımları.
//!
//! Blok kimlikleri `u8`, blok olmayan eşyalar `100`'den başlayan `u16`'dır.
//! Bu ayrım envanterin tek bir tamsayı tipiyle çalışmasını sağlar.

/// Blok kimlikleri (chunk verisinde `u8` olarak saklanır).
pub const AIR: u8 = 0;
pub const GRASS: u8 = 1;
pub const DIRT: u8 = 2;
pub const STONE: u8 = 3;
pub const COBBLE: u8 = 4;
pub const SAND: u8 = 5;
pub const LOG: u8 = 6;
pub const LEAVES: u8 = 7;
pub const PLANKS: u8 = 8;
pub const BRICK: u8 = 9;
pub const GLASS: u8 = 10;
pub const WATER: u8 = 11;
pub const BEDROCK: u8 = 12;
pub const CRAFT_TABLE: u8 = 13;

/// Yalnızca eşya olan kimlikler (blok değildir).
pub const STICK: u16 = 100;
pub const APPLE: u16 = 101;

/// Doku atlasındaki döşeme indeksleri.
pub const T_GRASS_TOP: u8 = 0;
pub const T_GRASS_SIDE: u8 = 1;
pub const T_DIRT: u8 = 2;
pub const T_STONE: u8 = 3;
pub const T_COBBLE: u8 = 4;
pub const T_SAND: u8 = 5;
pub const T_LOG_SIDE: u8 = 6;
pub const T_LOG_TOP: u8 = 7;
pub const T_LEAVES: u8 = 8;
pub const T_PLANKS: u8 = 9;
pub const T_BRICK: u8 = 10;
pub const T_GLASS: u8 = 11;
pub const T_WATER: u8 = 12;
pub const T_CRAFT_TOP: u8 = 13;
pub const T_CRAFT_SIDE: u8 = 14;

/// Bir bloğun görsel ve fiziksel özellikleri.
#[derive(Clone, Copy)]
pub struct BlockDef {
    pub name: &'static str,
    pub top: u8,
    pub bottom: u8,
    pub side: u8,
    pub transparent: bool,
    pub liquid: bool,
    pub tint: [f32; 3],
    pub vary: bool,
}

const fn def(
    name: &'static str,
    top: u8,
    bottom: u8,
    side: u8,
    transparent: bool,
    tint: [f32; 3],
    vary: bool,
) -> BlockDef {
    BlockDef { name, top, bottom, side, transparent, liquid: false, tint, vary }
}

/// Sıvı bloklar ayrı bir mesh'e gider ve çarpışmaya girmez.
const fn liquid_def(
    name: &'static str,
    top: u8,
    bottom: u8,
    side: u8,
    tint: [f32; 3],
) -> BlockDef {
    BlockDef { name, top, bottom, side, transparent: true, liquid: true, tint, vary: false }
}

static BLOCK_DEFS: [BlockDef; 14] = [
    def("Hava", T_GRASS_TOP, T_DIRT, T_GRASS_SIDE, true, [1.0, 1.0, 1.0], false),
    def("Çim Bloğu", T_GRASS_TOP, T_DIRT, T_GRASS_SIDE, false, [1.0, 1.0, 1.0], true),
    def("Toprak", T_DIRT, T_DIRT, T_DIRT, false, [1.0, 1.0, 1.0], false),
    def("Taş", T_STONE, T_STONE, T_STONE, false, [1.0, 1.0, 1.0], false),
    def("Parke Taşı", T_COBBLE, T_COBBLE, T_COBBLE, false, [1.0, 1.0, 1.0], false),
    def("Kum", T_SAND, T_SAND, T_SAND, false, [1.0, 1.0, 1.0], false),
    def("Kütük", T_LOG_TOP, T_LOG_TOP, T_LOG_SIDE, false, [1.0, 1.0, 1.0], false),
    def("Yaprak", T_LEAVES, T_LEAVES, T_LEAVES, false, [1.0, 1.0, 1.0], true),
    def("Tahta", T_PLANKS, T_PLANKS, T_PLANKS, false, [1.0, 1.0, 1.0], false),
    def("Tuğla", T_BRICK, T_BRICK, T_BRICK, false, [1.0, 1.0, 1.0], false),
    def("Cam", T_GLASS, T_GLASS, T_GLASS, true, [1.0, 1.0, 1.0], false),
    liquid_def("Su", T_WATER, T_WATER, T_WATER, [1.0, 1.0, 1.0]),
    def("Ana Kaya", T_STONE, T_STONE, T_STONE, false, [0.42, 0.42, 0.45], false),
    def("Çalışma Tezgahı", T_CRAFT_TOP, T_PLANKS, T_CRAFT_SIDE, false, [1.0, 1.0, 1.0], false),
];

/// Blok tanımını döndürür; geçersiz kimlikte `Hava` tanımı verilir.
pub fn block_def(id: u8) -> &'static BlockDef {
    if (id as usize) < BLOCK_DEFS.len() {
        &BLOCK_DEFS[id as usize]
    } else {
        &BLOCK_DEFS[0]
    }
}

/// Su ve hava dışındaki her blok katıdır (oyuncu çarpışması için).
#[inline]
pub fn is_solid(id: u8) -> bool {
    id != AIR && !block_def(id).liquid
}

#[inline]
pub fn is_liquid(id: u8) -> bool {
    block_def(id).liquid
}

/// Envanterdeki bir eşyanın ikonu nasıl çizilir.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    /// Doku atlasından alınan döşeme.
    Tile(u8),
    Cubuk,
    Elma,
}

/// Eşya tanımı (bloklar ve yalnızca eşya olanlar için ortak).
#[derive(Clone, Copy)]
pub struct ItemDef {
    pub name: &'static str,
    pub icon: Icon,
    /// Yerleştirilebilirse blok kimliği.
    pub placeable: Option<u8>,
    /// Yendiğinde kazandırdığı açlık puanı.
    pub food: i32,
}

/// Kimliğe göre eşya tanımı.
pub fn item_def(id: u16) -> Option<ItemDef> {
    if id <= CRAFT_TABLE as u16 {
        let b = block_def(id as u8);
        Some(ItemDef { name: b.name, icon: Icon::Tile(b.side), placeable: Some(id as u8), food: 0 })
    } else {
        match id {
            STICK => Some(ItemDef { name: "Çubuk", icon: Icon::Cubuk, placeable: None, food: 0 }),
            APPLE => Some(ItemDef { name: "Elma", icon: Icon::Elma, placeable: None, food: 6 }),
            _ => None,
        }
    }
}

/// Kullanıcıya gösterilen eşya adı.
pub fn item_name(id: u16) -> &'static str {
    item_def(id).map(|d| d.name).unwrap_or("?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_solid_but_water_is_not() {
        assert!(is_solid(STONE));
        assert!(is_solid(GRASS));
        assert!(!is_solid(AIR));
        assert!(!is_solid(WATER));
        assert!(is_liquid(WATER));
        assert!(!is_liquid(STONE));
        // Suyun tanımı sıvı olarak işaretlenmiş olmalı; aksi hâlde su
        // yanlışlıkla katı mesh'e girer.
        assert!(block_def(WATER).liquid, "su sıvı olarak işaretlenmemiş");
        assert!(block_def(WATER).transparent);
    }

    #[test]
    fn block_defs_cover_every_block_id() {
        for id in 0..=CRAFT_TABLE {
            assert!(!block_def(id).name.is_empty(), "blok {id} tanımsız");
        }
    }

    #[test]
    fn blocks_are_placeable_items() {
        let d = item_def(GRASS as u16).unwrap();
        assert_eq!(d.placeable, Some(GRASS));
        assert_eq!(d.icon, Icon::Tile(T_GRASS_SIDE));
    }

    #[test]
    fn stick_and_apple_are_not_placeable() {
        assert_eq!(item_def(STICK).unwrap().placeable, None);
        assert_eq!(item_def(APPLE).unwrap().placeable, None);
        assert_eq!(item_def(APPLE).unwrap().food, 6);
        assert_eq!(item_name(APPLE), "Elma");
    }

    #[test]
    fn unknown_item_has_no_def() {
        assert!(item_def(9999).is_none());
        assert_eq!(item_name(9999), "?");
    }
}
