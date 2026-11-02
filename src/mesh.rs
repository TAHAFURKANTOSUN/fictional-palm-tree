//! Chunk mesh üretimi: görünen yüzler + ambient occlusion.
//!
//! Yüz başına 4 köşe üretilir; komşu veriler 3×3 tabloya önceden alınır, böylece
//! mesh sırasında hiç `HashMap` araması yapılmaz.
//!
//! Köşeler doğrudan GPU'nun beklediği **interleaved** düzende yazılır
//! (`konum(3) + uv(2) + renk(3)`); böylece yükleme öncesi ayrı bir kopyalama /
//! birleştirme adımı gerekmez.

use crate::blocks::*;
use crate::noise::hash3f;
use crate::world::{ckey, Chunk, World, CHUNK_AREA, CHUNK_SIZE, SEA_LEVEL, WORLD_HEIGHT};

/// Doku atlası döşeme boyutu ve sütun sayısı.
pub const TILE: i32 = 16;
pub const ATLAS_COLS: i32 = 16;

/// Köşe başına `f32` sayısı: konum(3) + uv(2) + renk(3).
///
/// Normaller bilerek yoktur: gölgelendirme (yön + AO) köşe rengine önceden
/// işlenir ve shader normal kullanmaz. Eskiden 11 float (44 bayt) olan köşe
/// 8 float'a (32 bayt) indi — GPU belleği ve yükleme boyutu ~%27 küçüldü.
pub const FLOATS_PER_VERTEX: usize = 8;

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

/// Bir chunk katmanının (katı veya sıvı) geometrisi.
pub struct MeshData {
    /// Interleaved köşeler: `[x, y, z, u, v, r, g, b]` × köşe sayısı.
    pub verts: Vec<f32>,
    pub idx: Vec<u32>,
    /// Geometrinin eksen hizalı sınır kutusu (frustum elemesi için).
    min: [f32; 3],
    max: [f32; 3],
}

