//! Chunk mesh üretimi: görünen yüzler + ambient occlusion.
//!
//! # Vertex biçimi
//!
//! Her yüz 4 köşedir, her köşe [`VERTEX_BYTES`] = 8 bayt tutar:
//!
//! | bayt | içerik                                                        |
//! |------|---------------------------------------------------------------|
//! | 0–2  | chunk-yerel konum: `x` 0..=16, `y` 0..=64, `z` 0..=16          |
//! | 3    | döşeme indeksi (alt 4 bit) + uv köşesi (`<< CORNER_SHIFT`)     |
//! | 4–6  | renk `r, g, b` (ton × blok varyasyonu × yüz ışığı × AO)        |
//! | 7    | dolgu (4 bayt hizalama)                                        |
//!
//! Konumlar chunk'a göre yereldir; çizici chunk'ın kameraya göre ofsetini
//! ekler. Böylece hem vertex küçülür (eskiden `f32` olarak 44 bayt + 6 indeks)
//! hem de orijinden çok uzakta `f32` hassasiyeti bozulmaz.
//!
//! İndeks tamponu **yoktur**: her yüz aynı `0,1,2, 0,2,3` desenini kullanır,
//! çizici bunu tüm chunk'lar için tek bir ortak tampondan okur. AO'ya göre
//! köşegenin çevrilmesi gerektiğinde indeksler değil, köşelerin yazım sırası
//! bir kaydırılır.
//!
//! # Üretim
//!
//! Chunk ve 8 komşusunun ilgili katmanları önce 18×18'lik çerçeveli bir
//! tampona kopyalanır. Ardından her komşu okuması (yüz görünürlüğü ve köşe
//! başına üç AO örneği) sabit bir indeks farkından ibarettir: iç döngüde ne
//! `HashMap` araması ne de sınır/komşu-chunk dallanması kalır.

use crate::atlas::ATLAS_COLS;
use crate::blocks::*;
use crate::noise::hash3f;
use crate::world::{ckey, World, CHUNK_AREA, CHUNK_SIZE, WORLD_HEIGHT};

/// Köşe başına bayt sayısı.
pub const VERTEX_BYTES: usize = 8;
/// Yüz başına bayt sayısı (4 köşe).
pub const QUAD_BYTES: usize = 4 * VERTEX_BYTES;
/// 3. baytta uv köşesinin başladığı bit: `döşeme | köşe << CORNER_SHIFT`.
pub const CORNER_SHIFT: u32 = 4;

// Döşeme indeksi 4 bite sığmalı (çizicideki gölgelendirici de buna güvenir).
const _: () = assert!(ATLAS_COLS <= 1 << CORNER_SHIFT);
// Yerel konumlar `u8`'e sığmalı.
const _: () = assert!(CHUNK_SIZE <= 255 && WORLD_HEIGHT <= 255);

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
    /// Köşeler saat yönünün tersine; `i`. köşenin uv'si `i`. uv köşesidir
    /// (`0:(0,0) 1:(1,0) 2:(1,1) 3:(0,1)`).
    pub verts: [[u8; 3]; 4],
}

pub const FACES: [Face; 6] = [
    Face {
        n: [1, 0, 0], b: 1, c: 2, shade: 0.76, tile: TileKind::Side,
        verts: [[1, 0, 1], [1, 0, 0], [1, 1, 0], [1, 1, 1]],
    },
    Face {
        n: [-1, 0, 0], b: 1, c: 2, shade: 0.76, tile: TileKind::Side,
        verts: [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]],
    },
    Face {
        n: [0, 1, 0], b: 0, c: 2, shade: 1.00, tile: TileKind::Top,
        verts: [[0, 1, 1], [1, 1, 1], [1, 1, 0], [0, 1, 0]],
    },
    Face {
        n: [0, -1, 0], b: 0, c: 2, shade: 0.48, tile: TileKind::Bottom,
        verts: [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]],
    },
    Face {
        n: [0, 0, 1], b: 0, c: 1, shade: 0.62, tile: TileKind::Side,
        verts: [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]],
    },
    Face {
        n: [0, 0, -1], b: 0, c: 1, shade: 0.62, tile: TileKind::Side,
        verts: [[1, 0, 0], [0, 0, 0], [0, 1, 0], [1, 1, 0]],
    },
];

