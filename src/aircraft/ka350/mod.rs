//! Beechcraft King Air 350: a twin turboprop, 6.3 t, with a T-tail.
//!
//! Like the Hornet, the airframe is a mesh made elsewhere: a textured model
//! cut into its moving pieces by `assets/blender/ka350.py`, which writes
//! `assets/ka350.bin.gz` (one mesh per piece, with the hinge each turns about)
//! and the textures in `assets/ka350/` (render.rs `ka350_textures`, drawn by
//! plane.wgsl under the patterns `pat::TEX`, `TEXIN` and `TEXCUT`). How the
//! pieces move, what they belong to and what they are to the damage model is
//! set here, by name. Rebuild the asset with
//!
//!   blender -b --factory-startup --python assets/blender/ka350.py -- <ka350.glb> assets
//!
//! The cockpit and cabin are part of the model: their panels are pictures,
//! so the instruments on them do not move.
//!
//! The two engines are flown as one (the flight model has a single engine and
//! propeller): the figures below are for the pair.
//!
//! Body frame as everywhere: +X right, +Y up, -Z forward, origin at the centre
//! of gravity.
use super::*;
use crate::airframe::*;
use crate::render::PartVertex;
use std::collections::HashMap;
use std::io::Read;

const KT: f64 = 1.0 / 1.94384;

// ---------------------------------------------------------------- geometry (measured off the model by ka350.py)

/// Fuselage stations: z, top, the height of its widest point, bottom.
const FUSE: [[f32; 4]; 12] = [
    [-4.68, -0.20, -0.28, -0.36],
    [-4.55, -0.06, -0.28, -0.50],
    [-3.75, 0.23, -0.20, -0.65],
    [-2.95, 0.41, -0.15, -0.75],
    [-1.75, 0.99, 0.05, -0.81],
    [0.00, 1.02, 0.10, -0.80],
    [2.75, 1.05, 0.10, -0.72],
    [4.25, 1.00, 0.20, -0.51],
    [5.25, 0.92, 0.25, -0.41],
    [6.25, 0.82, 0.40, 0.05],
    [7.25, 0.60, 0.42, 0.31],
    [8.17, 0.68, 0.64, 0.60],
];

fn fuse(z: f32) -> (f32, f32, f32) {
    let f = |r: [f32; 4]| (r[1], r[2], r[3]);
    if z <= FUSE[0][0] {
        return f(FUSE[0]);
    }
    for w in FUSE.windows(2) {
        let (a, b) = (w[0], w[1]);
        if z <= b[0] {
            let t = (z - a[0]) / (b[0] - a[0]);
            return (a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t);
        }
    }
    f(FUSE[FUSE.len() - 1])
}

// The wing: where it leaves the fuselage, its tip (short of the winglet), the
// leading edge (straight out to the nacelle, swept beyond), chords, height
// and dihedral (m up per m out).
const W_X0: f32 = 0.8;
const W_X1: f32 = 8.3;
const W_KINK: f32 = 3.3;
const W_LE0: f32 = -0.45;
const W_SWEEP: f32 = 0.11;
const W_C0: f32 = 1.8;
const W_C1: f32 = 0.8;
const W_Y0: f32 = -0.6;
const W_DI: f32 = 0.1;

pub fn wing_sect(x: f32) -> Sect {
    let x = x.max(0.0);
    let d = (x - W_X0).max(0.0);
    let c = W_C0 + (W_C1 - W_C0) * (d / (W_X1 - W_X0)).min(1.0);
    Sect { le: Vec3::new(x, W_Y0 + W_DI * d, W_LE0 + W_SWEEP * (x - W_KINK).max(0.0)), c, cdir: Vec3::Z, up: Vec3::new(-W_DI, 1.0, 0.0).normalize(), tf: 1.0 }
}

/// Where the propellers turn, and how far their tips reach.
const PROP_HUB: Vec3 = Vec3::new(2.566, -0.251, -3.381);
const PROP_R: f32 = 1.353;

/// The captain's eye, in the left seat.
const EYE: Vec3 = Vec3::new(-0.40, 0.60, -1.70);

