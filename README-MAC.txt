Openflight - Mac

1. Install Rust once: open Terminal and follow https://rustup.rs
2. Double-click "Run on Mac.command" in this folder.
   The first time, macOS may refuse because the file was downloaded:
   right-click it, choose Open, then Open again.
3. The first run builds the game, which takes several minutes and
   needs an internet connection. After that it starts straight away.

This is the game's source, not a ready-made Mac app. It has not been
tested on a Mac.

How it works
"Run on Mac.command" is a small script. It finds Rust's cargo tool, then runs
"cargo run --release", which compiles the game from the source code in this
folder (the first time only; later runs reuse the build in the "target"
folder) and starts it. If the game stops with an error, the window stays open
so you can read why.

The game downloads map data (satellite imagery, terrain heights, buildings,
roads) as you fly and keeps it in your user cache folder (fablegame), so
places you've flown before load faster. Your career logbook is career.txt in
this folder. The full description of the game and its controls is in
README.md.
