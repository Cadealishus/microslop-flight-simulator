# Photorealistic 3D Tiles: plan

2026-11-13

## Status (2026-11-13)

Phases 1 to 5 are in, as a first version: `src/tiles3d.rs`, `src/shaders/photo.wgsl`, the coverage mask in `render.rs`, the zone helpers in `common.wgsl`, the setting and usage rows in `main.rs`, and `--photoreal` / `--no-photoreal` on the command line. Checked by flying over San Francisco, KSFO, the Davis farmland and the Nevada desert.

What the plan got wrong or left open, now settled:

- **No Draco.** Google serves plain glTF: float positions, u8/u16 indices, 256-1024 px JPEGs (one per primitive; a third of the tiles have two or three, see *Holes* below), `KHR_materials_unlit`, a y-up node matrix. No decoder library is needed; a Draco tile would be logged and skipped.
- **Billing** is per root request (`root.json`, max-age=0); child tiles carry the session in their URLs (nested subtree links do not, so it is passed down). Tiles say `max-age=14400`; nothing is cached on disk. The usage panel counts roots (billed sessions) apart from tiles.
- **Datum.** Mesh heights are WGS84 ellipsoidal, ours are sea level. Rather than ship a geoid model, the offset is measured: terrain minus mesh at sample vertices, against fine terrain tiles only, and the densest metre of the upper half of the distribution (roofs and trees only pull it down). San Francisco gives +32.4 to +32.7 m downtown and in the Marina, matching the EGM96 geoid there (about -32 m); the zone stays off until it is known.
- **Frame.** Each vertex goes ECEF to lat/lon/ellipsoid height to our sphere on the worker threads; tiles are drawn camera-relative with the datum as a radial lift.
- **Blending.** The drawn tiles are rendered top-down each frame into a 4096 px coverage mask (red). Terrain, buildings, roads, trees (all three kinds), street cars and airfield pavement discard where it is set; the mesh and our world are dithered into each other across a 1.6 km band at the 9 km edge of the zone. Airport aircraft, runway lights and the player's aircraft are kept.
- **Water.** Our water stays inside the zone. Our terrain's water is rendered into the same mask (green, its surface height), and the mesh gives way within 2 m (plus 12% of the tile's geometric error) of it, so piers, hulls and bridges stand out of our waves.
- **Physics.** `World::ground` returns the top of the mesh (a per-tile grid of triangles, finest tile first), blended in over 2.5 s when a tile arrives; our buildings and trees under the mesh are not there for collisions either. Landing on the KSFO mesh runway and crashing into mesh towers both work.
- **Lighting.** At first the photos were lit as albedo, mostly flat (70% up, 30% the true normal), with shadows from the aircraft only. Since 2026-11-14 they are relit by our sun (see *Relighting* below). At first the zone shrank to nothing at dusk, where our world took over with its lights; since 2026-11-13 it stays on at night (see below).
- **Coverage.** Outside cities Google's mesh is still better than ours (real desert and farmland colours), so the zone is used everywhere by day.

Measured on the dev machine (RTX 3060, 1600x900): over the Marina 62-82 fps with the mesh against 41-47 fps without (our buildings and trees cost more than the mesh); the discards cost nothing measurable when the zone is off. About 100 MB a minute of downloads over a city once loaded (90 MB for the first load), 600-650 MB of GPU memory (budget 640 MB, least recently drawn first out).

