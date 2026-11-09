//! Sonsuz, chunk tabanlı dünya.
//!
//! Dünya sabit boyutlu değildir: yalnızca oyuncunun çevresindeki chunk'lar
//! bellekte tutulur, gerisi talep üzerine üretilir ve boşaltılır.
//! Oyuncunun yaptığı değişiklikler `edits` haritasında saklanır, böylece bir
//! chunk boşaltılıp yeniden üretildiğinde binalar kaybolmaz.

use std::collections::HashMap;

use crate::blocks::*;
use crate::noise::{hash3f, Perlin};

pub const CHUNK_SIZE: i32 = 16;
pub const CHUNK_AREA: usize = 256; // CHUNK_SIZE^2
pub const WORLD_HEIGHT: i32 = 64;
pub const SEA_LEVEL: i32 = 26;

/// Yüklü tutulan alan, görüş mesafesinden bu kadar chunk geniştir; böylece
/// oyuncu bir chunk sınırında ileri geri gittiğinde aynı chunk'lar tekrar tekrar
/// üretilmez. `stream_plan`'ın veri kümesi her zaman bu sınırın içinde kalır.
pub const KEEP_MARGIN: i32 = 2;

/// Chunk anahtarı: iki `i32` koordinat tek bir `u64`'e paketlenir (üst 32 bit
/// `cx`, alt 32 bit `cz`). Tüm `i32` aralığında birebirdir — hiçbir chunk
/// çifti aynı anahtara düşmez.
#[inline]
pub fn ckey(cx: i32, cz: i32) -> u64 {
    ((cx as u32 as u64) << 32) | (cz as u32 as u64)
}

/// Chunk içi yerel indeks: `x + 16*(z + 16*y)`.
#[inline]
pub fn li(x: usize, z: usize, y: usize) -> usize {
    x + CHUNK_SIZE as usize * (z + CHUNK_SIZE as usize * y)
}

pub struct Chunk {
    pub cx: i32,
    pub cz: i32,
    pub data: Vec<u8>,
    pub meshed: bool,
    /// Bu chunk'taki **en yüksek dolu blok** (arazi + su yüzeyi + ağaç + oyuncu
    /// düzenlemesi). Mesh'in `y` aralığını daraltmak için kullanılır.
    ///
    /// Yalnızca arazi yüksekliğine bakmak yetmez: ağaç yaprakları zeminin 6–8
    /// blok üstüne çıkar, bu yüzden ağaç tepeleri eskiden mesh dışında kalıp
    /// görünmez oluyordu (bloklar katı kalıyordu).
    pub content_top: i32,
    /// Bu chunk'ta **opak olmayan** (hava, su, cam) bir hücrenin bulunduğu en
    /// düşük `y`. Bunun bir altındaki katmandan daha aşağıda hiçbir yüz
    /// görünemez; mesh'in alt sınırı buradan gelir.
    ///
    /// Arazi yüksekliğinden türetilmez, çünkü oyuncu kazarak yüzeyin altında
    /// boşluk açabilir. Blok kaldırıldıkça düşer, doldurulunca yükselmez
    /// (fazladan taranan katman zararsızdır, eksik taranan katman görünmez
    /// duvar demektir).
    pub min_open: i32,
}

pub struct World {
    pub seed: i32,
    pub chunks: HashMap<u64, Chunk>,
    pub edits: HashMap<u64, HashMap<usize, u8>>,
    nc: Perlin,
    nd: Perlin,
}

impl World {
    pub fn new(seed: i32) -> Self {
        Self {
            seed,
            chunks: HashMap::new(),
            edits: HashMap::new(),
            nc: Perlin::new((seed as u32) ^ 0x9e37_79b9),
            nd: Perlin::new((seed as u32) ^ 0x85eb_ca6b),
        }
    }

    /// Dünya yüksekliği — **normalize edilmeyen** iki gürültü katmanı.
    /// Chunk sınırında süreklilik bu sayede garanti edilir.
    pub fn height_at(&self, wx: i32, wz: i32) -> i32 {
        let c = self.nc.fbm(wx as f64 * 0.0055, wz as f64 * 0.0055, 4);
        let d = self.nd.fbm(wx as f64 * 0.028, wz as f64 * 0.028, 2);
        let h = (SEA_LEVEL as f64 + 2.0 + c * 34.0 + d * 4.5).floor() as i32;
        h.clamp(4, WORLD_HEIGHT - 10)
    }

