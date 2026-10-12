# VoxelCraft (Rust + WebAssembly) — Ayrıntılı Kurulum Rehberi

Bu rehber, projeyi **Windows** üzerinde sıfırdan derleyip çalıştırmayı adım
adım anlatır. Sonunda, gerçekten yaşanmış hatalar dâhil, karşılaşılabilecek
sorunlar ve çözümleri yer alır. Linux/macOS için kısa notlar en sondadır.

---

## 0. Sadece oynamak mı istiyorsunuz?

Hiçbir şey kurmanıza gerek yok. Proje klasöründeki **`voxelcraft-rs.html`**
dosyasına çift tıklayın. Wasm dosyanın içine gömülüdür, sunucu gerekmez.
Oyun açılınca ekrana bir kez tıklayarak fareyi kilitleyin.

> `voxelcraft-rs.html` bir derleme çıktısıdır ve `.gitignore` içindedir.
> Projeyi git'ten yeni klonladıysanız bu dosya yoktur; aşağıdaki adımlarla
> üretmeniz gerekir.

Rust kodunu değiştirmek veya yeniden derlemek istiyorsanız devam edin.

---

## 1. Gereken araçlar

| Araç | Ne için | Zorunlu mu? |
|---|---|---|
| **Rust (rustup + cargo)** | Kodu derlemek ve testleri çalıştırmak | Evet |
| **`wasm32-unknown-unknown` hedefi** | Tarayıcı için WebAssembly üretmek | Evet |
| **wasm-bindgen-cli 0.2.129** | Wasm ile JavaScript arasındaki bağlayıcıyı üretmek | Evet |
| **Node.js (LTS)** | Tek dosyalık `voxelcraft-rs.html` dosyasını paketlemek | Hayır, ama önerilir |
| **Python 3** | `index.html` için yerel HTTP sunucusu | Hayır, Node veya başka bir sunucu da olur |
| **Git Bash** | `build.sh` betiğini çalıştırmak | Hayır, PowerShell komutları aşağıda |

Komutların tamamı **PowerShell** içindir. Komutları proje klasöründe
çalıştırın:

```powershell
cd C:\Users\<kullanıcı>\Downloads\voxelcraft-rs
```

---

## 2. Rust'ı kurun

1. <https://rustup.rs> adresinden `rustup-init.exe` dosyasını indirip çalıştırın.
2. Kurulum bitince PowerShell'i **kapatıp yeniden açın**. PATH ancak bu şekilde güncellenir.
3. Kontrol edin:

```powershell
rustc --version
cargo --version
rustup show          # etkin toolchain'i gösterir
```