**Destroying mesh buildings** (2026-11-14, `src/skin.rs`): a crash into the mesh is matched to the nearest of our (hidden) footprints within 5 m (`World::photo_building`), which comes down as usual (destruct.rs). Its outlines, grown up to 2 m short of the neighbours (`World::cut_rings`), are cut out of the mesh (`Photo::take_building`, up to 16 cuts in the shaders, `photo_cut` in common.wgsl): nothing above the street there is drawn or stands, and our own ground shows through. The finest tiles in memory over the cut (seen or not) hand over their triangles; they are split to about a metre (0.3 m round the aircraft's hole), given to the nearest piece of outer wall or roof, and drawn with their tile's texture on that piece's slot (`vs_skin` in photo.wgsl), so the real building leans, breaks and falls. Standing walls are not drawn (the mesh is the building); pieces move onto the mesh they carry and take its mean colour. The datum also became sturdier on the way: samples on our buildings' footprints are left out, and once measured it only drifts within 4 m (it used to wander tens of metres among downtown towers).

**Smooth pavement** (2026-11-13, `src/pave.rs`): photogrammetry roads and runways are lumpy (cars melted into the street, half-metre waves, noise). As each tile is decoded, before its normals and collision grid, the vertices on a carriageway (`roads::carriageway`, a little short of the kerbs) or on paved runways, taxiways, helipads and aprons (OSM aeroway lines and polygons, z14 vector tiles fetched on the worker) are moved onto a smooth surface fitted through the mesh's own pavement: median per cell (2 m roads, 4 m airfield; the mesh has only a vertex or two per cell on flat pavement, so a lower quantile would bias it low), Gaussian-blurred, then refitted twice on the vertices near it (6/4.5/3.5 m blur for roads, 20-28 m for airfields). It follows the mesh's own level, so streets still meet their kerbs. Bridge decks (plus 3 m) and tunnels are left alone, as is anything more than 4.5 m (roads), 3 m (aprons: parked aircraft) or 5 m (runways) above the surface; the move fades out over the last 2 m inside the pavement and towards those limits. Tiles wider than 3 km are skipped. `--no-pave` turns it off to compare. Measured over SFO and downtown: mean move about 0, extremes about ±3 m.

**Night** (2026-11-13): the zone no longer fades out at dusk; the mesh stays day and night. It is lit by moon and sky as before, plus two lights of our own:

- *Street lamps.* When the sun is below about 7 degrees, our roads are drawn top-down into the coverage mask as well (`vs_top`/`fs_top` in roads.wgsl): blue holds the same lamp pools our roads show (`lamp_pool`), alpha the road's height. The mask is now `Rgba16Float` (128 MB instead of 64). The mesh (`photo_lamps`) reads a soft 4x4-texel footprint (texels are about 5.6 m) so the light spills over pavements and the foot of walls, and fades it with height above the road (gone 14 m up), so roofs and upper floors stay dark. It fades out between 1.5 and 3.8 km, a little sooner than our roads, and is a third as strong as on our roads: the photographed street is light concrete, not the dark asphalt we draw.
- *Windows* (`src/photonight.rs`, `photo_windows`). As each tile is decoded, vertices inside an OSM building footprint (grown 1.5 m) are tagged in the normal's spare byte with the building's orientation (63 steps over 90 degrees) and whether it is taller than 20 m. On walls of tagged triangles the shader lays a grid of windows square to the building, in world-fixed coordinates (the camera's travel in `G.east.w`/`G.north.w`): storeys of 3 m with single windows on low buildings, 3.7 m with ribbons on tall ones, occupancy and colours as in facade.wgsl, averaged when below a pixel. Green photo pixels (tree crowns in a footprint) get none.

Checked over downtown San Francisco at 22:00 from 250 m and from street level, against our own world at the same spot.

- *Traffic* (2026-11-13, `World::photo_car_light`, `tiles3d::Streets`). Our cars stay hidden in the zone, but at night (sun below about 8.5 degrees) each moving one that the mesh hides gets its head and tail lights back as sprites, drawn like the aircraft and obstruction lights (lights.wgsl). Headlights are warm white (intensity 60 each), tail lights red (12). They show from above and from their own end, not the other. Past 500 m each pair becomes one sprite at double intensity, and they fade out from 1.6 to 2.2 km, where our cars stop. Parked cars stay dark. Each light sits on the mesh's street, not on our road: the highest mesh surface no more than 3 m above our road (passing under crowns, awnings and bridges). Where that is more than 4 m down, as on the elevated I-80 approach to the Bay Bridge, where the mesh's deck is higher than our road, the lowest surface up to 8 m above our road is used, failing that our road's own height. Downtown the mesh street is -4 to +8 m from our road, mean about -0.5. The lookup runs in `car_instances`, sharing the cars' poses, on each road tile's collision tiles gathered once (boxes packed side by side). Each car's offset is cached and refreshed every 8th frame, staggered. About 0.1-0.2 ms a frame on top of `car_instances` (0.7-1 ms) for about 400 lit cars and 800-2000 sprites. Traffic density is the existing daily curve (`cars::activity`), so after 21:00 the streets are sparse. Checked over SoMa/downtown SF at 18:00-21:30 from 80-300 m, A/B against no car lights.

- *Obstruction lights* (2026-11-13, `photonight::beacons`, `Photo::beacons`). OSM buildings 45 m or taller (`BEACON_H`) get flashing red lights (FAA L-864 style, 1.8 s period) at night. Each light sits on the mesh's own highest vertex inside the footprint, not at the OSM height, and roofs wider than 45 m also get one near each corner that stands within 15% of the top. Tiles holding only a tower's foot get none. A building split over several drawn tiles is lit only from the tile that holds its top (keyed by its footprint's centre to 4 m). The lights go out on destroyed buildings (cuts) and outside the zone. They are drawn as the aircraft lights already are (lights.wgsl, the per-frame light buffer), so they cost no extra pass. Checked over SoMa/downtown SF at 22:00 from 300 m.

**Logo**: the credit line now starts with Google's official "Google Maps" logo (the outlined variant from Google's attribution assets, `assets/google_maps_logo.png`, unaltered, 22 px with its outline, alt text "Google Maps") on its own line at the bottom right, whenever Google imagery or 3D tiles are on screen; the credits follow below it.

**Relighting** (2026-11-14, `src/relight.rs`, `photo_shade` in photo.wgsl): the photographs' own sunlight is taken out as each tile decodes, and the mesh is lit by our sun, with its own shadows.

- *De-lighting.* Shadowed texels are 6-10x darker than lit ones and bluer (blue share about 0.45 against 0.32 in the sun, measured on 60 downtown SF textures). Each texel is classed on its own (the atlases are hundreds of small islands, so neighbours mean nothing): darker than 42% (fully below 17%) of the tile's 75th-percentile brightness, and bluer than its lit texels' median. Shadowed texels are lifted by the tile's measured lit/shadow ratio (2-6x), per channel so the blue cast goes too. How lit each texel was goes into the texture's alpha.
- *The photographer's sun.* Lit walls face the sun and shadowed walls face away, so the area-weighted lit share against wall facing gives its azimuth per tile (±30 degrees alone). Pooled over the drawn tiles it is steady: SF's imagery was taken with the sun at 127-141 degrees (morning). The elevation can't be read off this way, so 50 degrees is assumed.
- *Shading.* Where our sun is within about 14 degrees of the photographer's, the photographed shadows are kept (finer than our cascades); from 35 degrees off, our own cascades take over, with more of the true normal (75% instead of 30%). In between, the two are mixed. The relit mesh takes 0.65 of our skylight by day, plus the same ground bounce as our buildings. That gives the photographs' sun-to-shade contrast of about 6:1 (our sky alone gives 4:1 and looked flat and blue).
- *Shadows.* The drawn tiles are drawn into the three cascades. Our hidden buildings (per batch), trees and cars (per instance) leave the cascades where the mesh covers them, so the shadow passes now see the coverage mask (drawn before them). Only faces turned away from the sun are drawn (the winding is consistent: about 10,000 shared edges opposite against 100-370 the same way per tile), so the ground casts nothing. That cut the cost from 2.3 to about 0.75 ms. A cascade leaves out tiles that the next finer one covers wholly. A fragment stage (band, cuts) is used only for tiles that need it. Our water reads its shadow 4 m up, above the mesh's still water.
- `FABLE_NO_RELIGHT=1` gives the old look (photographs as they come, no mesh shadows) for comparison. Measured over downtown SF at 15:30 with vsync off: about 79 fps against 84 with it off.

Checked over downtown SF at 10:00, 13:00 and 15:30 in November, overcast at 13:00, and over SFO and the bay at 15:30. Not yet seen: other cities' imagery (taken at other hours), and destroyed buildings' pieces (lit by our light alone, `own`).

**Disk cache** (2026-11-13, `get3d` in gmaps.rs): meshes (`.glb`) are kept in `<cache>/fablegame/g3d` for exactly their own `Cache-Control` max-age (Google sends `private, max-age=14400, must-revalidate`; `private` only rules out shared caches). Root (max-age=0, billed) and subtree JSON always go out. Expired files are removed on read and at start-up, and the cache is capped at 3 GB (soonest to expire go first). `FABLE_NO_3D_CACHE=1` turns it off. The usage panel shows "N from disk" on the 3D row. Limitation found by testing: Google gives every file a new opaque name in each session (0 of 120 root-level URIs matched between two roots), so the cache only helps within one session, e.g. coming back over a city after the in-memory LRU dropped it. A new launch downloads everything again. Keying tiles by their place in the tree instead would carry across sessions, but that is a guess at identity which Google deliberately hides, so it was not done.

**Detail setting** (2026-11-13, `tiles3d::Params`, `gmaps::Detail`): *3D detail* Low / Medium / High / Ultra under the 3D checkbox in Settings, saved as `photoreal_detail` in google_maps.json, `--photo-detail low|medium|high|ultra` on the command line. It can be changed in flight: the zone eases to its new radius, and the request cap and GPU budget take effect the next frame. Each level sets four things:

| Level | Screen error | Zone (band) | GPU budget | Requests in flight |
| --- | --- | --- | --- | --- |
| Low | 32 px | 4 km (0.7 km) | 256 MB | 3 |
| Medium | 24 px | 6 km (1.1 km) | 400 MB | 5 |
| High (default, as before) | 16 px | 9 km (1.6 km) | 640 MB | 8 |
| Ultra | 11 px | 9 km (1.6 km) | 1 GB | 12 |

The band scales with the zone. The coverage mask keeps its 9 km size at every level. There are 12 fetch threads; a counter under the queue lock lets only the level's number work at once, so a slow connection gets the closest tiles first instead of many half-done ones.

Measured flying east at about 115 kt, 400 m above SF (Sunset to the bay), disk cache off, one root per level:

| Level | Streaming once loaded | First ~25 s | Tiles drawn | GPU memory | fps (vsync off) |
| --- | --- | --- | --- | --- | --- |
| Low | 43 MB/min | 24 MB | 50-70 | 230 MB | 95-130 |
| Medium | 75 MB/min | 43 MB | 100-140 | 380 MB (at its cap) | 94-137 |
| High | 91 MB/min | 65 MB | 180-240 | 590 MB | 88-130 |
| Ultra | 237 MB/min | 130 MB | 340-515 | 1.0 GB | 80-116 |

Tile counts follow the screen error as expected (about 1/error²), but bytes do not: Medium draws half High's tiles and streams only 20% less. Not verified, but likely: most of the stream is the finest tiles passing under the aircraft, and their level changes in whole steps (each halves the error), so a 1.5x threshold often picks the same level near the aircraft. Medium also sat at its GPU cap, so some tiles may have been fetched twice. Low is the level that saves data for slow connections (about half of High); Medium mostly saves GPU memory. The settings hint quotes these rates, plus that billing is one session per flight at every level.

**LOD crossfade** (2026-11-13, `Photo::crossfade`, `lod_keep` in photo.wgsl): a tile no longer swaps for its children (or back) in one frame. The new tiles fade in over 0.5 s while the old one fades out by the same amount, dithered against the same noise (in one, below the threshold; in the other, above it), so each pixel shows exactly one of them, never both and never neither; TAA smooths the dither into a fade. A tile that replaces nothing on screen (the first load, the zone's edge, turning to look) appears at once, since fading it in would show holes; so does one whose predecessor dropped out mid-fade. Fades go one level at a time: a tile still fading in does not split yet, and a tile does not take back the place of children still fading in (three overlapping levels would leave holes). Leaving tiles count as used, so they are not evicted mid-fade. In the shadow cascades the one more than half there casts. Nodes now know their parent. `FABLE_LOD_FADE=S` sets the length, 0 restores the instant swap. Checked over the Marina/North Beach at 250 m: with incoming tiles drawn red and leaving ones green (a temporary hook), the fading areas are a clean red/green checker with no holes; in recordings the instant swaps change a whole block between two frames, the fade over several, at the price of fine speckle during the half second (TAA keeps only about three frames of history at flying speed). About 5-10 extra tiles are drawn at a time over a city.

**Holes and floating geometry** (2026-11-14): flying low over a city, sky showed through the mesh in many places (tree crowns, backyards, along streets) and fragments stood on nothing. Three causes, found by temporarily painting magenta every pixel whose ray should meet the ground but saw sky:

- *Dropped primitives* (most of it). About a third of the tiles hold two or three primitives, each with its own JPEG (typically 256x512 + 256x256, or 512x512 + 256x512 + 256x256), and the decoder kept only the first texture's triangles: 15% of all triangles were thrown away (measured over 3000 cached tiles). Now each tile's photographs are packed side by side into one texture as it decodes (`pack`, `blit_padded`: largest first, guillotine free corners, 4 texels of repeated edge round each against bleeding) and the other primitives' texture coordinates are moved into their part of it. A tile stays one texture and one draw, so the renderer, skin.rs, relight.rs and the night lights are unchanged; single-texture tiles are decoded exactly as before. Waste in the packed atlases is under 25% (17% on the common 512+256 pair). Each photograph is de-lit on its own before packing.
- *Cracks along streets from pave.rs.* Each tile fits its own smooth pavement surface, so neighbouring tiles moved the vertices of their shared edge to different heights: thin bright lines across streets at every tile edge (gone with `--no-pave`). The move now fades in from the tile's edge (`EDGE_FADE`: 4 m for roads, 16 m on airfields), so the edges stay where Google put them and still meet. The tile's edge is its convex hull seen from above (tiles are cut from convex cells); topology does not work for this, since a third of the vertices of Google's meshes are on open edges.
- *Frustum culling* tested the tiles' bounding spheres at their ellipsoid heights, while the tiles are drawn lifted by the datum (+32 m in San Francisco): small tiles at the edges of the view could be culled while on screen. The test now lifts the sphere too (`Ctx::in_view`).

Cost, measured over downtown SF at 250 m (vsync off, 666 tiles drawn, alternating runs): 79 fps with the fix against 78.5 without, i.e. none; GPU memory for the same 857 tiles 889 MB against 711 MB (the triangles and photographs that were missing). The GPU budgets per detail level were left as they are, so fewer unused tiles stay cached.

What is left: rare one-frame specks where tiles of different levels meet (Google's own cracks; the same with everything above off), and the stipple of LOD crossfades.

Still open: nothing from the original plan.

## Goal and scope

Add Google Photorealistic 3D Tiles as an opt-in scenery mode: real photogrammetry meshes within a few kilometres of the plane, with the current procedural world everywhere else. It sits behind the existing Google Maps setting and key, and stays off by default.

- **In scope:** streaming, decoding and drawing the meshes near the plane; ground collision against them; turning it on and off at runtime; counting its requests and bytes in the existing usage panel; showing Google's attribution.
- **Out of scope for v1:** destroying photogrammetry buildings, relighting the baked textures, night lighting on the meshes, and anything offline.
- **Success:** a flight over a covered city looks photoreal near the plane, holds 60 fps on the dev machine, has no gaps or double ground where the two worlds meet, and lands safely on runways drawn from the mesh.

## How the data works

The data is a tree of tiles described by OGC 3D Tiles JSON, with compressed glTF meshes at each node. Details below are from memory and must be checked against Google's docs before coding (see Risks).

- **Entry point:** `GET tile.googleapis.com/v1/3dtiles/root.json?key=KEY`. Child URLs in the tree carry a `session` parameter that must be passed along.
- **Tree:** each node has a bounding volume, a `geometricError` in metres, `REPLACE` refinement and either content (a `.glb`) or a link to another JSON subtree.
- **Meshes:** glTF binary with Draco-compressed geometry and JPEG textures. Positions are in Earth-centred (ECEF) metres on the WGS84 ellipsoid, with a per-tile transform.
- **Choosing what to draw:** a node is split into its children when its screen-space error, `geometricError × screen height / (distance × 2 tan(fov/2))`, is above a threshold (about 16 px).
- **Attribution:** each glTF carries copyright strings in `asset.copyright`. Google requires the ones in view to be shown, plus the Google logo.
- **Billing:** believed to be per root request, with a session that covers its child tiles for a few hours. Needs confirming, since it decides whether we count requests or sessions in the usage panel.

## Architecture

One new module, `tiles3d.rs`, owns the tile tree. It fetches through the existing `gmaps.rs` client, so the key, session and usage counts are shared with the 2D imagery.

```
Google 3D Tiles ──> gmaps.rs ──────────> tiles3d.rs (new) ──────> Decode
root.json,          key and session      screen-error traversal    glTF + Draco
glb tiles           request/byte counts  worker pool, LRU budget   ECEF to game frame
                                               │                     │        │
                                               │ coverage            │        │ triangles
                                               v                     v        v
                                          World blend           Collision BVH   Mesh pass (wgpu)
                                          hide our tiles        World::ground   baked albedo, tint, fog
                                          blend band at edge    building/crash  depth + motion for TAA
```

Decoded tiles feed three consumers: a new render pass, a CPU collision structure for physics, and a coverage mask that tells `world.rs` where to hide its own terrain, buildings, trees and cars. Decoding runs on the existing `sources::Loader` worker threads, with the same per-frame GPU upload budget as building tiles.

## What breaks or changes

The meshes are one baked surface, so every system that relies on separate terrain, buildings, trees and cars has to be switched off or adapted inside the 3D zone.

| System | Inside the 3D zone | Plan |
| --- | --- | --- |
| Lighting and time of day | Shadows and sun direction are baked into the textures | v1: flat lighting with a time-of-day tint and fog. Later: de-light the albedo approximately |
| Night | Textures show daytime | Fade the 3D zone out after dusk and fall back to the current world with city lights |
| Terrain and buildings | Duplicated by the mesh | Hide our terrain and building tiles under the mesh coverage, with a blend band at the edge |
| Trees, cars, roofs | Part of the mesh | Hide ours inside the zone |
| Ground collision | Our heightfield no longer matches what is drawn | Raycast the decoded mesh on the CPU (BVH per tile) for `World::ground` and building hits |
| Building destruction and craters | Mesh can't be cut | v1: crash explodes but the mesh stays. Later: carve a hole with a stencil or SDF mask |
| Airports | Runways are in the mesh, possibly bumpy | Keep our levelled runway geometry drawn on top, and collide with it first |
| Water | Mesh water is static | Keep our water shader where the vector water mask says so |
| TAA, shadows, impostors | New geometry pass | Mesh writes depth and motion like terrain; shadows only from the plane in v1 |

## Phases

Five phases, each ending in something you can fly and judge. Phase 1 is the go/no-go point: if the look or the cost disappoints, the work stops there.

1. **Spike: see one city.** Fetch the root, walk the tree near the plane with a fixed error threshold, decode glTF and Draco, and draw unlit meshes over the current world. Count requests and bytes. *Gate: screenshots over SF and a measured cost per 10-minute flight.*
2. **Streaming and LOD.** Screen-space-error traversal, a priority queue on the existing worker pool, a GPU byte budget with LRU eviction, and no holes while children load. *Gate: a 5-minute low pass over a city at 60 fps with no visible pops.*
3. **Blend with the world.** Hide our terrain, buildings, trees and cars under the mesh, add a blend band at the edge of the zone, and convert ECEF on the WGS84 ellipsoid to our spherical frame. *Gate: no double ground or seams at the zone edge.*
4. **Physics and airports.** Mesh BVH raycasts for ground and building hits, our runway geometry on top, and crashes against the mesh. *Gate: takeoff and landing at KSFO with the mesh on.*
5. **Look and polish.** Time-of-day tint, fog, the night fallback, attribution and the Google logo, a settings toggle and usage counts. *Gate: user sign-off.*

## Risks and open questions

The biggest unknowns are Google's terms and the real cost per flight, so both are checked before phase 1 code is written.

- **Terms of service.** Google's terms restrict caching tiles to disk and mixing their 3D Tiles with other map data. Either could rule out our blend with OSM buildings, or the disk cache. *Read the current Map Tiles API policies first.*
- **Cost.** A fast, low flight walks many tiles. If billing is per root request, a session is cheap; if it is per tile, a 10-minute city flight could be costly. Phase 1 measures it.
- **Draco decoding in Rust.** There is no mature pure-Rust Draco decoder. Options: bind Google's C++ `draco` library, or port only the mesh-decoding path. *Pick in phase 1; the C++ binding is the safer bet.*
- **Performance.** City tiles are dense. The GPU budget, LOD threshold and a cap on in-flight requests must keep it inside the frame time we have (see the GPU-bound profile from Nov 13).
- **Precision.** ECEF coordinates need camera-relative f64 transforms, as terrain already does, or the mesh will jitter.
- **Coverage.** Outside covered cities the 3D tiles are coarse satellite-draped terrain. The zone should only turn on where the mesh is better than ours, for example by tile depth.
- **Open:** should the 3D zone follow the plane only, or also the chase camera? Do you want this in the globe menu too, or in flight only?
