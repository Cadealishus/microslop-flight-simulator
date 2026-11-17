# Turns the King Air 350 model (a glTF binary, not kept in the repository)
# into what the game loads: assets/ka350.bin.gz (one mesh per moving piece,
# with the hinge each turns about), assets/ka350/tex<N>.jpg (its textures,
# one per layer of the array plane.wgsl samples; <N>a.jpg the alpha of those
# that have one) and assets/aircraft/ka350.png (the start screen's picture).
#
#   blender -b --factory-startup --python assets/blender/ka350.py -- <ka350.glb> assets
#
# Blender's frame here: x right, y forward, z up. The game's body frame: +X
# right, +Y up, -Z forward, the origin at the centre of gravity (CY, CZ).
# What each piece is to the game (group, role, how it moves) is set by name in
# src/aircraft/ka350/mod.rs.
import gzip
import os
import struct
import sys

import bpy
import numpy as np
from mathutils import Vector

GLB, OUT = sys.argv[sys.argv.index("--") + 1:][:2]
CY, CZ = 1.25, -0.15
TEX = 2048

# Texture layers, by the name of the material's base colour image.
LAYERS = ["kingair_sign_s", "h_parts1", "h_lg", "h_ext_details_1", "h_int_cabine", "h_int_cockpit", "h_cockpit_1", "h_cockpit_2", "h_int_door_high", "h_lights"]
ALPHA = {3, 9}
# Styles (the high nibble of a vertex's last byte): 0 glossy paint, 1 satin
# metal, 2 interior, 3 decal (cut out by alpha), 4 glass, 5 light lens.
MATS = [
    ("vehicle_generic_smallspecmap", 0, 0), ("h_parts1", 1, 1), ("h_lg", 2, 1), ("Material.009", 3, 3), ("Material.003", 3, 3),
    ("h_int_cabine", 4, 2), ("h_int_cockpit", 5, 2), ("h_cockpit_1", 6, 2), ("h_cockpit_2", 7, 2), ("h_int_door_high", 8, 2),
    ("h_lights", 9, 5), ("window_ext", 0, 4),
]


def game(v):
    return (v[0], v[2] - CZ, -(v[1] - CY))


def gdir(v):
    return (v[0], v[2], -v[1])


bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=GLB)
D = bpy.data.objects
ARM = D["ka350"]


def bone(name):
    b = ARM.data.bones[name]
    m = ARM.matrix_world @ b.matrix_local
    return m.translation.copy(), m.col[0].xyz.copy(), m.col[1].xyz.copy(), m.col[2].xyz.copy()


def style_of(mat):
    for prefix, layer, style in MATS:
        if mat and mat.name.startswith(prefix):
            return style * 16 + layer
    return None


def tris(name):
    """Every triangle corner of an object: position, normal (Blender frame), uv, style byte."""
    o = D[name]
    me = o.data
    me.calc_loop_triangles()
    nl, nt = len(me.loops), len(me.loop_triangles)
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
    st = np.array([style_of(m) if style_of(m) is not None else 255 for m in me.materials] or [255], np.int32)
    S = np.repeat(st[np.clip(tm, 0, len(st) - 1)], 3)
    P, N, U = P[tl], N[tl], uv.reshape(-1, 2)[tl]
    keep = S != 255
    return P[keep], N[keep], U[keep], S[keep]


def cat(parts):
    parts = [p for p in parts if len(p[0])]
    if not parts:
        return (np.zeros((0, 3), np.float32), np.zeros((0, 3), np.float32), np.zeros((0, 2), np.float32), np.zeros(0, np.int32))
    return tuple(np.concatenate([p[k] for p in parts]) for k in range(4))


def pick(t, mask_fn):
    """The triangles of `t` whose centroid passes `mask_fn(cx, cy, cz)`."""
    P = t[0].reshape(-1, 3, 3)
    c = P.mean(axis=1)
    m = np.repeat(mask_fn(c[:, 0], c[:, 1], c[:, 2]), 3)
    return tuple(a[m] for a in t)


pieces = []  # (name, pivot, axis, angle, tris): pivot and axis in the game's frame


def add(name, t, pivot=(0, 0, 0), axis=(1, 0, 0), angle=0.0):
    if len(t[0]) == 0:
        print("EMPTY", name)
        return
    a = Vector(axis).normalized()
    pieces.append((name, tuple(pivot), tuple(a), angle, t))


