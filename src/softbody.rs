//! A car's body as a lattice of nodes and beams (the way BeamNG builds its
//! cars), so it deforms where and as far as it is pushed, and the deformation
//! spreads through the structure.
//!
//! The nodes are made from the car's own mesh: the body's bounding box is cut
//! into cells, and every cell with some of the car in it gets a node at the
//! centre of what is there. Each node is joined by a beam to every other within
//! a little under two cells, which braces the lattice in every direction.
//!
//! What hits the car pushes nodes in (`push`). Then `relax` lets the beams pull
//! their neighbours along (position based: each beam nudged back toward its
//! length a few times over). A beam squashed or stretched past its yield point
//! keeps its new length: the dent stays. Stretched far enough it snaps. Each
//! node can only go so far from where it started: about a metre at the front,
//! less at the back, the sides and the roof, and little in the passenger cell,
//! which is also three times as strong.
//!
//! The mesh follows (`Skinning`): every vertex is tied to its four nearest
//! nodes, and moves with them.
//!
//! The lattice lives in the model frame (+X right, +Y up, -Z forward, origin on
//! the ground under the middle). It carries no momentum of its own: the car is
//! still one rigid body for driving, so the handling is untouched.
use glam::Vec3;
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub struct Beam {
    pub a: u16,
    pub b: u16,
    /// its length now (it changes when the beam yields), and as built
    l0: f32,
    lb: f32,
    pub broken: bool,
}
impl Beam {
    /// How far it has yielded: 0 as built, 1 at twice the yield strain.
    pub fn bent(&self) -> f32 {
        ((self.l0 - self.lb).abs() / (self.lb * YIELD * 2.0)).min(1.0)
    }
}

/// Strain (fraction of length) past which a beam yields, and at which it snaps.
const YIELD: f32 = 0.03;
const SNAP: f32 = 0.6;

#[derive(Clone)]
pub struct Lattice {
    pub rest: Vec<Vec3>,
    pub pos: Vec<Vec3>,
    /// how far each node may go from where it started (m), and the force it
    /// resists with while it goes (N)
    pub max: Vec<f32>,
    pub strength: Vec<f32>,
    pub beams: Vec<Beam>,
    /// nodes pushed since the last relax (held where they were pushed to while it runs)
    pinned: Vec<bool>,
    /// nodes that have moved since the mesh last followed
    pub dirty: Vec<bool>,
    pub cell: f32,
}

/// What the car says about its structure: how far its front, rear and sides
/// crush (m), and the force a square hit on the front is held at (N).
pub struct Build {
    pub lo: Vec3,
    pub hi: Vec3,
    pub crush: [f32; 3],
    pub front_force: f32,
}

impl Lattice {
    /// Nodes from the mesh's vertices (model frame), beams between neighbours.
    pub fn build(verts: &[Vec3], b: &Build) -> Lattice {
        let size = b.hi - b.lo;
        let cell = (size.max_element() / 14.0).max(0.25);
        let key = |p: Vec3| {
            let q = (p - b.lo) / cell;
            (q.x.floor() as i32, q.y.floor() as i32, q.z.floor() as i32)
        };
        // A node at the middle of what of the car is in each cell.
        let mut sums: HashMap<(i32, i32, i32), (Vec3, u32)> = HashMap::new();
        for &p in verts {
            let e = sums.entry(key(p)).or_insert((Vec3::ZERO, 0));
            e.0 += p;
            e.1 += 1;
        }
        let mut cells: Vec<_> = sums.into_iter().filter(|(_, (_, n))| *n >= 3).collect();
        cells.sort_by_key(|(k, _)| *k);
        let rest: Vec<Vec3> = cells.iter().map(|(_, (s, n))| *s / *n as f32).collect();
        let n = rest.len();
        let mut grid: HashMap<(i32, i32, i32), Vec<u16>> = HashMap::new();
        for (i, p) in rest.iter().enumerate() {
            grid.entry(key(*p)).or_default().push(i as u16);
        }
        // Beams to every node within reach.
        let reach = cell * 1.75;
        let mut beams = Vec::new();
        for (i, p) in rest.iter().enumerate() {
            let (x, y, z) = key(*p);
            for dx in -2..=2 {
                for dy in -2..=2 {
                    for dz in -2..=2 {
                        for &j in grid.get(&(x + dx, y + dy, z + dz)).map(|v| v.as_slice()).unwrap_or(&[]) {
                            let j = j as usize;
                            if j <= i {
                                continue;
                            }
                            let l = p.distance(rest[j]);
                            if l < reach && l > 1e-3 {
                                beams.push(Beam { a: i as u16, b: j as u16, l0: l, lb: l, broken: false });
                            }
                        }
                    }
                }
            }
        }
        // How far each node may crush: by the face of the car it is nearest; deep
        // inside (the passenger cell) not far, and three times as strong.
        let front_nodes = rest.iter().filter(|p| p.z < b.lo.z + cell * 1.2).count().max(1) as f32;
        let base = b.front_force / front_nodes;
        let (mut max, mut strength) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for p in &rest {
            let faces = [(p.z - b.lo.z, b.crush[0]), (b.hi.z - p.z, b.crush[1]), (p.x - b.lo.x, b.crush[2]), (b.hi.x - p.x, b.crush[2]), (b.hi.y - p.y, 0.3)];
            let (d, m) = faces.iter().copied().fold((f32::MAX, 0.0), |acc, f| if f.0 < acc.0 { f } else { acc });
            let deep = d > b.crush[2] + cell;
            max.push(if deep { 0.2 } else { m });
            strength.push(if deep { base * 3.0 } else { base });
        }
        Lattice { pos: rest.clone(), rest, max, strength, beams, pinned: vec![false; n], dirty: vec![false; n], cell }
    }

