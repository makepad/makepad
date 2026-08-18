//! Encode vertex-animation frames as a standard glTF skeleton.
//!
//! Quake MDL stores a full vertex pose per frame. The character player only
//! skins named clips. A coarse rigid-cluster fit with soft weights makes
//! the mesh wobble (each vert is pulled by bones that do not share its
//! motion). The honest conversion is one translation bone per unique
//! vertex: IBM = T(−rest), frame translation = frame position, weight 1.
//! That is exact and still a normal skinned GLB. If the mesh has more
//! unique verts than the engine palette (256), extras share a bone with
//! hard weights — never soft blends.

use makepad_gltf::{
    write_glb_mesh_skinned, GlbAnimChannel, GlbAnimClip, GlbAnimPath, GlbJoint, GlbSkinnedMesh,
};

const FPS: f32 = 10.0;
/// Same cap as `is_playable_skin` / JOINTS_0 u8.
const MAX_JOINTS: usize = 256;

#[derive(Clone, Debug)]
pub struct NamedClip {
    pub name: String,
    pub frames: Vec<usize>,
}

/// Strip a Quake frame label down to its action stem (`stand1` → `stand`,
/// `prowl_14` → `prowl`, `runb3` → `runb`).
pub fn clip_stem(name: &str) -> String {
    let mut s: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    s.make_ascii_lowercase();
    while s
        .chars()
        .last()
        .is_some_and(|c| c.is_ascii_digit() || c == '_')
    {
        s.pop();
    }
    if s.is_empty() {
        "frame".into()
    } else {
        s
    }
}

/// Group frames that share a stem, preserving first-seen order.
pub fn group_named_clips(names: &[String]) -> Vec<NamedClip> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let stem = clip_stem(name);
        if let Some(at) = order.iter().position(|s| s == &stem) {
            groups[at].push(i);
        } else {
            order.push(stem);
            groups.push(vec![i]);
        }
    }
    order
        .into_iter()
        .zip(groups)
        .map(|(name, frames)| NamedClip { name, frames })
        .collect()
}

fn loco_alias(stem: &str) -> Option<&'static str> {
    match stem {
        "stand" | "idle" | "hover" | "wait" | "axstnd" => Some("idle"),
        "walk" | "prowl" | "fly" | "swim" => Some("walk"),
        "run" | "runb" | "axrun" | "rockrun" => Some("run"),
        // Keep leap off the jump slot: MeshView's landing-contact gate
        // rejects a character whose "jump" is a Quake lunge.
        "leap" | "jump" => Some("leap"),
        _ => None,
    }
}

/// Rename the first stand/walk/run groups to the motion-domain clip names
/// the existing character player already looks up. Later groups keep their
/// stem (`smash`, `death`, `pain`, …) so showcase can cycle every state.
pub fn alias_loco_clips(clips: Vec<NamedClip>) -> Vec<NamedClip> {
    let mut used = std::collections::BTreeSet::new();
    let mut out: Vec<NamedClip> = clips
        .into_iter()
        .map(|mut clip| {
            if let Some(alias) = loco_alias(&clip.name) {
                if used.insert(alias) {
                    clip.name = alias.to_string();
                }
            }
            clip
        })
        .collect();
    let rank = |name: &str| match name {
        "idle" => 0,
        "walk" => 1,
        "run" => 2,
        "leap" | "jump" => 3,
        _ => 4,
    };
    out.sort_by(|a, b| rank(&a.name).cmp(&rank(&b.name)).then(a.name.cmp(&b.name)));
    out
}

