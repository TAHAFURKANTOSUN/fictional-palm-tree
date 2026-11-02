# Kod İncelemesi ve Optimizasyon Raporu

Bu belge, projenin tamamı (≈6 900 satır Rust + araç betikleri) üzerinde yapılan
kod incelemesinin sonuçlarını, bu çalışmada **yapılan** optimizasyon ve
düzeltmeleri ve **hâlâ eksik olan** noktaları özetler.

---

## 1. Ölçülen kazanımlar

Ölçüm: `cargo run --release --example bench` (yerel, 8 bölgelik görüş
mesafesi; 361 chunk üretimi + 289 chunk mesh kurulumu, aynı tohum, 15 turun en
iyisi). "Önce" değerleri aynı ölçüm betiği değişiklik öncesi koda uyarlanarak
alınmıştır.

| Ölçüt | Önce | Sonra | Değişim |
|---|---:|---:|---:|
| Mesh kurulumu (chunk başına) | 0,100 ms | 0,065 ms | **−%35** (~1,5× hızlı) |
| Chunk üretimi (chunk başına) | 0,046 ms | 0,034 ms | **−%26** |
| Köşe boyutu (GPU) | 44 bayt | 32 bayt | **−%27** |
| İndeks boyutu (GPU) | 4 bayt | 2 bayt* | **−%50** |
| Varsayılan görüşte köşe + indeks belleği | ~28,6 MB | ~20,0 MB | −8,6 MB |
| HUD Canvas2D çağrısı (durağan kare) | ~1 500+ | 0 | kare atlanır |

\* 65 536 köşeyi aşan (patolojik) chunk'larda otomatik olarak 32 bit'e düşülür.

Üretilen geometri (yüz/köşe sayısı) değişmedi; görsel çıktı aynıdır.

Uçtan uca doğrulama: `tools/headless_test.mjs` başsız Chromium'da hem eski hem
yeni sürümle 3'er kez çalıştırıldı — yeni sürümde tüm kontroller geçti, **0
konsol / Promise hatası**. Not: konteynerde GPU olmadığından (SwiftShader) FPS
her iki sürümde de ~20'de sabit kaldı; bu ortamda FPS farkı ölçülemez. GPU
tarafı kazanımlar (bellek, HUD çağrıları) yukarıdaki tabloda hesaplanmıştır.

## 2. Yapılan optimizasyonlar

### Mesh / dünya (`mesh.rs`, `world.rs`)
- **Arazi yükseklikleri önbelleğe alındı.** Mesh kurulumu her chunk için
  18×18 = 324 sütunda 6 oktavlık Perlin gürültüsünü yeniden hesaplıyordu
  (~2 000 gürültü örneği). Yükseklikler artık üretim sırasında
  `Chunk::heights` içine yazılıyor ve mesh bunları okuyor. Ağaç taraması da
  chunk içindeki sütunlar için aynı önbelleği kullanıyor.
- **Interleaved köşe tamponu.** Mesh eskiden 4 ayrı `Vec` (konum, normal, uv,
  renk) üretiyor, `gl.rs` bunları her yüklemede yeni bir `Vec`'e
  birleştiriyordu. Artık köşeler doğrudan GPU düzeninde tek bir `Vec`'e
  yazılıyor; ekstra kopya ve ayırma yok.
- **Kullanılmayan normaller kaldırıldı.** Shader `a_nor`'u hiç okumuyordu
  (yön gölgesi köşe rengine önceden işleniyor). Köşe 11 → 8 float.
- **Mesh tamponları yeniden kullanılıyor.** `build_chunk_mesh_into` +
  `Game::mesh_solid/mesh_liquid`: her mesh için yeni `Vec` ayrılmıyor.
- **Sınır kutusu mesh sırasında tutuluyor.** Kapsayıcı küre için tüm
  köşelerin yeniden taranmasına gerek kalmadı.
- **AO erken çıkışı.** Köşenin iki yan komşusu doluysa çapraz örnek
  okunmuyor (sonuç zaten en karanlık seviye).

### Çizim (`gl.rs`)
- 16 bit indeksler (uygun olduğunda).
- `u_atlas` uniform konumu her karede sorgulanıyordu → başlangıçta bir kez
  ayarlanıyor.