/// Does a view from the eye along `d` end on the flight deck (rather than
/// out of the windshield or the side window beside the seat)?
fn sees_inside(_eye: Vec3, d: Vec3) -> bool {
    let yaw = d.x.atan2(-d.z).to_degrees();
    let pitch = d.y.clamp(-1.0, 1.0).asin().to_degrees();
    let windshield = (-58.0..72.0).contains(&yaw) && (-11.0..22.0).contains(&pitch);
    let side = (-115.0..-58.0).contains(&yaw) && (-14.0..20.0).contains(&pitch);
    !(windshield || side)
}

// ---------------------------------------------------------------- the model

struct Obj {
    pivot: Vec3,
    axis: Vec3,
    /// how far it turns to stow (the gear legs and their doors), rad
    angle: f32,
    mesh: Mesh,
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let s = &self.b[self.at..self.at + n];
        self.at += n;
        s
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn v3(&mut self) -> Vec3 {
        Vec3::new(self.f32(), self.f32(), self.f32())
    }
}

/// The pieces written by ka350.py, by name. A vertex there is 24 bytes:
/// position, normal (three signed bytes), style and texture layer (a nibble
/// each), texture coordinates.
fn load() -> HashMap<String, Obj> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(&include_bytes!("../../../assets/ka350.bin.gz")[..]).read_to_end(&mut raw).expect("ka350 model");
    let mut r = Reader { b: &raw, at: 0 };
    assert_eq!(r.take(4), b"KA35", "ka350 model: not a model file");
    let n = r.u32();
    let mut out = HashMap::new();
    for _ in 0..n {
        let len = r.u32() as usize;
        let name = String::from_utf8_lossy(r.take(len)).into_owned();
        let (pivot, axis, angle) = (r.v3(), r.v3(), r.f32());
        let nv = r.u32() as usize;
        let mut mesh = Mesh::default();
        mesh.v.reserve(nv);
        for _ in 0..nv {
            let pos = [r.f32(), r.f32(), r.f32()];
            let b = r.take(4);
            let nrm = [b[0] as i8 as f32 / 127.0, b[1] as i8 as f32 / 127.0, b[2] as i8 as f32 / 127.0];
            let (style, layer) = (b[3] >> 4, (b[3] & 15) as f32);
            let uv = [r.f32(), r.f32()];
            // style: 0 glossy paint, 1 satin metal, 2 interior, 3 decal, 4 glass, 5 light lens
            let (id, pattern, col) = match style {
                0 => (mat::PAINT, pat::TEX, [0.80, 0.80, 0.78]),
                1 => (mat::SATIN, pat::TEX, [1.0; 3]),
                2 => (mat::PLASTIC, pat::TEXIN, [1.0; 3]),
                4 => (mat::GLASS, 0.0, [0.03, 0.04, 0.05]),
                _ => (mat::PAINT, pat::TEXCUT, [1.0; 3]),
            };
            mesh.v.push(PartVertex { pos, nrm, col: [col[0], col[1], col[2], 1.0], uv, mat: [id, pattern, 0.0, layer] });
        }
        let ni = r.u32() as usize;
        mesh.i = (0..ni).map(|_| r.u32()).collect();
        out.insert(name, Obj { pivot, axis, angle, mesh });
    }
    out
}

