//! Cars you can drive, and what happens when you crash them.
//!
//! The models come from downloaded glTF files through
//! `tools/vehicle_import.py`: a body (split here into its opaque skin and its
//! glass) and four wheels, each centred on its hub.
//!
//! The car is one rigid body hung on four springs. Each wheel casts down from
//! its strut top to the ground; the spring and damper push the body up along
//! the ground's normal, and the tyre pushes along the ground, within a friction
//! circle: drive and brakes along the wheel, a side force that rises with the
//! slip angle and then saturates (so you can slide it, and the handbrake lets
//! the rear go). Around the body is a ring of hull points that collide with
//! the ground (rollovers, scrapes) and with buildings, and the other cars.
//!
//! Crashes are soft-body in the way that matters to the eye: every hard hit
//! dents the mesh around where it struck, as deep as the speed lost, with a
//! little noise so the panels crinkle instead of moving like rubber; glass near
//! a hard hit bursts out, a wheel hit hard enough comes off, and a fast car can
//! bring down a small building.
//!
//! Body frame (as the aircraft): +X right, +Y up, -Z forward; the model's
//! origin is on the ground under the middle of the car. `Car::pos` is the
//! centre of mass, `Kind::cg` above that origin.
use crate::airframe::{mat, pat};
use crate::crash::{Rng, DUST, GLASS, SPARK};
use crate::geo;
use crate::render::{DrawObj, PartVertex, Particle, Renderer};
use crate::world::World;
use glam::{DMat3, DQuat, DVec2, DVec3, Mat4, Vec3};
use std::io::Read;

/// The shader's id for a car (DrawObj glow.y): no aircraft's paint or panel lines.
const SHADER_KIND: f32 = 20.0;

pub struct Kind {
    pub key: &'static str,
    pub name: &'static str,
    model: &'static [u8],
    /// kg, W, and the top speed the drag is set for (m/s)
    mass: f64,
    power: f64,
    vmax: f64,
    /// centre of mass above the ground (m)
    cg: f64,
    /// suspension: natural frequency (Hz), damping ratio, travel up and down from rest (m)
    freq: f64,
    zeta: f64,
    travel: f64,
    /// tyre friction, steering lock (rad), downforce per (m/s)² (N)
    mu: f64,
    steer: f64,
    down: f64,
    /// how far the front, rear and sides can crush (m), and the deceleration
    /// the crumpling front holds while it does (g)
    crush: [f64; 3],
    crush_g: f64,
    /// where the driver's eyes are (body frame, from the model origin)
    pub eye: Vec3,
    /// how far back the chase camera sits
    pub cam: f64,
}

pub static KINDS: [Kind; 2] = [
    Kind {
        key: "raptor",
        name: "Ford F-150 Raptor R",
        model: include_bytes!("../assets/vehicles/raptor.bin.gz"),
        mass: 2700.0,
        power: 522_000.0,
        vmax: 58.0,
        cg: 0.75,
        freq: 1.25,
        zeta: 0.32,
        travel: 0.3,
        mu: 1.0,
        steer: 0.6,
        down: 0.0,
        crush: [0.9, 0.6, 0.35],
        crush_g: 32.0,
        eye: Vec3::new(-0.42, 1.78, -0.15),
        cam: 9.0,
    },
    Kind {
        key: "countach",
        name: "Lamborghini Countach LPI 800-4",
        model: include_bytes!("../assets/vehicles/countach.bin.gz"),
        mass: 1595.0,
        power: 600_000.0,
        vmax: 98.0,
        cg: 0.45,
        freq: 2.1,
        zeta: 0.4,
        travel: 0.11,
        mu: 1.3,
        steer: 0.55,
        down: 0.25,
        crush: [0.75, 0.6, 0.3],
        crush_g: 38.0,
        eye: Vec3::new(-0.38, 0.98, 0.15),
        cam: 7.5,
    },
];

/// What the driver asks for: throttle (+) or brake / reverse (-), steering (+ right), handbrake.
#[derive(Default, Clone, Copy)]
pub struct Drive {
    pub throttle: f64,
    pub steer: f64,
    pub handbrake: bool,
}

/// Something worth a sound, a spray of particles or a fallen building.
pub struct Hit {
    pub pos: DVec3,
    pub normal: DVec3,
    /// speed lost into the obstacle (m/s)
    pub speed: f64,
    pub kind: HitKind,
}
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum HitKind {
    Ground,
    Scrape,
    Building(geo::TileId, usize, f32),
    Car,
    Glass,
    Wheel,
    /// a test wall (`Garage::walls`)
    Wall,
    /// a part tore off (`Car::part_states`)
    Part,
}

/// A solid box that cars hit: the crash test's wall. Its own frame: x across, y up, z through it.
pub struct Wall {
    pub center: DVec3,
    pub rot: DQuat,
    pub half: DVec3,
    mesh: Option<usize>,
}
impl Wall {
    /// A wall at `center` with its length along `across` and its height along `up`
    /// (the third axis, through it, follows: built this way it is always a true rotation).
    pub fn new(center: DVec3, across: DVec3, up: DVec3, half: DVec3) -> Wall {
        let x = across.normalize();
        let z = x.cross(up).normalize();
        let y = z.cross(x);
        Wall { center, rot: DQuat::from_mat3(&DMat3::from_cols(x, y, z)), half, mesh: None }
    }
    /// Inside? Then the way out through the nearest face, and how deep.
    fn inside(&self, p: DVec3) -> Option<(DVec3, f64)> {
        let q = self.rot.inverse() * (p - self.center);
        let d = self.half - q.abs();
        if d.min_element() <= 0.0 {
            return None;
        }
        let (axis, depth) = if d.x <= d.y && d.x <= d.z { (DVec3::X * q.x.signum(), d.x) } else if d.y <= d.z { (DVec3::Y * q.y.signum(), d.y) } else { (DVec3::Z * q.z.signum(), d.z) };
        Some((self.rot * axis, depth))
    }
}

/// What the physics did lately, for the F4 overlay and the crash test's report.
#[derive(Default)]
pub struct Debug {
    /// hull points touching something this frame: where, the surface's normal
    pub contacts: Vec<(DVec3, DVec3)>,
    /// where each wheel meets the ground
    pub wheel_at: [Option<DVec3>; 4],
    /// deceleration now and the most since the last reset (g, over 1/30 s)
    pub g: f64,
    pub g_peak: f64,
    /// the last hits: when (car time), what, speed lost
    pub hits: Vec<(f64, HitKind, f64)>,
    pub glass_broken: usize,
    vel_hist: [DVec3; 4],
}

// ---------------------------------------------------------------- the model
struct Part {
    v: Vec<PartVertex>,
    i: Vec<u32>,
    /// which part of the car each vertex belongs to (0 the body itself)
    pid: Vec<u8>,
    hub: Vec3,
    r: f32,
}
impl Part {
    fn empty() -> Part {
        Part { v: vec![PartVertex::default(); 3], i: vec![0, 1, 2], pid: vec![0; 3], hub: Vec3::ZERO, r: 0.0 }
    }
    /// The triangles whose first vertex passes `keep`, as a mesh of their own.
    fn subset(&self, keep: impl Fn(&PartVertex, u8) -> bool) -> Part {
        let mut map = vec![u32::MAX; self.v.len()];
        let mut p = Part { v: Vec::new(), i: Vec::new(), pid: Vec::new(), hub: self.hub, r: self.r };
        for t in self.i.chunks(3) {
            let a = t[0] as usize;
            if !keep(&self.v[a], self.pid[a]) {
                continue;
            }
            for &k in t {
                let k = k as usize;
                if map[k] == u32::MAX {
                    map[k] = p.v.len() as u32;
                    p.v.push(self.v[k]);
                    p.pid.push(self.pid[k]);
                }
                p.i.push(map[k]);
            }
        }
        if p.i.is_empty() { Part::empty() } else { p }
    }
}

/// A part of the car that swings open or comes off, as the model's file describes it.
#[derive(Clone)]
pub struct PartDef {
    /// 1 door, 2 hood, 3 tailgate, 4 rear lid, 5 bumper, 6 mirror
    pub kind: u8,
    pub name: String,
    /// the hinge: a point on it and its axis (model frame), which way round
    /// the part opens (+1, -1, 0 for bolted on) and how far (rad)
    pivot: Vec3,
    axis: Vec3,
    sign: f32,
    most: f32,
    lo: Vec3,
    hi: Vec3,
}

struct Shape {
    body: Part,
    glass: Part,
    /// each part: what it is, its skin, its glass
    parts: Vec<(PartDef, Part, Part)>,
    wheels: [Part; 4],
    lo: Vec3,
    hi: Vec3,
}