/// Encode vertex frames as a skinned GLB the existing character player can play.
pub fn write_skinned_from_vertex_frames(
    unique_rest: &[[f32; 3]],
    unique_frames: &[Vec<[f32; 3]>],
    corners: &[usize],
    uvs: &[[f32; 2]],
    indices: &[u32],
    clips: &[NamedClip],
    base_color_png: &[u8],
) -> Result<Vec<u8>, String> {
    if unique_rest.is_empty() || unique_frames.is_empty() || corners.is_empty() {
        return Err("empty vertex-anim mesh".into());
    }
    if uvs.len() != corners.len() {
        return Err("uvs must match expanded corners".into());
    }
    let n_unique = unique_rest.len();
    for (i, frame) in unique_frames.iter().enumerate() {
        if frame.len() != n_unique {
            return Err(format!("frame {i} vert count mismatch"));
        }
    }
    let frames = strip_root_xz(unique_rest, unique_frames);
    // Keep the caller rest (stand/idle), not MDL frame 0 — player.mdl
    // starts on axrun.
    let rest = unique_rest.to_vec();
    let assign = assign_vertex_bones(n_unique, MAX_JOINTS, &rest, &frames);
    let bones = assign.iter().copied().max().unwrap_or(0) + 1;
    let rest_pos: Vec<[f32; 3]> = corners
        .iter()
        .map(|&vi| rest.get(vi).copied().unwrap_or([0.0; 3]))
        .collect();
    let joints_0: Vec<[u16; 4]> = corners
        .iter()
        .map(|&vi| [assign.get(vi).copied().unwrap_or(0) as u16, 0, 0, 0])
        .collect();
    let weights_0 = vec![[1.0f32, 0.0, 0.0, 0.0]; corners.len()];
    let glb_joints: Vec<GlbJoint> = (0..bones)
        .map(|k| {
            let c = bone_rest(&rest, &assign, k);
            GlbJoint::at(&format!("v{k}"), None, c, c)
        })
        .collect();
    let glb_clips = bake_translation_clips(&rest, &frames, &assign, bones, clips);
    let glb = write_glb_mesh_skinned(&GlbSkinnedMesh {
        positions: &rest_pos,
        normals: None,
        uvs: Some(uvs),
        indices,
        joints_0: &joints_0,
        weights_0: &weights_0,
        joints: &glb_joints,
        clips: &glb_clips,
        base_color_png: Some(base_color_png),
    });
    if !glb.starts_with(b"glTF") {
        return Err("skinned GLB write failed".into());
    }
    Ok(glb)
}

/// One bone per unique vertex until the engine palette fills. Overflow
/// verts glue to the primary whose *motion* is closest — never to a
/// stride-index neighbor. Index pairing mixed a raising hand with a hip
/// and the translation-only centroid tore the mesh into spikes.
fn assign_vertex_bones(
    n_unique: usize,
    max_joints: usize,
    rest: &[[f32; 3]],
    frames: &[Vec<[f32; 3]>],
) -> Vec<usize> {
    if n_unique == 0 {
        return Vec::new();
    }
    if n_unique <= max_joints || max_joints == 0 {
        return (0..n_unique).collect();
    }
    let sigs = motion_sigs(n_unique, rest, frames);
    let primaries = farthest_motion_primaries(max_joints, &sigs);
    let mut assign = vec![0usize; n_unique];
    for i in 0..n_unique {
        let mut best = 0usize;
        let mut best_d = f32::MAX;
        for (k, &p) in primaries.iter().enumerate() {
            let d = sig_dist2(&sigs[i], &sigs[p]);
            if d < best_d {
                best_d = d;
                best = k;
            }
        }
        assign[i] = best;
    }
    assign
}

fn motion_sigs(n: usize, rest: &[[f32; 3]], frames: &[Vec<[f32; 3]>]) -> Vec<Vec<f32>> {
    let step = (frames.len() / 64).max(1);
    let mut sigs = vec![Vec::new(); n];
    for i in 0..n {
        let r = rest.get(i).copied().unwrap_or([0.0; 3]);
        // Rest is in the signature so static leftover verts still spread
        // in space instead of collapsing onto bone 0.
        sigs[i].extend_from_slice(&[r[0] * 0.25, r[1] * 0.25, r[2] * 0.25]);
        for frame in frames.iter().step_by(step) {
            let p = frame.get(i).copied().unwrap_or(r);
            sigs[i].extend_from_slice(&[p[0] - r[0], p[1] - r[1], p[2] - r[2]]);
        }
    }
    sigs
}

fn sig_dist2(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let d = x - y;
            d * d
        })
        .sum()
}