// --- çerçeveli çalışma tamponu -------------------------------------------

/// Çerçeveli tamponun kenarı: chunk + her yanda bir blok.
const PAD: usize = CHUNK_SIZE as usize + 2;
const PAD_AREA: usize = PAD * PAD;

/// Çerçeveli tamponda `x`, `y`, `z` yönünde bir blokluk indeks farkı.
const AXIS_STEP: [isize; 3] = [1, PAD_AREA as isize, PAD as isize];

/// `FACE_STEP[f]`: `f` yüzünün baktığı komşu hücreye indeks farkı.
const FACE_STEP: [isize; 6] = {
    let mut out = [0isize; 6];
    let mut f = 0;
    while f < 6 {
        let n = FACES[f].n;
        out[f] = n[0] as isize * AXIS_STEP[0]
            + n[1] as isize * AXIS_STEP[1]
            + n[2] as isize * AXIS_STEP[2];
        f += 1;
    }
    out
};

/// `AO_STEP[f][v]`: `f` yüzünün `v` köşesini gölgeleyebilecek üç hücreye
/// (iki kenar komşusu + çapraz) indeks farkları.
const AO_STEP: [[[isize; 3]; 4]; 6] = {
    let mut out = [[[0isize; 3]; 4]; 6];
    let mut f = 0;
    while f < 6 {
        let face = &FACES[f];
        let mut v = 0;
        while v < 4 {
            let p = face.verts[v];
            let sb = if p[face.b] == 1 { AXIS_STEP[face.b] } else { -AXIS_STEP[face.b] };
            let sc = if p[face.c] == 1 { AXIS_STEP[face.c] } else { -AXIS_STEP[face.c] };
            out[f][v] = [FACE_STEP[f] + sb, FACE_STEP[f] + sc, FACE_STEP[f] + sb + sc];
            v += 1;
        }
        f += 1;
    }
    out
};

/// `SHADE[f][ao]`: yüz ışığı × ambient occlusion seviyesi.
const SHADE: [[f32; 4]; 6] = {
    let mut out = [[0.0f32; 4]; 6];
    let mut f = 0;
    while f < 6 {
        let mut level = 0;
        while level < 4 {
            out[f][level] = FACES[f].shade * AO_LEVELS[level];
            level += 1;
        }
        f += 1;
    }
    out
};

// --- çıktı ----------------------------------------------------------------

/// Çözülmüş tek bir köşe (testler ve hata ayıklama için).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Vertex {
    /// Chunk-yerel konum.
    pub pos: [u8; 3],
    /// Doku atlasındaki döşeme indeksi.
    pub tile: u8,
    /// uv köşesi: `0:(0,0) 1:(1,0) 2:(1,1) 3:(0,1)`.
    pub corner: u8,
    pub rgb: [u8; 3],
}

/// Bir chunk katmanının (katı ya da sıvı) paketlenmiş geometrisi.
pub struct MeshData {
    /// Modül belgesindeki biçimde, yüz başına [`QUAD_BYTES`] bayt.
    pub verts: Vec<u8>,
    /// Geometrinin kapladığı `y` aralığı (blok sınırları). Mesh boşken
    /// `y_min > y_max` olur.
    pub y_min: u8,
    pub y_max: u8,
}