fn load(bytes: &[u8]) -> Shape {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(bytes).read_to_end(&mut raw).expect("car model");
    let mut at = 0usize;
    let mut take = |n: usize| {
        let s = raw[at..at + n].to_vec();
        at += n;
        s
    };
    let u32_ = |b: &[u8]| u32::from_le_bytes(b[..4].try_into().unwrap());
    let f32_ = |b: &[u8]| f32::from_le_bytes(b[..4].try_into().unwrap());
    assert_eq!(take(4), b"VEH1", "car model: not a model file");
    let n = u32_(&take(4)) as usize;
    let mut parts = Vec::new();
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for _ in 0..n {
        let h = take(16);
        let hub = Vec3::new(f32_(&h[0..]), f32_(&h[4..]), f32_(&h[8..]));
        let r = f32_(&h[12..]);
        let nv = u32_(&take(4)) as usize;
        let vb = take(nv * 20);
        let srgb = |c: u8| (c as f32 / 255.0).powf(2.2);
        let v: Vec<PartVertex> = vb
            .chunks(20)
            .map(|c| {
                let pos = [f32_(&c[0..]), f32_(&c[4..]), f32_(&c[8..])];
                let nrm = [c[12] as i8 as f32 / 127.0, c[13] as i8 as f32 / 127.0, c[14] as i8 as f32 / 127.0];
                let id = c[15] as f32;
                let pattern = if id == mat::RUBBER { pat::TREAD } else { 0.0 };
                PartVertex { pos, nrm, col: [srgb(c[16]), srgb(c[17]), srgb(c[18]), 1.0], uv: [0.0; 2], mat: [id, pattern, 0.0, 0.0] }
            })
            .collect();
        let pid: Vec<u8> = vb.chunks(20).map(|c| c[19]).collect();
        let ni = u32_(&take(4)) as usize;
        let i: Vec<u32> = take(ni * 4).chunks(4).map(|c| u32_(c)).collect();
        for p in &v {
            let p = Vec3::from(p.pos) + hub;
            lo = lo.min(p);
            hi = hi.max(p);
        }
        parts.push(Part { v, i, pid, hub, r });
    }
    // The parts table (models converted before parts have none).
    let mut defs = Vec::new();
    if raw.len() >= at + 8 && &raw[at..at + 4] == b"PRT1" {
        let n = u32_(&raw[at + 4..]) as usize;
        at += 8;
        for _ in 0..n {
            let (kind, nl) = (raw[at], raw[at + 1] as usize);
            let name = String::from_utf8_lossy(&raw[at + 2..at + 2 + nl]).into_owned();
            at += 2 + nl;
            let f: Vec<f32> = raw[at..at + 56].chunks(4).map(f32_).collect();
            at += 56;
            let v3 = |i: usize| Vec3::new(f[i], f[i + 1], f[i + 2]);
            defs.push(PartDef { kind, name, pivot: v3(0), axis: v3(3), sign: f[6], most: f[7], lo: v3(8), hi: v3(11) });
        }
    }
    let mut it = parts.into_iter();
    let whole = it.next().unwrap();
    let wheels: [Part; 4] = [it.next().unwrap(), it.next().unwrap(), it.next().unwrap(), it.next().unwrap()];
    // The glass goes in its own meshes (it is drawn in the transparent pass), and each part in its own.
    let glass = |v: &PartVertex| v.mat[0] == mat::GLASS;
    let parts = defs.into_iter().enumerate().map(|(k, d)| {
        let id = k as u8 + 1;
        (d, whole.subset(|v, p| p == id && !glass(v)), whole.subset(|v, p| p == id && glass(v)))
    }).collect();
    Shape { body: whole.subset(|v, p| p == 0 && !glass(v)), glass: whole.subset(|v, p| p == 0 && glass(v)), parts, wheels, lo, hi }
}

/// What a part is doing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PartState {
    Latched,
    /// unlatched, hanging from its hinge
    Swinging,
    Gone,
}

/// A part of a car: its skins (dented with the body), its hinge and how it is holding up.
struct CarPart {
    def: PartDef,
    skin: Skin,
    glass: Skin,
    state: PartState,
    /// how far open (rad, 0 shut) and how fast it is swinging
    angle: f64,
    rate: f64,
    /// what is left of the hinge (1 new); hard slams against its stop wear it
    hinge: f64,
    centre: Vec3,
    mass: f64,
}
impl CarPart {
    /// Its pose on the car: rotation about the hinge (model frame).
    fn swing(&self) -> Mat4 {
        let d = &self.def;
        if self.angle == 0.0 || d.sign == 0.0 {
            return Mat4::IDENTITY;
        }
        Mat4::from_translation(d.pivot) * Mat4::from_axis_angle(d.axis.normalize(), d.sign * self.angle as f32) * Mat4::from_translation(-d.pivot)
    }
    fn near(&self, p: Vec3, by: f32) -> bool {
        p.cmpge(self.def.lo - Vec3::splat(by)).all() && p.cmple(self.def.hi + Vec3::splat(by)).all()
    }
}

/// A piece that has come off a car: a part or a wheel, tumbling on its own.
pub struct Debris {
    skins: Vec<(Skin, bool)>,
    wheel: Option<usize>,
    /// the point of its mesh that is its centre of mass
    offset: DVec3,
    /// its corners (or a wheel's rim), from the centre of mass
    pts: Vec<DVec3>,
    pub pos: DVec3,
    pub rot: DQuat,
    pub vel: DVec3,
    w: DVec3,
    mass: f64,
    inertia: DVec3,
    still: f64,
    /// which car and part it came from (a wheel: part usize::MAX), to take back on a reset
    owner: Option<(usize, usize)>,
}

/// An impulse at `at` on a rigid body that stops it moving into a surface with
/// normal `n` (restitution `e`, friction `mu`, at most `cap`); returns the speed it was moving into it.
#[allow(clippy::too_many_arguments)]
fn rb_impulse(pos: DVec3, vel: &mut DVec3, w: &mut DVec3, rot: DQuat, inertia: DVec3, m: f64, at: DVec3, n: DVec3, e: f64, mu: f64, cap: f64) -> f64 {
    let r = at - pos;
    let v = *vel + w.cross(r);
    let vn = v.dot(n);
    if vn >= 0.0 {
        return 0.0;
    }
    let rm = DMat3::from_quat(rot);
    let ii = rm * DMat3::from_diagonal(1.0 / inertia) * rm.transpose();
    let kn = 1.0 / m + n.dot((ii * r.cross(n)).cross(r));
    let j = (-(1.0 + e) * vn / kn).min(cap);
    *vel += n * (j / m);
    *w += ii * r.cross(n * j);
    let vt = v - n * vn;
    let st = vt.length();
    if st > 1e-4 {
        let t = vt / st;
        let kt = 1.0 / m + t.dot((ii * r.cross(t)).cross(r));
        let jt = (st / kt).min(mu * j);
        *vel -= t * (jt / m);
        *w -= ii * r.cross(t * jt);
    }
    -vn
}

/// A car's own copy of a mesh, dented as it crashes.
struct Skin {
    v: Vec<PartVertex>,
    /// as it came from the model, for a reset
    orig_i: Vec<u32>,
    rest: Vec<Vec3>,
    i: Vec<u32>,
    /// on the GPU once first drawn
    mesh: Option<usize>,
    /// the vertices moved since the last upload (first, end), and whether the indices changed
    dirty: Option<(usize, usize)>,
    idx_dirty: bool,
}
impl Skin {
    fn empty() -> Skin {
        Skin::new(&Part::empty())
    }
    fn new(p: &Part) -> Skin {
        Skin { v: p.v.clone(), orig_i: p.i.clone(), rest: p.v.iter().map(|v| Vec3::from(v.pos)).collect(), i: p.i.clone(), mesh: None, dirty: None, idx_dirty: false }
    }
    fn upload(&mut self, r: &mut Renderer) -> usize {
        match self.mesh {
            None => {
                let m = r.add_dynamic_mesh(&self.v, &self.i);
                self.mesh = Some(m);
                self.dirty = None;
                self.idx_dirty = false;
                m
            }
            Some(m) => {
                if let Some((a, b)) = self.dirty.take() {
                    // Only the stretch of vertices that moved.
                    r.write_mesh_range(m, a, &self.v[a..b], if self.idx_dirty { Some(&self.i) } else { None });
                    self.idx_dirty = false;
                }
                m
            }
        }
    }
}

// ---------------------------------------------------------------- the car
pub struct Car {
    pub kind: &'static Kind,
    pub pos: DVec3,
    pub vel: DVec3,
    pub rot: DQuat,
    /// angular velocity (planet frame)
    pub w: DVec3,
    steer: f64,
    /// per wheel: suspension length now, spin angle, spin rate, on the ground, come off
    len: [f64; 4],
    spin: [f64; 4],
    spin_w: [f64; 4],
    pub grounded: [bool; 4],
    lost: [bool; 4],
    /// the strut tops (body frame, from the centre of mass), wheel radius, rest length
    strut: [DVec3; 4],
    radius: [f64; 4],
    rest: f64,
    k: f64,
    c: f64,
    inertia: DVec3,
    /// the hull points now, as built, which way each crushes into the body,
    /// how far it has, how far it can, and crush not yet shown in the mesh
    hull: Vec<DVec3>,
    hull0: Vec<DVec3>,
    inward: Vec<DVec3>,
    crush: Vec<f64>,
    crush_max: Vec<f64>,
    crush_shown: Vec<f64>,
    lo: Vec3,
    hi: Vec3,
    body: Skin,
    glass: Skin,
    parts: Vec<CarPart>,
    /// parts and wheels just come off, for the garage to turn into debris
    shed: Vec<usize>,
    shed_wheels: Vec<usize>,
    prev_vel: DVec3,
    h: f64,
    last_flush: f64,
    /// accumulated damage 0..1 (engine power falls off with it)
    pub damage: f64,
    /// buildings near the car, in a flat frame at `near_o`: outline, base, top, tile and index, area
    near: Vec<(Vec<DVec2>, f64, f64, (geo::TileId, usize), f32)>,
    near_o: DVec3,
    near_t: f64,
    time: f64,
    pub scrape: f32,
    dent_cool: f64,
    /// how long it has been lying on its side or roof, nearly still
    stuck: f64,
    pub debug: Debug,
}

/// Every car in the world, and the models they share.
#[derive(Default)]
pub struct Garage {
    shapes: Vec<Option<Shape>>,
    wheel_mesh: Vec<Option<[usize; 4]>>,
    pub cars: Vec<Car>,
    pub walls: Vec<Wall>,
    pub debris: Vec<Debris>,
}

