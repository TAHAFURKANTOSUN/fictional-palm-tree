//! Sonsuz, chunk tabanlı dünya.
//!
//! Dünya sabit boyutlu değildir: yalnızca oyuncunun çevresindeki chunk'lar
//! bellekte tutulur, gerisi talep üzerine üretilir ve boşaltılır.
//! Oyuncunun yaptığı değişiklikler `edits` haritasında saklanır, böylece bir
//! chunk boşaltılıp yeniden üretildiğinde binalar kaybolmaz.

use std::collections::HashMap;

use crate::blocks::*;
use crate::noise::{hash2f, hash3f, Perlin};

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
    /// Bu chunk'taki bitki ve ekin hücrelerinin `(yerel [x, y, z], blok_id)` önbelleği.
    /// `refresh_torches` her seferinde 16.384 hücreyi taramak yerine doğrudan bunu okur.
    pub plants: Vec<([u8; 3], u8)>,
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
            let old = ch.data[idx];
            ch.data[idx] = v;
            let local = [(x & 15) as u8, y as u8, (z & 15) as u8];
            if is_plant(old) {
                ch.plants.retain(|&(p, _)| p != local);
            }
            if is_plant(v) {
                ch.plants.push((local, v));
            }
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

        // Köyler: yerleşim planı ağaçlardan önce hesaplanır ki evlerin içine
        // ağaç dikilmesin. Evler ağaçlardan sonra yazılır: 9×9'luk alan
        // temizlendiği için taşan yapraklar da silinir.
        let villages = self.villages_near(cx, cz);

        // Ağaçlar 3 blok taşma payıyla taranır: chunk sınırına denk gelen bir
        // ağaç ikiye bölünse de her iki tarafta tutarlı çıkar.
        let terrain_top = content_top;
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
                if villages.iter().any(|v| v.blocks_tree(wx, wz)) {
                    continue;
                }
                content_top = content_top.max(place_tree(&mut data, x, h + 1, z, wx, wz));
            }
        }

        // Köy evleri ve kuyu. Her chunk yalnızca kendi hücrelerine düşen
        // blokları yazar; komşu chunk aynı planı yeniden üretir.
        if !villages.is_empty() {
            let mut writer = Writer { data: &mut data, x0, z0, top: content_top };
            for v in &villages {
                self.write_village(&mut writer, v);
            }
            content_top = writer.top;
        }

        // Yabani bitkiler: çimenlerde ot, gelincik ve karahindiba; kıyı kumunda
        // şeker kamışı. En yüksek arazi sütununun üstüne konmaz ki küp mesh'i
        // her zaman chunk'ın en yüksek bloğunu kapsasın.
        let mut plants = Vec::with_capacity(24);
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let wx = x0 + x;
                let wz = z0 + z;
                if villages.iter().any(|v| v.blocks_tree(wx, wz)) {
                    continue;
                }
                let h = self.height_at(wx, wz);
                if h + 2 <= terrain_top {
                    if let Some(plant) = wild_plant_at(wx, wz, h) {
                        let idx = li(x as usize, z as usize, (h + 1) as usize);
                        if data[idx] == AIR {
                            data[idx] = plant;
                            plants.push(([x as u8, (h + 1) as u8, z as u8], plant));
                        }
                    }
                }
            }
        }

        // Ağaçlar zeminin üstündedir; alt sınırı aşağı çekecek açık hücre
        // oluşturmaz. Oyuncu düzenlemeleri ise oluşturabilir.
        let mut min_open = min_h + 1;

        if let Some(ed) = self.edits.get(&key) {
            for (&idx, &v) in ed {
                let old = data[idx];
                data[idx] = v;
                let lx = (idx % CHUNK_SIZE as usize) as u8;
                let lz = ((idx / CHUNK_SIZE as usize) % CHUNK_SIZE as usize) as u8;
                let y = (idx / CHUNK_AREA) as i32;
                let local = [lx, y as u8, lz];
                if is_plant(old) {
                    plants.retain(|&(p, _)| p != local);
                }
                if is_plant(v) {
                    plants.push((local, v));
                }
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
            Chunk { cx, cz, data, meshed: false, content_top, min_open, plants },
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
            if tree || wild_plant_at(xi, zi, h).is_some() {
                continue;
            }
            // Evin içine doğmayalım.
            let cell = (xi.div_euclid(VILLAGE_CELL), zi.div_euclid(VILLAGE_CELL));
            if self.village_at(cell.0, cell.1).is_some_and(|v| v.blocks_tree(xi, zi)) {
                continue;
            }
            return [xi as f64 + 0.5, h as f64 + 1.02, zi as f64 + 0.5];
        }
        let h = self.height_at(0, 0);
        [0.5, h as f64 + 1.02, 0.5]
    }

    // ---- Köyler ----------------------------------------------------------

    /// Bir köy hücresinin köyü (yoksa `None`). Tamamen koordinatlardan
    /// türetilir: aynı hücre her zaman aynı köyü verir, chunk'ın yüklenme
    /// sırası sonucu değiştirmez.
    pub fn village_at(&self, vcx: i32, vcz: i32) -> Option<Village> {
        // Hücrelerin yaklaşık yarısında köy var; gerisi boş kır.
        if hash2f(vcx, vcz, 0x51ad_1f03) > 0.45 {
            return None;
        }
        let cx = vcx * VILLAGE_CELL + 52 + (hash2f(vcx, vcz, 0x2b71_9c17) * 56.0) as i32;
        let cz = vcz * VILLAGE_CELL + 52 + (hash2f(vcx, vcz, 0x77c3_4a1d) * 56.0) as i32;
        let h = self.height_at(cx, cz);
        // Alçak kır: deniz seviyesinin belirgin üstünde, yüksek dağlarda değil.
        if h <= SEA_LEVEL + 2 || h > SEA_LEVEL + 13 {
            return None;
        }
        // Düzlük: köy alanındaki en alçak ve en yüksek sütun birbirine yakın
        // olmalı; evler uçurumun kenarına kurulmaz.
        let (mut lo, mut hi) = (h, h);
        let mut dz = -VILLAGE_RADIUS;
        while dz <= VILLAGE_RADIUS {
            let mut dx = -VILLAGE_RADIUS;
            while dx <= VILLAGE_RADIUS {
                if dx * dx + dz * dz <= VILLAGE_RADIUS * VILLAGE_RADIUS {
                    let hh = self.height_at(cx + dx, cz + dz);
                    lo = lo.min(hh);
                    hi = hi.max(hh);
                }
                dx += 3;
            }
            dz += 3;
        }
        if hi - lo > 3 || lo <= SEA_LEVEL + 1 {
            return None;
        }
        let base = hi;
        // 2–5 ev, merkezin çevresine eşit açılarla (hafif sapmayla) dağıtılır.
        let n = 2 + (hash2f(vcx, vcz, 0x5d31_2b07) * 4.0) as usize;
        let mut houses = Vec::with_capacity(n);
        for i in 0..n {
            let jitter = (hash2f(vcx * 31 + i as i32, vcz * 17 + i as i32, 0x3f21_8c05) - 0.5) * 0.5;
            let a = i as f64 / n as f64 * std::f64::consts::TAU + jitter;
            let r = 12.0 + hash2f(vcx * 13 + i as i32, vcz * 29 + i as i32, 0x6a17_2d03) * 8.0;
            let hx = cx + (a.cos() * r).round() as i32;
            let hz = cz + (a.sin() * r).round() as i32;
            // Evin tabanı: ayak izindeki en yüksek sütun; altı doldurulur.
            let mut hb = self.height_at(hx, hz);
            for ddz in -3..=3 {
                for ddx in -3..=3 {
                    hb = hb.max(self.height_at(hx + ddx, hz + ddz));
                }
            }
            let (sx, sz) = (if cx >= hx { 1 } else { -1 }, if cz >= hz { 1 } else { -1 });
            let (door, out, dir) = if (cx - hx).abs() >= (cz - hz).abs() {
                ([hx + sx * 3, hz], [hx + sx * 4, hz], [sx, 0])
            } else {
                ([hx, hz + sz * 3], [hx, hz + sz * 4], [0, sz])
            };
            houses.push(House { x: hx, z: hz, base: hb, door, out, dir });
        }
        Some(Village { cx, cz, base, houses })
    }

    /// Bu chunk'a taşan köyler. Köy hücresi chunk'tan geniş olduğu için en
    /// fazla dört hücre denetlenir.
    pub fn villages_near(&self, cx: i32, cz: i32) -> Vec<Village> {
        let (x0, z0) = (cx * CHUNK_SIZE, cz * CHUNK_SIZE);
        let m = VILLAGE_RADIUS + 10;
        let (c0x, c1x) = ((x0 - m).div_euclid(VILLAGE_CELL), (x0 + CHUNK_SIZE + m).div_euclid(VILLAGE_CELL));
        let (c0z, c1z) = ((z0 - m).div_euclid(VILLAGE_CELL), (z0 + CHUNK_SIZE + m).div_euclid(VILLAGE_CELL));
        let mut out = Vec::new();
        for vcz in c0z..=c1z {
            for vcx in c0x..=c1x {
                if let Some(v) = self.village_at(vcx, vcz) {
                    out.push(v);
                }
            }
        }
        out
    }

    /// Oyuncunun çevresindeki **gerçekten üretilmiş** evlerin kapı eşikleri —
    /// köylülerin evi. Ev henüz üretilmemişse (chunk yüklü değilse) atlanır.
    pub fn village_homes(&self, x: i32, z: i32, radius: i32) -> Vec<[f64; 3]> {
        let mut out = Vec::new();
        let (c0x, c1x) = ((x - radius).div_euclid(VILLAGE_CELL), (x + radius).div_euclid(VILLAGE_CELL));
        let (c0z, c1z) = ((z - radius).div_euclid(VILLAGE_CELL), (z + radius).div_euclid(VILLAGE_CELL));
        for vcz in c0z..=c1z {
            for vcx in c0x..=c1x {
                let Some(v) = self.village_at(vcx, vcz) else { continue };
                for h in &v.houses {
                    if (h.x - x).abs() > radius || (h.z - z).abs() > radius {
                        continue;
                    }
                    // Kapı boşluğu ve tahta döşeme yerinde olmalı: yalnızca
                    // gerçekten üretilmiş evler köylü barındırır.
                    if self.get_block(h.door[0], h.base, h.door[1]) != PLANKS
                        || self.get_block(h.door[0], h.base + 1, h.door[1]) != AIR
                    {
                        continue;
                    }
                    out.push([h.door[0] as f64 + 0.5, h.base as f64 + 1.0, h.door[1] as f64 + 0.5]);
                }
            }
        }
        out
    }

    /// Köyün kuyusunu ve bütün evlerini bu chunk'a yazar.
    fn write_village(&self, w: &mut Writer, v: &Village) {
        // Kuyu: 3×3 parke taşı bilezik, ortasında su.
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (wx, wz) = (v.cx + dx, v.cz + dz);
                let g = self.height_at(wx, wz);
                for y in (g + 1)..=v.base {
                    w.set(wx, y, wz, DIRT);
                }
                for y in (v.base + 1)..=(v.base + 4) {
                    w.set(wx, y, wz, AIR);
                }
                let b = if dx == 0 && dz == 0 { WATER } else { COBBLE };
                w.set(wx, v.base, wz, b);
                if dx != 0 || dz != 0 {
                    w.set(wx, v.base + 1, wz, COBBLE);
                }
            }
        }
        for h in &v.houses {
            self.write_house(w, h);
        }
    }

    /// Tek bir ev: zemin düzlenir; döşeme, duvarlar, pencereler, kapı, patika
    /// ve çatı yazılır. Chunk dışına düşen hücreler `Writer` tarafından atlanır.
    fn write_house(&self, w: &mut Writer, h: &House) {
        let (hx, hz, base) = (h.x, h.z, h.base);
        // 1) 9×9 alan: altını doldur, üstünü temizle (ağaç, ot, kar).
        for dz in -4..=4 {
            for dx in -4..=4 {
                let (wx, wz) = (hx + dx, hz + dz);
                let g = self.height_at(wx, wz);
                for y in (g + 1)..=base {
                    w.set(wx, y, wz, DIRT);
                }
                for y in (base + 1)..=(base + 7) {
                    w.set(wx, y, wz, AIR);
                }
            }
        }
        // 2) Tahta döşeme.
        for dz in -3..=3 {
            for dx in -3..=3 {
                w.set(hx + dx, base, hz + dz, PLANKS);
            }
        }
        // 3) Duvarlar: köşeler kütük, ilk sıra parke taşı, üstü tahta.
        for dz in -3..=3i32 {
            for dx in -3..=3i32 {
                let (ex, ez) = (dx.abs() == 3, dz.abs() == 3);
                if !ex && !ez {
                    continue;
                }
                let corner = ex && ez;
                for y in 1..=3 {
                    let b = if corner {
                        LOG
                    } else if y == 1 {
                        COBBLE
                    } else {
                        PLANKS
                    };
                    w.set(hx + dx, base + y, hz + dz, b);
                }
                // 4) Pencereler: her duvarda iki cam.
                if (ex && dz.abs() == 2) || (ez && dx.abs() == 2) {
                    w.set(hx + dx, base + 2, hz + dz, GLASS);
                }
            }
        }
        // 5) Kapı: 1×2 boşluk; önünde eşik ve taş patika.
        w.set(h.door[0], base + 1, h.door[1], AIR);
        w.set(h.door[0], base + 2, h.door[1], AIR);
        for i in 1..=4 {
            let (wx, wz) = (h.door[0] + h.dir[0] * i, h.door[1] + h.dir[1] * i);
            w.set(wx, base, wz, COBBLE);
            w.set(wx, base + 1, wz, AIR);
        }
        // 6) Çatı: 9×9 saçak, üstünde 5×5 ikinci kat.
        for dz in -4..=4i32 {
            for dx in -4..=4i32 {
                if dx.abs() == 4 && dz.abs() == 4 {
                    continue;
                }
                w.set(hx + dx, base + 4, hz + dz, PLANKS);
            }
        }
        for dz in -2..=2 {
            for dx in -2..=2 {
                w.set(hx + dx, base + 5, hz + dz, PLANKS);
            }
        }
        // 7) İç meşale, kapı yanında meşale ve ara sıra çalışma tezgahı.
        w.set(hx - 2, base + 1, hz - 2, TORCH);
        let (px, pz) = (h.dir[1], h.dir[0]); // kapı yönünün dikmesi
        w.set(h.out[0] + px, base + 1, h.out[1] + pz, TORCH);
        if hash2f(hx, hz, 0x2f19_6b11) > 0.5 {
            w.set(hx + 2, base + 1, hz + 2, CRAFT_TABLE);
        }
    }
}

