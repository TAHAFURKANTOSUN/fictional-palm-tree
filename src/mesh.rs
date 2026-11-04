//! Chunk mesh üretimi: görünen yüzler + ambient occlusion.
//!
//! Çıktı doğrudan GPU'nun okuduğu **paketlenmiş** biçimdedir: köşe başına
//! 8 bayt (bkz. [`Vertex`]). `gl.rs` bu baytları ara dönüşüm ya da kopya
//! olmadan yükler.
//!
//! * **İndeks üretilmez.** Her yüz 4 ardışık köşedir ve tüm chunk'lar tek bir
//!   paylaşılan dörtgen indeks tamponuyla ([`QUAD_PATTERN`]) çizilir. AO'ya göre
//!   seçilen köşegen, indeksler yerine köşe sırası döndürülerek uygulanır.
//! * **UV gönderilmez.** Döşeme numarası ve köşe kodu tek bir bayta sığar;
//!   doku koordinatı gölgelendiricide hesaplanır. Normal hiç kullanılmadığı
//!   için hiç üretilmez (gölgelendirme köşe rengine işlenmiştir).
//! * **Komşu tablosu.** Chunk ve 8 komşusunun ilgili dilimi önce 18×18×H'lik
//!   dolgulu bir diziye kopyalanır; böylece mesh sırasında her komşu okuması
//!   tek bir dizi indeksidir (`HashMap` ya da chunk hesabı yok).
//! * **Üç katman.** Opak (çoğu blok), oyuk (saydam pikselli döşemeler: cam)
//!   ve sıvı. Opak katman `discard`'sız gölgelendiriciyle çizilir.
//! * **Yön grupları.** Her katmanın yüzleri 6 normal yönüne göre gruplanır
//!   ([`MeshData::groups`]); çizici kameraya sırtını dönen grupları hiç
//!   göndermez ([`visible_groups`]).

use crate::atlas::cutout_tiles;
use crate::blocks::*;
use crate::noise::hash3f;
use crate::world::{ckey, World, CHUNK_AREA, CHUNK_SIZE, WORLD_HEIGHT};

/// Paketlenmiş köşe (8 bayt):
///
/// | bayt | anlam |
/// |---|---|
/// | 0..3 | chunk-yerel `x, y, z` (`x,z ∈ 0..=16`, `y ∈ 0..=64`) |
/// | 3 | `döşeme * 4 + köşe` (köşe: bit0 = u, bit1 = v) |
/// | 4..7 | renk `r, g, b` (unorm8) — doku tonu × gölge × AO |
/// | 7 | 255 (kullanılmıyor; 4 baytlık hizalı biçim için) |
pub type Vertex = [u8; 8];

/// Bir köşenin bayt boyutu (GPU tampon adımı).
pub const VERTEX_BYTES: usize = std::mem::size_of::<Vertex>();

/// Dörtgen başına indeks deseni: `(0,1,2)` ve `(0,2,3)` üçgenleri.
pub const QUAD_PATTERN: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// Döşeme numarası `tile * 4 + 3` olarak bir bayta sığmalı.
const MAX_TILES: u8 = 64;

const AO_LEVELS: [f32; 4] = [0.52, 0.70, 0.86, 1.0];

/// `FACES[..].verts` sırasındaki köşelerin UV kodu: `u | v << 1`.
/// (0 → (u0,v0), 1 → (u1,v0), 2 → (u1,v1), 3 → (u0,v1))
const CORNER_UV: [u8; 4] = [0b00, 0b01, 0b11, 0b10];

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