impl Garage {
    fn shape(&mut self, k: usize) -> &Shape {
        if self.shapes.len() < KINDS.len() {
            self.shapes.resize_with(KINDS.len(), || None);
            self.wheel_mesh.resize(KINDS.len(), None);
        }
        if self.shapes[k].is_none() {
            self.shapes[k] = Some(load(KINDS[k].model));
        }
        self.shapes[k].as_ref().unwrap()
    }

    /// Put a new car of kind `k` on the ground at `at`, its nose to `heading` (radians from north).
    pub fn spawn(&mut self, k: usize, at: DVec3, heading: f64, world: &World) -> usize {
        let kind = &KINDS[k];
        let s = self.shape(k);
        let at = clear_spot(world, at, heading, (s.hi - s.lo).as_dvec3());
        let (e, n, u) = geo::enu(at);
        let fwd = n * heading.cos() + e * heading.sin();
        let right = fwd.cross(u);
        let rot = DQuat::from_mat3(&DMat3::from_cols(right, u, -fwd));
        let (lo, hi) = (s.lo, s.hi);
        let size = (hi - lo).as_dvec3();
        let m = kind.mass;
        let inertia = DVec3::new(m / 12.0 * (size.y * size.y + size.z * size.z), m / 12.0 * (size.x * size.x + size.z * size.z), m / 12.0 * (size.x * size.x + size.y * size.y));
        // Springs that carry a quarter of the weight each at the chosen frequency.
        let om = std::f64::consts::TAU * kind.freq;
        let k_s = m / 4.0 * om * om;
        let c_s = 2.0 * kind.zeta * (k_s * m / 4.0).sqrt();
        let sag = 9.81 / (om * om);
        let cg = DVec3::new(0.0, kind.cg, 0.0);
        let strut = [0, 1, 2, 3].map(|w| s.wheels[w].hub.as_dvec3() - cg + DVec3::Y * kind.travel);
        let radius = [0, 1, 2, 3].map(|w| s.wheels[w].r as f64);
        // The hull: the corners and edges of the body, low and high.
        // Each point crushes into the body: the front and back ones along the car,
        // the side ones across it, the roof ones down.
        let mut hull = Vec::new();
        let (mut inward, mut crush_max) = (Vec::new(), Vec::new());
        let (x0, x1, z0, z1) = (lo.x as f64 * 0.94, hi.x as f64 * 0.94, lo.z as f64 * 0.97, hi.z as f64 * 0.97);
        let ys = [lo.y as f64 + (radius[0] * 0.9).max(0.22), hi.y as f64 * 0.62, hi.y as f64 * 0.97];
        for (j, y) in ys.iter().enumerate() {
            let (sx, sz) = if j == 2 { (0.7, 0.45) } else { (1.0, 1.0) };
            for (x, z) in [(x0, z0), (x1, z0), (x0, z1), (x1, z1), (0.0, z0), (0.0, z1), (x0, 0.0), (x1, 0.0)] {
                hull.push(DVec3::new(x * sx, *y, z * sz) - cg);
                let (dir, most) = if j == 2 {
                    (DVec3::NEG_Y, 0.3)
                } else if x != 0.0 && z != 0.0 {
                    // A corner gives way to a hit from the front (or back) or the side.
                    let along = if z < 0.0 { DVec3::Z } else { DVec3::NEG_Z };
                    ((along + DVec3::X * -x.signum() * 0.8).normalize(), kind.crush[if z < 0.0 { 0 } else { 1 }])
                } else if z < 0.0 {
                    (DVec3::Z, kind.crush[0])
                } else if z > 0.0 {
                    (DVec3::NEG_Z, kind.crush[1])
                } else {
                    (DVec3::X * -x.signum(), kind.crush[2])
                };
                inward.push(dir);
                crush_max.push(most);
            }
        }
        let nh = hull.len();
        let parts = s
            .parts
            .iter()
            .map(|(d, skin, glass)| {
                let mass = match d.kind { 1 => 32.0, 2 => 20.0, 3 => 28.0, 4 => 18.0, 5 => 14.0, _ => 3.0 };
                CarPart { def: d.clone(), skin: Skin::new(skin), glass: Skin::new(glass), state: PartState::Latched, angle: 0.0, rate: 0.0, hinge: 1.0, centre: (d.lo + d.hi) * 0.5, mass }
            })
            .collect();
        let g = ground(world, at).unwrap_or(geo::pos_to_lla(at).2);
        let pos = geo::lla_to_pos(geo::pos_to_lla(at).0, geo::pos_to_lla(at).1, g) + u * (kind.cg + 0.05);
        let (body, glass) = (Skin::new(&s.body), Skin::new(&s.glass));
        let rest = kind.travel + sag;
        self.cars.push(Car {
            kind,
            pos,
            vel: DVec3::ZERO,
            rot,
            w: DVec3::ZERO,
            steer: 0.0,
            len: [rest; 4],
            spin: [0.0; 4],
            spin_w: [0.0; 4],
            grounded: [false; 4],
            lost: [false; 4],
            strut,
            radius,
            rest,
            k: k_s,
            c: c_s,
            inertia,
            hull0: hull.clone(),
            hull,
            inward,
            crush: vec![0.0; nh],
            crush_max,
            crush_shown: vec![0.0; nh],
            lo,
            hi,
            body,
            glass,
            parts,
            shed: Vec::new(),
            shed_wheels: Vec::new(),
            prev_vel: DVec3::ZERO,
            h: 1.0 / 120.0,
            last_flush: 0.0,
            damage: 0.0,
            near: Vec::new(),
            near_o: DVec3::ZERO,
            near_t: -1.0,
            time: 0.0,
            scrape: 0.0,
            dent_cool: 0.0,
            stuck: 0.0,
            debug: Debug::default(),
        });
        self.cars.len() - 1
    }

    /// The start of a frame: forget last frame's contacts.
    pub fn begin_frame(&mut self) {
        for c in &mut self.cars {
            c.debug.contacts.clear();
        }
    }

    /// Car `i` as new again, at `at` facing `heading` (its GPU meshes are reused).
    pub fn reset(&mut self, i: usize, at: DVec3, heading: f64, world: &World) {
        let k = self.cars[i].kind_index();
        let (bm, gm) = (self.cars[i].body.mesh, self.cars[i].glass.mesh);
        let mut pm: Vec<(Option<usize>, Option<usize>)> = self.cars[i].parts.iter().map(|p| (p.skin.mesh, p.glass.mesh)).collect();
        // Its parts lying about come back (and its wheels are new).
        self.debris.retain(|d| match d.owner {
            Some((ci, pi)) if ci == i => {
                if pi < pm.len() && d.skins.len() == 2 {
                    pm[pi] = (d.skins[0].0.mesh, d.skins[1].0.mesh);
                }
                false
            }
            _ => true,
        });
        self.spawn(k, at, heading, world);
        let mut c = self.cars.pop().unwrap();
        let mut skins: Vec<(&mut Skin, Option<usize>)> = vec![(&mut c.body, bm), (&mut c.glass, gm)];
        for (p, m) in c.parts.iter_mut().zip(&pm) {
            skins.push((&mut p.skin, m.0));
            skins.push((&mut p.glass, m.1));
        }
        for (s, m) in skins {
            s.mesh = m;
            s.i = s.orig_i.clone();
            s.dirty = Some((0, s.v.len()));
            s.idx_dirty = true;
        }
        self.cars[i] = c;
    }

    /// One physics step for every car (`drive` is for car `driven`, the rest coast with the handbrake on).
    pub fn step(&mut self, h: f64, driven: Option<usize>, drive: Drive, world: &World, hits: &mut Vec<Hit>) {
        let n = self.cars.len();
        for i in 0..n {
            let d = if Some(i) == driven { drive } else { Drive { throttle: 0.0, steer: 0.0, handbrake: true } };
            self.cars[i].step(h, d, world, &self.walls, hits);
            self.shed_debris(i);
        }
        for d in &mut self.debris {
            d.step(h, world, &self.walls);
        }
        // Car against car: each a row of three spheres along its length.
        for a in 0..n {
            for b in a + 1..n {
                let (l, r) = self.cars.split_at_mut(b);
                collide(&mut l[a], &mut r[0], hits);
            }
        }
    }