    /// Yüklü değilse `AIR` döner (chunk akışı bunu bekler).
    #[inline]
    pub fn get_block(&self, x: i32, y: i32, z: i32) -> u8 {
        if y < 0 {
            return STONE;
        }
        if y >= WORLD_HEIGHT {
            return AIR;
        }
        match self.chunks.get(&ckey(x >> 4, z >> 4)) {
            Some(ch) => ch.data[li((x & 15) as usize, (z & 15) as usize, y as usize)],
            None => AIR,
        }
    }

    /// Bloğu yazar ve değişikliği kalıcı olarak kaydeder.
    pub fn set_block_raw(&mut self, x: i32, y: i32, z: i32, v: u8) -> bool {
        if y < 0 || y >= WORLD_HEIGHT {
            return false;
        }
        let key = ckey(x >> 4, z >> 4);
        let idx = li((x & 15) as usize, (z & 15) as usize, y as usize);

        {
            let ch = match self.chunks.get_mut(&key) {
                Some(c) => c,
                None => return false,
            };
            ch.data[idx] = v;
            if y > ch.content_top {
                ch.content_top = y;
            }
            if y < ch.min_open && !is_opaque(v) {
                ch.min_open = y;
            }
        }

        self.edits.entry(key).or_default().insert(idx, v);
        true
    }

    /// Chunk verisini üretir (arazi + ağaçlar + oyuncu değişiklikleri).
    pub fn generate_chunk(&mut self, cx: i32, cz: i32) -> u64 {
        let key = ckey(cx, cz);
        let mut data = vec![AIR; CHUNK_AREA * WORLD_HEIGHT as usize];
        let x0 = cx * CHUNK_SIZE;
        let z0 = cz * CHUNK_SIZE;

        // Chunk'taki gerçek en yüksek dolu blok — mesh'in y aralığı buna göre
        // daraltılır. Ağaç yaprakları da dâhil edilmezse tepeleri görünmez olur.
        let mut content_top = 0;
        // En alçak sütun: onun hemen üstü (hava ya da su) ilk açık hücredir.
        let mut min_h = WORLD_HEIGHT;

        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let h = self.height_at(x0 + x, z0 + z);
                let base = x as usize + CHUNK_SIZE as usize * z as usize;

                for y in 0..=h {
                    let b = if y == 0 {
                        BEDROCK
                    } else if y < h - 4 {
                        STONE
                    } else if y < h {
                        DIRT
                    } else {
                        GRASS
                    };
                    data[base + CHUNK_AREA * y as usize] = b;
                }
                for y in (h + 1)..=SEA_LEVEL {
                    data[base + CHUNK_AREA * y as usize] = WATER;
                }
                if h <= SEA_LEVEL + 1 {
                    let ys = if h - 3 < 1 { 1 } else { h - 3 };
                    for y in ys..=h {
                        data[base + CHUNK_AREA * y as usize] = SAND;
                    }
                }

                min_h = min_h.min(h);
                content_top = content_top.max(h);
                if h < SEA_LEVEL {
                    content_top = content_top.max(SEA_LEVEL);
                }
            }
        }

        // Ağaçlar 3 blok taşma payıyla taranır: chunk sınırına denk gelen bir
        // ağaç ikiye bölünse de her iki tarafta tutarlı çıkar.
        for z in -3..CHUNK_SIZE + 3 {
            for x in -3..CHUNK_SIZE + 3 {
                let wx = x0 + x;
                let wz = z0 + z;
                if hash3f(wx, 7, wz) <= 0.9935 {
                    continue;
                }
                let h = self.height_at(wx, wz);
                if h <= SEA_LEVEL + 1 {
                    continue;
                }
                content_top = content_top.max(place_tree(&mut data, x, h + 1, z, wx, wz));
            }
        }

        // Ağaçlar zeminin üstündedir; alt sınırı aşağı çekecek açık hücre
        // oluşturmaz. Oyuncu düzenlemeleri ise oluşturabilir.
        let mut min_open = min_h + 1;

        if let Some(ed) = self.edits.get(&key) {
            for (&idx, &v) in ed {
                data[idx] = v;
                let y = (idx / CHUNK_AREA) as i32;
                if y > content_top {
                    content_top = y;
                }
                if y < min_open && !is_opaque(v) {
                    min_open = y;
                }
            }
        }

        self.chunks.insert(
            key,
            Chunk { cx, cz, data, meshed: false, content_top, min_open },
        );
        key
    }

    /// Chunk yüklü değilse üretir.
    pub fn ensure_chunk(&mut self, cx: i32, cz: i32) {
        if !self.chunks.contains_key(&ckey(cx, cz)) {
            self.generate_chunk(cx, cz);
        }
    }

    /// Oyuncunun çevresindeki 3×3 chunk'ı garanti eder (güvenlik ağı:
    /// akış yetişmezse oyuncu boşluğa düşmesin).
    pub fn ensure_around(&mut self, px: f64, pz: f64) {
        let pcx = (px / CHUNK_SIZE as f64).floor() as i32;
        let pcz = (pz / CHUNK_SIZE as f64).floor() as i32;
        for dz in -1..=1 {
            for dx in -1..=1 {
                self.ensure_chunk(pcx + dx, pcz + dz);
            }
        }
    }

    pub fn unload_chunk(&mut self, key: u64) {
        self.chunks.remove(&key);
    }

    /// Oyuncunun doğacağı uygun noktayı altın açı spirali ile arar:
    /// deniz seviyesinin üstünde ve ağaç gövdesinin içinde olmayan bir yer.
    pub fn find_spawn(&self) -> [f64; 3] {
        const GA: f64 = 2.399_963_23;
        for i in 1..600 {
            let r = (i as f64).sqrt() * 6.0;
            let x = (i as f64 * GA).cos() * r;
            let z = (i as f64 * GA).sin() * r;
            let (xi, zi) = (x.round() as i32, z.round() as i32);
            let h = self.height_at(xi, zi);
            if h <= SEA_LEVEL + 1 || h > 40 {
                continue;
            }
            let mut tree = false;
            for dz in -2..=2 {
                for dx in -2..=2 {
                    if hash3f(xi + dx, 7, zi + dz) > 0.9935 {
                        tree = true;
                    }
                }
            }
            if tree {
                continue;
            }
            return [xi as f64 + 0.5, h as f64 + 1.02, zi as f64 + 0.5];
        }
        let h = self.height_at(0, 0);
        [0.5, h as f64 + 1.02, 0.5]
    }
}