/// Bir chunk katmanının GPU'ya hazır geometrisi.
#[derive(Default, Clone)]
pub struct MeshData {
    /// Dörtlü gruplar hâlinde köşeler; her grup bir yüzdür. Yüzler
    /// [`FACES`] sırasına göre yön yön dizilidir.
    pub verts: Vec<Vertex>,
    /// Her yöndeki yüz sayısı (`FACES` sırasıyla); toplamı `face_count()`.
    pub groups: [u32; 6],
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.verts.is_empty()
    }

    /// Yüz (dörtgen) sayısı.
    pub fn face_count(&self) -> u32 {
        (self.verts.len() / 4) as u32
    }

    /// Paylaşılan indeks tamponundan çizilecek indeks sayısı.
    pub fn index_count(&self) -> u32 {
        self.face_count() * QUAD_PATTERN.len() as u32
    }

    /// Tamponu temizler — yeniden kullanılırken ayırma yapılmaz.
    pub fn reset(&mut self) {
        self.verts.clear();
        self.groups = [0; 6];
    }

    /// `f` yön grubunun ilk yüzünün katman içindeki sırası.
    pub fn group_start(&self, f: usize) -> u32 {
        self.groups[..f].iter().sum()
    }

    /// Her yön grubundaki yüzlerin, kendi normal ekseni boyunca düzlem
    /// koordinatı aralığı `[min, max]` (chunk-yerel). Boş grupta `[255, 0]`.
    pub fn plane_spans(&self) -> [[u8; 2]; 6] {
        let mut spans = [[u8::MAX, 0]; 6];
        let mut q = 0usize;
        for (f, &n) in self.groups.iter().enumerate() {
            let axis = FACE_AXIS[f];
            for quad in self.verts[q * 4..(q + n as usize) * 4].chunks_exact(4) {
                let p = quad[0][axis];
                spans[f][0] = spans[f][0].min(p);
                spans[f][1] = spans[f][1].max(p);
            }
            q += n as usize;
        }
        spans
    }

    /// İki katmanı yön grupları korunarak birleştirir (testler ve opak/oyuk
    /// ayrımını önemsemeyen çağıranlar için).
    pub fn merged(a: &MeshData, b: &MeshData) -> MeshData {
        let mut out = MeshData::default();
        let (mut qa, mut qb) = (0usize, 0usize);
        for f in 0..6 {
            let (na, nb) = (a.groups[f] as usize, b.groups[f] as usize);
            out.verts.extend_from_slice(&a.verts[qa * 4..(qa + na) * 4]);
            out.verts.extend_from_slice(&b.verts[qb * 4..(qb + nb) * 4]);
            out.groups[f] = (na + nb) as u32;
            qa += na;
            qb += nb;
        }
        out
    }

    /// GPU'ya gidecek ham baytlar (kopya yok).
    pub fn as_bytes(&self) -> &[u8] {
        self.verts.as_flattened()
    }

    /// Chunk-yerel kapsayıcı kutu `(min, max)`; görüş konisi elemesi için.
    pub fn local_bounds(&self) -> Option<([u8; 3], [u8; 3])> {
        let first = self.verts.first()?;
        let mut min = [first[0], first[1], first[2]];
        let mut max = min;
        for v in &self.verts {
            for a in 0..3 {
                min[a] = min[a].min(v[a]);
                max[a] = max[a].max(v[a]);
            }
        }
        Some((min, max))
    }
}

/// Her yön grubunun normal ekseni (0 = x, 1 = y, 2 = z) ve yönü.
const FACE_AXIS: [usize; 6] = [0, 0, 1, 1, 2, 2];
const FACE_POSITIVE: [bool; 6] = [true, false, true, false, true, false];

/// Kameraya **dönük olabilecek** yön gruplarının bit maskesi (bit `f` ⇒
/// `FACES[f]`). `cam` chunk-yerel kamera konumudur.
///
/// Bir yüz yalnızca kamera düzleminin ön tarafındaysa görünür (perspektifte
/// arka yüz elemesiyle birebir aynı kural). `+n` yönlü bir grubun en alçak
/// düzlemi bile kameranın önündeyse grubun hiçbir yüzü görünmez; bu test
/// muhafazakârdır (görünen bir yüzü asla atmaz).
pub fn visible_groups(cam: [f32; 3], spans: &[[u8; 2]; 6], groups: &[u32; 6]) -> u8 {
    let mut mask = 0u8;
    for f in 0..6 {
        if groups[f] == 0 {
            continue;
        }
        let c = cam[FACE_AXIS[f]];
        let [lo, hi] = spans[f];
        let visible = if FACE_POSITIVE[f] { c > lo as f32 } else { c < hi as f32 };
        if visible {
            mask |= 1 << f;
        }
    }
    mask
}

/// Köşenin chunk-yerel konumu.
#[inline]
pub fn vertex_pos(v: &Vertex) -> [u8; 3] {
    [v[0], v[1], v[2]]
}

/// Köşenin döşeme numarası ve UV kodu (`u | v << 1`).
#[inline]
pub fn vertex_tile(v: &Vertex) -> (u8, u8) {
    (v[3] >> 2, v[3] & 3)
}

