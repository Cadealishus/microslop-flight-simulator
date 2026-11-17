//! Aircraft made from downloaded models: the F-14A Tomcat, A-10C, F-117A,
//! L-39ZA and C-17A.
//!
//! `assets/blender/import_plane.py` cuts each model into the pieces the game
//! moves and breaks off, by where they are (and, where the model names them,
//! its control surfaces), bakes its textures and measures it; the measurements
//! are in `measured/<key>.rs`. Here the pieces are given their roles and motions by
//! name, and each type its flying qualities, starting from the type it is most
//! like (the fighters from the Hornet, the C-17 from the 747).
//!
//! What these models do not have, the game does without: their control
//! surfaces (except the C-17's) are part of the wing and tail, and their
//! landing gear slides up into the airframe rather than folding.
use super::*;
use crate::airframe::*;
use crate::render::PartVertex;
use std::io::Read;

const KT: f64 = 1.0 / 1.94384;

// ---------------------------------------------------------------- the model

/// The pieces of an imported model (import_plane.py's format: per piece its
/// name, hinge, how far it stows, and vertices of 24 bytes: position, normal
/// in three signed bytes, style and texture layer in a nibble each, texture
/// coordinates).
pub fn build(bytes: &[u8], lend_gear: Option<[Vec3; 3]>) -> Model {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(bytes).read_to_end(&mut raw).expect("imported aircraft model");
    let mut at = 0usize;
    let mut take = |n: usize| {
        let s = &raw[at..at + n];
        at += n;
        s.to_vec()
    };
    let u32_ = |b: Vec<u8>| u32::from_le_bytes(b.try_into().unwrap());
    let f32s = |b: &[u8]| -> Vec<f32> { b.chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect() };
    assert_eq!(take(4), b"IMP1", "imported aircraft model: not a model file");
    let n = u32_(take(4));
    let mut b = Builder::default();
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for _ in 0..n {
        let len = u32_(take(4)) as usize;
        let name: &'static str = Box::leak(String::from_utf8_lossy(&take(len)).into_owned().into_boxed_str());
        let h = f32s(&take(28));
        let (pivot, axis, stow) = (Vec3::new(h[0], h[1], h[2]), Vec3::new(h[3], h[4], h[5]), h[6]);
        let nv = u32_(take(4)) as usize;
        let vb = take(nv * 24);
        let mut mesh = Mesh::default();
        mesh.v.reserve(nv);
        for c in vb.chunks(24) {
            let f = f32s(&c[0..12]);
            let pos = [f[0], f[1], f[2]];
            let nrm = [c[12] as i8 as f32 / 127.0, c[13] as i8 as f32 / 127.0, c[14] as i8 as f32 / 127.0];
            let (style, layer) = (c[15] >> 4, (c[15] & 15) as f32);
            let uv = f32s(&c[16..24]);
            let (id, pattern, col) = match style {
                1 => (mat::SATIN, pat::TEX, [1.0; 3]),
                2 => (mat::PLASTIC, pat::TEXIN, [1.0; 3]),
                3 => (mat::PAINT, pat::TEXCUT, [1.0; 3]),
                4 => (mat::GLASS, 0.0, [0.03, 0.04, 0.05]),
                _ => (mat::PAINT, pat::TEX, [1.0; 3]),
            };
            lo = lo.min(Vec3::from(pos));
            hi = hi.max(Vec3::from(pos));
            mesh.v.push(PartVertex { pos, nrm, col: [col[0], col[1], col[2], 1.0], uv: [uv[0], uv[1]], mat: [id, pattern, 0.0, layer] });
        }
        let ni = u32_(take(4)) as usize;
        let ib = take(ni * 4);
        mesh.i = ib.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        let s = if name.starts_with("left") { -1.0 } else { 1.0 };
        let gear = |k: u8| (G_GEAR, Role::Gear(k), Anim::slide(Drive::new(Input::Gear, -stow).smooth(0.15, 0.85).offset(stow), Vec3::Y));
        let (group, role, anim) = match name {
            "radome" => (G_PROP, Role::Other, Anim::Fixed),
            "nose" => (G_ENGINE, Role::Other, Anim::Fixed),
            "tail cone" => (G_TAIL, Role::TailCone, Anim::Fixed),
            "tail" => (G_TAIL, Role::Stab, Anim::Fixed),
            "left wing" => (G_LWING, Role::Other, Anim::Fixed),
            "right wing" => (G_RWING, Role::Other, Anim::Fixed),
            "left aileron" => (G_LWING, Role::Aileron(0), Anim::hinge(Drive::new(Input::Aileron(s), 0.35))),
            "right aileron" => (G_RWING, Role::Aileron(1), Anim::hinge(Drive::new(Input::Aileron(s), 0.35))),
            "left elevator" | "right elevator" => (G_TAIL, Role::Elevator, Anim::hinge(Drive::new(Input::Elevator, 0.38))),
            "left flap" => (G_LWING, Role::Flap(0), Anim::hinge(Drive::new(Input::Flaps, 35f32.to_radians()))),
            "right flap" => (G_RWING, Role::Flap(1), Anim::hinge(Drive::new(Input::Flaps, 35f32.to_radians()))),
            "rudder" | "left rudder" => (G_TAIL, Role::Rudder, Anim::hinge(Drive::new(Input::Rudder, 0.45))),
            "nose gear" => gear(0),
            "left main gear" => gear(1),
            "right main gear" => gear(2),
            _ => (G_FUSE, Role::Other, Anim::Fixed),
        };
        let k = b.add(name, group, mesh, anim, pivot, axis, None);
        b.role(k, role);
        b.pieces[k].glass = name == "glass";
    }
    if let Some(to) = lend_gear {
        super::fa18::gear_into(&mut b, to);
    }
    // Every breakable group needs something in it, even if the model put nothing there.
    for g in 0..N_GROUPS {
        if !b.pieces.iter().any(|p| p.group == g && !p.glass) {
            let mut m = Mesh::default();
            let v = PartVertex { col: [0.5, 0.5, 0.5, 1.0], ..Default::default() };
            m.v = vec![v; 3];
            m.i = vec![0, 1, 2];
            b.fixed("(nothing)", g, m);
        }
    }
    let size = (hi - lo).max_element();
    let vox = (size / 260.0).max(0.04);
    bake_ao(&mut b.pieces, &AoBake { vox, reach: vox * 45.0, rays: 16, open: &|_, _| false });
    Model::finish(b.pieces)
}