    pub fn len(&self) -> usize {
        self.rest.len()
    }

    /// How far node `i` has moved from where it started.
    pub fn moved(&self, i: usize) -> f32 {
        self.pos[i].distance(self.rest[i])
    }

    /// Can node `i` give any further (along `dir`)?
    pub fn can_give(&self, i: usize, dir: Vec3) -> bool {
        let next = self.pos[i] + dir.normalize_or_zero() * 0.01;
        next.distance(self.rest[i]) < self.max[i]
    }

    /// Push node `i` by `d` (it stops at its limit); held there until the next relax.
    pub fn push(&mut self, i: usize, d: Vec3) {
        let mut p = self.pos[i] + d;
        let off = p - self.rest[i];
        if off.length() > self.max[i] {
            p = self.rest[i] + off.normalize() * self.max[i];
        }
        self.pos[i] = p;
        self.pinned[i] = true;
        self.dirty[i] = true;
    }

    /// The beams pull the structure after what was pushed; bent too far they
    /// stay bent, stretched too far they snap. Returns whether anything moved.
    pub fn relax(&mut self, iters: usize) -> bool {
        if !self.pinned.iter().any(|p| *p) {
            return false;
        }
        for _ in 0..iters {
            for bm in &self.beams {
                if bm.broken {
                    continue;
                }
                let (a, b) = (bm.a as usize, bm.b as usize);
                let d = self.pos[b] - self.pos[a];
                let l = d.length();
                if l < 1e-6 {
                    continue;
                }
                let c = (l - bm.l0) / l * 0.5;
                let (wa, wb) = (if self.pinned[a] { 0.0 } else { 1.0 }, if self.pinned[b] { 0.0 } else { 1.0 });
                let sum = wa + wb;
                if sum == 0.0 {
                    continue;
                }
                let corr = d * c * (2.0 / sum) * 0.6;
                if wa > 0.0 {
                    self.pos[a] += corr * wa;
                    self.dirty[a] = true;
                }
                if wb > 0.0 {
                    self.pos[b] -= corr * wb;
                    self.dirty[b] = true;
                }
            }
            // Nobody leaves their limits (the passenger cell stays a cell).
            for i in 0..self.pos.len() {
                let off = self.pos[i] - self.rest[i];
                if off.length() > self.max[i] {
                    self.pos[i] = self.rest[i] + off.normalize() * self.max[i];
                }
            }
        }
        // Yield and snap.
        for bm in &mut self.beams {
            if bm.broken {
                continue;
            }
            let l = self.pos[bm.a as usize].distance(self.pos[bm.b as usize]);
            if l > bm.lb * (1.0 + SNAP) {
                bm.broken = true;
                continue;
            }
            let strain = (l - bm.l0) / bm.lb;
            if strain.abs() > YIELD {
                bm.l0 += (strain - YIELD * strain.signum()) * bm.lb;
            }
        }
        self.pinned.iter_mut().for_each(|p| *p = false);
        true
    }