#[inline]
fn unorm8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// AO köşegeni: `true` ise dörtgen `1–3` köşegeninden bölünür. Aksi hâlde
/// gölgeleme "yamalı" görünür.
#[inline]
pub fn flip_diagonal(ao: [usize; 4]) -> bool {
    ao[0] + ao[2] > ao[1] + ao[3]
}

/// Dolgulu komşu tablosunun yatay boyutu: chunk + her yanda 1 blok.
const PX: usize = CHUNK_SIZE as usize + 2;
const PAREA: usize = PX * PX;

/// Dolgulu tabloda bir eksen boyunca 1 bloğun ofseti.
const AXIS_STRIDE: [isize; 3] = [1, PAREA as isize, PX as isize];

/// Katman indeksleri.
const OPAQUE: usize = 0;
const CUTOUT: usize = 1;
const LIQUID: usize = 2;

/// Yeniden kullanılabilir mesh üreticisi. Tamponlar kareler arasında
/// korunur; kararlı durumda mesh üretimi hiç bellek ayırmaz.
#[derive(Default)]
pub struct Mesher {
    /// 18×18×(y aralığı + 2) blok kimlikleri.
    pad: Vec<u8>,
    /// Katman × yön başına geçici köşe kovaları.
    buckets: [[Vec<Vertex>; 6]; 3],
    /// Saydam pikseli olmayan katı yüzler (`discard`'sız çizilir).
    pub opaque: MeshData,
    /// Saydam pikselli döşemeler (cam): alfa testiyle çizilir.
    pub cutout: MeshData,
    pub liquid: MeshData,
}