- Görünür chunk listesi her karede yeniden ayrılıyordu → tampon alan
  olarak tutulup yeniden kullanılıyor.

### Arayüz (`hud.rs`)
- **Kirli-durum denetimi.** HUD her karede tamamen silinip yeniden
  çiziliyordu; kalp/açlık ikonları piksel piksel çizildiği için bu, kare
  başına 1 500'ü aşkın wasm→JS çağrısı demekti. Artık ekranda görünen her şeyin
  özeti (`Hud::signature`) hesaplanıyor; değişmediyse kare atlanıyor.
  Sürekli değerler (nefes, vinyet, bildirim saydamlığı) görünür adımlara
  yuvarlanıyor.
- `DebugInfo` artık kare başına 3 `String` ayırmıyor; metinler yalnızca panel
  gerçekten çizilirken üretiliyor.

### Girdi (`app.rs`)
- Basılı tuşlar `HashMap<String, bool>` yerine bit maskesinde tutuluyor
  (kare başına ~14 dize hash'i ve her tuşta bir `String` ayırması yerine bit
  testi). Her fiziksel tuşun kendi biti olduğundan `W`+`↑` davranışı korundu.
- Üç yerde tekrarlanan "mesh kur + GPU'ya yükle" kodu `Game::remesh`
  yardımcısında birleştirildi.

## 3. Düzeltilen hatalar

| # | Önem | Hata | Düzeltme |
|---|---|---|---|
| H1 | **Yüksek** | Arazinin altına kazılan kuyu/tünellerin taban ve duvar yüzleri mesh'e girmiyordu: mesh'in `y` alt sınırı yalnızca arazi yüksekliğinden hesaplanıyordu. Derin kazıda dünyanın "içi" görünüyordu. | Chunk başına en alçak düzenleme (`min_edit_y`) tutuluyor; mesh alt sınırı 3×3 komşuluğun en alçak düzenlemesine göre genişletiliyor. Regresyon testi: `deep_shaft_walls_are_meshed`. |
| H2 | **Orta** | Craft sonucuna elde başka eşya varken (veya yığın taşacakken) tıklanınca malzemeler harcanıyor, ürün kayboluyordu. | `crafting::take_result_into` önce ürünün elde tutulabileceğini doğruluyor. 3 yeni test. |
| H3 | **Orta** | Pencere odağı kaybolunca (Alt+Tab) `keyup` gelmediği için tuşlar basılı kalıyor, oyuncu durmadan yürüyordu. | `blur` dinleyicisi tuş durumunu ve sürüklemeyi sıfırlıyor. |
| H4 | Düşük | `cargo clippy --all-targets` atlas testindeki `0 * ATLAS_W` (deny-by-default `erasing_op`) yüzünden **derlenmiyordu**. | Test düzeltildi; kütüphane kodu her iki hedefte de clippy uyarısız. |
| H5 | Düşük | Başsız testteki yüzme kontrolü (H6) aralıklı olarak başarısız oluyordu: 13. adım oyuncuyu hayatta kalma modunda y=90'a ışınlıyor, oyuncu düşüp ölüyor; yavaş çizimde yeniden doğma yüzme adımına denk gelip oyuncuyu sudan çıkarıyordu. | Test, yüzmeden önce yeniden doğmayı bekliyor. |
| H6 | Düşük | `build.sh serve` Linux/macOS'ta `python` bulunmadığında çalışmıyordu. | `python3` → `python` sırasıyla aranıyor. |

## 4. Eksikler ve öneriler (yapılmadı)

Önem sırasına göre:

### Doğruluk / sağlamlık
1. **Olay işleyicilerinde `borrow_mut()` paniği.** Tüm DOM olay işleyicileri
   `g.borrow_mut()` kullanıyor ve `panic = "abort"` ayarlı. Herhangi bir
   yeniden girişte (ör. ileride senkron tetiklenen bir olay) oyun tamamen
   durur. Hata ayıklama kancalarındaki gibi `try_borrow_mut` ile olayı atlamak
   daha güvenli.
