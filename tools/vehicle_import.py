# Turns downloaded glTF models (not kept in the repository) into what the
# game loads, with nothing but Python, NumPy and Pillow (no Blender):
#
#   python tools/vehicle_import.py car  <model.glb|scene.gltf> <key> <fwd> [<length m> [<paint r,g,b>]]
#   python tools/vehicle_import.py ped  <walking.glb> <key>
#
# run from the repository's root.
#
# `car` writes assets/vehicles/<key>.bin.gz (src/vehicle.rs reads it): the body
# and its four wheels (front left, front right, rear left, rear right), each
# wheel centred on its hub so the game can spin and steer it. <fwd> is the
# axis the nose points along in the file (+x, -x, +z or -z). Materials become
# the game's own (paint, chrome, rubber, plastic, upholstery, glass) and each
# vertex carries its colour, sampled from the model's textures. Decals and
# lenses with a see-through texture keep only their opaque parts.
#
# `ped` writes assets/vehicles/<key>.bin.gz and assets/vehicles/<key>.jpg (src/pilot.rs):
# a skinned character, its texture, and its first animation baked at 30
# frames a second into one skinning matrix per joint per frame, walked on the
# spot (the forward drift is taken out and measured, so the game can match
# the stride to the speed and the feet do not slide).
#
# Game frame: +X right, +Y up, -Z forward, metres, the origin on the ground.
import gzip
import io
import json
import os
import re
import struct
import sys

import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None

# Game material ids (src/airframe.rs, `mat`), and the byte the file stores.
PAINT, ALU, STEEL, RUBBER, PLASTIC, FABRIC, GLASS, LAMP, BLACK = 0, 2, 3, 4, 5, 6, 7, 8, 10


# ---------------------------------------------------------------- glTF reading
class Gltf:
    def __init__(self, path):
        self.dir = os.path.dirname(os.path.abspath(path))
        data = open(path, "rb").read()
        self.bin = None
        if data[:4] == b"glTF":
            n = struct.unpack("<I", data[12:16])[0]
            self.j = json.loads(data[20:20 + n])
            o = 20 + n
            if o < len(data):
                bl = struct.unpack("<I", data[o:o + 4])[0]
                self.bin = data[o + 8:o + 8 + bl]
        else:
            self.j = json.loads(data)
        self.buffers = []
        for b in self.j.get("buffers", []):
            if "uri" in b:
                self.buffers.append(open(os.path.join(self.dir, b["uri"]), "rb").read())
            else:
                self.buffers.append(self.bin)
        self.images = {}

    def view(self, i):
        v = self.j["bufferViews"][i]
        b = self.buffers[v["buffer"]]
        o = v.get("byteOffset", 0)
        return b[o:o + v["byteLength"]], v.get("byteStride")

    def acc(self, i):
        a = self.j["accessors"][i]
        comps = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}[a["type"]]
        dt = {5120: np.int8, 5121: np.uint8, 5122: np.int16, 5123: np.uint16, 5125: np.uint32, 5126: np.float32}[a["componentType"]]
        n = a["count"]
        if "bufferView" not in a:
            return np.zeros((n, comps), dt)
        raw, stride = self.view(a["bufferView"])
        off = a.get("byteOffset", 0)
        isz = np.dtype(dt).itemsize
        if stride and stride != comps * isz:
            out = np.empty((n, comps), dt)
            for k in range(n):
                s = off + k * stride
                out[k] = np.frombuffer(raw[s:s + comps * isz], dt)
            res = out
        else:
            res = np.frombuffer(raw[off:off + n * comps * isz], dt).reshape(n, comps)
        if a.get("normalized"):
            res = res.astype(np.float32) / float(np.iinfo(dt).max)
        return res

    def image(self, ti):
        """Texture index -> float RGBA array (linear rgb, alpha as stored)."""
        if ti in self.images:
            return self.images[ti]
        tex = self.j["textures"][ti]
        src = tex.get("source")
        if src is None:
            for e in tex.get("extensions", {}).values():
                src = e.get("source", src)
        im = self.j["images"][src]
        if "uri" in im:
            img = Image.open(os.path.join(self.dir, im["uri"]))
        else:
            raw, _ = self.view(im["bufferView"])
            img = Image.open(io.BytesIO(raw))
        img = img.convert("RGBA")
        if max(img.size) > 1024:
            img.thumbnail((1024, 1024))
        a = np.asarray(img).astype(np.float32) / 255.0
        a[..., :3] = srgb_to_lin(a[..., :3])
        self.images[ti] = a
        return a


