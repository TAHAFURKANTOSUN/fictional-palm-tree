Param(
    [Parameter(Position=0)][string]$Action = "",
    [Parameter(Position=1)][int]$Port = 8080
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$WasmTarget = "wasm32-unknown-unknown"
$OutDir = "pkg"
$Bindgen = if ($env:WASM_BINDGEN) { $env:WASM_BINDGEN } else { "wasm-bindgen" }

Write-Host "==> Motor testleri (saf Rust çekirdeği)"
cargo test --lib --quiet
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "==> $WasmTarget için derleniyor"
cargo build --lib --release --target $WasmTarget
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "==> JS bağlayıcısı üretiliyor ($OutDir)"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
& $Bindgen --target web --no-typescript --out-dir $OutDir "target/$WasmTarget/release/voxelcraft_rs.wasm"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "==> no-modules çıktısı üretiliyor (tek dosyalık sürüm için)"
New-Item -ItemType Directory -Force -Path "pkg-nomodules" | Out-Null
& $Bindgen --target no-modules --no-typescript --out-dir "pkg-nomodules" "target/$WasmTarget/release/voxelcraft_rs.wasm"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

if (Get-Command node -ErrorAction SilentlyContinue) {
    Write-Host "==> Tek dosyalık sürüm paketleniyor (voxelcraft-rs.html)"
    node tools/build_single_file.mjs
}

$WasmSize = [math]::Round((Get-Item "$OutDir/voxelcraft_rs_bg.wasm").Length / 1KB, 1)
Write-Host "==> Hazır: $OutDir/voxelcraft_rs_bg.wasm (${WasmSize} KB)"

if ($Action -eq "serve") {
    Write-Host "==> Çok oyunculu sunucu + HTTP web sunucusu başlatılıyor (port $Port, Ctrl+C ile durdurun)"
    cargo run --release --bin server -- $Port
}
