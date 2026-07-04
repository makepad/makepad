// Port of box3d/benchmark/main.c + box3d/shared/benchmarks.c (+ the CreateHuman
// scaffolding from box3d/shared/human.c, copied from tests/test_determinism.rs).
//
// Run:  cargo run --release -p makepad-box3d --example benchmark -- [-b=<i>] [-r=<n>] [-nc]
//
// Deviations from C:
// - The port is single threaded: the C thread-count sweep collapses to one
//   configuration ("thread count: 1"); -t=/-w= are accepted and ignored.
// - The C harness allocates the per-step min-profile array once for ALL
//   benchmarks (never reset); this port resets it per benchmark so the printed
//   profile summary is per benchmark.
// - The -s .dat / .csv file output is replaced by a printed summary table.
// - b3DestroyMesh/b3DestroyHull are Arc drops.

use std::sync::Arc;

use makepad_box3d::body::*;
use makepad_box3d::hull::{create_cylinder, create_hull, create_rock, make_box_hull, make_offset_box_hull};
use makepad_box3d::id::{BodyId, JointId, NULL_BODY_ID, NULL_JOINT_ID};
use makepad_box3d::joint::*;
use makepad_box3d::math_functions::{
    clamp_int, compute_cos_sin, cross, inv_rotate_vector, make_quat_from_axis_angle, max_int, min_int, mul_add,
    normalize_quat, offset_pos, quat, rotate_vector, sub_pos, pos, vec3, Pos, Quat, Transform, Vec2, Vec3, WorldTransform,
    DEG_TO_RAD, PI,
};
use makepad_box3d::mesh::{create_grid_mesh, create_torus_mesh, create_wave_mesh};
use makepad_box3d::physics_world::*;
use makepad_box3d::shape::{create_capsule_shape, create_hull_shape, create_mesh_shape, create_sphere_shape};
use makepad_box3d::timer::{get_milliseconds, get_ticks};
use makepad_box3d::types::*;

const BENCHMARK_DEBUG: bool = cfg!(debug_assertions);

// ---------------------------------------------------------------------------
// human.c scaffolding (CreateHuman/DestroyHuman — the parts the benchmarks use)
// Copied from tests/test_determinism.rs; verified against the
// benchmarks.c call: CreateHuman(human, worldId, position, frictionTorque,
// hertz, dampingRatio, groupIndex, NULL, false).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum BoneJointType {
    None,
    Revolute,
    Spherical,
}

const BONE_PELVIS: usize = 0;
const BONE_SPINE_01: usize = 1;
const BONE_SPINE_02: usize = 2;
const BONE_SPINE_03: usize = 3;
const BONE_NECK: usize = 4;
const BONE_HEAD: usize = 5;
const BONE_THIGH_L: usize = 6;
const BONE_CALF_L: usize = 7;
const BONE_THIGH_R: usize = 8;
const BONE_CALF_R: usize = 9;
const BONE_UPPER_ARM_L: usize = 10;
const BONE_LOWER_ARM_L: usize = 11;
const BONE_UPPER_ARM_R: usize = 12;
const BONE_LOWER_ARM_R: usize = 13;
const BONE_COUNT: usize = 14;

struct Bone {
    body_id: BodyId,
    joint_id: JointId,
    local_frame_a: Transform,
    local_frame_b: Transform,
    reference_frame: Transform,
    joint_type: BoneJointType,
    swing_limit: f32,
    twist_limit: Vec2,
    joint_friction: f32,
    parent_index: i32,
}

impl Default for Bone {
    fn default() -> Self {
        Bone {
            body_id: NULL_BODY_ID,
            joint_id: NULL_JOINT_ID,
            local_frame_a: Transform::IDENTITY,
            local_frame_b: Transform::IDENTITY,
            reference_frame: Transform::IDENTITY,
            joint_type: BoneJointType::None,
            swing_limit: 0.0,
            twist_limit: Vec2 { x: 0.0, y: 0.0 },
            joint_friction: 1.0,
            parent_index: -1,
        }
    }
}

#[derive(Default)]
struct Human {
    bones: Vec<Bone>,
    filter_joints: Vec<JointId>,
}

fn transform(px: f32, py: f32, pz: f32, qx: f32, qy: f32, qz: f32, qs: f32) -> Transform {
    Transform { p: vec3(px, py, pz), q: quat(vec3(qx, qy, qz), qs) }
}

