// Facades, shared by the buildings and the pieces of wrecked buildings
// (inst.wgsl), so a building looks the same before and after it breaks up.

fn box(x: f32, half: f32, fw: f32) -> f32 { return clamp((half - abs(x)) / fw + 0.5, 0.0, 1.0); }
fn rect(cx: f32, fy: f32, w: f32, y0: f32, y1: f32, fw: vec2f) -> f32 {
  return box(cx, w * 0.5, fw.x) * box(fy - (y0 + y1) * 0.5, (y1 - y0) * 0.5, fw.y);
}

// Brickwork in stretcher bond: every brick its own shade, a few burnt dark,
// mortar joints between. `fine` fades it out where bricks are not resolved.
fn brick(uv: vec2f, fw: vec2f, fine: f32, wall: vec3f) -> vec3f {
  let row = floor(uv.y / 0.085);
  let bu = uv.x / 0.24 + row * 0.5;
  let bid = vec2f(floor(bu), row);
  let jy = 1.0 - box(fract(uv.y / 0.085) - 0.5, 0.40, fw.y / 0.085);
  let jx = 1.0 - box(fract(bu) - 0.5, 0.465, fw.x / 0.24);
  var c = wall * (0.8 + 0.4 * hash2(bid));
  c = mix(c, c * vec3f(0.62, 0.58, 0.6), step(0.91, hash2(bid + 7.0)));
  return mix(wall, mix(c, wall * 1.2 + 0.07, max(jx, jy) * 0.7), fine);
}

// Dressed stone: big blocks with fine dark joints, each block a little different.
fn ashlar(uv: vec2f, fw: vec2f, fine: f32, wall: vec3f) -> vec3f {
  let row = floor(uv.y / 0.34);
  let bu = uv.x / 0.72 + row * 0.5;
  let jy = 1.0 - box(fract(uv.y / 0.34) - 0.5, 0.472, fw.y / 0.34);
  let jx = 1.0 - box(fract(bu) - 0.5, 0.486, fw.x / 0.72);
  let c = wall * (0.9 + 0.2 * hash2(vec2f(floor(bu), row) + 3.0));
  return mix(wall, mix(c, wall * 0.5, max(jx, jy) * 0.8), fine);
}

struct Facade {
  bar: vec2f,   // glazing bars in the window: x = a centre post, y = height of a transom (0 none)
  trimc: vec3f, // colour of frames and bars
  albedo: vec3f,
  glass: f32,   // how much of this pixel is window glass
  lit: f32,     // how brightly the room behind is lit at night
  lcol: vec3f,  // colour of that light
  // The window bay this pixel is in, for looking into the room behind it:
  cell: vec4f,  // position across the bay (centred), height in the storey, bay width, storey height
  wid: vec2f,   // bay column and storey
  open: vec4f,  // window opening: half width, sill, head (storey coords), depth of the reveal; x = 0 none
  tilt: vec2f,  // how this pane is tilted out of true (panes never sit perfectly flat)
  coat: f32,    // reflectance of the glass head on (coated glass mirrors far more)
  span: f32,    // opaque glass spandrel panels: reflect like glass, nothing behind
  bays: f32,    // how many bays one room spans
  on: f32,      // the room's light: 0 off, else brightness
  aa: f32,      // 1 when single windows are resolved on screen
};