    /// Turn what has just come off car `i` into debris.
    fn shed_debris(&mut self, i: usize) {
        let c = &mut self.cars[i];
        let origin = c.origin();
        for pi in std::mem::take(&mut c.shed) {
            let p = &mut c.parts[pi];
            let swing = p.swing();
            let centre = swing.transform_point3(p.centre).as_dvec3();
            let rot = c.rot * DQuat::from_mat4(&glam::DMat4::from_cols_array(&swing.to_cols_array().map(|x| x as f64)));
            let pos = origin + c.rot * centre;
            let half = ((p.def.hi - p.def.lo) * 0.5).as_dvec3();
            let pts = [-1.0, 1.0].iter().flat_map(|&x| [-1.0, 1.0].iter().flat_map(move |&y| [-1.0, 1.0].map(move |z| DVec3::new(x, y, z) * half))).collect();
            let m = p.mass;
            let inertia = DVec3::new(half.y * half.y + half.z * half.z, half.x * half.x + half.z * half.z, half.x * half.x + half.y * half.y) * (m / 3.0);
            let r = pos - c.pos;
            let kick = (pos - c.pos).normalize_or_zero() * 2.0 + pos.normalize() * 1.5;
            let skins = vec![(std::mem::replace(&mut p.skin, Skin::empty()), false), (std::mem::replace(&mut p.glass, Skin::empty()), true)];
            self.debris.push(Debris { skins, wheel: None, offset: p.centre.as_dvec3(), pts, pos, rot, vel: c.vel + c.w.cross(r) + kick, w: c.w + DVec3::new(1.5, -2.0, 1.0), mass: m, inertia, still: 0.0, owner: Some((i, pi)) });
        }
        let wm = self.wheel_mesh.get(c.kind_index()).copied().flatten();
        for w in std::mem::take(&mut c.shed_wheels) {
            let hub = c.strut[w] + DVec3::new(0.0, c.kind.cg, 0.0) - DVec3::Y * c.len[w];
            let steer = if w < 2 { c.steer } else { 0.0 };
            let rot = c.rot * DQuat::from_rotation_y(-steer);
            let pos = origin + c.rot * hub;
            let r = c.radius[w];
            let pts = (0..24).map(|k| {
                let a = (k % 12) as f64 / 12.0 * std::f64::consts::TAU;
                DVec3::new(if k < 12 { -0.14 } else { 0.14 }, a.sin() * r, a.cos() * r)
            }).collect();
            let m = 35.0;
            let inertia = DVec3::new(0.5 * m * r * r, 0.25 * m * r * r, 0.25 * m * r * r);
            let spin = rot * DVec3::X * -c.spin_w[w];
            self.debris.push(Debris { skins: Vec::new(), wheel: wm.map(|m| m[w]), offset: DVec3::ZERO, pts, pos, rot, vel: c.vel + c.w.cross(pos - c.pos) + c.right() * if w % 2 == 0 { -2.0 } else { 2.0 }, w: c.w + spin, mass: m, inertia, still: 0.0, owner: Some((i, usize::MAX)) });
        }
        // Not forever: the oldest go.
        while self.debris.len() > 80 {
            self.debris.remove(0);
        }
    }

    /// Upload the dents and draw every car.
    pub fn draw(&mut self, cam: DVec3, r: &mut Renderer, objs: &mut Vec<DrawObj>) {
        for c in &self.cars {
            let k = c.kind_index();
            if self.wheel_mesh[k].is_none() {
                let s = self.shapes[k].as_ref().unwrap();
                self.wheel_mesh[k] = Some([0, 1, 2, 3].map(|w| r.add_mesh(&s.wheels[w].v, &s.wheels[w].i)));
            }
        }
        for w in &mut self.walls {
            let m = *w.mesh.get_or_insert_with(|| {
                let (v, i) = box_mesh(w.half.as_vec3(), [0.42, 0.41, 0.39]);
                r.add_mesh(&v, &i)
            });
            let model = Mat4::from_rotation_translation(w.rot.as_quat(), (w.center - cam).as_vec3());
            objs.push(DrawObj { mesh: m, model, glow: [0.0, SHADER_KIND, 0.0, 0.0], cast: 0b11, ..DrawObj::DEFAULT });
        }
        for (di, d) in self.debris.iter_mut().enumerate() {
            let model = Mat4::from_rotation_translation(d.rot.as_quat(), (d.pos - cam).as_vec3()) * Mat4::from_translation(-d.offset.as_vec3());
            let o = DrawObj { model, glow: [0.0, SHADER_KIND, 0.0, di as f32 * 0.29], cast: 0b11, ..DrawObj::DEFAULT };
            if let Some(w) = d.wheel {
                objs.push(DrawObj { mesh: w, ..o });
            }
            for (s, glass) in &mut d.skins {
                let m = s.upload(r);
                objs.push(DrawObj { mesh: m, glass: *glass, ..o });
            }
        }
        for (ci, c) in self.cars.iter_mut().enumerate() {
            let wm = self.wheel_mesh[c.kind_index()].unwrap();
            let (body, glass) = (c.body.upload(r), c.glass.upload(r));
            let origin = c.pos - c.rot * DVec3::new(0.0, c.kind.cg, 0.0);
            let base = Mat4::from_rotation_translation(c.rot.as_quat(), (origin - cam).as_vec3());
            let o = DrawObj { glow: [0.0, SHADER_KIND, 0.0, ci as f32 * 0.37], cast: 0b11, ..DrawObj::DEFAULT };
            objs.push(DrawObj { mesh: body, model: base, ..o });
            objs.push(DrawObj { mesh: glass, model: base, glass: true, ..o });
            for p in &mut c.parts {
                if p.state == PartState::Gone {
                    continue;
                }
                let m = base * p.swing();
                let (sk, gl) = (p.skin.upload(r), p.glass.upload(r));
                objs.push(DrawObj { mesh: sk, model: m, ..o });
                objs.push(DrawObj { mesh: gl, model: m, glass: true, ..o });
            }
            for w in 0..4 {
                if c.lost[w] {
                    continue;
                }
                let hub = c.strut[w] + DVec3::new(0.0, c.kind.cg, 0.0) - DVec3::Y * c.len[w];
                let steer = if w < 2 { c.steer } else { 0.0 };
                let m = Mat4::from_translation(hub.as_vec3()) * Mat4::from_rotation_y(-steer as f32) * Mat4::from_rotation_x(-c.spin[w] as f32);
                objs.push(DrawObj { mesh: wm[w], model: base * m, ..o });
            }
        }
    }
}

impl Car {
    /// The hull points (planet frame).
    pub fn hull_points(&self) -> Vec<DVec3> {
        self.hull.iter().map(|h| self.pos + self.rot * *h).collect()
    }
    /// Set it moving at `v` straight away (a scripted start), without the g meter
    /// or the hinges taking the jump for a crash.
    pub fn launch(&mut self, v: DVec3) {
        self.vel = v;
        self.prev_vel = v;
        self.debug.vel_hist = [v; 4];
        self.debug.g_peak = 0.0;
    }
    /// Per wheel: suspension length, its rest length, on the ground, come off.
    pub fn wheel(&self, w: usize) -> (f64, f64, bool, bool) {
        (self.len[w], self.rest, self.grounded[w], self.lost[w])
    }
    pub fn wheels_lost(&self) -> usize {
        self.lost.iter().filter(|l| **l).count()
    }
    /// How far the most crumpled vertex of the body has moved (m).
    pub fn deepest_dent(&self) -> f32 {
        self.body.v.iter().zip(&self.body.rest).map(|(v, r)| (Vec3::from(v.pos) - *r).length()).fold(0.0, f32::max)
    }
    pub fn kind_index(&self) -> usize {
        KINDS.iter().position(|k| std::ptr::eq(k, self.kind)).unwrap_or(0)
    }
    pub fn forward(&self) -> DVec3 {
        self.rot * DVec3::NEG_Z
    }
    pub fn up(&self) -> DVec3 {
        self.rot * DVec3::Y
    }
    pub fn right(&self) -> DVec3 {
        self.rot * DVec3::X
    }
    /// The model origin (on the ground under the middle).
    pub fn origin(&self) -> DVec3 {
        self.pos - self.rot * DVec3::new(0.0, self.kind.cg, 0.0)
    }
    /// Where someone stands to get in at the driver's door (planet frame, on the ground).
    pub fn door(&self) -> DVec3 {
        self.origin() + self.rot * DVec3::new(self.lo.x as f64 - 0.55, 0.0, self.kind.eye.z as f64)
    }
    pub fn eye(&self) -> DVec3 {
        self.origin() + self.rot * self.kind.eye.as_dvec3()
    }
    pub fn length(&self) -> f64 {
        (self.hi.z - self.lo.z) as f64
    }
    pub fn speed(&self) -> f64 {
        self.vel.length()
    }

    fn inv_inertia_world(&self) -> DMat3 {
        let r = DMat3::from_quat(self.rot);
        r * DMat3::from_diagonal(1.0 / self.inertia) * r.transpose()
    }

    /// An impulse at `at` that stops the car moving into a surface with normal `n`;
    /// returns the speed it was moving into it.
    fn impulse(&mut self, at: DVec3, n: DVec3, e: f64, mu: f64) -> f64 {
        self.impulse_capped(at, n, e, mu, f64::MAX)
    }
    fn impulse_capped(&mut self, at: DVec3, n: DVec3, e: f64, mu: f64, cap: f64) -> f64 {
        rb_impulse(self.pos, &mut self.vel, &mut self.w, self.rot, self.inertia, self.kind.mass, at, n, e, mu, cap)
    }

    /// Hull point `k` at `p` is `depth` into something whose surface normal is `n`.
    /// Where it can crush (the crumple zones), it gives way as far as it is pushed,
    /// pushing back with about the force that holds the car at `crush_g`: the
    /// crushing takes the energy, the car does not bounce. Crushed as far as it
    /// goes, it is a hard hit.
    fn crumple_contact(&mut self, k: usize, p: DVec3, n: DVec3, depth: f64, kind: HitKind, hits: &mut Vec<Hit>) {
        let along = n.dot(self.rot * self.inward[k]);
        let vn = (self.vel + self.w.cross(p - self.pos)).dot(n);
        if along > 0.3 && self.crush[k] < self.crush_max[k] {
            let dc = (depth / along).min(self.crush_max[k] - self.crush[k]);
            self.crush[k] += dc;
            self.hull[k] = self.hull0[k] + self.inward[k] * self.crush[k];
            // (Six points across the front share the load in a square hit.)
            let force = self.kind.mass * 9.81 * self.kind.crush_g / 6.0;
            self.impulse_capped(p, n, 0.0, 0.3, force * self.h);
        } else {
            // Crushed as far as it goes: the frame behind it, much stiffer, but not infinitely.
            let force = self.kind.mass * 9.81 * self.kind.crush_g / 6.0 * 3.0;
            self.impulse_capped(p, n, 0.05, 0.35, force * self.h);
            self.pos += n * (depth * 0.5).min(0.05);
        }
        self.debug.contacts.push((p, n));
        if -vn > 2.0 {
            self.dent(p, n, -vn, kind, false, hits);
        }
    }