impl Mesher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Üç katmanın toplam yüz sayısı.
    pub fn face_count(&self) -> u32 {
        self.opaque.face_count() + self.cutout.face_count() + self.liquid.face_count()
    }

    /// `(cx, cz)` chunk'ının mesh'ini `opaque` / `cutout` / `liquid`
    /// katmanlarına üretir.
    ///
    /// 8 komşusunun verisi hazır değilse `false` döner (sınırda hayalet yüz
    /// oluşmasın); katmanlar o durumda boş bırakılır.
    pub fn build(&mut self, world: &World, cx: i32, cz: i32) -> bool {
        self.opaque.reset();
        self.cutout.reset();
        self.liquid.reset();
        for b in self.buckets.iter_mut().flatten() {
            b.clear();
        }

        let mut nbs: [&[u8]; 9] = [&[]; 9];
        let mut open_bottom = i32::MAX;
        let mut content_top = 0;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let Some(n) = world.chunks.get(&ckey(cx + dx, cz + dz)) else {
                    return false;
                };
                nbs[((dx + 1) + 3 * (dz + 1)) as usize] = &n.data;
                // Yüz görünürlüğü yalnızca kendisine ve 4 kenar komşusuna
                // bağlıdır; çapraz komşular yalnızca AO'yu etkiler.
                if dx == 0 || dz == 0 {
                    open_bottom = open_bottom.min(n.open_bottom);
                }
                if dx == 0 && dz == 0 {
                    content_top = n.content_top;
                }
            }
        }

        // y aralığı: `open_bottom - 1`'in altındaki her bloğun 6 komşusu da
        // opaktır (görünmez); `content_top`'ın üstü ise tamamen havadır.
        let y0 = (open_bottom - 1).max(0);
        let y1 = content_top.min(WORLD_HEIGHT - 1);
        if y1 < y0 {
            return true;
        }

        // --- dolgulu komşu tablosu: x,z ∈ [-1,16], y ∈ [y0-1, y1+1] ---
        let py0 = y0 - 1;
        let layers = (y1 - y0 + 3) as usize;
        self.pad.clear();
        self.pad.resize(PAREA * layers, AIR);
        let cs = CHUNK_SIZE as usize;
        for layer in 0..layers {
            let y = py0 + layer as i32;
            let base = layer * PAREA;
            let row = &mut self.pad[base..base + PAREA];
            if y < 0 {
                row.fill(STONE);
                continue;
            }
            if y >= WORLD_HEIGHT {
                continue; // zaten AIR
            }
            let yo = CHUNK_AREA * y as usize;
            for pz in 0..PX {
                let (nz, lz) = match pz {
                    0 => (0, cs - 1),
                    p if p == PX - 1 => (2, 0),
                    p => (1, p - 1),
                };
                let src = yo + cs * lz;
                let dst = &mut row[pz * PX..(pz + 1) * PX];
                dst[0] = nbs[3 * nz][src + cs - 1];
                dst[1..=cs].copy_from_slice(&nbs[1 + 3 * nz][src..src + cs]);
                dst[PX - 1] = nbs[2 + 3 * nz][src];
            }
        }
        let pad = &self.pad[..];

        // --- yüz başına sabitler: komşu ofseti + 4 köşenin AO örnekleri ---
        let mut nb_off = [0isize; 6];
        let mut ao_off = [[[0isize; 3]; 4]; 6];
        for (f, face) in FACES.iter().enumerate() {
            nb_off[f] = face.n[0] as isize * AXIS_STRIDE[0]
                + face.n[1] as isize * AXIS_STRIDE[1]
                + face.n[2] as isize * AXIS_STRIDE[2];
            for (i, p) in face.verts.iter().enumerate() {
                let ob = if p[face.b] == 1 { 1 } else { -1 } * AXIS_STRIDE[face.b];
                let oc = if p[face.c] == 1 { 1 } else { -1 } * AXIS_STRIDE[face.c];
                ao_off[f][i] = [nb_off[f] + ob, nb_off[f] + oc, nb_off[f] + ob + oc];
            }
        }

        let x0 = cx * CHUNK_SIZE;
        let z0 = cz * CHUNK_SIZE;
        let cutout = cutout_tiles();

        for y in y0..=y1 {
            let layer = (y - py0) as usize * PAREA;
            for z in 0..CHUNK_SIZE {
                let row = layer + PX * (z as usize + 1) + 1;
                for x in 0..CHUNK_SIZE {
                    let p = row + x as usize;
                    let id = pad[p];
                    if id == AIR {
                        continue;
                    }
                    let def = block_def(id);
                    let mut vary: Option<f32> = None;

                    for (f, face) in FACES.iter().enumerate() {
                        let nb = pad[(p as isize + nb_off[f]) as usize];
                        let visible = nb == AIR || (block_def(nb).transparent && nb != id);
                        if !visible {
                            continue;
                        }

                        // Varyasyon blok başına bir kez hesaplanır (yüz başına değil).
                        let m = *vary.get_or_insert_with(|| {
                            if def.vary {
                                0.90 + hash3f(x0 + x, y, z0 + z) as f32 * 0.20
                            } else {
                                1.0
                            }
                        });

                        let tile = match face.tile {
                            TileKind::Top => def.top,
                            TileKind::Bottom => def.bottom,
                            TileKind::Side => def.side,
                        };
                        debug_assert!(tile < MAX_TILES);

                        let mut quad = [[0u8; 8]; 4];
                        let mut ao = [0usize; 4];
                        for (i, v) in face.verts.iter().enumerate() {
                            let o = &ao_off[f][i];
                            let s1 = is_opaque(pad[(p as isize + o[0]) as usize]);
                            let s2 = is_opaque(pad[(p as isize + o[1]) as usize]);
                            let sc = is_opaque(pad[(p as isize + o[2]) as usize]);
                            let level = if s1 && s2 {
                                0
                            } else {
                                3 - (s1 as usize + s2 as usize + sc as usize)
                            };
                            ao[i] = level;

                            let k = face.shade * AO_LEVELS[level];
                            quad[i] = [
                                x as u8 + v[0],
                                y as u8 + v[1],
                                z as u8 + v[2],
                                tile * 4 + CORNER_UV[i],
                                unorm8(def.tint[0] * m * k),
                                unorm8(def.tint[1] * m * k),
                                unorm8(def.tint[2] * m * k),
                                255,
                            ];
                        }

                        let layer = if def.liquid {
                            LIQUID
                        } else if cutout & (1 << tile) != 0 {
                            CUTOUT
                        } else {
                            OPAQUE
                        };
                        let out = &mut self.buckets[layer][f];
                        // Köşegen AO'ya göre seçilir: indeks deseni sabit
                        // olduğundan köşe sırası döndürülür — (1,2,3)+(1,3,0)
                        // üçgenleri eskisiyle birebir aynıdır.
                        if flip_diagonal(ao) {
                            out.extend_from_slice(&[quad[1], quad[2], quad[3], quad[0]]);
                        } else {
                            out.extend_from_slice(&quad);
                        }
                    }
                }
            }
        }

        // Kovaları katmanlara yön sırasıyla diz.
        for (layer, out) in [&mut self.opaque, &mut self.cutout, &mut self.liquid]
            .into_iter()
            .enumerate()
        {
            for (f, bucket) in self.buckets[layer].iter().enumerate() {
                out.groups[f] = (bucket.len() / 4) as u32;
                out.verts.extend_from_slice(bucket);
            }
        }

        true
    }
}