/// Every piece: name (as ka350.py writes it), breakable group, what it is to
/// the damage model, and the piece it hangs from (listed before it). The
/// engines and propellers go with the wings they are on.
const PIECES: &[(&str, usize, Role, Option<&str>)] = &[
    ("radome", G_PROP, Role::Other, None),
    ("nose", G_ENGINE, Role::Other, None),
    ("fuselage", G_FUSE, Role::Other, None),
    ("interior", G_FUSE, Role::Other, None),
    ("tail cone", G_TAIL, Role::TailCone, None),
    ("tail", G_TAIL, Role::Stab, None),
    ("left wing", G_LWING, Role::Other, None),
    ("right wing", G_RWING, Role::Other, None),
    ("left aileron", G_LWING, Role::Aileron(0), None),
    ("right aileron", G_RWING, Role::Aileron(1), None),
    ("left flap", G_LWING, Role::Flap(0), None),
    ("right flap", G_RWING, Role::Flap(1), None),
    ("left elevator", G_TAIL, Role::Elevator, None),
    ("right elevator", G_TAIL, Role::Elevator, None),
    ("rudder", G_TAIL, Role::Rudder, None),
    ("left prop", G_LWING, Role::Prop, None),
    ("right prop", G_RWING, Role::Prop, None),
    ("left spinner", G_LWING, Role::Prop, None),
    ("right spinner", G_RWING, Role::Prop, None),
    ("nose leg", G_GEAR, Role::Gear(0), None),
    ("nose fork", G_GEAR, Role::Gear(0), Some("nose leg")),
    ("nose wheel", G_GEAR, Role::Gear(0), Some("nose fork")),
    ("left main leg", G_GEAR, Role::Gear(1), None),
    ("left main wheel", G_GEAR, Role::Gear(1), Some("left main leg")),
    ("right main leg", G_GEAR, Role::Gear(2), None),
    ("right main wheel", G_GEAR, Role::Gear(2), Some("right main leg")),
    ("left nose door", G_ENGINE, Role::Other, None),
    ("right nose door", G_ENGINE, Role::Other, None),
    ("left outer door", G_LWING, Role::Other, None),
    ("left inner door", G_LWING, Role::Other, None),
    ("right inner door", G_RWING, Role::Other, None),
    ("right outer door", G_RWING, Role::Other, None),
    ("glass", G_FUSE, Role::Other, None),
];

/// How a piece moves. Hinges point so that a positive angle puts a trailing
/// edge down (the rudder: to the right); the gear is modelled down and locked,
/// its doors open, and a piece's `stow` is how far it turns to put itself away.
fn anim(name: &str, stow: f32) -> Anim {
    let s = if name.starts_with("left") { -1.0 } else { 1.0 };
    let leg = if name.starts_with("nose") { 0 } else if s < 0.0 { 1 } else { 2 };
    if name.ends_with("aileron") {
        Anim::hinge(Drive::new(Input::Aileron(s), 0.35))
    } else if name.ends_with("flap") {
        Anim::hinge(Drive::new(Input::Flaps, 35f32.to_radians()))
    } else if name.ends_with("elevator") {
        Anim::hinge(Drive::new(Input::Elevator, 0.38))
    } else if name == "rudder" {
        Anim::hinge(Drive::new(Input::Rudder, 0.45))
    } else if name.ends_with("prop") || name.ends_with("spinner") {
        Anim::hinge(Drive::new(Input::Prop, 1.0))
    } else if name.ends_with(" leg") {
        Anim::hinge(Drive::new(Input::Gear, -stow).smooth(0.15, 0.85).offset(stow))
    } else if name.ends_with(" door") {
        // open before the legs come down, shut once they are up
        Anim::hinge(Drive::new(Input::Gear, -stow).smooth(0.0, 0.2).offset(stow))
    } else if name == "nose fork" {
        Anim::turn_slide(Drive::new(Input::Steer, 1.0), Drive::new(Input::Compress(0), 1.0), Vec3::Y)
    } else if name == "nose wheel" {
        Anim::hinge(Drive::new(Input::Wheel(0), -1.0))
    } else if name.ends_with("main wheel") {
        Anim::turn_slide(Drive::new(Input::Wheel(leg), -1.0), Drive::new(Input::Compress(leg), 1.0), Vec3::Y)
    } else {
        Anim::Fixed
    }
}