fn farthest_motion_primaries(k: usize, sigs: &[Vec<f32>]) -> Vec<usize> {
    let n = sigs.len();
    if n == 0 {
        return Vec::new();
    }
    let k = k.min(n);
    let mut energy = vec![0.0f32; n];
    for (i, sig) in sigs.iter().enumerate() {
        // skip the rest-prefix (3 floats)
        energy[i] = sig.iter().skip(3).map(|x| x * x).sum();
    }
    let mut start = 0usize;
    for i in 1..n {
        if energy[i] > energy[start] {
            start = i;
        }
    }
    let mut chosen = vec![start];
    let mut nearest = vec![f32::MAX; n];
    for i in 0..n {
        nearest[i] = sig_dist2(&sigs[i], &sigs[start]);
    }
    nearest[start] = -1.0;
    while chosen.len() < k {
        let mut best = 0usize;
        let mut best_d = -1.0f32;
        for i in 0..n {
            if nearest[i] > best_d {
                best_d = nearest[i];
                best = i;
            }
        }
        chosen.push(best);
        nearest[best] = -1.0;
        for i in 0..n {
            if nearest[i] < 0.0 {
                continue;
            }
            let d = sig_dist2(&sigs[i], &sigs[best]);
            if d < nearest[i] {
                nearest[i] = d;
            }
        }
    }
    chosen
}

/// Walk cycles often carry a little planar drift. Locomotion already owns
/// travel, so subtract each frame's XZ centroid relative to rest.
fn strip_root_xz(rest: &[[f32; 3]], frames: &[Vec<[f32; 3]>]) -> Vec<Vec<[f32; 3]>> {
    let rc = centroid(rest);
    frames
        .iter()
        .map(|frame| {
            let fc = centroid(frame);
            let dx = fc[0] - rc[0];
            let dz = fc[2] - rc[2];
            frame
                .iter()
                .map(|p| [p[0] - dx, p[1], p[2] - dz])
                .collect()
        })
        .collect()
}

fn centroid(pts: &[[f32; 3]]) -> [f32; 3] {
    if pts.is_empty() {
        return [0.0; 3];
    }
    let n = pts.len() as f32;
    let mut c = [0.0f32; 3];
    for p in pts {
        c[0] += p[0];
        c[1] += p[1];
        c[2] += p[2];
    }
    [c[0] / n, c[1] / n, c[2] / n]
}

fn bone_rest(rest: &[[f32; 3]], assign: &[usize], bone: usize) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    let mut n = 0.0f32;
    for (i, p) in rest.iter().enumerate() {
        if assign.get(i).copied() != Some(bone) {
            continue;
        }
        sum[0] += p[0];
        sum[1] += p[1];
        sum[2] += p[2];
        n += 1.0;
    }
    if n > 0.0 {
        [sum[0] / n, sum[1] / n, sum[2] / n]
    } else {
        [0.0; 3]
    }
}

fn bone_at(frame: &[[f32; 3]], assign: &[usize], bone: usize) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    let mut n = 0.0f32;
    for (i, p) in frame.iter().enumerate() {
        if assign.get(i).copied() != Some(bone) {
            continue;
        }
        sum[0] += p[0];
        sum[1] += p[1];
        sum[2] += p[2];
        n += 1.0;
    }
    if n > 0.0 {
        [sum[0] / n, sum[1] / n, sum[2] / n]
    } else {
        [0.0; 3]
    }
}

