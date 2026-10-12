//! Çalışma tezgahı: 3×3 ızgara, şekilli ve şekilsiz tarifler.
//!
//! Şekilli tarifler **konumdan bağımsızdır**: malzeme ızgaranın neresine
//! konursa konsun, dolu hücrelerin sınırlayıcı kutusu desenle karşılaştırılır.

use crate::blocks::{CRAFT_TABLE, DIRT, GRASS, LEAVES, LOG, PLANKS, SAND, STICK, STONE};
use crate::inventory::{Inventory, Stack};

pub type Grid = [Option<Stack>; 9];

pub enum RecipeKind {
    /// `pattern` satırları, `key` harf → eşya kimliği eşlemesi.
    Shaped { pattern: &'static [&'static str], key: &'static [(char, u16)] },
    /// Sırasız: yalnızca içerik kümesi önemli.
    Shapeless { inputs: &'static [u16] },
}

pub struct Recipe {
    pub id: &'static str,
    pub kind: RecipeKind,
    pub out: (u16, u8),
}

pub static RECIPES: [Recipe; 6] = [
    Recipe {
        id: "planks",
        kind: RecipeKind::Shapeless { inputs: &[LOG as u16] },
        out: (PLANKS as u16, 4),
    },
    Recipe {
        id: "stick",
        kind: RecipeKind::Shaped { pattern: &["P", "P"], key: &[('P', PLANKS as u16)] },
        out: (STICK, 4),
    },
    Recipe {
        id: "table",
        kind: RecipeKind::Shaped { pattern: &["PP", "PP"], key: &[('P', PLANKS as u16)] },
        out: (CRAFT_TABLE as u16, 1),
    },
    Recipe {
        id: "glass",
        kind: RecipeKind::Shapeless { inputs: &[SAND as u16, PLANKS as u16] },
        out: (crate::blocks::GLASS as u16, 2),
    },
    Recipe {
        id: "brick",
        kind: RecipeKind::Shaped { pattern: &["SS", "SS"], key: &[('S', STONE as u16)] },
        out: (crate::blocks::BRICK as u16, 4),
    },
    Recipe {
        id: "grass",
        kind: RecipeKind::Shapeless { inputs: &[DIRT as u16, LEAVES as u16] },
        out: (GRASS as u16, 1),
    },
];

/// Tarifin ihtiyaç duyduğu eşyalar ve adetleri.
pub fn recipe_needs(rec: &Recipe) -> Vec<(u16, u32)> {
    let mut need: Vec<(u16, u32)> = Vec::new();
    let mut bump = |id: u16| {
        match need.iter_mut().find(|(i, _)| *i == id) {
            Some((_, n)) => *n += 1,
            None => need.push((id, 1)),
        }
    };
    match &rec.kind {
        RecipeKind::Shaped { pattern, key } => {
            for row in pattern.iter() {
                for ch in row.chars() {
                    if let Some((_, id)) = key.iter().find(|(c, _)| *c == ch) {
                        bump(*id);
                    }
                }
            }
        }
        RecipeKind::Shapeless { inputs } => {
            for id in inputs.iter() {
                bump(*id);
            }
        }
    }
    need
}

pub fn can_craft(inv: &Inventory, rec: &Recipe) -> bool {
    recipe_needs(rec)
        .iter()
        .all(|(id, n)| inv.count(*id) >= *n)
}

/// Dolu hücrelerin sınırlayıcı kutusu — ızgaradaki konumdan bağımsız eşleştirme.
struct Bounds {
    w: usize,
    h: usize,
    cells: Vec<Vec<u16>>,
}

fn grid_bounds(grid: &Grid) -> Option<Bounds> {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (3i32, -1i32, 3i32, -1i32);
    for y in 0..3i32 {
        for x in 0..3i32 {
            if grid[(y * 3 + x) as usize].is_some() {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    if max_x < 0 {
        return None;
    }
    let w = (max_x - min_x + 1) as usize;
    let h = (max_y - min_y + 1) as usize;
    let mut cells = Vec::with_capacity(h);
    for y in 0..h {
        let mut row = Vec::with_capacity(w);
        for x in 0..w {
            let idx = ((min_y as usize + y) * 3 + (min_x as usize + x)) as usize;
            row.push(grid[idx].map(|s| s.id).unwrap_or(0));
        }
        cells.push(row);
    }
    Some(Bounds { w, h, cells })
}

fn pattern_bounds(pattern: &[&str], key: &[(char, u16)]) -> Bounds {
    let h = pattern.len();
    let w = pattern.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    let mut cells = Vec::with_capacity(h);
    for row in pattern {
        let mut out = Vec::with_capacity(w);
        for x in 0..w {
            let id = row
                .chars()
                .nth(x)
                .and_then(|ch| key.iter().find(|(c, _)| *c == ch))
                .map(|(_, id)| *id)
                .unwrap_or(0);
            out.push(id);
        }
        cells.push(out);
    }
    Bounds { w, h, cells }
}

fn shaped_match(grid: &Grid, pattern: &[&str], key: &[(char, u16)]) -> bool {
    let Some(bb) = grid_bounds(grid) else {
        return false;
    };
    let pc = pattern_bounds(pattern, key);
    if bb.w != pc.w || bb.h != pc.h {
        return false;
    }
    for y in 0..pc.h {
        for x in 0..pc.w {
            if bb.cells[y][x] != pc.cells[y][x] {
                return false;
            }
        }
    }
    true
}

fn shapeless_match(grid: &Grid, inputs: &[u16]) -> bool {
    let mut cells: Vec<u16> = grid.iter().flatten().map(|s| s.id).collect();
    if cells.len() != inputs.len() {
        return false;
    }
    for id in inputs {
        match cells.iter().position(|c| c == id) {
            Some(p) => {
                cells.swap_remove(p);
            }
            None => return false,
        }
    }
    cells.is_empty()
}

/// Izgaraya uyan ilk tarifi döndürür.
pub fn find_recipe(grid: &Grid) -> Option<&'static Recipe> {
    RECIPES.iter().find(|rec| match &rec.kind {
        RecipeKind::Shaped { pattern, key } => shaped_match(grid, pattern, key),
        RecipeKind::Shapeless { inputs } => shapeless_match(grid, inputs),
    })
}

pub fn craft_output(grid: &Grid) -> Option<(u16, u8)> {
    find_recipe(grid).map(|r| r.out)
}

/// Izgaradaki malzemeleri envantere geri koyar.
pub fn clear_grid(grid: &mut Grid, inv: &mut Inventory) {
    for slot in grid.iter_mut() {
        if let Some(s) = *slot {
            let left = inv.add(s.id, s.count as u32);
            *slot = if left > 0 {
                Some(Stack::new(s.id, left as u8))
            } else {
                None
            };
        }
    }
}

/// Tarifi envanterden ızgaraya otomatik yerleştirir.
/// Yetersiz malzemede `Err(eksik eşya kimliği)` döner.
pub fn auto_fill(grid: &mut Grid, inv: &mut Inventory, rec: &Recipe) -> Result<(), u16> {
    clear_grid(grid, inv);

    for (id, n) in recipe_needs(rec) {
        if inv.count(id) < n {
            return Err(id);
        }
    }

    match &rec.kind {
        RecipeKind::Shaped { pattern, key } => {
            for (y, row) in pattern.iter().enumerate() {
                for (x, ch) in row.chars().enumerate() {
                    if let Some((_, id)) = key.iter().find(|(c, _)| *c == ch) {
                        inv.remove(*id, 1);
                        grid[y * 3 + x] = Some(Stack::new(*id, 1));
                    }
                }
            }
        }
        RecipeKind::Shapeless { inputs } => {
            let mut k = 0usize;
            for id in inputs.iter() {
                inv.remove(*id, 1);
                while k < 9 && grid[k].is_some() {
                    k += 1;
                }
                if k < 9 {
                    grid[k] = Some(Stack::new(*id, 1));
                    k += 1;
                }
            }
        }
    }
    Ok(())
}

/// Sonuç yuvasından bir üretim alır; ızgaradaki her malzemeden bir adet eksiltir.
pub fn take_result(grid: &mut Grid) -> Option<(u16, u8)> {
    let out = craft_output(grid)?;
    for slot in grid.iter_mut() {
        if let Some(s) = slot {
            s.count -= 1;
            if s.count == 0 {
                *slot = None;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{AIR, BRICK, GLASS};

    fn empty() -> Grid {
        [None; 9]
    }

    fn put(grid: &mut Grid, i: usize, id: u16) {
        grid[i] = Some(Stack::new(id, 1));
    }

    #[test]
    fn one_log_makes_four_planks() {
        let mut g = empty();
        put(&mut g, 4, LOG as u16);
        assert_eq!(craft_output(&g), Some((PLANKS as u16, 4)));
    }

    #[test]
    fn vertical_planks_make_sticks_at_any_offset() {
        let mut g = empty();
        put(&mut g, 0, PLANKS as u16);
        put(&mut g, 3, PLANKS as u16);
        assert_eq!(find_recipe(&g).map(|r| r.id), Some("stick"));

        let mut g = empty();
        put(&mut g, 2, PLANKS as u16);
        put(&mut g, 5, PLANKS as u16);
        assert_eq!(find_recipe(&g).map(|r| r.id), Some("stick"), "sağ sütun da çalışmalı");
    }

    #[test]
    fn horizontal_planks_do_not_make_sticks() {
        let mut g = empty();
        put(&mut g, 1, PLANKS as u16);
        put(&mut g, 2, PLANKS as u16);
        assert!(find_recipe(&g).is_none(), "yatay tahta çubuk vermemeli");
    }

    #[test]
    fn two_by_two_planks_make_a_crafting_table_in_any_corner() {
        for (a, b, c, d) in [(0, 1, 3, 4), (4, 5, 7, 8), (1, 2, 4, 5)] {
            let mut g = empty();
            for i in [a, b, c, d] {
                put(&mut g, i, PLANKS as u16);
            }
            assert_eq!(
                find_recipe(&g).map(|r| r.id),
                Some("table"),
                "köşe {a},{b},{c},{d} çalışmadı"
            );
        }
    }

    #[test]
    fn two_by_two_stone_makes_bricks() {
        let mut g = empty();
        for i in [3, 4, 6, 7] {
            put(&mut g, i, STONE as u16);
        }
        assert_eq!(craft_output(&g), Some((BRICK as u16, 4)));
    }

    #[test]
    fn shapeless_recipes_ignore_position() {
        let mut g = empty();
        put(&mut g, 0, SAND as u16);
        put(&mut g, 8, PLANKS as u16);
        assert_eq!(craft_output(&g), Some((GLASS as u16, 2)));

        let mut g = empty();
        put(&mut g, 4, PLANKS as u16);
        put(&mut g, 2, SAND as u16);
        assert_eq!(craft_output(&g), Some((GLASS as u16, 2)));
    }

    #[test]
    fn wrong_or_extra_ingredients_do_not_match() {
        let mut g = empty();
        put(&mut g, 0, SAND as u16);
        put(&mut g, 8, SAND as u16);
        assert!(find_recipe(&g).is_none(), "iki kum cam vermemeli");

        let mut g = empty();
        put(&mut g, 0, SAND as u16);
        put(&mut g, 1, PLANKS as u16);
        put(&mut g, 2, PLANKS as u16);
        assert!(find_recipe(&g).is_none(), "fazla malzeme eşleşmemeli");
    }

    #[test]
    fn empty_grid_has_no_output() {
        assert!(craft_output(&empty()).is_none());
    }

    #[test]
    fn auto_fill_consumes_from_the_inventory() {
        let mut inv = Inventory::new();
        inv.add(LOG as u16, 3);
        let mut g = empty();
        assert!(auto_fill(&mut g, &mut inv, &RECIPES[0]).is_ok());
        assert_eq!(inv.count(LOG as u16), 2);
        assert_eq!(craft_output(&g), Some((PLANKS as u16, 4)));
    }

    #[test]
    fn auto_fill_reports_the_missing_ingredient() {
        let mut inv = Inventory::new();
        let mut g = empty();
        // Tezgah için 4 tahta gerekir, envanter boş.
        assert_eq!(auto_fill(&mut g, &mut inv, &RECIPES[2]), Err(PLANKS as u16));
    }

    #[test]
    fn take_result_consumes_one_of_each_ingredient() {
        let mut g = empty();
        put(&mut g, 0, PLANKS as u16);
        put(&mut g, 3, PLANKS as u16);
        assert_eq!(take_result(&mut g), Some((STICK, 4)));
        assert!(g.iter().all(|s| s.is_none()), "malzemeler tükenmeliydi");
    }

    #[test]
    fn clear_grid_returns_everything_to_the_inventory() {
        let mut inv = Inventory::new();
        let mut g = empty();
        put(&mut g, 0, PLANKS as u16);
        put(&mut g, 4, STONE as u16);
        clear_grid(&mut g, &mut inv);
        assert!(g.iter().all(|s| s.is_none()));
        assert_eq!(inv.count(PLANKS as u16), 1);
        assert_eq!(inv.count(STONE as u16), 1);
    }

    #[test]
    fn full_chain_log_to_table_works() {
        let mut inv = Inventory::new();
        let mut g = empty();

        inv.add(LOG as u16, 1);
        auto_fill(&mut g, &mut inv, &RECIPES[0]).unwrap();
        let (id, n) = take_result(&mut g).unwrap();
        inv.add(id, n as u32); // 4 tahta

        auto_fill(&mut g, &mut inv, &RECIPES[1]).unwrap();
        let (id, n) = take_result(&mut g).unwrap();
        assert_eq!(id, STICK);
        inv.add(id, n as u32); // 4 çubuk

        inv.add(PLANKS as u16, 4);
        assert!(auto_fill(&mut g, &mut inv, &RECIPES[2]).is_ok());
        let (id, n) = take_result(&mut g).unwrap();
        assert_eq!(id, CRAFT_TABLE as u16);
        inv.add(id, n as u32);

        assert_eq!(inv.count(CRAFT_TABLE as u16), 1, "tezgah üretilemedi");
        assert_eq!(inv.count(STICK), 4);
        assert_eq!(inv.count(LOG as u16), 0);
        let _ = AIR;
    }

    #[test]
    fn every_recipe_has_a_distinct_id() {
        let mut ids: Vec<&str> = RECIPES.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), RECIPES.len());
    }

    #[test]
    fn recipe_needs_counts_multiples() {
        let needs = recipe_needs(&RECIPES[2]); // 2x2 tahta
        assert_eq!(needs, vec![(PLANKS as u16, 4)]);
    }
}
