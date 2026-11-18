#!/usr/bin/env bash
# VoxelCraft (Rust/WASM) — güvenli başsız (headless) tarayıcı duman testi.
#
# Platformlar: macOS, Linux, Windows (Git Bash / MSYS). Betik ortamı kendisi
# algılar: Chrome yolunu, port→PID sorgusunu ve süreç kapatmayı buna göre seçer.
#
# NEDEN BU BETİK VAR
#   Chrome'u görüntü adına göre topluca kapatmayın (`taskkill /F /IM chrome.exe`
#   ya da `pkill chrome`): makinede açık olan KULLANICI pencerelerini de kapatır.
#   Bu betik yalnızca kendi başlattığı Chrome süreç ağacını — CDP portunu
#   dinleyen PID üzerinden — kapatır. Aynı şekilde yalnızca kendi başlattığı
#   HTTP sunucusunu kapatır.
#
# KULLANIM
#   ./tools/smoke.sh              # varsayılan 8799 portu
#   ./tools/smoke.sh 8811         # özel HTTP portu
#
# ORTAM DEĞİŞKENLERİ
#   CHROME    tarayıcı ikilisinin tam yolu (verilmezse otomatik aranır)
#   CDP_PORT  DevTools portu (varsayılan: 9334)
#
# GEREKSİNİM: ./build.sh çalıştırılmış olmalı (pkg/ hazır) + Node.js + Chrome.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

HTTP_PORT="${1:-8799}"
CDP_PORT="${CDP_PORT:-9334}"
TARGET="http://127.0.0.1:${HTTP_PORT}/index.html"

case "$(uname -s)" in
  Darwin) OS=mac ;;
  MINGW* | MSYS* | CYGWIN*) OS=windows ;;
  *) OS=linux ;;
esac

# --- Chrome / Chromium ikilisini bul -----------------------------------------
# (Dizi yerine satır listesi kullanılıyor: macOS'un varsayılan bash 3.2'si
#  `set -u` altında boş dizi genişletmesinde hata verir.)
find_chrome() {
  if [ -n "${CHROME:-}" ]; then
    printf '%s\n' "$CHROME"
    return 0
  fi
  local c
  local list=""
  case "$OS" in
    mac)
      list="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome
$HOME/Applications/Google Chrome.app/Contents/MacOS/Google Chrome
/Applications/Chromium.app/Contents/MacOS/Chromium
/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge
/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"
      ;;
    windows)
      list="/c/Program Files/Google/Chrome/Application/chrome.exe
/c/Program Files (x86)/Google/Chrome/Application/chrome.exe
${LOCALAPPDATA:-/c/Users/Default/AppData/Local}/Google/Chrome/Application/chrome.exe"
      ;;
  esac
  while IFS= read -r c; do
    if [ -n "$c" ] && [ -x "$c" ]; then
      printf '%s\n' "$c"
      return 0
    fi
  done <<EOF
$list
EOF
  for c in google-chrome google-chrome-stable chromium chromium-browser microsoft-edge; do
    if command -v "$c" >/dev/null 2>&1; then
      command -v "$c"
      return 0
    fi
  done
  return 1
}

CHROME="$(find_chrome || true)"
if [ -z "$CHROME" ]; then
  echo "Chrome bulunamadı. CHROME=/tam/yol olarak belirtin." >&2
  exit 1
fi

PYTHON="$(command -v python3 || command -v python || true)"

PROFILE="$(mktemp -d)/cprof-smoke"
HTTP_LOG="$(mktemp)"
CHROME_LOG="$(mktemp)"