/// Oyuncunun chunk'ı `(pcx, pcz)` iken akışın istediği chunk'ları hesaplar.
/// Her iki liste de `(cx, cz, uzaklık²)` biçimindedir ve yakından uzağa
/// sıralıdır.
///
/// * `mesh` — `render_dist` yarıçaplı dairedeki, çizilecek chunk'lar.
/// * `data` — verisi üretilecek chunk'lar: `mesh`'teki her chunk'ın **8
///   komşusunun tamamı**. Mesh üretimi köşe komşularını da okur (ambient
///   occlusion), bu yüzden veri kümesi yalnızca "bir halka daha geniş daire"
///   olamaz: dairenin kenarındaki chunk'ların çapraz komşuları o dairenin
///   dışında kalır ve bu chunk'lar hiçbir zaman mesh'lenemezdi.
pub fn stream_plan(
    pcx: i32,
    pcz: i32,
    render_dist: i32,
    data: &mut Vec<(i32, i32, i32)>,
    mesh: &mut Vec<(i32, i32, i32)>,
) {
    data.clear();
    mesh.clear();
    let r2 = render_dist * render_dist;
    let reach = render_dist + 1;
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            // Bu chunk'a (Chebyshev) 1 uzaklıktaki chunk'lardan merkeze en
            // yakın olanı daire içindeyse, bir mesh'in komşusudur.
            let nx = (dx.abs() - 1).max(0);
            let nz = (dz.abs() - 1).max(0);
            if nx * nx + nz * nz > r2 {
                continue;
            }
            let d2 = dx * dx + dz * dz;
            data.push((pcx + dx, pcz + dz, d2));
            if d2 <= r2 {
                mesh.push((pcx + dx, pcz + dz, d2));
            }
        }
    }
    data.sort_by_key(|e| e.2);
    mesh.sort_by_key(|e| e.2);
}