// ---- Köyler --------------------------------------------------------------

/// Köy hücresi boyu (blok): her hücrede en fazla bir köy bulunur. Evler
/// hücrenin ortasına yerleştiği için komşu hücrelerin köyleri üst üste binmez.
pub const VILLAGE_CELL: i32 = 160;
/// Köy merkezinden en uzak evin merkezine olan yatay uzaklık (blok).
pub const VILLAGE_RADIUS: i32 = 26;

/// Köydeki tek bir ev. Kapı her zaman köyün merkezine bakar.
#[derive(Clone, Debug)]
pub struct House {
    pub x: i32,
    pub z: i32,
    /// Evin tabanı: tahta döşemenin bulunduğu `y`.
    pub base: i32,
    /// Kapı boşluğunun hücresi.
    pub door: [i32; 2],
    /// Kapının dışındaki basamak (köylünün "evi" buradır).
    pub out: [i32; 2],
    /// Kapının baktığı yön (birim vektör).
    pub dir: [i32; 2],
}

/// Bir köy: merkez, düzlenmiş zemin seviyesi ve evleri.
#[derive(Clone, Debug)]
pub struct Village {
    pub cx: i32,
    pub cz: i32,
    pub base: i32,
    pub houses: Vec<House>,
}

impl Village {
    /// Bu sütuna ağaç ya da bitki dikilmesin mi? (Ev ayak izi + bir blok pay,
    /// kuyu çevresi.)
    pub fn blocks_tree(&self, x: i32, z: i32) -> bool {
        if (x - self.cx).abs() <= 4 && (z - self.cz).abs() <= 4 {
            return true;
        }
        self.houses.iter().any(|h| (x - h.x).abs() <= 5 && (z - h.z).abs() <= 5)
    }
}