// Port of CreateHuman (human.c). colorize is always false in the benchmarks so
// the color plumbing is dropped; userData is unused (0).
fn create_human(
    world: &mut World,
    position: Pos,
    friction_torque: f32,
    hertz: f32,
    damping_ratio: f32,
    group_index: i32,
) -> Human {
    let mut human = Human::default();
    for _ in 0..BONE_COUNT {
        human.bones.push(Bone::default());
    }

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;

    let mut shape_def = default_shape_def();
    shape_def.base_material.rolling_resistance = 0.2;

    {
        let bone = &mut human.bones[BONE_PELVIS];
        bone.parent_index = -1;

        body_def.name = "pelvis".to_string();
        bone.reference_frame = transform(0.0, 0.932087, -0.051708, 0.739169, 0.0, 0.0, 0.673520);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        let capsule = Capsule { center1: vec3(0.07, 0.0, -0.08), center2: vec3(-0.07, 0.0, -0.08), radius: 0.13 };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_PELVIS].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_SPINE_01];
        bone.parent_index = BONE_PELVIS as i32;

        body_def.name = "spine_01".to_string();
        bone.reference_frame = transform(0.0, 1.113505, -0.03481, 0.739973, 0.0, 0.0, 0.672637);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Spherical;
        bone.local_frame_a = transform(0.000000, 0.000000, -0.182204, -0.999999, 0.000000, -0.000000, 0.001194);
        bone.local_frame_b = transform(0.000000, 0.000000, -0.007736, -1.000000, 0.000000, -0.000000, 0.000000);
        bone.swing_limit = 25.0 * DEG_TO_RAD;
        bone.twist_limit = Vec2 { x: -15.0 * DEG_TO_RAD, y: 15.0 * DEG_TO_RAD };

        let capsule =
            Capsule { center1: vec3(0.06, -0.0, -0.052264), center2: vec3(-0.06, 0.0, -0.052264), radius: 0.12 };
        shape_def.filter.group_index = -group_index;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_SPINE_01].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_SPINE_02];
        bone.parent_index = BONE_SPINE_01 as i32;

        // C: bodyDef.name assignment is commented out; the previous name sticks.
        bone.reference_frame = transform(0.0, 1.194336, -0.027087, 0.703611, 0.0, 0.0, 0.710586);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Spherical;
        bone.local_frame_a = transform(0.000000, -0.000000, -0.088935, -0.998619, -0.000000, 0.000000, -0.052540);
        bone.local_frame_b = transform(-0.000000, 0.000000, -0.008199, -1.000000, 0.000000, -0.000000, 0.000000);
        bone.swing_limit = 25.0 * DEG_TO_RAD;
        bone.twist_limit = Vec2 { x: -15.0 * DEG_TO_RAD, y: 15.0 * DEG_TO_RAD };

        let capsule = Capsule {
            center1: vec3(0.08, -0.015133, -0.091801),
            center2: vec3(-0.08, -0.015133, -0.091801),
            radius: 0.10,
        };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_SPINE_02].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_SPINE_03];
        bone.parent_index = BONE_SPINE_02 as i32;

        body_def.name = "spine_03".to_string();
        bone.reference_frame = transform(-0.0, 1.31043, -0.028232, 0.669856, 0.000001, -0.000001, 0.742491);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Spherical;
        bone.local_frame_a = transform(-0.000000, 0.000000, -0.124298, -0.998921, 0.000001, -0.000001, -0.046434);
        bone.local_frame_b = transform(0.000000, 0.000000, 0.000000, -1.000000, 0.000000, -0.000001, 0.000000);
        bone.swing_limit = 15.0 * DEG_TO_RAD;
        bone.twist_limit = Vec2 { x: -10.0 * DEG_TO_RAD, y: 10.0 * DEG_TO_RAD };

        let capsule = Capsule {
            center1: vec3(0.11, -0.039753, -0.13),
            center2: vec3(-0.11, -0.039753, -0.13),
            radius: 0.145,
        };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_SPINE_03].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_NECK];
        bone.parent_index = BONE_SPINE_03 as i32;

        body_def.name = "neck".to_string();
        bone.reference_frame = transform(0.0, 1.575582, -0.055837, 0.879922, 0.0, 0.0, 0.475118);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Spherical;
        bone.local_frame_a = transform(0.000001, -0.000259, -0.266585, -0.942192, -0.000001, 0.000000, 0.335074);
        bone.local_frame_b = transform(0.000000, 0.000000, 0.000000, -1.000000, 0.000000, -0.000001, 0.000000);
        bone.swing_limit = 45.0 * DEG_TO_RAD;
        bone.twist_limit = Vec2 { x: -15.0 * DEG_TO_RAD, y: 15.0 * DEG_TO_RAD };
        bone.joint_friction = 0.8;

        let capsule = Capsule {
            center1: vec3(-0.000001, -0.0, -0.02),
            center2: vec3(0.0, -0.005, -0.08),
            radius: 0.07,
        };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_NECK].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_HEAD];
        bone.parent_index = BONE_NECK as i32;

        body_def.name = "head".to_string();
        bone.reference_frame = transform(0.0, 1.653348, -0.003241, 0.750288, 0.0, 0.0, 0.661111);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Spherical;
        bone.local_frame_a = transform(0.000000, 0.001321, -0.093873, -0.974301, -0.000000, -0.000000, -0.225251);
        bone.local_frame_b = transform(0.000000, 0.001268, -0.005104, -1.000000, 0.000000, -0.00000, 0.000000);
        bone.swing_limit = 15.0 * DEG_TO_RAD;
        bone.twist_limit = Vec2 { x: -15.0 * DEG_TO_RAD, y: 15.0 * DEG_TO_RAD };
        bone.joint_friction = 0.4;

        let capsule = Capsule {
            center1: vec3(-0.000001, 0.016892, -0.05869),
            center2: vec3(0.0, -0.003629, -0.115072),
            radius: 0.0975,
        };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_HEAD].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_THIGH_L];
        bone.parent_index = BONE_PELVIS as i32;

        body_def.name = "thigh_l".to_string();
        bone.reference_frame = transform(0.090416, 0.986104, -0.035090, -0.703287, -0.070715, 0.053866, 0.705327);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Spherical;
        bone.local_frame_a = transform(0.05, 0.011537, -0.055325, -0.714896, -0.022305, -0.698361, -0.026790);
        bone.local_frame_b = transform(0.0, 0.0, 0.0, -0.002064, 0.758987, 0.017046, 0.650880);
        bone.swing_limit = 10.0 * DEG_TO_RAD;
        bone.twist_limit = Vec2 { x: -60.0 * DEG_TO_RAD, y: 40.0 * DEG_TO_RAD };

        let capsule = Capsule {
            center1: vec3(0.023719, 0.006008, -0.039068),
            center2: vec3(-0.064492, -0.004664, -0.424718),
            radius: 0.09,
        };
        shape_def.filter.group_index = -group_index;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_THIGH_L].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_CALF_L];
        bone.parent_index = BONE_THIGH_L as i32;

        body_def.name = "calf_l".to_string();
        bone.reference_frame = transform(0.101198, 0.527027, -0.037374, -0.653328, -0.066860, 0.058582, 0.751838);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Revolute;
        bone.local_frame_a = transform(-0.069989, 0.000253, -0.453844, -0.000677, 0.760087, 0.105674, 0.641171);
        bone.local_frame_b = transform(0.0, 0.0, 0.0, -0.044589, 0.765540, 0.053368, 0.639619);
        bone.twist_limit = Vec2 { x: -5.0 * DEG_TO_RAD, y: 45.0 * DEG_TO_RAD };

        let capsule = Capsule {
            center1: vec3(0.001778, 0.0, 0.009841),
            center2: vec3(-0.078577, 0.014707, -0.41816),
            radius: 0.075,
        };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_CALF_L].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_THIGH_R];
        bone.parent_index = BONE_PELVIS as i32;

        body_def.name = "thigh_r".to_string();
        bone.reference_frame = transform(-0.090416, 0.986104, -0.03509, -0.703287, 0.070715, -0.053865, 0.705326);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Spherical;
        bone.local_frame_a = transform(-0.05, 0.011537, -0.055326, -0.039089, -0.714094, 0.043177, 0.697623);
        bone.local_frame_b = transform(0.0, 0.0, 0.0, 0.758805, -0.019886, -0.651012, -0.001759);
        bone.swing_limit = 10.0 * DEG_TO_RAD;
        bone.twist_limit = Vec2 { x: -30.0 * DEG_TO_RAD, y: 60.0 * DEG_TO_RAD };

        let capsule = Capsule {
            center1: vec3(-0.023719, 0.006008, -0.039068),
            center2: vec3(0.064492, -0.004664, -0.424718),
            radius: 0.09,
        };
        shape_def.filter.group_index = -group_index;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_THIGH_R].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_CALF_R];
        bone.parent_index = BONE_THIGH_R as i32;

        body_def.name = "calf_r".to_string();
        bone.reference_frame = transform(-0.101198, 0.527027, -0.037373, -0.653327, 0.06686, -0.058582, 0.751839);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Revolute;
        bone.local_frame_a = transform(0.069988, 0.000253, -0.453844, 0.760086, -0.000675, -0.641171, -0.105676);
        bone.local_frame_b = transform(0.0, 0.0, 0.0, 0.765540, -0.044589, -0.639619, -0.053368);
        bone.twist_limit = Vec2 { x: -45.0 * DEG_TO_RAD, y: 5.0 * DEG_TO_RAD };

        let capsule = Capsule {
            center1: vec3(-0.001820, 0.0, 0.010071),
            center2: vec3(0.077883, 0.014825, -0.418047),
            radius: 0.075,
        };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_CALF_R].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_UPPER_ARM_L];
        bone.parent_index = BONE_SPINE_03 as i32;

        body_def.name = "upper_arm_l".to_string();
        bone.reference_frame = transform(0.20378, 1.484275, -0.115897, 0.143082, 0.695980, -0.690130, 0.13733);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Spherical;
        bone.local_frame_a = transform(0.203780, -0.069369, -0.181921, -0.278486, 0.445600, -0.097014, 0.845266);
        bone.local_frame_b = transform(0.000000, 0.000000, 0.000000, -0.201396, -0.001586, 0.901850, 0.382234);
        bone.swing_limit = 60.0 * DEG_TO_RAD;
        bone.twist_limit = Vec2 { x: -5.0 * DEG_TO_RAD, y: 5.0 * DEG_TO_RAD };

        let capsule = Capsule {
            center1: vec3(0.0, 0.0, 0.0),
            center2: vec3(-0.091118, 0.037775, 0.229719),
            radius: 0.075,
        };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_UPPER_ARM_L].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_LOWER_ARM_L];
        bone.parent_index = BONE_UPPER_ARM_L as i32;

        body_def.name = "lower_arm_l".to_string();
        bone.reference_frame = transform(0.305614, 1.242908, -0.117599, 0.165048, 0.563437, -0.802002, 0.109959);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Revolute;
        bone.local_frame_a = transform(-0.095482, 0.039584, 0.240723, 0.512487, -0.180629, 0.839474, 0.003742);
        bone.local_frame_b = transform(0.0, 0.0, 0.0, 0.503803, -0.029831, 0.858168, 0.094017);
        bone.twist_limit = Vec2 { x: -5.0 * DEG_TO_RAD, y: 60.0 * DEG_TO_RAD };

        let capsule = Capsule {
            center1: vec3(0.0, 0.0, 0.0),
            center2: vec3(-0.142406, 0.039392, 0.261092),
            radius: 0.05,
        };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_LOWER_ARM_L].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_UPPER_ARM_R];
        bone.parent_index = BONE_SPINE_03 as i32;

        body_def.name = "upper_arm_r".to_string();
        bone.reference_frame = transform(-0.20378, 1.484276, -0.115899, 0.143083, -0.695978, 0.690132, 0.137329);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Spherical;
        bone.local_frame_a = transform(-0.203779, -0.069371, -0.181922, -0.253621, -0.414842, 0.106962, 0.867261);
        bone.local_frame_b = transform(0.000000, 0.000000, 0.000000, -0.201397, 0.001587, -0.901850, 0.382233);
        bone.swing_limit = 60.0 * DEG_TO_RAD;
        bone.twist_limit = Vec2 { x: -5.0 * DEG_TO_RAD, y: 5.0 * DEG_TO_RAD };

        let capsule = Capsule {
            center1: vec3(0.0, 0.0, 0.0),
            center2: vec3(0.091118, 0.037775, 0.229718),
            radius: 0.075,
        };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_UPPER_ARM_R].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    {
        let bone = &mut human.bones[BONE_LOWER_ARM_R];
        bone.parent_index = BONE_UPPER_ARM_R as i32;

        body_def.name = "lower_arm_r".to_string();
        bone.reference_frame = transform(-0.305614, 1.242907, -0.117599, 0.165048, -0.563437, 0.802002, 0.109959);
        body_def.rotation = bone.reference_frame.q;
        body_def.position = offset_pos(position, bone.reference_frame.p);
        bone.joint_type = BoneJointType::Revolute;
        bone.local_frame_a = transform(0.095484, 0.039585, 0.240723, -0.180627, 0.512487, -0.003744, -0.839474);
        bone.local_frame_b = transform(0.0, 0.0, 0.0, -0.029831, 0.503803, -0.094017, -0.858169);
        bone.twist_limit = Vec2 { x: -60.0 * DEG_TO_RAD, y: 5.0 * DEG_TO_RAD };

        let capsule = Capsule {
            center1: vec3(0.0, 0.0, 0.0),
            center2: vec3(0.142406, 0.039392, 0.261092),
            radius: 0.05,
        };
        shape_def.filter.group_index = 0;
        let body_id = create_body(world, &body_def);
        human.bones[BONE_LOWER_ARM_R].body_id = body_id;
        create_capsule_shape(world, body_id, &shape_def, &capsule);
    }

    // Create joints
    for i in 1..BONE_COUNT {
        let parent_index = human.bones[i].parent_index as usize;
        let body_id_a = human.bones[parent_index].body_id;
        let body_id_b = human.bones[i].body_id;

        human.bones[i].local_frame_a.q = normalize_quat(human.bones[i].local_frame_a.q);
        human.bones[i].local_frame_b.q = normalize_quat(human.bones[i].local_frame_b.q);

        let bone = &human.bones[i];
        match bone.joint_type {
            BoneJointType::Revolute => {
                let mut joint_def = default_revolute_joint_def();
                joint_def.base.body_id_a = body_id_a;
                joint_def.base.body_id_b = body_id_b;
                joint_def.base.local_frame_a = bone.local_frame_a;
                joint_def.base.local_frame_b = bone.local_frame_b;
                joint_def.enable_limit = true;
                joint_def.lower_angle = bone.twist_limit.x;
                joint_def.upper_angle = bone.twist_limit.y;
                joint_def.enable_spring = hertz > 0.0;
                joint_def.hertz = hertz;
                joint_def.damping_ratio = damping_ratio;
                joint_def.enable_motor = true;
                joint_def.max_motor_torque = bone.joint_friction * friction_torque;
                let joint_id = create_revolute_joint(world, &joint_def);
                human.bones[i].joint_id = joint_id;
            }
            BoneJointType::Spherical => {
                let mut joint_def = default_spherical_joint_def();
                joint_def.base.body_id_a = body_id_a;
                joint_def.base.body_id_b = body_id_b;
                joint_def.base.local_frame_a = bone.local_frame_a;
                joint_def.base.local_frame_b = bone.local_frame_b;
                joint_def.enable_cone_limit = true;
                joint_def.cone_angle = bone.swing_limit;
                joint_def.enable_twist_limit = true;
                joint_def.lower_twist_angle = bone.twist_limit.x;
                joint_def.upper_twist_angle = bone.twist_limit.y;
                joint_def.enable_spring = hertz > 0.0;
                joint_def.hertz = hertz;
                joint_def.damping_ratio = damping_ratio;
                joint_def.enable_motor = true;
                joint_def.max_motor_torque = bone.joint_friction * friction_torque;
                let joint_id = create_spherical_joint(world, &joint_def);
                human.bones[i].joint_id = joint_id;
            }
            BoneJointType::None => {}
        }
    }

    // Disable some collisions
    let mut filter_def = default_filter_joint_def();
    filter_def.base.body_id_a = human.bones[BONE_THIGH_L].body_id;
    filter_def.base.body_id_b = human.bones[BONE_THIGH_R].body_id;
    let filter_joint = create_filter_joint(world, &filter_def);
    human.filter_joints.push(filter_joint);

    human
}