pub fn build() -> Model {
    let objs = load();
    let mut b = Builder::default();
    let mut at: HashMap<&str, usize> = HashMap::new();
    for &(name, group, role, parent) in PIECES {
        let o = objs.get(name).unwrap_or_else(|| panic!("ka350 model has no piece called {name:?}"));
        let k = b.add(name, group, o.mesh.clone(), anim(name, o.angle), o.pivot, o.axis, parent.map(|p| at[p]));
        b.role(k, role);
        if name == "glass" {
            b.pieces[k].glass = true;
        }
        if name.ends_with("prop") {
            b.pieces[k].vis = Vis::SlowProp;
        }
        if name.ends_with("spinner") {
            b.pieces[k].vis = Vis::FastProp;
        }
        at.insert(name, k);
    }
    // The blur of each propeller once it turns too fast to follow.
    for (name, group, sx) in [("left prop disc", G_LWING, -1.0), ("right prop disc", G_RWING, 1.0)] {
        let hub = PROP_HUB * Vec3::new(sx, 1.0, 1.0) + Vec3::Z * 0.25;
        let mut disc = Mesh::default();
        disc.disc(&mt(mat::PROPDISC, 0.1, 0.1, 0.1), hub, Vec3::Z, Vec3::Y, PROP_R, 64);
        let d = b.add(name, group, disc, Anim::hinge(Drive::new(Input::Prop, 1.0)), hub, Vec3::Z, None);
        b.role(d, Role::Prop);
        b.pieces[d].vis = Vis::FastProp;
        b.pieces[d].glass = true;
    }
    bake_ao(&mut b.pieces, &AoBake { vox: 0.05, reach: 2.5, rays: 20, open: &|_, _| false });
    Model::finish(b.pieces)
}

// ---------------------------------------------------------------- the type

const fn main_leg(x: f64, name: &'static str, kick: f64) -> Leg {
    Leg {
        at: DVec3::new(x, -1.714, 0.688),
        wheel_r: 0.26,
        travel: 0.25,
        k: 2.8e5,
        c: 2.6e4,
        nf_max: 3.5e5,
        steer: 0.0,
        brakes: true,
        side_k: 7.0e4,
        side_mu: 0.85,
        roll_k: 900.0,
        roll_mu: 0.02,
        brake_k: 2.2e4,
        brake_mu: 0.55,
        fold_vn: 5.5,
        fold_kick: DVec3::new(0.3, 0.0, kick),
        name,
    }
}

