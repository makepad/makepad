// Port of box3d/test/test_determinism.c + the scenario scaffolding from
// box3d/shared/determinism.c and box3d/shared/human.c (falling ragdolls on a
// grid mesh + torus mesh).
//
// DEVIATION from C: the C test compares the settled world hash against a
// hard-coded EXPECTED_HASH (0x50313037 float build) captured from the C
// binary. The Rust port preserves the C float operation order but cannot
// promise bit-equality with a C build (different IEEE remainder
// implementation, different code generation), so this port runs the identical
// scenario TWICE from scratch and requires both runs to produce the same
// sleep step and the same hash (internal determinism). The hash is printed so
// it can be tracked across Rust builds/platforms. The C worker-count loop
// (1..6 workers must agree) collapses to the serial port's single
// configuration.

use makepad_box3d::body::*;
use makepad_box3d::core::{hash, HASH_INIT};
use makepad_box3d::ensure;
use makepad_box3d::id::{BodyId, JointId, NULL_BODY_ID, NULL_JOINT_ID};
use makepad_box3d::joint::*;
use makepad_box3d::math_functions::{
    normalize_quat, offset_pos, quat, vec3, Pos, Transform, Vec2, Vec3, WorldTransform,
    DEG_TO_RAD,
};
use makepad_box3d::mesh::{create_grid_mesh, create_torus_mesh};
use makepad_box3d::physics_world::*;
use makepad_box3d::shape::{create_capsule_shape, create_mesh_shape};
use makepad_box3d::types::*;

// ---------------------------------------------------------------------------
// human.c scaffolding (CreateHuman only — the parts the scenario uses)
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

// Port of CreateHuman (human.c). colorize is always false in the determinism
// scenario so the color plumbing is dropped; userData is unused (0).
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

        let capsule = Capsule { center1: vec3(0.06, -0.0, -0.052264), center2: vec3(-0.06, 0.0, -0.052264), radius: 0.12 };
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

// ---------------------------------------------------------------------------
// determinism.c scenario
// ---------------------------------------------------------------------------

const RAGDOLL_GROUP_SIZE: usize = 2;
const RAGDOLL_GRID_COUNT: usize = 2;
const GRID_SIZE: f32 = 15.0;

#[derive(Default)]
struct FallingRagdollData {
    groups: Vec<Vec<Human>>, // [grid*grid][group_size]
    step_count: i32,
    sleep_step: i32,
    hash: u32,
}

fn create_group(data: &mut FallingRagdollData, world: &mut World, row_index: usize, column_index: usize) {
    let group_index = row_index * RAGDOLL_GRID_COUNT + column_index;

    let span = RAGDOLL_GRID_COUNT as f32 * GRID_SIZE;
    let group_distance = 1.0 * span / RAGDOLL_GRID_COUNT as f32;

    let mut position = makepad_box3d::math_functions::pos(
        -0.5 * span + group_distance * (column_index as f32 + 0.5),
        15.0,
        -0.5 * span + group_distance * (row_index as f32 + 0.5),
    );

    let friction_torque = 5.0;
    let hertz = 1.0;
    let damping_ratio = 0.7;

    for _ in 0..RAGDOLL_GROUP_SIZE {
        let human = create_human(world, position, friction_torque, hertz, damping_ratio, group_index as i32);
        data.groups[group_index].push(human);
        position.x += 0.75;
    }
}

fn create_falling_ragdolls(world: &mut World) -> FallingRagdollData {
    let mut data = FallingRagdollData::default();
    for _ in 0..RAGDOLL_GRID_COUNT * RAGDOLL_GRID_COUNT {
        data.groups.push(Vec::new());
    }
    data.sleep_step = 0;

    let half_mesh_grid_rows = 4;
    let mesh_grid_cell_width = GRID_SIZE / (2.0 * half_mesh_grid_rows as f32);
    let grid_mesh = create_grid_mesh(2 * half_mesh_grid_rows, 2 * half_mesh_grid_rows, mesh_grid_cell_width, 0, true);
    let torus_mesh = create_torus_mesh(16, 16, 0.25 * GRID_SIZE, 1.0);

    let span = GRID_SIZE * RAGDOLL_GRID_COUNT as f32;
    let mut body_def = default_body_def();
    let shape_def = default_shape_def();

    // C mutates position.x/.z in place; track f32 locals and assign through pos()
    // so the float expressions stay identical in both precision modes.
    let mut px = -0.5 * span + 0.5 * GRID_SIZE;
    for i in 0..RAGDOLL_GRID_COUNT {
        let mut pz = -0.5 * span + 0.5 * GRID_SIZE;
        for j in 0..RAGDOLL_GRID_COUNT {
            body_def.position = makepad_box3d::math_functions::pos(px, 0.0, pz);
            let body = create_body(world, &body_def);
            create_mesh_shape(world, body, &shape_def, &grid_mesh, Vec3::ONE);
            create_mesh_shape(world, body, &shape_def, &torus_mesh, Vec3::ONE);

            create_group(&mut data, world, i, j);

            pz += GRID_SIZE;
        }

        px += GRID_SIZE;
    }

    data
}