// Port of DestroyHuman (human.c).
fn destroy_human(world: &mut World, human: &mut Human) {
    for joint_id in human.filter_joints.drain(..) {
        destroy_joint(world, joint_id, false);
    }

    for i in 0..human.bones.len() {
        if human.bones[i].joint_id.is_null() {
            continue;
        }
        destroy_joint(world, human.bones[i].joint_id, false);
        human.bones[i].joint_id = NULL_JOINT_ID;
    }

    for i in 0..human.bones.len() {
        if human.bones[i].body_id.is_null() {
            continue;
        }
        destroy_body(world, human.bones[i].body_id);
        human.bones[i].body_id = NULL_BODY_ID;
    }
}

// ---------------------------------------------------------------------------
// benchmarks.c scenarios
// ---------------------------------------------------------------------------

trait Scenario {
    fn capacity(&self, _capacity: &mut Capacity) {}
    fn create(&mut self, world: &mut World);
    // C: stepFcn(worldId, stepCount); a default no-op matches a NULL stepFcn.
    fn step(&mut self, _world: &mut World, _step_index: i32) {}
}

// --- joint_grid ---

struct JointGrid;

impl Scenario for JointGrid {
    fn create(&mut self, world: &mut World) {
        world_enable_sleeping(world, false);

        let n: i32 = if BENCHMARK_DEBUG { 10 } else { 100 };

        let mut bodies: Vec<BodyId> = Vec::with_capacity((n * n) as usize);
        let mut index = 0usize;

        let mut shape_def = default_shape_def();
        shape_def.filter.category_bits = 2;
        // C: maskBits = ~2u (32-bit) zero-extended into the u64 field.
        shape_def.filter.mask_bits = (!2u32) as u64;

        let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.4 };

