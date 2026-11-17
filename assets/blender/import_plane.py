# Turns a downloaded aircraft model (a glTF binary, not kept in the
# repository) into what the game loads, for any model:
#
#   blender -b --factory-startup --python assets/blender/import_plane.py -- <model.glb> <key>
#
# run from the repository's root. It writes
#   assets/<key>.bin.gz          the airframe, one mesh per piece (aircraft/imported.rs reads it)
#   assets/<key>/tex<N>.jpg      its textures at 1024 px (<N>a.jpg: alpha), the last a palette
#                                of the colours of its untextured materials
#   assets/aircraft/<key>.png    the start screen's picture
#   src/aircraft/measured/<key>.rs    its measurements: fuselage, wing, tail, gear, eye, contact points
#
# The model is cut into the pieces the damage model breaks off (by where they
# are: radome, nose, fuselage, tail cone, tail, left and right wing), the
# landing gear (the lowest parts, which slide up into the airframe when
# stowed), the glass, and, where the model names them (as GTA conversions do:
# aileron_l, elevator_r, rudder, ...), the control surfaces, hinged along their
# leading edges. The nose is found by which end has the fin.
import gzip
import os
import struct
import sys

import bpy
import numpy as np
from mathutils import Vector

ARGS = sys.argv[sys.argv.index("--") + 1:]
GLB, KEY = ARGS[:2]
# Optional third argument: +y or -y, the way the nose points in the file
# (otherwise guessed from which end the fin is at).
FWD = ARGS[2] if len(ARGS) > 2 else None
TEX = 1024
MAX_LAYERS = 15
TARGET_VERTS = 220_000

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=GLB)
dg = bpy.context.evaluated_depsgraph_get()


def base(name):
    n = name.split(".")[0]
    return n.lower()


# ---------------------------------------------------------------- materials
def bsdf(m):
    if not m or not m.node_tree:
        return None
    for n in m.node_tree.nodes:
        if n.type == "BSDF_PRINCIPLED":
            return n
    return None


def image_into(sock, depth=0):
    if depth > 3 or not sock.is_linked:
        return None
    n = sock.links[0].from_node
    if n.type == "TEX_IMAGE":
        return n.image if n.image and n.image.size[0] > 8 else None
    for i in n.inputs:
        if i.type in ("RGBA", "VALUE") and i.is_linked:
            im = image_into(i, depth + 1)
            if im:
                return im
    return None


MINFO = {}


def minfo(m):
    if m in MINFO:
        return MINFO[m]
    b = bsdf(m)
    img, col, glass, cut = None, (0.6, 0.6, 0.6), False, False
    name = (m.name if m else "").lower()
    any_img = None
    if m is not None and m.node_tree:
        ims = [n.image for n in m.node_tree.nodes if n.type == "TEX_IMAGE" and n.image and n.image.size[0] > 8]
        any_img = max(ims, key=lambda i: i.size[0] * i.size[1]) if ims else None
    if b:
        img = image_into(b.inputs["Base Color"])
        col = tuple(b.inputs["Base Color"].default_value[:3])
        a = b.inputs["Alpha"]
        tr = b.inputs.get("Transmission Weight") or b.inputs.get("Transmission")
        if any_img is None and not a.is_linked and a.default_value < 0.9:
            glass = True
        if tr is not None and not tr.is_linked and tr.default_value > 0.5:
            glass = True
        if a.is_linked and img is not None:
            cut = True
    # (GTA conversions hang their pictures off mix nodes the walk above may miss)
    if img is None:
        img = any_img
    if any(w in name for w in ("glass", "window", "canopy", "hudgls", "windscreen")) and img is None:
        glass = True
    MINFO[m] = (img, col, glass, cut)
    return MINFO[m]


# ---------------------------------------------------------------- triangles
SKIP = ("icosphere", "bound", "col", "shadow", "lod1", "lod2", "lod3")
objs = [o for o in bpy.data.objects if o.type == "MESH" and not any(base(o.name).startswith(s) or base(o.name).endswith("." + s) for s in SKIP) and ".col" not in o.name.lower()]
total = sum(len(o.data.vertices) for o in objs)
ratio = min(1.0, TARGET_VERTS / max(total, 1))
print("MODEL", len(objs), "meshes", total, "vertices; decimate", round(ratio, 3))


def tris_of(o):
    if ratio < 0.999 and len(o.data.vertices) > 2000:
        md = o.modifiers.new("dec", "DECIMATE")
        md.ratio = ratio
        md.use_collapse_triangulate = True
    oe = o.evaluated_get(bpy.context.evaluated_depsgraph_get())
    me = oe.to_mesh()
    me.calc_loop_triangles()
    nl, nt = len(me.loops), len(me.loop_triangles)
    if nt == 0:
        return None
    co = np.empty(len(me.vertices) * 3, np.float32)
    me.vertices.foreach_get("co", co)
    lv = np.empty(nl, np.int32)
    me.loops.foreach_get("vertex_index", lv)
    nr = np.empty(nl * 3, np.float32)
    me.corner_normals.foreach_get("vector", nr)
    uv = np.zeros(nl * 2, np.float32)
    if me.uv_layers:
        me.uv_layers[0].uv.foreach_get("vector", uv)
    tl = np.empty(nt * 3, np.int32)
    me.loop_triangles.foreach_get("loops", tl)
    tm = np.empty(nt, np.int32)
    me.loop_triangles.foreach_get("material_index", tm)
    mw = np.array(o.matrix_world, np.float32)
    P = co.reshape(-1, 3)[lv] @ mw[:3, :3].T + mw[:3, 3]
    N = nr.reshape(-1, 3) @ mw[:3, :3].T
    mats = list(me.materials) or [None]
    M = np.repeat(np.clip(tm, 0, len(mats) - 1), 3)
    out = (P[tl], N[tl], uv.reshape(-1, 2)[tl], M, mats)
    oe.to_mesh_clear()
    return out