fn facade(uv: vec2f, base: vec3f, ftype: u32, seed: f32, wealth: f32, H: f32, fw_in: vec2f) -> Facade {
  let fw = max(fw_in, vec2f(0.0015));
  var floor_h = 3.0;
  var cell_w = 3.0 + seed * 1.2;
  var winfrac = 0.18;
  if (ftype == 1u) { floor_h = 3.2; cell_w = 2.5 + seed * 0.9; winfrac = 0.22; }
  else if (ftype == 2u) { floor_h = 3.7; cell_w = 1.5; winfrac = 0.45; }
  else if (ftype == 3u) { floor_h = 3.9; cell_w = 1.5; winfrac = 0.8; }
  else if (ftype == 4u) { floor_h = 6.0; cell_w = 8.0; winfrac = 0.03; }
  else if (ftype == 5u) { floor_h = 3.6; cell_w = 2.8 + seed; winfrac = 0.25; }
  else if (ftype == 6u) { floor_h = 3.0; cell_w = 3.2 + seed * 0.6; winfrac = 0.2; }
  let y = max(uv.y, 0.0);
  let g = vec2f(uv.x / cell_w + seed * 7.0, y / floor_h);
  let id = floor(g);
  let cx = (fract(g.x) - 0.5) * cell_w;
  let fy = fract(g.y) * floor_h;
  let h1 = hash2(id + seed * 31.0);
  let h2 = hash2(id * 1.7 + 11.0 + seed * 13.0);
  let hc = hash2(vec2f(id.x, seed * 57.0));
  let top = H - uv.y;
  let ground = id.y < 0.5;
  let aa = clamp(1.0 - max(fw.x / cell_w, fw.y / floor_h) * 1.7, 0.0, 1.0);
  let fine = clamp(1.0 - max(fw.x, fw.y) * 4.0, 0.0, 1.0);
  let n1 = textureSampleLevel(t_wave, s_repeat, uv / 5.0 + seed, 0.0).a;
  let n2 = textureSampleLevel(t_wave, s_repeat, uv / 0.7 + seed, log2(max(max(fw.x, fw.y) * 256.0 / 0.7, 1.0))).a;
  var wall = base * (0.9 + 0.2 * n1);
  var glass = 0.0;
  var frame = 0.0;      // lighter trim
  var dark = 0.0;       // doors, louvres, shadow lines
  var darkcol = vec3f(0.035, 0.03, 0.028);
  let trim = mix(base * 1.35 + 0.05, vec3f(0.62, 0.61, 0.58), 0.35 + 0.4 * wealth);
  var open = vec4f(0.0);
  var ocx = cx;
  var ofy = fy;
  var ocw = cell_w;
  var span = 0.0;
  var coat = 0.04;
  var bays = 1.0;
  var tilt_k = 0.012;
  var acc = 0.0;                 // accents: shutters, air conditioners
  var acccol = vec3f(0.3);
  var pipe = 0.0;                // rainwater downpipes
  var bar = vec2f(0.0);
  let hb = fract(seed * 17.3);   // picks made once for the whole building
  let hb2 = fract(seed * 29.7);
  let shutcol = mix(vec3f(0.07, 0.16, 0.11), mix(vec3f(0.1, 0.13, 0.22), vec3f(0.22, 0.1, 0.07), step(0.5, fract(seed * 41.0))), step(0.5, fract(seed * 23.0))) * (0.72 + 0.28 * step(0.5, fract(fy / 0.07)) * fine);

  if (ftype == 0u) {
    // House: scattered windows, a front door, siding or brick or stucco.
    let mat = fract(seed * 5.3);
    if (mat < 0.4) { wall *= 1.0 - 0.13 * box(fract(uv.y / 0.19) - 0.92, 0.08, fw.y / 0.19) * fine; }
    else if (mat < 0.62) { wall = brick(uv, fw, fine, wall); }
    else { wall *= 0.92 + 0.16 * n2; }
    // The footing: a band of bare concrete at the ground.
    wall = mix(wall, vec3f(0.34, 0.33, 0.31) * (0.8 + 0.4 * n2), box(uv.y - 0.17, 0.17, fw.y) * 0.85);
    let ww = 0.95 + 0.6 * wealth;
    let has = step(0.28, h1);
    let door = step(hc, 0.16) * f32(ground);
    // Some houses keep the car behind a panelled door at the front.
    let gar = step(0.16, hc) * step(hc, 0.3) * f32(ground) * step(3.3, cell_w);
    let win = has * (1.0 - door) * (1.0 - gar);
    glass = rect(cx, fy, ww, 0.95, 2.15, fw) * win;
    frame = rect(cx, fy, ww + 0.22, 0.84, 2.26, fw) * win * (0.4 + 0.6 * wealth);
    // A sill under every window and the line of shadow below it.
    frame = max(frame, rect(cx, fy, ww + 0.4, 0.8, 0.9, fw) * win);
    dark = rect(cx, fy, 1.0, 0.0, 2.1, fw) * door;
    // A panelled front door with a step and a small light over it.
    frame = max(frame, (rect(cx, fy, 1.24, 0.0, 2.22, fw) - rect(cx, fy, 1.0, 0.0, 2.1, fw)) * door);
    dark = max(dark, rect(cx, fy, ww + 0.36, 0.765, 0.8, fw) * win * 0.55 * fine);
    darkcol = mix(vec3f(0.09, 0.05, 0.03), vec3f(0.05, 0.08, 0.12), step(0.5, h2));
    darkcol *= 1.0 + 0.5 * door * fine * (box(abs(cx) - 0.24, 0.15, fw.x) * (box(fy - 0.55, 0.3, fw.y) + box(fy - 1.45, 0.42, fw.y)));
    bar = vec2f(1.0, select(0.0, 1.55, hb > 0.4)) * win;
    // Shutters either side of the windows on some houses.
    acc = box(abs(cx) - (ww * 0.5 + 0.33), 0.2, fw.x) * box(fy - 1.55, 0.62, fw.y) * win * step(0.6, hb2);
    acc = max(acc, rect(cx, fy, 2.5, 0.0, 2.1, fw) * gar);
    acccol = mix(shutcol, mix(vec3f(0.6, 0.59, 0.56), base * 0.8, 0.4) * (1.0 - 0.22 * box(fract(fy / 0.5) - 0.5, 0.05, fw.y / 0.5) * fine), gar);
    pipe = box(cx + cell_w * 0.5 - 0.09, 0.035, fw.x) * step(hash2(vec2f(id.x, seed * 77.0)), 0.2);
    // Boarded or curtained-off windows in rougher places.
    let boarded = step(h2, 0.25 * (1.0 - wealth) - 0.05);
    glass *= 1.0 - boarded * 0.85;
    if (has * (1.0 - door) * (1.0 - boarded) > 0.5) { open = vec4f(ww * 0.5, 0.95, 2.15, 0.12); }
  } else if (ftype == 1u || ftype == 5u || ftype == 6u) {
    // Town houses, shops with flats above, apartment blocks.
    let ww = select(1.15, 1.45, ftype == 6u);
    var has = step(0.07, h1);
    glass = rect(cx, fy, ww, 0.85, 2.3, fw) * has;
    frame = rect(cx, fy, ww + 0.26, 0.72, 2.44, fw) * has;
    // Sill and lintel standing a little wider than the frame, shadow under the sill.
    frame = max(frame, max(rect(cx, fy, ww + 0.46, 0.66, 0.78, fw), rect(cx, fy, ww + 0.4, 2.4, 2.56, fw) * step(0.45, wealth)) * has);
    dark = rect(cx, fy, ww + 0.42, 0.625, 0.66, fw) * has * 0.55 * fine;
    bar = vec2f(step(0.3, hb), select(0.0, 1.9, hb2 > 0.5)) * has;
    if (has > 0.5) { open = vec4f(ww * 0.5, 0.85, 2.3, 0.16); }
    // What the walls are made of.
    let mat = fract(seed * 5.3);
    if (ftype == 6u) {
      if (mat < 0.6) { wall = brick(uv, fw, fine, wall); }
      else {
        // Precast concrete panels, one a bay and a storey, a little different each.
        let pj = max(1.0 - box(cx, cell_w * 0.5 - 0.02, fw.x), 1.0 - box(fy - floor_h * 0.5, floor_h * 0.5 - 0.02, fw.y));
        wall = mix(wall * (0.93 + 0.12 * hash2(id + 3.0)) * (0.94 + 0.12 * n2), wall * 0.55, pj * 0.7 * fine);
      }
    } else if (mat < 0.36) { wall = brick(uv, fw, fine, wall); }
    else if (mat < 0.56) { wall = ashlar(uv, fw, fine, wall); }
    else { wall *= 0.92 + 0.16 * n2; }
    // A darker plinth where the wall meets the street.
    wall = mix(wall, wall * 0.6, box(uv.y - 0.28, 0.28, fw.y) * 0.8);
    if (ftype == 6u) {
      // Balconies on some stacks.
      let bal = step(hc, 0.35) * f32(!ground);
      dark = max(dark, max(box(fy - 0.07, 0.07, fw.y), box(fy - 0.95, 0.03, fw.y) * 0.7) * box(cx, 1.35, fw.x) * bal);
      // Railings: thin bars under the handrail.
      dark = max(dark, box(fy - 0.5, 0.45, fw.y) * box(cx, 1.35, fw.x) * bal * 0.6 * step(0.72, fract(cx / 0.13)) * fine);
      glass = max(glass, rect(cx, fy, 1.5, 0.15, 2.35, fw) * bal);
      if (bal > 0.5) { open = vec4f(0.75, 0.15, 2.35, 0.08); bar = vec2f(1.0, 0.0); }
      // The edge of each floor slab showing as a pale band.
      frame = max(frame, box(fy - 0.05, 0.05, fw.y) * 0.55 * step(0.6, mat));
    } else {
      // Belt course under each floor, cornice under the roof.
      frame = max(frame, box(fy - 0.07, 0.07, fw.y) * 0.7);
      // Shutters folded back beside the windows on some streets.
      acc = box(abs(cx) - (ww * 0.5 + 0.36), 0.2, fw.x) * box(fy - 1.575, 0.725, fw.y) * has * step(0.66, hb2) * step(ww + 1.3, cell_w);
      acccol = shutcol;
    }
    // Window air conditioners where nobody fitted anything better.
    let ac = rect(cx - 0.12, fy, 0.62, 0.3, 0.64, fw) * has * step(hash2(id * 3.3 + seed * 5.0), 0.3 * (0.75 - wealth));
    acccol = mix(acccol, vec3f(0.5, 0.5, 0.48) * (0.6 + 0.4 * step(0.5, fract(fy / 0.045)) * fine), step(acc, ac));
    acc = max(acc, ac);
    pipe = box(cx + cell_w * 0.5 - 0.09, 0.04, fw.x) * step(hash2(vec2f(id.x, seed * 77.0)), 0.13);
    if (top < 0.75 && H < 250.0) {
      glass = 0.0;
      open = vec4f(0.0);
      bar = vec2f(0.0);
      acc = 0.0;
      dark = 0.0;
      if (top > 0.0) {
        // The cornice: a pale band with a row of dentils and a shadow under it.
        let dent = box(top - 0.2, 0.09, fw.y) * box(fract(uv.x / 0.3) - 0.5, 0.25, fw.x / 0.3) * clamp(1.0 - fw.x / 0.1, 0.0, 1.0);
        frame = 1.0 - 0.3 * dent;
        dark = box(top - 0.72, 0.03, fw.y) * 0.5 * fine;
      } else {
        // Above the roof line: a plain parapet in the wall's own finish.
        frame = 0.0;
      }
    }
    if (ground) {
      glass = 0.0;
      frame = 0.0;
      dark = 0.0;
      acc = 0.0;
      bar = vec2f(0.0);
      open = vec4f(0.0);
      if (ftype == 5u) { pipe = 0.0; }
      else if (wealth > 0.45 && mat >= 0.56) {
        // Rusticated ground floor: deep channels cut along the stucco.
        wall *= 1.0 - 0.3 * box(fract(uv.y / 0.42) - 0.5, 0.06, fw.y / 0.42) * fine;
      }
      if (ftype == 5u) {
        // Shop front: plate glass, mullions, a coloured sign band.
        let sx = (fract(uv.x / 2.6) - 0.5) * 2.6;
        glass = box(uv.y - 1.75, 1.3, fw.y) * box(sx, 1.22, fw.x);
        ocx = sx;
        ocw = 2.6;
        ofy = uv.y;
        open = vec4f(1.22, 0.45, 3.05, 0.1);
        let sid = floor(uv.x / (cell_w * 2.0) + seed * 3.0);
        let hue = hash2(vec2f(sid, seed * 3.0));
        let sign = box(uv.y - 3.25, 0.32, fw.y) * step(0.35, hash2(vec2f(sid, 5.0)));
        let sc = 0.5 + 0.5 * cos(6.283 * (hue + vec3f(0.0, 0.33, 0.67)));
        wall = mix(wall, mix(sc * sc * 0.5, vec3f(0.5), 0.3), sign);
        // The shop's name: a row of pale letters across the middle of the sign.
        let lx = uv.x / 0.22;
        let word = box(fract(uv.x / (cell_w * 2.0) + seed * 3.0) - 0.5, 0.34, fw.x / (cell_w * 2.0));
        let letters = box(uv.y - 3.25, 0.13, fw.y) * step(0.28, hash2(vec2f(floor(lx), sid))) * box(fract(lx) - 0.5, 0.33, fw.x / 0.22) * word * sign * fine;
        wall = mix(wall, vec3f(0.86, 0.85, 0.8), letters);
      } else if (hc < 0.42) {
        // Garage door.
        dark = rect(cx, fy, min(2.4, cell_w - 0.4), 0.0, 2.25, fw);
        darkcol = mix(vec3f(0.3, 0.29, 0.27), base * 0.6, 0.5) * (1.0 - 0.2 * box(fract(uv.y / 0.45) - 0.5, 0.06, fw.y / 0.45) * fine);
      } else if (hc < 0.7) {
        dark = rect(cx, fy, 1.05, 0.0, 2.3, fw);
        darkcol = vec3f(0.06, 0.04, 0.03);
        frame = rect(cx, fy, 1.4, 0.0, 2.5, fw);
      } else {
        glass = rect(cx, fy, ww, 1.0, 2.3, fw);
        frame = rect(cx, fy, ww + 0.26, 0.86, 2.44, fw);
        frame = max(frame, rect(cx, fy, ww + 0.46, 0.8, 0.92, fw));
        open = vec4f(ww * 0.5, 1.0, 2.3, 0.16);
        bar = vec2f(step(0.3, hb), 0.0);
      }
    }
  } else if (ftype == 2u) {
    // Office: three ways of putting windows in a concrete frame.
    let v = fract(seed * 9.1);
    bays = 3.0;
    coat = 0.06 + 0.06 * fract(seed * 13.0);
    // Precast panels two bays wide, a joint between them and under each floor.
    let pj = max(1.0 - box((fract(g.x * 0.5) - 0.5) * 2.0 * cell_w, cell_w - 0.018, fw.x), 1.0 - box(fy - floor_h * 0.5, floor_h * 0.5 - 0.018, fw.y));
    wall *= (0.95 + 0.1 * n2) * (0.95 + 0.1 * hash2(vec2f(floor(g.x * 0.5), id.y) + seed * 3.0));
    wall = mix(wall, wall * 0.5, pj * 0.75 * fine);
    if (v < 0.4) {
      // Ribbon windows between spandrels.
      glass = box(fy - 1.95, 0.9, fw.y) * (1.0 - box(cx, 0.035, fw.x) * 0.8);
      open = vec4f(cell_w * 0.5 - 0.035, 1.05, 2.85, 0.12);
      // A drip edge over each ribbon and a sill under it.
      frame = box(fy - 1.0, 0.045, fw.y) * 0.8;
      dark = box(fy - 2.9, 0.04, fw.y) * 0.6 * fine;
    } else if (v < 0.72) {
      // Punched windows deep in a grid of piers and spandrels.
      let pw = cell_w * 0.5 - 0.24;
      glass = rect(cx, fy, pw * 2.0, 1.0, 2.9, fw);
      frame = rect(cx, fy, pw * 2.0 + 0.14, 0.93, 2.97, fw);
      open = vec4f(pw, 1.0, 2.9, 0.3);
      bays = 2.0;
    } else {
      // Pale piers running the full height, dark spandrels set back between them.
      let pier = 1.0 - box(cx, cell_w * 0.5 - 0.17, fw.x);
      let band = box(fy - 2.0, 1.0, fw.y);
      glass = band * (1.0 - pier);
      dark = (1.0 - band) * (1.0 - pier) * 0.8;
      darkcol = base * 0.28 + 0.01;
      frame = pier;
      open = vec4f(cell_w * 0.5 - 0.17, 1.0, 3.0, 0.24);
    }
    if (uv.y < 4.2) {
      frame = 0.0;
      dark = 0.0;
      // Polished stone around the lobby glazing.
      wall = mix(base * 0.45, vec3f(0.16, 0.15, 0.15), 0.5) * (0.85 + 0.3 * n1);
      glass = box(uv.y - 2.0, 1.6, fw.y) * (1.0 - box((fract(uv.x / 2.4) - 0.5) * 2.4, 0.05, fw.x));
      ocx = (fract(uv.x / 2.4) - 0.5) * 2.4;
      ocw = 2.4;
      ofy = uv.y;
      open = vec4f(1.15, 0.4, 3.6, 0.06);
      bays = 1.0;
    }
    if (top < 1.3 && H < 250.0) { glass = 0.0; frame = 0.0; open = vec4f(0.0); dark = 0.5 * step(0.5, fract(uv.y / 0.16)) * fine; darkcol = vec3f(0.035, 0.03, 0.028); }
  } else if (ftype == 3u) {
    // Curtain wall: glass everywhere, a mullion grid, darker spandrel panels.
    let mull = max(box(cx, 0.03, fw.x), box(fy, 0.06, fw.y));
    let sp = box(fy - 0.5, 0.45, fw.y) * (1.0 - mull);
    // Vision glass between the mullions; back-painted spandrel glass over each floor slab.
    glass = 1.0 - mull * 0.85 - sp;
    span = sp;
    wall = base * 0.5;
    dark = sp * 0.55;
    darkcol = base * 0.35;
    open = vec4f(cell_w * 0.5 - 0.03, 0.95, floor_h, 0.03);
    bays = 4.0;
    // Some towers are near mirrors, some show their insides.
    // No two panes come out of the coating line quite alike.
    coat = mix(0.06, 0.4, smoothstep(0.35, 0.9, fract(seed * 7.77))) * (0.86 + 0.28 * h1);
    tilt_k = 0.022;
    let v = fract(seed * 4.3);
    if (v > 0.62) {
      // Aluminium fins standing proud at every bay.
      let fin = box(cx, 0.075, fw.x);
      frame = fin;
      glass *= 1.0 - fin;
      span *= 1.0 - fin;
      dark *= 1.0 - fin;
    } else if (v < 0.22) {
      // Solid bands: opaque spandrels in stone or metal, not glass.
      wall = mix(base, vec3f(0.68, 0.67, 0.64), 0.3) * (0.93 + 0.14 * n2);
      wall *= 1.0 - 0.45 * (1.0 - box((fract(g.x * 0.5) - 0.5) * 2.0 * cell_w, cell_w - 0.015, fw.x)) * fine;
      span = 0.0;
      dark = 0.0;
    }
    // Plant floors: louvred bands partway up a tall tower and under its roof.
    let every = 14.0 + floor(hb * 12.0);
    if (H > 45.0 && id.y > 2.5 && (top < floor_h * 1.15 || (id.y - every * floor(id.y / every)) > every - 1.5)) {
      glass = 0.0;
      span = 0.0;
      frame = 0.0;
      open = vec4f(0.0);
      wall = base * 0.4 + 0.02;
      dark = (0.3 + 0.55 * step(0.5, fract(uv.y / 0.22)) * fine) * (1.0 - box(cx, 0.05, fw.x));
      darkcol = base * 0.18 + 0.012;
    }
    if (uv.y < 5.0) {
      frame = 0.0;
      dark = 0.0;
      span = 0.0;
      glass = 1.0 - box((fract(uv.x / 3.0) - 0.5) * 3.0, 0.06, fw.x);
      ocx = (fract(uv.x / 3.0) - 0.5) * 3.0;
      ocw = 3.0;
      ofy = uv.y;
      open = vec4f(1.44, 0.05, 4.9, 0.03);
      bays = 1.0;
      coat = 0.06;
    }
  } else {
    // Industrial shed: ribbed sheeting, roller doors, a few high windows.
    wall *= 0.93 + 0.07 * cos(uv.x / 0.3 * 6.283) * fine;
    // The firm's colour in a stripe under the eaves; sheets lap every few metres.
    let stripe = box(top - 1.1, 0.32, fw.y) * step(7.0, H) * step(0.45, hb);
    wall = mix(wall, (0.5 + 0.5 * cos(6.283 * (hb2 + vec3f(0.0, 0.33, 0.67)))) * 0.3 + 0.04, stripe);
    wall *= 1.0 - 0.18 * box(fract(uv.y / 2.4) - 0.5, 0.012, fw.y / 2.4) * fine;
    pipe = box(cx + cell_w * 0.5 - 0.1, 0.05, fw.x) * step(0.5, fract(id.x * 0.5));
    wall = mix(wall, vec3f(0.2, 0.2, 0.19) * (0.8 + 0.4 * n2), step(uv.y, 0.9));
    let door = step(h1, 0.3) * f32(ground);
    dark = rect(cx, uv.y, 4.2, 0.0, 4.4, fw) * door;
    darkcol = vec3f(0.22, 0.23, 0.24) * (1.0 - 0.25 * box(fract(uv.y / 0.3) - 0.5, 0.08, fw.y / 0.3) * fine);
    glass = rect(cx, uv.y, 5.0, 4.9, 5.7, fw) * step(h2, 0.3) * step(6.2, H);
  }

  // Weathering: streaks run down from the roof line and sills, dirt creeps up from the street.
  let streak = textureSampleLevel(t_wave, s_repeat, vec2f(uv.x / 2.3, uv.y / 17.0) + seed * 3.0, 0.0).a;
  let grime = (1.0 - wealth) * (0.55 * smoothstep(0.45, 0.8, streak) + 0.35 * (1.0 - smoothstep(0.0, 1.6, uv.y)));
  wall *= 1.0 - grime * 0.55;
  let homes = ftype != 2u && ftype != 3u && ftype != 4u;
  if (homes && open.x > 0.0) {
    // Dirty water runs off the ends of every sill and stains the wall below.
    let drip = box(abs(ocx) - open.x, 0.07, fw.x) * smoothstep(open.y - 0.95, open.y - 0.12, ofy) * step(ofy, open.y - 0.1);
    wall *= 1.0 - 0.4 * (1.0 - wealth) * drip;
  }
  if (ftype != 3u) {
    // Spray paint at arm's reach where nobody cleans it off.
    let tagn = textureSampleLevel(t_wave, s_repeat, uv / vec2f(2.6, 1.9) + seed * 5.0, log2(max(max(fw.x, fw.y) * 256.0 / 1.9, 1.0))).a;
    let tag = smoothstep(0.6, 0.64, tagn) * box(uv.y - 1.25, 0.85, fw.y) * clamp((0.38 - wealth) * 4.0, 0.0, 1.0);
    let tagc = 0.5 + 0.5 * cos(6.283 * (hash1(floor(uv.x / 2.6 + seed * 9.0)) + vec3f(0.0, 0.33, 0.67)));
    wall = mix(wall, tagc * 0.42, tag * 0.8);
    // A coping or fascia along the top of the wall.
    if (ftype != 1u && ftype != 5u && ftype != 2u) { frame = max(frame, box(top - 0.11, 0.11, fw.y) * 0.8); }
  }
  // Light never quite reaches the foot of a wall or the underside of the eaves.
  let ao = (0.8 + 0.2 * smoothstep(0.0, 2.2, uv.y)) * (1.0 - 0.16 * exp(-max(top, 0.0) / 0.45) * f32(ftype != 3u) * step(0.0, top));
  // Patched repairs in a different shade.
  wall = mix(wall, wall * vec3f(1.12, 1.05, 0.95), step(0.72 + 0.25 * wealth, n1) * 0.5);

  var f: Facade;
  let glasscol = vec3f(0.03, 0.04, 0.055) + base * select(0.0, 0.25, ftype == 3u);
  var a = mix(wall, trim, clamp(frame - glass, 0.0, 1.0));
  a = mix(a, darkcol, clamp(dark, 0.0, 1.0));
  a = mix(a, acccol, clamp(acc, 0.0, 1.0));
  a = mix(a, vec3f(0.1, 0.1, 0.11), pipe * 0.9);
  a = mix(a, glasscol, glass);
  f.bar = bar;
  f.trimc = trim;
  let avg = mix(wall, glasscol, winfrac);
  // Too far for single windows but not for storeys: each floor's row of
  // windows still reads as a darker band, so a facade keeps its floors.
  let aay = clamp(1.0 - fw.y / floor_h * 1.7, 0.0, 1.0) * f32(ftype != 4u);
  let wrow = min(winfrac / 0.48, 1.0) * box(fy - floor_h * 0.52, floor_h * 0.24, fw.y);
  f.albedo = mix(mix(avg, mix(wall, glasscol, wrow), aay), a, aa) * ao;
  f.glass = mix(mix(winfrac, wrow, aay), glass, aa);
  // Who is in tonight: some buildings nearly dark, some blazing. Offices
  // light whole floors at a time; shop fronts stay lit for the street.
  let occ = hash2(vec2f(seed * 53.0, 7.1));
  let office = ftype == 2u || ftype == 3u;
  let hw = hash2(id * 1.37 + seed * 91.0);
  var litp = 0.32 * mix(0.1, 1.7, occ * occ);
  var on = step(1.0 - litp, hw);
  var bright = 0.4 + 0.9 * h2;
  if (office) {
    // After hours: a few floors where the cleaners are, behind tinted glass.
    litp = 0.2 * mix(0.1, 2.2, occ * occ);
    on = step(1.0 - litp, hash2(vec2f(id.y * 3.1, seed * 17.0))) * step(0.1, hw);
    bright *= 0.6;
  }
  if (ground && ftype == 5u) { on = step(0.2, hc); bright = 1.6; litp = 0.8; }
  // Incandescent, warm LED, cold fluorescent; now and then a flickering TV.
  let hk = hash2(id * 2.31 + seed * 7.0);
  var lc = mix(vec3f(1.0, 0.6, 0.28), vec3f(1.0, 0.82, 0.6), smoothstep(0.3, 0.75, hk));
  if (office) { lc = mix(vec3f(0.82, 0.9, 1.0), vec3f(1.0, 0.85, 0.65), step(0.72, hk)); }
  else if (hk > 0.94) { lc = vec3f(0.45, 0.6, 1.0) * (0.65 + 0.45 * sin(G.time.x * 9.0 + hk * 80.0) * sin(G.time.x * 3.7 + h1 * 50.0)); }
  // Lit from the ceiling: brighter toward the top of the window.
  let room = 0.7 + 0.45 * smoothstep(0.6, 2.4, fy);
  // From afar: the average, dimmed (a flat glow reads far brighter than the
  // scattered points it stands for) and broken into lit and dark blocks of a
  // floor or two in long runs, which stay resolvable much further out than
  // single windows.
  let bid = vec2f(floor(g.x / 9.0 + hash1(id.y + seed * 9.0) * 3.0), floor(g.y / 1.5));
  let baa = clamp(1.0 - fw.y / (floor_h * 1.5) * 1.7, 0.0, 1.0);
  let blocks = mix(1.0, smoothstep(0.3, 0.85, hash2(bid + seed * 23.0)) * 1.9 + 0.1, baa);
  f.lit = mix(litp * winfrac * select(0.36, 0.26, office) * blocks, on * bright * room * glass, aa);
  f.lcol = mix(select(vec3f(1.0, 0.72, 0.42), vec3f(1.0, 0.86, 0.68), office), lc, aa);
  f.cell = vec4f(ocx, ofy, ocw, floor_h);
  f.wid = id;
  f.open = open;
  f.tilt = (vec2f(hash2(id * 3.17 + seed * 5.0), hash2(id * 5.71 + seed * 9.0)) - 0.5) * tilt_k;
  f.coat = coat;
  f.span = span * aa;
  f.bays = bays;
  f.on = on * bright;
  f.aa = aa;
  // A name in lights under the roof of some tall buildings: pale letters by
  // day, glowing in the firm's colour at night.
  if ((ftype == 2u || ftype == 3u) && H > 38.0 && H < 250.0 && hb2 > 0.62) {
    let lh = clamp(H * 0.035, 1.4, 3.2);
    let lw = lh * 0.8;
    let n = 4.0 + floor(hb * 6.0);
    let lxs = (fract(uv.x / 70.0 + hb) - 0.5) * 70.0 / lw + n * 0.5;
    let ty = (top - 1.8) / lh;
    if (lxs > 0.0 && lxs < n && ty > 0.0 && ty < 1.0) {
      // Each letter a blocky glyph on a 3 by 5 grid, always with its left stem.
      let gx = floor(fract(lxs) * 4.0);
      let glyph = step(gx, 2.5) * max(step(gx, 0.5), step(0.42, hash2(vec2f(floor(lxs) * 7.0 + gx, floor(ty * 5.0) + seed * 91.0))));
      let s = mix(0.5, glyph, clamp(1.0 - max(fw.x, fw.y) / (lh * 0.2), 0.0, 1.0));
      let hue = 0.5 + 0.5 * cos(6.283 * (fract(seed * 61.0) + vec3f(0.0, 0.33, 0.67)));
      f.albedo = mix(f.albedo, vec3f(0.82), s);
      f.glass *= 1.0 - s;
      f.span *= 1.0 - s;
      f.lit = mix(f.lit, 4.0, s);
      f.lcol = mix(f.lcol, mix(hue, vec3f(1.0), 0.35), s);
      f.open.x = 0.0;
    }
  }
  return f;
}