    /// Swing the unlatched parts on their hinges: thrown about by the car's
    /// acceleration and by gravity, an open hood caught by the wind, slamming
    /// against their stops (which wears the hinge until the part tears off).
    fn swing_parts(&mut self, h: f64, up: DVec3) {
        let acc = (self.vel - self.prev_vel) / h;
        self.prev_vel = self.vel;
        if !self.parts.iter().any(|p| p.state == PartState::Swinging) {
            return;
        }
        let inv = self.rot.inverse();
        // What a part feels in the car's frame (per kg), and the oncoming air. (The
        // spike of a crash itself is cut off at 12 g: the parts do not collide with
        // what the car hit, which in truth holds them shut against it.)
        let f = inv * (-up * 9.81 - acc).clamp_length_max(120.0);
        let wind = (-(inv * self.vel).z).max(0.0);
        for (pi, p) in self.parts.iter_mut().enumerate() {
            if p.state != PartState::Swinging {
                continue;
            }
            let d = &p.def;
            let r = (p.centre - d.pivot).as_dvec3();
            let axis = d.axis.normalize().as_dvec3();
            let sign = d.sign as f64;
            let mut torque = r.cross(f).dot(axis) * p.mass * sign;
            if d.kind == 2 {
                // An open hood scoops the air and flies up.
                torque += 0.45 * wind * wind * (0.2 + p.angle.sin()).min(1.0);
            }
            let inertia = p.mass * r.length_squared() * 1.3 + 0.3;
            p.rate += (torque / inertia - 1.5 * p.rate) * h;
            p.angle += p.rate * h;
            if p.angle < 0.0 {
                p.angle = 0.0;
                p.rate = -p.rate * 0.2;
            }
            if p.angle > d.most as f64 {
                p.angle = d.most as f64;
                if p.rate > 6.0 {
                    p.hinge -= (p.rate - 6.0) * 0.08;
                }
                p.rate = -p.rate * 0.25;
            }
            if p.hinge <= 0.0 {
                p.state = PartState::Gone;
                self.shed.push(pi);
            }
        }
    }

    fn step(&mut self, h: f64, d: Drive, world: &World, walls: &[Wall], hits: &mut Vec<Hit>) {
        self.time += h;
        self.h = h;
        self.dent_cool -= h;
        let kind = self.kind;
        let m = kind.mass;
        let up = self.pos.normalize();
        // The ground's slope, from three samples about the car.
        let (e, nn, _) = geo::enu(self.pos);
        let g0 = ground(world, self.pos);
        let water = world.ground(geo::pos_to_lla(self.pos).0, geo::pos_to_lla(self.pos).1).is_some_and(|g| g.water);
        let normal = match (g0, ground(world, self.pos + e * 1.5), ground(world, self.pos + nn * 1.5)) {
            (Some(a), Some(b), Some(c)) => (e * 1.5 + up * (b - a)).cross(nn * 1.5 + up * (c - a)).normalize_or(up),
            _ => up,
        };
        let mut force = -up * 9.81 * m;
        let mut torque = DVec3::ZERO;
        let v2 = self.vel.length_squared();
        // Air: drag set by the top speed at full power, and downforce.
        let cda = kind.power / kind.vmax.powi(3);
        force -= self.vel * (cda * v2.sqrt()) * if water { 25.0 } else { 1.0 };
        force -= self.up() * kind.down * v2;
        // Steering, lighter at speed.
        let lock = kind.steer / (1.0 + v2.sqrt() / 12.0);
        let want = d.steer * lock;
        self.steer += (want - self.steer).clamp(-2.4 * h, 2.4 * h);
        let fwd_speed = self.vel.dot(self.forward());
        // Power falls with damage; a car with no wheels left has no drive.
        let alive = 1.0 - (self.damage * 0.7).min(0.85);
        let n_drive = self.lost.iter().filter(|l| !**l).count().max(1) as f64;
        let (mut drive_f, mut brake) = (0.0, 0.0);
        if d.throttle > 0.05 {
            if fwd_speed < -1.0 { brake = d.throttle } else { drive_f = d.throttle * (kind.power * alive / fwd_speed.abs().max(6.0)).min(kind.mu * m * 9.81 * 0.95) };
        } else if d.throttle < -0.05 {
            if fwd_speed > 1.0 { brake = -d.throttle } else { drive_f = d.throttle * (kind.power * 0.25 / fwd_speed.abs().max(4.0)).min(m * 4.0) };
        }
        let mu = kind.mu * if water { 0.15 } else { 1.0 };
        let share = m / 4.0;
        for w in 0..4 {
            if self.lost[w] {
                self.grounded[w] = false;
                continue;
            }
            let top = self.pos + self.rot * self.strut[w];
            let bdown = -self.up();
            // Down the strut to the ground.
            let gt = ground(world, top);
            let alt = top.length() - geo::R;
            let cosb = (-bdown).dot(up).max(0.2);
            let reach = gt.map(|g| (alt - g) / cosb - self.radius[w]).unwrap_or(f64::MAX);
            let max_len = self.rest + 0.02;
            if reach > max_len {
                // In the air: the wheel hangs at full droop.
                self.len[w] += (max_len - self.len[w]) * (h * 12.0).min(1.0);
                self.grounded[w] = false;
                self.debug.wheel_at[w] = None;
                self.spin_w[w] *= 1.0 - h * 0.3;
                self.spin[w] += self.spin_w[w] * h;
                continue;
            }
            let len = reach.max(0.0);
            let rate = (len - self.len[w]) / h;
            self.len[w] = len;
            self.grounded[w] = true;
            let contact = top + bdown * (len + self.radius[w]);
            self.debug.wheel_at[w] = Some(contact);
            let r = contact - self.pos;
            let mut nf = self.k * (self.rest - len) - self.c * rate;
            // The anti-roll bar: the wheel opposite pressed further in pushes this one down too.
            let other = w ^ 1;
            if self.grounded[other] && !self.lost[other] {
                nf += self.k * 1.1 * (self.len[other] - len);
            }
            // The bump stop.
            if len < 0.03 {
                nf += self.k * 8.0 * (0.03 - len);
            }
            let nf = nf.clamp(0.0, m * 9.81 * 3.0);
            let mut f = normal * nf;
            // The tyre, in the ground's plane.
            let steer = if w < 2 { self.steer } else { 0.0 };
            let heading = self.rot * DQuat::from_rotation_y(-steer) * DVec3::NEG_Z;
            let wf = (heading - normal * heading.dot(normal)).normalize_or_zero();
            let wr = wf.cross(normal);
            let v = self.vel + self.w.cross(r);
            let (vl, vs) = (v.dot(wf), v.dot(wr));
            let rear_locked = d.handbrake && w >= 2;
            // Tyres grip less per newton the harder they are pressed (which is what keeps cars on their wheels).
            let load = (nf / (share * 9.81) - 1.0).clamp(-1.0, 3.0);
            let grip = mu * nf * (1.0 - 0.14 * load) * if rear_locked { 0.55 } else { 1.0 };
            let slip = vs.atan2(vl.abs() + 0.8);
            let mut fs = -grip * (slip * 7.0).tanh();
            // Slow: never more than stops the slide this step.
            let stop_s = vs.abs() * share / h * 0.5;
            fs = fs.clamp(-stop_s, stop_s);
            let mut fl = drive_f / n_drive;
            let stop_l = vl.abs() * share / h * 0.5;
            if brake > 0.0 || rear_locked {
                let b = if rear_locked { grip * 0.9 } else { brake * mu * nf * 0.95 };
                fl -= vl.signum() * b.min(stop_l);
            }
            if d.throttle.abs() < 0.05 {
                // Off the gas: slowing, and holding still when parked.
                // (Rolling resistance and engine braking, about 0.6 m/s².)
                let hold = if vl.abs() < 0.6 { stop_l } else { 0.06 * nf };
                fl -= vl.signum() * hold.min(stop_l);
            }
            let ft = (fl * fl + fs * fs).sqrt();
            if ft > mu * nf && ft > 0.0 {
                let k = mu * nf / ft;
                fl *= k;
                fs *= k;
            }
            f += wf * fl + wr * fs;
            force += f;
            torque += r.cross(f);
            // The wheel turns with the ground (stopped if locked, spinning up if the drive slips).
            self.spin_w[w] = if rear_locked { 0.0 } else { vl / self.radius[w] + if d.throttle > 0.9 && fwd_speed < 8.0 && w >= 2 { 18.0 } else { 0.0 } };
            self.spin[w] += self.spin_w[w] * h;
            // A slide leaves dust or rubber smoke.
            if (vs.abs() > 6.0 || (rear_locked && vl.abs() > 6.0)) && (self.time * 30.0).fract() < h * 30.0 {
                hits.push(Hit { pos: contact, normal, speed: vs.abs().max(vl.abs()) * 0.3, kind: HitKind::Scrape });
            }
        }
        // Integrate.
        self.vel += force / m * h;
        self.w += self.inv_inertia_world() * torque * h;
        self.w *= 1.0 - h * 0.15;
        self.pos += self.vel * h;
        let wl = self.w.length();
        if wl > 1e-9 {
            self.rot = (DQuat::from_axis_angle(self.w / wl, wl * h) * self.rot).normalize();
        }
        // The hull against the ground.
        self.scrape = 0.0;
        for k in 0..self.hull.len() {
            let p = self.pos + self.rot * self.hull[k];
            let Some(g) = ground(world, p) else { continue };
            let depth = g - (p.length() - geo::R);
            if depth <= 0.0 {
                continue;
            }
            let vn = self.impulse(p, normal, 0.15, 0.55);
            self.pos += normal * (depth * 0.6).min(0.2);
            self.debug.contacts.push((p, normal));
            let slide = (self.vel + self.w.cross(p - self.pos)).reject_from(normal).length();
            if slide > 2.0 {
                self.scrape = self.scrape.max((slide / 20.0).min(1.0) as f32);
                if (self.time * 40.0 + k as f64 * 0.37).fract() < h * 40.0 {
                    hits.push(Hit { pos: p, normal, speed: slide, kind: HitKind::Scrape });
                }
            }
            if vn > 3.5 {
                self.dent(p, normal, vn, HitKind::Ground, true, hits);
            }
        }
        // The hull against the test walls.
        for k in 0..self.hull.len() {
            let p = self.pos + self.rot * self.hull[k];
            for wall in walls {
                if let Some((nrm, depth)) = wall.inside(p) {
                    self.crumple_contact(k, p, nrm, depth, HitKind::Wall, hits);
                }
            }
        }
        // The g meter: the change in velocity over the last four steps.
        let old = self.debug.vel_hist[0];
        self.debug.vel_hist.rotate_left(1);
        self.debug.vel_hist[3] = self.vel;
        self.debug.g = (self.vel - old).length() / (4.0 * h) / 9.81;
        if self.time > 0.5 {
            self.debug.g_peak = self.debug.g_peak.max(self.debug.g);
        }
        // On its roof or side and nearly still for a while: back onto its wheels (the marshals' job).
        if self.up().dot(up) < 0.3 && self.speed() < 1.0 {
            self.stuck += h;
        } else {
            self.stuck = 0.0;
        }
        if self.stuck > 2.5 {
            let f = self.forward();
            let flat = (f - up * f.dot(up)).normalize_or(self.right().cross(up));
            self.rot = DQuat::from_mat3(&DMat3::from_cols(flat.cross(up), up, -flat));
            self.pos += up * 1.0;
            self.w = DVec3::ZERO;
            self.vel = DVec3::ZERO;
            self.stuck = 0.0;
        }
        // The hull against buildings.
        if self.time - self.near_t > 0.25 || (self.pos - self.near_o).length() > 25.0 {
            self.refresh_near(world);
        }
        if !self.near.is_empty() {
            let (e, n, _) = geo::enu(self.near_o);
            for k in 0..self.hull.len() {
                let p = self.pos + self.rot * self.hull[k];
                let alt = p.length() - geo::R;
                let q = DVec2::new((p - self.near_o).dot(e), (p - self.near_o).dot(n));
                let mut push = None;
                for (ring, base, top, id, area) in &self.near {
                    if alt < base - 0.5 || alt > *top {
                        continue;
                    }
                    if let Some((out, depth)) = inside(ring, q) {
                        push = Some((out, depth, *id, *area));
                        break;
                    }
                }
                if let Some((out, depth, id, area)) = push {
                    let nrm = (e * out.x + n * out.y).normalize();
                    self.crumple_contact(k, p, nrm, depth, HitKind::Building(id.0, id.1, area), hits);
                }
            }
        }
        self.swing_parts(h, up);
        // The mesh catches up with the crush, sixty times a second.
        if self.time - self.last_flush >= 1.0 / 60.0 {
            self.last_flush = self.time;
            let cg = DVec3::new(0.0, self.kind.cg, 0.0);
            for k in 0..self.hull.len() {
                let dc = self.crush[k] - self.crush_shown[k];
                if dc > 0.004 {
                    self.crush_shown[k] = self.crush[k];
                    // The damage is how much of a crumple zone's worth is used up.
                    let used: f64 = self.crush.iter().sum::<f64>() / (6.0 * self.kind.crush[0]);
                    self.damage = self.damage.max(used.min(1.0));
                    let at = (self.hull0[k] + cg).as_vec3();
                    self.deform(at, self.inward[k].as_vec3(), dc as f32, (0.5 + self.crush[k] * 0.9) as f32);
                }
            }
        }
    }