raw = []  # (object base name, P, N, UV, material per corner)
for o in objs:
    t = tris_of(o)
    if t is None:
        continue
    P, N, U, M, mats = t
    raw.append((base(o.name), P, N, U, np.array([id(mats[i]) for i in M]), {id(m): m for m in mats}))

allP = np.concatenate([r[1] for r in raw])
lo, hi = allP.min(axis=0), allP.max(axis=0)
L = float(hi[1] - lo[1])
# Untextured shells over the whole aircraft (shadow and collision stand-ins) go.
keep = []
for r in raw:
    mm = r[5]
    textured = any(minfo(m)[0] is not None for m in mm.values())
    span = float(r[1][:, 1].max() - r[1][:, 1].min())
    if not textured and span > 0.6 * L and len(r[1]) < 3 * 4000:
        print("DROP shell", r[0], len(r[1]) // 3, "tris")
        continue
    keep.append(r)
raw = keep
allP = np.concatenate([r[1] for r in raw])
lo, hi = allP.min(axis=0), allP.max(axis=0)
L = float(hi[1] - lo[1])
cx = 0.5 * (lo[0] + hi[0])

# Which way is forward: the fin stands at the back.
cen = allP[np.abs(allP[:, 0] - cx) < 0.06 * L]
y0, y1 = lo[1] + L / 3, hi[1] - L / 3
top_lo = cen[cen[:, 1] < y0][:, 2].max()
top_hi = cen[cen[:, 1] > y1][:, 2].max()
F = 1.0 if top_lo > top_hi else -1.0  # +1: the nose is toward +y
if FWD:
    F = 1.0 if FWD == "+y" else -1.0
print("FORWARD", "+y" if F > 0 else "-y", "length", round(L, 2), "span", round(float(hi[0] - lo[0]), 2), "height", round(float(hi[2] - lo[2]), 2))


def to_game(P):
    return np.stack([F * (P[:, 0] - cx), P[:, 2], -F * P[:, 1]], axis=1)


def n_game(N):
    return np.stack([F * N[:, 0], N[:, 2], -F * N[:, 1]], axis=1)


G = to_game(allP)
zn, zt = G[:, 2].min(), G[:, 2].max()

# ---------------------------------------------------------------- measurements (before centring)
# Where the fuselage ends: the length of what lies at each distance out falls off.
ax = np.abs(G[:, 0])
bins = np.arange(0.0, ax.max() + 0.1, 0.1 * max(L / 15.0, 0.5))
ext = []
for b0, b1 in zip(bins[:-1], bins[1:]):
    q = G[(ax >= b0) & (ax < b1)]
    ext.append(q[:, 2].max() - q[:, 2].min() if len(q) > 10 else 0.0)
ext = np.array(ext)
x0 = float(bins[np.argmax(ext < 0.55 * ext[:3].max())]) if (ext < 0.55 * ext[:3].max()).any() else 0.5
x1 = float(ax.max())
zb0 = zt - 0.30 * (zt - zn)


def wing_cluster(q):
    z = np.sort(q[:, 2])
    gaps = np.where(np.diff(z) > 0.04 * L)[0]
    zmax = z[gaps[0]] if len(gaps) else z[-1]
    return q[q[:, 2] <= zmax]


xs, les, tes, ys = [], [], [], []
# (not out to the tip: tip tanks and missile rails would read as a long chord there)
for x in np.linspace(x0 + 0.1 * (x1 - x0), x0 + 0.8 * (x1 - x0), 14):
    q = G[(np.abs(ax - x) < 0.02 * L) & (G[:, 2] < zb0)]
    if len(q) < 20:
        continue
    w = wing_cluster(q)
    xs.append(x)
    les.append(np.percentile(w[:, 2], 1))
    tes.append(np.percentile(w[:, 2], 99))
    ys.append(np.median(w[:, 1]))
xs, les, tes, ys = map(np.array, (xs, les, tes, ys))


def robust_fit(x, y):
    p = np.polyfit(x, y, 1)
    r = np.abs(np.polyval(p, x) - y)
    k = r < max(np.percentile(r, 70) * 2.0, 0.05)
    return np.polyfit(x[k], y[k], 1) if k.sum() >= 3 else p


ple = robust_fit(xs, les)
pc = robust_fit(xs, tes - les)
py = robust_fit(xs, ys)
sweep = float(np.clip(ple[0], -0.1, 1.3))
le0 = float(np.polyval(ple, x0))
c0 = float(max(np.polyval(pc, x0), 0.3))
c1 = float(np.clip(np.polyval(pc, x1), 0.15, c0))
wy0 = float(np.polyval(py, x0))
di = float(np.clip(py[0], -0.2, 0.2))
xm = x0 + 0.4 * (x1 - x0)
zcg = float(np.polyval(ple, xm) + 0.3 * np.polyval(pc, xm))
area = float(2 * (x0 * c0 + 0.5 * (c0 + c1) * (x1 - x0)))
fus = G[ax < max(x0 * 0.9, 0.3)]


def section(z, half_band=None):
    q = fus[np.abs(fus[:, 2] - z) < (half_band or 0.025 * L)]
    if len(q) < 10:
        return None
    return float(np.percentile(q[:, 1], 98)), float(np.percentile(q[:, 1], 2)), float(np.percentile(np.abs(q[:, 0]), 98))


sc = section(zcg) or (1.0, -1.0, 1.0)
# The fuselage's top and bottom along its length, looked up by z (np.interp).
ZT = np.linspace(zn, zt, 120)
_s = [section(z, 0.02 * L) for z in ZT]
TOP = np.array([s[0] if s else sc[0] for s in _s])
BOT = np.array([s[1] if s else sc[1] for s in _s])
ycg = 0.5 * (sc[0] + sc[1])
O = np.array([0.0, ycg, zcg])
print("WING x0=%.2f x1=%.2f le0=%.2f sweep=%.3f c0=%.2f c1=%.2f y0=%.2f di=%.3f area~%.1f  CG y=%.2f z=%.2f" % (x0, x1, le0 - zcg, sweep, c0, c1, wy0 - ycg, di, area, ycg, zcg))

# ---------------------------------------------------------------- the pieces
def tri_centroids(Pg):
    return Pg.reshape(-1, 3, 3).mean(axis=1)


ymin = G[:, 1].min()
# Landing gear: the lowest points, in front (nose) and either side further back (mains).
low = G[G[:, 1] < ymin + 0.02 * L]
nose_c = low[low[:, 2] < zcg - 0.12 * L]
legs = []
if len(nose_c):
    legs.append(("nose gear", np.array([0.0, nose_c[:, 1].min(), float(np.median(nose_c[:, 2]))])))
# Mains: the lowest points out to the side (not the belly), either side.
band = G[(G[:, 2] > zcg - 0.12 * L) & (G[:, 2] < zcg + 0.2 * L)]
for side, sg in (("left", -1.0), ("right", 1.0)):
    q = band[band[:, 0] * sg > max(0.6 * x0, 0.08 * L)]
    if len(q):
        q = q[q[:, 1] < np.percentile(q[:, 1], 0.5) + 0.01 * L]
    if len(q):
        legs.append((side + " main gear", np.array([float(np.median(q[:, 0])), q[:, 1].min(), float(np.median(q[:, 2]))])))
if len(legs) < 3:
    # no gear modelled down: it rests on its belly where it is lowest
    legs = [("nose gear", np.array([0.0, ymin, zn + 0.15 * L])), ("left main gear", np.array([-max(x0, 0.8 * sc[2]), ymin, zcg + 0.05 * L])), ("right main gear", np.array([max(x0, 0.8 * sc[2]), ymin, zcg + 0.05 * L]))]
    print("NO GEAR found")


def fuse_bottom(z):
    return np.interp(z, ZT, BOT)


# Is the gear modelled down, or is the model in flight with it up? Without
# it, the game gives it the Hornet's (aircraft/fa18 `gear_into`), whose legs
# reach 1.21 m below where they hang from: so the contact points go down by that.
GEAR_MODELLED = bool(legs[0][1][1] < fuse_bottom(legs[0][1][2]) - 0.03 * L)
if not GEAR_MODELLED:
    print("GEAR not modelled: the Hornet's is borrowed")
    legs = [(n, at - np.array([0.0, 1.21, 0.0])) for n, at in legs]


pieces = {}  # name -> [P, N, U, style byte list...]


def add_tris(name, P, N, U, S):
    if name not in pieces:
        pieces[name] = [[], [], [], []]
    for k, a in enumerate((P, N, U, S)):
        pieces[name][k].append(a)


# Textures: the images used most (by area) get layers; the rest of the
# materials are painted from a palette of their colours.
area_by_img = {}
for r in raw:
    Pg = to_game(r[1]).reshape(-1, 3, 3)
    a = 0.5 * np.linalg.norm(np.cross(Pg[:, 1] - Pg[:, 0], Pg[:, 2] - Pg[:, 0]), axis=1)
    mid = r[4][::3]
    for k, m in r[5].items():
        img = minfo(m)[0]
        if img is not None:
            area_by_img[img.name] = area_by_img.get(img.name, 0.0) + float(a[mid == k].sum())
imgs = sorted(area_by_img, key=lambda n: -area_by_img[n])[:MAX_LAYERS]
LAYER = {n: i for i, n in enumerate(imgs)}
PAL = len(imgs)
palette = []


def pal_uv(col):
    col = tuple(round(c, 3) for c in col)
    if col not in palette:
        palette.append(col)
    i = min(palette.index(col), 63)
    return ((i % 8) + 0.5) / 8.0, ((i // 8) + 0.5) / 8.0


CONTROL = {"aileron_l": "left aileron", "aileron_r": "right aileron", "elevator_l": "left elevator", "elevator_r": "right elevator", "rudder": "rudder"}
INTERIOR = ("cockpit", "dials", "steeringwheel", "seat", "interior", "cabin", "pit")
GLASSN = ("windscreen", "window", "glass", "canopy")
cut_layers = set()
for name, P, N, U, M, mm in raw:
    Pg, Ng = to_game(P), n_game(N)
    Ug = np.stack([U[:, 0], 1.0 - U[:, 1]], axis=1)
    S = np.zeros(len(P), np.int64)
    glass = np.zeros(len(P), bool)
    for k, m in mm.items():
        img, col, gl, cut = minfo(m)
        sel = M == k
        if gl:
            glass[sel] = True
            S[sel] = 4 * 16
        elif img is not None and img.name in LAYER:
            style = 2 if any(w in name for w in INTERIOR) else 3 if cut else 0
            if cut:
                cut_layers.add(LAYER[img.name])
            S[sel] = style * 16 + LAYER[img.name]
        else:
            c = col if img is None else tuple(np.array(img.pixels[:4 * 64]).reshape(-1, 4)[:, :3].mean(axis=0))
            u, v = pal_uv(c)
            Ug[sel] = (u, v)
            S[sel] = (2 if any(w in name for w in INTERIOR) else 0) * 16 + PAL
    if any(w in name for w in GLASSN):
        glass[:] = True
        S[:] = 4 * 16
    C = tri_centroids(Pg)
    tsel = lambda m: np.repeat(m, 3)
    gm = tsel(glass.reshape(-1, 3).any(axis=1))
    if gm.any():
        add_tris("glass", Pg[gm], Ng[gm], Ug[gm], S[gm])
    rest = ~gm
    if name in CONTROL:
        # (by the side each triangle is on: a file's left and right may be mixed up)
        side = tsel(C[:, 0] < 0)
        for nm, m in ((CONTROL[name], rest & (side if "left" in CONTROL[name] else ~side) if name != "rudder" else rest), (CONTROL[name].replace("left", "right") if "left" in CONTROL[name] else CONTROL[name].replace("right", "left"), rest & (~side if "left" in CONTROL[name] else side) if name != "rudder" else None)):
            if m is not None and m.any():
                add_tris(nm, Pg[m], Ng[m], Ug[m], S[m])
        continue
    if any(w in name for w in INTERIOR):
        add_tris("interior", Pg[rest], Ng[rest], Ug[rest], S[rest])
        continue
    # by where each triangle is
    cx_, cy_, cz_ = C[:, 0], C[:, 1], C[:, 2]
    region = np.full(len(C), "fuselage", dtype=object)
    region[cz_ < zn + 0.20 * (zt - zn)] = "nose"
    region[cz_ < zn + 0.05 * (zt - zn)] = "radome"
    tail = cz_ > zb0
    region[tail] = "tail cone"
    region[tail & ((np.abs(cx_) > x0) | (cy_ > np.interp(cz_, ZT, TOP) + 0.02 * L))] = "tail"
    wing = (np.abs(cx_) > x0) & ~tail
    region[wing & (cx_ < 0)] = "left wing"
    region[wing & (cx_ > 0)] = "right wing"
    for lname, at in (legs if GEAR_MODELLED else []):
        r_xz = 0.06 * L if lname.startswith("nose") else 0.09 * L
        near = (np.hypot(cx_ - at[0], cz_ - at[2]) < r_xz) & (cy_ < fuse_bottom(cz_) - 0.004 * L)
        region[near] = lname
    for reg in set(region):
        m = tsel(region == reg) & rest
        if m.any():
            add_tris(reg, Pg[m], Ng[m], Ug[m], S[m])


def cat(name):
    return [np.concatenate(a) for a in pieces[name]]


# ---------------------------------------------------------------- control surfaces the model does not have
# Cut out of the trailing edges: along the span (or up the fin) in strips,
# whatever lies in the last part of each strip's chord.
def split_piece(src, dst, tri_mask):
    P, N, U, S = cat(src)
    m = np.repeat(tri_mask, 3)
    pieces[src] = [[P[~m]], [N[~m]], [U[~m]], [S[~m]]]
    if m.any():
        add_tris(dst, P[m], N[m], U[m], S[m])
        print("CUT %-14s %5d tris from %s" % (dst, int(tri_mask.sum()), src))


def te_mask(C, a, lo_s, hi_s, frac0, within=None, nb=28):
    within = np.ones(len(C), bool) if within is None else within
    if within.sum() < 8:
        return np.zeros(len(C), bool)
    amin, amax = a[within].min(), a[within].max()
    s = (a - amin) / max(amax - amin, 1e-6)
    out = np.zeros(len(C), bool)
    for b0, b1 in zip(np.linspace(0, 1, nb + 1)[:-1], np.linspace(0, 1, nb + 1)[1:]):
        k = within & (s >= b0) & (s < b1 + (1e-6 if b1 >= 1 else 0))
        if k.sum() < 4 or b1 <= lo_s or b0 >= hi_s:
            continue
        z = C[k, 2]
        le, te = np.percentile(z, 1), np.percentile(z, 99)
        f = (z - le) / max(te - le, 1e-6)
        out[np.where(k)[0][f > frac0]] = True
    return out


def tri_info(name):
    P = cat(name)[0].reshape(-1, 3, 3)
    C = P.mean(axis=1)
    n = np.cross(P[:, 1] - P[:, 0], P[:, 2] - P[:, 0])
    n /= np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-12)
    return C, n


def wing_at(C):
    d = np.clip(np.abs(C[:, 0]) - x0, 0.0, x1 - x0)
    c = c0 + (c1 - c0) * d / max(x1 - x0, 1e-6)
    return wy0 + di * d, le0 + sweep * d, c


def wing_slab(C):
    yw, le, c = wing_at(C)
    return (np.abs(C[:, 1] - yw) < 0.10 * c + 0.03) & (C[:, 2] > le - 0.1 * c) & (C[:, 2] < le + 1.15 * c)


if "left aileron" not in pieces:
    for side in ("left", "right"):
        w = side + " wing"
        if w not in pieces:
            continue
        # only the wing's own skin: what lies in the thin slab about the
        # measured wing plane (not nacelles, pods, pylons or stores)
        C, n = tri_info(w)
        m = te_mask(C, np.abs(C[:, 0]), 0.12, 0.52, 0.72, wing_slab(C))
        if m.sum() < 20:  # a wing the planform fit does not describe (a delta): its flat skin instead
            m = te_mask(C, np.abs(C[:, 0]), 0.12, 0.52, 0.84, np.abs(n[:, 1]) > 0.5)
        split_piece(w, side + " flap", m)
        C, n = tri_info(w)
        m = te_mask(C, np.abs(C[:, 0]), 0.58, 0.93, 0.75, wing_slab(C))
        if m.sum() < 20:
            m = te_mask(C, np.abs(C[:, 0]), 0.58, 0.93, 0.84, np.abs(n[:, 1]) > 0.5)
        split_piece(w, side + " aileron", m)
if "left elevator" not in pieces and "tail" in pieces:
    for side, sg in (("left", -1.0), ("right", 1.0)):
        C, n = tri_info("tail")
        stab = (C[:, 0] * sg > max(0.5 * x0, 0.02 * L)) & (np.abs(n[:, 1]) > 0.6)
        # behind the wing, not the back of it, and all at the tailplane's height
        yw, le, c = wing_at(C)
        stab &= (np.abs(C[:, 0]) > x1) | (C[:, 2] > le + c + 0.02 * L)
        if stab.sum() > 8:
            stab &= np.abs(C[:, 1] - np.median(C[stab, 1])) < 0.025 * L
        split_piece("tail", side + " elevator", te_mask(C, np.abs(C[:, 0]), 0.0, 1.0, 0.62, stab))
if "rudder" not in pieces and "tail" in pieces:
    C, n = tri_info("tail")
    fin = (np.abs(n[:, 0]) > 0.55) & (C[:, 1] > np.interp(C[:, 2], ZT, TOP) + 0.01 * L)
    twin = fin.any() and np.median(np.abs(C[fin, 0])) > 0.4
    if twin:
        for nm, sg in (("left rudder", -1.0), ("rudder", 1.0)):
            C, n = tri_info("tail")
            fin = (np.abs(n[:, 0]) > 0.55) & (C[:, 1] > np.interp(C[:, 2], ZT, TOP) + 0.01 * L) & (C[:, 0] * sg > 0)
            split_piece("tail", nm, te_mask(C, C[:, 1], 0.15, 0.95, 0.66, fin))
    else:
        split_piece("tail", "rudder", te_mask(C, C[:, 1], 0.15, 0.95, 0.66, fin))


# Hinges: control surfaces along their leading edges, gear sliding up.
def hinge(P, spanwise):
    k = 0 if spanwise else 1
    a = P[:, k] if not spanwise else np.abs(P[:, 0])
    ends = []
    for near in (a < a.min() + 0.15 * (a.max() - a.min()), a > a.max() - 0.15 * (a.max() - a.min())):
        q = P[near]
        ends.append(q[np.argmin(q[:, 2])])
    d = ends[1] - ends[0]
    if spanwise and d[0] < 0:
        d = -d
    if not spanwise and d[1] < 0:
        d = -d
    return ends[0], d / np.linalg.norm(d)


out = [b"IMP1", b""]
count = 0
for name in sorted(pieces):
    P, N, U, S = cat(name)
    P = P - O
    pivot, axis, angle = np.zeros(3), np.array([1.0, 0.0, 0.0]), 0.0
    if name in ("left aileron", "right aileron", "left elevator", "right elevator", "left flap", "right flap"):
        pivot, axis = hinge(P, True)
    elif name in ("rudder", "left rudder"):
        pivot, axis = hinge(P, False)
    elif name.endswith("gear"):
        # how far it slides up to stow: its height below the fuselage's bottom
        angle = max(float(fuse_bottom(float(np.median(P[:, 2])) + zcg) - ycg - P[:, 1].min()) * 0.9, 0.0)
    n8 = np.clip(np.round(N / np.maximum(np.linalg.norm(N, axis=1, keepdims=True), 1e-9) * 127), -127, 127).astype(np.int64)
    key = np.concatenate([np.round(P * 1e4).astype(np.int64), n8, np.round(U * 1e4).astype(np.int64), S[:, None]], axis=1)
    _, first, inv = np.unique(key, axis=0, return_index=True, return_inverse=True)
    v = np.zeros(len(first), np.dtype([("pos", "<f4", 3), ("nrm", "i1", 3), ("sl", "u1"), ("uv", "<f4", 2)]))
    v["pos"], v["nrm"], v["sl"], v["uv"] = P[first], n8[first], S[first], U[first]
    idx = inv.reshape(-1).astype("<u4")
    nb = name.encode()
    out += [struct.pack("<I", len(nb)), nb, struct.pack("<7f", *pivot, *axis, angle), struct.pack("<I", len(v)), v.tobytes(), struct.pack("<I", len(idx)), idx.tobytes()]
    count += 1
    print("PIECE %-16s v=%6d t=%6d lo=(%.1f %.1f %.1f) hi=(%.1f %.1f %.1f) stow=%.2f" % (name, len(v), len(idx) // 3, *P.min(axis=0), *P.max(axis=0), angle))
out[1] = struct.pack("<I", count)
os.makedirs("assets/aircraft", exist_ok=True)
with open("assets/%s.bin.gz" % KEY, "wb") as f:
    f.write(gzip.compress(b"".join(out), 9, mtime=0))

# ---------------------------------------------------------------- textures
os.makedirs("assets/%s" % KEY, exist_ok=True)
for f in os.listdir("assets/%s" % KEY):
    os.remove(os.path.join("assets/%s" % KEY, f))


def save(px, path):
    o = bpy.data.images.new("o", TEX, TEX, alpha=False)
    o.pixels.foreach_set(px.reshape(-1).astype(np.float32))
    # (Blender reads a relative path from the .blend file's folder: there is none)
    o.filepath_raw = os.path.abspath(path)
    o.file_format = "JPEG"
    o.save()
    bpy.data.images.remove(o)


tex_rs = []
for n, k in sorted(LAYER.items(), key=lambda t: t[1]):
    img = bpy.data.images[n].copy()
    img.scale(TEX, TEX)
    px = np.empty(TEX * TEX * 4, np.float32)
    img.pixels.foreach_get(px)
    px = px.reshape(TEX, TEX, 4)
    if img.colorspace_settings.name != "sRGB":
        img.colorspace_settings.name = "sRGB"
    save(np.dstack([px[:, :, :3], np.ones((TEX, TEX, 1))]), "assets/%s/tex%d.jpg" % (KEY, k))
    a = None
    if k in cut_layers:
        save(np.dstack([px[:, :, 3:4]] * 3 + [np.ones((TEX, TEX, 1))]), "assets/%s/tex%da.jpg" % (KEY, k))
        a = "tex%da.jpg" % k
    tex_rs.append(("tex%d.jpg" % k, a))
    print("TEX", k, n, "area %.1f" % area_by_img[n], "cut" if a else "")
pal = np.ones((TEX, TEX, 4), np.float32)
for i, c in enumerate(palette[:64]):
    s = TEX // 8
    y, x = (i // 8) * s, (i % 8) * s
    # (the image's first row is at the bottom in Blender)
    pal[TEX - y - s:TEX - y, x:x + s, :3] = [((cc / 12.92) if cc <= 0.04045 else ((cc + 0.055) / 1.055) ** 2.4) ** 0 * cc for cc in c]
save(pal, "assets/%s/tex%d.jpg" % (KEY, PAL))
tex_rs.append(("tex%d.jpg" % PAL, None))
print("PALETTE", len(palette), "colours")

# ---------------------------------------------------------------- the measurements, as Rust
g = lambda v: "%.3f" % v
st = np.linspace(zn, zt, 14)
fuse_rows = []
for z in st:
    s = section(z, 0.03 * L) or (ycg, ycg, 0.1)
    fuse_rows.append((z - zcg, s[0] - ycg, 0.5 * (s[0] + s[1]) - ycg, s[1] - ycg, s[2]))
fins = G[(G[:, 2] > zb0) & (np.abs(G[:, 0]) < max(x0 * 1.2, 0.6))]
fin_tip = fins[np.argmax(fins[:, 1])] - O
stabs = G[(G[:, 2] > zb0) & (np.abs(G[:, 0]) > x0)]
stab_tip = (stabs[np.argmax(stabs[:, 0])] - O) if len(stabs) else np.array([x0 * 2, 0.0, zt - zcg - 1.0])
fin_y0 = (section(fin_tip[2] + zcg) or (ycg + 0.5,))[0] - ycg
front = G[(G[:, 2] < zn + 0.3 * L) & (np.abs(G[:, 0]) < 0.4 * max(sc[2], 0.5))]
cano = front[np.argmax(front[:, 1])] - O
eye = (0.0, cano[1] - 0.025 * L, cano[2] + 0.01 * L)
# Better, from the glass: the front seat is under the front of the canopy
# (the back seat of a tandem is not it), a flight deck's left seat just
# behind its windscreen.
if "glass" in pieces and len(cat("glass")[0]) > 30:
    Gl = cat("glass")[0]
    Gl = Gl[(Gl[:, 2] < zn + 0.45 * L) & (Gl[:, 1] > fuse_bottom(Gl[:, 2]) + 0.25 * (np.interp(Gl[:, 2], ZT, TOP) - fuse_bottom(Gl[:, 2])))]
    if len(Gl) > 30:
        zf = np.percentile(Gl[:, 2], 1)
        ln = np.percentile(Gl[:, 2], 99) - zf
        cab = Gl[Gl[:, 2] < zf + 2.5]
        wide = np.percentile(cab[:, 0], 98) - np.percentile(cab[:, 0], 2) > 2.4
        if wide:
            eye = (-0.6, float(np.median(cab[:, 1])) + 0.1 - O[1], zf + 1.0 - O[2])
        else:
            eye = (0.0, float(np.percentile(cab[:, 1], 98)) - 0.36 - O[1], zf + min(0.5 * ln, 1.2) - O[2])
        print("EYE from glass: wide=%s len=%.2f" % (wide, ln), eye)
# by hand, where the canopy is not glass to find
NUDGE = {"l39": (0.0, 0.15, -0.35)}
key = ARGS[1]
if key in NUDGE:
    eye = tuple(float(a) + b for a, b in zip(eye, NUDGE[key]))
legs_c = [(n, at - O) for n, at in legs]
wheel_r = float(np.clip(0.022 * L, 0.2, 0.65))
mid_rows = [r for r in fuse_rows if abs(r[0]) < 0.3 * L]
half = max(r[4] for r in mid_rows) if mid_rows else sc[2]
top = max(r[1] for r in mid_rows) if mid_rows else 1.0
bot = min(r[3] for r in mid_rows) if mid_rows else -1.0
rs = []
rs.append("// Generated by assets/blender/import_plane.py from the model; measured in the")
rs.append("// game's body frame (+X right, +Y up, -Z forward, origin at the centre of gravity).")
rs.append("use crate::aircraft::{Hole, Part, Tail};")
rs.append("use glam::{DVec3, Vec3};")
rs.append("pub const MODEL: &[u8] = include_bytes!(\"../../../assets/%s.bin.gz\");" % KEY)
rs.append("pub const STILL: &[u8] = include_bytes!(\"../../../assets/aircraft/%s.png\");" % KEY)
rs.append("pub const TEXTURES: &[(&[u8], Option<&[u8]>)] = &[")
for c, a in tex_rs:
    rs.append("    (include_bytes!(\"../../../assets/%s/%s\"), %s)," % (KEY, c, "Some(include_bytes!(\"../../../assets/%s/%s\"))" % (KEY, a) if a else "None"))
rs.append("];")
rs.append("/// Length, span (tip to tip), height.")
rs.append("pub const SIZE: [f32; 3] = [%s, %s, %s];" % (g(L), g(2 * x1), g(float(G[:, 1].max() - ymin))))
rs.append("/// Fuselage stations: z, top, height of the widest point, bottom, half width.")
rs.append("pub const FUSE: [[f32; 5]; %d] = [" % len(fuse_rows))
for r in fuse_rows:
    rs.append("    [%s]," % ", ".join(g(v) for v in r))
rs.append("];")
rs.append("/// Wing: root and tip span stations, leading edge z at the root, sweep (m back per m out),")
rs.append("/// root and tip chords, height at the root, dihedral (m up per m out).")
rs.append("pub const WING: [f32; 8] = [%s];" % ", ".join(g(v) for v in (x0, x1, le0 - zcg, sweep, c0, c1, wy0 - ycg, di)))
rs.append("pub const EYE: Vec3 = Vec3::new(%s, %s, %s);" % tuple(g(v) for v in eye))
rs.append("/// Where each leg's tyres meet the ground (nose, left, right), and their radius.")
rs.append("pub const LEGS: [DVec3; 3] = [%s];" % ", ".join("DVec3::new(%s, %s, %s)" % tuple(g(v) for v in at) for _, at in legs_c))
rs.append("pub const WHEEL_R: f64 = %s;" % g(wheel_r))
rs.append("/// Whether the model has its gear down (otherwise the game lends it the Hornet's).")
rs.append("pub const GEAR_MODELLED: bool = %s;" % ("true" if GEAR_MODELLED else "false"))
fz = fin_tip[2]
rs.append("pub const TAIL: Tail = Tail { fin_y0: %s, fin_h: %s, rudder_y0: %s, rudder_h: %s, stab_root: %s, elev_root: %s, break_z: [%s, %s], fin_base: DVec3::new(%s, %s, %s), fin_tip: DVec3::new(%s, %s, %s), stab_tip: DVec3::new(%s, %s, %s) };" % (
    g(fin_y0), g(max(fin_tip[1] - fin_y0, 0.3)), g(fin_y0), g(max(fin_tip[1] - fin_y0, 0.3) * 0.85), g(x0 * 0.6), g(x0 * 0.5), g(zb0 - zcg), g(zb0 - zcg + 0.1 * L),
    g(abs(fin_tip[0])), g(fin_y0), g(fz - 0.1 * L), g(abs(fin_tip[0])), g(fin_tip[1]), g(fz), g(abs(stab_tip[0])), g(stab_tip[1]), g(stab_tip[2])))
r0, rN = fuse_rows[0], fuse_rows[-1]
r1, r2 = fuse_rows[1], fuse_rows[2]
cont = [((0.0, r0[2], r0[0]), "Nose"), ((0.0, r1[3], r1[0]), "Nose"), ((0.0, r1[1], r1[0]), "Nose")]
for r in fuse_rows[3:-3:2]:
    cont.append(((0.0, r[3], r[0]), "Belly"))
for r in fuse_rows[4:-4:3]:
    cont += [((-r[4], r[2], r[0]), "Side"), ((r[4], r[2], r[0]), "Side"), ((0.0, r[1], r[0]), "Roof")]
rs.append("pub const CONTACTS: &[([f64; 3], Part)] = &[%s];" % ", ".join("([%s, %s, %s], Part::%s)" % (g(p[0]), g(p[1]), g(p[2]), k) for p, k in cont))
tp = [(0.0, fuse_rows[-3][3], fuse_rows[-3][0]), (0.0, fuse_rows[-2][3], fuse_rows[-2][0]), (0.0, rN[2], rN[0]), (0.0, fuse_rows[-2][1], fuse_rows[-2][0])]
rs.append("pub const TAIL_PTS: &[[f64; 3]] = &[%s];" % ", ".join("[%s, %s, %s]" % tuple(g(v) for v in p) for p in tp))
tipz = le0 - zcg + sweep * (x1 - x0) + 0.4 * c1
tipy = wy0 - ycg + di * (x1 - x0)
nose_low = min(r[3] for r in fuse_rows[:4])
rs.append("pub const HARD: [DVec3; 7] = [DVec3::new(0.0, %s, %s), DVec3::new(%s, %s, %s), DVec3::new(%s, %s, %s), DVec3::new(0.0, %s, %s), DVec3::new(%s, %s, %s), DVec3::new(0.0, %s, 0.0), DVec3::new(0.0, %s, %s)];" % (
    g(r0[2]), g(r0[0]), g(-x1), g(tipy), g(tipz), g(x1), g(tipy), g(tipz), g(rN[2]), g(rN[0]), g(abs(fin_tip[0])), g(fin_tip[1]), g(fz), g(bot), g(nose_low), g(fuse_rows[2][0])))
rs.append("pub const HOLE: Hole = Hole { fus: [%s, %s, %s, %s], wing: [%s, %s, %s, %s], stab: [%s, 0.06, %s], fin: [%s, %s, 0.12, 0.05], pods: [[0.0; 3]; 2] };" % (
    g(half), g(0.5 * (top - bot)), g(0.5 * (top + bot)), g(0.4 * half), g(wy0 - ycg), g(di), g(0.06 * c0), g(0.05 * c1), g(stab_tip[1]), g(abs(stab_tip[0])), g(fin_y0), g(max(fin_tip[1] - fin_y0, 0.3))))
rs.append("/// Span stations the ailerons run between, and the fuel tanks.")
rs.append("pub const AILERON: [f32; 2] = [%s, %s];" % (g(x0 + 0.6 * (x1 - x0)), g(x1 * 0.97)))
rs.append("pub const TANK: [f32; 2] = [%s, %s];" % (g(x0), g(x0 + 0.6 * (x1 - x0))))
os.makedirs("src/aircraft/measured", exist_ok=True)
open("src/aircraft/measured/%s.rs" % KEY, "w", encoding="utf-8").write("\n".join(rs) + "\n")
print("GEO legs", [(n, tuple(round(float(v), 2) for v in at)) for n, at in legs_c], "eye", tuple(round(float(v), 2) for v in eye), "fin tip", tuple(round(float(v), 2) for v in fin_tip), "stab tip", tuple(round(float(v), 2) for v in stab_tip))

# ---------------------------------------------------------------- the start screen's picture
try:
    for o in bpy.data.objects:
        o.hide_render = o not in objs
    sc_ = bpy.context.scene
    cam = bpy.data.objects.new("cam", bpy.data.cameras.new("cam"))
    sc_.collection.objects.link(cam)
    c = Vector(((lo + hi) * 0.5).tolist())
    fwd = Vector((0.0, F, 0.0))
    side = Vector((1.0, 0.0, 0.0))
    cam.location = c + (fwd * 0.75 + side * 0.85 + Vector((0, 0, 0.42))).normalized() * L * 1.32
    cam.rotation_euler = (c - cam.location).to_track_quat("-Z", "Y").to_euler()
    cam.data.lens = 46
    sc_.camera = cam
    sc_.render.engine = "BLENDER_WORKBENCH"
    sc_.display.shading.light = "STUDIO"
    sc_.display.shading.color_type = "TEXTURE"
    sc_.display.shading.show_cavity = True
    sc_.render.film_transparent = True
    sc_.render.resolution_x, sc_.render.resolution_y = 680, 392
    sc_.render.image_settings.file_format = "PNG"
    sc_.render.image_settings.color_mode = "RGBA"
    sc_.view_settings.view_transform = "Standard"
    sc_.render.filepath = os.path.abspath("assets/aircraft/%s.png" % KEY)
    bpy.ops.render.render(write_still=True)
    print("STILL ok")
except Exception as e:
    print("STILL FAILED", e)