impl Default for MeshData {
    fn default() -> Self {
        Self { verts: Vec::new(), idx: Vec::new(), min: [f32::MAX; 3], max: [f32::MIN; 3] }
    }
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }

    pub fn face_count(&self) -> u32 {
        (self.idx.len() / 6) as u32
    }

    pub fn vertex_count(&self) -> u32 {
        (self.verts.len() / FLOATS_PER_VERTEX) as u32
    }

    /// Köşe konumları (`[x, y, z]`).
    pub fn positions(&self) -> impl Iterator<Item = &[f32]> {
        self.verts.chunks_exact(FLOATS_PER_VERTEX).map(|v| &v[0..3])
    }

    /// Köşe renkleri (`[r, g, b]`, gölge + AO dâhil).
    pub fn colors(&self) -> impl Iterator<Item = &[f32]> {
        self.verts.chunks_exact(FLOATS_PER_VERTEX).map(|v| &v[5..8])
    }

    /// Tamponları temizler — havuzdan yeniden kullanılırken ayırma yapılmaz.
    pub fn reset(&mut self) {
        self.verts.clear();
        self.idx.clear();
        self.min = [f32::MAX; 3];
        self.max = [f32::MIN; 3];
    }

    /// Kapsayıcı küre (merkez, yarıçap). Görüş konisi (frustum) elemesi için
    /// kullanılır. Sınır kutusu mesh kurulurken tutulduğu için tüm köşeleri
    /// yeniden taramaya gerek yoktur.
    pub fn bounding_sphere(&self) -> ([f32; 3], f32) {
        if self.is_empty() {
            return ([0.0; 3], 0.0);
        }
        let (min, max) = (self.min, self.max);
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

    /// Sınır kutusunu `(x, y, z)` köşeli birim küpü kapsayacak şekilde büyütür.
    #[inline]
    fn grow_bounds(&mut self, x: f32, y: f32, z: f32) {
        for ((lo, hi), p) in self.min.iter_mut().zip(self.max.iter_mut()).zip([x, y, z]) {
            *lo = lo.min(p);
            *hi = hi.max(p + 1.0);
        }
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

    let vary = if def.vary { 0.90 + hash3f(wx, wy, wz) as f32 * 0.20 } else { 1.0 };
    let tint = [def.tint[0] * vary, def.tint[1] * vary, def.tint[2] * vary];

    let base = bk.vertex_count();
    bk.grow_bounds(wx as f32, wy as f32, wz as f32);
    bk.verts.reserve(4 * FLOATS_PER_VERTEX);
    let mut ao = [0usize; 4];

    // AO örnekleri yüzün dışındaki katmandan okunur.
    let bx = wx + face.n[0];
    let by = wy + face.n[1];
    let bz = wz + face.n[2];

    for (i, p) in face.verts.iter().enumerate() {
        let (u, v) = match i {
            0 => (u0, v0),
            1 => (u1, v0),
            2 => (u1, v1),
            _ => (u0, v1),
        };

        let ob: i32 = if p[face.b] == 1.0 { 1 } else { -1 };
        let oc: i32 = if p[face.c] == 1.0 { 1 } else { -1 };
        let mut db = [0i32; 3];
        let mut dc = [0i32; 3];
        db[face.b] = ob;
        dc[face.c] = oc;

        let s1 = occl(bx + db[0], by + db[1], bz + db[2]);
        let s2 = occl(bx + dc[0], by + dc[1], bz + dc[2]);
        // İki yan da doluysa köşe zaten en karanlık seviyededir; çapraz
        // örneği okumaya gerek yok.
        let level = if s1 && s2 {
            0
        } else {
            let sc = occl(bx + db[0] + dc[0], by + db[1] + dc[1], bz + db[2] + dc[2]);
            3 - (s1 as usize + s2 as usize + sc as usize)
        };
        ao[i] = level;

        let f = face.shade * AO_LEVELS[level];
        bk.verts.extend_from_slice(&[
            wx as f32 + p[0],
            wy as f32 + p[1],
            wz as f32 + p[2],
            u,
            v,
            (tint[0] * f).clamp(0.0, 1.0),
            (tint[1] * f).clamp(0.0, 1.0),
            (tint[2] * f).clamp(0.0, 1.0),
        ]);
    }

    // Köşegen yönü AO'ya göre seçilir; aksi halde gölgeleme "yamalı" görünür.
    if ao[0] + ao[2] > ao[1] + ao[3] {
        bk.idx.extend_from_slice(&[base + 1, base + 2, base + 3, base + 1, base + 3, base]);
    } else {
        bk.idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Bir chunk için katı ve sıvı mesh üretir (yeni tamponlarla).
///
/// 8 komşusunun verisi hazır değilse `None` döner (sınırda hayalet yüz oluşmasın).
pub fn build_chunk_mesh(world: &World, cx: i32, cz: i32) -> Option<(MeshData, MeshData)> {
    let mut solid = MeshData::default();
    let mut liquid = MeshData::default();
    build_chunk_mesh_into(world, cx, cz, &mut solid, &mut liquid).then_some((solid, liquid))
}

/// [`build_chunk_mesh`] ile aynı, ancak verilen tamponları **yeniden kullanır**
/// (önce temizlenir). Oyun döngüsü bunu kullanır: her mesh için yeni `Vec`
/// ayırmak yerine kapasite korunur.
///
/// Komşular hazır değilse `false` döner ve tamponlar boş kalır.
pub fn build_chunk_mesh_into(
    world: &World,
    cx: i32,
    cz: i32,
    solid: &mut MeshData,
    liquid: &mut MeshData,
) -> bool {
    solid.reset();
    liquid.reset();

    let Some(ch) = world.chunks.get(&ckey(cx, cz)) else { return false };

    // 3×3 komşu tablosu: (dx+1) + 3*(dz+1)
    let mut nbs: [Option<&Chunk>; 9] = [None; 9];
    for dz in -1..=1 {
        for dx in -1..=1 {
            match world.chunks.get(&ckey(cx + dx, cz + dz)) {
                Some(n) => nbs[((dx + 1) + 3 * (dz + 1)) as usize] = Some(n),
                None => return false,
            }
        }
    }
    let nbs: [&Chunk; 9] = nbs.map(|n| n.expect("komşu yukarıda doğrulandı"));
    let datas: [&[u8]; 9] = nbs.map(|n| n.data.as_slice());

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
        let d = datas[(((x >> 4) + 1) + 3 * ((z >> 4) + 1)) as usize];
        d[(x & 15) as usize + CHUNK_SIZE as usize * ((z & 15) as usize + CHUNK_SIZE as usize * y as usize)]
    };

    let occl = |wx: i32, wy: i32, wz: i32| -> bool { occludes(get(wx - x0, wy, wz - z0)) };

    // Mesh'in y aralığını daralt: yeraltı bloklarının hiçbir yüzü görünmez.
    // Komşu sütun yükseklikleri de hesaba katılır (chunk sınırı duvarları).
    // Yükseklikler üretimde önbelleğe alındığı için gürültü yeniden örneklenmez.
    let mut min_h = i32::MAX;
    let mut max_h = i32::MIN;
    for z in -1..=CHUNK_SIZE {
        for x in -1..=CHUNK_SIZE {
            let n = nbs[(((x >> 4) + 1) + 3 * ((z >> 4) + 1)) as usize];
            let h = n.heights[((x & 15) + CHUNK_SIZE * (z & 15)) as usize] as i32;
            min_h = min_h.min(h);
            max_h = max_h.max(h);
        }
    }
    // Oyuncu kazısı arazinin altını açığa çıkarabilir: kendisinde veya bir
    // komşuda yapılan en alçak düzenlemenin bir altı da taranmalıdır
    // (kazılan kuyunun tabanı ve duvarları).
    let min_edit = nbs.iter().map(|n| n.min_edit_y).min().unwrap_or(WORLD_HEIGHT);

    // Üst sınır **arazi yüksekliği değil**, chunk'ın gerçek en yüksek dolu
    // bloğudur: ağaç yaprakları zeminin 6–8 blok üstüne çıkar. Yalnızca arazi
    // yüksekliğine bakılırsa yaprak tepeleri mesh dışında kalır ve görünmez
    // (ama katı) bloklar oluşur.
    let y_top = max_h.max(SEA_LEVEL).max(ch.content_top);
    let y0 = (min_h - 1).min(min_edit - 1).max(0);
    let y1 = (y_top + 2).min(WORLD_HEIGHT - 1);

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
                let bk = if def.liquid { &mut *liquid } else { &mut *solid };

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

    true
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
        assert_eq!(solid.verts.len() % FLOATS_PER_VERTEX, 0);
        assert_eq!(solid.vertex_count(), solid.face_count() * 4, "yüz başına 4 köşe");
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
            let verts = m.vertex_count();
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
        for p in solid.positions() {
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
        assert_eq!(ma.verts, mb.verts);
        assert_eq!(ma.idx, mb.idx);
    }

    #[test]
    fn bounding_sphere_covers_geometry() {
        let mut w = test_world(31);
        w.generate_chunk(0, 0);
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        let (c, r) = solid.bounding_sphere();
        for p in solid.positions() {
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
                    .positions()
                    .chain(liquid.positions())
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

        let cols = |m: &MeshData| m.colors().flatten().copied().collect::<Vec<f32>>();
        let poss = |m: &MeshData| m.positions().flatten().copied().collect::<Vec<f32>>();
        assert_ne!(
            cols(&before), cols(&after),
            "köşe AO'su çapraz komşuya bağlı olmalı: değişiklik mesh'e yansımadı"
        );
        assert_eq!(
            poss(&before), poss(&after),
            "yalnızca AO değişmeli; konumlar aynı kalmalı"
        );
    }

    #[test]
    fn reused_buffers_give_identical_mesh() {
        let w = test_world(4242);
        let (fresh_s, fresh_l) = build_chunk_mesh(&w, 1, 1).unwrap();
        let mut s = MeshData::default();
        let mut l = MeshData::default();
        // Önce başka bir chunk'la doldur; sonra yeniden kullan.
        assert!(build_chunk_mesh_into(&w, 0, 0, &mut s, &mut l));
        assert!(build_chunk_mesh_into(&w, 1, 1, &mut s, &mut l));
        assert_eq!(s.verts, fresh_s.verts);
        assert_eq!(s.idx, fresh_s.idx);
        assert_eq!(l.verts, fresh_l.verts);
        assert_eq!(s.bounding_sphere(), fresh_s.bounding_sphere());
    }

    #[test]
    fn reset_clears_buffers() {
        let mut m = MeshData::default();
        m.verts.push(1.0);
        m.idx.push(0);
        m.reset();
        assert!(m.is_empty() && m.verts.is_empty());
        assert_eq!(m.bounding_sphere().1, 0.0, "sınır kutusu da sıfırlanmalı");
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

#[cfg(test)]
mod dig_tests {
    use super::*;

    /// Oyuncu arazinin derinine bir kuyu kazdığında, kuyunun dibindeki ve
    /// duvarlarındaki yüzler mesh'e girmelidir (aksi hâlde dünyanın içi görünür).
    #[test]
    fn deep_shaft_walls_are_meshed() {
        let mut w = test_world(20261012);
        let (x, z) = (8, 8);
        let h = w.height_at(x, z);
        let bottom = 3;
        for y in bottom..=h {
            assert!(w.set_block_raw(x, y, z, AIR));
        }
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        let min_y = solid.positions().map(|p| p[1]).fold(f32::MAX, f32::min);
        assert!(
            min_y <= bottom as f32,
            "kuyu dibi mesh'te yok: en düşük yüz y={min_y}, kuyu dibi y={bottom}"
        );
    }
}
