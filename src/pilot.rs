//! The pilot on foot: a photo-scanned person (from `tools/vehicle_import.py
//! ped`), skinned on the CPU every frame from a baked walk cycle.
//!
//! The walk is played at the rate the pilot is really moving, measured from
//! the model's own stride (a planted foot slides back at the walking speed),
//! so the feet stay put on the ground. Standing still blends to the frame of
//! the cycle with the feet together; running plays the cycle faster and leans
//! into it; turning on the spot shuffles the feet. Getting in and out of a
//! car or an aircraft is walking to the door, turning to it and stepping up
//! into the seat (or down out of it), the camera gliding between the two.
use crate::airframe::{mat, pat};
use crate::geo;
use crate::render::{DrawObj, PartVertex, Renderer};
use crate::world::World;
use glam::{DMat3, DQuat, DVec2, DVec3, Mat4, Vec3, Vec4};
use std::io::Read;

const SHADER_KIND: f32 = 21.0;
const TEX_KEY: u32 = 1000;
static MODEL: &[u8] = include_bytes!("../assets/vehicles/pilot.bin.gz");
static TEXTURE: &[u8] = include_bytes!("../assets/vehicles/pilot.jpg");

const WALK: f64 = 1.45;
const RUN: f64 = 3.4;
const BACK: f64 = 0.9;
const TURN: f64 = 2.6;

struct Model {
    rest: Vec<(Vec3, Vec3, [u8; 4], [f32; 4])>,
    out: Vec<PartVertex>,
    nj: usize,
    nf: usize,
    fps: f32,
    stride_speed: f32,
    still: usize,
    /// per frame, per joint: rows of a 3x4 skinning matrix
    frames: Vec<[Vec4; 3]>,
    mesh: usize,
    tex: u8,
}

fn load(r: &mut Renderer) -> Model {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(MODEL).read_to_end(&mut raw).expect("pilot model");
    let u32_ = |b: &[u8]| u32::from_le_bytes(b[..4].try_into().unwrap());
    let f32_ = |b: &[u8]| f32::from_le_bytes(b[..4].try_into().unwrap());
    assert_eq!(&raw[0..4], b"PED1", "pilot model: not a model file");
    let (nv, nj, nf) = (u32_(&raw[4..]) as usize, u32_(&raw[8..]) as usize, u32_(&raw[12..]) as usize);
    let (fps, stride_speed) = (f32_(&raw[16..]), f32_(&raw[20..]));
    let still = u32_(&raw[28..]) as usize;
    let mut at = 32;
    let mut rest = Vec::with_capacity(nv);
    let mut out = Vec::with_capacity(nv);
    for c in raw[at..at + nv * 32].chunks(32) {
        let p = Vec3::new(f32_(&c[0..]), f32_(&c[4..]), f32_(&c[8..]));
        let n = Vec3::new(c[12] as i8 as f32, c[13] as i8 as f32, c[14] as i8 as f32) / 127.0;
        let uv = [f32_(&c[16..]), f32_(&c[20..])];
        let j = [c[24], c[25], c[26], c[27]];
        let w = [c[28] as f32 / 255.0, c[29] as f32 / 255.0, c[30] as f32 / 255.0, c[31] as f32 / 255.0];
        rest.push((p, n, j, w));
        out.push(PartVertex { pos: p.into(), nrm: n.into(), col: [1.0; 4], uv, mat: [mat::FABRIC, pat::TEX, 0.0, 0.0] });
    }
    at += nv * 32;
    let ni = u32_(&raw[at..]) as usize;
    at += 4;
    let idx: Vec<u32> = raw[at..at + ni * 4].chunks(4).map(u32_).collect();
    at += ni * 4;
    let frames = raw[at..at + nf * nj * 48]
        .chunks(48)
        .map(|c| {
            let f: Vec<f32> = c.chunks(4).map(f32_).collect();
            [Vec4::new(f[0], f[1], f[2], f[3]), Vec4::new(f[4], f[5], f[6], f[7]), Vec4::new(f[8], f[9], f[10], f[11])]
        })
        .collect();
    let mesh = r.add_dynamic_mesh(&out, &idx);
    let tex = r.plane_tex_slot(TEX_KEY, &[(TEXTURE, None)]);
    Model { rest, out, nj, nf, fps, stride_speed, still, frames, mesh, tex }
}

/// Stepping into a seat or out of one: from, to, how far along, how long.
struct Step {
    from: DVec3,
    to: DVec3,
    t: f64,
    dur: f64,
    /// facing during it (radians from north)
    face: f64,
}

pub struct Pilot {
    model: Option<Model>,
    /// feet, planet frame
    pub pos: DVec3,
    /// which way the pilot faces, radians from north
    pub heading: f64,
    speed: f64,
    vy: f64,
    phase: f32,
    shuffle: f32,
    lean: f64,
    /// walking to a door by itself: where, and which way to face there
    pub auto: Option<(DVec3, f64)>,
    step: Option<Step>,
    pub shown: bool,
    near: Vec<(Vec<DVec2>, f64, f64)>,
    near_o: DVec3,
}

