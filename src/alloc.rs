//! Sayfa ayırıcı.
//!
//! Tüm chunk geometrisi tek bir büyük GPU köşe tamponunda (arena) durur; bu
//! sayede bir çizim geçişi tek bir `multiDrawElements` çağrısıyla yapılabilir.
//! Arena sabit boyutlu sayfalara bölünür ve her chunk'a **bitişik** bir sayfa
//! aralığı verilir. Bu modül yalnızca hangi sayfaların dolu olduğunu tutar
//! (saf Rust; GPU'ya dokunmaz, `cargo test` ile doğrulanır).

/// Boş aralıklar listesi; başlangıca göre sıralı ve birleştirilmiş tutulur.
#[derive(Debug, Clone)]
pub struct PageAlloc {
    /// `(başlangıç, uzunluk)` — hiçbiri boş değil, hiçbiri komşusuna değmez.
    free: Vec<(u32, u32)>,
    capacity: u32,
}

impl PageAlloc {
    pub fn new(capacity: u32) -> Self {
        let free = if capacity > 0 { vec![(0, capacity)] } else { Vec::new() };
        Self { free, capacity }
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Boş sayfa sayısı.
    pub fn free_pages(&self) -> u32 {
        self.free.iter().map(|r| r.1).sum()
    }

    /// `n` bitişik sayfa ayırır (ilk uyan). Yer yoksa `None`.
    pub fn alloc(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return None;
        }
        let i = self.free.iter().position(|&(_, len)| len >= n)?;
        let (start, len) = self.free[i];
        if len == n {
            self.free.remove(i);
        } else {
            self.free[i] = (start + n, len - n);
        }
        Some(start)
    }

    /// `[start, start + n)` aralığını serbest bırakır; komşu boşluklarla
    /// birleştirir.
    pub fn free(&mut self, start: u32, n: u32) {
        if n == 0 {
            return;
        }
        debug_assert!(start + n <= self.capacity, "kapasite dışı serbest bırakma");
        let i = self.free.partition_point(|&(s, _)| s < start);
        debug_assert!(i == 0 || self.free[i - 1].0 + self.free[i - 1].1 <= start, "çift serbest bırakma");
        debug_assert!(i == self.free.len() || start + n <= self.free[i].0, "çift serbest bırakma");

        let joins_prev = i > 0 && self.free[i - 1].0 + self.free[i - 1].1 == start;
        let joins_next = i < self.free.len() && start + n == self.free[i].0;
        match (joins_prev, joins_next) {
            (true, true) => {
                self.free[i - 1].1 += n + self.free[i].1;
                self.free.remove(i);
            }
            (true, false) => self.free[i - 1].1 += n,
            (false, true) => self.free[i] = (start, n + self.free[i].1),
            (false, false) => self.free.insert(i, (start, n)),
        }
    }

    /// Kapasiteyi büyütür; yeni sayfalar boş olarak eklenir.
    pub fn grow(&mut self, new_capacity: u32) {
        if new_capacity <= self.capacity {
            return;
        }
        let (start, n) = (self.capacity, new_capacity - self.capacity);
        self.capacity = new_capacity;
        self.free(start, n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fit_and_exhaustion() {
        let mut a = PageAlloc::new(10);
        assert_eq!(a.alloc(4), Some(0));
        assert_eq!(a.alloc(4), Some(4));
        assert_eq!(a.alloc(4), None);
        assert_eq!(a.alloc(2), Some(8));
        assert_eq!(a.free_pages(), 0);
        assert_eq!(a.alloc(0), None);
    }

    #[test]
    fn freeing_coalesces_neighbours() {
        let mut a = PageAlloc::new(9);
        let (x, y, z) = (a.alloc(3).unwrap(), a.alloc(3).unwrap(), a.alloc(3).unwrap());
        a.free(x, 3);
        a.free(z, 3);
        assert_eq!(a.alloc(4), None, "parçalı boşluk 4 sayfa vermemeli");
        a.free(y, 3);
        assert_eq!(a.free, vec![(0, 9)], "üç boşluk tek aralıkta birleşmeli");
        assert_eq!(a.alloc(9), Some(0));
    }

    #[test]
    fn growth_extends_the_last_free_run() {
        let mut a = PageAlloc::new(4);
        assert_eq!(a.alloc(2), Some(0));
        a.grow(8);
        assert_eq!(a.capacity(), 8);
        assert_eq!(a.alloc(6), Some(2), "kuyruk + yeni sayfalar tek aralık olmalı");
        a.grow(4); // küçültme yok
        assert_eq!(a.capacity(), 8);
    }

    /// Rastgele ayırma/serbest bırakma dizisi, sayfa sayfa tutulan basit bir
    /// modelle karşılaştırılır: çakışma yok, kayıp yok, liste hep birleşik.
    #[test]
    fn randomized_against_bitmap_model() {
        let mut a = PageAlloc::new(64);
        let mut used = vec![false; 64];
        let mut live: Vec<(u32, u32)> = Vec::new();
        let mut seed = 0x2545_f491_u32;
        let mut rnd = |m: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed % m
        };
        for step in 0..5000 {
            if live.is_empty() || rnd(3) != 0 {
                let n = 1 + rnd(9);
                match a.alloc(n) {
                    Some(s) => {
                        for p in s..s + n {
                            assert!(!used[p as usize], "adım {step}: sayfa {p} iki kez verildi");
                            used[p as usize] = true;
                        }
                        live.push((s, n));
                    }
                    None => {
                        // Gerçekten yer olmamalı: hiçbir bitişik n boş sayfa yok.
                        let cap = a.capacity() as usize;
                        let fits = (0..=cap.saturating_sub(n as usize))
                            .any(|s| (s..s + n as usize).all(|p| !used[p]));
                        assert!(!fits, "adım {step}: {n} sayfa sığıyordu ama verilmedi");
                        if a.capacity() < 512 {
                            let c = a.capacity() * 2;
                            a.grow(c);
                            used.resize(c as usize, false);
                        }
                    }
                }
            } else {
                let (s, n) = live.swap_remove(rnd(live.len() as u32) as usize);
                a.free(s, n);
                for p in s..s + n {
                    used[p as usize] = false;
                }
            }
            assert_eq!(a.free_pages() as usize, used.iter().filter(|u| !**u).count());
            for w in a.free.windows(2) {
                assert!(w[0].0 + w[0].1 < w[1].0, "boşluklar sıralı ve birleşik olmalı");
            }
        }
    }
}
