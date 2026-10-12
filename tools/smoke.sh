#!/usr/bin/env bash
# VoxelCraft (Rust/WASM) — güvenli başsız (headless) tarayıcı duman testi.
#
# NEDEN BU BETİK VAR
#   Chrome'u kapatmak için `taskkill /F /IM chrome.exe` KULLANMAYIN.
#   `/IM` görüntü adına göre eşleşir; makinede açık olan KULLANICI Chrome
#   pencerelerini de zorla kapatır. Bu betik yalnızca kendi başlattığı Chrome
#   süreç ağacını — CDP portunu dinleyen PID üzerinden — kapatır.
#   Aynı şekilde yalnızca kendi başlattığı HTTP sunucusunu kapatır.
#
# KULLANIM
#   ./tools/smoke.sh              # varsayılan 8799 portu
#   ./tools/smoke.sh 8811         # özel HTTP portu
#
# ORTAM DEĞİŞKENLERİ
#   CHROME    chrome.exe yolu (varsayılan: Program Files)
#   CDP_PORT  DevTools portu (varsayılan: 9334)
#
# GEREKSİNİM: ./build.sh çalıştırılmış olmalı (pkg/ hazır).

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

HTTP_PORT="${1:-8799}"
CDP_PORT="${CDP_PORT:-9334}"
CHROME="${CHROME:-/c/Program Files/Google/Chrome/Application/chrome.exe}"
TARGET="http://127.0.0.1:${HTTP_PORT}/index.html"

PROFILE="$(mktemp -d)/cprof-smoke"
HTTP_LOG="$(mktemp)"
CHROME_LOG="$(mktemp)"

# Belirtilen yerel portu dinleyen sürecin Windows PID'i (yoksa boş).
pid_on_port() {
  netstat -ano 2>/dev/null | grep "127.0.0.1:$1" | grep LISTENING | awk '{print $5}' | head -1
}

cleanup() {
  local code=$?
  trap - EXIT INT TERM

  # Chrome: yalnızca bizim örneğimiz — CDP portunu dinleyen süreç ağacı.
  # (MSYS bash'in `$!` değeri Windows PID'i DEĞİLDİR; bu yüzden port kullanılır.)
  local cpid
  cpid="$(pid_on_port "$CDP_PORT")"
  if [ -n "$cpid" ]; then
    echo "==> Chrome kapatılıyor (yalnızca PID $cpid ağacı)"
    taskkill //F //T //PID "$cpid" >/dev/null 2>&1 || true
  fi

  # HTTP sunucusu: yalnızca bizim örneğimiz.
  local hpid
  hpid="$(pid_on_port "$HTTP_PORT")"
  if [ -n "$hpid" ]; then
    taskkill //F //PID "$hpid" >/dev/null 2>&1 || true
  fi

  rm -rf "$PROFILE" "$HTTP_LOG" "$CHROME_LOG" 2>/dev/null || true
  exit "$code"
}
trap cleanup EXIT INT TERM

if [ ! -f pkg/voxelcraft_rs_bg.wasm ]; then
  echo "pkg/ eksik — önce ./build.sh çalıştırın." >&2
  exit 1
fi

if [ -n "$(pid_on_port "$HTTP_PORT")" ]; then
  echo "UYARI: $HTTP_PORT zaten kullanımda, mevcut sunucu kullanılacak." >&2
else
  echo "==> http://127.0.0.1:$HTTP_PORT sunuluyor"
  python -m http.server "$HTTP_PORT" --bind 127.0.0.1 >"$HTTP_LOG" 2>&1 &
  sleep 1
fi

echo "==> başsız Chrome (izole profil: $PROFILE)"
"$CHROME" --headless=new --remote-debugging-port="$CDP_PORT" \
  --enable-unsafe-swiftshader --no-first-run --no-default-browser-check \
  --user-data-dir="$PROFILE" --window-size=1400,860 "$TARGET" >"$CHROME_LOG" 2>&1 &
sleep 3

CDP_PORT="$CDP_PORT" TARGET_URL="$TARGET" node tools/headless_test.mjs
