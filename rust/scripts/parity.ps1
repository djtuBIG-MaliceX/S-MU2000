# rust/scripts/parity.ps1 — build the Rust port and stage same-named tools into build-rust/
# for the regression harness (tools/run_tests.py finds tools via SMU_BUILD).
#
#   pwsh rust/scripts/parity.ps1            # build release + stage exes
#   pwsh rust/scripts/parity.ps1 -Test      # ... then run the harness against build-rust/
#   pwsh rust/scripts/parity.ps1 -Test -Only piano
param(
    [switch]$Test,
    [string]$Only = ""
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root

cargo build --release --manifest-path rust\Cargo.toml
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Force build-rust | Out-Null
Copy-Item build-rust/target/release/*.exe build-rust/ -Force
Write-Host "staged: $((Get-ChildItem build-rust/*.exe).Name -join ', ')"

if ($Test) {
    $env:SMU_BUILD = "build-rust"
    if ($Only) { python tools/run_tests.py --only $Only }
    else       { python tools/run_tests.py }
    if ($LASTEXITCODE -ne 0) { throw "harness failed (SMU_BUILD=build-rust)" }
}