        let mut joint_def = default_spherical_joint_def();
        let mut body_def = default_body_def();
        body_def.enable_sleep = false;

        for k in 0..n {
            for i in 0..n {
                let fk = k as f32;
                let fi = i as f32;

                body_def.body_type = if i == 0 { BodyType::Static } else { BodyType::Dynamic };

                body_def.position = pos(fk, -fi, 0.0);

                let body = create_body(world, &body_def);

                create_sphere_shape(world, body, &shape_def, &sphere);

                if i > 0 {
                    joint_def.base.body_id_a = bodies[index - 1];
                    joint_def.base.body_id_b = body;
                    joint_def.base.local_frame_a.p = vec3(0.0, -0.5, 0.0);
                    joint_def.base.local_frame_b.p = vec3(0.0, 0.5, 0.0);
                    create_spherical_joint(world, &joint_def);
                }

                if k > 0 {
                    joint_def.base.body_id_a = bodies[index - n as usize];
                    joint_def.base.body_id_b = body;
                    joint_def.base.local_frame_a.p = vec3(0.5, 0.0, 0.0);
                    joint_def.base.local_frame_b.p = vec3(-0.5, 0.0, 0.0);
                    create_spherical_joint(world, &joint_def);
                }

                bodies.push(body);
                index += 1;
            }
        }
    }
}

// --- large_pyramid ---

struct LargePyramid;

impl Scenario for LargePyramid {
    fn create(&mut self, world: &mut World) {
        world_enable_sleeping(world, false);

        let base_count: i32 = if BENCHMARK_DEBUG { 20 } else { 90 };

        {
            let mut body_def = default_body_def();
            body_def.position = pos(0.0, -1.0, 0.0);
            let ground_id = create_body(world, &body_def);

            let box_hull = make_box_hull(400.0, 1.0, 400.0);
            let shape_def = default_shape_def();
            create_hull_shape(world, ground_id, &shape_def, &box_hull);
        }

        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;

        let mut shape_def = default_shape_def();
        shape_def.density = 100.0;

        let h = 0.5f32;
        let box_hull = make_box_hull(h, h, h);

        let shift = 1.0 * h;

        for i in 0..base_count {
            let y = (2.0 * i as f32 + 1.0) * shift;

            for j in i..base_count {
                let x = (i as f32 + 1.0) * shift + 2.0 * (j - i) as f32 * shift - h * base_count as f32;

                body_def.position = pos(x, y, 0.0);

                let body_id = create_body(world, &body_def);
                create_hull_shape(world, body_id, &shape_def, &box_hull);
            }
        }
    }
}

// --- many_pyramids ---

fn create_small_pyramid(world: &mut World, base_count: i32, extent: f32, center_x: f32, base_z: f32) {
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.enable_sleep = false;

    let mut shape_def = default_shape_def();
    shape_def.density = 100.0;

    let box_hull = make_box_hull(extent, extent, extent);

    for i in 0..base_count {
        let y = (2.0 * i as f32 + 1.0) * extent;

        for j in i..base_count {
            let x = (i as f32 + 1.0) * extent + 2.0 * (j - i) as f32 * extent + center_x - 0.5;
            body_def.position = pos(x, y, base_z);

            let body_id = create_body(world, &body_def);
            create_hull_shape(world, body_id, &shape_def, &box_hull);
        }
    }
}

struct ManyPyramids;

impl Scenario for ManyPyramids {
    fn create(&mut self, world: &mut World) {
        let base_count = 10;
        let extent = 0.5f32;
        let row_count: i32 = if BENCHMARK_DEBUG { 3 } else { 14 };
        let column_count: i32 = if BENCHMARK_DEBUG { 3 } else { 14 };
        let ground_extent = extent * column_count as f32 * (base_count as f32 + 1.0);

        {
            let mut body_def = default_body_def();
            body_def.position = pos(0.0, -1.0, 0.0);
            let ground_id = create_body(world, &body_def);

            let shape_def = default_shape_def();
            let box_hull = make_box_hull(ground_extent, 1.0, ground_extent);
            create_hull_shape(world, ground_id, &shape_def, &box_hull);
        }

        let base_width = 2.0 * extent * base_count as f32;
        let mut base_z = -ground_extent + 2.0 * extent;
        let delta_z = 2.0 * (ground_extent - 2.0 * extent) / (row_count as f32 - 1.0);

        for _i in 0..row_count {
            for j in 0..column_count {
                let center_x = -ground_extent + j as f32 * (base_width + 2.0 * extent) + 2.0 * extent;
                create_small_pyramid(world, base_count, extent, center_x, base_z);
            }

            base_z += delta_z;
        }
    }
}