fn bake_translation_clips(
    rest: &[[f32; 3]],
    frames: &[Vec<[f32; 3]>],
    assign: &[usize],
    bones: usize,
    clips: &[NamedClip],
) -> Vec<GlbAnimClip> {
    let rest_t: Vec<[f32; 3]> = (0..bones).map(|k| bone_rest(rest, assign, k)).collect();
    clips
        .iter()
        .filter(|c| !c.frames.is_empty())
        .map(|clip| {
            let n = clip.frames.len();
            let mut times = Vec::with_capacity(n + 1);
            for i in 0..n {
                times.push(i as f32 / FPS);
            }
            if n > 1 {
                times.push(n as f32 / FPS);
            }
            let mut channels = Vec::with_capacity(bones);
            for k in 0..bones {
                let mut tvals = Vec::new();
                for (i, &fi) in clip.frames.iter().enumerate() {
                    let t = frames
                        .get(fi)
                        .map(|f| bone_at(f, assign, k))
                        .unwrap_or(rest_t[k]);
                    tvals.extend_from_slice(&t);
                    if i + 1 == n && n > 1 {
                        tvals.extend_from_slice(&t);
                    }
                }
                channels.push(GlbAnimChannel {
                    joint: k,
                    path: GlbAnimPath::Translation,
                    times: times.clone(),
                    values: tvals,
                });
            }
            GlbAnimClip {
                name: clip.name.clone(),
                channels,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_render::skin::{PoseBuffer, SkinnedModel, SKIN_VERTEX_FLOATS};

    #[test]
    fn stems_and_aliases_match_quake_vocab() {
        let names = [
            "stand1", "stand2", "walk1", "walk2", "run1", "smash1", "smash2", "prowl_1", "prowl_2",
            "hover1", "fly1", "leap1",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        let clips = alias_loco_clips(group_named_clips(&names));
        let names: Vec<_> = clips.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"idle"), "{names:?}");
        assert!(names.contains(&"walk"), "{names:?}");
        assert!(names.contains(&"run"), "{names:?}");
        assert!(names.contains(&"smash"), "{names:?}");
        assert!(names.contains(&"leap"), "{names:?}");
        assert!(!names.contains(&"jump"), "leap must not become jump: {names:?}");
        let idle = clips.iter().find(|c| c.name == "idle").unwrap();
        assert_eq!(idle.frames, vec![0, 1]);
        let walk = clips.iter().find(|c| c.name == "walk").unwrap();
        assert_eq!(walk.frames, vec![2, 3]);
        assert!(names.contains(&"prowl"), "second walk-like stem keeps its name: {names:?}");
    }

    #[test]
    fn per_vertex_bones_replay_the_authored_positions() {
        // Left verts stay, right verts lift 0.4 on the walk frame.
        let rest = vec![
            [0.0, 0.0, 0.0],
            [0.1, 0.0, 0.0],
            [0.0, 0.2, 0.0],
            [1.0, 0.0, 0.0],
            [1.1, 0.0, 0.0],
            [1.0, 0.2, 0.0],
        ];
        let mut up = rest.clone();
        for p in up.iter_mut().skip(3) {
            p[1] += 0.4;
        }
        let frames = vec![rest.clone(), up.clone()];
        let corners: Vec<usize> = (0..6).collect();
        let uvs = vec![[0.0, 0.0]; 6];
        let indices = vec![0u32, 1, 2, 3, 4, 5];
        let clips = alias_loco_clips(group_named_clips(&[
            "stand1".into(),
            "walk1".into(),
        ]));
        let png = crate::classic_import::encode_png_rgba(&[200, 200, 200, 255], 1, 1).unwrap();
        let glb = write_skinned_from_vertex_frames(
            &rest, &frames, &corners, &uvs, &indices, &clips, &png,
        )
        .expect("write");
        let model = SkinnedModel::parse_glb(&glb).expect("engine parse");
        assert_eq!(model.joint_count(), 6);
        let walk = model.clip_index_any(&["walk"]).unwrap();
        let mut pose = PoseBuffer::new();
        let mut pal = Vec::new();
        let mut packed = Vec::new();
        model.sample_clip(walk, 0.0, &mut pose);
        model.palette(&pose, &mut pal);
        model.skin_to_packed(&pal, &mut packed);
        for i in 0..6 {
            let o = i * SKIN_VERTEX_FLOATS;
            let got = [packed[o], packed[o + 1], packed[o + 2]];
            let want = up[i];
            let err = (got[0] - want[0]).abs()
                + (got[1] - want[1]).abs()
                + (got[2] - want[2]).abs();
            assert!(
                err < 1.0e-4,
                "vert {i} got {got:?} want {want:?} err={err}"
            );
        }
    }

    #[test]
    fn overflow_bones_glue_to_similar_motion_not_index() {
        // 6 verts, 4-bone cap. Left trio stays; right trio lifts.
        // Stride-index pairing would mix a left vert onto a right bone.
        let rest: Vec<[f32; 3]> = (0..6)
            .map(|i| [i as f32 * 0.1, 0.0, 0.0])
            .collect();
        let mut up = rest.clone();
        for p in up.iter_mut().skip(3) {
            p[1] += 1.0;
        }
        let frames = vec![rest.clone(), up];
        let assign = assign_vertex_bones(6, 4, &rest, &frames);
        assert_eq!(assign.len(), 6);
        assert!(assign.iter().copied().max().unwrap() < 4);
        let mut left_bones = std::collections::BTreeSet::new();
        let mut right_bones = std::collections::BTreeSet::new();
        for i in 0..3 {
            left_bones.insert(assign[i]);
        }
        for i in 3..6 {
            right_bones.insert(assign[i]);
        }
        assert!(
            left_bones.is_disjoint(&right_bones),
            "left {left_bones:?} mixed with right {right_bones:?} assign={assign:?}"
        );
    }
}