/// A wing section from the measured planform (measured `WING`).
pub fn wing(w: &[f32; 8], x: f32) -> Sect {
    let [x0, x1, le0, sweep, c0, c1, y0, di] = *w;
    let x = x.max(0.0);
    let d = (x - x0).max(0.0);
    let c = c0 + (c1 - c0) * (d / (x1 - x0)).min(1.0);
    Sect { le: Vec3::new(x, y0 + di * d, le0 + sweep * d), c, cdir: Vec3::Z, up: Vec3::new(-di, 1.0, 0.0).normalize(), tf: 1.0 }
}

/// The fuselage at station z from the measured stations (measured `FUSE`): top,
/// height of its widest point, bottom.
pub fn fuse(f: &[[f32; 5]], z: f32) -> (f32, f32, f32) {
    let r = |a: &[f32; 5]| (a[1], a[2], a[3]);
    if z <= f[0][0] {
        return r(&f[0]);
    }
    for w in f.windows(2) {
        if z <= w[1][0] {
            let t = (z - w[0][0]) / (w[1][0] - w[0][0]).max(1e-4);
            let (a, b) = (r(&w[0]), r(&w[1]));
            return (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t);
        }
    }
    r(&f[f.len() - 1])
}

/// The panel is below the glareshield, whichever way the eye turns.
pub fn sees_inside(d: Vec3) -> bool {
    d.y < -0.42
}

/// A landing gear leg carrying `load` (N) at rest, its oleo `travel` (m) down to 40 % there.
pub const fn leg(at: DVec3, wheel_r: f64, load: f64, travel: f64, nose: bool, name: &'static str) -> Leg {
    let k = load / (0.4 * travel);
    Leg {
        at,
        wheel_r,
        travel,
        k,
        c: 0.12 * k,
        nf_max: 4.0 * load,
        steer: if nose { 1.0 } else { 0.0 },
        brakes: !nose,
        side_k: 0.3 * k,
        side_mu: 0.8,
        roll_k: 0.004 * k,
        roll_mu: 0.02,
        brake_k: if nose { 0.0 } else { 0.09 * k },
        brake_mu: if nose { 0.0 } else { 0.6 },
        fold_vn: 6.0,
        fold_kick: DVec3::new(0.3, 0.0, if nose { 0.0 } else { 0.2 }),
        name,
    }
}