2. **Uzak koordinatlarda hassasiyet.** Köşe konumları dünya koordinatında
   `f32` olarak GPU'ya gidiyor. ~50 000 blok ötesinde titreme (jitter) başlar;
   README'deki "±1 000 000 blok" iddiası yalnızca üretim için geçerli.
   Çözüm: kamera-göreli çizim (chunk başına öteleme uniform'u, köşeler chunk-yerel).
   Ayrıca `ckey` 18 bit sarmal kullanıyor: ±131 072 chunk'ta (~2,1 M blok)
   anahtarlar çakışır.
3. **Saydam katman sıralaması yok.** Su, derinlik yazımı açıkken sırasız
   çiziliyor; su içinden su/cam görüntülerinde nadiren yanlış örtüşme olabilir.
   Uzaktan yakına sıralama veya sıvı geçişinde `depth_mask(false)` önerilir.
4. **Fizik kare hızına bağlı.** Yerçekimi açık Euler ile ve `dt ≤ 0,05`
   kırpılarak uygulanıyor; düşük FPS'te zıplama yüksekliği az da olsa değişir.
   Sabit zaman adımlı (ör. 60 Hz) bir fizik döngüsü önerilir.
5. **`item_def(0)` (Hava) yerleştirilebilir bir eşya** olarak dönüyor;
   `debug_add_item(0, n)` ile envantere "hava" girebilir.

### Performans (sonraki adımlar)
6. **HiDPI desteklenmiyor.** Tuvaller CSS piksel boyutunda; `devicePixelRatio`
   kullanılmadığı için yüksek çözünürlüklü ekranlarda görüntü bulanık.
   (Eklenirken GPU yükü artacağından bir "çözünürlük ölçeği" ayarı önerilir.)
7. **Mesh/üretim ana iş parçacığında.** 4 ms kare bütçesiyle sınırlandırılmış
   olsa da hızlı uçuşta takılma yapabilir. Web Worker (veya `wasm-bindgen-rayon`)
   ile arka plana alınabilir.
8. **Köşe sıkıştırma.** Renk 3×`f32` yerine 4×`u8` (normalize), konum chunk-yerel
   `u8`/`i16` olarak paketlenirse köşe 32 → ~12 bayta iner.
9. **Greedy meshing** düz yüzeylerde üçgen sayısını büyük ölçüde azaltır
   (AO ile birlikte uygulanması daha karmaşıktır).
10. **`wasm-opt`** derleme hattında yok; `wasm-opt -O3` genellikle boyutu
    %10–20 küçültür.

### Özellik / ürün eksikleri
11. **Kalıcı kayıt yok.** Dünya düzenlemeleri (`World::edits`) ve envanter
    sayfa yenilenince kaybolur; tohum da her açılışta rastgele. `localStorage`/
    IndexedDB'ye tohum + düzenleme kaydı önerilir. `edits` haritası da oturum
    boyunca sınırsız büyür.
12. **Tohum seçilemiyor / paylaşılamıyor** (`Math.random`).
13. **Hata ayıklama API'si üretimde açık.** `debug_*` fonksiyonları her derlemede
    dışa aktarılıyor (konsoldan hile mümkün). Bir Cargo özelliği
    (`features = ["debug-api"]`) arkasına alınabilir.
14. **Ölümde envanter korunuyor**, su akışı yok, gece/gündüz yok — tasarım
    kararı olabilir, belgelenmeli.

### Araçlar / süreç
15. **CI yok.** `cargo test`, `cargo clippy` ve wasm derlemesi her push'ta
    otomatik çalışmıyor. Basit bir GitHub Actions iş akışı önerilir.
16. **`tools/smoke.sh` yalnızca Windows'ta çalışıyor** (`taskkill`, `netstat -ano`,
    `chrome.exe` yolu). Linux/macOS dalı eklenmeli.
17. Test kodunda kalan birkaç clippy uyarısı (`field_reassign_with_default`,
    `needless_range_loop`) — işlevsel değil, temizlik.
18. **`rustfmt` hiç uygulanmamış** (`cargo fmt --check` tüm dosyalarda fark
    veriyor). Diff'i boğmamak için bu çalışmada yapılmadı; ayrı bir
    "yalnızca biçimlendirme" commit'i ile uygulanıp CI'a eklenmesi önerilir.
