# Rebuilds the browser version and pushes it to the Browser Edition repo
# (browser-edition/, served by GitHub Pages).
Set-Location $PSScriptRoot
.\build-web.ps1
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Copy-Item web\index.html browser-edition\
if (Test-Path web\loading.jpg) { Copy-Item web\loading.jpg browser-edition\ }
Copy-Item web\index.html, web\jsfiddle.js browser-edition\
Copy-Item web\pkg\openflight.js, web\pkg\part*.bin, web\pkg\parts.json browser-edition\pkg\
git -C browser-edition add -A
git -C browser-edition commit -m "Update the game"
git -C browser-edition push