    fn refresh_near(&mut self, world: &World) {
        self.near_t = self.time;
        self.near_o = self.pos;
        let (e, n, _) = geo::enu(self.pos);
        self.near.clear();
        for (ring, base, top, bridge, id, area) in footprints(world, self.pos, 40.0) {
            if bridge {
                continue;
            }
            let pts: Vec<DVec2> = ring.iter().map(|p| DVec2::new((*p - self.pos).dot(e), (*p - self.pos).dot(n))).collect();
            self.near.push((pts, base, top, id, area));
        }
    }

    /// A hit at `at` (planet frame), the surface's normal there (pointing out of the
    /// obstacle) and the speed lost: what it breaks (parts, glass, wheels), and, with
    /// `deform`, the dent (the crumple zones make their own, from the crush).
    #[allow(clippy::too_many_arguments)]
    fn dent(&mut self, at: DVec3, normal: DVec3, dv: f64, kind: HitKind, deform: bool, hits: &mut Vec<Hit>) {
        hits.push(Hit { pos: at, normal, speed: dv, kind });
        if self.debug.hits.last().is_none_or(|l| self.time - l.0 > 0.05 || l.1 != kind) {
            self.debug.hits.push((self.time, kind, dv));
            if self.debug.hits.len() > 8 {
                self.debug.hits.remove(0);
            }
        }
        let inv = self.rot.inverse();
        let p = (inv * (at - self.origin())).as_vec3();
        let push = (inv * normal).as_vec3();
        // (The mesh work is the costly part: not more than about fifteen times a second for small knocks.)
        let busy = self.dent_cool > 0.0 && dv < 8.0;
        if !busy {
            self.dent_cool = 0.06;
        }
        let dv = dv as f32;
        let radius = (0.35 + 0.055 * dv).min(1.5);
        if deform && !busy {
            self.damage = (self.damage + (dv as f64 / 60.0).powi(2)).min(1.0);
            self.deform(p, push, (0.0022 * dv * dv + 0.009 * dv).min(0.55), radius);
        }
        // Parts near the hit: latches let go, bolts shear, hinges tear.
        for (pi, part) in self.parts.iter_mut().enumerate() {
            if part.state == PartState::Gone || !part.near(p, 0.35) {
                continue;
            }
            // (Speeds lost at the part, m/s: what knocks it open, what tears it off.)
            let (pop, tear) = match part.def.kind {
                5 => (f32::MAX, 12.0),
                6 => (f32::MAX, 6.0),
                2 => (8.0, 18.0),
                _ => (9.0, 20.0),
            };
            if dv > tear {
                part.state = PartState::Gone;
                self.shed.push(pi);
                hits.push(Hit { pos: at, normal, speed: dv as f64, kind: HitKind::Part });
            } else if dv > pop && part.state == PartState::Latched {
                // Knocked open.
                part.state = PartState::Swinging;
                part.rate = 2.5;
            } else if part.state == PartState::Swinging {
                part.hinge -= (dv as f64 / 25.0).powi(2);
            }
        }
        // Glass near a hard hit bursts.
        let mut broke = 0;
        if dv > 12.0 && !busy {
            let reach = radius * 1.3;
            let glass = std::iter::once(&mut self.glass).chain(self.parts.iter_mut().filter(|p| p.state != PartState::Gone).map(|p| &mut p.glass));
            for s in glass {
                let mut n = 0;
                for t in s.i.chunks_mut(3) {
                    if t[0] == t[1] {
                        continue;
                    }
                    let c = (Vec3::from(s.v[t[0] as usize].pos) + Vec3::from(s.v[t[1] as usize].pos) + Vec3::from(s.v[t[2] as usize].pos)) / 3.0;
                    if c.distance(p) < reach {
                        t[1] = t[0];
                        t[2] = t[0];
                        n += 1;
                    }
                }
                if n > 0 {
                    s.idx_dirty = true;
                    s.dirty = Some(s.dirty.unwrap_or((0, 0)));
                    broke += n;
                }
            }
        }
        if broke > 0 {
            self.debug.glass_broken += broke;
            hits.push(Hit { pos: at, normal, speed: dv as f64, kind: HitKind::Glass });
        }
        // A wheel hit hard enough comes off.
        if dv > 15.0 {
            for w in 0..4 {
                let hub = (self.strut[w] + DVec3::new(0.0, self.kind.cg, 0.0) - DVec3::Y * self.len[w]).as_vec3();
                if !self.lost[w] && hub.distance(p) < 1.0 {
                    self.lost[w] = true;
                    self.shed_wheels.push(w);
                    hits.push(Hit { pos: at, normal, speed: dv as f64, kind: HitKind::Wheel });
                }
            }
        }
    }

    /// Push the skin in around `p` (model frame) along `push` by up to `depth`,
    /// fading out at `radius`, crinkled; every mesh of the car that is still on it.
    fn deform(&mut self, p: Vec3, push: Vec3, depth: f32, radius: f32) {
        let skins = [&mut self.body, &mut self.glass].into_iter().chain(self.parts.iter_mut().filter(|q| q.state != PartState::Gone).flat_map(|q| [&mut q.skin, &mut q.glass]));
        for s in skins {
            let mut touched: Option<(usize, usize)> = None;
            for (k, v) in s.v.iter_mut().enumerate() {
                let q = Vec3::from(v.pos);
                let dist = q.distance(p);
                if dist > radius {
                    continue;
                }
                let x = dist / radius;
                let fall = (1.0 - x * x).powi(2);
                let crinkle = 0.65 + 0.7 * hash(k as u32);
                let side = Vec3::new(hash(k as u32 ^ 0x9e37) - 0.5, hash(k as u32 ^ 0x51ed) - 0.5, hash(k as u32 ^ 0x2f1b) - 0.5);
                let mut np = q + push * depth * fall * crinkle + side * depth * fall * 0.18;
                // Metal folds, it does not stretch forever.
                let rest = s.rest[k];
                let moved = np - rest;
                if moved.length() > 0.95 {
                    np = rest + moved.normalize() * 0.95;
                }
                v.pos = np.into();
                // Crinkled panels catch the light unevenly.
                v.nrm = (Vec3::from(v.nrm) + side * fall * (depth * 4.0).min(1.2)).normalize_or_zero().into();
                touched = Some(touched.map_or((k, k + 1), |(a, b)| (a.min(k), b.max(k + 1))));
            }
            if let Some((a, b)) = touched {
                s.dirty = Some(s.dirty.map_or((a, b), |(x, y)| (x.min(a), y.max(b))));
            }
        }
    }