# ---- the fixed airframe, cut into the groups the damage model breaks it into
def body_region(x, y, z):
    r = np.full(len(x), 2)                      # fuselage
    r[y > 4.2] = 1                              # nose section
    r[y > 5.0] = 0                              # radome
    r[y <= -3.6] = 3                            # tail cone
    r[(y <= -3.6) & (z > 1.0)] = 4              # fin and tailplane
    wing = (np.abs(x) > 0.95) & (y > -3.0)
    r[wing & (x < 0)] = 5
    r[wing & (x > 0)] = 6
    return r


REGIONS = ["radome", "nose", "fuselage", "tail cone", "tail", "left wing", "right wing"]
body = cat([tris(n) for n in ["bodyshell", "chassis.001", "door_dside_f.001", "chassis.002", "beacons", "extra_7", "extra_9"]])
wing = cat([tris(n) for n in ["wing_l", "wing_r", "eng", "chassis", "chassis.003", "navlights", "strobes", "extra_4", "extralight_1", "extralight_2"]])
wing_tail = pick(wing, lambda x, y, z: y < -5.0)
wing = pick(wing, lambda x, y, z: y >= -5.0)
for k, name in enumerate(REGIONS):
    parts = [pick(body, lambda x, y, z, k=k: body_region(x, y, z) == k)]
    if name == "tail":
        parts += [tris("tail"), wing_tail]
    if name == "left wing":
        parts.append(pick(wing, lambda x, y, z: x < 0))
    if name == "right wing":
        parts.append(pick(wing, lambda x, y, z: x >= 0))
    add(name, cat(parts))
add("interior", cat([tris(n) for n in ["cabin1", "cabin2.002", "cabin2.003", "cock1", "cock2", "cock2.002", "cock4.002", "steeringwheel", "door_dside_f", "door_pside_f"]]))
add("glass", tris("windscreen"))

# ---- control surfaces: hinged on their bones
for side, s in (("left", "l"), ("right", "r")):
    h, bx, by, bz = bone("aileron_" + s)
    add(side + " aileron", tris("aileron_" + s), game(h), gdir(bx if bx.x > 0 else -bx))
    h, bx, by, bz = bone("elevator_" + s)
    add(side + " elevator", tris("elevator_" + s), game(h), gdir(bx if bx.x > 0 else -bx))
h, bx, by, bz = bone("rudder")
add("rudder", tris("rudder"), game(h), gdir(bz if bz.z > 0 else -bz))
# The flaps are one mesh with no hinge of its own: each side turns about the
# line through the foremost points of its two ends.
flaps = tris("mod_a")
for side, sg in (("left", -1.0), ("right", 1.0)):
    t = pick(flaps, lambda x, y, z: x * sg > 0)
    P = t[0]
    ax = np.abs(P[:, 0])
    ends = []
    for near in (ax < ax.min() + 0.3, ax > ax.max() - 0.3):
        q = P[near]
        ends.append(Vector(q[np.argmax(q[:, 1])].tolist()))
    d = ends[1] - ends[0]
    add(side + " flap", t, game(ends[0]), gdir(d if d.x > 0 else -d))
    print("FLAP", side, [tuple(round(c, 3) for c in game(e)) for e in ends])

# ---- propellers
for side, n in (("left", "1"), ("right", "2")):
    h, bx, by, bz = bone("prop_" + n)
    add(side + " prop", tris("prop_%s_slow" % n), game(h), gdir(-by))
    add(side + " spinner", tris("prop_%s_fast" % n), game(h), gdir(-by))
    P = tris("prop_%s_slow" % n)[0]
    print("PROP", side, "hub", tuple(round(c, 3) for c in game(h)), "radius", round(float(np.max(np.linalg.norm(P[:, [0, 2]] - np.array([h.x, h.z]), axis=1))), 3))


# ---- landing gear: modelled down and locked, the doors open
def centre(t):
    lo, hi = t[0].min(axis=0), t[0].max(axis=0)
    return Vector(((lo + hi) * 0.5).tolist()), lo, hi


h, bx, by, bz = bone("gear_f")
add("nose leg", cat([tris("gear_f"), tris("headlight_l")]), game(h), (1, 0, 0), -1.65)
h, bx, by, bz = bone("steering")
add("nose fork", tris("steering"), game(h), gdir(bz if bz.z > 0 else -bz))
t = tris("wheel_lf.child")
c, lo, hi = centre(t)
add("nose wheel", t, game(c), (1, 0, 0))
print("WHEEL nose centre", tuple(round(v, 3) for v in game(c)), "radius", round(float(hi[2] - lo[2]) * 0.5, 3))
legs = cat([tris("gear_rl"), tris("gear_lm1"), tris("gear_rm1")])
wheels = tris("wheel_lr.child")
for side, sg in (("left", -1.0), ("right", 1.0)):
    t = pick(legs, lambda x, y, z: x * sg > 0)
    leg = pick(tris("gear_rl"), lambda x, y, z: x * sg > 0)[0]
    top = leg[leg[:, 2] > leg[:, 2].max() - 0.06]
    piv = Vector((float(top[:, 0].mean()), float(top[:, 1].mean()), float(leg[:, 2].max())))
    add(side + " main leg", t, game(piv), (1, 0, 0), 1.6)
    w = pick(wheels, lambda x, y, z: x * sg > 0)
    c, lo, hi = centre(w)
    add(side + " main wheel", w, game(c), (1, 0, 0))
    print("WHEEL", side, "centre", tuple(round(v, 3) for v in game(c)), "radius", round(float(hi[2] - lo[2]) * 0.5, 3), "leg pivot", tuple(round(v, 3) for v in game(piv)))
