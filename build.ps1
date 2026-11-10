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
cargo build --release --target $WasmTarget
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
    Write-Host "==> http://localhost:$Port üzerinde sunuluyor (Ctrl+C ile durdurun)"
    $Py = $null
    foreach ($cand in @("python", "py", "python3")) {
        try {
            $null = & $cand --version 2>$null
            if ($LASTEXITCODE -eq 0) { $Py = $cand; break }
        } catch {}
    }
    if ($Py) {
        & $Py -m http.server $Port
    } elseif (Get-Command node -ErrorAction SilentlyContinue) {
        node -e "const http=require('http'),fs=require('fs'),path=require('path'),mime={'.html':'text/html;charset=utf-8','.js':'text/javascript;charset=utf-8','.wasm':'application/wasm','.css':'text/css','.json':'application/json'};http.createServer((q,s)=>{let u=decodeURIComponent(q.url.split('?')[0]);if(u==='/')u='/index.html';const f=path.join(process.cwd(),u);fs.readFile(f,(e,d)=>{if(e){s.writeHead(404);s.end('Not found');return;}s.writeHead(200,{'Content-Type':mime[path.extname(f)]||'application/octet-stream'});s.end(d);});}).listen($Port);"
    } else {
        Write-Error "Python veya Node bulunamadı."
        exit 1
    }
}
