# Builds the browser version into web/ (open it through a web server, e.g.
#   python -m http.server 8000 --directory web
# then http://localhost:8000).
#
# Needs, once:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.130 --locked
Set-Location $PSScriptRoot
cmd /c "cargo build --release --target wasm32-unknown-unknown --target-dir target\w --bin openflight 2>&1"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
wasm-bindgen --target web --no-typescript --out-dir web\pkg target\w\wasm32-unknown-unknown\release\openflight.wasm
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# The game file in pieces of 18 MB: the CDN mirror (jsDelivr) serves no file
# over 20 MB, and the page counts its loading percentage by them.
$wasm = [IO.File]::ReadAllBytes("$PSScriptRoot\web\pkg\openflight_bg.wasm")
Get-ChildItem web\pkg -Filter "part*.bin" | Remove-Item
$piece = 18MB
$names = @()
for ($i = 0; $i * $piece -lt $wasm.Length; $i++) {
    $n = [Math]::Min($piece, $wasm.Length - $i * $piece)
    $buf = New-Object byte[] $n
    [Array]::Copy($wasm, $i * $piece, $buf, 0, $n)
    [IO.File]::WriteAllBytes("$PSScriptRoot\web\pkg\part$i.bin", $buf)
    $names += "`"part$i.bin`""
}
"{`"v`": $([DateTimeOffset]::UtcNow.ToUnixTimeSeconds()), `"size`": $($wasm.Length), `"parts`": [$($names -join ', ')]}" | Set-Content -NoNewline -Encoding ascii web\pkg\parts.json
Remove-Item web\pkg\size.txt -ErrorAction SilentlyContinue

$mb = [math]::Round($wasm.Length / 1MB, 1)
Write-Host "Built web\pkg ($mb MB in $($names.Count) pieces)"