pub static SPEC: Spec = Spec {
    id: 3,
    name: "King Air 350",
    keys: &["ka350", "kingair", "king-air", "b350", "350"],
    blurb: "Twin turboprop, 6.3 t, T-tail. Quick and steady: 260 kt flat out, 105 kt down the approach.",
    still: include_bytes!("../../../assets/aircraft/ka350.png"),
    build,
    livery: 0,
    textures: &[
        (include_bytes!("../../../assets/ka350/tex0.jpg"), None),
        (include_bytes!("../../../assets/ka350/tex1.jpg"), None),
        (include_bytes!("../../../assets/ka350/tex2.jpg"), None),
        (include_bytes!("../../../assets/ka350/tex3.jpg"), Some(include_bytes!("../../../assets/ka350/tex3a.jpg"))),
        (include_bytes!("../../../assets/ka350/tex4.jpg"), None),
        (include_bytes!("../../../assets/ka350/tex5.jpg"), None),
        (include_bytes!("../../../assets/ka350/tex6.jpg"), None),
        (include_bytes!("../../../assets/ka350/tex7.jpg"), None),
        (include_bytes!("../../../assets/ka350/tex8.jpg"), None),
        (include_bytes!("../../../assets/ka350/tex9.jpg"), Some(include_bytes!("../../../assets/ka350/tex9a.jpg"))),
    ],
    groups: [("fuselage", 2_600.0), ("tail", 350.0), ("left wing", 1_100.0), ("right wing", 1_100.0), ("nose section", 450.0), ("radome", 40.0), ("gear", 300.0)],
    span: W_X1,
    wing_sect,
    fuse,
    aileron: [4.92, 8.0],
    tank: [1.0, 6.5],
    tail: Tail {
        fin_y0: 0.8,
        fin_h: 2.1,
        rudder_y0: 0.69,
        rudder_h: 1.95,
        stab_root: 0.15,
        elev_root: 0.11,
        break_z: [5.3, 6.6],
        fin_base: DVec3::new(0.0, 0.8, 6.8),
        fin_tip: DVec3::new(0.0, 2.95, 8.8),
        stab_tip: DVec3::new(2.75, 2.85, 8.9),
    },
    eye: EYE,
    sees_inside,
    contacts: &[
        ([0.0, -0.28, -4.68], Part::Nose),
        ([0.0, -0.65, -3.75], Part::Nose),
        ([0.0, 0.23, -3.75], Part::Nose),
        ([0.0, -0.78, -1.75], Part::Belly),
        ([0.0, -0.80, 0.0], Part::Belly),
        ([0.0, -0.75, 2.0], Part::Belly),
        ([0.0, -0.51, 4.25], Part::Belly),
        ([-0.80, 0.1, -1.5], Part::Side),
        ([0.80, 0.1, -1.5], Part::Side),
        ([-0.81, 0.1, 2.5], Part::Side),
        ([0.81, 0.1, 2.5], Part::Side),
        ([0.0, 1.0, -1.6], Part::Roof),
        ([0.0, 1.05, 2.5], Part::Roof),
        // the nacelles under the wings
        ([-2.57, -0.9, -1.5], Part::Pod(0)),
        ([-2.57, -0.9, 0.3], Part::Pod(0)),
        ([2.57, -0.9, -1.5], Part::Pod(1)),
        ([2.57, -0.9, 0.3], Part::Pod(1)),
    ],
    tail_pts: &[[0.0, 0.05, 6.25], [0.0, 0.31, 7.25], [0.0, 0.6, 8.1], [0.0, 0.9, 7.0]],
    wing_st: [1.2 / W_X1, 3.0 / W_X1, 4.6 / W_X1, 6.2 / W_X1, 7.6 / W_X1],
    hard: [
        DVec3::new(0.0, -0.28, -4.68),
        DVec3::new(-8.74, 0.3, 0.6),
        DVec3::new(8.74, 0.3, 0.6),
        DVec3::new(0.0, 0.6, 8.17),
        DVec3::new(0.0, 2.96, 9.0),
        DVec3::new(0.0, -0.80, 0.0),
        // (the propellers' tips, a little higher than they are: they clear the
        // ground by a hand's breadth, less than the nose strut gives under braking)
        DVec3::new(0.0, -1.42, -3.2),
    ],
    gear: &[
        Leg {
            at: DVec3::new(0.0, -1.756, -4.154),
            wheel_r: 0.29,
            travel: 0.25,
            k: 1.1e5,
            c: 1.3e4,
            nf_max: 1.6e5,
            steer: 1.0,
            brakes: false,
            side_k: 2.6e4,
            side_mu: 0.8,
            roll_k: 450.0,
            roll_mu: 0.02,
            brake_k: 0.0,
            brake_mu: 0.0,
            fold_vn: 5.0,
            fold_kick: DVec3::new(-0.3, 0.0, 0.0),
            name: "NOSE GEAR",
        },
        main_leg(-2.576, "LEFT MAIN GEAR", 0.3),
        main_leg(2.576, "RIGHT MAIN GEAR", -0.3),
    ],
    hull: Hull { k: 4.5e5, c: 6.0e4, nf_max: 9.0e5, drag: 2.0e4 },
    hole: Hole { fus: [0.81, 0.92, 0.12, 0.6], wing: [-0.6, W_DI, 0.16, 0.06], stab: [2.85, 0.07, 2.79], fin: [0.8, 2.1, 0.12, 0.05], pods: [[2.57, -0.35, 0.5], [0.0; 3]] },
    // A few aboard and most of the fuel (maximum take-off weight is 6.8 t).
    mass: 6_300.0,
    // Pitch, yaw, roll: estimates for a twin of this size with its engines on the wings.
    inertia: DVec3::new(43.0e3, 68.0e3, 30.0e3),
    wing_area: 28.8,
    b: 17.65,
    // Aspect ratio 10.8.
    cl0: 0.22,
    cl_a: 5.4,
    cd0: 0.026,
    k_ind: 0.037,
    cd_gear: 0.015,
    cd_flap: [0.015, 0.06],
    flap_cl: 0.6,
    stall_alpha: 0.27,
    stall_flap: 0.04,
    stall_slat: 0.0,
    alpha_trim: 0.24,
    mcrit: 0.0,
    cd_wave: 0.0,
    spoiler_cd: 0.0,
    spoiler_cl: 0.0,
    // Flaps: up, approach, down.
    flaps: &[0.0, 14.0, 35.0],
    vfe: &[1e9, 202.0 * KT, 158.0 * KT],
    flap_rate: 1.0 / 8.0,
    slats_at: 0,
    gear_rate: 1.0 / 6.0,
    // 263 kt indicated.
    vne: 263.0 * KT,
    mmo: 0.0,
    flutter_v: 150.0,
    g_limit: 3.3,
    g_break: 5.5,
    fbw_g: 0.0,
    steer_max: 0.6,
    steer_fade: 0.0,
    // Two PT6A-60As, 1050 shp each at 1700 rpm, turning 105 in propellers:
    // both together, as one engine and (fixed-pitch) propeller.
    engine: Engine::Piston(Piston {
        power: 1.566e6,
        w_rated: 1700.0 * std::f64::consts::TAU / 60.0,
        prop_d: 2.67,
        ct0: 0.44,
        cp0: 0.415,
        j_t: 1.95,
        j_p: 3.2,
        prop_i: 40.0,
        static_thrust: 22_000.0,
        fric0: 150.0,
        fric1: 400.0,
        blade_break_y: PROP_HUB.y + 0.6,
    }),
    handling: Handling {
        q_ref: 5000.0,
        a_max: 2.2,
        damp_v: 68.0,
        ad_min: 0.3,
        ad_max: 2.2,
        pitch_k: 7.0,
        pitch_k_lost: 2.0,
        pitch_ctl: 0.55,
        alpha_ctl: 0.26,
        pitch_d: 3.5,
        roll_ctl: 3.0,
        roll_d: 4.0,
        roll_beta: 0.6,
        roll_clean: 1.0,
        yaw_ctl: 1.1,
        yaw_beta: 4.5,
        yaw_d: 2.6,
        dihedral: 0.3,
        adverse: 0.25,
        stall_pitch: 0.8,
        stall_drop: 1.4,
        stall_yaw: 2.0,
    },
    cruise: 110.0,
    approach: (54.0, 2),
    cam_dist: 24.0,
    shadow_r: 10.5,
    sound: Sound::Piston,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_builds_whole() {
        let m = build();
        let (c, h) = m.bounds();
        for p in &m.pieces {
            assert!(p.mesh.v.iter().all(|v| v.pos.iter().chain(v.nrm.iter()).all(|x| x.is_finite())), "{} has NaNs", p.name);
            assert!(!p.mesh.i.is_empty(), "{} is empty", p.name);
        }
        for g in &m.groups {
            assert!(!g.i.is_empty(), "a breakable group is empty");
        }
        // 14.2 m long, 17.65 m across the winglets.
        let (len, span) = (h.z * 2.0, h.x * 2.0);
        println!("{} pieces, length {len:.2} m, span {span:.2} m, centre {c:?}", m.pieces.len());
        assert!((len - 14.2).abs() < 0.4, "{len}");
        assert!((span - 17.65).abs() < 0.4, "{span}");
    }

    #[test]
    fn flies_like_a_king_air() {
        let p = crate::plane::Plane::spawn(&SPEC, 0.6, -2.1, 1000.0, 0.0, 100.0);
        // Thrust: about 22 kN standing still, still some left at 260 kt.
        let (t0, t1) = (p.thrust(1.0, 0.0, 0.0), p.thrust(1.0, 134.0, 0.0));
        println!("static thrust {t0:.0} N, at 260 kt {t1:.0} N");
        assert!((t0 - 22_000.0).abs() < 3_000.0, "{t0}");
        // Drag at 260 kt at sea level, clean: the top speed is near there.
        let q = 0.5 * 1.225 * 134.0f64 * 134.0;
        let cl = SPEC.mass * 9.81 / (q * SPEC.wing_area);
        let drag = q * SPEC.wing_area * (SPEC.cd0 + SPEC.k_ind * cl * cl);
        println!("drag at 260 kt {drag:.0} N");
        assert!(t1 > drag * 0.7 && t1 < drag * 1.6, "{t1} against {drag}");
    }
}
