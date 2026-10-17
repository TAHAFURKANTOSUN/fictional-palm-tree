//! Chunk mesh üretimi: görünen yüzler + ambient occlusion.
//!
//! Yüz başına 4 köşe üretilir; komşu veriler 3×3 tabloya önceden alınır, böylece
//! mesh sırasında hiç `HashMap` araması yapılmaz.

use crate::blocks::*;
use crate::noise::hash3f;
use crate::world::{ckey, World, CHUNK_AREA, CHUNK_SIZE, SEA_LEVEL, WORLD_HEIGHT};

/// Doku atlası döşeme boyutu ve sütun sayısı.
pub const TILE: i32 = 16;
pub const ATLAS_COLS: i32 = 16;

/// Doku kenarında komşu döşemeye taşmayı önleyen yarım piksel payı.
const EPS_V: f32 = 0.5 / TILE as f32;

const AO_LEVELS: [f32; 4] = [0.52, 0.70, 0.86, 1.0];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    Top,
    Bottom,
    Side,
}

pub struct Face {
    pub n: [i32; 3],
    /// Köşe bileşenlerinin hangi eksenlerde olduğunu belirtir (AO komşuları için).
    pub b: usize,
    pub c: usize,
    pub shade: f32,
    pub tile: TileKind,
    pub verts: [[f32; 3]; 4],
}

