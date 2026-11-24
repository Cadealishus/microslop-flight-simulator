Openflight - Windows

1. Install Rust once: download and run rustup-init.exe from https://rustup.rs
   If it asks to install the Visual Studio C++ build tools, say yes.
2. Double-click "Run on Windows.bat" in this folder.
   The first time, Windows may warn that the file came from the internet:
   click "More info", then "Run anyway".
3. The first run builds the game, which takes several minutes and needs an
   internet connection. After that it starts straight away.

No install wanted? The Web Edition runs in Chrome or Edge:
https://cadealishus.github.io/microslop-flight-simulator-web-edition/

How it works
"Run on Windows.bat" is a small script. It finds Rust's cargo tool, then runs
"cargo run --release", which compiles the game from the source code in this
folder (the first time only; later runs reuse the build in the "target"
folder) and starts it. If the game stops with an error, the window stays open
so you can read why.

The game downloads map data (satellite imagery, terrain heights, buildings,
roads) as you fly and keeps it in your user cache folder (openflight), so
places you've flown before load faster. Your career logbook is career.txt in
this folder. The full description of the game and its controls is in
README.md.
