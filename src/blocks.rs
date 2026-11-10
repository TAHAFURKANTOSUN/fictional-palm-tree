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
/// Meşale: chunk verisinde durur ama küp olarak çizilmez (küçük bir model
/// olarak ayrıca çizilir), içinden geçilir ve çevresini aydınlatır.
pub const TORCH: u8 = 14;

/// Yalnızca eşya olan kimlikler (blok değildir).
pub const STICK: u16 = 100;
pub const APPLE: u16 = 101;
pub const FISHING_ROD: u16 = 102;
pub const FISH: u16 = 103;
pub const CAR: u16 = 104;
pub const PISTOL: u16 = 105;
pub const SNIPER: u16 = 106;

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
/// Yalnızca envanter ikonu olarak kullanılır.
pub const T_TORCH: u8 = 15;

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

const BLOCK_COUNT: usize = 15;

static BLOCK_DEFS: [BlockDef; BLOCK_COUNT] = BLOCK_TABLE;

const BLOCK_TABLE: [BlockDef; BLOCK_COUNT] = [
    def("Hava", T_GRASS_TOP, T_DIRT, T_GRASS_SIDE, true, [1.0, 1.0, 1.0], false),
    def("Çim Bloğu", T_GRASS_TOP, T_DIRT, T_GRASS_SIDE, false, [1.0, 1.0, 1.0], true),
    def("Toprak", T_DIRT, T_DIRT, T_DIRT, false, [1.0, 1.0, 1.0], false),
    def("Taş", T_STONE, T_STONE, T_STONE, false, [1.0, 1.0, 1.0], false),
    def("Parke Taşı", T_COBBLE, T_COBBLE, T_COBBLE, false, [1.0, 1.0, 1.0], false),
    def("Kum", T_SAND, T_SAND, T_SAND, false, [1.0, 1.0, 1.0], false),
    def("Kütük", T_LOG_TOP, T_LOG_TOP, T_LOG_SIDE, false, [1.0, 1.0, 1.0], false),
    // Yaprak dokusunda delikler vardır: arkası görünür, komşu yüzleri gizlemez.
    def("Yaprak", T_LEAVES, T_LEAVES, T_LEAVES, true, [1.0, 1.0, 1.0], true),
    def("Tahta", T_PLANKS, T_PLANKS, T_PLANKS, false, [1.0, 1.0, 1.0], false),
    def("Tuğla", T_BRICK, T_BRICK, T_BRICK, false, [1.0, 1.0, 1.0], false),
    def("Cam", T_GLASS, T_GLASS, T_GLASS, true, [1.0, 1.0, 1.0], false),
    liquid_def("Su", T_WATER, T_WATER, T_WATER, [1.0, 1.0, 1.0]),
    def("Ana Kaya", T_STONE, T_STONE, T_STONE, false, [0.42, 0.42, 0.45], false),
    def("Çalışma Tezgahı", T_CRAFT_TOP, T_PLANKS, T_CRAFT_SIDE, false, [1.0, 1.0, 1.0], false),
    def("Meşale", T_TORCH, T_TORCH, T_TORCH, true, [1.0, 1.0, 1.0], false),
];

/// Blok tanımını döndürür; geçersiz kimlikte `Hava` tanımı verilir.
pub fn block_def(id: u8) -> &'static BlockDef {
    if (id as usize) < BLOCK_DEFS.len() {
        &BLOCK_DEFS[id as usize]
    } else {
        &BLOCK_DEFS[0]
    }
}

/// `OPAQUE[id]`: blok komşu yüzleri tamamen gizliyor mu? Derleme zamanında
/// `BLOCK_TABLE`'dan üretilir; tanımsız kimlikler (ve hava) opak değildir.
static OPAQUE: [bool; 256] = {
    let mut table = [false; 256];
    let mut id = 1; // 0 = hava
    while id < BLOCK_COUNT {
        table[id] = !BLOCK_TABLE[id].transparent;
        id += 1;
    }
    table
};

/// Komşusunun yüzünü tamamen gizleyen (opak) blok mu?
///
/// Mesh üretiminin en sıcak sorgusudur (yüz görünürlüğü + köşe başına üç
/// ambient occlusion örneği), bu yüzden dallanmasız bir tablodan okunur.
#[inline]
pub fn is_opaque(id: u8) -> bool {
    OPAQUE[id as usize]
}

/// Su ve hava dışındaki her blok katıdır (oyuncu çarpışması için).
#[inline]
pub fn is_solid(id: u8) -> bool {
    id != AIR && id != TORCH && !block_def(id).liquid
}

/// Chunk mesh'ine küp olarak giren blok mu? (Meşale ayrı bir modelle çizilir.)
#[inline]
pub fn is_cube(id: u8) -> bool {
    id != AIR && id != TORCH
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
    Olta,
    Balik,
    Araba,
    Tabanca,
    Tufek,
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
    if (id as usize) < BLOCK_COUNT {
        let b = block_def(id as u8);
        Some(ItemDef { name: b.name, icon: Icon::Tile(b.side), placeable: Some(id as u8), food: 0 })
    } else {
        match id {
            STICK => Some(ItemDef { name: "Çubuk", icon: Icon::Cubuk, placeable: None, food: 0 }),
            APPLE => Some(ItemDef { name: "Elma", icon: Icon::Elma, placeable: None, food: 6 }),
            FISHING_ROD => Some(ItemDef { name: "Olta", icon: Icon::Olta, placeable: None, food: 0 }),
            FISH => Some(ItemDef { name: "Balık", icon: Icon::Balik, placeable: None, food: 5 }),
            CAR => Some(ItemDef { name: "Araba", icon: Icon::Araba, placeable: None, food: 0 }),
            PISTOL => Some(ItemDef { name: "Tabanca", icon: Icon::Tabanca, placeable: None, food: 0 }),
            SNIPER => Some(ItemDef { name: "Dürbünlü Tüfek", icon: Icon::Tufek, placeable: None, food: 0 }),
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
    fn opacity_table_matches_the_block_definitions() {
        assert!(!is_opaque(AIR), "hava opak olamaz");
        for id in 1..=CRAFT_TABLE {
            assert_eq!(
                is_opaque(id),
                !block_def(id).transparent,
                "blok {id} için opaklık tablosu tanımla uyuşmuyor"
            );
        }
        assert!(!is_opaque(WATER) && !is_opaque(GLASS));
        assert!(!is_opaque(LEAVES), "yaprak delikli: arkasındaki yüzler çizilmeli");
        // Tanımsız kimlikler hava gibi davranır.
        assert!(!is_opaque(200));
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
    fn torch_is_a_walk_through_block_that_is_not_meshed() {
        assert!(!is_solid(TORCH), "meşalenin içinden geçilebilmeli");
        assert!(!is_opaque(TORCH) && !is_cube(TORCH));
        assert!(is_cube(STONE) && !is_cube(AIR));
        assert_eq!(item_def(TORCH as u16).unwrap().placeable, Some(TORCH));
        assert_eq!(item_name(TORCH as u16), "Meşale");
    }

    #[test]
    fn fish_is_food_and_tools_are_not_placeable() {
        assert_eq!(item_def(FISH).unwrap().food, 5);
        for id in [FISHING_ROD, FISH, CAR] {
            assert_eq!(item_def(id).unwrap().placeable, None);
        }
    }

    #[test]
    fn unknown_item_has_no_def() {
        assert!(item_def(9999).is_none());
        assert_eq!(item_name(9999), "?");
    }
}