    /// Each part: its name and what it is doing, and how far open.
    pub fn part_states(&self) -> Vec<(&str, PartState, f64)> {
        self.parts.iter().map(|p| (p.def.name.as_str(), p.state, p.angle)).collect()
    }
}

impl Debris {
    fn step(&mut self, h: f64, world: &World, walls: &[Wall]) {
        if self.still > 1.5 {
            return;
        }
        let up = self.pos.normalize();
        self.vel -= up * 9.81 * h;
        self.vel *= 1.0 - h * 0.05;
        self.w *= 1.0 - h * 0.3;
        self.pos += self.vel * h;
        let wl = self.w.length();
        if wl > 1e-9 {
            self.rot = (DQuat::from_axis_angle(self.w / wl, wl * h) * self.rot).normalize();
        }
        for k in 0..self.pts.len() {
            let p = self.pos + self.rot * self.pts[k];
            if let Some(g) = ground(world, p) {
                let depth = g - (p.length() - geo::R);
                if depth > 0.0 {
                    rb_impulse(self.pos, &mut self.vel, &mut self.w, self.rot, self.inertia, self.mass, p, up, 0.25, 0.55, f64::MAX);
                    self.pos += up * (depth * 0.8).min(0.3);
                }
            }
            for wall in walls {
                if let Some((n, depth)) = wall.inside(p) {
                    rb_impulse(self.pos, &mut self.vel, &mut self.w, self.rot, self.inertia, self.mass, p, n, 0.2, 0.4, f64::MAX);
                    self.pos += n * (depth * 0.8).min(0.3);
                }
            }
        }
        if self.vel.length() < 0.15 && self.w.length() < 0.4 {
            self.still += h;
        } else {
            self.still = 0.0;
        }
    }
}

fn collide(a: &mut Car, b: &mut Car, hits: &mut Vec<Hit>) {
    if (a.pos - b.pos).length() > (a.length() + b.length()) * 0.6 {
        return;
    }
    let balls = |c: &Car| {
        let r = ((c.hi.x - c.lo.x) as f64 * 0.5).max(0.6);
        let l = c.length() * 0.5 - r;
        [-1.0, 0.0, 1.0].map(|s| (c.pos + c.forward() * (l * s), r))
    };
    for (pa, ra) in balls(a) {
        for (pb, rb) in balls(b) {
            let d = pb - pa;
            let dist = d.length();
            if dist >= ra + rb || dist < 1e-6 {
                continue;
            }
            let n = d / dist;
            let at = pa + n * ra;
            let (ma, mb) = (a.kind.mass, b.kind.mass);
            let rv = (b.vel + b.w.cross(at - b.pos)) - (a.vel + a.w.cross(at - a.pos));
            let vn = rv.dot(n);
            let pen = ra + rb - dist;
            a.pos -= n * pen * mb / (ma + mb);
            b.pos += n * pen * ma / (ma + mb);
            if vn >= 0.0 {
                continue;
            }
            let j = -(1.0 + 0.2) * vn / (1.0 / ma + 1.0 / mb);
            a.vel -= n * (j / ma);
            b.vel += n * (j / mb);
            let iia = a.inv_inertia_world();
            let iib = b.inv_inertia_world();
            a.w -= iia * (at - a.pos).cross(n * j) * 0.5;
            b.w += iib * (at - b.pos).cross(n * j) * 0.5;
            if -vn > 2.0 {
                a.dent(at, -n, -vn * mb / (ma + mb) * 2.0, HitKind::Car, true, hits);
                b.dent(at, n, -vn * ma / (ma + mb) * 2.0, HitKind::Car, true, hits);
            }
        }
    }
}

/// The nearest place to `at` where a car of `size` facing `heading` stands clear of every building.
pub fn clear_spot(world: &World, at: DVec3, heading: f64, size: DVec3) -> DVec3 {
    let (e, n, _) = geo::enu(at);
    let f = n * heading.cos() + e * heading.sin();
    let r = f.cross(at.normalize());
    let clear = |p: DVec3| {
        [(0.0, 0.0), (0.55, 0.55), (0.55, -0.55), (-0.55, 0.55), (-0.55, -0.55), (0.55, 0.0), (-0.55, 0.0)].iter().all(|(a, b)| {
            let q = p + f * (a * size.z) + r * (b * size.x);
            let (la, lo, _) = geo::pos_to_lla(q);
            world.building(la, lo).is_none()
        })
    };
    if clear(at) {
        return at;
    }
    // Out in rings, 2 m apart.
    for ring in 1..40 {
        let rad = ring as f64 * 2.0;
        let steps = (ring * 8).max(8);
        for k in 0..steps {
            let a = k as f64 / steps as f64 * std::f64::consts::TAU;
            let p = at + (e * a.cos() + n * a.sin()) * rad;
            if clear(p) {
                return p;
            }
        }
    }
    at
}

/// Inside a ring (flat frame)? Then the way out through the nearest edge and how far.
fn inside(ring: &[DVec2], q: DVec2) -> Option<(DVec2, f64)> {
    let n = ring.len();
    if n < 3 {
        return None;
    }
    let mut c = false;
    let mut best = (f64::MAX, DVec2::ZERO);
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a.y > q.y) != (b.y > q.y) && q.x < (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
        let ab = b - a;
        let t = ((q - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
        let p = a + ab * t;
        let dd = (q - p).length();
        if dd < best.0 {
            best = (dd, p);
        }
    }
    if !c {
        return None;
    }
    let out = (best.1 - q).normalize_or_zero();
    Some((out, best.0))
}

/// Height of the ground (or a building's roof is not counted: they collide from the side).
pub fn ground(world: &World, p: DVec3) -> Option<f64> {
    let (la, lo, _) = geo::pos_to_lla(p);
    world.ground(la, lo).map(|g| g.h)
}

/// Standing buildings near a point: outline, base, top, bridge, which, area.
pub fn footprints(world: &World, p: DVec3, r: f64) -> Vec<(Vec<DVec3>, f64, f64, bool, (geo::TileId, usize), f32)> {
    world.footprints_near_ids(p, r)
}

/// A box (half extents) of rough painted concrete.
fn box_mesh(h: Vec3, col: [f32; 3]) -> (Vec<PartVertex>, Vec<u32>) {
    let (mut v, mut i) = (Vec::new(), Vec::new());
    for axis in 0..3 {
        for s in [-1.0f32, 1.0] {
            let mut n = Vec3::ZERO;
            n[axis] = s;
            let (u, w) = ((axis + 1) % 3, (axis + 2) % 3);
            let b = v.len() as u32;
            for (a, c) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let mut p = n * h;
                p[u] = a * h[u];
                p[w] = c * h[w];
                v.push(PartVertex { pos: p.into(), nrm: n.into(), col: [col[0], col[1], col[2], 1.0], uv: [0.0; 2], mat: [mat::PLASTIC, 0.0, 0.0, 0.0] });
            }
            if s > 0.0 { i.extend([b, b + 1, b + 2, b, b + 2, b + 3]) } else { i.extend([b, b + 2, b + 1, b, b + 3, b + 2]) }
        }
    }
    (v, i)
}

fn hash(x: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    h ^= h >> 15;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 13;
    (h & 0xFFFF) as f32 / 65535.0
}