> **MSVC mi GNU mu?** rustup varsayılan olarak `x86_64-pc-windows-msvc`
> kurar ve bu sürüm Visual Studio Build Tools ister. `x86_64-pc-windows-gnu`
> toolchain'i bu projeyi derlemek için yeterlidir. Ancak `cargo install` ile
> bazı araçları kaynaktan derlerken MinGW araçları (`dlltool.exe`, `gcc`)
> gerekebilir (bkz. [Hata 3](#hata-3-dlltoolexe-program-not-found)). Bu
> rehber o adımı hazır ikiliyle atlar, bu yüzden iki toolchain de olur.

---

## 3. WebAssembly hedefini ekleyin

```powershell
rustup target add wasm32-unknown-unknown
```

Beklenen çıktı `installed` veya `is up to date` olmalıdır.

---

## 4. wasm-bindgen-cli 0.2.129'u kurun

> ⚠️ Sürüm **tam olarak 0.2.129** olmalıdır. `Cargo.lock` içindeki
> `wasm-bindgen` kütüphanesi bu sürüme sabitlenmiştir. CLI farklı bir
> sürümdeyse derleme "schema version mismatch" hatasıyla durur
> (bkz. [Hata 5](#hata-5-schema-version-mismatch)).

### Yol A — Hazır ikili (önerilen, en hızlısı)

Herhangi bir derleyici veya MinGW gerektirmez:

```powershell
$v    = "0.2.129"
$name = "wasm-bindgen-$v-x86_64-pc-windows-msvc"
$tmp  = "$env:TEMP\wbg"
New-Item -ItemType Directory -Force $tmp | Out-Null
Invoke-WebRequest "https://github.com/rustwasm/wasm-bindgen/releases/download/$v/$name.tar.gz" -OutFile "$tmp\wbg.tar.gz"
tar -xzf "$tmp\wbg.tar.gz" -C $tmp
Copy-Item "$tmp\$name\wasm-bindgen.exe" "$env:USERPROFILE\.cargo\bin\" -Force
wasm-bindgen --version     # -> wasm-bindgen 0.2.129
```

`Invoke-WebRequest` başarısız olursa dosyayı tarayıcıdan elle indirin.
GitHub'da wasm-bindgen projesinin *Releases* sayfasına gidin, `0.2.129`
sürümünü açın ve `wasm-bindgen-0.2.129-x86_64-pc-windows-msvc.tar.gz`
dosyasını indirin. Ardından `tar` ve `Copy-Item` satırlarını dosyanın indiği
yola göre düzenleyip çalıştırın.

### Yol B — Kaynaktan derleme

```powershell
cargo install wasm-bindgen-cli --version 0.2.129
```

Bu yol birkaç dakika sürer. GNU toolchain'de MinGW kurulu değilse
[Hata 3](#hata-3-dlltoolexe-program-not-found) ile karşılaşırsınız; o
durumda Yol A'yı kullanın.

---

## 5. (İsteğe bağlı) Node.js ve Python

```powershell
winget install OpenJS.NodeJS.LTS
winget install Python.Python.3.12
```

Kurulumdan sonra PowerShell'i **kapatıp yeniden açın**, sonra kontrol edin:

```powershell
node --version
python --version
```

---

## 6. Derleme

PowerShell'de, proje klasöründe şu komutları sırayla çalıştırın. Bu adımlar
`build.sh` betiğinin yaptığı işin aynısıdır:

```powershell
# 1) Motor testleri (saf Rust çekirdeği, tarayıcı gerekmez)
cargo test --lib

# 2) WebAssembly derlemesi (release + LTO, yaklaşık 1 dakika)
cargo build --release --target wasm32-unknown-unknown

# 3) index.html için ES modülü bağlayıcısı -> pkg/
wasm-bindgen --target web --no-typescript --out-dir pkg target/wasm32-unknown-unknown/release/voxelcraft_rs.wasm

# 4) Tek dosyalık sürüm için no-modules bağlayıcısı -> pkg-nomodules/
wasm-bindgen --target no-modules --no-typescript --out-dir pkg-nomodules target/wasm32-unknown-unknown/release/voxelcraft_rs.wasm

# 5) Tek dosyalık HTML'i paketle -> voxelcraft-rs.html (Node gerekir)
node tools/build_single_file.mjs
```

Her adımın başarılı sonucu şöyle görünür:

| Adım | Başarılı çıktı |
|---|---|
| 1 | `test result: ok. 116 passed` |
| 2 | `Finished ... release ... [optimized]` satırı |
| 3–4 | Çıktı yok; `pkg\` ve `pkg-nomodules\` içinde `.js` + `_bg.wasm` dosyaları oluşur |
| 5 | `Yazıldı: ...voxelcraft-rs.html (≈433 KB, ...)` |

Git Bash kullanıyorsanız bu adımların hepsi tek komuttur:

```bash
./build.sh
```

---

## 7. Çalıştırma

**Seçenek 1: Tek dosya.** `voxelcraft-rs.html` dosyasına çift tıklayın.

**Seçenek 2: Yerel sunucu** (`index.html` + `pkg/`):

```powershell
python -m http.server 8080
# veya:  npx serve -l 8080 .
```

Ardından tarayıcıda <http://localhost:8080/index.html> adresini açın. Sunucuyu
`Ctrl+C` ile durdurursunuz.

Tarayıcının **WebGL2** desteklemesi gerekir. Chrome, Edge ve Firefox'un
güncel sürümleri destekler.

Kontrollerin listesi için `README.md` dosyasına bakın.

---

## 8. Karşılaşılabilecek hatalar ve çözümleri

### Hata 1: `cargo.exe` "bu işletim sistemi platformu için geçerli bir uygulama değil"

```
Program 'cargo.exe' failed to run: Belirtilen yürütülebilir dosya bu işletim
sistemi platformu için geçerli bir uygulama değil.
```

(İngilizce: *"The specified executable is not a valid application for this OS platform"*)

**Sebep:** `cargo` komutu, `%USERPROFILE%\.cargo\bin\cargo.exe` adlı küçük bir
**vekil dosyayı** çalıştırır. Bu dosya bozulmuş veya 0 bayta düşmüştür.
Genellikle yarıda kesilen bir `rustup update` ya da antivirüs karantinası
buna yol açar. Gerçek cargo toolchain klasöründe çoğunlukla sağlamdır.

**Teşhis:**

```powershell
where.exe cargo
Get-Item (where.exe cargo)   | Select FullName, Length   # 0 ise vekil bozuk
rustup which cargo
Get-Item (rustup which cargo) | Select FullName, Length   # onlarca MB olmalı
```

**Çözüm:** `.cargo\bin` içindeki bütün vekil dosyalar aslında `rustup.exe`'nin
kopyasıdır. Boş olanların üzerine onu kopyalayın:

```powershell
$bin = "$env:USERPROFILE\.cargo\bin"
Get-ChildItem $bin -Filter *.exe | Where-Object Length -eq 0 | ForEach-Object {
    Copy-Item "$bin\rustup.exe" $_.FullName -Force
    "Onarıldı: $($_.Name)"
}
cargo --version
```

Toolchain içindeki dosya da bozuksa (`rustup which cargo` küçük bir boyut
gösteriyorsa) toolchain'i yeniden kurun:

```powershell
rustup toolchain uninstall stable
rustup toolchain install stable
rustup target add wasm32-unknown-unknown
```

Sorun tekrarlıyorsa *Windows Güvenliği → Virüs ve tehdit koruması → Koruma
geçmişi* bölümüne bakın. Karantinaya alınan dosyayı geri yükleyin, sonra
`%USERPROFILE%\.cargo` ve `%USERPROFILE%\.rustup` klasörlerini dışlamalara
ekleyin.

---

### Hata 2: `'cargo' / 'rustup' / 'node' is not recognized ...`

```
node : The term 'node' is not recognized as the name of a cmdlet ...
```

**Sebep:** Araç kurulu değil ya da kurulumdan sonra PowerShell yeniden
açılmadı, bu yüzden PATH güncellenmedi.

**Çözüm:**
- Aracı kurun (bkz. bölüm 2 ve 5), sonra **PowerShell'i kapatıp yeniden açın**.
- `node` için: Node olmadan da oynayabilirsiniz. 5. adımı atlayın ve
  oyunu yerel sunucuyla açın (bölüm 7, seçenek 2).
- Rust için PATH'i elle kontrol edin: `$env:Path -split ';' | Select-String cargo`
  komutu `...\.cargo\bin` yolunu göstermelidir.

---

### Hata 3: `dlltool.exe: program not found`

```
error: error calling dlltool 'dlltool.exe': program not found
error: could not compile `windows-sys` (lib) due to 1 previous error
error: failed to compile `wasm-bindgen-cli v0.2.129`
```

**Sebep:** `x86_64-pc-windows-gnu` toolchain'inde `cargo install` komutu
`windows-sys` bağımlılığını derlerken MinGW'deki `dlltool.exe` aracını ister.
Bu araç sistemde yoktur.

**Çözüm (önerilen):** Kaynaktan derlemeyi bırakın ve
[4. adım, Yol A](#yol-a--hazır-ikili-önerilen-en-hızlısı) ile hazır ikiliyi
kurun.

Diğer seçenekler:
- **MSVC toolchain'ine geçin:** Visual Studio Build Tools'u ("C++ ile masaüstü
  geliştirme" iş yüküyle) kurun, ardından:
  ```powershell
  rustup toolchain install stable-x86_64-pc-windows-msvc
  rustup default stable-x86_64-pc-windows-msvc
  rustup target add wasm32-unknown-unknown
  ```
- **MinGW-w64 kurun** (ör. MSYS2 ile `pacman -S mingw-w64-x86_64-toolchain`)
  ve `C:\msys64\mingw64\bin` klasörünü PATH'e ekleyin.

> Bu hata yalnızca araç kurulumunu etkiler. **Oyunun kendisi** bundan
> etkilenmez, çünkü wasm hedefi kendi linker'ını (`rust-lld`) kullanır.

---

### Hata 4: `linker 'link.exe' not found` / `linker 'cc' not found`

**Sebep:** `cargo test --lib`, testleri bilgisayarınızın kendi mimarisi için
derler ve bunun için bir linker gerekir. MSVC toolchain'inde Visual Studio
Build Tools kurulu değildir.

**Çözüm:**
- Visual Studio Build Tools'u "C++ ile masaüstü geliştirme" iş yüküyle kurun, **veya**
- GNU toolchain'ine geçin. Bu toolchain kendi linker'ını içinde getirir:
  ```powershell
  rustup toolchain install stable-x86_64-pc-windows-gnu
  rustup default stable-x86_64-pc-windows-gnu
  rustup target add wasm32-unknown-unknown
  ```

Testleri atlamak isterseniz `cargo build --release --target wasm32-unknown-unknown`
komutu linker sorunu olmadan da çalışır.

---

### Hata 5: schema version mismatch

```
it looks like the Rust project used to create this wasm file was linked against
version of wasm-bindgen that uses a different bindgen format than this binary
  rust wasm file schema version: 0.2.129
     this binary schema version: 0.2.xxx
```

**Sebep:** `wasm-bindgen` CLI sürümü, `Cargo.lock` içindeki kütüphane
sürümüyle aynı değil.

**Çözüm:** CLI'yi tam olarak 0.2.129 sürümüyle yeniden kurun (bölüm 4).
Kurulu sürümü `wasm-bindgen --version` ile kontrol edin. PATH'te birden fazla
kopya olabilir; `where.exe wasm-bindgen` ile hepsini görürsünüz.

---

### Hata 6: `can't find crate for 'core'` / `target may not be installed`

```
error[E0463]: can't find crate for `core`
  = note: the `wasm32-unknown-unknown` target may not be installed
```

**Çözüm:**

```powershell
rustup target add wasm32-unknown-unknown
```

Birden fazla toolchain varsa hedefi **etkin olan** toolchain'e ekleyin.
Hangisinin etkin olduğunu `rustup show` gösterir.

---

### Hata 7: `lock file version 4 requires -Znext-lockfile-bump`

**Sebep:** `Cargo.lock` dosyasının 4. sürüm biçimi için Cargo 1.78 veya üstü
gerekir.

**Çözüm:**

```powershell
rustup update stable
```

---

### Hata 8: `index.html` boş ekran veya CORS / `Failed to fetch` hatası

```
Access to fetch at 'file:///.../voxelcraft_rs_bg.wasm' from origin 'null'
has been blocked by CORS policy
```

**Sebep:** `index.html` dosyası çift tıklanarak (`file://`) açılmış.
Tarayıcılar yerel dosyalardan `fetch` ile wasm yüklemeye izin vermez.

**Çözüm:** Bir HTTP sunucusu kullanın (`python -m http.server 8080`) **veya**
`voxelcraft-rs.html` dosyasını açın.

---

### Hata 9: `404` — `pkg/voxelcraft_rs.js` bulunamadı

**Sebep:** `pkg/` klasörü yok. Proje git'ten yeni klonlandıysa bu klasör
`.gitignore` içinde olduğu için gelmez.

**Çözüm:** Bölüm 6'daki 2. ve 3. adımları çalıştırın. Sunucuyu **proje
klasöründe** başlattığınızdan emin olun.

---

### Hata 10: `Eksik dosya: ...pkg-nomodules...` (build_single_file.mjs)

**Sebep:** Tek dosyalık paketleyici `pkg-nomodules/` klasörüne ihtiyaç duyar.

**Çözüm:** Bölüm 6'daki 4. adımı çalıştırın, ardından 5. adımı tekrarlayın.

---

### Hata 11: "WebGL2 desteklenmiyor" / siyah ekran

**Çözüm:**
- Tarayıcıyı güncelleyin.
- Chrome/Edge'de `chrome://settings/system` sayfasında *"Kullanılabilir
  olduğunda grafik hızlandırmayı kullan"* seçeneğini açın.
- `chrome://gpu` sayfasında *WebGL2* satırı "Hardware accelerated" olmalıdır.
- Ekran kartı sürücüsünü güncelleyin.

---

### Hata 12: Fare kilitlenmiyor / kamera dönmüyor

**Sebep:** Tarayıcılar fare kilidine (Pointer Lock) yalnızca bir kullanıcı
tıklamasından sonra izin verir.

**Çözüm:** Oyun ekranına bir kez tıklayın. `ESC` fareyi bırakır; geri almak
için tekrar tıklayın.

---

### Hata 13: `OSError: [WinError 10048]` / port kullanımda

**Sebep:** 8080 portu başka bir program tarafından kullanılıyor.

**Çözüm:** Başka bir port seçin:

```powershell
python -m http.server 8090
```

---

### Hata 14: `python` yazınca Microsoft Store açılıyor

**Sebep:** Windows, `python` komutunu Store'a yönlendiren bir kısayolla gelir.

**Çözüm:** Python'u gerçekten kurun (`winget install Python.Python.3.12`)
veya *Ayarlar → Uygulamalar → Gelişmiş uygulama ayarları → Uygulama yürütme
diğer adları* bölümünden `python.exe` / `python3.exe` kısayollarını kapatın.
`py -m http.server 8080` komutu da çalışır.

---

### Hata 15: `./build.sh` PowerShell'de çalışmıyor

**Sebep:** `build.sh` bir bash betiğidir.

**Çözüm:** Git Bash'te çalıştırın (`./build.sh`) veya bölüm 6'daki PowerShell
komutlarını kullanın.

---

### Hata 16: `Invoke-WebRequest` sertifika/bağlantı hatası veya `tar` bulunamadı

**Çözüm:**
- Eski Windows PowerShell'de TLS 1.2'yi açın:
  `[Net.ServicePointManager]::SecurityProtocol = 'Tls12'`
- Dosyayı tarayıcıdan elle indirin (bölüm 4, Yol A'nın altındaki not).
- `tar` yoksa (Windows 10 1803 öncesi) arşivi 7-Zip ile açın.

---

### Hata 17: `VCRUNTIME140.dll bulunamadı` (wasm-bindgen.exe çalışırken)

**Sebep:** Hazır ikili MSVC ile derlenmiştir ve Visual C++ çalışma zamanına
ihtiyaç duyar.

**Çözüm:** Microsoft'un **Visual C++ Redistributable (x64)** paketini kurun.

---

## 9. Hızlı sağlık kontrolü

Bu komutların hepsi bir sürüm numarası yazdırıyorsa ortam hazırdır:

```powershell
rustc --version
cargo --version
rustup target list --installed     # wasm32-unknown-unknown görünmeli
wasm-bindgen --version             # 0.2.129 olmalı
node --version                     # isteğe bağlı
python --version                   # isteğe bağlı
```

---

## 10. Linux / macOS

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129
./build.sh            # test + derleme + pkg/ + voxelcraft-rs.html
./build.sh serve 8080 # derler ve http://localhost:8080 üzerinde sunar
```

Linux'ta `cargo install` linker hatası verirse `build-essential` paketini
kurun. macOS'ta `xcode-select --install` ile komut satırı araçlarını kurun.