def srgb_to_lin(c):
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def lin_to_srgb(c):
    c = np.clip(c, 0, 1)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1 / 2.4) - 0.055)


def node_matrix(n):
    if "matrix" in n:
        return np.array(n["matrix"], np.float64).reshape(4, 4).T
    t = np.array(n.get("translation", [0, 0, 0]), np.float64)
    r = n.get("rotation", [0, 0, 0, 1])
    s = np.array(n.get("scale", [1, 1, 1]), np.float64)
    return trs(t, r, s)


def trs(t, q, s):
    x, y, z, w = q
    r = np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])
    m = np.eye(4)
    m[:3, :3] = r * s
    m[:3, 3] = t
    return m


def walk_nodes(g):
    """(node index, global matrix, names of it and its ancestors)."""
    sc = g.j["scenes"][g.j.get("scene", 0)]
    out = []
    stack = [(i, np.eye(4), []) for i in sc["nodes"]]
    while stack:
        i, parent, names = stack.pop()
        n = g.j["nodes"][i]
        m = parent @ node_matrix(n)
        nm = names + [n.get("name", "")]
        out.append((i, m, nm))
        for c in n.get("children", []):
            stack.append((c, m, nm))
    return out


def sample(img, uv):
    h, w = img.shape[:2]
    u = np.mod(uv[:, 0], 1.0) * (w - 1)
    v = np.mod(uv[:, 1], 1.0) * (h - 1)
    return img[v.astype(int), u.astype(int)]


# ---------------------------------------------------------------- materials
def classify(m):
    """A glTF material -> (game material, linear rgba factor, texture index or None, blend)."""
    if m is None:
        return PAINT, np.array([0.6, 0.6, 0.6, 1.0]), None, False
    name = m.get("name", "").lower()
    pbr = m.get("pbrMetallicRoughness", {})
    f = np.array(pbr.get("baseColorFactor", [1, 1, 1, 1]), np.float64)
    tex = pbr.get("baseColorTexture", {}).get("index")
    metal = pbr.get("metallicFactor", 1.0)
    rough = pbr.get("roughnessFactor", 1.0)
    ext = m.get("extensions", {})
    blend = m.get("alphaMode") == "BLEND"
    lum = f[:3].mean()
    if "KHR_materials_transmission" in ext or re.search(r"glass|window|windshield|lens", name) or (blend and tex is None and f[3] < 0.6):
        return GLASS, f, tex, blend
    if re.search(r"tire|tyre|rubber", name):
        return RUBBER, f, tex, blend
    if re.search(r"leather|cloth|fabric|seat|carpet|costura", name):
        return FABRIC, f, tex, blend
    if re.search(r"chrome|exhaust|metal|rim|disc|brake|break", name) and metal > 0.3:
        return STEEL, f, tex, blend
    if re.search(r"light|lamp|emis", name) and "glass" not in name:
        return STEEL, f, tex, blend
    if "KHR_materials_clearcoat" in ext or re.search(r"paint|paaint|body|carpaint|primary", name):
        return PAINT, f, tex, blend
    if metal > 0.6:
        return STEEL, f, tex, blend
    if tex is None and lum < 0.06 and rough > 0.5:
        return BLACK, f, tex, blend
    if rough > 0.55:
        return PLASTIC, f, tex, blend
    return PAINT, f, tex, blend


# ---------------------------------------------------------------- the car
AXES = {"+x": np.array([1, 0, 0.0]), "-x": np.array([-1, 0, 0.0]), "+z": np.array([0, 0, 1.0]), "-z": np.array([0, 0, -1.0])}
WHEEL = re.compile(r"(?<![a-z])(wheel|tire|tyre|rim)(?![a-z])|disc_scale", re.I)
NOT_WHEEL = re.compile(r"caliper|steer", re.I)