/// What the keys ask for on foot.
#[derive(Default, Clone, Copy)]
pub struct Walk {
    pub fwd: f64,
    pub turn: f64,
    pub run: bool,
}

impl Pilot {
    pub fn new() -> Pilot {
        Pilot { model: None, pos: DVec3::X * geo::R, heading: 0.0, speed: 0.0, vy: 0.0, phase: 0.0, shuffle: 0.0, lean: 0.0, auto: None, step: None, shown: false, near: Vec::new(), near_o: DVec3::ZERO }
    }

    pub fn rot(&self) -> DQuat {
        let (e, n, u) = geo::enu(self.pos);
        let f = n * self.heading.cos() + e * self.heading.sin();
        DQuat::from_mat3(&DMat3::from_cols(f.cross(u), u, -f))
    }
    pub fn forward(&self) -> DVec3 {
        let (e, n, _) = geo::enu(self.pos);
        n * self.heading.cos() + e * self.heading.sin()
    }
    /// The heading (radians from north) of a direction at the pilot.
    pub fn heading_of(&self, d: DVec3) -> f64 {
        let (e, n, _) = geo::enu(self.pos);
        d.dot(e).atan2(d.dot(n))
    }
    pub fn busy(&self) -> bool {
        self.step.is_some() || self.auto.is_some()
    }
    pub fn stepping(&self) -> bool {
        self.step.is_some()
    }

    /// Stand at `at` (on the ground) facing `heading`, after stepping down out of `seat`.
    pub fn get_out(&mut self, seat: DVec3, at: DVec3, heading: f64, world: &World) {
        let g = crate::vehicle::ground(world, at).unwrap_or(at.length() - geo::R);
        let to = at.normalize() * (geo::R + g);
        self.pos = seat;
        self.heading = heading;
        self.step = Some(Step { from: seat, to, t: 0.0, dur: 0.75, face: heading });
        self.shown = true;
        self.speed = 0.0;
        self.vy = 0.0;
        self.auto = None;
    }

    /// Climb from where the pilot stands up into `seat`, facing `face`.
    pub fn get_in(&mut self, seat: DVec3, face: f64) {
        self.step = Some(Step { from: self.pos, to: seat, t: 0.0, dur: 0.6, face });
        self.auto = None;
    }

    /// Returns true once a `get_in` has finished (the pilot is in the seat).
    pub fn update(&mut self, dt: f64, walk: Walk, world: &World) -> bool {
        if dt <= 0.0 {
            return false;
        }
        if let Some(s) = &mut self.step {
            s.t += dt;
            let k = (s.t / s.dur).min(1.0);
            let e = k * k * (3.0 - 2.0 * k);
            let up = s.to.normalize();
            // A little hop up and over the sill.
            self.pos = s.from.lerp(s.to, e) + up * (0.25 * (k * std::f64::consts::PI).sin());
            self.heading = s.face;
            self.speed = 0.6;
            self.phase += (dt * 0.8) as f32;
            if k >= 1.0 {
                let into_seat = (s.to - s.from).dot(up) > 0.2;
                self.pos = s.to;
                self.step = None;
                self.speed = 0.0;
                return into_seat;
            }
            return false;
        }
        let mut walk = walk;
        if let Some((target, face)) = self.auto {
            // Walk to the door by itself, turn to face it.
            let d = target - self.pos;
            let flat = d - self.pos.normalize() * d.dot(self.pos.normalize());
            let dist = flat.length();
            if dist < 0.3 {
                walk = Walk { fwd: 0.0, turn: wrap(face - self.heading).clamp(-1.0, 1.0) * 2.0, run: false };
                if wrap(face - self.heading).abs() < 0.12 {
                    self.auto = None;
                    return true;
                }
            } else {
                let want = self.heading_of(flat);
                let turn = wrap(want - self.heading);
                walk = Walk { fwd: if turn.abs() < 0.9 { (dist / 1.0).min(1.0) } else { 0.0 }, turn: (turn * 3.0).clamp(-1.0, 1.0), run: dist > 6.0 };
            }
        }
        self.heading = wrap(self.heading + walk.turn * TURN * dt);
        let target = if walk.fwd > 0.0 { walk.fwd * if walk.run { RUN } else { WALK } } else { walk.fwd * BACK };
        let accel = if target.abs() > self.speed.abs() { 5.0 } else { 7.0 };
        self.speed += (target - self.speed).clamp(-accel * dt, accel * dt);
        self.lean += ((self.speed / RUN).max(0.0) * 0.12 - self.lean) * (dt * 5.0).min(1.0);
        let up = self.pos.normalize();
        let step = self.forward() * self.speed * dt;
        let mut next = self.pos + step;
        // Buildings: walk into a wall and stop against it; walk off a roof and fall.
        if (next - self.near_o).length() > 30.0 || self.near.is_empty() {
            self.near_o = next;
            let (e, n, _) = geo::enu(next);
            self.near = crate::vehicle::footprints(world, next, 40.0)
                .into_iter()
                .filter(|f| !f.3)
                .map(|(ring, base, top, ..)| (ring.iter().map(|p| DVec2::new((*p - next).dot(e), (*p - next).dot(n))).collect(), base, top))
                .collect();
        }
        let mut floor = crate::vehicle::ground(world, next).unwrap_or(next.length() - geo::R);
        {
            let (e, n, _) = geo::enu(self.near_o);
            let q = DVec2::new((next - self.near_o).dot(e), (next - self.near_o).dot(n));
            let alt = next.length() - geo::R;
            for (ring, base, top) in &self.near {
                if alt < *base - 0.5 || !inside(ring, q) {
                    continue;
                }
                if alt > top - 0.6 {
                    floor = floor.max(*top);
                } else {
                    next = self.pos;
                    self.speed = 0.0;
                }
            }
        }
        let alt = next.length() - geo::R;
        let mut h = alt;
        if alt > floor + 0.05 {
            self.vy -= 9.81 * dt;
            h = (alt + self.vy * dt).max(floor);
        } else {
            h = h.max(floor);
            if alt < floor + 0.6 {
                h = floor;
            }
        }
        if h <= floor + 1e-3 {
            self.vy = 0.0;
        }
        self.pos = next.normalize() * (geo::R + h);
        let _ = up;
        // The cycle runs at the speed the feet would really carry it.
        if let Some(m) = &self.model {
            // (Backwards plays it backwards; turning on the spot shuffles.)
            let rate = self.speed as f32 / m.stride_speed.max(0.1);
            let shuffling = self.speed.abs() < 0.2 && walk.turn.abs() > 0.1;
            self.shuffle += ((if shuffling { 1.0 } else { 0.0 }) - self.shuffle) * (dt as f32 * 6.0).min(1.0);
            self.phase += if shuffling { 0.7 } else { rate } * dt as f32;
        }
        false
    }

