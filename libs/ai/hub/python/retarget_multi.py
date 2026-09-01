# Retarget HY-Motion SMPL-H clips onto a rigged GLB character (UniRig or
# SkinTokens rigs — the chain classifier reads the armature, not bone names).
# VERBATIM the motion campaign's proven DIRECTION-BASED retarget
# (ai_stage/retarget.py on .123; global-delta transfer is WRONG — it
# double-applies the rest pose) + one addition: --in-place strips the
# HORIZONTAL pelvis travel so clips play in place and a game host drives
# movement from its own transform (vertical stays — the jump arc reads).
# Runs under venv_unirig python (bpy 4.2). Usage:
#   python retarget_multi.py <rigged.glb> <out.glb> <clip1=name1.npz>
#       [<clip2=...>] [--in-place]
import sys, os, json
import numpy as np
import bpy
from mathutils import Matrix, Vector, Quaternion

TMPL = os.environ.get(
    "HYMOTION_WOODEN_DIR",
    r"C:\ai\HY-Motion-1.0\scripts\gradio\static\assets\dump_wooden",
)
SMPL_NAMES = json.load(open(os.path.join(TMPL, "joint_names.json")))
KIN = np.fromfile(os.path.join(TMPL, "kintree.bin"), dtype=np.int32)
JT = np.fromfile(os.path.join(TMPL, "j_template.bin"), dtype=np.float32).reshape(-1, 3).astype(np.float64)
NAME2IDX = {n: i for i, n in enumerate(SMPL_NAMES)}

def aa2R(v):
    th = np.linalg.norm(v)
    if th < 1e-9: return np.eye(3)
    a = v / th
    K = np.array([[0,-a[2],a[1]],[a[2],0,-a[0]],[-a[1],a[0],0]])
    return np.eye(3) + np.sin(th)*K + (1-np.cos(th))*(K@K)

def smpl_fk(poses_t, Rh_t, trans_t):
    J = len(KIN)
    R = [None]*J
    P = [None]*J
    # HY-Motion's official `construct_smpl_data_dict` stores the same root
    # rotation twice: once at poses[0:3] and again in Rh.  Rh is a legacy
    # EasyMocap alias, not a second transform.  Multiplying both here used to
    # square the pelvis rotation and poison every descendant direction before
    # retargeting (most visibly, both knees crossed the centre line).  Match
    # the official WoodenMesh/simple_lbs contract: apply joint 0 exactly once.
    R[0] = aa2R(poses_t[0:3])
    P[0] = trans_t.astype(np.float64)
    for i in range(1, J):
        p = KIN[i]
        R[i] = R[p] @ aa2R(poses_t[3*i:3*i+3])
        P[i] = P[p] + R[p] @ (JT[i]-JT[p])
    return R, np.array(P)

def rot_between(a, b):
    a = a/ (np.linalg.norm(a)+1e-12); b = b/(np.linalg.norm(b)+1e-12)
    c = np.cross(a, b); d = float(np.dot(a, b))
    if d > 0.999999: return np.eye(3)
    if d < -0.999999:
        # pick any orthogonal axis
        ax = np.cross(a, [1.0,0,0])
        if np.linalg.norm(ax) < 1e-6: ax = np.cross(a, [0,1.0,0])
        ax /= np.linalg.norm(ax)
        K = np.array([[0,-ax[2],ax[1]],[ax[2],0,-ax[0]],[-ax[1],ax[0],0]])
        return np.eye(3) + 2*(K@K)
    K = np.array([[0,-c[2],c[1]],[c[2],0,-c[0]],[-c[1],c[0],0]])
    return np.eye(3) + K + K@K*(1.0/(1.0+d))

# ---------- import rig ----------
rig_path, out_path = sys.argv[1], sys.argv[2]
clips = []
IN_PLACE = False
for a in sys.argv[3:]:
    if a == "--in-place":
        IN_PLACE = True
        continue
    nm, path = a.split("=", 1)
    clips.append((nm, path))