/// Sparks, dust and glass for the hits of a step.
pub fn particles(hits: &[Hit], rng: &mut Rng, out: &mut Vec<(DVec3, Particle)>) {
    for hit in hits {
        let up = hit.pos.normalize().as_vec3();
        let n = hit.normal.as_vec3();
        let s = hit.speed as f32;
        let dir = |rng: &mut Rng| (n * rng.range(0.2, 1.0) + Vec3::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)) * 0.7 + up * 0.4).normalize_or_zero();
        match hit.kind {
            HitKind::Scrape => {
                for _ in 0..3 {
                    let d = dir(rng);
                    out.push((hit.pos, Particle { vel: (d * rng.range(3.0, 10.0) * (s / 15.0).min(1.5)).into(), life: rng.range(0.3, 0.9), size0: rng.range(0.03, 0.07), size1: 0.02, kind: SPARK, drag: 0.6, seed: rng.f(), alpha: rng.range(0.5, 1.0), ..Default::default() }));
                }
                out.push((hit.pos, Particle { vel: (up * 1.0).into(), life: rng.range(1.5, 3.0), size0: 0.5, size1: 2.5, kind: DUST, drag: 1.5, buoy: 0.5, seed: rng.f(), alpha: 0.25, ..Default::default() }));
            }
            HitKind::Glass => {
                for _ in 0..40 {
                    let d = dir(rng);
                    out.push((hit.pos, Particle { vel: (d * rng.range(2.0, 7.0) + up * 1.5).into(), life: rng.range(1.0, 2.2), size0: rng.range(0.015, 0.04), size1: 0.015, kind: GLASS, drag: 0.15, seed: rng.f(), alpha: 1.0, ..Default::default() }));
                }
            }
            _ if s > 4.0 => {
                let k = ((s / 3.0) as usize).min(25);
                for _ in 0..k {
                    let d = dir(rng);
                    out.push((hit.pos, Particle { vel: (d * rng.range(4.0, 14.0) * (s / 15.0).min(2.0)).into(), life: rng.range(0.4, 1.2), size0: rng.range(0.04, 0.09), size1: 0.02, kind: SPARK, drag: 0.5, seed: rng.f(), alpha: 1.0, ..Default::default() }));
                }
                for _ in 0..(k / 3).max(1) {
                    out.push((hit.pos, Particle { vel: (dir(rng) * 2.0 + up).into(), life: rng.range(2.0, 4.0), size0: 0.8, size1: 4.0, kind: DUST, drag: 1.2, buoy: 0.6, seed: rng.f(), alpha: 0.35, ..Default::default() }));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn flat_world() -> World {
        let dir = std::env::temp_dir().join("openflight-test-cache");
        let src = Arc::new(crate::sources::Sources::new(crate::fetch::Fetcher::new(dir, true), String::new()));
        let mut w = World::new(crate::sources::Loader::new(src, 0), 14);
        w.flat = Some(0.0);
        w
    }
    fn run(g: &mut Garage, w: &World, secs: f64, d: Drive) -> Vec<Hit> {
        let mut hits = Vec::new();
        for _ in 0..(secs * 120.0) as usize {
            g.step(1.0 / 120.0, Some(0), d, w, &mut hits);
        }
        hits
    }
    fn agl(c: &Car) -> f64 {
        c.origin().length() - geo::R
    }

    #[test]
    fn settles_accelerates_turns_and_stops() {
        let w = flat_world();
        for k in 0..KINDS.len() {
            let mut g = Garage::default();
            g.spawn(k, geo::lla_to_pos(0.6, -2.1, 0.0), 0.0, &w);
            run(&mut g, &w, 3.0, Drive::default());
            let c = &g.cars[0];
            assert!(c.grounded.iter().all(|x| *x), "{}: not on its wheels", c.kind.key);
            assert!(agl(c).abs() < 0.08, "{}: rests at {:.3} m", c.kind.key, agl(c));
            assert!(c.speed() < 0.05, "{}: creeps at {:.3} m/s", c.kind.key, c.speed());
            run(&mut g, &w, 4.0, Drive { throttle: 1.0, ..Default::default() });
            let v = g.cars[0].vel.dot(g.cars[0].forward());
            println!("{}: {:.1} m/s after 4 s", KINDS[k].key, v);
            assert!(v > 15.0 && v < 46.0, "{}: {v:.1} m/s after 4 s full throttle", KINDS[k].key);
            let h0 = g.cars[0].forward();
            run(&mut g, &w, 2.0, Drive { throttle: 0.3, steer: 1.0, ..Default::default() });
            let c = &g.cars[0];
            let turned = h0.angle_between(c.forward()).to_degrees();
            assert!(turned > 30.0, "{}: turned {turned:.0}°", c.kind.key);
            assert!(c.up().dot(c.pos.normalize()) > 0.8, "{}: rolled over in a turn", c.kind.key);
            // Holding S brakes to a stop (and then reverses).
            let mut slowest = f64::MAX;
            for _ in 0..6 * 120 {
                g.step(1.0 / 120.0, Some(0), Drive { throttle: -1.0, ..Default::default() }, &w, &mut Vec::new());
                slowest = slowest.min(g.cars[0].speed());
            }
            assert!(slowest < 1.0, "{}: never slower than {slowest:.1} m/s braking", KINDS[k].key);
            assert!(g.cars[0].vel.dot(g.cars[0].forward()) < -1.0, "{}: does not reverse", KINDS[k].key);
        }
    }

    #[test]
    fn a_dropped_car_dents_and_survives() {
        let w = flat_world();
        let mut g = Garage::default();
        g.spawn(1, geo::lla_to_pos(0.6, -2.1, 0.0), 0.0, &w);
        // Drop it on its nose from 6 m.
        let c = &mut g.cars[0];
        c.pos += c.pos.normalize() * 6.0;
        c.rot = c.rot * DQuat::from_rotation_x(-1.2);
        let hits = run(&mut g, &w, 4.0, Drive::default());
        let c = &g.cars[0];
        assert!(hits.iter().any(|h| h.speed > 5.0), "no hard hit");
        assert!(c.body.dirty.is_some() || c.damage > 0.0, "no dent");
        assert!(c.pos.is_finite() && agl(c) > -1.0 && agl(c) < 3.0, "ended at {:.2} m", agl(c));
        let moved = c.body.v.iter().zip(&c.body.rest).map(|(v, r)| (Vec3::from(v.pos) - *r).length()).fold(0.0f32, f32::max);
        println!("deepest dent {moved:.2} m, damage {:.2}", c.damage);
        assert!(moved > 0.05 && moved <= 0.76);
    }

    #[test]
    fn into_a_wall_and_reset() {
        let w = flat_world();
        let mut g = Garage::default();
        let p = geo::lla_to_pos(0.6, -2.1, 0.0);
        let (e, n, u) = geo::enu(p);
        g.spawn(0, p, 0.0, &w);
        // A wall 30 m ahead, across the road.
        g.walls.push(Wall::new(p + n * 30.0 + u * 1.5, e, u, DVec3::new(6.0, 1.5, 0.5)));
        run(&mut g, &w, 1.0, Drive::default());
        g.cars[0].launch(n * 25.0);
        let hits = run(&mut g, &w, 3.0, Drive::default());
        let c = &g.cars[0];
        let front = (c.pos - p).dot(n) + c.length() / 2.0;
        let gone: Vec<_> = c.part_states().into_iter().filter(|p| p.1 != PartState::Latched).map(|p| format!("{} {:?}", p.0, p.1)).collect();
        println!("nose {front:.2} m along (wall face at 29.5), now {:.1} m/s, peak {:.0} g, damage {:.2}, dent {:.2} m, glass {}, parts: {gone:?}, debris {}", c.speed(), c.debug.g_peak, c.damage, c.deepest_dent(), c.debug.glass_broken, g.debris.len());
        assert!(hits.iter().any(|h| h.kind == HitKind::Wall && h.speed > 15.0), "no hard hit on the wall");
        assert!(front < 30.6, "went through the wall: nose at {front:.2} m");
        // The crumple zone takes the energy: a real car's 30-60 g, and hardly a bounce.
        assert!(c.debug.g_peak > 15.0 && c.debug.g_peak < 60.0, "a 90 km/h wall hit at {:.0} g", c.debug.g_peak);
        assert!(front > 28.0, "bounced {:.1} m back off the wall", 29.5 - front);
        assert!(c.deepest_dent() > 0.3, "barely dented");
        // The front bumper comes off, the hood is knocked open (or off), and they lie about.
        assert!(c.part_states().iter().any(|p| p.0 == "bumper front" && p.1 == PartState::Gone), "front bumper still on");
        assert!(c.part_states().iter().any(|p| p.0 == "hood front" && p.1 != PartState::Latched), "hood still latched");
        assert!(!g.debris.is_empty());
        g.reset(0, p, 0.0, &w);
        assert!(g.cars[0].deepest_dent() < 1e-6 && g.cars[0].damage == 0.0 && g.cars.len() == 1);
        assert!(g.debris.is_empty(), "the parts lying about came back with the reset");
        assert!(g.cars[0].part_states().iter().all(|p| p.1 == PartState::Latched));
    }

    #[test]
    fn a_side_hit_knocks_the_doors_open() {
        let w = flat_world();
        let mut g = Garage::default();
        let p = geo::lla_to_pos(0.6, -2.1, 0.0);
        let (e, n, u) = geo::enu(p);
        g.spawn(0, p, 0.0, &w);
        // A wall just off the car's left side; the car slides into it sideways at about 40 km/h.
        g.walls.push(Wall::new(p - e * 2.2 + u * 1.5, n, u, DVec3::new(8.0, 1.5, 0.5)));
        run(&mut g, &w, 1.0, Drive::default());
        g.cars[0].launch(-e * 12.0);
        run(&mut g, &w, 2.5, Drive::default());
        let states = g.cars[0].part_states();
        println!("side hit: {:?}, peak {:.0} g, hits {:?}", states.iter().filter(|p| p.1 != PartState::Latched).collect::<Vec<_>>(), g.cars[0].debug.g_peak, g.cars[0].debug.hits);
        let left_doors = states.iter().filter(|p| p.0.starts_with("door left") && p.1 != PartState::Latched).count();
        let right_doors = states.iter().filter(|p| p.0.starts_with("door right") && p.1 != PartState::Latched).count();
        assert!(left_doors >= 1, "no left door knocked open");
        assert_eq!(right_doors, 0, "the far side's doors opened");
    }

    #[test]
    fn two_cars_collide() {
        let w = flat_world();
        let mut g = Garage::default();
        let p = geo::lla_to_pos(0.6, -2.1, 0.0);
        let (_, n, _) = geo::enu(p);
        g.spawn(0, p, 0.0, &w);
        g.spawn(1, p + n * 30.0, 0.0, &w);
        run(&mut g, &w, 1.0, Drive::default());
        g.cars[0].vel = n * 25.0;
        let hits = run(&mut g, &w, 3.0, Drive::default());
        assert!(hits.iter().any(|h| h.kind == HitKind::Car), "no crash");
        assert!(g.cars[1].vel.dot(n) > 2.0 || (g.cars[1].pos - (p + n * 30.0)).length() > 2.0, "the struck car did not move");
        assert!((g.cars[0].pos - g.cars[1].pos).length() > 2.5, "they went through each other");
    }
}
