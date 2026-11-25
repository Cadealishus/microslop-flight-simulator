# Openflight

Open world flight simulator in pure rust created by Claude Fable 5.2 and Opus 5.5

# Demo
[Demo](https://github.com/user-attachments/assets/4f8833da-13c0-4d20-a50b-c0acf2d88df1)

# Play
* **Windows:** install Rust from https://rustup.rs, then double-click `Run on Windows.bat` (see `README-WINDOWS.txt`).
* **Mac:** install Rust, then double-click `Run on Mac.command` (see `README-MAC.txt`).
* **Browser, no install:** https://cadealishus.github.io/microslop-flight-simulator-web-edition/ (Chrome or Edge)

The first run on Windows or Mac builds the game from this source code, which takes several minutes. After that it starts straight away.

# What's in it

**The whole real world.** Fly anywhere on Earth. The ground is built as you fly from free online map data:
* satellite imagery (Esri World Imagery), draped over real terrain heights (AWS Terrain Tiles)
* buildings from OpenStreetMap at their mapped height, each with generated walls: brick, stone, glass towers, offices, houses, sheds, shops, lit windows at night
* roads with traffic, trees and land cover, coastlines with breaking waves, and an ocean with swell
* every airport in the database, with runways, taxiways, aprons, gates, runway lights, approach lights and airport traffic

**Aircraft:** Cessna 172RG, King Air 350, Boeing 747-400 (and a FedUp freighter), F/A-18C Hornet, F-14A Tomcat, A-10C Thunderbolt II, F-117A Nighthawk, L-39ZA Albatros and C-17A Globemaster III. Each has its own flight model (weight, engines, lift, drag), working gear, flaps and control surfaces, and a cockpit with live instruments.

**Weather and time:** clouds you can fly through, storms, wind and gusts, haze, sun and moon positions for the real date, and a day and night cycle. By default the weather is the live report (METAR) from the nearest airport, and real aircraft flying nearby (ADS-B) appear in the sky.

**Crashes and damage:** parts can break off, engines can catch fire, and buildings can be knocked down.

**On foot and on the road:** stop on the ground (or crash) and press **Enter** to climb out of the aircraft as the pilot, a photo-scanned person whose walk is matched to the speed they move, so the feet don't slide. Press **1** for a Ford F-150 Raptor R or **2** for a Lamborghini Countach LPI 800-4, walk up to the driver's door and press **Enter** to get in. The cars hang on four springs with tyres that grip, slide and drift (Space is the handbrake), roll over and scrape sparks. They crash in pieces: the front, back and sides are crumple zones that crush as far as a real car's (about a metre at the front) and take the energy as they do, so a car hitting a wall stops dead rather than bouncing, at the deceleration a real one does (30 to 50 g at 90 km/h), and the body crumples as far as it crushed. Doors, hoods and tailgates are knocked open and swing on their hinges (an open hood catches the wind) until a hard enough hit or slam tears them off; bumpers and mirrors shear off; windows burst; wheels come off and roll away. Everything that comes off tumbles and lies where it lands. A fast enough car brings down a small building. Walk back to the aircraft and press **Enter** to fly again. Start straight away in a car with `--drive raptor` (or `?drive=countach` in the browser), or on foot with `--drive foot`.

**Sound:** engines, wind, gear, flaps and cockpit warnings are made by a synthesizer as you fly, not from recordings (apart from the fighters' roar and the menu music).

**Multiplayer:** fly with friends in a room with a 5-letter code. Windows, Mac and the browser version all play together. Other players appear as the aircraft they're really flying, with green name tags (and a box around them when they're within 3 km). Their crashes play the full explosion, fire and wreck for everyone, and the host's time of day and weather are everyone's. See [Multiplayer](#multiplayer).

**Graphics settings** (pause menu, Settings), saved in `settings.txt`:
* **Quality**: Potato, Low, Medium, High. Each draws the world at a lower resolution than the next (the menus stay sharp) and sets the switches below. **Potato** is for the slowest computers (built-in graphics, a Celeron, 8 GB): 40% resolution, short render distance, and no shadows, ambient occlusion, trees or road traffic. A lower resolution is stretched back up smoothly (bicubic) and sharpened, so it stays clean rather than blocky.
* **Render distance**: Short, Medium, Far: how far out buildings, roads and fine ground detail appear.
* **Switches**: Shadows, Ambient occlusion, Clouds, Trees, Road traffic, each on or off.

**Game modes**, from the main menu:
* **Free Flight:** pick any spot or airport on the globe, an aircraft, the time and the weather.
* **World Photographer:** a scenic spot with the HUD hidden; take photos with F12.
* **Career:** a pilot's logbook with ranks, stats, achievements and delivery jobs between airports. Softer landings pay more.
* **Challenge League:** five hard landings (San Francisco, St Maarten, Gibraltar, Queenstown, Courchevel), scored by how softly you touch down.
* **Marketplace:** every aircraft, to pick from.

# Controls

| Key | Does |
|---|---|
| W / S or ↑ / ↓ | pitch (nose down / up) |
| A / D or ← / → | roll |
| Q / E | rudder |
| Shift / Ctrl | throttle up / down |
| B | wheel brakes |
| Z | reverse thrust |
| Space | afterburner / boost |
| F / V | flaps down / up |
| L | landing gear |
| / | speedbrake |
| C | cockpit or chase camera |
| mouse drag, wheel | look around, zoom |
| R | start again here |
| Enter | get out of the aircraft or car / get into the nearest one |
| G | go to a place or airport |
| T / Y | time of day later / earlier |
| N / M | more / fewer clouds |
| F3 | weather panel |
| K | set the heading indicator to the compass |
| P | pause |
| H | hide the HUD |
| U | sound on / off |
| Tab | flight panel |
| Esc | pause menu |
| F1 | help |
| F2 | anti-aliasing on / off |
| F12 | screenshot |

**Gamepad (Xbox layout, as Microsoft Flight Simulator sets it up):** left stick pitch and roll, triggers rudder, hold A / B throttle up / down, hold X brakes, Y + B parking brake, Y + d-pad trim, bumpers flaps, left stick click gear, right stick look around, d-pad quick views, View button cockpit or chase camera, Menu button pause.

**On foot:** W / S walk forward / back, A / D turn, Shift run, 1 / 2 call a Raptor / Countach, Enter get in. **Driving:** W gas, S brake and reverse, A / D steer, Space handbrake, C inside or chase view, mouse drag look around, Enter get out (below 15 km/h).

**Crash testing:** `--crashtest raptor@25` (or `countach@40`, any speed in m/s) puts the car on the Bonneville Salt Flats driving at a concrete wall 40 m ahead, the same crash every time. R runs it again with the car as new, O cycles slow motion (x1, x0.25, x0.1), F4 shows the physics overlay (the car's collision points, red where they touch, the normals they push along, the wheels' springs, the velocity, g-force and the last hits). A second after the impact the game writes a report to its log: the hardest hit, peak g, damage, deepest dent, glass burst, wheels lost and the rebound. Without map data (offline) the test runs on flat ground.

# Multiplayer

1. One player opens the pause menu (Esc), goes to **Multiplayer** and clicks **Host a room**. A 5-letter code appears.
2. Everyone else opens **Multiplayer**, types the code and clicks **Join**. They start right beside the host, flying as the host is (or parked beside them on the ground).
3. Set your name on the same page.

You can also start straight into a room: `--join K7Q2M` (or `--host`) on the command line, or `?join=K7Q2M` on the Web Edition's address. `--name Cade` / `&name=Cade` sets your name.

**How it works** (`net.rs`): there is no game server. Every player connects to three free public MQTT brokers (message relays: EMQX, HiveMQ, Mosquitto) over secure WebSockets, subscribes to the room's topic, and sends its aircraft's state ten times a second to all three. The same message arriving through two relays counts once, so players meet as long as any one relay works for both. Each report is stamped with the sender's clock, and other aircraft are played back about a quarter of a second behind, gliding between reports, so they move smoothly even when reports arrive unevenly. A crash is sent as its own message, and everyone runs the game's own crash for it (fireball, fire simulation, breaking wreck, crater, buildings knocked down). The host also sends the time of day and weather.

Rooms are public: anyone with the code can join, and positions and names go through the public relays.

# How it works

**Everything is Rust** (no game engine): [wgpu](https://wgpu.rs) for the graphics, winit for the window and input, egui for the menus and HUD.

**Loading the world** (`sources.rs`, `world.rs`): the Earth is cut into map tiles at several zoom levels, finer near the aircraft. For each tile in view, worker threads download the imagery, elevation and map data (`fetch.rs`), keep them in a disk cache, and build the meshes: terrain (`sources.rs`), buildings and roads (`buildings.rs`, `roads.rs`), trees (`trees.rs`), shorelines (`shore.rs`) and airfields (`airports.rs`, `airfield.rs`, `pave.rs`). The finished tiles go to the GPU, and tiles far behind are dropped.

**Drawing** (`render.rs`, `src/shaders/*.wgsl`): the shaders draw the terrain, ocean, buildings (their walls are drawn in `facade.wgsl`, not stored as pictures), clouds, sky, aircraft and particles, then add shadows, ambient occlusion, haze, temporal anti-aliasing and the final colour grade.

**Flying** (`plane.rs`, `airframe.rs`, `aircraft/`): each aircraft is a set of pieces (wings, tail, gear, surfaces...) with mass and aerodynamic shape. Every frame the forces on each piece are added up and the aircraft is moved in small steps. Contact points on the gear and airframe handle the ground, and crashes break pieces off (`damage.rs`, `crash.rs`, `destruct.rs`).

**The rest:** `weather.rs` and `live.rs` (weather and real traffic), `traffic.rs` and `cars.rs` (airport and road traffic), `audio.rs` (the synthesizer), `menu.rs` (globe and menus), `career.rs` (logbook), `cockpit.rs` and `gauges.rs` (instruments), `plat.rs` (what differs between the desktop and the browser).

**Saved on your computer:** downloaded map tiles in your user cache folder (`openflight`), and next to the game the career logbook (`career.txt`), graphics settings (`settings.txt`) and your multiplayer name (`mp_name.txt`).

# Command-line options

`cargo run --release -- <options>`, for example `--no-menu --airport KSFO --runway 28R`. The useful ones:

| Option | Does |
|---|---|
| `--no-menu` | fly straight away |
| `--airport KSFO`, `--runway 28R`, `--approach` | start on a runway, or on final approach |
| `--lat`, `--lon`, `--agl`, `--heading` | start at a spot, height (m above ground) and heading |
| `--place "Paris"` | start over a named place |
| `--aircraft 747` | which aircraft: `c172`, `ka350`, `747`, `fedup`, `fa18`, `f14`, `a10`, `f117`, `l39`, `c17` |
| `--time 18.5`, `--clouds 0.7`, `--wind 15,270` (knots, from degrees), `--storms 0.5` | time and weather |
| `--cockpit` | start in the cockpit view |
| `--offline` | use only the map cache (no downloads) |
| `--mute`, `--no-hud`, `--no-vsync` | sound, HUD and vsync off |
| `--host`, `--join K7Q2M`, `--name Cade` | multiplayer: make a room, join one, your name |
| `--help` | the full list |

# Compiling
* Install rust
```
cargo build --release
```

running
```
cargo run --release
```

# Browser version (Web Edition)
The same code also compiles to WebAssembly and draws with WebGPU in the browser. It's published at https://github.com/Cadealishus/microslop-flight-simulator-web-edition, which also has a JSFiddle launcher.

What's different there (`plat.rs`): a browser page has one thread and can't wait on downloads, so the background work runs between frames, and downloads are started and checked on instead of waited for. A job that finds a file missing asks for everything it needs at once and runs again once it arrives. The career logbook goes in the browser's storage. Scenery loads slower than on the desktop. The live weather (METAR) and live traffic (ADS-B) feeds don't allow requests from web pages, so they're off there, and Google's tiles aren't available.

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

`build-web.ps1` compiles the game, splits the 73 MB game file into pieces of 18 MB or less (so the jsDelivr mirror will serve them) and writes `web/pkg/parts.json` listing them. `publish-web.ps1` builds and pushes it all to the Web Edition repo, which needs to be cloned into `browser-edition/`.

Desktop options go in the address: `?no-menu&airport=KSFO&runway=28R` is `--no-menu --airport KSFO --runway 28R`.

# Contributions
Please contribute anything if you want to! Improvements to flight sim realism, graphics, optimization, UI, and especially plane models are needed! (((=

# Vehicle models
The cars and the pilot are downloaded models converted with `tools/vehicle_import.py` (Python with NumPy and Pillow, no Blender): `python tools/vehicle_import.py car <model.glb> <key> <nose axis> [<length m> [<paint r,g,b>]]` for a car (its wheels need "wheel", "tire" or "rim" in their names), `python tools/vehicle_import.py ped <walking.glb> <key>` for a character with a walk animation. Credits and licences: [assets/vehicles/CREDITS.md](assets/vehicles/CREDITS.md). The two cars are CC-BY-NC-SA (non-commercial).