print("in_place:", IN_PLACE)

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=rig_path)
arm = [o for o in bpy.data.objects if o.type == 'ARMATURE'][0]
bpy.context.view_layer.objects.active = arm
print("armature:", arm.name, "bones:", len(arm.data.bones))

bones = arm.data.bones
bone_names = [b.name for b in bones]
parent = {b.name: (b.parent.name if b.parent else None) for b in bones}
children = {}
for b in bones:
    children.setdefault(parent[b.name], []).append(b.name)
head = {b.name: np.array((arm.matrix_world @ b.matrix_local).to_translation()) for b in bones}
tailp = {b.name: np.array((arm.matrix_world @ Matrix.Translation(b.tail_local)).to_translation()) for b in bones}
roots = children.get(None, [])
root = roots[0]
print("root:", root, "root children:", children.get(root))

# ---------- chain classification (blender z-up) ----------
def chain_down(start):
    ch = [start]
    while True:
        cs = children.get(ch[-1], [])
        if len(cs) != 1: break
        ch.append(cs[0])
    return ch

root_kids = children.get(root, [])
legs, spine_start = [], None
for k in root_kids:
    if head[k][2] < head[root][2] - 1e-6: legs.append(k)
    else: spine_start = k
assert len(legs) == 2 and spine_start, f"unexpected root children {root_kids}"
leg_chains = [chain_down(l) for l in legs]

spine = [spine_start]
while True:
    cs = children.get(spine[-1], [])
    if len(cs) == 1: spine.append(cs[0])
    else: break
chest = spine[-1]
chest_kids = children.get(chest, [])
# classify by chain ENDPOINT: arms end far out in |x|, neck ends high in z
kid_chains = [chain_down(k) for k in chest_kids]
ends = [tailp[ch[-1]] for ch in kid_chains]
xdev = [abs(e[0] - head[chest][0]) for e in ends]
arm_idx = sorted(range(len(kid_chains)), key=lambda i: -xdev[i])[:2]
rest_idx = [i for i in range(len(kid_chains)) if i not in arm_idx]
arm_chains = [kid_chains[i] for i in arm_idx]
neck_chain = []
if rest_idx:
    ni = max(rest_idx, key=lambda i: ends[i][2])
    neck_chain = kid_chains[ni]
assert len(arm_chains) == 2, f"chest kids {chest_kids}"
print("spine:", spine, "\nchest:", chest, "\nlegs:", leg_chains, "\narms:", arm_chains, "\nneck:", neck_chain)

# left = +x in blender after gltf import (gltf +x preserved)
def lr(chains):
    a, b = chains
    return (a, b) if head[a[0]][0] > head[b[0]][0] else (b, a)
Lleg, Rleg = lr(leg_chains)
Larm, Rarm = lr(arm_chains)

# mapping: bone -> (smpl_from, smpl_to, rig_child) direction contract;
# pelvis is handled separately.  Auto-riggers are allowed to create
# unconnected Blender bones: a bone's local +Y/tail direction is then NOT the
# direction from that joint node to its child node.  Retargeting the +Y axis
# made SkinTokens hips/shoulders fold inward even for a clean SMPL pose.  The
# hierarchy edge is the segment whose endpoint we must place, while applying
# its alignment to the complete rest frame preserves the rigger's bone roll.
mapping = {}
def assign_dirs(chain, pairs):
    for i, pr in enumerate(pairs):
        if i < len(chain):
            rig_child = chain[i + 1] if i + 1 < len(chain) else None
            mapping[chain[i]] = (NAME2IDX[pr[0]], NAME2IDX[pr[1]], rig_child)
