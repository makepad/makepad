# Rig, animation and shape-key operations

These objects are entries in `model.apply.operations`. Unknown/duplicate fields
fail. Names obey document limits. Joint indices are zero-based u32 JSON integers;
mesh vertex IDs are canonical positive decimal strings. A transform is
`{translation?:[x,y,z],rotation?:[x,y,z,w],scale?:[x,y,z]}` with defaults zero,
identity quaternion, and one. Quaternions must be normalized and scales nonzero.
All numeric input must be finite. Rotation angles are radians.

- `skeleton {joints:[{name,parent,translation}]}` creates/replaces the skeleton.
  Exactly one root has parent:null; other parents are earlier joint ordinals.
  Translation is parent-local XYZ. Preserve weighted joint identities on replacement.
- `clip {name,channels:[{joint,path,keys:[{time,value}]}]}` creates/replaces a clip.
  Path is `translation|rotation|scale`; time is increasing seconds. Values always
  have four lanes: XYZ plus0 for translation/scale, normalized XYZW for rotation.
  Keys replace local TRS components, so translation keys include the rest offset.
  Use names `idle` and `walk` for automatic character gait selection.
- `delete_clip {name}` removes a clip and its attached options.
- `rig_rest {joint,transform}` sets the joint's full local rest transform.
- `rig_rename {joint,name}` changes a joint name, retaining its index.
- `rig_parent {joint,parent}` changes the parent index; null makes a root.
  Parents must precede children. This keeps the local rest unchanged.
- `rig_mirror {joints:[indices],axis,names:[{joint,name}]}` duplicates selected
  joints in parent-first order, with one explicit unique new name per joint. Axis
  is X/Y/Z as0/1/2. It mirrors full world rests and maps selected parents to
  duplicated parents. Existing joints/weights retain their indices. Local shear
  is refused; include the parent chain or remove nonuniform parent scale. The
  single skeleton root cannot be duplicated: select limbs/subtrees.
- `rig_roll {joint,angle}` rotates the local rest about its local Y axis.
- `pose {name,joints:[{joint,transform}]}` creates/replaces a sparse named FK pose.
  Omitted joints inherit their local rests. Joint rows must be unique.
- `delete_pose {name}` removes an existing pose.
- `constraint {name,joint,enabled?,kind}` creates/replaces a constraint. Enabled
  defaults true. Kind is one of:
  - `{type:"copy",target,translation?,rotation?,scale?,influence?}`: local-space
    copy; defaults true,true,false,1. Influence is0..1.
  - `{type:"aim",target,axis?,influence?}`: aim local axis(default[0,1,0]) at the
    target joint in global space. Influence defaults1.
  - `{type:"limit",min_translation,max_translation,min_scale,max_scale,max_angle}`:
    vector bounds and angular bound relative to rest, max_angle0..pi. Scale
    minimum must be positive and each maximum at least its minimum.
  - `{type:"ik",middle,end,target,pole,clamp_reach?}`: joint is chain root;
    middle/end are its direct child/grandchild. Target is another joint index;
    pole is a global position. Clamp-reach defaults false.
  Enabled constraints have one writer per joint and an acyclic dependency graph.
- `delete_constraint {name}` removes an existing constraint.
- `solve_pose {pose}` evaluates enabled constraints and replaces that named pose.
- `ik {pose,root,middle,end,target,pole,clamp_reach?}` solves an explicit global
  target position on a named pose. Analytic two-bone IK preserves bone lengths;
  unreachable targets fail unless clamp_reach is true. The chain and its
  ancestors require uniform positive scale. Degenerate targets/poles fail.
- `bake_clip {name,poses:[{time,pose}],fps?,solve_constraints?}` samples at fps
  (default30, range1..120) to ordinary TRS channels. Pose keys begin at0 and
  strictly increase; at least two are required. Constraint solving defaults true.
- `blend_clip {name,a,b,weight,fps?}` blends two existing clips at weight0..1 into
  a sampled clip, normalizing their timelines to the longer duration. Fps
  defaults30. Source clips remain unchanged.
- `retarget_clip {name,source,joints:[{source,target}],translation_scale?}` maps
  every animated joint into the current skeleton. Source rows must be unique.
  Translation scale is positive and defaults1. This is an explicit joint mapping;
  it does not infer names or anatomical correspondence.
- `clip_options {name,interpolation?,root_motion?,events?,morph_keys?}` attaches
  authored animation metadata to an existing clip. Interpolation is
  `linear|step|cubic` (default linear); cubic uses smoothstep easing between keys.
  Root-motion flag defaults false. Events are ordered
  `{time,name,payload?}` rows (payload defaults empty string, max4096bytes).
  Morph keys are `[{morph,keys:[{time,weight}]}]`, with unique target names and
  strictly increasing nonnegative times. Times must fit the clip duration.
  Events/root-motion metadata require an application consumer to trigger actions.
- `weight_locks {object,joints:[indices]}` replaces the set of locked weight
  groups. Weight tools preserve each existing locked weight and normalize the
  free remainder. Empty joint list unlocks every group.
- `weight_edit {object,vertices:[decimalIDs],edit}` edits a selected set; an empty
  vertex list means every vertex. Duplicate IDs fail. Edit is one of:
  - `{type:"assign",joint}` binds every selected vertex rigidly to one joint.
    An existing positive locked influence on another joint rejects the whole
    transaction. Useful for eye/limb children beneath a soft-body rigid frame.
  - `{type:"normalize",max_influences?}` sorts/prunes only unlocked weights and
    normalizes to unit sum. Default maximum is4.
  - `{type:"smooth",iterations?,factor?}` averages connected neighbor weights;
    defaults1 iteration and factor0.5, iterations1..64, factor0..1.
  - `{type:"mirror",axis,joints:[{source,target}],tolerance?}` uses the nearest
    vertex mirrored across X/Y/Z (axis0/1/2). Missing joint-map entries keep their
    joint. Positive tolerance defaults0.001; unmatched vertices fail.
  - `{type:"transfer",source,max_distance}` copies nearest source-vertex weights
    within a positive distance. This operation uses vertex correspondence rather
    than triangle surface interpolation.
  - `{type:"bind",max_influences?,power?}` derives weights from distance to rest
    bone segments, defaults4 influences and inverse-distance power2 (positive,
    at most8). Locks remain exact. An influence cap that cannot fit locked weights
    and the remaining weight mass fails without changing the mesh.
- `morph {name,object,deltas:[{vertex,delta}],weight?}` creates/replaces a sparse
  shape target. Deltas are 3-vectors and IDs unique; default weight0. The engine
  pins the object's exact topology signature when the target is authored.
- `capture_morph {name,object,target,weight?}` captures positional differences
  from a second object with matching topology and element IDs. Weight defaults0.
- `delete_morph {name}` removes an existing shape target.
- `morph_weight {name,weight}` sets a target's current default influence.

Morph weights may range from-2 to2. Topology changes invalidate pinned targets
and fail until those targets are explicitly removed/rebuilt. Rest, pose,
constraint, clip-option, lock and morph data survive source checkpoints directly;
morph topology signatures are serialized explicitly, never reconstructed by
re-executing an authoring operation. Source references are validated on open.
Limits include64 poses,128 constraints,32 morph targets,1024 events per clip,
document joint/keyframe bounds and aggregate memory/transaction/work budgets.
