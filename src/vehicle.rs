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
#[derive(PartialEq, Clone, Copy)]
pub enum HitKind {
    Ground,
    Scrape,
    Building(geo::TileId, usize, f32),
    Car,
    Glass,
    Wheel,
}

// ---------------------------------------------------------------- the model
struct Part {
    v: Vec<PartVertex>,
    i: Vec<u32>,
    hub: Vec3,
    r: f32,
}

struct Shape {
    body: Part,
    glass: Part,
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
        let ni = u32_(&take(4)) as usize;
        let i: Vec<u32> = take(ni * 4).chunks(4).map(|c| u32_(c)).collect();
        for p in &v {
            let p = Vec3::from(p.pos) + hub;
            lo = lo.min(p);
            hi = hi.max(p);
        }
        parts.push(Part { v, i, hub, r });
    }
    let mut it = parts.into_iter();
    let whole = it.next().unwrap();
    let wheels: [Part; 4] = [it.next().unwrap(), it.next().unwrap(), it.next().unwrap(), it.next().unwrap()];
    // The glass goes in its own mesh (it is drawn in the transparent pass).
    let split = |want_glass: bool| {
        let mut map = vec![u32::MAX; whole.v.len()];
        let mut p = Part { v: Vec::new(), i: Vec::new(), hub: Vec3::ZERO, r: 0.0 };
        for t in whole.i.chunks(3) {
            let g = whole.v[t[0] as usize].mat[0] == mat::GLASS;
            if g != want_glass {
                continue;
            }
            for &k in t {
                if map[k as usize] == u32::MAX {
                    map[k as usize] = p.v.len() as u32;
                    p.v.push(whole.v[k as usize]);
                }
                p.i.push(map[k as usize]);
            }
        }
        if p.i.is_empty() {
            p.v = vec![PartVertex::default(); 3];
            p.i = vec![0, 1, 2];
        }
        p
    };
    Shape { body: split(false), glass: split(true), wheels, lo, hi }
}

