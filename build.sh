#!/usr/bin/env bash
# VoxelCraft (Rust) derleme betiği.
#
# Gereksinimler: rustup + wasm32-unknown-unknown hedefi + wasm-bindgen-cli.
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.129
#
# İsteğe bağlı: PATH'te `wasm-opt` (binaryen) varsa çıktı ayrıca küçültülür
# (~%30 daha küçük .wasm). Yoksa bu adım sessizce atlanır.
#
# Kullanım:
#   ./build.sh          # wasm'ı derler ve pkg/ üretir
#   ./build.sh serve    # derler ve http://localhost:8080 üzerinde sunar
set -euo pipefail

cd "$(dirname "$0")"

WASM_TARGET="wasm32-unknown-unknown"
OUT_DIR="pkg"
BINDGEN="${WASM_BINDGEN:-wasm-bindgen}"

echo "==> Motor testleri (saf Rust çekirdeği)"
cargo test --lib --quiet

echo "==> $WASM_TARGET için derleniyor"
cargo build --release --target "$WASM_TARGET"

echo "==> JS bağlayıcısı üretiliyor ($OUT_DIR)"
mkdir -p "$OUT_DIR"
"$BINDGEN" --target web --no-typescript --out-dir "$OUT_DIR" \
  "target/$WASM_TARGET/release/voxelcraft_rs.wasm"

echo "==> no-modules çıktısı üretiliyor (tek dosyalık sürüm için)"
"$BINDGEN" --target no-modules --no-typescript --out-dir pkg-nomodules \
  "target/$WASM_TARGET/release/voxelcraft_rs.wasm"

# wasm-opt, wasm-bindgen'in ÇIKTISI üzerinde çalışmalıdır (ham derleme çıktısı
# bindgen'in ihtiyaç duyduğu özel bölümleri taşır). Başarısız olursa — örneğin
# eski bir binaryen sürümü yeni wasm özelliklerini tanımazsa — derleme
# bozulmaz, küçültülmemiş dosya kullanılır.
if command -v wasm-opt >/dev/null 2>&1; then
  echo "==> wasm-opt ile küçültülüyor"
  for wasm in "$OUT_DIR/voxelcraft_rs_bg.wasm" pkg-nomodules/voxelcraft_rs_bg.wasm; do
    if wasm-opt -O3 "$wasm" -o "$wasm.opt" 2>/dev/null; then
      mv "$wasm.opt" "$wasm"
    else
      rm -f "$wasm.opt"
      echo "    (wasm-opt başarısız oldu, $wasm küçültülmeden bırakıldı)"
    fi
  done
else
  echo "==> wasm-opt bulunamadı, küçültme atlandı (isteğe bağlı)"
fi

echo "==> Tek dosyalık sürüm paketleniyor (voxelcraft-rs.html)"
if command -v node >/dev/null 2>&1; then
  node tools/build_single_file.mjs
else
  echo "    (node bulunamadı, atlandı)"
fi

echo "==> Hazır: $OUT_DIR/voxelcraft_rs_bg.wasm ($(du -h "$OUT_DIR/voxelcraft_rs_bg.wasm" | cut -f1))"

if [ "${1:-}" = "serve" ]; then
  PORT="${2:-8080}"
  # Linux/macOS'ta komut genellikle `python3`, Windows'ta `python`'dır.
  PYTHON="$(command -v python3 || command -v python || true)"
  if [ -z "$PYTHON" ]; then
    echo "Python bulunamadı; pkg/ dizinini herhangi bir HTTP sunucusuyla sunun." >&2
    exit 1
  fi
  echo "==> http://localhost:$PORT üzerinde sunuluyor (Ctrl+C ile durdurun)"
  "$PYTHON" -m http.server "$PORT"
fi