# Doors: hinged along the fuselage, swinging shut toward the middle of their well.
for name, src, well in (("left nose door", "gear_door_fl", 0.0), ("right nose door", "gear_door_fr", 0.0), ("left outer door", "gear_door_rl1", -2.572), ("left inner door", "gear_door_rml", -2.572), ("right inner door", "gear_door_rmr", 2.572), ("right outer door", "gear_door_rr1", 2.572)):
    h, bx, by, bz = bone(src)
    a = Vector(gdir(by))
    a = a if a.z > 0 else -a
    add(name, tris(src), game(h), a, 1.45 if h.x < well else -1.45)

# ---- write the model
VT = np.dtype([("pos", "<f4", 3), ("nrm", "i1", 3), ("sl", "u1"), ("uv", "<f4", 2)])
blob = [b"KA35", struct.pack("<I", len(pieces))]
total = 0
for name, pivot, axis, angle, (P, N, U, S) in pieces:
    Pg = np.stack([P[:, 0], P[:, 2] - CZ, -(P[:, 1] - CY)], axis=1)
    Ng = np.stack([N[:, 0], N[:, 2], -N[:, 1]], axis=1)
    Ng /= np.maximum(np.linalg.norm(Ng, axis=1, keepdims=True), 1e-9)
    n8 = np.clip(np.round(Ng * 127), -127, 127).astype(np.int64)
    # (the image's first row is at v = 1 in Blender, at v = 0 in the game)
    Ug = np.stack([U[:, 0], 1.0 - U[:, 1]], axis=1)
    key = np.concatenate([np.round(Pg * 1e4).astype(np.int64), n8, np.round(Ug * 1e4).astype(np.int64), S.astype(np.int64)[:, None]], axis=1)
    _, first, inv = np.unique(key, axis=0, return_index=True, return_inverse=True)
    v = np.zeros(len(first), VT)
    v["pos"], v["nrm"], v["sl"], v["uv"] = Pg[first], n8[first], S[first], Ug[first]
    idx = inv.reshape(-1).astype("<u4")
    nb = name.encode()
    blob += [struct.pack("<I", len(nb)), nb, struct.pack("<7f", *pivot, *axis, angle), struct.pack("<I", len(v)), v.tobytes(), struct.pack("<I", len(idx)), idx.tobytes()]
    total += len(v)
    lo, hi = Pg.min(axis=0), Pg.max(axis=0)
    print("PIECE %-18s v=%6d t=%6d lo=(%.2f %.2f %.2f) hi=(%.2f %.2f %.2f) uv=[%.2f %.2f]..[%.2f %.2f]" % (name, len(v), len(idx) // 3, *lo, *hi, *Ug.min(axis=0), *Ug.max(axis=0)))
raw = b"".join(blob)
with open(os.path.join(OUT, "ka350.bin.gz"), "wb") as f:
    f.write(gzip.compress(raw, 9, mtime=0))
print("wrote ka350.bin.gz:", len(pieces), "pieces,", total, "vertices,", len(raw), "bytes raw")

# ---- measurements for the flight and damage models (game frame)
sk = cat([tris("wing_r"), tris("aileron_r"), pick(flaps, lambda x, y, z: x > 0)])[0]
for s in (1.0, 2.0, 3.5, 4.5, 5.5, 6.5, 7.5, 8.3, 8.7):
    q = sk[np.abs(sk[:, 0] - s) < 0.12]
    if len(q):
        print("WING x=%.1f le_z=%.2f te_z=%.2f y=%.2f..%.2f" % (s, -(q[:, 1].max() - CY), -(q[:, 1].min() - CY), q[:, 2].min() - CZ, q[:, 2].max() - CZ))
bs = tris("bodyshell")[0]
for y in (5.8, 5.0, 4.2, 3.0, 1.25, 0.0, -1.5, -3.0, -4.0, -5.0, -6.0, -6.8):
    q = bs[(np.abs(bs[:, 1] - y) < 0.12) & (bs[:, 2] < 1.0)]
    if len(q):
        print("FUSE z=%.2f half=%.2f top=%.2f bot=%.2f" % (-(y - CY), np.abs(q[:, 0]).max(), q[:, 2].max() - CZ, q[:, 2].min() - CZ))
tl = tris("tail")[0]
fin = tl[np.abs(tl[:, 0]) < 0.12]
for z in (0.8, 1.5, 2.2, 2.6):
    q = fin[np.abs(fin[:, 2] - z) < 0.12]
    if len(q):
        print("FIN y=%.2f z=%.2f..%.2f" % (z - CZ, -(q[:, 1].max() - CY), -(q[:, 1].min() - CY)))
st = tl[np.abs(tl[:, 0]) > 0.3]
print("STAB x<=%.2f y=%.2f..%.2f z=%.2f..%.2f" % (np.abs(st[:, 0]).max(), st[:, 2].min() - CZ, st[:, 2].max() - CZ, -(st[:, 1].max() - CY), -(st[:, 1].min() - CY)))

# ---- textures
os.makedirs(os.path.join(OUT, "ka350"), exist_ok=True)
for k, base in enumerate(LAYERS):
    cands = [i for i in bpy.data.images if i.name.split(".")[0] == base and i.size[0] > 8]
    cands.sort(key=lambda i: -i.depth)
    img = cands[0]
    img.scale(TEX, TEX)
    px = np.empty(TEX * TEX * 4, np.float32)
    img.pixels.foreach_get(px)
    px = px.reshape(TEX, TEX, 4)
    for suffix, data in (("", np.dstack([px[:, :, :3], np.ones((TEX, TEX, 1), np.float32)])), ("a", np.dstack([px[:, :, 3:4]] * 3 + [np.ones((TEX, TEX, 1), np.float32)]))):
        if suffix and k not in ALPHA:
            continue
        o = bpy.data.images.new("out%d%s" % (k, suffix), TEX, TEX, alpha=False)
        # (alpha is data, not a colour: kept from the sRGB curve the colour gets on saving)
        o.colorspace_settings.name = img.colorspace_settings.name if not suffix else "sRGB"
        o.pixels.foreach_set(data.reshape(-1).astype(np.float32))
        o.filepath_raw = os.path.join(OUT, "ka350", "tex%d%s.jpg" % (k, suffix))
        o.file_format = "JPEG"
        o.save()
    print("TEX", k, base, "from", img.name, "depth", img.depth, "alpha", float(px[:, :, 3].min()), float(px[:, :, 3].max()))

# ---- the start screen's picture
try:
    used = {"bodyshell", "wing_l", "wing_r", "eng", "tail", "aileron_l", "aileron_r", "elevator_l", "elevator_r", "rudder", "mod_a", "prop_1_slow", "prop_2_slow", "gear_f", "steering", "wheel_lf.child", "gear_rl", "wheel_lr.child", "windscreen", "gear_door_fl", "gear_door_fr", "gear_door_rl1", "gear_door_rml", "gear_door_rmr", "gear_door_rr1"}
    for o in D:
        o.hide_render = o.name not in used
    for m in bpy.data.materials:
        m.diffuse_color = (0.03, 0.04, 0.05, 1.0) if m.name.startswith(("window_ext", "h_lg")) else (0.5, 0.5, 0.52, 1.0) if m.name.startswith("h_parts1") else (0.9, 0.9, 0.88, 1.0)
    sc = bpy.context.scene
    cam = bpy.data.objects.new("cam", bpy.data.cameras.new("cam"))
    sc.collection.objects.link(cam)
    cam.location = (13.0, 15.5, 8.0)
    cam.rotation_euler = (Vector((0.0, -1.2, 0.2)) - cam.location).to_track_quat("-Z", "Y").to_euler()
    cam.data.lens = 46
    sc.camera = cam
    sc.render.engine = "BLENDER_WORKBENCH"
    sc.display.shading.light = "STUDIO"
    sc.display.shading.color_type = "MATERIAL"
    sc.display.shading.show_shadows = False
    sc.display.shading.show_cavity = True
    sc.render.film_transparent = True
    sc.render.resolution_x, sc.render.resolution_y = 680, 392
    sc.render.image_settings.file_format = "PNG"
    sc.render.image_settings.color_mode = "RGBA"
    sc.view_settings.view_transform = "Standard"
    sc.render.filepath = os.path.join(OUT, "aircraft", "ka350.png")
    bpy.ops.render.render(write_still=True)
    print("wrote the still")
except Exception as e:  # no display to render with: the picture is kept as it was
    print("STILL FAILED", e)