pub const FACES: [Face; 6] = [
    Face {
        n: [1, 0, 0], b: 1, c: 2, shade: 0.76, tile: TileKind::Side,
        verts: [[1.0, 0.0, 1.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0]],
    },
    Face {
        n: [-1, 0, 0], b: 1, c: 2, shade: 0.76, tile: TileKind::Side,
        verts: [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [0.0, 1.0, 0.0]],
    },
    Face {
        n: [0, 1, 0], b: 0, c: 2, shade: 1.00, tile: TileKind::Top,
        verts: [[0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
    },
    Face {
        n: [0, -1, 0], b: 0, c: 2, shade: 0.48, tile: TileKind::Bottom,
        verts: [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
    },
    Face {
        n: [0, 0, 1], b: 0, c: 1, shade: 0.62, tile: TileKind::Side,
        verts: [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]],
    },
    Face {
        n: [0, 0, -1], b: 0, c: 1, shade: 0.62, tile: TileKind::Side,
        verts: [[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
    },
];

/// Bir chunk'ın katı ve sıvı geometrisi.
#[derive(Default)]
pub struct MeshData {
    pub pos: Vec<f32>,
    pub nor: Vec<f32>,
    pub uv: Vec<f32>,
    pub col: Vec<f32>,
    pub idx: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }

    pub fn face_count(&self) -> u32 {
        (self.idx.len() / 6) as u32
    }

    /// Tamponları temizler — havuzdan yeniden kullanılırken ayırma yapılmaz.
    pub fn reset(&mut self) {
        self.pos.clear();
        self.nor.clear();
        self.uv.clear();
        self.col.clear();
        self.idx.clear();
    }

    /// Vertex konumlarının kapsayıcı küresini (merkez, yarıçap) döndürür.
    /// Görüş konisi (frustum) elemesi için kullanılır.
    pub fn bounding_sphere(&self) -> ([f32; 3], f32) {
        if self.pos.is_empty() {
            return ([0.0; 3], 0.0);
        }
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for p in self.pos.chunks_exact(3) {
            for a in 0..3 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
        let center = [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ];
        let r = ((max[0] - center[0]).powi(2)
            + (max[1] - center[1]).powi(2)
            + (max[2] - center[2]).powi(2))
        .sqrt();
        (center, r)
    }
}

#[inline]
fn occludes(id: u8) -> bool {
    id != AIR && !block_def(id).transparent
}

fn push_face(
    bk: &mut MeshData,
    occl: &impl Fn(i32, i32, i32) -> bool,
    wx: i32,
    wy: i32,
    wz: i32,
    def: &BlockDef,
    face: &Face,
) {
    let tile_index = match face.tile {
        TileKind::Top => def.top,
        TileKind::Bottom => def.bottom,
        TileKind::Side => def.side,
    };

    let atlas_w = (TILE * ATLAS_COLS) as f32;
    let u0 = (tile_index as f32 * TILE as f32 + 0.5) / atlas_w;
    let u1 = ((tile_index as f32 + 1.0) * TILE as f32 - 0.5) / atlas_w;
    let v0 = EPS_V;
    let v1 = 1.0 - EPS_V;

    let tint = def.tint;
    let (mut vr, mut vg, mut vb) = (1.0f32, 1.0f32, 1.0f32);
    if def.vary {
        let m = 0.90 + hash3f(wx, wy, wz) as f32 * 0.20;
        vr = m;
        vg = m;
        vb = m;
    }

    let base = (bk.pos.len() / 3) as u32;
    let mut ao = [0usize; 4];

    for (i, p) in face.verts.iter().enumerate() {
        bk.pos.push(wx as f32 + p[0]);
        bk.pos.push(wy as f32 + p[1]);
        bk.pos.push(wz as f32 + p[2]);
        bk.nor.push(face.n[0] as f32);
        bk.nor.push(face.n[1] as f32);
        bk.nor.push(face.n[2] as f32);

        let (u, v) = match i {
            0 => (u0, v0),
            1 => (u1, v0),
            2 => (u1, v1),
            _ => (u0, v1),
        };
        bk.uv.push(u);
        bk.uv.push(v);

        let ob: i32 = if p[face.b] == 1.0 { 1 } else { -1 };
        let oc: i32 = if p[face.c] == 1.0 { 1 } else { -1 };

        let bx = wx + face.n[0];
        let by = wy + face.n[1];
        let bz = wz + face.n[2];

        let s1 = occl(
            bx + if face.b == 0 { ob } else { 0 },
            by + if face.b == 1 { ob } else { 0 },
            bz + if face.b == 2 { ob } else { 0 },
        );
        let s2 = occl(
            bx + if face.c == 0 { oc } else { 0 },
            by + if face.c == 1 { oc } else { 0 },
            bz + if face.c == 2 { oc } else { 0 },
        );
        let sc = occl(
            bx + if face.b == 0 { ob } else { 0 } + if face.c == 0 { oc } else { 0 },
            by + if face.b == 1 { ob } else { 0 } + if face.c == 1 { oc } else { 0 },
            bz + if face.b == 2 { ob } else { 0 } + if face.c == 2 { oc } else { 0 },
        );

        let level = if s1 && s2 {
            0
        } else {
            3 - (s1 as usize + s2 as usize + sc as usize)
        };
        ao[i] = level;

        let f = face.shade * AO_LEVELS[level];
        bk.col.push((tint[0] * vr * f).clamp(0.0, 1.0));
        bk.col.push((tint[1] * vg * f).clamp(0.0, 1.0));
        bk.col.push((tint[2] * vb * f).clamp(0.0, 1.0));
    }

    // Köşegen yönü AO'ya göre seçilir; aksi halde gölgeleme "yamalı" görünür.
    if ao[0] + ao[2] > ao[1] + ao[3] {
        bk.idx.extend_from_slice(&[base + 1, base + 2, base + 3, base + 1, base + 3, base]);
    } else {
        bk.idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Bir chunk için katı ve sıvı mesh üretir.
///
/// 8 komşusunun verisi hazır değilse `None` döner (sınırda hayalet yüz oluşmasın).
pub fn build_chunk_mesh(world: &World, cx: i32, cz: i32) -> Option<(MeshData, MeshData)> {
    let ch = world.chunks.get(&ckey(cx, cz))?;

    let mut nbs: [&[u8]; 9] = [&[]; 9];
    for dz in -1..=1 {
        for dx in -1..=1 {
            let n = world.chunks.get(&ckey(cx + dx, cz + dz))?;
            nbs[((dx + 1) + 3 * (dz + 1)) as usize] = &n.data;
        }
    }

    let x0 = cx * CHUNK_SIZE;
    let z0 = cz * CHUNK_SIZE;

    // Yerel okuma: x,z ∈ [-1,16], y ∈ [0,63].
    let get = |x: i32, y: i32, z: i32| -> u8 {
        if y < 0 {
            return STONE;
        }
        if y >= WORLD_HEIGHT {
            return AIR;
        }
        let d = nbs[(((x >> 4) + 1) + 3 * ((z >> 4) + 1)) as usize];
        d[(x & 15) as usize + CHUNK_SIZE as usize * ((z & 15) as usize + CHUNK_SIZE as usize * y as usize)]
    };

    let occl = |wx: i32, wy: i32, wz: i32| -> bool { occludes(get(wx - x0, wy, wz - z0)) };

    // Mesh'in y aralığını daralt: yeraltı bloklarının hiçbir yüzü görünmez.
    // Komşu sütun yükseklikleri de hesaba katılır (chunk sınırı duvarları).
    let mut min_h = i32::MAX;
    let mut max_h = i32::MIN;
    for z in -1..=CHUNK_SIZE {
        for x in -1..=CHUNK_SIZE {
            let h = world.height_at(x0 + x, z0 + z);
            min_h = min_h.min(h);
            max_h = max_h.max(h);
        }
    }
    // Üst sınır **arazi yüksekliği değil**, chunk'ın gerçek en yüksek dolu
    // bloğudur: ağaç yaprakları zeminin 6–8 blok üstüne çıkar. Yalnızca arazi
    // yüksekliğine bakılırsa yaprak tepeleri mesh dışında kalır ve görünmez
    // (ama katı) bloklar oluşur.
    let y_top = max_h.max(SEA_LEVEL).max(ch.content_top);
    let y0 = (min_h - 1).max(0);
    let y1 = (y_top + 2).min(WORLD_HEIGHT - 1);

    let mut solid = MeshData::default();
    let mut liquid = MeshData::default();

    for y in y0..=y1 {
        let y_base = CHUNK_AREA * y as usize;
        for z in 0..CHUNK_SIZE {
            let row_base = y_base + CHUNK_SIZE as usize * z as usize;
            for x in 0..CHUNK_SIZE {
                let id = ch.data[row_base + x as usize];
                if id == AIR {
                    continue;
                }
                let def = block_def(id);
                let bk = if def.liquid { &mut liquid } else { &mut solid };

                for face in FACES.iter() {
                    let nb = get(x + face.n[0], y + face.n[1], z + face.n[2]);
                    let visible = if nb == AIR {
                        true
                    } else {
                        block_def(nb).transparent && nb != id
                    };
                    if visible {
                        push_face(bk, &occl, x0 + x, y, z0 + z, def, face);
                    }
                }
            }
        }
    }

    Some((solid, liquid))
}

/// Testlerin ihtiyaç duyduğu, komşuları hazır küçük dünya.
/// (-2..=3 aralığı üretilir; böylece -1..=2 arası her chunk'ın 8 komşusu hazırdır.)
#[cfg(test)]
pub(crate) fn test_world(seed: i32) -> World {
    let mut w = World::new(seed);
    for cz in -2..=3 {
        for cx in -2..=3 {
            w.generate_chunk(cx, cz);
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_has_balanced_attributes() {
        let mut w = test_world(5);
        w.generate_chunk(0, 0);
        let (solid, liquid) = build_chunk_mesh(&w, 0, 0).expect("mesh üretilemedi");
        assert_eq!(solid.pos.len() % 3, 0);
        assert_eq!(solid.nor.len(), solid.pos.len());
        assert_eq!(solid.uv.len() / 2, solid.pos.len() / 3);
        assert_eq!(solid.col.len(), solid.pos.len());
        assert_eq!(solid.idx.len() % 6, 0, "her yüz 2 üçgen olmalı");
        assert!(!solid.is_empty(), "yüzey chunk'ı boş mesh üretmemeli");
        let _ = liquid;
    }

    #[test]
    fn every_index_is_in_range() {
        let mut w = test_world(9);
        w.generate_chunk(0, 0);
        let (solid, liquid) = build_chunk_mesh(&w, 0, 0).unwrap();
        for (name, m) in [("solid", &solid), ("liquid", &liquid)] {
            let verts = (m.pos.len() / 3) as u32;
            for &i in &m.idx {
                assert!(i < verts, "{name}: indeks {i} tepe sayısı {verts} dışında");
            }
        }
    }

    #[test]
    fn face_count_matches_index_count() {
        let mut w = test_world(11);
        w.generate_chunk(0, 0);
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        assert_eq!(solid.face_count() as usize * 6, solid.idx.len());
    }

    #[test]
    fn mesh_requires_all_eight_neighbours() {
        let mut w = World::new(3);
        w.generate_chunk(0, 0); // yalnızca kendisi
        assert!(
            build_chunk_mesh(&w, 0, 0).is_none(),
            "komşular yokken mesh üretilmemeli"
        );
    }

    #[test]
    fn buried_blocks_produce_no_faces() {
        // Tamamen taşla çevrili bir blok hiç yüz üretmemeli.
        let mut w = test_world(21);
        w.generate_chunk(0, 0);
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        // Yüzeyin çok altındaki yüzler mesh'te olmamalı (y aralığı daraltıldı).
        let mut min_y = f32::MAX;
        for p in solid.pos.chunks_exact(3) {
            min_y = min_y.min(p[1]);
        }
        let h_min = (-1..=CHUNK_SIZE)
            .flat_map(|x| (-1..=CHUNK_SIZE).map(move |z| (x, z)))
            .map(|(x, z)| w.height_at(x, z))
            .min()
            .unwrap();
        assert!(
            min_y >= (h_min - 2) as f32,
            "y aralığı daraltılmadı: en düşük yüz {min_y}, min yükseklik {h_min}"
        );
    }

    #[test]
    fn mesh_is_deterministic() {
        let mut a = test_world(77);
        let mut b = test_world(77);
        a.generate_chunk(2, 2);
        b.generate_chunk(2, 2);
        let (ma, _) = build_chunk_mesh(&a, 2, 2).unwrap();
        let (mb, _) = build_chunk_mesh(&b, 2, 2).unwrap();
        assert_eq!(ma.pos, mb.pos);
        assert_eq!(ma.idx, mb.idx);
        assert_eq!(ma.col, mb.col);
    }

    #[test]
    fn bounding_sphere_covers_geometry() {
        let mut w = test_world(31);
        w.generate_chunk(0, 0);
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        let (c, r) = solid.bounding_sphere();
        for p in solid.pos.chunks_exact(3) {
            let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt();
            assert!(d <= r + 1e-3, "kapsayıcı küre geometriyi kapsamıyor");
        }
    }

    /// H1 regresyonu: ağaç yaprakları mesh'in `y` aralığının üstünde kalmamalı.
    /// Chunk'taki en yüksek dolu bloğun üst yüzü mutlaka mesh'e girmelidir.
    #[test]
    fn mesh_covers_the_highest_block_of_every_chunk() {
        let w = test_world(20261012);
        let mut checked = 0;
        for cz in -1..=2 {
            for cx in -1..=2 {
                let ch = w.chunks.get(&ckey(cx, cz)).expect("chunk yok");

                // Chunk'taki gerçek en yüksek dolu blok ve ağaç var mı?
                let mut top = 0;
                let mut has_leaves = false;
                for y in 0..WORLD_HEIGHT {
                    let base = CHUNK_AREA * y as usize;
                    for &b in &ch.data[base..base + CHUNK_AREA] {
                        if b != AIR {
                            top = y;
                        }
                        if b == LEAVES {
                            has_leaves = true;
                        }
                    }
                }
                if !has_leaves {
                    continue; // ağaçsız chunk'ta eski formül de doğru çalışıyordu
                }

                let (solid, liquid) = build_chunk_mesh(&w, cx, cz).expect("mesh üretilemedi");
                let max_y = solid
                    .pos
                    .chunks_exact(3)
                    .chain(liquid.pos.chunks_exact(3))
                    .map(|p| p[1])
                    .fold(f32::MIN, f32::max);

                assert!(
                    max_y >= (top + 1) as f32 - 1e-3,
                    "({cx},{cz}) ağaç tepesi mesh dışında: en yüksek blok y={top}, \
                     mesh üst sınırı y={max_y}"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "test dünyasında ağaçlı chunk bulunamadı");
    }

    /// H3'ün gerekçesi: chunk köşesindeki bir yüzün ambient occlusion'u
    /// **çapraz** komşu chunk'ı da okur. Bu yüzden köşedeki bir blok değişince
    /// çapraz chunk'ın mesh'i de tazelenmelidir (`app.rs` → `remesh_around`).
    #[test]
    fn corner_ao_depends_on_the_diagonal_neighbour_chunk() {
        let mut w = test_world(20241012);
        let h = w.height_at(0, 0);

        // (0,0) chunk'ının köşesinde -X yüzü havaya bakan katı bir blok kur.
        assert!(w.set_block_raw(0, h, 0, STONE));
        assert!(w.set_block_raw(-1, h, 0, AIR));
        assert!(w.set_block_raw(0, h + 1, 0, AIR));
        // Çapraz chunk'ı (-1,-1) bu bloğun AO örneklerinden temizle.
        assert!(w.set_block_raw(-1, h - 1, -1, AIR));
        assert!(w.set_block_raw(-1, h + 1, -1, AIR));

        let (before, _) = build_chunk_mesh(&w, 0, 0).expect("mesh üretilemedi");

        // Yalnızca **çapraz** chunk'taki (-1,-1) blokları değiştir.
        assert!(w.set_block_raw(-1, h - 1, -1, STONE));
        assert!(w.set_block_raw(-1, h + 1, -1, STONE));

        let (after, _) = build_chunk_mesh(&w, 0, 0).expect("mesh üretilemedi");

        assert_ne!(
            before.col, after.col,
            "köşe AO'su çapraz komşuya bağlı olmalı: değişiklik mesh'e yansımadı"
        );
        assert_eq!(
            before.pos, after.pos,
            "yalnızca AO değişmeli; konumlar aynı kalmalı"
        );
    }

    #[test]
    fn reset_clears_buffers() {
        let mut m = MeshData::default();
        m.pos.push(1.0);
        m.idx.push(0);
        m.reset();
        assert!(m.is_empty() && m.pos.is_empty());
    }

    #[test]
    fn solid_and_liquid_are_split_into_separate_meshes() {
        // Okyanus chunk'ında sıvı mesh dolu, katı mesh deniz tabanını içerir.
        let mut w = World::new(20261012);
        let mut ocean = None;
        'outer: for i in 0..20_000 {
            let x = (i % 200) * 4 - 400;
            let z = (i / 200) * 4 - 400;
            if w.height_at(x, z) < SEA_LEVEL - 2 {
                ocean = Some((x, z));
                break 'outer;
            }
        }
        let (x, z) = ocean.expect("okyanus sütunu bulunamadı");
        let (cx, cz) = (x >> 4, z >> 4);
        for dz in -1..=1 {
            for dx in -1..=1 {
                w.generate_chunk(cx + dx, cz + dz);
            }
        }
        let (solid, liquid) = build_chunk_mesh(&w, cx, cz).unwrap();
        assert!(!liquid.is_empty(), "okyanus chunk'ında sıvı mesh boş");
        assert!(!solid.is_empty(), "deniz tabanı mesh'i boş");
    }

    #[test]
    fn land_chunks_have_no_liquid_mesh() {
        let w = test_world(20261012);
        // Deniz seviyesinin belirgin şekilde üstünde bir chunk bul.
        let mut land = None;
        for cz in -1..=2 {
            for cx in -1..=2 {
                let mut all_high = true;
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        if w.height_at(cx * CHUNK_SIZE + x, cz * CHUNK_SIZE + z) <= SEA_LEVEL + 2 {
                            all_high = false;
                        }
                    }
                }
                if all_high {
                    land = Some((cx, cz));
                }
            }
        }
        if let Some((cx, cz)) = land {
            let (_, liquid) = build_chunk_mesh(&w, cx, cz).unwrap();
            assert!(liquid.is_empty(), "karada sıvı mesh üretilmemeli");
        }
    }
}