assign_dirs(Lleg, [("L_Hip","L_Knee"),("L_Knee","L_Ankle"),("L_Ankle","L_Foot"),("L_Ankle","L_Foot")])
assign_dirs(Rleg, [("R_Hip","R_Knee"),("R_Knee","R_Ankle"),("R_Ankle","R_Foot"),("R_Ankle","R_Foot")])
sp = spine
sp_pairs = [("Spine1","Spine2"),("Spine2","Spine3"),("Spine3","Neck")]
if len(sp) >= 3:
    idxs = [round(i*(len(sp)-1)/2) for i in range(3)]
    for pr, ci in zip(sp_pairs, idxs):
        rig_child = sp[ci + 1] if ci + 1 < len(sp) else (neck_chain[0] if neck_chain else None)
        mapping[sp[ci]] = (NAME2IDX[pr[0]], NAME2IDX[pr[1]], rig_child)
else:
    for i in range(len(sp)):
        rig_child = sp[i + 1] if i + 1 < len(sp) else (neck_chain[0] if neck_chain else None)
        mapping[sp[i]] = (NAME2IDX[sp_pairs[i][0]], NAME2IDX[sp_pairs[i][1]], rig_child)
assign_dirs(Larm, [("L_Collar","L_Shoulder"),("L_Shoulder","L_Elbow"),("L_Elbow","L_Wrist"),("L_Wrist","L_Middle1")])
assign_dirs(Rarm, [("R_Collar","R_Shoulder"),("R_Shoulder","R_Elbow"),("R_Elbow","R_Wrist"),("R_Wrist","R_Middle1")])
if neck_chain:
    for i, bn in enumerate(neck_chain):
        rig_child = neck_chain[i + 1] if i + 1 < len(neck_chain) else None
        mapping[bn] = (NAME2IDX["Neck"], NAME2IDX["Head"], rig_child)
print("mapping:")
for bn, (a, b, rig_child) in sorted(mapping.items()):
    print("  ", bn, "->", SMPL_NAMES[a], "->", SMPL_NAMES[b], "via", rig_child or "tail")

# smpl gltf(y-up) -> blender(z-up)
M = np.array([[1,0,0],[0,0,-1],[0,1,0]], dtype=np.float64)

# rig facing from foot direction (blender space, ground plane XY)
fdir = np.zeros(3)
for ch in (Lleg, Rleg):
    ank = ch[2] if len(ch) > 2 else ch[-1]
    ft  = ch[3] if len(ch) > 3 else ch[-1]
    fdir += (tailp[ft] - head[ank])
fdir[2] = 0.0
fdir /= (np.linalg.norm(fdir) + 1e-9)
smpl_fwd_b = M @ np.array([0,0,1.0])  # smpl canonical forward in blender space
smpl_fwd_b[2] = 0; smpl_fwd_b /= np.linalg.norm(smpl_fwd_b)
ang = np.arctan2(fdir[1], fdir[0]) - np.arctan2(smpl_fwd_b[1], smpl_fwd_b[0])
Yaw = np.array([[np.cos(ang),-np.sin(ang),0],[np.sin(ang),np.cos(ang),0],[0,0,1]])
C = Yaw @ M
print("rig fwd:", fdir, "yaw deg:", np.degrees(ang))

# left/right sanity: smpl L_Hip offset mapped into blender vs rig left hip
def flip(ji):
    n = SMPL_NAMES[ji]
    if n.startswith("L_"): return NAME2IDX["R_" + n[2:]]
    if n.startswith("R_"): return NAME2IDX["L_" + n[2:]]
    return ji
lhip_b = C @ (JT[NAME2IDX["L_Hip"]] - JT[NAME2IDX["Pelvis"]])
rig_lhip = head[Lleg[0]] - head[root]
if lhip_b[0] * rig_lhip[0] < 0:
    print("MIRROR DETECTED -> swapping L/R smpl assignment")
    mapping = {bn: (flip(a), flip(b), rig_child) for bn, (a, b, rig_child) in mapping.items()}

# scale: rig height vs smpl height
zs = [head[b][2] for b in bone_names] + [tailp[b][2] for b in bone_names]
rig_h = max(zs) - min(zs)
smpl_h = JT[:,1].max() - JT[:,1].min() + 0.3  # head top fudge
scale = rig_h / 1.75
print("rig_h:", rig_h, "scale:", scale)