// --- rain ---

const RAIN_GRID_SIZE: f32 = 15.0;
const RAIN_GRID_COUNT: usize = if BENCHMARK_DEBUG { 3 } else { 10 };
const RAIN_GROUP_SIZE: usize = if BENCHMARK_DEBUG { 2 } else { 3 };

#[derive(Default)]
struct Rain {
    groups: Vec<Vec<Human>>, // [RAIN_GRID_COUNT * RAIN_GRID_COUNT][<= RAIN_GROUP_SIZE]
    grid_mesh: Option<Arc<MeshData>>,
    torus_mesh: Option<Arc<MeshData>>,
    column_count: usize,
    column_index: usize,
}

impl Rain {
    fn create_group(&mut self, world: &mut World, row_index: usize, column_index: usize) {
        assert!(row_index < RAIN_GRID_COUNT && column_index < RAIN_GRID_COUNT);

        let group_index = row_index * RAIN_GRID_COUNT + column_index;

        let span = RAIN_GRID_COUNT as f32 * RAIN_GRID_SIZE;
        let group_distance = 1.0 * span / RAIN_GRID_COUNT as f32;

        let mut position = pos(
            -0.5 * span + group_distance * (column_index as f32 + 0.5),
            20.0,
            -0.5 * span + group_distance * (row_index as f32 + 0.5),
        );

        let friction_torque = 5.0;
        let hertz = 1.0;
        let damping_ratio = 0.7;

        for _ in 0..RAIN_GROUP_SIZE {
            let human = create_human(world, position, friction_torque, hertz, damping_ratio, group_index as i32);
            self.groups[group_index].push(human);
            position.x += 0.75;
        }
    }

    fn destroy_group(&mut self, world: &mut World, row_index: usize, column_index: usize) {
        assert!(row_index < RAIN_GRID_COUNT && column_index < RAIN_GRID_COUNT);

        let group_index = row_index * RAIN_GRID_COUNT + column_index;

        let mut humans = std::mem::take(&mut self.groups[group_index]);
        for human in humans.iter_mut() {
            destroy_human(world, human);
        }
    }
}

impl Scenario for Rain {
    // C: GetRainCapacity is a no-op with RAIN_LARGE_WORLD == 0.

    fn create(&mut self, world: &mut World) {
        self.groups.clear();
        for _ in 0..RAIN_GRID_COUNT * RAIN_GRID_COUNT {
            self.groups.push(Vec::new());
        }
        self.column_count = 0;
        self.column_index = 0;

        let half_mesh_grid_rows = 4;
        let mesh_grid_cell_width = RAIN_GRID_SIZE / (2.0 * half_mesh_grid_rows as f32);
        let grid_mesh = create_grid_mesh(2 * half_mesh_grid_rows, 2 * half_mesh_grid_rows, mesh_grid_cell_width, 1, true);
        let torus_mesh = create_torus_mesh(16, 16, 0.25 * RAIN_GRID_SIZE, 1.0);

        let span = RAIN_GRID_SIZE * RAIN_GRID_COUNT as f32;
        let mut body_def = default_body_def();
        let shape_def = default_shape_def();

        let mut px = -0.5 * span + 0.5 * RAIN_GRID_SIZE;
        for _i in 0..RAIN_GRID_COUNT {
            let mut pz = -0.5 * span + 0.5 * RAIN_GRID_SIZE;
            for _j in 0..RAIN_GRID_COUNT {
                body_def.position = pos(px, 0.0, pz);
                let body = create_body(world, &body_def);
                create_mesh_shape(world, body, &shape_def, &grid_mesh, Vec3::ONE);
                create_mesh_shape(world, body, &shape_def, &torus_mesh, Vec3::ONE);

                pz += RAIN_GRID_SIZE;
            }

            px += RAIN_GRID_SIZE;
        }

        self.grid_mesh = Some(grid_mesh);
        self.torus_mesh = Some(torus_mesh);
    }

    fn step(&mut self, world: &mut World, step_index: i32) {
        let delay: i32 = if BENCHMARK_DEBUG { 0x7F } else { 0x2F };
        let increment = 1usize;

        if (step_index & delay) == 0 {
            if self.column_count < RAIN_GRID_COUNT {
                let mut i = 0;
                while i < RAIN_GRID_COUNT {
                    let column = self.column_count;
                    self.create_group(world, i, column);
                    i += increment;
                }

                self.column_count = min_int(self.column_count as i32 + increment as i32, RAIN_GRID_COUNT as i32) as usize;
            } else {
                let mut i = 0;
                while i < RAIN_GRID_COUNT {
                    let column = self.column_index;
                    self.destroy_group(world, i, column);
                    self.create_group(world, i, column);
                    i += increment;
                }

                self.column_index += increment;
                if self.column_index >= RAIN_GRID_COUNT {
                    self.column_index = 0;
                }
            }
        }
    }
}

// --- large_world (static floor) ---

const STATIC_FLOOR_CELL_SIZE: f32 = 10.0;
const STATIC_FLOOR_GRID: i32 = if BENCHMARK_DEBUG { 32 } else { 1000 };
const STATIC_FLOOR_SPHERES: i32 = if BENCHMARK_DEBUG { 16 } else { 100 };
const STATIC_FLOOR_DROP_INTERVAL: i32 = if BENCHMARK_DEBUG { 8 } else { 5 };

#[derive(Default)]
struct LargeWorld {
    spheres_dropped: i32,
}

impl Scenario for LargeWorld {
    fn capacity(&self, capacity: &mut Capacity) {
        let floor_count = STATIC_FLOOR_GRID * STATIC_FLOOR_GRID;
        capacity.static_shape_count = floor_count;
        capacity.static_body_count = floor_count;
        capacity.dynamic_shape_count = STATIC_FLOOR_SPHERES;
        capacity.dynamic_body_count = STATIC_FLOOR_SPHERES;
        capacity.contact_count = max_int(1024, 8 * STATIC_FLOOR_SPHERES);
    }

    fn create(&mut self, world: &mut World) {
        self.spheres_dropped = 0;

        let cell = STATIC_FLOOR_CELL_SIZE;
        let grid_count = STATIC_FLOOR_GRID;
        let half_span = 0.5 * cell * grid_count as f32;

        let box_hull = make_box_hull(0.5 * cell, 0.25, 0.5 * cell);

        let mut body_def = default_body_def();
        let mut shape_def = default_shape_def();

        // The trigger: every static shape gets buffered into the move set on creation.
        shape_def.invoke_contact_creation = true;

        for i in 0..grid_count {
            let x = -half_span + (i as f32 + 0.5) * cell;
            for j in 0..grid_count {
                let z = -half_span + (j as f32 + 0.5) * cell;
                body_def.position = pos(x, 0.0, z);
                let body = create_body(world, &body_def);
                create_hull_shape(world, body, &shape_def, &box_hull);
            }
        }
    }