// ---------------------------------------------------------------- interiors
// The room behind a window, found by casting the view ray into a box the
// size of the room (interior mapping): back wall, side walls, floor and
// ceiling, a layer of furniture (and now and then someone at home), and the
// curtains or blinds hanging just inside the glass.
// q: where the ray meets the glass, in storey coordinates; d: view direction
// in facade space (x along the wall, y up, z into the building).
fn interior(q: vec2f, d: vec3f, f: Facade, ftype: u32, seed: f32, wealth: f32, night: f32, amb: vec3f, sunl: vec3f, pix: f32) -> vec3f {
  let cw = f.cell.z;
  let fh = f.cell.w;
  let office = ftype == 2u || ftype == 3u;
  let shop = ftype == 5u && f.wid.y < 0.5;
  let bays = f.bays;
  let bay = f.wid.x - bays * floor(f.wid.x / bays);
  let rw = cw * bays;
  let x0 = (bay + 0.5) * cw + q.x - rw * 0.5;
  let rid = vec2f(floor(f.wid.x / bays), f.wid.y) + seed * 31.0;
  let h1 = hash2(rid * 1.31);
  let h2 = hash2(rid * 2.73 + 4.0);
  let h3 = hash2(rid * 0.77 + 9.0);
  let depth = select(select(4.2, 6.0, wealth > 0.6), 9.0, office || shop);
  let o = vec3f(x0, q.y, 0.0);
  let dd = vec3f(select(d.x, 1e-4, abs(d.x) < 1e-4), select(d.y, 1e-4, abs(d.y) < 1e-4), max(d.z, 1e-3));
  let tx = (select(-rw * 0.5, rw * 0.5, dd.x > 0.0) - x0) / dd.x;
  let ty = (select(0.02, fh - 0.25, dd.y > 0.0) - q.y) / dd.y;
  let tz = depth / dd.z;
  var t = min(min(tx, ty), tz);
  var p = o + dd * t;

  // Surfaces of the room.
  let paint = mix(vec3f(0.72, 0.68, 0.6), vec3f(0.55, 0.6, 0.62), h2) * mix(0.8, 1.05, h3);
  var alb = paint;
  var emis = 0.0;
  if (t == ty && dd.y < 0.0) {
    // Floor: boards, tiles or carpet.
    alb = select(mix(vec3f(0.32, 0.2, 0.11), vec3f(0.42, 0.4, 0.37), step(0.55, h1)), vec3f(0.2, 0.21, 0.22), office);
    alb *= 0.85 + 0.15 * step(0.5, fract(p.x / 0.18 + step(0.5, fract(p.z / 1.2)) * 0.5));
  } else if (t == ty) {
    // Ceiling, with light panels in offices and shops.
    alb = vec3f(0.78);
    if (office || shop) {
      let pc = vec2f(fract(p.x / 2.4) - 0.5, fract(p.z / 1.8) - 0.5);
      emis = step(abs(pc.x), 0.25) * step(abs(pc.y), 0.15);
    }
  } else if (t == tz) {
    // Back wall: pictures and shelves at home, partitions at work.
    if (!office && !shop) {
      let pic = step(abs(p.x - (h1 - 0.5) * rw * 0.5), 0.35) * step(abs(p.y - 1.6), 0.25);
      alb = mix(alb, mix(vec3f(0.25, 0.3, 0.4), vec3f(0.5, 0.35, 0.2), h3), pic * step(0.35, h2));
    } else {
      alb *= 0.9 + 0.1 * step(0.5, fract(p.x / 1.6));
    }
  } else {
    alb *= 0.85;
  }
  // Furniture standing partway back: sofas, tables and shelves at home; desks,
  // screens and cabinets at work; shelves of goods in shops.
  let zf = depth * (0.35 + 0.35 * h3);
  let tf = zf / dd.z;
  if (tf < t) {
    let pf = o + dd * tf;
    let fx = (h1 - 0.5) * rw * 0.6;
    var hit = 0.0;
    var fc = vec3f(0.0);
    if (office) {
      // A row of desks with screens and chairs.
      let dx = fract(pf.x / 1.6) - 0.5;
      let desk = step(pf.y, 0.75) * step(0.68, pf.y) + step(pf.y, 0.7) * step(abs(dx), 0.04);
      let screen = step(abs(dx - 0.1), 0.22) * step(abs(pf.y - 1.0), 0.17);
      hit = max(desk, screen);
      fc = select(vec3f(0.35, 0.33, 0.3), vec3f(0.03), screen > 0.5);
    } else if (shop) {
      let shelf = step(abs(fract(pf.y / 0.5) - 0.5), 0.35) * step(pf.y, 1.8);
      hit = shelf;
      let gid = floor(pf.x / 0.3) + floor(pf.y / 0.5) * 7.0;
      fc = (0.5 + 0.5 * cos(6.283 * (hash1(gid + seed * 9.0) + vec3f(0.0, 0.33, 0.67)))) * 0.6;
    } else {
      let sofa = step(abs(pf.x - fx), 0.9) * step(pf.y, 0.85) * (1.0 - step(0.45, pf.y) * step(abs(pf.x - fx), 0.75) * step(0.55, h2));
      let shelf = step(abs(pf.x + fx * 0.8), 0.45) * step(pf.y, 1.95) * step(h1, 0.6);
      let lamp = step(abs(pf.x - fx - 1.1), 0.02) * step(pf.y, 1.45) + step(abs(pf.x - fx - 1.1), 0.18) * step(abs(pf.y - 1.5), 0.12);
      hit = max(max(sofa, shelf), lamp * step(0.4, h3));
      fc = select(mix(vec3f(0.35, 0.18, 0.12), vec3f(0.18, 0.22, 0.3), h2), vec3f(0.3, 0.2, 0.12), shelf > 0.5);
    }
    // Somebody home: a figure standing in the room.
    if (!shop && h2 > 0.82) {
      let px = (h3 - 0.5) * rw * 0.5;
      let head = step(length(vec2f(pf.x - px, pf.y - 1.6)), 0.12);
      let body = step(abs(pf.x - px), 0.21 - 0.08 * smoothstep(0.9, 0.5, pf.y)) * step(pf.y, 1.46);
      if (max(head, body) > 0.5) { hit = 1.0; fc = mix(vec3f(0.12, 0.13, 0.18), vec3f(0.45, 0.2, 0.15), h1); }
    }
    if (hit > 0.5) {
      alb = fc;
      t = tf;
      p = pf;
      emis = 0.0;
    }
  }
  // Light: daylight through the window falling off into the room, and the
  // room's own lamps (offices and shops keep theirs on in the day too).
  let lit = f.on * select(night, 1.0, office || shop);
  let fall = 1.0 - 0.6 * clamp(p.z / depth, 0.0, 1.0);
  let day = amb * 0.55 * fall + sunl * 0.004;
  let lamp_l = f.lcol * lit * 0.035 * G.sun_dir.w * (0.55 + 0.45 * smoothstep(0.0, fh, p.y));
  var col = alb * (day + lamp_l) + f.lcol * emis * lit * 0.2 * G.sun_dir.w;

  // Curtains and blinds just inside the glass.
  let ho = f.open;
  let mid = (ho.y + ho.z) * 0.5;
  let wh = hash2(f.wid * 1.91 + seed * 23.0);
  let wh2 = hash2(f.wid * 4.37 + seed * 7.0);
  var cov = 0.0;
  var ccol = vec3f(0.0);
  if (office) {
    // Venetian blinds let down to different heights.
    let drop = select(0.0, (ho.z - ho.y) * mix(0.1, 1.0, wh2 * wh2), wh < 0.35);
    // Slats only where they can be resolved; beyond that their average.
    let slat = mix(0.91, 0.82 + 0.18 * step(0.5, fract(q.y / 0.06)), clamp(1.0 - pix / 0.03, 0.0, 1.0));
    cov = step(ho.z - drop, q.y);
    ccol = vec3f(0.7, 0.7, 0.68) * slat;
  } else if (!shop) {
    // Curtains drawn partly across, and sometimes a net curtain.
    let close = select(mix(0.05, 0.5, wh2), 0.0, wh < 0.25);
    let side = step(ho.x * (1.0 - 2.0 * close), abs(q.x)) * step(0.02, close);
    let folds = 1.0 - 0.12 * (0.5 + 0.5 * sin(q.x * 31.0 + wh * 9.0)) * clamp(1.0 - pix / 0.05, 0.0, 1.0);
    let cc = 0.5 + 0.45 * cos(6.283 * (fract(seed * 3.3 + wh * 0.7) + vec3f(0.0, 0.33, 0.67)));
    let net = select(0.0, 0.55, wh > 0.7) * (1.0 - side);
    cov = max(side, net);
    ccol = mix(mix(cc * vec3f(0.8, 0.72, 0.62), vec3f(0.78, 0.76, 0.72), 0.55), vec3f(0.88, 0.87, 0.84), step(side, 0.5)) * folds;
  }
  if (cov > 0.0) {
    // Fabric is lit from the window side by day and glows with the lamp behind it at night.
    let cl = ccol * (amb * 0.75 + sunl * 0.01) + ccol * f.lcol * lit * 0.03 * G.sun_dir.w;
    col = mix(col, cl, cov);
  }
  return col;
}