def car(path, key, fwd, length=None, paint=None):
    g = Gltf(path)
    # glTF is +Y up; turn the file's forward to -Z.
    f = AXES[fwd]
    right = np.cross(f, [0, 1, 0])
    rot = np.eye(4)
    rot[0, :3], rot[1, :3], rot[2, :3] = right, [0, 1, 0], -f
    parts = []  # (positions, normals, rgba, mat, tris, is wheel)
    mats = [classify(m) for m in g.j.get("materials", [])]
    for i, m, names in walk_nodes(g):
        n = g.j["nodes"][i]
        if "mesh" not in n:
            continue
        wm = rot @ m
        nm = np.linalg.inv(wm[:3, :3]).T
        joined = " ".join(names)
        wheel = bool(WHEEL.search(joined)) and not NOT_WHEEL.search(joined)
        for pr in g.j["meshes"][n["mesh"]]["primitives"]:
            if pr.get("mode", 4) != 4:
                continue
            at = pr["attributes"]
            p = g.acc(at["POSITION"]).astype(np.float64)
            nv = len(p)
            nr = g.acc(at["NORMAL"]).astype(np.float64) if "NORMAL" in at else np.tile([0, 1, 0.0], (nv, 1))
            uv = g.acc(at["TEXCOORD_0"]).astype(np.float64) if "TEXCOORD_0" in at else np.zeros((nv, 2))
            idx = g.acc(pr["indices"]).reshape(-1).astype(np.int64) if "indices" in pr else np.arange(nv)
            gm, fac, tex, blend = mats[pr["material"]] if "material" in pr else classify(None)
            col = np.tile(fac, (nv, 1))
            if tex is not None:
                col = col * sample(g.image(tex), uv)
            if "COLOR_0" in at:
                c0 = g.acc(at["COLOR_0"]).astype(np.float64)
                col[:, :c0.shape[1]] *= c0[:, :4]
            tris = idx.reshape(-1, 3)
            # See-through decals: keep only the triangles that are mostly opaque.
            if blend and gm != GLASS:
                a = col[:, 3][tris].mean(1)
                tris = tris[a > 0.45]
            if len(tris) == 0:
                continue
            P = (np.c_[p, np.ones(nv)] @ wm.T)[:, :3]
            N = nr @ nm.T
            N /= np.linalg.norm(N, axis=1, keepdims=True) + 1e-12
            mname = g.j["materials"][pr["material"]].get("name", "").lower() if "material" in pr else ""
            if paint is not None and gm == PAINT and re.search(r"primary|paint|paaint|carpaint", mname):
                # Recolour the paint, keeping the texture's shading.
                lum = col[:, :3].mean(1, keepdims=True)
                shade = lum / max(float(lum.mean()), 1e-3) if lum.mean() > 0.02 else np.ones_like(lum)
                col[:, :3] = np.clip(np.array(paint) * np.clip(shade, 0.3, 1.6), 0, 1)
            parts.append((P, N, col, gm, tris, wheel, joined))
    allp = np.concatenate([p[0] for p in parts])
    lo, hi = allp.min(0), allp.max(0)
    if length:
        # Files come in centimetres, inches or whatever: scale to the real length.
        k = length / (hi[2] - lo[2])
        parts = [(p[0] * k,) + p[1:] for p in parts]
        allp, lo, hi = allp * k, lo * k, hi * k
    centre = np.array([(lo[0] + hi[0]) / 2, lo[1], (lo[2] + hi[2]) / 2])
    size = hi - lo
    print(f"{key}: {len(allp)} vertices, size {size.round(2)} m (w h l)")
    # The wheels: each wheel piece goes to the corner its centre is at.
    corners = [[] for _ in range(4)]  # FL FR RL RR
    body = []
    for pt in parts:
        P = pt[0] - centre
        pt = (P,) + pt[1:]
        # (Some files name body panels after the wheel they sit by: a wheel
        # piece is wheel sized and out at the side.)
        c = P.mean(0)
        ext = P.max(0) - P.min(0)
        if pt[5] and abs(c[0]) > 0.25 * size[0] and ext.max() < 0.3 * size[2]:
            k = (0 if c[2] < 0 else 2) + (0 if c[0] < 0 else 1)
            corners[k].append(pt + (0,))
        else:
            body.append(pt)
    hubs = []
    for k in range(4):
        if not corners[k]:
            sys.exit(f"{key}: no wheel found at corner {k}; are the wheels named wheel/tire/rim in the file?")
        P = np.concatenate([p[0] for p in corners[k]])
        a, b = P.min(0), P.max(0)
        hub = (a + b) / 2
        r = (b[1] - a[1]) / 2
        hubs.append((hub, r))
        print(f"  wheel {['FL', 'FR', 'RL', 'RR'][k]}: hub {hub.round(3)}, radius {r:.3f}")
    # The parts that swing open or come off: each body piece goes to the part its
    # name and place say it is (or stays with the body), and each part gets a hinge.
    slots = {}
    tagged = []
    for pt in body:
        P = pt[0]
        c = (P.min(0) + P.max(0)) / 2
        crel = np.array([c[0] / size[0], (c[1] - size[1] / 2) / size[1], c[2] / size[2]])
        erel = (P.max(0) - P.min(0)) / size
        slot = part_slot(pt[6], crel, erel)
        pid = 0
        if slot is not None:
            pid = slots.setdefault(slot, len(slots) + 1)
        tagged.append(pt + (pid,))
    body = tagged
    table = []
    for slot, pid in sorted(slots.items(), key=lambda kv: kv[1]):
        P = np.concatenate([p[0] for p in body if p[7] == pid])
        a, b = P.min(0), P.max(0)
        c = (a + b) / 2
        kind, where = slot
        if kind == "door":
            # Hinged on its front edge, swinging out.
            hinge = (1, (c[0], c[1], a[2]), (0, 1, 0), 1.0 if c[0] > 0 else -1.0, 1.15)
        elif kind == "hood":
            # Hinged at the windscreen, the front edge lifting.
            hinge = (2, (0, b[1], b[2]), (1, 0, 0), 1.0, 1.3)
        elif kind == "lid" and (b[1] - a[1]) > (b[2] - a[2]):
            # A tailgate: hinged along its bottom, falling open backwards.
            hinge = (3, (0, a[1], c[2]), (1, 0, 0), 1.0, 1.57)
        elif kind == "lid":
            # An engine cover or boot lid behind the cabin: hinged at its front edge.
            hinge = (4, (0, b[1], a[2]), (1, 0, 0), -1.0, 1.2)
        else:
            # Bolted on: bumpers and mirrors.
            hinge = (5 if kind == "bumper" else 6, tuple(c), (1, 0, 0), 0.0, 0.0)
        name = f"{kind} {where}"
        table.append((name, hinge, a, b))
        print(f"  part {pid}: {name}, {len(P)} vertices, hinge at {np.round(hinge[1], 2)}")
    out = io.BytesIO()
    out.write(b"VEH1")
    out.write(struct.pack("<I", 5))
    meta = {"size": size.tolist(), "parts": len(table)}
    groups = [(body, np.zeros(3), 0.0)] + [(corners[k], hubs[k][0], hubs[k][1]) for k in range(4)]
    for pieces, hub, r in groups:
        P = np.concatenate([p[0] for p in pieces]) - hub
        N = np.concatenate([p[1] for p in pieces])
        C = np.concatenate([p[2] for p in pieces])
        M = np.concatenate([np.full(len(p[0]), p[3]) for p in pieces])
        PID = np.concatenate([np.full(len(p[0]), p[7]) for p in pieces])
        T, base = [], 0
        for p in pieces:
            T.append(p[4] + base)
            base += len(p[0])
        T = np.concatenate(T)
        # Drop vertices no triangle uses.
        used = np.zeros(len(P), bool)
        used[T.reshape(-1)] = True
        remap = np.cumsum(used) - 1
        P, N, C, M, PID, T = P[used], N[used], C[used], M[used], PID[used], remap[T]
        out.write(struct.pack("<3ff", *hub, r))
        out.write(struct.pack("<I", len(P)))
        vb = np.zeros(len(P), dtype=[("p", "<f4", 3), ("n", "i1", 3), ("m", "u1"), ("c", "u1", 4)])
        vb["p"] = P
        vb["n"] = np.clip(np.round(N * 127), -127, 127)
        vb["m"] = M
        rgb = lin_to_srgb(C[:, :3])
        vb["c"][:, :3] = np.round(rgb * 255)
        # (The fourth byte is which part the vertex belongs to, 0 the body.)
        vb["c"][:, 3] = PID
        out.write(vb.tobytes())
        out.write(struct.pack("<I", T.size))
        out.write(T.astype("<u4").tobytes())
    # The parts table: kind, name, hinge point, axis, the way it opens, how far, its box.
    out.write(b"PRT1")
    out.write(struct.pack("<I", len(table)))
    for name, (kind, pivot, axis, sign, most), a, b in table:
        nb = name.encode()
        out.write(struct.pack("<BB", kind, len(nb)) + nb)
        out.write(struct.pack("<3f3fff3f3f", *pivot, *axis, sign, most, *a, *b))
    os.makedirs("assets/vehicles", exist_ok=True)
    dst = f"assets/vehicles/{key}.bin.gz"
    with gzip.open(dst, "wb", 9) as fz:
        fz.write(out.getvalue())
    print(f"  wrote {dst} ({os.path.getsize(dst) / 1e6:.1f} MB)", json.dumps(meta))