impl Default for MeshData {
    fn default() -> Self {
        Self { verts: Vec::new(), y_min: u8::MAX, y_max: 0 }
    }
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.verts.is_empty()
    }

    pub fn face_count(&self) -> u32 {
        (self.verts.len() / QUAD_BYTES) as u32
    }

    /// Tamponu temizler; kapasite korunduğu için yeniden kullanımda ayırma
    /// yapılmaz.
    pub fn reset(&mut self) {
        self.verts.clear();
        self.y_min = u8::MAX;
        self.y_max = 0;
    }

    /// Köşeleri yazıldıkları sırayla çözer (yüz başına art arda 4 köşe).
    pub fn vertices(&self) -> impl Iterator<Item = Vertex> + '_ {
        self.verts.chunks_exact(VERTEX_BYTES).map(|b| Vertex {
            pos: [b[0], b[1], b[2]],
            tile: b[3] & ((1 << CORNER_SHIFT) - 1),
            corner: b[3] >> CORNER_SHIFT,
            rgb: [b[4], b[5], b[6]],
        })
    }
}

/// Mesh üretiminin yeniden kullanılan tamponları. Uygulama tek bir örnek
/// tutar; böylece chunk başına hiç bellek ayrılmaz.
#[derive(Default)]
pub struct MeshBuffers {
    pub solid: MeshData,
    pub liquid: MeshData,
    /// Chunk + 1 blokluk komşu çerçevesi, katman katman (`PAD_AREA` bayt).
    pad: Vec<u8>,
}

impl MeshBuffers {
    pub fn face_count(&self) -> u32 {
        self.solid.face_count() + self.liquid.face_count()
    }

    /// İki katmanın birlikte kapladığı `y` aralığı; ikisi de boşsa `None`.
    /// Görüş konisi elemesi **her iki** katmanı da kapsayan kutuyu kullanmalı,
    /// yoksa deniz tabanı görüş dışındayken su yüzeyi de elenir.
    pub fn y_bounds(&self) -> Option<(u8, u8)> {
        let lo = self.solid.y_min.min(self.liquid.y_min);
        let hi = self.solid.y_max.max(self.liquid.y_max);
        (lo <= hi).then_some((lo, hi))
    }
}

#[inline]
fn color_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// `pad[i]` hücresindeki bloğun `f` yüzünü `out`'a ekler.
#[inline]
fn push_face(out: &mut MeshData, pad: &[u8], i: usize, f: usize, pos: [u8; 3], tile: u8, tint: [f32; 3]) {
    let face = &FACES[f];

    // Köşe başına AO: iki kenar komşusu da doluysa en koyu seviye, aksi hâlde
    // dolu komşu sayısı kadar koyulaşır.
    let mut ao = [0usize; 4];
    for (v, steps) in AO_STEP[f].iter().enumerate() {
        let s1 = is_opaque(pad[(i as isize + steps[0]) as usize]);
        let s2 = is_opaque(pad[(i as isize + steps[1]) as usize]);
        let sc = is_opaque(pad[(i as isize + steps[2]) as usize]);
        ao[v] = if s1 && s2 { 0 } else { 3 - (s1 as usize + s2 as usize + sc as usize) };
    }

    // Köşegen yönü AO'ya göre seçilir; aksi hâlde gölgeleme "yamalı" görünür.
    // Ortak indeks deseni `0,1,2, 0,2,3` olduğundan köşegeni çevirmek için
    // köşeler bir kaydırılarak yazılır (`1,2,3,0`): üçgenler `1,2,3` ve
    // `1,3,0` olur, sarım yönü değişmez.
    let order: [usize; 4] = if ao[0] + ao[2] > ao[1] + ao[3] { [1, 2, 3, 0] } else { [0, 1, 2, 3] };

    let mut quad = [0u8; QUAD_BYTES];
    for (slot, &v) in order.iter().enumerate() {
        let p = face.verts[v];
        let k = SHADE[f][ao[v]];
        let o = slot * VERTEX_BYTES;
        quad[o] = pos[0] + p[0];
        quad[o + 1] = pos[1] + p[1];
        quad[o + 2] = pos[2] + p[2];
        quad[o + 3] = tile | (v as u8) << CORNER_SHIFT;
        quad[o + 4] = color_byte(tint[0] * k);
        quad[o + 5] = color_byte(tint[1] * k);
        quad[o + 6] = color_byte(tint[2] * k);
    }
    out.verts.extend_from_slice(&quad);
    out.y_min = out.y_min.min(pos[1]);
    out.y_max = out.y_max.max(pos[1] + 1);
}

