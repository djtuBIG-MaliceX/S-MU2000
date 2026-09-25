# build_wasm.ps1 - build the browser target with the local emsdk.
#
#   .\build_wasm.ps1            configure + build gui-wasm and render-wasm
#   .\build_wasm.ps1 -Serve     ...and serve build-wasm/ on :8080
#
# Requires the emsdk at D:\opt\emsdk and ninja at D:\opt\ninja
# (edit the two paths below to move).
param([switch]$Serve)

$ErrorActionPreference = 'Stop'

$emsdk = 'D:\opt\emsdk'
if (-not (Test-Path "$emsdk\emsdk_env.ps1")) { throw "emsdk not found at $emsdk" }

$env:EMSDK = $emsdk
$env:Path = @(
    'D:\opt\ninja',
    $emsdk,
    "$emsdk\node\22.16.0_64bit\bin",
    "$emsdk\python\3.13.3_64bit",
    "$emsdk\upstream\bin",
    "$emsdk\upstream\emscripten"
) + $env:Path

Push-Location $PSScriptRoot
try {
    emcmake cmake -B build-wasm -G Ninja -DCMAKE_BUILD_TYPE=Release
    if ($LASTEXITCODE) { throw "emcmake cmake failed" }
    ninja -C build-wasm gui-wasm render-wasm
    if ($LASTEXITCODE) { throw "ninja failed" }
    Write-Host "`nsmu2000.js / render.js are in build-wasm\."

    if ($Serve) {
        Write-Host "Serving build-wasm\ on http://localhost:8080 (Ctrl+C stops)"
        # not plain `python -m http.server`: no-store headers keep Chrome from
        # serving a stale smu2000.wasm/index.html after a rebuild, and the
        # right Content-Type for .wasm is set explicitly.
        & "$emsdk\python\3.13.3_64bit\python.exe" -c @"
import http.server
class H(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Cache-Control', 'no-store')
        super().end_headers()
H.extensions_map['.wasm'] = 'application/wasm'
http.server.test(HandlerClass=H, port=8080, directory='build-wasm')
"@
    }
}
finally { Pop-Location }
