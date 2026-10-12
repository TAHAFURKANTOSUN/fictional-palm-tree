// Tek dosyalık, çift tıklanabilir sürüm üretir.
//
// `index.html` + `pkg/` ikilisi bir HTTP sunucusu gerektirir (wasm `fetch` ile
// yüklenir ve tarayıcılar `file://` üzerinde CORS'a izin vermez). Bu betik
// aynı oyunu **tek bir HTML dosyasına** paketler: JS bağlayıcısı satır içine
// gömülür, wasm base64 olarak taşınır ve `file://` üzerinden çalışır.
//
// Kullanım:
//   ./build.sh                       # önce pkg-nomodules üretilmeli
//   node tools/build_single_file.mjs # -> voxelcraft-rs.html

import fs from 'fs';
import path from 'path';

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')), '..');
const gluePath = path.join(root, 'pkg-nomodules', 'voxelcraft_rs.js');
const wasmPath = path.join(root, 'pkg-nomodules', 'voxelcraft_rs_bg.wasm');
const htmlPath = path.join(root, 'index.html');
const outPath = path.join(root, 'voxelcraft-rs.html');

for (const p of [gluePath, wasmPath, htmlPath]) {
  if (!fs.existsSync(p)) {
    console.error('Eksik dosya:', p);
    console.error('Önce `./build.sh` çalıştırın (no-modules çıktısı gerekir).');
    process.exit(1);
  }
}

const glue = fs.readFileSync(gluePath, 'utf8');
const wasmB64 = fs.readFileSync(wasmPath).toString('base64');
let html = fs.readFileSync(htmlPath, 'utf8');

// Satır içi <script> içinde "</script" dizisi bloğu erken kapatır.
const safeGlue = glue.replace(/<\/script/gi, '<\\/script');

const banner = `<!--
  VoxelCraft (Rust + WebAssembly) — tek dosyalık sürüm.
  Bu dosya otomatik üretildi: tools/build_single_file.mjs
  Rust kaynağı: src/  ·  Derleme: ./build.sh  ·  Ayrıntılar: README.md
-->`;

const moduleStart = html.indexOf('<script type="module">');
const moduleEnd = html.indexOf('</script>', moduleStart);
if (moduleStart < 0 || moduleEnd < 0) {
  console.error('index.html içinde module script bulunamadı.');
  process.exit(1);
}

const inline = `<script>
${safeGlue}
</script>

<script>
(function () {
  const loadingText = document.getElementById('loadingText');
  const errEl = document.getElementById('err');

  // requestPointerLock kullanıcı hareketi olmadan reddedilir; reddi yutuyoruz.
  window.__requestLock = function (el) {
    try {
      const p = el && el.requestPointerLock && el.requestPointerLock();
      if (p && typeof p.catch === 'function') p.catch(() => {});
    } catch (e) { /* fare kilidi desteklenmiyor */ }
  };

  const B64 = "${wasmB64}";
  const bin = atob(B64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);

  loadingText.textContent = 'Motor yükleniyor…';
  wasm_bindgen({ module_or_path: bytes }).then(function () {
    window.__vcRs = wasm_bindgen;
    loadingText.textContent = 'Dünya oluşturuluyor…';
    wasm_bindgen.start();
  }).catch(function (e) {
    loadingText.textContent = 'Başlatılamadı';
    errEl.textContent = String(e && e.message ? e.message : e);
    console.error(e);
  });
})();
</script>`;

html = html.slice(0, moduleStart) + inline + html.slice(moduleEnd + '</script>'.length);
html = html.replace('<!DOCTYPE html>', '<!DOCTYPE html>\n' + banner);
html = html.replace(
  '<title>VoxelCraft — Rust + WebAssembly</title>',
  '<title>VoxelCraft — Rust + WebAssembly (tek dosya)</title>'
);

fs.writeFileSync(outPath, html);
const kb = (Buffer.byteLength(html) / 1024).toFixed(0);
console.log(`Yazıldı: ${outPath} (${kb} KB, gömülü wasm ${(wasmB64.length / 1024).toFixed(0)} KB base64)`);