    /// The nodes within `r` of `p` (where they are now), nearest first.
    pub fn near(&self, p: Vec3, r: f32) -> Vec<usize> {
        let mut out: Vec<(f32, usize)> = self.pos.iter().enumerate().filter_map(|(i, q)| {
            let d = q.distance(p);
            (d < r).then_some((d, i))
        }).collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().map(|x| x.1).collect()
    }

    /// How bent the structure is at each node: the most any of its beams has
    /// yielded (m of length gained or lost), or snapped (counted as 0.2 m).
    pub fn bends(&self) -> Vec<f32> {
        let mut b = vec![0.0f32; self.rest.len()];
        for bm in &self.beams {
            let x = if bm.broken { 0.2 } else { (bm.l0 - bm.lb).abs() };
            for i in [bm.a as usize, bm.b as usize] {
                b[i] = b[i].max(x);
            }
        }
        b
    }

    /// Beams that have yielded and that have snapped.
    pub fn stats(&self) -> (usize, usize) {
        (self.beams.iter().filter(|b| !b.broken && b.bent() > 0.05).count(), self.beams.iter().filter(|b| b.broken).count())
    }
}

/// Each vertex's four nearest nodes and how much each moves it.
pub struct Skinning {
    idx: Vec<[u16; 4]>,
    w: Vec<[f32; 4]>,
}

impl Skinning {
    pub fn new(l: &Lattice, verts: &[Vec3]) -> Skinning {
        // Nodes binned in cells of the lattice's size, to look only near each vertex.
        let c = l.cell;
        let key = |p: Vec3| ((p.x / c).floor() as i32, (p.y / c).floor() as i32, (p.z / c).floor() as i32);
        let mut bins: HashMap<(i32, i32, i32), Vec<u16>> = HashMap::new();
        for (i, p) in l.rest.iter().enumerate() {
            bins.entry(key(*p)).or_default().push(i as u16);
        }
        let mut idx = Vec::with_capacity(verts.len());
        let mut w = Vec::with_capacity(verts.len());
        for &v in verts {
            let (x, y, z) = key(v);
            let mut best = [(f32::MAX, 0u16); 4];
            for r in 1..4 {
                for dx in -r..=r {
                    for dy in -r..=r {
                        for dz in -r..=r {
                            for &i in bins.get(&(x + dx, y + dy, z + dz)).map(|b| b.as_slice()).unwrap_or(&[]) {
                                let d = l.rest[i as usize].distance_squared(v);
                                if d < best[3].0 && !best.iter().any(|b| b.1 == i && b.0 < f32::MAX) {
                                    best[3] = (d, i);
                                    best.sort_by(|a, b| a.0.total_cmp(&b.0));
                                }
                            }
                        }
                    }
                }
                if best[3].0 < f32::MAX {
                    break;
                }
            }
            let mut ws = [0.0f32; 4];
            for k in 0..4 {
                if best[k].0 < f32::MAX {
                    ws[k] = 1.0 / (best[k].0.sqrt() + 0.05).powi(2);
                }
            }
            let sum: f32 = ws.iter().sum::<f32>().max(1e-9);
            idx.push([best[0].1, best[1].1, best[2].1, best[3].1]);
            w.push(ws.map(|x| x / sum));
        }
        Skinning { idx, w }
    }

    /// Where vertex `k` (resting at `rest`) is now, how bent the structure under
    /// it is (`bends`), and whether any of its nodes moved.
    pub fn place(&self, l: &Lattice, bends: &[f32], k: usize, rest: Vec3) -> (Vec3, f32, bool) {
        let mut d = Vec3::ZERO;
        let mut bend = 0.0;
        let mut moved = false;
        for j in 0..4 {
            let i = self.idx[k][j] as usize;
            let w = self.w[k][j];
            if w > 0.0 {
                d += (l.pos[i] - l.rest[i]) * w;
                bend += bends[i] * w;
                moved |= l.dirty[i];
            }
        }
        (rest + d, bend, moved)
    }

    pub fn len(&self) -> usize {
        self.idx.len()
    }
}