/// The three legs at the measured points, for an aircraft of `mass` kg.
macro_rules! legs {
    ($g:path, $mass:expr, $travel:expr) => {{
        use $g as g;
        // the Hornet's wheels, when its gear is lent
        const R: f64 = if g::GEAR_MODELLED { g::WHEEL_R } else { 0.38 };
        &[
            leg(g::LEGS[0], R * 0.7, 0.10 * $mass * 9.81, $travel, true, "NOSE GEAR"),
            leg(g::LEGS[1], R, 0.45 * $mass * 9.81, $travel, false, "LEFT MAIN GEAR"),
            leg(g::LEGS[2], R, 0.45 * $mass * 9.81, $travel, false, "RIGHT MAIN GEAR"),
        ]
    }};
}

/// The airframe sliding along the ground, for an aircraft of `mass` kg.
const fn hull(mass: f64) -> Hull {
    Hull { k: 80.0 * mass, c: 10.0 * mass, nf_max: 160.0 * mass, drag: 3.0 * mass }
}

/// How the mass is shared between the breakable groups.
const fn groups(mass: f32) -> [(&'static str, f32); 7] {
    [("fuselage", 0.42 * mass), ("tail", 0.06 * mass), ("left wing", 0.15 * mass), ("right wing", 0.15 * mass), ("nose section", 0.06 * mass), ("radome", 0.004 * mass), ("gear", 0.04 * mass)]
}

/// Each type's functions over its measurements.
macro_rules! shape {
    ($m:ident) => {
        pub mod $m {
            use crate::aircraft::measured::$m as g;
            pub fn build() -> crate::airframe::Model {
                super::build(g::MODEL, if g::GEAR_MODELLED { None } else { Some(g::LEGS.map(|l| l.as_vec3())) })
            }
            pub fn wing_sect(x: f32) -> crate::airframe::Sect {
                super::wing(&g::WING, x)
            }
            pub fn fuse(z: f32) -> (f32, f32, f32) {
                super::fuse(&g::FUSE, z)
            }
            pub fn sees_inside(_eye: glam::Vec3, d: glam::Vec3) -> bool {
                super::sees_inside(d)
            }
        }
    };
}
shape!(f14);
shape!(a10);
shape!(f117);
shape!(l39);
shape!(c17);

/// The measured geometry of a type (everything but how it flies), the rest
/// from the type it is most like (the Hornet unless another is given).
macro_rules! geometry {
    ($m:ident) => {
        geometry!($m, fa18::BASE)
    };
    ($m:ident, $base:expr) => {
        Spec {
            still: measured::$m::STILL,
            build: $m::build,
            textures: measured::$m::TEXTURES,
            span: measured::$m::WING[1],
            wing_sect: $m::wing_sect,
            fuse: $m::fuse,
            aileron: measured::$m::AILERON,
            tank: measured::$m::TANK,
            tail: measured::$m::TAIL,
            eye: measured::$m::EYE,
            sees_inside: $m::sees_inside,
            contacts: measured::$m::CONTACTS,
            tail_pts: measured::$m::TAIL_PTS,
            wing_st: [0.15, 0.35, 0.55, 0.75, 0.92],
            hard: measured::$m::HARD,
            hole: measured::$m::HOLE,
            ..$base
        }
    };
}
use super::measured;

// ---------------------------------------------------------------- the types

const F14_MASS: f64 = 26_500.0;
pub static F14: Spec = Spec {
    id: 4,
    name: "F-14A Tomcat",
    keys: &["f14", "f-14", "tomcat"],
    blurb: "Swing-wing carrier fighter, 26.5 t. Two TF30s with afterburners (Space). 140 kt approach.",
    groups: groups(F14_MASS as f32),
    gear: legs!(measured::f14, F14_MASS, 0.35),
    hull: hull(F14_MASS),
    mass: F14_MASS,
    inertia: DVec3::new(361.0e3, 447.0e3, 131.0e3),
    wing_area: 52.5,
    b: 19.55,
    cd0: 0.024,
    k_ind: 0.09,
    // Two TF30-P-414As: 93 kN each in afterburner, about 55 kN dry.
    engine: Engine::Fan(Fan { n: 2, thrust: 52_000.0, idle: 0.62, idle_thrust: 0.06, n1_rpm: 14_000.0, spool_up: 0.8, spool_down: 0.7, reverse: 0.0, reverser_time: 1.0, boost: 1.75, mach_lapse: [0.25, 0.45] }),
    vne: 780.0 * KT,
    mmo: 2.34,
    g_limit: 7.5,
    g_break: 11.0,
    fbw_g: 6.5,
    cruise: 200.0,
    approach: (72.0, 2),
    cam_dist: 30.0,
    shadow_r: 13.0,
    ..geometry!(f14)
};

const A10_MASS: f64 = 15_000.0;
pub static A10: Spec = Spec {
    id: 5,
    name: "A-10C Thunderbolt II",
    keys: &["a10", "a-10", "warthog", "thunderbolt"],
    blurb: "Straight-winged ground attacker, 15 t. Two TF34 fans, no afterburner: slow, tough and steady. 120 kt approach.",
    groups: groups(A10_MASS as f32),
    gear: legs!(measured::a10, A10_MASS, 0.3),
    hull: hull(A10_MASS),
    mass: A10_MASS,
    inertia: DVec3::new(147.0e3, 182.0e3, 61.0e3),
    wing_area: 47.0,
    b: 17.53,
    cl0: 0.15,
    cl_a: 5.0,
    cd0: 0.032,
    k_ind: 0.05,
    stall_alpha: 0.3,
    alpha_trim: 0.26,
    mcrit: 0.72,
    cd_wave: 0.0,
    // Two TF34-GE-100As, 40 kN each.
    engine: Engine::Fan(Fan { n: 2, thrust: 40_000.0, idle: 0.5, idle_thrust: 0.05, n1_rpm: 7_300.0, spool_up: 0.6, spool_down: 0.6, reverse: 0.0, reverser_time: 1.0, boost: 1.0, mach_lapse: [0.6, 0.3] }),
    vne: 450.0 * KT,
    mmo: 0.75,
    g_limit: 7.33,
    g_break: 11.0,
    fbw_g: 6.0,
    cruise: 150.0,
    approach: (62.0, 2),
    cam_dist: 26.0,
    shadow_r: 11.0,
    sound: Sound::Jet,
    ..geometry!(a10)
};

const F117_MASS: f64 = 20_000.0;
pub static F117: Spec = Spec {
    id: 6,
    name: "F-117A Nighthawk",
    keys: &["f117", "f-117", "nighthawk"],
    blurb: "Faceted stealth attacker, 20 t. Two F404s without afterburners, subsonic only, a long fast approach at 165 kt.",
    groups: groups(F117_MASS as f32),
    gear: legs!(measured::f117, F117_MASS, 0.3),
    hull: hull(F117_MASS),
    mass: F117_MASS,
    inertia: DVec3::new(298.0e3, 369.0e3, 46.0e3),
    wing_area: 84.8,
    b: 13.2,
    cl0: 0.0,
    cl_a: 3.2,
    cd0: 0.03,
    k_ind: 0.2,
    stall_alpha: 0.4,
    mcrit: 0.82,
    cd_wave: 0.05,
    // Two F404-F1D2s, 48 kN each.
    engine: Engine::Fan(Fan { n: 2, thrust: 48_000.0, idle: 0.6, idle_thrust: 0.06, n1_rpm: 13_200.0, spool_up: 0.9, spool_down: 0.8, reverse: 0.0, reverser_time: 1.0, boost: 1.0, mach_lapse: [0.4, 0.3] }),
    vne: 560.0 * KT,
    mmo: 0.92,
    g_limit: 6.0,
    g_break: 9.0,
    fbw_g: 6.0,
    cruise: 220.0,
    approach: (85.0, 2),
    cam_dist: 30.0,
    shadow_r: 12.0,
    sound: Sound::Jet,
    ..geometry!(f117)
};

const L39_MASS: f64 = 4_600.0;
pub static L39: Spec = Spec {
    id: 7,
    name: "L-39ZA Albatros",
    keys: &["l39", "l-39", "albatros"],
    blurb: "Two-seat jet trainer, 4.6 t. One small AI-25 fan, gentle and nimble. 100 kt approach.",
    groups: groups(L39_MASS as f32),
    gear: legs!(measured::l39, L39_MASS, 0.22),
    hull: hull(L39_MASS),
    mass: L39_MASS,
    inertia: DVec3::new(25.0e3, 31.0e3, 5.5e3),
    wing_area: 18.8,
    b: 9.46,
    cl0: 0.12,
    cl_a: 4.8,
    cd0: 0.024,
    k_ind: 0.06,
    stall_alpha: 0.3,
    alpha_trim: 0.26,
    mcrit: 0.78,
    cd_wave: 0.0,
    // One AI-25TL, 16.9 kN.
    engine: Engine::Fan(Fan { n: 1, thrust: 16_900.0, idle: 0.5, idle_thrust: 0.05, n1_rpm: 10_600.0, spool_up: 0.7, spool_down: 0.7, reverse: 0.0, reverser_time: 1.0, boost: 1.0, mach_lapse: [0.5, 0.3] }),
    vne: 430.0 * KT,
    mmo: 0.8,
    g_limit: 8.0,
    g_break: 12.0,
    fbw_g: 6.5,
    cruise: 120.0,
    approach: (52.0, 2),
    cam_dist: 18.0,
    shadow_r: 8.0,
    sound: Sound::Jet,
    ..geometry!(l39)
};

const C17_MASS: f64 = 200_000.0;
pub static C17: Spec = Spec {
    id: 8,
    name: "C-17A Globemaster III",
    keys: &["c17", "c-17", "globemaster"],
    blurb: "Four-engined strategic airlifter, 200 t, T-tail. Short-field capable: reversers on Z, 125 kt approach.",
    groups: groups(C17_MASS as f32),
    gear: legs!(measured::c17, C17_MASS, 0.5),
    hull: hull(C17_MASS),
    // the flight deck: the left seat, behind the windshield
    mass: C17_MASS,
    inertia: DVec3::new(2.2e7, 3.3e7, 1.2e7),
    wing_area: 353.0,
    b: 51.75,
    // Four F117-PW-100s, 180 kN each, with reversers.
    engine: Engine::Fan(Fan { n: 4, thrust: 180_000.0, idle: 0.25, idle_thrust: 0.035, n1_rpm: 4_000.0, spool_up: 0.6, spool_down: 0.5, reverse: 0.4, reverser_time: 2.0, boost: 1.05, mach_lapse: [0.78, 0.38] }),
    vne: 350.0 * KT,
    mmo: 0.825,
    cruise: 210.0,
    // the left seat, at the height of its head-up display
    eye: Vec3::new(-0.78, 1.8, -15.35),
    approach: (64.0, b747::BASE.approach.1),
    cam_dist: 80.0,
    shadow_r: 34.0,
    ..geometry!(c17, b747::BASE)
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_models_build_whole() {
        for s in [&F14, &A10, &F117, &L39, &C17] {
            let m = (s.build)();
            let (c, h) = m.bounds();
            for p in &m.pieces {
                assert!(p.mesh.v.iter().all(|v| v.pos.iter().chain(v.nrm.iter()).all(|x| x.is_finite())), "{}: {} has NaNs", s.name, p.name);
            }
            for g in &m.groups {
                assert!(!g.i.is_empty(), "{}: a breakable group is empty", s.name);
            }
            println!("{}: {} pieces, {:.1} m long, {:.1} m across, centre {c:?}", s.name, m.pieces.len(), h.z * 2.0, h.x * 2.0);
        }
    }

    #[test]
    fn imported_types_fly() {
        for s in [&F14, &A10, &F117, &L39, &C17] {
            let p = crate::plane::Plane::spawn(s, 0.6, -2.1, 1500.0, 0.0, s.cruise);
            let t = p.thrust(1.0, s.cruise, 1500.0);
            let q = 0.5 * 1.06 * s.cruise * s.cruise;
            let cl = s.mass * 9.81 / (q * s.wing_area);
            let drag = q * s.wing_area * (s.cd0 + s.k_ind * cl * cl);
            println!("{}: thrust {t:.0} N against drag {drag:.0} N at cruise", s.name);
            assert!(t > drag, "{} cannot hold its cruise", s.name);
        }
    }
}