/// Bir chunk için `(katı, sıvı)` mesh üretir; katı = opak + oyuk
/// (tek seferlik kullanım ve testler). Sık çağrılan yerlerde tamponları
/// korumak ve katmanları ayrı almak için [`Mesher`] kullanın.
///
/// 8 komşusunun verisi hazır değilse `None` döner (sınırda hayalet yüz oluşmasın).
pub fn build_chunk_mesh(world: &World, cx: i32, cz: i32) -> Option<(MeshData, MeshData)> {
    let mut m = Mesher::new();
    if !m.build(world, cx, cz) {
        return None;
    }
    Some((MeshData::merged(&m.opaque, &m.cutout), m.liquid))
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

    /// Bir mesh'teki tüm köşelerin dünya-yerel `y` değerleri.
    fn ys(m: &MeshData) -> impl Iterator<Item = u8> + '_ {
        m.verts.iter().map(|v| vertex_pos(v)[1])
    }

    #[test]
    fn mesh_is_whole_quads_in_packed_format() {
        let w = test_world(5);
        let (solid, liquid) = build_chunk_mesh(&w, 0, 0).expect("mesh üretilemedi");
        assert!(!solid.is_empty(), "yüzey chunk'ı boş mesh üretmemeli");
        for m in [&solid, &liquid] {
            assert_eq!(m.verts.len() % 4, 0, "her yüz 4 köşe olmalı");
            assert_eq!(m.as_bytes().len(), m.verts.len() * VERTEX_BYTES);
            assert_eq!(VERTEX_BYTES, 8);
            for v in &m.verts {
                let [x, y, z] = vertex_pos(v);
                assert!(x <= 16 && z <= 16 && y as i32 <= WORLD_HEIGHT, "konum chunk dışında: {v:?}");
                let (tile, _) = vertex_tile(v);
                assert!(tile < MAX_TILES);
                assert_eq!(v[7], 255);
            }
        }
    }

    #[test]
    fn face_count_matches_index_count() {
        let w = test_world(11);
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        assert_eq!(solid.face_count() as usize * 4, solid.verts.len());
        assert_eq!(solid.index_count(), solid.face_count() * 6);
    }

    /// Paylaşılan indeks deseni + döndürülmüş köşe sırası, eski AO'ya bağlı
    /// indekslerle **aynı üçgenleri** (aynı sarım yönüyle) vermeli.
    #[test]
    fn rotated_quads_reproduce_the_ao_diagonal() {
        let tris = |order: [usize; 4]| -> Vec<[usize; 3]> {
            QUAD_PATTERN
                .chunks_exact(3)
                .map(|t| [order[t[0] as usize], order[t[1] as usize], order[t[2] as usize]])
                .collect()
        };
        // Eski kod: çevrilmişse [1,2,3, 1,3,0], değilse [0,1,2, 0,2,3].
        assert_eq!(tris([1, 2, 3, 0]), vec![[1, 2, 3], [1, 3, 0]]);
        assert_eq!(tris([0, 1, 2, 3]), vec![[0, 1, 2], [0, 2, 3]]);
        assert!(flip_diagonal([3, 0, 3, 0]));
        assert!(!flip_diagonal([0, 3, 0, 3]));
        assert!(!flip_diagonal([2, 2, 2, 2]), "eşitlikte köşegen değişmemeli");
    }

    /// UV kodu, döndürmeden sonra da her köşenin **kendi** köşesini taşımalı:
    /// bir dörtgenin 4 köşesi 4 farklı UV köşesine sahip olmalı.
    #[test]
    fn every_quad_uses_all_four_uv_corners() {
        let w = test_world(9);
        let (solid, liquid) = build_chunk_mesh(&w, 0, 0).unwrap();
        for m in [&solid, &liquid] {
            for q in m.verts.chunks_exact(4) {
                let mut seen = [false; 4];
                for v in q {
                    seen[vertex_tile(v).1 as usize] = true;
                }
                assert_eq!(seen, [true; 4], "dörtgen köşe kodları bozuk: {q:?}");
                assert!(q.iter().all(|v| vertex_tile(v).0 == vertex_tile(&q[0]).0));
            }
        }
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
        // Yüzeyin çok altındaki yüzler mesh'te olmamalı (y aralığı daraltıldı).
        let w = test_world(21);
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        let min_y = ys(&solid).min().unwrap();
        let h_min = (-1..=CHUNK_SIZE)
            .flat_map(|x| (-1..=CHUNK_SIZE).map(move |z| (x, z)))
            .map(|(x, z)| w.height_at(x, z))
            .min()
            .unwrap();
        assert!(
            min_y as i32 >= h_min - 2,
            "y aralığı daraltılmadı: en düşük yüz {min_y}, min yükseklik {h_min}"
        );
    }

    /// Arazinin altına kazılan çukurun tabanı görünmeli. Eskiden y aralığı
    /// gürültü yüksekliğinden hesaplandığı için taban mesh dışında kalıyor ve
    /// çukura bakınca boşluk görünüyordu.
    #[test]
    fn deep_hole_floor_is_meshed() {
        let mut w = test_world(20261012);
        let (x, z) = (8, 8);
        let h = w.height_at(x, z);
        for y in (h - 6)..=h {
            assert!(w.set_block_raw(x, y, z, AIR));
        }
        let floor_top = (h - 6) as u8; // tabandaki bloğun üst yüzü
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        // Tabandaki bloğun üst yüzü: 4 köşesi de y = floor_top, x,z ∈ {8, 9}.
        let found = solid.verts.chunks_exact(4).any(|q| {
            q.iter().all(|v| {
                let [px, py, pz] = vertex_pos(v);
                py == floor_top && (8..=9).contains(&px) && (8..=9).contains(&pz)
            })
        });
        assert!(found, "çukur tabanı (y={floor_top}) mesh'te yok");
    }

    #[test]
    fn mesh_is_deterministic() {
        let a = test_world(77);
        let b = test_world(77);
        let (ma, _) = build_chunk_mesh(&a, 2, 2).unwrap();
        let (mb, _) = build_chunk_mesh(&b, 2, 2).unwrap();
        assert_eq!(ma.verts, mb.verts);
    }

    /// `Mesher` tamponlarını korur: aynı chunk iki kez üretilince sonuç aynı,
    /// ikinci üretimde de yeni ayırma gerekmez.
    #[test]
    fn mesher_reuses_buffers() {
        let w = test_world(42);
        let mut m = Mesher::new();
        assert!(m.build(&w, 1, 1));
        let first = m.opaque.verts.clone();
        let cap = m.opaque.verts.capacity();
        assert!(m.build(&w, 0, 0));
        assert!(m.build(&w, 1, 1));
        assert_eq!(m.opaque.verts, first);
        assert!(m.opaque.verts.capacity() >= cap);
        let (fresh, _) = build_chunk_mesh(&w, 1, 1).unwrap();
        assert_eq!(fresh.verts.len(), first.len(), "cam yokken katı = opak");
    }

    #[test]
    fn local_bounds_cover_geometry() {
        let w = test_world(31);
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        let (mn, mx) = solid.local_bounds().unwrap();
        for v in &solid.verts {
            let p = vertex_pos(v);
            for a in 0..3 {
                assert!(mn[a] <= p[a] && p[a] <= mx[a], "kutu geometriyi kapsamıyor");
            }
        }
        assert!(MeshData::default().local_bounds().is_none());
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
                let max_y = ys(&solid).chain(ys(&liquid)).max().unwrap_or(0) as i32;

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

        let colors = |m: &MeshData| m.verts.iter().map(|v| [v[4], v[5], v[6]]).collect::<Vec<_>>();
        // AO köşegeni değişirse köşe sırası döner; konumları dörtgen başına
        // sırasız karşılaştır.
        let quads = |m: &MeshData| {
            m.verts
                .chunks_exact(4)
                .map(|q| {
                    let mut p: Vec<[u8; 3]> = q.iter().map(vertex_pos).collect();
                    p.sort();
                    p
                })
                .collect::<Vec<_>>()
        };
        assert_ne!(
            colors(&before), colors(&after),
            "köşe AO'su çapraz komşuya bağlı olmalı: değişiklik mesh'e yansımadı"
        );
        assert_eq!(
            quads(&before), quads(&after),
            "yalnızca AO değişmeli; yüzler aynı kalmalı"
        );
    }

    /// Yüzler yön gruplarına doğru dizilmeli: grup `f`'deki her dörtgen
    /// `FACES[f]` normaline sahip olmalı (4 köşesi de aynı düzlemde).
    #[test]
    fn faces_are_grouped_by_direction() {
        let w = test_world(8);
        let mut m = Mesher::new();
        assert!(m.build(&w, 0, 0));
        for layer in [&m.opaque, &m.cutout, &m.liquid] {
            assert_eq!(layer.groups.iter().sum::<u32>(), layer.face_count());
            let mut q = 0;
            for (f, &n) in layer.groups.iter().enumerate() {
                let a = FACE_AXIS[f];
                for quad in layer.verts[q * 4..(q + n as usize) * 4].chunks_exact(4) {
                    assert!(quad.iter().all(|v| v[a] == quad[0][a]), "grup {f} düzlemsel değil");
                    let other = (a + 1) % 3;
                    assert!(quad.iter().any(|v| v[other] != quad[0][other]));
                }
                q += n as usize;
            }
            for (span, &n) in layer.plane_spans().iter().zip(&layer.groups) {
                if n > 0 {
                    assert!(span[0] <= span[1]);
                }
            }
        }
    }

    /// Cam yüzleri oyuk katmanına, geri kalan katılar opak katmana gider.
    #[test]
    fn glass_goes_to_the_cutout_layer() {
        let mut w = test_world(13);
        let (x, z) = (4, 4);
        let h = w.height_at(x, z);
        // Havada asılı (6 yüzü de açık) bir cam bloğu.
        for y in h + 1..h + 8 {
            assert!(w.set_block_raw(x, y, z, AIR));
        }
        assert!(w.set_block_raw(x, h + 4, z, GLASS));
        let mut m = Mesher::new();
        assert!(m.build(&w, 0, 0));
        assert_eq!(m.cutout.face_count(), 6, "havada tek cam bloğu 6 yüz");
        let glass_tile = block_def(GLASS).side;
        assert!(m.cutout.verts.iter().all(|v| vertex_tile(v).0 == glass_tile));
        assert!(m.opaque.verts.iter().all(|v| vertex_tile(v).0 != glass_tile));
        let (solid, _) = build_chunk_mesh(&w, 0, 0).unwrap();
        assert_eq!(solid.face_count(), m.opaque.face_count() + m.cutout.face_count());
    }

    /// Yön elemesi muhafazakâr olmalı: kameraya dönük (ön yüzü görünen) her
    /// dörtgenin grubu maskede bulunmalı; ve kamera dışarıdayken bir şeyler
    /// gerçekten elenmeli.
    #[test]
    fn group_culling_never_drops_front_faces() {
        let w = test_world(20261012);
        let mut m = Mesher::new();
        assert!(m.build(&w, 0, 0));
        let layer = &m.opaque;
        let spans = layer.plane_spans();
        let mut culled_any = false;
        for cam in [
            [8.0f32, 80.0, 8.0],
            [-20.0, 30.0, 5.0],
            [40.0, 10.0, -30.0],
            [7.5, 33.2, 9.1],
            [16.0, 64.0, 16.0],
            [0.0, 0.0, 0.0],
        ] {
            let mask = visible_groups(cam, &spans, &layer.groups);
            culled_any |= mask.count_ones() < layer.groups.iter().filter(|&&g| g > 0).count() as u32;
            let mut q = 0;
            for f in 0..6 {
                for quad in layer.verts[q * 4..(q + layer.groups[f] as usize) * 4].chunks_exact(4) {
                    let a = FACE_AXIS[f];
                    let p = quad[0][a] as f32;
                    let front = if FACE_POSITIVE[f] { cam[a] > p } else { cam[a] < p };
                    if front {
                        assert!(mask & (1 << f) != 0, "kamera {cam:?}: görünen grup {f} elendi");
                    }
                }
                q += layer.groups[f] as usize;
            }
        }
        assert!(culled_any, "dışarıdaki kameralar için hiçbir grup elenmedi");
    }

    #[test]
    fn reset_clears_buffers() {
        let mut m = MeshData::default();
        m.verts.push([0; 8]);
        m.reset();
        assert!(m.is_empty() && m.as_bytes().is_empty());
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
