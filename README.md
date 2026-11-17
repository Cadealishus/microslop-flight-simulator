Open world flight simulator in pure rust created by Claude Fable 5.2 and Opus 5.5

# Demo
[Demo](https://github.com/user-attachments/assets/4f8833da-13c0-4d20-a50b-c0acf2d88df1)



# Play
* **Windows:** install Rust from https://rustup.rs, then double-click `Run on Windows.bat` (see `README-WINDOWS.txt`).
* **Mac:** install Rust, then double-click `Run on Mac.command` (see `README-MAC.txt`).
* **Browser, no install:** https://cadealishus.github.io/microslop-flight-simulator-web-edition/

# Compiling
* Install rust
```
cargo build --release
```

running
```
cargo run --release
```

# Browser version
The same game compiled to WebAssembly, drawn with WebGPU (Chrome or Edge; recent Safari and Firefox too).

Once:
```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
```

Build (Windows PowerShell), then serve the `web` folder and open http://localhost:8000:
```
./build-web.ps1
python -m http.server 8000 --directory web
```

Desktop options go in the address: `?no-menu&airport=KSFO&runway=28R` is `--no-menu --airport KSFO --runway 28R`.

In the browser all loading runs on the page's one thread, so scenery comes in slower than on the desktop. The live METAR weather and ADS-B traffic feeds don't allow requests from web pages, so they're off there, and Google's tiles aren't available.

# Contributions
Please contribute anything if you want to! Improvements to flight sim realism, graphics, optimization, UI, and especially plane models are needed! (((=