/// Ağacı **yalnızca bu chunk'a ait** hücrelere yazar (taşan kısımlar atlanır,
/// komşu chunk kendi taramasında aynı ağacı üretir).
///
/// Gerçekten yazılan en yüksek `y` değerini döndürür; `generate_chunk` bunu
/// `content_top` olarak kullanır (aksi hâlde yaprak tepeleri mesh dışında kalır).
fn place_tree(data: &mut [u8], lx: i32, ly: i32, lz: i32, wx: i32, wz: i32) -> i32 {
    let th = 4 + (hash3f(wx, 3, wz) * 3.0).floor() as i32;
    let mut written_top = 0;

    for i in 0..th {
        let y = ly + i;
        if !(0..CHUNK_SIZE).contains(&lx) || !(0..CHUNK_SIZE).contains(&lz) {
            continue;
        }
        if y < 0 || y >= WORLD_HEIGHT {
            continue;
        }
        data[li(lx as usize, lz as usize, y as usize)] = LOG;
        written_top = written_top.max(y);
    }

    let top = ly + th;
    for dy in -2..=1i32 {
        let r: i32 = if dy <= -1 { 2 } else { 1 };
        for dx in -r..=r {
            for dz in -r..=r {
                if dx == 0 && dz == 0 && dy <= 0 {
                    continue;
                }
                if dx.abs() == r && dz.abs() == r && hash3f(wx + dx, dy, wz + dz) > 0.45 {
                    continue;
                }
                let x = lx + dx;
                let z = lz + dz;
                let y = top + dy;
                if !(0..CHUNK_SIZE).contains(&x) || !(0..CHUNK_SIZE).contains(&z) {
                    continue;
                }
                if y < 0 || y >= WORLD_HEIGHT {
                    continue;
                }
                let idx = li(x as usize, z as usize, y as usize);
                if data[idx] == AIR {
                    data[idx] = LEAVES;
                    written_top = written_top.max(y);
                }
            }
        }
    }

    written_top
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        World::new(20261012)
    }

    #[test]
    fn chunk_key_is_injective_for_nearby_chunks() {
        let mut seen = std::collections::HashSet::new();
        for cz in -40..40 {
            for cx in -40..40 {
                assert!(seen.insert(ckey(cx, cz)), "anahtar çakıştı: {cx},{cz}");
            }
        }
    }

    #[test]
    fn chunk_key_is_injective_far_from_origin() {
        // Eski 18 bitlik paketleme ±131072 chunk'ta (±2 milyon blok) sarıyordu.
        let far = [-200_000, -131_072, -1, 0, 1, 131_072, 200_000, i32::MAX, i32::MIN];
        let mut seen = std::collections::HashSet::new();
        for &cx in &far {
            for &cz in &far {
                assert!(seen.insert(ckey(cx, cz)), "anahtar çakıştı: {cx},{cz}");
            }
        }
    }

    #[test]
    fn local_index_matches_layout() {
        assert_eq!(li(0, 0, 0), 0);
        assert_eq!(li(15, 0, 0), 15);
        assert_eq!(li(0, 1, 0), 16);
        assert_eq!(li(0, 0, 1), 256);
        assert_eq!(li(15, 15, 63), CHUNK_AREA * 64 - 1);
    }

    #[test]
    fn height_stays_within_world_bounds() {
        let w = world();
        for i in 0..20_000 {
            let x = (i * 7919) % 400_000 - 200_000;
            let z = (i * 104_729) % 400_000 - 200_000;
            let h = w.height_at(x, z);
            assert!((4..=WORLD_HEIGHT - 10).contains(&h), "yükseklik dışı: {h}");
        }
    }

    #[test]
    fn height_is_continuous_at_chunk_boundaries() {
        // Chunk sınırı sürekliliği: komşu sütunlar arasında uçurum olmamalı.
        let w = world();
        let mut max_jump = 0;
        for i in 0..4000 {
            let x = (i % 200) * 16 - 1600; // tam chunk sınırları
            let z = (i / 200) * 16 - 1600;
            let a = w.height_at(x, z);
            max_jump = max_jump.max((a - w.height_at(x + 1, z)).abs());
            max_jump = max_jump.max((a - w.height_at(x, z + 1)).abs());
        }
        assert!(max_jump <= 2, "sınırda yükseklik sıçraması: {max_jump}");
    }

    #[test]
    fn generation_is_deterministic_and_order_independent() {
        let mut a = world();
        a.generate_chunk(400, 400);
        let solo = a.chunks.get(&ckey(400, 400)).unwrap().data.clone();

        let mut b = world();
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
            b.generate_chunk(400 + dx, 400 + dz);
        }
        b.generate_chunk(400, 400);
        let with_neighbours = b.chunks.get(&ckey(400, 400)).unwrap().data.clone();

        assert_eq!(solo, with_neighbours, "üretim sırasına bağlı fark var");
    }

    #[test]
    fn generated_chunk_has_bedrock_stone_and_surface() {
        let mut w = world();
        // Deniz seviyesinin belirgin üstünde bir sütun bul: yüzeyi çim olmalı.
        let mut found = None;
        for i in 0..5000 {
            let x = (i % 100) - 50;
            let z = (i / 100) - 50;
            if w.height_at(x, z) > SEA_LEVEL + 2 {
                found = Some((x, z));
                break;
            }
        }
        let (x, z) = found.expect("deniz üstü sütun bulunamadı");
        w.ensure_chunk(x >> 4, z >> 4);
        let h = w.height_at(x, z);

        assert_eq!(w.get_block(x, 0, z), BEDROCK, "en altta ana kaya olmalı");
        assert_eq!(w.get_block(x, h - 5, z), STONE, "derinde taş olmalı");
        assert_eq!(w.get_block(x, h, z), GRASS, "yüzey çim olmalı");
        assert_eq!(w.get_block(x, h + 1, z), AIR, "yüzeyin üstü boş olmalı");
    }

    #[test]
    fn beach_columns_are_sand() {
        let mut w = world();
        let mut found = None;
        for i in 0..8000 {
            let x = (i % 120) - 60;
            let z = (i / 120) - 60;
            let h = w.height_at(x, z);
            if h <= SEA_LEVEL + 1 && h > SEA_LEVEL - 3 {
                found = Some((x, z));
                break;
            }
        }
        if let Some((x, z)) = found {
            w.ensure_chunk(x >> 4, z >> 4);
            let h = w.height_at(x, z);
            assert_eq!(w.get_block(x, h, z), SAND, "kıyı şeridi kum olmalı");
        }
    }

    #[test]
    fn unloaded_chunk_reads_as_air_and_write_fails() {
        let mut w = world();
        assert_eq!(w.get_block(999_999, 30, 999_999), AIR);
        assert!(!w.set_block_raw(999_999, 30, 999_999, STONE));
    }

    #[test]
    fn below_world_is_stone_and_above_is_air() {
        let w = world();
        assert_eq!(w.get_block(0, -5, 0), STONE);
        assert_eq!(w.get_block(0, WORLD_HEIGHT + 5, 0), AIR);
    }

    #[test]
    fn player_edits_survive_unload_and_regenerate() {
        let mut w = world();
        w.generate_chunk(3000 >> 4, 3000 >> 4);
        let h = w.height_at(3000, 3000);
        assert!(w.set_block_raw(3000, h + 1, 3000, BRICK));
        let key = ckey(3000 >> 4, 3000 >> 4);

        w.unload_chunk(key);
        assert!(!w.chunks.contains_key(&key));
        w.generate_chunk(3000 >> 4, 3000 >> 4);

        assert_eq!(w.get_block(3000, h + 1, 3000), BRICK, "oyuncu değişikliği kayboldu");
    }

    /// Mesh'in alt sınırı `min_open`'dan gelir: üretimde en alçak sütunun
    /// hemen üstüdür, oyuncu kazdıkça düşer ve yeniden üretimde korunur.
    #[test]
    fn min_open_tracks_the_lowest_non_opaque_cell() {
        let mut w = world();
        let (cx, cz) = (3000 >> 4, 3000 >> 4);
        w.generate_chunk(cx, cz);
        let key = ckey(cx, cz);

        let mut min_h = WORLD_HEIGHT;
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                min_h = min_h.min(w.height_at(cx * CHUNK_SIZE + x, cz * CHUNK_SIZE + z));
            }
        }
        assert_eq!(w.chunks[&key].min_open, min_h + 1, "üretimde en alçak sütunun üstü olmalı");

        // Opak blok koymak açık hücre oluşturmaz.
        assert!(w.set_block_raw(3000, 2, 3000, BRICK));
        assert_eq!(w.chunks[&key].min_open, min_h + 1);

        // Yüzeyin altında boşluk açmak sınırı aşağı çeker (hava, su ve cam).
        assert!(w.set_block_raw(3000, 9, 3000, GLASS));
        assert_eq!(w.chunks[&key].min_open, 9);
        assert!(w.set_block_raw(3000, 5, 3000, AIR));
        assert_eq!(w.chunks[&key].min_open, 5);

        // Chunk boşaltılıp yeniden üretilince düzenlemelerden geri hesaplanır.
        w.unload_chunk(key);
        w.generate_chunk(cx, cz);
        assert_eq!(w.chunks[&key].min_open, 5, "yeniden üretimde alt sınır kayboldu");
    }

    /// Mesh'lenecek her chunk'ın 8 komşusu da veri kümesinde olmalı; aksi
    /// hâlde görüş dairesinin kenarındaki chunk'lar hiç çizilemez.
    #[test]
    fn stream_plan_requests_every_neighbour_of_every_mesh_chunk() {
        let (mut data, mut mesh) = (Vec::new(), Vec::new());
        for r in 3..=16 {
            stream_plan(7, -4, r, &mut data, &mut mesh);
            let have: std::collections::HashSet<(i32, i32)> =
                data.iter().map(|e| (e.0, e.1)).collect();
            let wanted: std::collections::HashSet<(i32, i32)> =
                mesh.iter().map(|e| (e.0, e.1)).collect();
            assert_eq!(have.len(), data.len(), "veri listesinde yinelenen chunk");

            for &(cx, cz, d2) in &mesh {
                assert!(d2 <= r * r);
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        assert!(
                            have.contains(&(cx + dx, cz + dz)),
                            "r={r}: ({cx},{cz}) mesh'inin komşusu ({},{}) istenmiyor",
                            cx + dx,
                            cz + dz
                        );
                    }
                }
            }

            let keep = (r + KEEP_MARGIN) * (r + KEEP_MARGIN);
            for &(cx, cz, d2) in &data {
                // Gereksiz chunk üretilmemeli: her veri chunk'ı bir mesh'e komşu.
                let used = (-1..=1)
                    .any(|dz| (-1..=1).any(|dx| wanted.contains(&(cx + dx, cz + dz))));
                assert!(used, "r={r}: ({cx},{cz}) hiçbir mesh'e komşu değil");
                // ... ve temizlik yarıçapının içinde kalmalı (üret-sil döngüsü olmasın).
                assert!(d2 <= keep, "r={r}: ({cx},{cz}) saklama yarıçapının dışında");
            }

            assert!(data.windows(2).all(|p| p[0].2 <= p[1].2), "veri yakından uzağa değil");
            assert!(mesh.windows(2).all(|p| p[0].2 <= p[1].2), "mesh yakından uzağa değil");
            assert_eq!((mesh[0].0, mesh[0].1), (7, -4), "ilk mesh oyuncunun chunk'ı olmalı");
        }
    }

    #[test]
    fn terrain_exists_far_from_origin() {
        // Sonsuzluk iddiası: ±1.000.000 blok uzakta da yüzey üretilmeli.
        let mut w = world();
        for (x, z) in [(50_000, -50_000), (-123_456, 98_765), (1_000_000, -1_000_000)] {
            let h = w.height_at(x, z);
            w.ensure_chunk(x >> 4, z >> 4);
            let surface = w.get_block(x, h, z);
            assert!(
                surface == GRASS || surface == SAND || surface == DIRT,
                "({x},{z}) yüzeyi beklenmedik: {surface}"
            );
            assert_eq!(w.get_block(x, h + 3, z), AIR);
        }
    }

    #[test]
    fn spawn_is_above_sea_level_and_tree_free() {
        let mut w = world();
        let s = w.find_spawn();
        let (x, z) = (s[0].floor() as i32, s[2].floor() as i32);
        let h = w.height_at(x, z);
        assert!(h > SEA_LEVEL + 1, "doğuş noktası suyun altında: h={h}");
        w.ensure_chunk(x >> 4, z >> 4);
        assert_eq!(w.get_block(x, h + 1, z), AIR, "doğuş noktası dolu");
    }

    #[test]
    fn ensure_around_loads_nine_chunks() {
        let mut w = world();
        w.ensure_around(0.5, 0.5);
        assert_eq!(w.chunks.len(), 9);
        w.ensure_around(0.5, 0.5);
        assert_eq!(w.chunks.len(), 9, "tekrar çağrı yeni chunk üretmemeli");
    }

    #[test]
    fn water_fills_up_to_sea_level_in_ocean_columns() {
        let mut w = world();
        // Deniz seviyesinin altında bir sütun bul.
        let mut found = None;
        for i in 0..4000 {
            let x = (i % 100) * 8 - 400;
            let z = (i / 100) * 8 - 400;
            let h = w.height_at(x, z);
            if h < SEA_LEVEL - 2 {
                found = Some((x, z, h));
                break;
            }
        }
        let (x, z, h) = found.expect("okyanus sütunu bulunamadı");
        w.ensure_chunk(x >> 4, z >> 4);
        assert_eq!(w.get_block(x, h, z), SAND, "deniz tabanı kum olmalı");
        assert_eq!(w.get_block(x, SEA_LEVEL, z), WATER);
        assert_eq!(w.get_block(x, SEA_LEVEL + 1, z), AIR);
    }
}