    pub fn draw(&mut self, cam: DVec3, r: &mut Renderer, objs: &mut Vec<DrawObj>) {
        if !self.shown {
            return;
        }
        if self.model.is_none() {
            self.model = Some(load(r));
        }
        let rot = self.rot();
        let m = self.model.as_mut().unwrap();
        // The pose: two frames of the cycle, blended toward standing when slow.
        let len = m.nf as f32 / m.fps;
        let t = self.phase.rem_euclid(len) * m.fps;
        let (f0, a) = (t.floor() as usize % m.nf, t.fract());
        let f1 = (f0 + 1) % m.nf;
        let moving = ((self.speed.abs() as f32) / 0.5).min(1.0);
        let b = moving.max(self.shuffle * 0.45);
        let nj = m.nj;
        let pose: Vec<[Vec4; 3]> = (0..nj)
            .map(|j| {
                let (p, q, s) = (m.frames[f0 * nj + j], m.frames[f1 * nj + j], m.frames[m.still * nj + j]);
                [0, 1, 2].map(|r| {
                    let walk = p[r] * (1.0 - a) + q[r] * a;
                    s[r] * (1.0 - b) + walk * b
                })
            })
            .collect();
        for (v, (p, n, j, w)) in m.out.iter_mut().zip(&m.rest) {
            let mut row = [Vec4::ZERO; 3];
            for k in 0..4 {
                if w[k] <= 0.0 {
                    continue;
                }
                let mj = &pose[j[k] as usize];
                for r in 0..3 {
                    row[r] += mj[r] * w[k];
                }
            }
            let p4 = p.extend(1.0);
            v.pos = [row[0].dot(p4), row[1].dot(p4), row[2].dot(p4)];
            let n4 = n.extend(0.0);
            v.nrm = Vec3::new(row[0].dot(n4), row[1].dot(n4), row[2].dot(n4)).normalize_or_zero().into();
        }
        r.write_mesh(m.mesh, &m.out, None);
        let lean = Mat4::from_rotation_x(-self.lean as f32);
        let base = Mat4::from_rotation_translation(rot.as_quat(), (self.pos - cam).as_vec3()) * lean;
        objs.push(DrawObj { mesh: m.mesh, model: base, glow: [0.0, SHADER_KIND, 0.0, 0.0], cast: 0b11, tex: m.tex, ..DrawObj::DEFAULT });
    }
}

fn wrap(a: f64) -> f64 {
    (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}

fn inside(ring: &[DVec2], q: DVec2) -> bool {
    let n = ring.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a.y > q.y) != (b.y > q.y) && q.x < (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
    }
    c
}