#[cfg(not(feature = "double-precision"))]
fn hash_world_transform(h: u32, xf: &WorldTransform) -> u32 {
    // C hashes the raw 28 bytes of b3WorldTransform (float mode): p then q.
    let mut bytes = [0u8; 28];
    bytes[0..4].copy_from_slice(&xf.p.x.to_le_bytes());
    bytes[4..8].copy_from_slice(&xf.p.y.to_le_bytes());
    bytes[8..12].copy_from_slice(&xf.p.z.to_le_bytes());
    bytes[12..16].copy_from_slice(&xf.q.v.x.to_le_bytes());
    bytes[16..20].copy_from_slice(&xf.q.v.y.to_le_bytes());
    bytes[20..24].copy_from_slice(&xf.q.v.z.to_le_bytes());
    bytes[24..28].copy_from_slice(&xf.q.s.to_le_bytes());
    hash(h, &bytes)
}

#[cfg(feature = "double-precision")]
fn hash_world_transform(h: u32, xf: &WorldTransform) -> u32 {
    // Double precision: the position is three f64 (C hashes the raw 40-byte struct).
    let mut bytes = [0u8; 40];
    bytes[0..8].copy_from_slice(&xf.p.x.to_le_bytes());
    bytes[8..16].copy_from_slice(&xf.p.y.to_le_bytes());
    bytes[16..24].copy_from_slice(&xf.p.z.to_le_bytes());
    bytes[24..28].copy_from_slice(&xf.q.v.x.to_le_bytes());
    bytes[28..32].copy_from_slice(&xf.q.v.y.to_le_bytes());
    bytes[32..36].copy_from_slice(&xf.q.v.z.to_le_bytes());
    bytes[36..40].copy_from_slice(&xf.q.s.to_le_bytes());
    hash(h, &bytes)
}

fn update_falling_ragdolls(world: &World, data: &mut FallingRagdollData) -> bool {
    if data.hash == 0 {
        let move_count = world_get_body_events(world).move_events.len();

        if move_count == 0 {
            let awake_count = world_get_awake_body_count(world);
            assert!(awake_count == 0, "no move events but {} awake bodies", awake_count);

            data.hash = HASH_INIT;
            for i in 0..RAGDOLL_GRID_COUNT {
                for j in 0..RAGDOLL_GRID_COUNT {
                    for k in 0..RAGDOLL_GROUP_SIZE {
                        let group_index = i * RAGDOLL_GRID_COUNT + j;
                        let human = &data.groups[group_index][k];

                        for b in 0..BONE_COUNT {
                            let body_id = human.bones[b].body_id;
                            let xf = body_get_transform(world, body_id);
                            data.hash = hash_world_transform(data.hash, &xf);
                        }
                    }
                }
            }

            data.sleep_step = data.step_count;
        }
    }

    data.step_count += 1;

    data.hash != 0
}

fn run_scenario() -> (i32, u32) {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    let mut data = create_falling_ragdolls(&mut world);

    let time_step = 1.0 / 60.0;

    let step_limit = 1000;
    let mut done = false;
    for _ in 0..step_limit {
        let sub_step_count = 4;
        world_step(&mut world, time_step, sub_step_count);

        done = update_falling_ragdolls(&world, &mut data);
        if done {
            break;
        }
    }

    destroy_world(world);

    assert!(done, "ragdolls did not settle within {} steps", step_limit);
    (data.sleep_step, data.hash)
}

#[test]
fn determinism_test() {
    let (sleep_step1, hash1) = run_scenario();
    println!("run 1: sleepStep={} hash=0x{:08X}", sleep_step1, hash1);

    let (sleep_step2, hash2) = run_scenario();
    println!("run 2: sleepStep={} hash=0x{:08X}", sleep_step2, hash2);

    ensure!(sleep_step1 == sleep_step2);
    ensure!(hash1 == hash2);
    ensure!(hash1 != 0);
}