    fn step(&mut self, world: &mut World, step_index: i32) {
        if self.spheres_dropped >= STATIC_FLOOR_SPHERES {
            return;
        }

        if step_index == 0 {
            return;
        }

        if (step_index % STATIC_FLOOR_DROP_INTERVAL) != 0 {
            return;
        }

        // Spread spheres in a coarse grid across the floor so they don't all pile on one box.
        let mut side = 1;
        while side * side < STATIC_FLOOR_SPHERES {
            side += 1;
        }

        let idx = self.spheres_dropped;
        let gi = idx % side;
        let gj = idx / side;

        let half_span = 0.5 * STATIC_FLOOR_CELL_SIZE * STATIC_FLOOR_GRID as f32;
        // Confine drops to the inner 80% of the floor so spheres can't roll off the edge.
        let inset = 0.1 * 2.0 * half_span;
        let usable = 2.0 * half_span - 2.0 * inset;
        let x = -half_span + inset + (gi as f32 + 0.5) * (usable / side as f32);
        let z = -half_span + inset + (gj as f32 + 0.5) * (usable / side as f32);

        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(x, 1.5, z);

        let shape_def = default_shape_def();
        let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };

        let body = create_body(world, &body_def);
        create_sphere_shape(world, body, &shape_def, &sphere);

        self.spheres_dropped += 1;
    }
}

// --- washer ---

struct Washer;

impl Scenario for Washer {
    fn capacity(&self, capacity: &mut Capacity) {
        capacity.static_shape_count = 16;
        capacity.dynamic_shape_count = 10000;
        capacity.static_body_count = 16;
        capacity.dynamic_body_count = 10000;
        capacity.contact_count = 60000;
    }

    fn create(&mut self, world: &mut World) {
        let kinematic = true;

        let ground_id;
        {
            let mut body_def = default_body_def();
            body_def.position.y = -1.0;
            ground_id = create_body(world, &body_def);

            let box_hull = make_box_hull(60.0, 1.0, 60.0);
            let shape_def = default_shape_def();
            create_hull_shape(world, ground_id, &shape_def, &box_hull);
        }

        {
            let motor_speed = 25.0f32;

            let mut body_def = default_body_def();
            body_def.position = pos(0.0, 21.0, 0.0);

            if kinematic {
                body_def.body_type = BodyType::Kinematic;
                body_def.angular_velocity = vec3(0.0, 0.0, (PI / 180.0) * motor_speed);
                body_def.linear_velocity = vec3(0.001, -0.002, 0.0);
            } else {
                body_def.body_type = BodyType::Dynamic;
            }

            let body_id = create_body(world, &body_def);

            let shape_def = default_shape_def();

            let r0 = 14.0f32;
            let r1 = 16.0f32;
            let r2 = 18.0f32;
            let nd = vec3(0.0, 0.0, -10.0);
            let pd = vec3(0.0, 0.0, 10.0);

            let angle = PI / 18.0;
            let q = make_quat_from_axis_angle(Vec3::AXIS_Z, angle);
            let qo = make_quat_from_axis_angle(Vec3::AXIS_Z, 0.1 * angle);
            let mut u1 = vec3(1.0, 0.0, 0.0);
            for i in 0..36 {
                let u2 = if i == 35 { vec3(1.0, 0.0, 0.0) } else { rotate_vector(q, u1) };

                {
                    let a1 = inv_rotate_vector(qo, u1);
                    let a2 = rotate_vector(qo, u2);
                    let p1 = mul_add(nd, r1, a1);
                    let p2 = mul_add(nd, r2, a1);
                    let p3 = mul_add(nd, r1, a2);
                    let p4 = mul_add(nd, r2, a2);
                    let p5 = mul_add(pd, r1, a1);
                    let p6 = mul_add(pd, r2, a1);
                    let p7 = mul_add(pd, r1, a2);
                    let p8 = mul_add(pd, r2, a2);

                    let points = [p1, p2, p3, p4, p5, p6, p7, p8];
                    let hull = create_hull(&points, 8).expect("washer blade hull");
                    create_hull_shape(world, body_id, &shape_def, &hull);
                }

                if i % 9 == 0 {
                    let p1 = mul_add(nd, r0, u1);
                    let p2 = mul_add(nd, r1, u1);
                    let p3 = mul_add(nd, r0, u2);
                    let p4 = mul_add(nd, r1, u2);
                    let p5 = mul_add(pd, r0, u1);
                    let p6 = mul_add(pd, r1, u1);
                    let p7 = mul_add(pd, r0, u2);
                    let p8 = mul_add(pd, r1, u2);

                    let points = [p1, p2, p3, p4, p5, p6, p7, p8];
                    let hull = create_hull(&points, 8).expect("washer paddle hull");
                    create_hull_shape(world, body_id, &shape_def, &hull);
                }

                u1 = u2;
            }

            if !kinematic {
                let mut joint_def = default_revolute_joint_def();
                joint_def.base.body_id_a = ground_id;
                joint_def.base.body_id_b = body_id;
                joint_def.base.local_frame_a.p.y = 10.0;
                joint_def.motor_speed = (PI / 180.0) * motor_speed;
                joint_def.max_motor_torque = 1e8;
                joint_def.enable_motor = true;

                create_revolute_joint(world, &joint_def);
            }
        }

        let grid_count: i32 = if BENCHMARK_DEBUG { 8 } else { 20 };
        let a = 0.2f32;

        let cube = make_box_hull(a, a, a);
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        let shape_def = default_shape_def();

        let mut x = -2.0 * a * grid_count as f32;
        for _i in 0..grid_count {
            let mut y = -2.0 * a * grid_count as f32 + 21.0;
            for _j in 0..grid_count {
                let mut z = -2.0 * a * grid_count as f32;
                for _k in 0..grid_count {
                    body_def.position = pos(x, y, z);
                    let body_id = create_body(world, &body_def);

                    create_hull_shape(world, body_id, &shape_def, &cube);
                    z += 4.0 * a;
                }

                y += 4.0 * a;
            }

            x += 4.0 * a;
        }
    }
}

// --- trees ---

struct Trees {
    scale: i32,
    mesh_data: Option<Arc<MeshData>>,
}

impl Trees {
    fn new(scale: i32) -> Trees {
        Trees { scale, mesh_data: None }
    }
}