/// Bir chunk'ın katı ve sıvı mesh'ini `out` içine üretir.
///
/// Chunk'ın kendisi ya da 8 komşusundan biri yüklü değilse `false` döner ve
/// `out` boş kalır (sınırda hayalet yüz oluşmasın).
pub fn build_chunk_mesh_into(world: &World, cx: i32, cz: i32, out: &mut MeshBuffers) -> bool {
    out.solid.reset();
    out.liquid.reset();

    let Some(ch) = world.chunks.get(&ckey(cx, cz)) else {
        return false;
    };
    let mut nbs = [ch; 9];
    for dz in -1..=1 {
        for dx in -1..=1 {
            match world.chunks.get(&ckey(cx + dx, cz + dz)) {
                Some(n) => nbs[((dx + 1) + 3 * (dz + 1)) as usize] = n,
                None => return false,
            }
        }
    }

    // Taranacak y aralığı. Bir yüz ancak bitişiğinde opak olmayan bir hücre
    // varsa görünür; o hücre bu chunk'ta ya da dört kenar komşusundan
    // birindedir. Dolayısıyla beşinin `min_open` değerinin bir altından daha
    // aşağıda hiçbir yüz yoktur. Üstte ise bu chunk'ın en yüksek dolu bloğu
    // sınırdır.
    //
    // Alt sınır **arazi yüksekliğinden** hesaplanmaz: oyuncu kazarak yüzeyin
    // altını açabilir, o zaman kuyunun duvarları ve tabanı da mesh'e
    // girmelidir. (Üst sınır için de aynısı geçerlidir: ağaç yaprakları
    // zeminin 6–8 blok üstüne çıkar.)
    let open = nbs[4].min_open.min(nbs[1].min_open).min(nbs[3].min_open).min(nbs[5].min_open).min(nbs[7].min_open);
    let y0 = (open - 1).max(0);
    let y1 = ch.content_top.min(WORLD_HEIGHT - 1);
    if y1 < y0 {
        return true;
    }

    // Çerçeveli tampon: y0-1 ..= y1+1 katmanları (alt ve üst komşular için
    // birer katman pay). Dünyanın altı taş, üstü hava sayılır.
    let layers = (y1 - y0 + 3) as usize;
    let size = CHUNK_SIZE as usize;
    let pad = &mut out.pad;
    pad.clear();
    pad.resize(layers * PAD_AREA, AIR);
    for (ly, layer) in pad.chunks_exact_mut(PAD_AREA).enumerate() {
        let wy = y0 - 1 + ly as i32;
        if wy < 0 {
            layer.fill(STONE);
            continue;
        }
        if wy >= WORLD_HEIGHT {
            continue;
        }
        let y_base = CHUNK_AREA * wy as usize;
        for (pz, row) in layer.chunks_exact_mut(PAD).enumerate() {
            // Satırın hangi chunk sırasından (kuzey / orta / güney) geldiği.
            let (nz, lz) = match pz {
                0 => (0, size - 1),
                p if p == PAD - 1 => (6, 0),
                p => (3, p - 1),
            };
            let src = y_base + size * lz;
            row[0] = nbs[nz].data[src + size - 1];
            row[1..=size].copy_from_slice(&nbs[nz + 1].data[src..src + size]);
            row[PAD - 1] = nbs[nz + 2].data[src];
        }
    }
    let pad = &out.pad[..];

    let x0 = cx * CHUNK_SIZE;
    let z0 = cz * CHUNK_SIZE;

    for ly in 1..layers - 1 {
        let wy = y0 - 1 + ly as i32;
        for z in 0..size {
            let row = ly * PAD_AREA + (z + 1) * PAD + 1;
            for x in 0..size {
                let i = row + x;
                let id = pad[i];
                if !is_cube(id) {
                    continue;
                }

                // Blok başına bir kez, yalnızca görünen ilk yüzde hesaplanır.
                let mut shading: Option<(&'static BlockDef, [f32; 3])> = None;

                for f in 0..6 {
                    let nb = pad[(i as isize + FACE_STEP[f]) as usize];
                    // Opak komşu yüzü gizler; aynı türden saydam komşu da
                    // (su-su, cam-cam) arada yüz bırakmaz.
                    if is_opaque(nb) || nb == id {
                        continue;
                    }

                    let (def, tint) = *shading.get_or_insert_with(|| {
                        let def = block_def(id);
                        let mut tint = def.tint;
                        if def.vary {
                            let m = 0.90 + hash3f(x0 + x as i32, wy, z0 + z as i32) as f32 * 0.20;
                            tint = [tint[0] * m, tint[1] * m, tint[2] * m];
                        }
                        (def, tint)
                    });
                    let tile = match FACES[f].tile {
                        TileKind::Top => def.top,
                        TileKind::Bottom => def.bottom,
                        TileKind::Side => def.side,
                    };
                    let layer_out = if def.liquid { &mut out.liquid } else { &mut out.solid };
                    push_face(layer_out, pad, i, f, [x as u8, wy as u8, z as u8], tile, tint);
                }
            }
        }
    }

    true
}

/// [`build_chunk_mesh_into`]'nun yeni tampon ayıran kolay sürümü:
/// `(katı, sıvı)` döndürür, komşular hazır değilse `None`.
pub fn build_chunk_mesh(world: &World, cx: i32, cz: i32) -> Option<(MeshData, MeshData)> {
    let mut out = MeshBuffers::default();
    build_chunk_mesh_into(world, cx, cz, &mut out).then_some((out.solid, out.liquid))
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
    use crate::world::SEA_LEVEL;

    /// Tüm köşelerin sıralanmış konumları (yazım sırasından bağımsız kıyas için).
    fn sorted_positions(m: &MeshData) -> Vec<[u8; 3]> {
        let mut v: Vec<[u8; 3]> = m.vertices().map(|v| v.pos).collect();
        v.sort_unstable();
        v
    }

    fn sorted_colors(m: &MeshData) -> Vec<([u8; 3], [u8; 3])> {
        let mut v: Vec<([u8; 3], [u8; 3])> = m.vertices().map(|v| (v.pos, v.rgb)).collect();
        v.sort_unstable();
        v
    }

    /// Dört köşesi de `inside` koşulunu sağlayan yüzlerin sayısı.
    fn quads_within(m: &MeshData, inside: impl Fn([u8; 3]) -> bool) -> usize {
        let verts: Vec<Vertex> = m.vertices().collect();
        verts.chunks_exact(4).filter(|q| q.iter().all(|v| inside(v.pos))).count()
    }

    #[test]
    fn mesh_is_made_of_whole_quads() {
        let w = test_world(5);
        let (solid, liquid) = build_chunk_mesh(&w, 0, 0).expect("mesh üretilemedi");
        for m in [&solid, &liquid] {
            assert_eq!(m.verts.len() % QUAD_BYTES, 0, "her yüz 4 köşe olmalı");
            assert_eq!(m.face_count() as usize * QUAD_BYTES, m.verts.len());
            assert_eq!(m.vertices().count(), m.face_count() as usize * 4);
        }
        assert!(!solid.is_empty(), "yüzey chunk'ı boş mesh üretmemeli");
    }

    #[test]
    fn every_vertex_is_in_range() {
        let w = test_world(9);
        let (solid, liquid) = build_chunk_mesh(&w, 0, 0).unwrap();
        for (name, m) in [("solid", &solid), ("liquid", &liquid)] {
            for v in m.vertices() {
                assert!(v.pos[0] <= CHUNK_SIZE as u8 && v.pos[2] <= CHUNK_SIZE as u8, "{name}: {v:?}");
                assert!(v.pos[1] <= WORLD_HEIGHT as u8, "{name}: {v:?}");
                assert!(v.tile <= T_CRAFT_SIDE, "{name}: bilinmeyen döşeme {v:?}");
                assert!(v.corner < 4, "{name}: geçersiz uv köşesi {v:?}");
            }
        }
    }

    /// Ortak indeks deseni `0,1,2, 0,2,3` ile her yüz: (a) ön yüzü boş/saydam
    /// hücreye bakan, (b) arkasında gerçekten blok olan, (c) saat yönünün
    /// tersine sarılı bir karedir ve uv köşeleri konumla birlikte döner.
    #[test]
    fn every_quad_faces_outwards_from_a_real_block() {
        let w = test_world(20261012);
        let mut quads = 0;
        for (cx, cz) in [(0, 0), (1, -1), (-1, 2)] {
            let (solid, liquid) = build_chunk_mesh(&w, cx, cz).unwrap();
            for m in [&solid, &liquid] {
                let verts: Vec<Vertex> = m.vertices().collect();
                for q in verts.chunks_exact(4) {
                    let p = |i: usize| [q[i].pos[0] as i32, q[i].pos[1] as i32, q[i].pos[2] as i32];
                    let sub = |a: [i32; 3], b: [i32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
                    let cross = |a: [i32; 3], b: [i32; 3]| {
                        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
                    };
                    // İki üçgen de aynı birim normale sahip olmalı.
                    let n = cross(sub(p(1), p(0)), sub(p(2), p(0)));
                    assert_eq!(n, cross(sub(p(2), p(0)), sub(p(3), p(0))), "üçgenler farklı yöne bakıyor");
                    assert_eq!(n.iter().map(|c| c.abs()).sum::<i32>(), 1, "normal eksene hizalı değil: {n:?}");

                    // uv köşeleri döngüsel sırada (köşegen çevrilse bile).
                    for i in 0..4 {
                        assert_eq!(q[(i + 1) % 4].corner, (q[i].corner + 1) % 4, "uv sırası bozuk");
                    }

                    // Yüzün arkasındaki blok ve önündeki hücre.
                    let min = [0, 1, 2].map(|a| (0..4).map(|i| p(i)[a]).min().unwrap());
                    let axis = n.iter().position(|c| *c != 0).unwrap();
                    let (mut behind, mut front) = (min, min);
                    if n[axis] > 0 {
                        behind[axis] -= 1;
                    } else {
                        front[axis] -= 1;
                    }
                    let (bx, bz) = (cx * CHUNK_SIZE, cz * CHUNK_SIZE);
                    let block = w.get_block(bx + behind[0], behind[1], bz + behind[2]);
                    let facing = w.get_block(bx + front[0], front[1], bz + front[2]);
                    assert_ne!(block, AIR, "yüzün arkasında blok yok");
                    assert!(!is_opaque(facing) && facing != block, "yüz gizli olmalıydı");
                    assert_eq!(is_liquid(block), std::ptr::eq(m, &liquid), "yüz yanlış katmanda");
                    quads += 1;
                }
            }
        }
        assert!(quads > 500, "beklenenden az yüz denetlendi: {quads}");
    }

    /// Meşale küp olarak çizilmez ve altındaki bloğun üst yüzünü gizlemez.
    #[test]
    fn torches_add_no_cube_faces() {
        let mut w = test_world(20261012);
        let h = w.height_at(8, 8);
        let (before, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        assert!(w.set_block_raw(8, h + 1, 8, TORCH));
        let (after, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        assert_eq!(before.verts, after.verts, "meşale mesh'i değiştirmemeli");
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
        // Yüzeyin çok altındaki yüzler mesh'te olmamalı.
        let w = test_world(21);
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        let min_y = solid.vertices().map(|v| v.pos[1] as i32).min().unwrap();
        let h_min = (-1..=CHUNK_SIZE)
            .flat_map(|x| (-1..=CHUNK_SIZE).map(move |z| (x, z)))
            .map(|(x, z)| w.height_at(x, z))
            .min()
            .unwrap();
        assert!(
            min_y >= h_min - 2,
            "gömülü bloklar yüz üretti: en düşük yüz {min_y}, min yükseklik {h_min}"
        );
    }

    /// Regresyon: mesh'in alt sınırı arazi yüksekliğinden hesaplanırken,
    /// yüzeyin birkaç blok altına kazılan kuyunun duvarları ve tabanı mesh'e
    /// girmiyor, oyuncu dünyanın içinden boşluğu görüyordu.
    #[test]
    fn faces_exposed_by_digging_are_meshed() {
        let mut w = test_world(20261012);
        let (x, z) = (8, 8);
        let h = w.height_at(x, z);
        assert!(h > 8, "test sütunu fazla alçak: {h}");
        for y in 3..=h {
            assert!(w.set_block_raw(x, y, z, AIR));
        }

        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        let in_shaft = |p: [u8; 3]| (8..=9).contains(&p[0]) && (8..=9).contains(&p[2]);
        assert_eq!(
            quads_within(&solid, |p| in_shaft(p) && p[1] == 3),
            1,
            "kuyunun tabanı (y=3) mesh'te yok"
        );
        // Taban + en alttaki blok hizasında (y 3..=4) kuyunun dört duvarı.
        assert_eq!(
            quads_within(&solid, |p| in_shaft(p) && (3..=4).contains(&p[1])),
            1 + 4,
            "kuyunun dip duvarları mesh'te yok"
        );
    }

    /// Aynı durumun chunk sınırındaki hâli: komşu chunk'ta açılan boşluk, bu
    /// chunk'ın sınırdaki bloklarının yan yüzlerini de açığa çıkarır.
    #[test]
    fn digging_in_a_neighbour_chunk_exposes_this_chunks_border_faces() {
        let mut w = test_world(20261012);
        // (-1, 8) batı komşusunun (chunk -1,0) doğu kenarındadır.
        let h = w.height_at(-1, 8);
        for y in 3..=h {
            assert!(w.set_block_raw(-1, y, 8, AIR));
        }
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        // (0, 3, 8) bloğunun -X yüzü: x == 0 düzleminde, y 3..=4 arasında.
        let exposed =
            quads_within(&solid, |p| p[0] == 0 && (3..=4).contains(&p[1]) && (8..=9).contains(&p[2]));
        assert_eq!(exposed, 1, "komşudaki kuyuya bakan sınır yüzü mesh'te yok");
    }

    #[test]
    fn mesh_is_deterministic() {
        let a = test_world(77);
        let b = test_world(77);
        let (ma, la) = build_chunk_mesh(&a, 2, 2).unwrap();
        let (mb, lb) = build_chunk_mesh(&b, 2, 2).unwrap();
        assert_eq!(ma.verts, mb.verts);
        assert_eq!(la.verts, lb.verts);
    }

    /// Tamponlar yeniden kullanıldığında önceki chunk'tan artık kalmamalı.
    #[test]
    fn reused_buffers_give_the_same_mesh_as_fresh_ones() {
        let w = test_world(20261012);
        let mut buf = MeshBuffers::default();
        for (cx, cz) in [(2, 2), (-1, 0), (0, 0), (2, 2)] {
            assert!(build_chunk_mesh_into(&w, cx, cz, &mut buf));
            let (solid, liquid) = build_chunk_mesh(&w, cx, cz).unwrap();
            assert_eq!(buf.solid.verts, solid.verts, "({cx},{cz}) katı mesh farklı");
            assert_eq!(buf.liquid.verts, liquid.verts, "({cx},{cz}) sıvı mesh farklı");
            assert_eq!((buf.solid.y_min, buf.solid.y_max), (solid.y_min, solid.y_max));
        }
        // Başarısız üretim tamponları boş bırakmalı.
        assert!(!build_chunk_mesh_into(&w, 50, 50, &mut buf));
        assert!(buf.solid.is_empty() && buf.liquid.is_empty() && buf.y_bounds().is_none());
    }

    #[test]
    fn y_bounds_cover_both_layers() {
        // Okyanus chunk'ı: katı mesh deniz tabanı, sıvı mesh su yüzeyidir.
        let (w, cx, cz) = ocean_world();
        let mut buf = MeshBuffers::default();
        assert!(build_chunk_mesh_into(&w, cx, cz, &mut buf));
        let (lo, hi) = buf.y_bounds().expect("okyanus chunk'ı boş olamaz");
        for m in [&buf.solid, &buf.liquid] {
            for v in m.vertices() {
                assert!((lo..=hi).contains(&v.pos[1]), "köşe y={} aralık dışında ({lo}..={hi})", v.pos[1]);
            }
        }
        assert_eq!(lo, buf.solid.y_min.min(buf.liquid.y_min));
        assert_eq!(hi, buf.solid.y_max.max(buf.liquid.y_max));
        // Su yüzeyi deniz tabanının üstündedir; kutu yalnızca katı mesh'ten
        // hesaplansaydı su yüzeyi dışarıda kalabilirdi.
        assert_eq!(buf.liquid.y_max as i32, SEA_LEVEL + 1, "su yüzeyi deniz seviyesinde olmalı");
        assert!(hi as i32 > SEA_LEVEL && buf.solid.y_min < buf.liquid.y_max);
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
                    .vertices()
                    .chain(liquid.vertices())
                    .map(|v| v.pos[1] as i32)
                    .max()
                    .unwrap();

                assert!(
                    max_y > top,
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
            sorted_colors(&before),
            sorted_colors(&after),
            "köşe AO'su çapraz komşuya bağlı olmalı: değişiklik mesh'e yansımadı"
        );
        assert_eq!(
            sorted_positions(&before),
            sorted_positions(&after),
            "yalnızca AO değişmeli; konumlar aynı kalmalı"
        );
    }

    #[test]
    fn reset_clears_buffers() {
        let mut m = MeshData::default();
        m.verts.extend_from_slice(&[0; QUAD_BYTES]);
        m.y_min = 3;
        m.y_max = 9;
        m.reset();
        assert!(m.is_empty() && m.face_count() == 0);
        assert!(m.y_min > m.y_max, "boş mesh'in y aralığı da boş olmalı");
    }

    /// Okyanus sütunu içeren, 8 komşusu hazır bir chunk: `(dünya, cx, cz)`.
    fn ocean_world() -> (World, i32, i32) {
        let mut w = World::new(20261012);
        let mut ocean = None;
        for i in 0..20_000 {
            let x = (i % 200) * 4 - 400;
            let z = (i / 200) * 4 - 400;
            if w.height_at(x, z) < SEA_LEVEL - 2 {
                ocean = Some((x, z));
                break;
            }
        }
        let (x, z) = ocean.expect("okyanus sütunu bulunamadı");
        let (cx, cz) = (x >> 4, z >> 4);
        for dz in -1..=1 {
            for dx in -1..=1 {
                w.generate_chunk(cx + dx, cz + dz);
            }
        }
        (w, cx, cz)
    }

    #[test]
    fn solid_and_liquid_are_split_into_separate_meshes() {
        // Okyanus chunk'ında sıvı mesh dolu, katı mesh deniz tabanını içerir.
        let (w, cx, cz) = ocean_world();
        let (solid, liquid) = build_chunk_mesh(&w, cx, cz).unwrap();
        assert!(!liquid.is_empty(), "okyanus chunk'ında sıvı mesh boş");
        assert!(!solid.is_empty(), "deniz tabanı mesh'i boş");
        assert!(liquid.vertices().all(|v| v.tile == T_WATER), "sıvı mesh'te su dışı döşeme");
        assert!(solid.vertices().all(|v| v.tile != T_WATER), "katı mesh'te su döşemesi");
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