# rest matrices (armature space)
ML = {b.name: b.matrix_local.copy() for b in bones}
order = []
def topo(bn):
    order.append(bn)
    for c in children.get(bn, []): topo(c)
for r in roots: topo(r)

scene = bpy.context.scene
scene.render.fps = 30
pose = arm.pose

def np2mat(R, t):
    m = Matrix.Identity(4)
    for i in range(3):
        for jj in range(3): m[i][jj] = R[i][jj]
        m[i][3] = t[i]
    return m

actions = []
for clip_name, npz_path in clips:
    d = np.load(npz_path)
    poses_all, trans_all, Rh_all = d["poses"], d["trans"], d["Rh"]
    T = poses_all.shape[0]
    act = bpy.data.actions.new(clip_name)
    arm.animation_data_create()
    arm.animation_data.action = act
    p0 = trans_all[0].copy()
    root_rest_t = np.array(ML[root].to_translation())
    # Rest direction of the actual hierarchy edge.  For terminal bones there
    # is no child joint to place, so retain the Blender +Y/tail direction.
    rest_dir = {}
    for bn in bone_names:
        mapped = mapping.get(bn)
        rig_child = mapped[2] if mapped else None
        if rig_child:
            rest_dir[bn] = np.array(ML[rig_child].to_translation() - ML[bn].to_translation())
        else:
            rest_dir[bn] = np.array(ML[bn].to_3x3() @ Vector((0,1,0)))
    pelvis_i = NAME2IDX["Pelvis"]
    for t in range(T):
        G, P = smpl_fk(poses_all[t], Rh_all[t], trans_all[t])
        Mpose = {}
        for bn in order:
            pb = pose.bones[bn]
            par = parent[bn]
            Mpar = Mpose[par] if par else Matrix.Identity(4)
            offset = (ML[par].inverted() @ ML[bn]) if par else ML[bn]
            Mhier = Mpar @ offset
            rest_rot = np.array(ML[bn].to_3x3())
            if bn == root:
                # pelvis: orientation delta (starts ~identity) + translation
                delta = C @ np.array(G[pelvis_i]) @ C.T
                want_rot = delta @ rest_rot
                d_tr = (trans_all[t] - p0).copy()
                if IN_PLACE:
                    # SMPL is y-up: x/z are the ground plane. Zero the
                    # horizontal travel, keep the vertical (crouch, jump).
                    d_tr[0] = 0.0
                    d_tr[2] = 0.0
                tvec = C @ (d_tr * scale) + root_rest_t
                Mdes = np2mat(want_rot, tvec)
            elif bn in mapping:
                a, b, _rig_child = mapping[bn]
                v = P[b] - P[a]
                u = C @ v
                R_align = rot_between(rest_dir[bn], u)
                want_rot = R_align @ rest_rot
                tvec = np.array(Mhier.to_translation())
                Mdes = np2mat(want_rot, tvec)
            else:
                Mdes = Mhier
            basis = Mhier.inverted() @ Mdes
            Mpose[bn] = Mdes
            pb.matrix_basis = basis
            q = basis.to_quaternion()
            pb.rotation_mode = 'QUATERNION'
            pb.rotation_quaternion = q
            pb.keyframe_insert("rotation_quaternion", frame=t)
            if bn == root:
                pb.location = basis.to_translation()
                pb.keyframe_insert("location", frame=t)
    # stash to NLA
    tr = arm.animation_data.nla_tracks.new()
    tr.name = clip_name
    tr.strips.new(clip_name, 1, act)
    actions.append(act)
    print("clip done:", clip_name, "frames:", T)

arm.animation_data.action = None
scene.frame_start = 0
scene.frame_end = 1
bpy.ops.object.select_all(action='SELECT')
bpy.ops.export_scene.gltf(filepath=out_path, export_format='GLB',
    export_animations=True, export_animation_mode='NLA_TRACKS',
    export_skins=True, export_yup=True, export_apply=False)
print("EXPORTED", out_path, os.path.getsize(out_path))
print("RETARGET-DONE")