impl Scenario for Trees {
    fn create(&mut self, world: &mut World) {
        let scale = self.scale;

        // float tilt = 0.15f * B3_PI;
        let tilt = 0.0 * PI;
        let mut body_def = default_body_def();
        body_def.position = pos(0.0, 0.0, 0.0);
        body_def.rotation = make_quat_from_axis_angle(vec3(1.0, 0.0, 0.0), tilt);
        let ground_id = create_body(world, &body_def);

        let x_count = scale * 150;
        let z_count = scale * 200;

        let cell_width = 1.0 / scale as f32;
        let amplitude = 0.4;
        let row_hz = 0.05;
        let column_hz = 0.1;

        let mesh_data = create_wave_mesh(x_count, z_count, cell_width, amplitude, row_hz, column_hz);
        let mut shape_def = default_shape_def();
        create_mesh_shape(world, ground_id, &shape_def, &mesh_data, Vec3::ONE);
        self.mesh_data = Some(mesh_data);

        body_def.body_type = BodyType::Dynamic;
        body_def.sleep_threshold = 0.2;
        body_def.rotation = Quat::IDENTITY;

        let body_count: i32 = if BENCHMARK_DEBUG { 10 } else { 50 };

        shape_def.base_material.friction = 0.9;
        shape_def.base_material.rolling_resistance = 0.05;
        shape_def.update_body_mass = false;
        shape_def.density = 1.0;

        let hull_count = 22;
        let mut hulls: Vec<Arc<HullData>> = Vec::with_capacity(hull_count);

        let mut y = 1.0f32;
        let mut r = 0.75f32;
        let l = 1.5f32;
        for _i in 0..hull_count {
            hulls.push(create_cylinder(l + 2.0 * r, r, y - r, 6));
            y += l + 2.0 * r;
            r = 0.95 * r;
        }

        let mut angular_velocity = -0.5f32;
        let mut z: f32 = if BENCHMARK_DEBUG { -15.0 } else { -70.0 };
        let cs = compute_cos_sin(tilt);
        let y_tilt = cs.sine / cs.cosine;
        for body_index in 0..body_count {
            body_def.position = pos(0.0, 1.0 - z * y_tilt, z);
            let body_id = create_body(world, &body_def);

            for shape_index in 0..22 {
                create_hull_shape(world, body_id, &shape_def, &hulls[shape_index]);
            }

            let velocity_scale = 0.5 + (0.5 * body_index as f32) / body_count as f32;
            body_apply_mass_from_shapes(world, body_id);
            let center = body_get_world_center_of_mass(world, body_id);
            let omega = vec3(0.0, 0.0, velocity_scale * angular_velocity);
            let v = cross(omega, sub_pos(center, body_def.position));
            body_set_angular_velocity(world, body_id, omega);
            body_set_linear_velocity(world, body_id, v);

            z += 3.0;
            angular_velocity = -angular_velocity;
        }
    }
}

// --- junkyard ---

#[derive(Default)]
struct Junkyard {
    pusher_id: BodyId,
    degrees: f32,
    radius: f32,
}

impl Scenario for Junkyard {
    fn create(&mut self, world: &mut World) {
        let ground_id;
        {
            let mut body_def = default_body_def();
            body_def.position.y = -1.0;
            ground_id = create_body(world, &body_def);
        }

        {
            let shape_def = default_shape_def();
            {
                let box_hull = make_box_hull(120.0, 1.0, 120.0);
                create_hull_shape(world, ground_id, &shape_def, &box_hull);
            }
            {
                let offset = vec3(-50.0, 8.0, 0.0);
                let box_hull = make_offset_box_hull(1.0, 8.0, 50.0, offset);
                create_hull_shape(world, ground_id, &shape_def, &box_hull);
            }
            {
                let offset = vec3(50.0, 8.0, 0.0);
                let box_hull = make_offset_box_hull(1.0, 8.0, 50.0, offset);
                create_hull_shape(world, ground_id, &shape_def, &box_hull);
            }
            {
                let offset = vec3(0.0, 8.0, -50.0);
                let box_hull = make_offset_box_hull(50.0, 8.0, 1.0, offset);
                create_hull_shape(world, ground_id, &shape_def, &box_hull);
            }
            {
                let offset = vec3(0.0, 8.0, 50.0);
                let box_hull = make_offset_box_hull(50.0, 8.0, 1.0, offset);
                create_hull_shape(world, ground_id, &shape_def, &box_hull);
            }
        }
        {
            let rock_hull = create_rock(1.5);

            let count: i32 = if BENCHMARK_DEBUG { 2 } else { 24 };
            let height = 24.0f32;
            let mut body_def = default_body_def();
            body_def.body_type = BodyType::Dynamic;
            let shape_def = default_shape_def();
            for y in 0..count {
                for x in 0..=20 {
                    for z in 0..=20 {
                        let px = -40.0 + 4.0 * x as f32;
                        let py = 4.0 * y as f32 + height + 1.0;
                        let pz = -40.0 + 4.0 * z as f32; body_def.position = pos(px, py, pz);
                        let body_id = create_body(world, &body_def);
                        create_hull_shape(world, body_id, &shape_def, &rock_hull);
                    }
                }
            }
        }

        self.radius = 35.0;
        let m_height = 24.0;

        let hull = create_cylinder(m_height, 4.0, 0.0, 16);
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Kinematic;
        body_def.position = pos(self.radius, 0.0, 0.0);
        self.pusher_id = create_body(world, &body_def);
        self.degrees = 0.0;
        let shape_def = default_shape_def();
        create_hull_shape(world, self.pusher_id, &shape_def, &hull);
    }

    fn step(&mut self, world: &mut World, _step_index: i32) {
        let time_step = 1.0 / 60.0;
        let omega = -6.0f32;
        self.degrees += omega * time_step;
        let cs = compute_cos_sin(self.degrees * PI / 180.0);
        let r = self.radius;
        let target_pos = pos(r * cs.cosine, 0.0, r * cs.sine);
        let target = WorldTransform { p: target_pos, q: Quat::IDENTITY };
        body_set_target_transform(world, self.pusher_id, target, time_step, false);
    }
}

// ---------------------------------------------------------------------------
// benchmark/main.c harness
// ---------------------------------------------------------------------------

struct Benchmark {
    name: &'static str,
    make: fn() -> Box<dyn Scenario>,
    total_step_count: i32,
}

// C: MinProfile mins these seven fields.
fn min_profile(p1: &mut Profile, p2: &Profile) {
    p1.step = p1.step.min(p2.step);
    p1.pairs = p1.pairs.min(p2.pairs);
    p1.collide = p1.collide.min(p2.collide);
    p1.constraints = p1.constraints.min(p2.constraints);
    p1.transforms = p1.transforms.min(p2.transforms);
    p1.refit = p1.refit.min(p2.refit);
    p1.sleep_islands = p1.sleep_islands.min(p2.sleep_islands);
}