# Belirtilen yerel portu dinleyen sürecin PID'i (yoksa boş).
pid_on_port() {
  # Windows'ta önce netstat: Git Bash'e eklenen bir `lsof` TCP soketlerini
  # görmeyebilir; macOS/Linux'ta ise lsof daha güvenilir.
  if [ "$OS" = windows ] && command -v netstat >/dev/null 2>&1; then
    netstat -ano 2>/dev/null | grep "127.0.0.1:$1" | grep LISTENING | awk '{print $5}' | head -1
    return
  fi
  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"$1" -sTCP:LISTEN -t 2>/dev/null | head -1
    return
  fi
  if command -v ss >/dev/null 2>&1; then
    ss -ltnp 2>/dev/null | grep ":$1 " | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | head -1
    return
  fi
  netstat -ano 2>/dev/null | grep "127.0.0.1:$1" | grep LISTENING | awk '{print $5}' | head -1
}

# Süreci ve alt süreçlerini kapatır (Chrome tek süreç değildir).
kill_tree() {
  local pid="$1"
  [ -n "$pid" ] || return 0
  if [ "$OS" = windows ]; then
    taskkill //F //T //PID "$pid" >/dev/null 2>&1 || true
    return 0
  fi
  local kid
  for kid in $(pgrep -P "$pid" 2>/dev/null || true); do
    kill_tree "$kid"
  done
  kill -TERM "$pid" 2>/dev/null || true
  local i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$pid" 2>/dev/null || return 0
    sleep 0.2
  done
  kill -9 "$pid" 2>/dev/null || true
}

cleanup() {
  local code=$?
  trap - EXIT INT TERM

  # Chrome: yalnızca bizim örneğimiz — CDP portunu dinleyen süreç ağacı.
  # (Git Bash'te `$!` değeri Windows PID'i DEĞİLDİR; bu yüzden port kullanılır.)
  local cpid
  cpid="$(pid_on_port "$CDP_PORT")"
  if [ -n "$cpid" ]; then
    echo "==> Chrome kapatılıyor (yalnızca PID $cpid ağacı)"
    kill_tree "$cpid"
  fi

  # HTTP sunucusu: yalnızca bizim örneğimiz.
  local hpid
  hpid="$(pid_on_port "$HTTP_PORT")"
  if [ -n "$hpid" ]; then
    kill_tree "$hpid"
  fi

  rm -rf "$PROFILE" "$HTTP_LOG" "$CHROME_LOG" 2>/dev/null || true
  exit "$code"
}
trap cleanup EXIT INT TERM

if [ ! -f pkg/voxelcraft_rs_bg.wasm ]; then
  echo "pkg/ eksik — önce ./build.sh çalıştırın." >&2
  exit 1
fi

if ! command -v node >/dev/null 2>&1; then
  echo "Node.js gerekli (test betiği JavaScript). macOS: brew install node" >&2
  exit 1
fi

if [ -n "$(pid_on_port "$HTTP_PORT")" ]; then
  echo "UYARI: $HTTP_PORT zaten kullanımda, mevcut sunucu kullanılacak." >&2
elif [ -n "$PYTHON" ]; then
  echo "==> http://127.0.0.1:$HTTP_PORT sunuluyor"
  "$PYTHON" -m http.server "$HTTP_PORT" --bind 127.0.0.1 >"$HTTP_LOG" 2>&1 &
  sleep 1
elif command -v npx >/dev/null 2>&1; then
  echo "==> http://127.0.0.1:$HTTP_PORT sunuluyor (npx http-server)"
  npx --yes http-server@14 . -a 127.0.0.1 -p "$HTTP_PORT" -c-1 --silent >"$HTTP_LOG" 2>&1 &
  sleep 3
else
  echo "Python bulunamadı — HTTP sunucusu başlatılamıyor." >&2
  exit 1
fi

echo "==> başsız Chrome: $CHROME"
echo "    (izole profil: $PROFILE)"
"$CHROME" --headless=new --remote-debugging-port="$CDP_PORT" \
  --enable-unsafe-swiftshader --no-first-run --no-default-browser-check \
  --user-data-dir="$PROFILE" --window-size=1400,860 "$TARGET" >"$CHROME_LOG" 2>&1 &
sleep 3

CDP_PORT="$CDP_PORT" TARGET_URL="$TARGET" node tools/headless_test.mjs