def part_slot(name, c, e):
    """Which part a piece is, from its node names, centre and size (both as fractions
    of the car's): (kind, where) or None for the body."""
    n = name.lower()
    side = "left" if c[0] < 0 else "right"
    if "door" in n and abs(c[0]) > 0.25 and e[0] < 0.2:
        rear = re.search(r"door[_ ]?(b[lr]|r[lr]|[lr]r)(?![a-z])|rear|door_[dp]side_r", n)
        return ("door", f"{side} {'rear' if rear else 'front'}")
    if "mirror" in n and abs(c[0]) > 0.3:
        return ("mirror", side)
    if "bumper" in n:
        return ("bumper", "front" if c[2] < 0 else "rear")
    if re.search(r"hood|bonnet|boot|trunk|tailgate|engine.?cover|hatch|lid", n) and abs(c[0]) < 0.15 and e[1] < 0.35:
        return ("hood", "front") if c[2] < 0 else ("lid", "rear")
    return None


# ---------------------------------------------------------------- the pilot
def ped(path, key):
    g = Gltf(path)
    nodes = g.j["nodes"]
    parent = {}
    for i, n in enumerate(nodes):
        for c in n.get("children", []):
            parent[c] = i
    skin = g.j["skins"][0]
    joints = skin["joints"]
    ibm = g.acc(skin["inverseBindMatrices"]).reshape(-1, 4, 4).transpose(0, 2, 1).astype(np.float64)
    mesh_node = next(i for i, n in enumerate(nodes) if "mesh" in n and "skin" in n)
    pr = g.j["meshes"][nodes[mesh_node]["mesh"]]["primitives"][0]
    at = pr["attributes"]
    P = g.acc(at["POSITION"]).astype(np.float64)
    N = g.acc(at["NORMAL"]).astype(np.float64)
    UV = g.acc(at["TEXCOORD_0"]).astype(np.float64)
    J = g.acc(at["JOINTS_0"]).astype(np.int64)
    W = g.acc(at["WEIGHTS_0"]).astype(np.float64)
    W /= W.sum(1, keepdims=True) + 1e-12
    I = g.acc(pr["indices"]).reshape(-1).astype(np.int64)
    # The texture, as it is.
    mat = g.j["materials"][pr["material"]]
    ti = mat["pbrMetallicRoughness"]["baseColorTexture"]["index"]
    im = g.j["images"][g.j["textures"][ti]["source"]]
    raw = open(os.path.join(g.dir, im["uri"]), "rb").read() if "uri" in im else g.view(im["bufferView"])[0]
    img = Image.open(io.BytesIO(raw)).convert("RGB")
    img = img.resize((2048, 2048), Image.LANCZOS)
    os.makedirs("assets/vehicles", exist_ok=True)
    img.save(f"assets/vehicles/{key}.jpg", quality=88)

    # The animation, sampled.
    anim = g.j["animations"][0]
    chans = {}
    t_end = 0.0
    for ch in anim["channels"]:
        smp = anim["samplers"][ch["sampler"]]
        t = g.acc(smp["input"]).reshape(-1).astype(np.float64)
        v = g.acc(smp["output"]).astype(np.float64)
        t_end = max(t_end, t[-1])
        chans[(ch["target"]["node"], ch["target"]["path"])] = (t, v)

    def value(node, path, t, rest):
        if (node, path) not in chans:
            return rest
        ts, vs = chans[(node, path)]
        if t <= ts[0]:
            return vs[0]
        if t >= ts[-1]:
            return vs[-1]
        k = np.searchsorted(ts, t) - 1
        a = (t - ts[k]) / (ts[k + 1] - ts[k])
        if path == "rotation":
            q0, q1 = vs[k], vs[k + 1]
            if np.dot(q0, q1) < 0:
                q1 = -q1
            q = q0 * (1 - a) + q1 * a
            return q / np.linalg.norm(q)
        return vs[k] * (1 - a) + vs[k + 1] * a

    def globals_at(t):
        cache = {}

        def gm(i):
            if i in cache:
                return cache[i]
            n = nodes[i]
            if "matrix" in n:
                local = node_matrix(n)
            else:
                local = trs(value(i, "translation", t, np.array(n.get("translation", [0, 0, 0.0]))),
                            value(i, "rotation", t, np.array(n.get("rotation", [0, 0, 0, 1.0]))),
                            value(i, "scale", t, np.array(n.get("scale", [1, 1, 1.0]))))
            m = gm(parent[i]) @ local if i in parent else local
            cache[i] = m
            return m
        return gm

    fps = 30.0
    nf = max(2, int(round(t_end * fps)))
    # glTF faces +Z; the game faces -Z: turn half way round about Y.
    flip = np.diag([-1.0, 1.0, -1.0, 1.0])
    frames = []
    for k in range(nf):
        gm = globals_at(k / fps)
        frames.append(np.stack([flip @ gm(j) @ ibm[n] for n, j in enumerate(joints)]))
    frames = np.stack(frames)  # nf, nj, 4, 4

    def skinned(fr):
        m = np.einsum("vk,vkij->vij", W, fr[J])
        return np.einsum("vij,vj->vi", m, np.c_[P, np.ones(len(P))])[:, :3]

    # Where the body is, frame by frame (the mean of its vertices, a sample of them).
    sub = slice(None, None, 17)
    Ps, Js, Ws = P[sub], J[sub], W[sub]
    cen = []
    floor = 1e9
    for fr in frames:
        m = np.einsum("vk,vkij->vij", Ws, fr[Js])
        q = np.einsum("vij,vj->vi", m, np.c_[Ps, np.ones(len(Ps))])[:, :3]
        cen.append(q.mean(0))
        floor = min(floor, q[:, 1].min())
    cen = np.array(cen)
    # Centred over the origin, feet on the ground, 1.8 m tall whatever units the file is in.
    for k in range(nf):
        off = np.eye(4)
        off[0, 3], off[1, 3], off[2, 3] = -cen[k][0], -floor, -cen[k][2]
        frames[k] = np.einsum("ij,njk->nik", off, frames[k])
    height = skinned(frames[0])[:, 1].max()
    sc = np.diag([1.8 / height] * 3 + [1.0])
    frames = np.einsum("ij,fnjk->fnik", sc, frames)
    dur = nf / fps
    # How fast it walks: a foot on the ground slides back at the walking speed.
    poses = np.stack([skinned(fr) for fr in frames])  # nf, nv, 3
    feet_y = poses[0][:, 1]
    speeds = []
    for side in (-1, 1):
        sel = (feet_y < 0.1) & (np.sign(poses[0][:, 0]) == side)
        c = poses[:, sel].mean(1)  # nf, 3
        low = c[:, 1] < c[:, 1].min() + 0.02
        v = np.gradient(c[:, 2]) * fps
        speeds.append(np.median(np.abs(v[low])))
    speed = float(np.mean(speeds))
    print(f"{key}: {len(P)} vertices, {len(joints)} joints, {nf} frames ({dur:.2f} s), stride speed {speed:.2f} m/s")
    # The frame with the feet closest together stands in for standing still.
    feet = feet_y < 0.1
    spread = [np.ptp(poses[k][feet][:, 2]) for k in range(nf)]
    still = int(np.argmin(spread))
    height = 1.8
    out = io.BytesIO()
    out.write(b"PED1")
    out.write(struct.pack("<IIIfff", len(P), len(joints), nf, fps, speed, height))
    out.write(struct.pack("<I", still))
    vb = np.zeros(len(P), dtype=[("p", "<f4", 3), ("n", "i1", 3), ("x", "u1"), ("uv", "<f4", 2), ("j", "u1", 4), ("w", "u1", 4)])
    vb["p"] = P
    vb["n"] = np.clip(np.round(N * 127), -127, 127)
    vb["uv"] = UV
    vb["j"] = J
    w = np.round(W * 255)
    w[:, 0] += 255 - w.sum(1)
    vb["w"] = w
    out.write(vb.tobytes())
    out.write(struct.pack("<I", I.size))
    out.write(I.astype("<u4").tobytes())
    out.write(frames[:, :, :3, :].astype("<f4").tobytes())
    dst = f"assets/vehicles/{key}.bin.gz"
    with gzip.open(dst, "wb", 9) as fz:
        fz.write(out.getvalue())
    print(f"  wrote {dst} ({os.path.getsize(dst) / 1e6:.1f} MB), standing frame {still}")


if __name__ == "__main__":
    a = sys.argv[1:]
    if a[0] == "car":
        car(a[1], a[2], a[3], float(a[4]) if len(a) > 4 else None, [float(x) for x in a[5].split(",")] if len(a) > 5 else None)
    elif a[0] == "ped":
        ped(a[1], a[2])
    else:
        sys.exit(__doc__)