/// A car's own copy of a mesh, dented as it crashes.
struct Skin {
    v: Vec<PartVertex>,
    rest: Vec<Vec3>,
    i: Vec<u32>,
    /// on the GPU once first drawn
    mesh: Option<usize>,
    /// the vertices moved since the last upload (first, end), and whether the indices changed
    dirty: Option<(usize, usize)>,
    idx_dirty: bool,
}
impl Skin {
    fn new(p: &Part) -> Skin {
        Skin { v: p.v.clone(), rest: p.v.iter().map(|v| Vec3::from(v.pos)).collect(), i: p.i.clone(), mesh: None, dirty: None, idx_dirty: false }
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
    hull: Vec<DVec3>,
    lo: Vec3,
    hi: Vec3,
    body: Skin,
    glass: Skin,
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
}

/// Every car in the world, and the models they share.
#[derive(Default)]
pub struct Garage {
    shapes: Vec<Option<Shape>>,
    wheel_mesh: Vec<Option<[usize; 4]>>,
    pub cars: Vec<Car>,
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
        let mut hull = Vec::new();
        let (x0, x1, z0, z1) = (lo.x as f64 * 0.94, hi.x as f64 * 0.94, lo.z as f64 * 0.97, hi.z as f64 * 0.97);
        let ys = [lo.y as f64 + (radius[0] * 0.9).max(0.22), hi.y as f64 * 0.62, hi.y as f64 * 0.97];
        for (j, y) in ys.iter().enumerate() {
            let (sx, sz) = if j == 2 { (0.7, 0.45) } else { (1.0, 1.0) };
            for (x, z) in [(x0, z0), (x1, z0), (x0, z1), (x1, z1), (0.0, z0), (0.0, z1), (x0, 0.0), (x1, 0.0)] {
                hull.push(DVec3::new(x * sx, *y, z * sz) - cg);
            }
        }
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
            hull,
            lo,
            hi,
            body,
            glass,
            damage: 0.0,
            near: Vec::new(),
            near_o: DVec3::ZERO,
            near_t: -1.0,
            time: 0.0,
            scrape: 0.0,
            dent_cool: 0.0,
            stuck: 0.0,
        });
        self.cars.len() - 1
    }

    /// One physics step for every car (`drive` is for car `driven`, the rest coast with the handbrake on).
    pub fn step(&mut self, h: f64, driven: Option<usize>, drive: Drive, world: &World, hits: &mut Vec<Hit>) {
        let n = self.cars.len();
        for i in 0..n {
            let d = if Some(i) == driven { drive } else { Drive { throttle: 0.0, steer: 0.0, handbrake: true } };
            self.cars[i].step(h, d, world, hits);
        }
        // Car against car: each a row of three spheres along its length.
        for a in 0..n {
            for b in a + 1..n {
                let (l, r) = self.cars.split_at_mut(b);
                collide(&mut l[a], &mut r[0], hits);
            }
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
        for (ci, c) in self.cars.iter_mut().enumerate() {
            let wm = self.wheel_mesh[c.kind_index()].unwrap();
            let (body, glass) = (c.body.upload(r), c.glass.upload(r));
            let origin = c.pos - c.rot * DVec3::new(0.0, c.kind.cg, 0.0);
            let base = Mat4::from_rotation_translation(c.rot.as_quat(), (origin - cam).as_vec3());
            let o = DrawObj { glow: [0.0, SHADER_KIND, 0.0, ci as f32 * 0.37], cast: 0b11, ..DrawObj::DEFAULT };
            objs.push(DrawObj { mesh: body, model: base, ..o });
            objs.push(DrawObj { mesh: glass, model: base, glass: true, ..o });
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
    fn kind_index(&self) -> usize {
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
        let r = at - self.pos;
        let v = self.vel + self.w.cross(r);
        let vn = v.dot(n);
        if vn >= 0.0 {
            return 0.0;
        }
        let ii = self.inv_inertia_world();
        let m = self.kind.mass;
        let kn = 1.0 / m + n.dot((ii * r.cross(n)).cross(r));
        let j = -(1.0 + e) * vn / kn;
        self.vel += n * (j / m);
        self.w += ii * r.cross(n * j);
        let vt = v - n * vn;
        let st = vt.length();
        if st > 1e-4 {
            let t = vt / st;
            let kt = 1.0 / m + t.dot((ii * r.cross(t)).cross(r));
            let jt = (st / kt).min(mu * j);
            self.vel -= t * (jt / m);
            self.w -= ii * r.cross(t * jt);
        }
        -vn
    }

    fn step(&mut self, h: f64, d: Drive, world: &World, hits: &mut Vec<Hit>) {
        self.time += h;
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
                self.spin_w[w] *= 1.0 - h * 0.3;
                self.spin[w] += self.spin_w[w] * h;
                continue;
            }
            let len = reach.max(0.0);
            let rate = (len - self.len[w]) / h;
            self.len[w] = len;
            self.grounded[w] = true;
            let contact = top + bdown * (len + self.radius[w]);
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
                // Rolling resistance, and holding still when parked.
                let hold = if vl.abs() < 0.6 { stop_l } else { 0.015 * nf };
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
            let slide = (self.vel + self.w.cross(p - self.pos)).reject_from(normal).length();
            if slide > 2.0 {
                self.scrape = self.scrape.max((slide / 20.0).min(1.0) as f32);
                if (self.time * 40.0 + k as f64 * 0.37).fract() < h * 40.0 {
                    hits.push(Hit { pos: p, normal, speed: slide, kind: HitKind::Scrape });
                }
            }
            if vn > 3.5 {
                self.dent(p, normal, vn, HitKind::Ground, hits);
            }
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
                    let vn = self.impulse(p, nrm, 0.12, 0.35);
                    self.pos += nrm * (depth * 0.8).min(0.5);
                    if vn > 2.0 {
                        self.dent(p, nrm, vn, HitKind::Building(id.0, id.1, area), hits);
                    }
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

    /// Crumple the body where it struck: `at` (planet frame), the surface's
    /// normal there (pointing out of the obstacle), and the speed lost.
    fn dent(&mut self, at: DVec3, normal: DVec3, dv: f64, kind: HitKind, hits: &mut Vec<Hit>) {
        hits.push(Hit { pos: at, normal, speed: dv, kind });
        if self.dent_cool > 0.0 && dv < 8.0 {
            return;
        }
        self.dent_cool = 0.06;
        let inv = self.rot.inverse();
        let origin = self.origin();
        let p = (inv * (at - origin)).as_vec3();
        let push = (inv * normal).as_vec3();
        let dv = dv as f32;
        let radius = (0.35 + 0.055 * dv).min(1.5);
        let depth = (0.0022 * dv * dv + 0.009 * dv).min(0.55);
        self.damage = (self.damage + (dv as f64 / 60.0).powi(2)).min(1.0);
        let mut broke = 0;
        for (si, s) in [&mut self.body, &mut self.glass].into_iter().enumerate() {
            let mut touched: Option<(usize, usize)> = None;
            for (k, v) in s.v.iter_mut().enumerate() {
                let q = Vec3::from(v.pos);
                let dist = q.distance(p);
                if dist > radius {
                    continue;
                }
                let x = dist / radius;
                let fall = (1.0 - x * x).powi(2);
                let hsh = hash(k as u32);
                let crinkle = 0.65 + 0.7 * hsh;
                let side = Vec3::new(hash(k as u32 ^ 0x9e37) - 0.5, hash(k as u32 ^ 0x51ed) - 0.5, hash(k as u32 ^ 0x2f1b) - 0.5);
                let mut np = q + push * depth * fall * crinkle + side * depth * fall * 0.18;
                // Metal folds, it does not stretch forever.
                let rest = s.rest[k];
                let moved = np - rest;
                if moved.length() > 0.75 {
                    np = rest + moved.normalize() * 0.75;
                }
                v.pos = np.into();
                // Crinkled panels catch the light unevenly.
                let nrm = (Vec3::from(v.nrm) + side * fall * (depth * 4.0).min(1.2)).normalize_or_zero();
                v.nrm = nrm.into();
                touched = Some(touched.map_or((k, k + 1), |(a, b)| (a.min(k), b.max(k + 1))));
            }
            if let Some((a, b)) = touched {
                s.dirty = Some(s.dirty.map_or((a, b), |(x, y)| (x.min(a), y.max(b))));
            }
            // Glass near a hard hit bursts.
            if si == 1 && dv > 7.0 {
                let reach = radius * 1.3;
                for t in s.i.chunks_mut(3) {
                    if t[0] == t[1] {
                        continue;
                    }
                    let c = (Vec3::from(s.v[t[0] as usize].pos) + Vec3::from(s.v[t[1] as usize].pos) + Vec3::from(s.v[t[2] as usize].pos)) / 3.0;
                    if c.distance(p) < reach {
                        t[1] = t[0];
                        t[2] = t[0];
                        broke += 1;
                    }
                }
                if broke > 0 {
                    s.idx_dirty = true;
                    s.dirty = Some(s.dirty.unwrap_or((0, 0)));
                }
            }
        }
        if broke > 0 {
            hits.push(Hit { pos: at, normal, speed: dv as f64, kind: HitKind::Glass });
        }
        // A wheel hit hard enough comes off.
        if dv > 15.0 {
            for w in 0..4 {
                let hub = (self.strut[w] + DVec3::new(0.0, self.kind.cg, 0.0) - DVec3::Y * self.len[w]).as_vec3();
                if !self.lost[w] && hub.distance(p) < 1.0 {
                    self.lost[w] = true;
                    hits.push(Hit { pos: at, normal, speed: dv as f64, kind: HitKind::Wheel });
                }
            }
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
                a.dent(at, -n, -vn * mb / (ma + mb) * 2.0, HitKind::Car, hits);
                b.dent(at, n, -vn * ma / (ma + mb) * 2.0, HitKind::Car, hits);
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
