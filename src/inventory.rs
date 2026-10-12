//! 36 yuvalı envanter: 0-8 kısayol çubuğu, 9-35 sırt çantası.

use crate::blocks::{
    item_def, APPLE, BRICK, COBBLE, CRAFT_TABLE, DIRT, GLASS, GRASS, LEAVES, LOG, PLANKS, SAND,
    STONE, WATER,
};

pub const INV_SIZE: usize = 36;
pub const HOTBAR_SIZE: usize = 9;
pub const MAX_STACK: u8 = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stack {
    pub id: u16,
    pub count: u8,
}

impl Stack {
    pub fn new(id: u16, count: u8) -> Self {
        Self { id, count }
    }
}

/// Envanterin anlık görüntüsü (mod değişiminde yedeklemek için).
pub type Slots = [Option<Stack>; INV_SIZE];

pub struct Inventory {
    pub slots: Slots,
    /// Kısayol çubuğunda seçili yuva.
    pub selected: usize,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

impl Inventory {
    pub fn new() -> Self {
        Self { slots: [None; INV_SIZE], selected: 0 }
    }

    pub fn count(&self, id: u16) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.id == id)
            .map(|s| s.count as u32)
            .sum()
    }

    pub fn is_full(&self) -> bool {
        self.slots.iter().all(|s| s.is_some())
    }

    /// Yığınları doldurur, sonra boş yuva arar. Sığmayan miktarı döndürür.
    pub fn add(&mut self, id: u16, mut count: u32) -> u32 {
        for slot in self.slots.iter_mut() {
            if count == 0 {
                break;
            }
            if let Some(s) = slot {
                if s.id == id && s.count < MAX_STACK {
                    let add = (MAX_STACK - s.count) as u32;
                    let take = add.min(count);
                    s.count += take as u8;
                    count -= take;
                }
            }
        }
        for slot in self.slots.iter_mut() {
            if count == 0 {
                break;
            }
            if slot.is_none() {
                let take = count.min(MAX_STACK as u32);
                *slot = Some(Stack::new(id, take as u8));
                count -= take;
            }
        }
        count
    }

    /// Baştan sona doğru siler (orijinal motorun davranışı).
    /// Silinemeyen miktarı döndürür.
    pub fn remove(&mut self, id: u16, mut count: u32) -> u32 {
        for slot in self.slots.iter_mut() {
            if count == 0 {
                break;
            }
            if let Some(s) = slot {
                if s.id == id {
                    let take = (s.count as u32).min(count);
                    s.count -= take as u8;
                    count -= take;
                    if s.count == 0 {
                        *slot = None;
                    }
                }
            }
        }
        count
    }

    pub fn selected_stack(&self) -> Option<Stack> {
        self.slots[self.selected]
    }

    /// Seçili yuvadan bir adet harcar.
    pub fn consume_selected(&mut self) {
        if let Some(s) = self.slots[self.selected].as_mut() {
            s.count -= 1;
            if s.count == 0 {
                self.slots[self.selected] = None;
            }
        }
    }

    pub fn select(&mut self, i: usize) {
        self.selected = i % HOTBAR_SIZE;
    }

    pub fn snapshot(&self) -> Slots {
        self.slots
    }

    pub fn restore(&mut self, slots: Slots) {
        self.slots = slots;
    }

    /// Hayatta kalma başlangıç kiti.
    pub fn give_starter_kit(&mut self) {
        for (id, n) in [
            (GRASS as u16, 16u32),
            (DIRT as u16, 16),
            (STONE as u16, 24),
            (COBBLE as u16, 12),
            (SAND as u16, 12),
            (LOG as u16, 8),
            (PLANKS as u16, 8),
            (LEAVES as u16, 8),
            (APPLE, 3), // kısayolun 9. yuvası — hemen yiyebilmek için
            (GLASS as u16, 6),
        ] {
            self.add(id, n);
        }
    }

    /// Yaratıcı mod paleti: her bloktan tam yığın.
    pub fn give_creative_palette(&mut self) {
        self.slots = [None; INV_SIZE];
        let palette: [u16; 13] = [
            GRASS as u16, DIRT as u16, STONE as u16, COBBLE as u16, SAND as u16,
            LOG as u16, LEAVES as u16, PLANKS as u16, BRICK as u16, GLASS as u16,
            CRAFT_TABLE as u16, WATER as u16, APPLE,
        ];
        for (i, id) in palette.iter().enumerate() {
            self.slots[i] = Some(Stack::new(*id, MAX_STACK));
        }
    }

    /// Yerleştirilebilir bir eşya mı?
    pub fn selected_placeable(&self) -> Option<u8> {
        let s = self.selected_stack()?;
        item_def(s.id)?.placeable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{BRICK, STICK};

    #[test]
    fn stacking_fills_partial_stacks_first() {
        let mut inv = Inventory::new();
        assert_eq!(inv.add(LOG as u16, 100), 0);
        assert_eq!(inv.count(LOG as u16), 100);
        // İlk yuva 64, ikinci yuva 36 olmalı.
        assert_eq!(inv.slots[0], Some(Stack::new(LOG as u16, 64)));
        assert_eq!(inv.slots[1], Some(Stack::new(LOG as u16, 36)));
    }

    #[test]
    fn add_returns_leftover_when_inventory_is_full() {
        let mut inv = Inventory::new();
        for i in 0..INV_SIZE {
            inv.slots[i] = Some(Stack::new(BRICK as u16, MAX_STACK));
        }
        assert_eq!(inv.add(STONE as u16, 10), 10, "dolu envanter fazlayı döndürmeli");
    }

    #[test]
    fn remove_takes_from_the_first_slot() {
        let mut inv = Inventory::new();
        inv.add(LOG as u16, 70);
        assert_eq!(inv.remove(LOG as u16, 10), 0);
        assert_eq!(inv.count(LOG as u16), 60);
        // İlk yığın 64'tü; 10 tanesi ondan düştü.
        assert_eq!(inv.slots[0], Some(Stack::new(LOG as u16, 54)));
        assert_eq!(inv.slots[1], Some(Stack::new(LOG as u16, 6)));
    }

    #[test]
    fn remove_spills_into_the_next_stack() {
        let mut inv = Inventory::new();
        inv.add(LOG as u16, 70);
        inv.remove(LOG as u16, 66);
        assert!(inv.slots[0].is_none());
        assert_eq!(inv.slots[1], Some(Stack::new(LOG as u16, 4)));
    }

    #[test]
    fn remove_reports_missing_amount() {
        let mut inv = Inventory::new();
        inv.add(STONE as u16, 5);
        assert_eq!(inv.remove(STONE as u16, 9), 4);
        assert_eq!(inv.count(STONE as u16), 0);
    }

    #[test]
    fn removing_a_stack_clears_the_slot() {
        let mut inv = Inventory::new();
        inv.add(STICK, 3);
        inv.remove(STICK, 3);
        assert!(inv.slots[0].is_none());
    }

    #[test]
    fn consume_selected_empties_slot_at_one() {
        let mut inv = Inventory::new();
        inv.add(GRASS as u16, 1);
        inv.select(0);
        inv.consume_selected();
        assert!(inv.selected_stack().is_none());
    }

    #[test]
    fn select_wraps_within_the_hotbar() {
        let mut inv = Inventory::new();
        inv.select(12);
        assert_eq!(inv.selected, 3);
        inv.select(9);
        assert_eq!(inv.selected, 0);
    }

    #[test]
    fn starter_kit_puts_apple_in_the_hotbar() {
        let mut inv = Inventory::new();
        inv.give_starter_kit();
        let apple_slot = inv
            .slots
            .iter()
            .position(|s| s.map(|s| s.id) == Some(APPLE))
            .expect("elma yok");
        assert!(apple_slot < HOTBAR_SIZE, "elma kısayolda olmalı, yuva {apple_slot}");
        assert_eq!(inv.count(APPLE), 3);
    }

    #[test]
    fn starter_kit_totals_match_the_original() {
        let mut inv = Inventory::new();
        inv.give_starter_kit();
        assert_eq!(inv.count(GRASS as u16), 16);
        assert_eq!(inv.count(STONE as u16), 24);
        assert_eq!(inv.count(GLASS as u16), 6);
    }

    #[test]
    fn creative_palette_gives_full_stacks() {
        let mut inv = Inventory::new();
        inv.give_creative_palette();
        assert_eq!(inv.slots[0], Some(Stack::new(GRASS as u16, 64)));
        assert_eq!(inv.slots.iter().flatten().count(), 13);
        for s in inv.slots.iter().flatten() {
            assert_eq!(s.count, MAX_STACK);
        }
    }

    #[test]
    fn snapshot_and_restore_round_trip() {
        let mut inv = Inventory::new();
        inv.give_starter_kit();
        let snap = inv.snapshot();
        inv.give_creative_palette();
        inv.restore(snap);
        assert_eq!(inv.count(GRASS as u16), 16, "yedekten geri yükleme başarısız");
        assert_eq!(inv.count(APPLE), 3);
    }

    #[test]
    fn only_blocks_are_placeable() {
        let mut inv = Inventory::new();
        inv.slots[0] = Some(Stack::new(GRASS as u16, 1));
        inv.slots[1] = Some(Stack::new(STICK, 1));
        inv.select(0);
        assert_eq!(inv.selected_placeable(), Some(GRASS));
        inv.select(1);
        assert_eq!(inv.selected_placeable(), None, "çubuk yerleştirilemez");
    }
}