// ---------------------------------------------------------------- shading
struct FShade { albedo: vec3f, extra: vec3f, N: vec3f, mirror: f32 };

// A wall's look at P: albedo, light on top of the diffuse (glass reflections,
// rooms behind the windows, lights at night), the normal to shade with and
// how much of it is mirror. N faces the viewer, Tw runs along the wall the
// way uv.x grows; fw is the pixel footprint in uv. Smashed: the glass is out.
fn facade_shade(P: vec3f, V: vec3f, N: vec3f, Tw: vec3f, uv: vec2f, fw: vec2f, base: vec3f, ftype: u32, seed: f32, wealth: f32, H: f32, smashed: bool) -> FShade {
  let l = light_at(P);
  let night = night_at(l.upn);
  let dist = length(P);
  var f = facade(uv, base, ftype, seed, wealth, H, fw);
  if (smashed) {
    // Glass gone: dark empty openings in the frames, nothing lit behind them.
    f.albedo = mix(f.albedo, vec3f(0.02, 0.019, 0.018), f.glass * 0.9);
    f.glass = 0.0;
    f.span = 0.0;
    f.lit = 0.0;
    f.open.x = 0.0;
  }
  var albedo = f.albedo;
  // Facade frame: x along the wall (the way uv.x runs), y up, z into the building.
  let Bw = normalize(cross(N, Tw)) * sign(dot(cross(N, Tw), l.upn));
  let Vt = vec3f(dot(V, Tw), dot(V, Bw), dot(V, -N));
  let St = vec3f(dot(G.sun_dir.xyz, Tw), dot(G.sun_dir.xyz, Bw), dot(G.sun_dir.xyz, N));
  let sh = shadow_at(P, N);
  // Close up, windows get depth: a recess, and a room behind the glass.
  let inner = f.aa * (1.0 - smoothstep(350.0, 750.0, dist));
  var gl = f.glass;
  // Pane-by-pane tilt only where single panes are resolved; further out it would shimmer.
  var tilt = f.tilt * f.aa * (1.0 - smoothstep(600.0, 1400.0, dist));
  var q = f.cell.xy;
  var gsun = sh;
  var room = vec3f(0.0);
  let o = f.open;
  if (inner > 0.0 && o.x > 0.0) {
    let mid = (o.y + o.z) * 0.5;
    let hh = (o.z - o.y) * 0.5;
    let fwq = max(fw, vec2f(0.004));
    let front = box(f.cell.x, o.x, fwq.x) * box(f.cell.y - mid, hh, fwq.y);
    // Where the view ray meets the glass set back in the opening.
    q = f.cell.xy + Vt.xy * (o.w / max(Vt.z, 0.08));
    let back = box(q.x, o.x, fwq.x) * box(q.y - mid, hh, fwq.y);
    // Glazing bars standing in the plane of the glass.
    let mb = max(box(q.x, 0.024, fwq.x) * step(0.5, f.bar.x), box(q.y - f.bar.y, 0.024, fwq.y) * step(0.01, f.bar.y)) * front * back;
    let pane = front * back - mb;
    let reveal = front - front * back;
    // The sun reaching the glass past the reveal.
    if (St.z > 0.0) {
      let qs = q + St.xy * (o.w / max(St.z, 0.05));
      gsun = sh * box(qs.x, o.x, 0.02) * box(qs.y - mid, hh, 0.02);
    }
    // A plate of glass bows slightly: its reflection swims toward the edges.
    tilt += vec2f(q.x / o.x, (q.y - mid) / hh) * select(0.006, 0.012, ftype == 3u);
    let rev_col = albedo * 0.75;
    let near_alb = mix(mix(mix(albedo, rev_col, reveal), f.trimc, mb), vec3f(0.012), pane);
    albedo = mix(albedo, near_alb, inner);
    gl = mix(gl, pane, inner);
    room = interior(q, Vt, f, ftype, seed, wealth, night, l.amb, l.sun * max(St.z, 0.0) * sh, max(fwq.x, fwq.y)) * pane;
  }
  // Glass: Fresnel reflection of sky, clouds and ground, the sun's glint,
  // and what is behind it seen through the tint.
  let Ng = normalize(N + Tw * tilt.x + Bw * tilt.y);
  let R = reflect(V, Ng);
  let ndv = max(dot(-V, Ng), 0.0);
  let F = f.coat + (1.0 - f.coat) * pow(1.0 - ndv, 5.0);
  let env = env_reflect(P, R, l.upn, dist < 2500.0);
  let Hh = normalize(G.sun_dir.xyz - V);
  let glint = pow(max(dot(Ng, Hh), 0.0), 1400.0) * 9.0 * gsun * F;
  let tint = select(vec3f(0.86, 0.9, 0.88), mix(vec3f(0.75), base / max(luma(base), 0.05) * 0.5, 0.5) * (1.0 - f.coat), ftype == 3u);
  let refl = env * F + l.sun * glint;
  var extra = (refl * gl) + (1.0 - F) * tint * room * inner;
  // Spandrel glass reflects the same way, with a dark panel behind it.
  extra += env * (F + 0.03) * f.span;
  // Lit rooms after dusk from afar, and the street lights washing the lowest floors.
  extra += f.lcol * 0.025 * G.sun_dir.w * night * f.lit * (1.0 - inner * step(0.001, o.x));
  extra += albedo * LAMP_COL * 0.012 * G.sun_dir.w * night * exp(-max(uv.y, 0.0) / 4.5) * (1.0 - gl);
  return FShade(albedo, extra, Ng, (gl + f.span) * F * (1.0 - smoothstep(900.0, 1500.0, dist)));
}