/// Chunk verisine yazan küçük yardımcı: chunk dışına düşen hücreleri atlar ve
/// yazılan en yüksek `y` değerini izler (`content_top` için).
struct Writer<'a> {
    data: &'a mut [u8],
    x0: i32,
    z0: i32,
    top: i32,
}

impl Writer<'_> {
    fn set(&mut self, wx: i32, wy: i32, wz: i32, b: u8) {
        let (lx, lz) = (wx - self.x0, wz - self.z0);
        if !(0..CHUNK_SIZE).contains(&lx) || !(0..CHUNK_SIZE).contains(&lz) {
            return;
        }
        if !(0..WORLD_HEIGHT).contains(&wy) {
            return;
        }
        self.data[li(lx as usize, lz as usize, wy as usize)] = b;
        self.top = self.top.max(wy);
    }
}

/// `(wx, wz)` yüzey yüksekliği `h` iken o sütunda doğal olarak çıkan yabani bitki.
fn wild_plant_at(wx: i32, wz: i32, h: i32) -> Option<u8> {
    if (wx, wz) == (-50, -50) {
        return None;
    }
    let r = hash3f(wx, 11, wz);
    if h == SEA_LEVEL || h == SEA_LEVEL + 1 {
        if r > 0.965 { Some(SUGAR_CANE) } else { None }
    } else if h > SEA_LEVEL + 1 {
        if r > 0.985 {
            Some(POPPY)
        } else if r > 0.970 {
            Some(DANDELION)
        } else if r > 0.915 {
            Some(TALL_GRASS)
        } else {
            None
        }
    } else {
        None
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

    #[test]
    fn wild_plants_spawn_on_grass_and_beaches() {
        let mut w = world();
        let (mut grass_plants, mut flowers, mut cane) = (0, 0, 0);
        for cz in -3..=3 {
            for cx in -3..=3 {
                let key = w.generate_chunk(cx, cz);
                let ch = &w.chunks[&key];
                for &b in &ch.data {
                    match b {
                        TALL_GRASS => grass_plants += 1,
                        POPPY | DANDELION => flowers += 1,
                        SUGAR_CANE => cane += 1,
                        _ => {}
                    }
                }
            }
        }
        assert!(grass_plants > 20, "çimenlerde yabani ot üretilmeli: {grass_plants}");
        assert!(flowers > 5, "çimenlerde gelincik ve karahindiba üretilmeli: {flowers}");
        assert!(cane > 0, "kıyılarda şeker kamışı üretilmeli: {cane}");
    }

    /// Yakınlardaki ilk köyü bulur (testler için).
    fn near_village(w: &World) -> Village {
        for vcz in -8..=8 {
            for vcx in -8..=8 {
                if let Some(v) = w.village_at(vcx, vcz) {
                    return v;
                }
            }
        }
        panic!("yakınlarda köy yok");
    }

    /// Köyün çevresindeki bütün chunk'ları üretir.
    fn load_village(w: &mut World, v: &Village) {
        let r = VILLAGE_RADIUS + 8;
        for cz in ((v.cz - r) >> 4)..=((v.cz + r) >> 4) {
            for cx in ((v.cx - r) >> 4)..=((v.cx + r) >> 4) {
                w.generate_chunk(cx, cz);
            }
        }
    }

    #[test]
    fn villages_are_flat_levelled_and_have_doors() {
        let mut w = world();
        let v = near_village(&w);
        load_village(&mut w, &v);

        assert!((2..=5).contains(&v.houses.len()), "ev sayısı: {}", v.houses.len());
        assert!(v.base > SEA_LEVEL + 1, "köy deniz seviyesinin üstünde olmalı");
        for h in &v.houses {
            // Döşeme, duvarlar, kapı boşluğu ve çatı.
            assert_eq!(w.get_block(h.x, h.base, h.z), PLANKS, "döşeme yok");
            assert_eq!(w.get_block(h.door[0], h.base + 1, h.door[1]), AIR, "kapı kapalı");
            assert_eq!(w.get_block(h.door[0], h.base + 2, h.door[1]), AIR, "kapı alçak");
            assert_eq!(w.get_block(h.door[0], h.base + 3, h.door[1]), PLANKS, "kapı üstü açık");
            assert_eq!(w.get_block(h.x, h.base + 4, h.z), PLANKS, "çatı yok");
            assert_eq!(w.get_block(h.x - 3, h.base + 2, h.z - 3), LOG, "köşe kütüğü yok");
            // Ev havada asılı kalmamalı: döşemenin altı dolu.
            for dz in -3..=3 {
                for dx in -3..=3 {
                    let (x, z) = (h.x + dx, h.z + dz);
                    let below = w.get_block(x, h.base - 1, z);
                    assert!(is_solid(below), "({x},{z}) evin altı boş: {below}");
                }
            }
        }
        // Kuyu: ortasında su, çevresinde taş bilezik.
        if v.houses.len() >= 3 {
            assert_eq!(w.get_block(v.cx, v.base, v.cz), WATER, "kuyuda su yok");
            assert_eq!(w.get_block(v.cx + 1, v.base + 1, v.cz), COBBLE, "kuyu bileziği yok");
        }
    }

    #[test]
    fn villages_keep_trees_and_plants_out_of_houses() {
        let mut w = world();
        let v = near_village(&w);
        load_village(&mut w, &v);
        for h in &v.houses {
            // Yalnızca iç mekân: duvarlar (|dx| = 3) kütük ve tahtadır.
            for dz in -2..=2 {
                for dx in -2..=2 {
                    let (x, z) = (h.x + dx, h.z + dz);
                    for y in (h.base + 1)..=(h.base + 3) {
                        let b = w.get_block(x, y, z);
                        assert!(
                            b != LOG && b != LEAVES,
                            "evin içine ağaç girmiş: ({x},{y},{z})"
                        );
                        assert!(!is_plant(b), "evin içine bitki dikilmiş: ({x},{y},{z})");
                    }
                }
            }
        }
    }

    #[test]
    fn village_plan_does_not_depend_on_generation_order() {
        let mut a = world();
        let v = near_village(&a);
        let (cx, cz) = (v.cx >> 4, v.cz >> 4);
        a.generate_chunk(cx, cz);
        let solo = a.chunks[&ckey(cx, cz)].data.clone();

        let mut b = world();
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1)] {
            b.generate_chunk(cx + dx, cz + dz);
        }
        b.generate_chunk(cx, cz);
        assert_eq!(solo, b.chunks[&ckey(cx, cz)].data, "köy üretimi sıraya bağlı");
    }

    #[test]
    fn village_homes_are_the_doorsteps() {
        let mut w = world();
        let v = near_village(&w);
        load_village(&mut w, &v);
        let homes = w.village_homes(v.cx, v.cz, VILLAGE_RADIUS + 8);
        assert_eq!(homes.len(), v.houses.len(), "her evin bir kapı eşiği olmalı");
        for (home, h) in homes.iter().zip(v.houses.iter()) {
            assert!((home[0] - (h.door[0] as f64 + 0.5)).abs() < 1e-9);
            assert!((home[2] - (h.door[1] as f64 + 0.5)).abs() < 1e-9);
            assert!((home[1] - (h.base as f64 + 1.0)).abs() < 1e-9);
        }
        assert!(w.village_homes(v.cx + 400, v.cz, 8).is_empty(), "uzakta ev yok");
        // Chunk'lar yüklü değilken ev sayılmaz (hayalet köylü doğmasın).
        let empty = world();
        assert!(empty.village_homes(v.cx, v.cz, VILLAGE_RADIUS + 8).is_empty());
    }
}