fn max_profile() -> Profile {
    let mut p = Profile::default();
    p.step = f32::MAX;
    p.pairs = f32::MAX;
    p.collide = f32::MAX;
    p.solve = f32::MAX;
    p.solver_setup = f32::MAX;
    p.constraints = f32::MAX;
    p.prepare_constraints = f32::MAX;
    p.integrate_velocities = f32::MAX;
    p.warm_start = f32::MAX;
    p.solve_impulses = f32::MAX;
    p.integrate_positions = f32::MAX;
    p.relax_impulses = f32::MAX;
    p.apply_restitution = f32::MAX;
    p.store_impulses = f32::MAX;
    p.split_islands = f32::MAX;
    p.transforms = f32::MAX;
    p.hit_events = f32::MAX;
    p.refit = f32::MAX;
    p.bullets = f32::MAX;
    p.sleep_islands = f32::MAX;
    p
}

struct Summary {
    name: &'static str,
    step_count: i32,
    min_ms: f32,
    profile_sums: [f32; 7], // step pairs collide constraints transforms refit sleep
}

fn main() {
    let benchmarks: Vec<Benchmark> = vec![
        Benchmark { name: "trees100", make: || Box::new(Trees::new(1)), total_step_count: 500 },
        Benchmark { name: "trees50", make: || Box::new(Trees::new(2)), total_step_count: 500 },
        Benchmark { name: "trees25", make: || Box::new(Trees::new(4)), total_step_count: 500 },
        Benchmark { name: "joint_grid", make: || Box::new(JointGrid), total_step_count: 100 },
        Benchmark { name: "junkyard", make: || Box::new(Junkyard::default()), total_step_count: 500 },
        Benchmark { name: "large_pyramid", make: || Box::new(LargePyramid), total_step_count: 200 },
        Benchmark { name: "many_pyramids", make: || Box::new(ManyPyramids), total_step_count: 100 },
        Benchmark { name: "rain", make: || Box::new(Rain::default()), total_step_count: 400 },
        Benchmark { name: "washer", make: || Box::new(Washer), total_step_count: 1000 },
        Benchmark { name: "large_world", make: || Box::new(LargeWorld::default()), total_step_count: 500 },
    ];

    let benchmark_count = benchmarks.len() as i32;

    let mut run_count = 4;
    let mut single_benchmark = -1;
    let mut enable_continuous = true;

    for arg in std::env::args().skip(1) {
        if let Some(v) = arg.strip_prefix("-b=") {
            single_benchmark = clamp_int(v.parse().unwrap_or(0), 0, benchmark_count - 1);
        } else if let Some(v) = arg.strip_prefix("-r=") {
            run_count = clamp_int(v.parse().unwrap_or(4), 1, 1000);
        } else if arg.starts_with("-nc") {
            enable_continuous = false;
            println!("Continuous disabled");
        } else if arg.starts_with("-t=") || arg.starts_with("-w=") || arg == "-s" {
            println!("note: {} ignored (serial port, no step-time files)", arg);
        } else if arg == "-h" {
            println!(
                "Usage\n-b=<integer>: run a single benchmark\n-r=<integer>: number of repeats (default is 4)\n-nc: disable continuous collision"
            );
            return;
        }
    }

    println!("Starting benchmarks (Rust port, single threaded)");
    println!("======================================");

    let mut summaries: Vec<Summary> = Vec::new();

    for (benchmark_index, benchmark) in benchmarks.iter().enumerate() {
        if single_benchmark != -1 && benchmark_index as i32 != single_benchmark {
            continue;
        }

        // C: #ifdef NDEBUG stepCount else 10
        let step_count = if BENCHMARK_DEBUG { 10 } else { benchmark.total_step_count };

        println!("benchmark: {}, steps = {}", benchmark.name, step_count);
        println!("thread count: 1");

        // Deviation: C initializes this array once for the whole app; the port
        // resets per benchmark so the summary is per benchmark.
        let mut profiles = vec![max_profile(); step_count as usize];

        let mut min_time = f32::MAX;
        let mut counters = Counters::default();
        let mut counters_acquired = false;

        for run_index in 0..run_count {
            let mut world_def = default_world_def();
            world_def.enable_continuous = enable_continuous;
            world_def.worker_count = 1;

            let mut scenario = (benchmark.make)();
            scenario.capacity(&mut world_def.capacity);

            let mut world = create_world(&world_def);

            scenario.create(&mut world);

            let time_step = 1.0 / 60.0;
            let sub_step_count = 4;

            // Initial step can be expensive and skew benchmark
            scenario.step(&mut world, 0);

            world_step(&mut world, time_step, sub_step_count);

            let profile = world_get_profile(&world);
            min_profile(&mut profiles[0], &profile);

            let ticks = get_ticks();

            for step_index in 1..step_count {
                scenario.step(&mut world, step_index);

                world_step(&mut world, time_step, sub_step_count);

                let profile = world_get_profile(&world);
                min_profile(&mut profiles[step_index as usize], &profile);
            }

            let ms = get_milliseconds(ticks);
            println!("run {} : {} (ms)", run_index, ms);

            if run_index == 0 {
                min_time = ms;
            } else {
                min_time = min_time.min(ms);
            }

            if !counters_acquired {
                counters = world_get_counters(&world);
                counters_acquired = true;
            }

            destroy_world(world);
            drop(scenario);
        }

        println!(
            "body {} / shape {} / contact {} / joint {} / stack {}\n",
            counters.body_count, counters.shape_count, counters.contact_count, counters.joint_count, counters.stack_used
        );

        let mut sums = [0.0f32; 7];
        for p in &profiles {
            sums[0] += p.step;
            sums[1] += p.pairs;
            sums[2] += p.collide;
            sums[3] += p.constraints;
            sums[4] += p.transforms;
            sums[5] += p.refit;
            sums[6] += p.sleep_islands;
        }

        summaries.push(Summary { name: benchmark.name, step_count, min_ms: min_time, profile_sums: sums });
    }

    println!("======================================");
    println!("All benchmarks complete!");
    println!();
    println!(
        "{:<14} {:>9} {:>10} {:>9} | {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "benchmark", "total ms", "ms/step", "steps/s", "step", "pairs", "collide", "constr", "xforms", "refit", "sleep"
    );
    for s in &summaries {
        let timed_steps = (s.step_count - 1).max(1) as f32;
        println!(
            "{:<14} {:>9.1} {:>10.3} {:>9.0} | {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1}",
            s.name,
            s.min_ms,
            s.min_ms / timed_steps,
            1000.0 * timed_steps / s.min_ms,
            s.profile_sums[0],
            s.profile_sums[1],
            s.profile_sums[2],
            s.profile_sums[3],
            s.profile_sums[4],
            s.profile_sums[5],
            s.profile_sums[6],
        );
    }
}
