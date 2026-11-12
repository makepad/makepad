//! Builder inputs for the rigid-body subset, with explicit body ownership.
//!
//! Translated from `mjspec.h`, `src/user/user_init.c`, the wrap functions of
//! `src/user/user_objects.cc` and the actuator setters of `src/user/user_api.cc` of the engine
//! named in CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited; mjspec.h
//! Copyright 2024 DeepMind Technologies Limited);
//! changed: Rust ownership, optional names, explicit parent indices; radians,
//! explicit inertias and autolimits are fixed by this subset's export contract.
//!
//! NaN in the first component retains C's unset sentinel for `ipos`,
//! `fullinertia`, and `fromto`; geom `mass` is likewise unset by default.
//! Set mass and inertia before using a moving body. Body zero is the world;
//! its body/inertial attributes and joints are not representable in MJCF.
//! Convex mesh assets are given as vertices only (no files, faces, normals or
//! textures); the compiler builds their hull. Other assets, visual-global, compiler
//! bookkeeping, alternative orientations, default classes, frames, and excluded
//! element types are omitted.

use crate::model::{default_sol_ref_imp, Opt, Statistic};
use crate::types::*;

/// The applicable mjsCompiler settings; defaults from mjs_defaultSpec.
#[derive(Clone, Debug)]
pub struct CompilerSpec {
    pub boundmass: f64,
    pub boundinertia: f64,
    pub settotalmass: f64,
    pub balanceinertia: bool,
    pub discardvisual: bool,
    pub usethread: bool,
    pub fusestatic: bool,
    pub alignfree: bool,
}

impl Default for CompilerSpec {
    fn default() -> Self {
        Self {
            boundmass: 0.0,
            boundinertia: 0.0,
            settotalmass: -1.0,
            balanceinertia: false,
            discardvisual: false,
            usethread: true,
            fusestatic: false,
            alignfree: false,
        }
    }
}

/// mjSpec subset. Body 0 is the world; each other parent must precede its child.
#[derive(Clone, Debug)]
pub struct ModelSpec {
    pub modelname: Option<String>,
    pub compiler: CompilerSpec,
    pub option: Opt,
    /// NaN retains each automatically computed statistic (center uses component zero).
    pub stat: Statistic,
    pub memory: i64,
    pub nuserdata: i32,
    pub nuser_body: i32,
    pub nuser_jnt: i32,
    pub nuser_geom: i32,
    pub nuser_site: i32,
    pub nuser_tendon: i32,
    pub nuser_actuator: i32,
    pub meshes: Vec<MeshSpec>,
    pub bodies: Vec<BodySpec>,
    pub equalities: Vec<EqualitySpec>,
    pub pairs: Vec<PairSpec>,
    pub excludes: Vec<ExcludeSpec>,
    pub tendons: Vec<TendonSpec>,
    pub actuators: Vec<ActuatorSpec>,
}

impl Default for ModelSpec {
    fn default() -> Self {
        Self {
            modelname: None,
            compiler: CompilerSpec::default(),
            option: Opt::default(),
            stat: Statistic { meaninertia: f64::NAN, meanmass: f64::NAN, meansize: f64::NAN, extent: f64::NAN, center: [f64::NAN, 0.0, 0.0] },
            memory: -1,
            nuserdata: 0,
            nuser_body: -1,
            nuser_jnt: -1,
            nuser_geom: -1,
            nuser_site: -1,
            nuser_tendon: -1,
            nuser_actuator: -1,
            meshes: Vec::new(),
            bodies: vec![BodySpec::default()],
            equalities: Vec::new(),
            pairs: Vec::new(),
            excludes: Vec::new(),
            tendons: Vec::new(),
            actuators: Vec::new(),
        }
    }
}

// mjtMeshInertia.
pub const MESH_INERTIA_CONVEX: i32 = 0;
pub const MESH_INERTIA_EXACT: i32 = 1;
pub const MESH_INERTIA_LEGACY: i32 = 2;
pub const MESH_INERTIA_SHELL: i32 = 3;

/// mjsMesh / mjs_defaultMesh for a mesh given as vertices only (x y z per vertex,
/// model units); the faces are those of the convex hull. Reference position and
/// orientation, normals, textures, hull size limit and signed-distance options are
/// outside the subset, so their defaults apply.
#[derive(Clone, Debug)]
pub struct MeshSpec {
    pub name: Option<String>,
    pub vert: Vec<f32>,
    pub scale: [f64; 3],
    pub inertia: i32,
}

impl Default for MeshSpec {
    fn default() -> Self {
        Self { name: None, vert: Vec::new(), scale: [1.0; 3], inertia: MESH_INERTIA_LEGACY }
    }
}

/// mjsBody / mjs_defaultBody. Full inertia is in the body frame for MJCF export.
#[derive(Clone, Debug)]
pub struct BodySpec {
    pub name: Option<String>,
    pub parent: usize,
    pub pos: [f64; 3],
    pub quat: [f64; 4],
    pub mass: f64,
    pub ipos: [f64; 3],
    pub iquat: [f64; 4],
    pub inertia: [f64; 3],
    pub fullinertia: [f64; 6],
    pub mocap: bool,
    pub gravcomp: f64,
    pub sleep: i32,
    pub simple: u8,
    pub fuse: u8,
    pub userdata: Vec<f64>,
    pub joints: Vec<JointSpec>,
    pub geoms: Vec<GeomSpec>,
    pub sites: Vec<SiteSpec>,
}

impl Default for BodySpec {
    fn default() -> Self {
        Self {
            name: None,
            parent: 0,
            pos: [0.0; 3],
            quat: [1.0, 0.0, 0.0, 0.0],
            mass: 0.0,
            ipos: [f64::NAN, 0.0, 0.0],
            iquat: [1.0, 0.0, 0.0, 0.0],
            inertia: [0.0; 3],
            fullinertia: [f64::NAN, 0.0, 0.0, 0.0, 0.0, 0.0],
            mocap: false,
            gravcomp: 0.0,
            sleep: SLEEP_AUTO,
            simple: 1,
            fuse: 1,
            userdata: Vec::new(),
            joints: Vec::new(),
            geoms: Vec::new(),
            sites: Vec::new(),
        }
    }
}

/// mjsJoint / mjs_defaultJoint. Non-auto align uses the MJCF freejoint shortcut.
#[derive(Clone, Debug)]
pub struct JointSpec {
    pub name: Option<String>,
    pub r#type: i32,
    pub pos: [f64; 3],
    pub axis: [f64; 3],
    pub r#ref: f64,
    pub align: i32,
    pub stiffness: [f64; NPOLY + 1],
    pub springref: f64,
    pub springyield: f64,
    pub springdamper: [f64; 2],
    pub limited: i32,
    pub range: [f64; 2],
    pub margin: f64,
    pub solref_limit: [f64; NREF],
    pub solimp_limit: [f64; NIMP],
    pub actfrclimited: i32,
    pub actfrcrange: [f64; 2],
    pub armature: f64,
    pub damping: [f64; NPOLY + 1],
    pub frictionloss: f64,
    pub solref_friction: [f64; NREF],
    pub solimp_friction: [f64; NIMP],
    pub group: i32,
    pub actgravcomp: bool,
    pub userdata: Vec<f64>,
}

impl Default for JointSpec {
    fn default() -> Self {
        let mut solref = [0.0; NREF];
        let mut solimp = [0.0; NIMP];
        default_sol_ref_imp(Some(&mut solref), Some(&mut solimp));
        Self {
            name: None,
            r#type: JNT_HINGE,
            pos: [0.0; 3],
            axis: [0.0, 0.0, 1.0],
            r#ref: 0.0,
            align: 2,
            stiffness: [0.0; NPOLY + 1],
            springref: 0.0,
            springyield: 0.0,
            springdamper: [0.0; 2],
            limited: 2,
            range: [0.0; 2],
            margin: 0.0,
            solref_limit: solref,
            solimp_limit: solimp,
            actfrclimited: 2,
            actfrcrange: [0.0; 2],
            armature: 0.0,
            damping: [0.0; NPOLY + 1],
            frictionloss: 0.0,
            solref_friction: solref,
            solimp_friction: solimp,
            group: 0,
            actgravcomp: false,
            userdata: Vec::new(),
        }
    }
}

/// mjsGeom / mjs_defaultGeom, with a mesh asset by name for `GEOM_MESH`; other assets
/// and plugins are outside the subset.
#[derive(Clone, Debug)]
pub struct GeomSpec {
    pub name: Option<String>,
    pub meshname: Option<String>,
    pub r#type: i32,
    pub pos: [f64; 3],
    pub quat: [f64; 4],
    pub fromto: [f64; 6],
    pub size: [f64; 3],
    pub contype: i32,
    pub conaffinity: i32,
    pub condim: i32,
    pub priority: i32,
    pub friction: [f64; 3],
    pub solmix: f64,
    pub solref: [f64; NREF],
    pub solimp: [f64; NIMP],
    pub margin: f64,
    pub gap: f64,
    pub surfacevel: [f64; 6],
    pub adhesion: f64,
    pub mass: f64,
    pub density: f64,
    pub typeinertia: i32,
    pub fluid_ellipsoid: f64,
    pub fluid_coefs: [f64; 5],
    pub rgba: [f32; 4],
    pub group: i32,
    pub castshadow: bool,
    pub userdata: Vec<f64>,
}

impl Default for GeomSpec {
    fn default() -> Self {
        let mut solref = [0.0; NREF];
        let mut solimp = [0.0; NIMP];
        default_sol_ref_imp(Some(&mut solref), Some(&mut solimp));
        Self {
            name: None,
            meshname: None,
            r#type: GEOM_SPHERE,
            pos: [0.0; 3],
            quat: [1.0, 0.0, 0.0, 0.0],
            fromto: [f64::NAN, 0.0, 0.0, 0.0, 0.0, 0.0],
            size: [0.0; 3],
            contype: 1,
            conaffinity: 1,
            condim: 3,
            priority: 0,
            friction: [1.0, 0.005, 0.0001],
            solmix: 1.0,
            solref: solref,
            solimp: solimp,
            margin: 0.0,
            gap: 0.0,
            surfacevel: [0.0; 6],
            adhesion: 0.0,
            mass: f64::NAN,
            density: 1000.0,
            typeinertia: 0,
            fluid_ellipsoid: 0.0,
            fluid_coefs: [0.5, 0.25, 1.5, 1.0, 1.0],
            rgba: [0.5, 0.5, 0.5, 1.0],
            group: 0,
            castshadow: true,
            userdata: Vec::new(),
        }
    }
}

/// mjsSite / mjs_defaultSite, without assets.
#[derive(Clone, Debug)]
pub struct SiteSpec {
    pub name: Option<String>,
    pub pos: [f64; 3],
    pub quat: [f64; 4],
    pub fromto: [f64; 6],
    pub size: [f64; 3],
    pub r#type: i32,
    pub group: i32,
    pub rgba: [f32; 4],
    pub userdata: Vec<f64>,
}

impl Default for SiteSpec {
    fn default() -> Self {
        Self {
            name: None,
            pos: [0.0; 3],
            quat: [1.0, 0.0, 0.0, 0.0],
            fromto: [f64::NAN, 0.0, 0.0, 0.0, 0.0, 0.0],
            size: [0.005; 3],
            r#type: GEOM_SPHERE,
            group: 0,
            rgba: [0.5, 0.5, 0.5, 1.0],
            userdata: Vec::new(),
        }
    }
}

/// mjsPair / mjs_defaultPair; both referenced geoms must have names.
#[derive(Clone, Debug)]
pub struct PairSpec {
    pub name: Option<String>,
    pub geomname1: Option<String>,
    pub geomname2: Option<String>,
    pub condim: i32,
    pub solref: [f64; NREF],
    pub solreffriction: [f64; NREF],
    pub solimp: [f64; NIMP],
    pub margin: f64,
    pub gap: f64,
    pub adhesion: f64,
    pub friction: [f64; 5],
}

impl Default for PairSpec {
    fn default() -> Self {
        let mut solref = [0.0; NREF];
        let mut solimp = [0.0; NIMP];
        default_sol_ref_imp(Some(&mut solref), Some(&mut solimp));
        Self {
            name: None,
            geomname1: None,
            geomname2: None,
            condim: 3,
            solref: solref,
            solreffriction: [0.0; NREF],
            solimp: solimp,
            margin: 0.0,
            gap: 0.0,
            adhesion: 0.0,
            friction: [1.0, 1.0, 0.005, 0.0001, 0.0001],
        }
    }
}

/// mjsExclude; empty names match the mjCBodyPair constructor (no C default function).
#[derive(Clone, Debug)]
pub struct ExcludeSpec {
    pub name: Option<String>,
    pub bodyname1: Option<String>,
    pub bodyname2: Option<String>,
}

impl Default for ExcludeSpec {
    fn default() -> Self {
        Self {
            name: None,
            bodyname1: None,
            bodyname2: None,
        }
    }
}

/// mjsEquality / mjs_defaultEquality. Connect, weld and joint equalities only.
#[derive(Clone, Debug)]
pub struct EqualitySpec {
    pub name: Option<String>,
    pub r#type: i32,
    pub data: [f64; NEQDATA],
    pub active: bool,
    pub name1: Option<String>,
    pub name2: Option<String>,
    pub objtype: i32,
    pub solref: [f64; NREF],
    pub solimp: [f64; NIMP],
}

impl Default for EqualitySpec {
    fn default() -> Self {
        let mut solref = [0.0; NREF];
        let mut solimp = [0.0; NIMP];
        default_sol_ref_imp(Some(&mut solref), Some(&mut solimp));
        Self {
            name: None,
            r#type: EQ_CONNECT,
            data: { let mut data = [0.0; NEQDATA]; data[1] = 1.0; data[10] = 1.0; data },
            active: true,
            name1: None,
            name2: None,
            objtype: OBJ_UNKNOWN,
            solref: solref,
            solimp: solimp,
        }
    }
}

/// mjsWrap with the fields mjs_wrapSite, mjs_wrapGeom, mjs_wrapJoint and mjs_wrapPulley set:
/// `name` is the site, geom or joint, `sidesite` the optional side site of a geom, and `prm`
/// the joint coefficient or the pulley divisor. A geom wrap is `WRAP_SPHERE` until compiled;
/// the compiler takes sphere or cylinder from the geom.
#[derive(Clone, Debug)]
pub struct WrapSpec {
    pub r#type: i32,
    pub name: Option<String>,
    pub sidesite: Option<String>,
    pub prm: f64,
}

impl WrapSpec {
    pub fn site(name: &str) -> Self {
        Self { r#type: WRAP_SITE, name: Some(name.into()), sidesite: None, prm: 0.0 }
    }

    pub fn geom(name: &str, sidesite: Option<&str>) -> Self {
        Self { r#type: WRAP_SPHERE, name: Some(name.into()), sidesite: sidesite.map(Into::into), prm: 0.0 }
    }

    pub fn joint(name: &str, coef: f64) -> Self {
        Self { r#type: WRAP_JOINT, name: Some(name.into()), sidesite: None, prm: coef }
    }

    pub fn pulley(divisor: f64) -> Self {
        Self { r#type: WRAP_PULLEY, name: None, sidesite: None, prm: divisor }
    }
}

/// mjsTendon / mjs_defaultTendon, without a material. The path makes it a fixed tendon when
/// every wrap is a joint, otherwise a spatial one. MJCF has no width or rgba on a fixed tendon,
/// so those keep their defaults there.
#[derive(Clone, Debug)]
pub struct TendonSpec {
    pub name: Option<String>,
    pub stiffness: [f64; NPOLY + 1],
    pub springlength: [f64; 2],
    pub springyield: f64,
    pub damping: [f64; NPOLY + 1],
    pub frictionloss: f64,
    pub solref_friction: [f64; NREF],
    pub solimp_friction: [f64; NIMP],
    pub armature: f64,
    pub limited: i32,
    pub actfrclimited: i32,
    pub range: [f64; 2],
    pub actfrcrange: [f64; 2],
    pub margin: f64,
    pub solref_limit: [f64; NREF],
    pub solimp_limit: [f64; NIMP],
    pub width: f64,
    pub rgba: [f32; 4],
    pub group: i32,
    pub userdata: Vec<f64>,
    pub path: Vec<WrapSpec>,
}

impl Default for TendonSpec {
    fn default() -> Self {
        let mut solref = [0.0; NREF];
        let mut solimp = [0.0; NIMP];
        default_sol_ref_imp(Some(&mut solref), Some(&mut solimp));
        Self {
            name: None,
            stiffness: [0.0; NPOLY + 1],
            springlength: [-1.0, -1.0],
            springyield: 0.0,
            damping: [0.0; NPOLY + 1],
            frictionloss: 0.0,
            solref_friction: solref,
            solimp_friction: solimp,
            armature: 0.0,
            limited: 2,
            actfrclimited: 2,
            range: [0.0; 2],
            actfrcrange: [0.0; 2],
            margin: 0.0,
            solref_limit: solref,
            solimp_limit: solimp,
            width: 0.003,
            rgba: [0.5, 0.5, 0.5, 1.0],
            group: 0,
            userdata: Vec::new(),
            path: Vec::new(),
        }
    }
}

// mjtActuator: the element an actuator is written with.
pub const ACTUATOR_GENERAL: i32 = 0;
pub const ACTUATOR_MOTOR: i32 = 1;
pub const ACTUATOR_POSITION: i32 = 2;
pub const ACTUATOR_VELOCITY: i32 = 3;
pub const ACTUATOR_INTVELOCITY: i32 = 4;
pub const ACTUATOR_DAMPER: i32 = 5;
pub const ACTUATOR_CYLINDER: i32 = 6;
pub const ACTUATOR_MUSCLE: i32 = 7;
pub const ACTUATOR_ADHESION: i32 = 8;
pub const ACTUATOR_PID: i32 = 9;
pub const ACTUATOR_ORIENTATION: i32 = 10;
pub const ACTUATOR_DCMOTOR: i32 = 11;

/// mjsActuator / mjs_defaultActuator, without a plugin. `target` names the transmission's joint,
/// tendon, site or body (a slider-crank's crank site), `refsite` a site transmission's reference
/// and `slidersite` a slider-crank's base. The `set_to_*` setters give a shortcut's control model.
/// Muscles are outside the subset. A delay (`delay`) reads the controls from a history buffer of
/// `nsample` samples, interpolated by `interp` (0 zero-order hold, 1 linear, 2 cubic).
#[derive(Clone, Debug)]
pub struct ActuatorSpec {
    pub name: Option<String>,
    pub r#type: i32,
    pub gaintype: i32,
    pub gainprm: [f64; NGAIN],
    pub biastype: i32,
    pub biasprm: [f64; NBIAS],
    pub dyntype: i32,
    pub dynprm: [f64; NDYN],
    pub actdim: i32,
    pub ctrlspec: i32,
    pub velrange: [f64; 2],
    pub ffrange: [f64; 2],
    pub actearly: bool,
    pub trntype: i32,
    pub gear: [f64; 6],
    pub target: Option<String>,
    pub refsite: Option<String>,
    pub slidersite: Option<String>,
    pub cranklength: f64,
    pub lengthrange: [f64; 2],
    pub inheritrange: f64,
    pub damping: [f64; NPOLY + 1],
    pub armature: f64,
    pub ctrllimited: i32,
    pub ctrlrange: [f64; 2],
    pub forcelimited: i32,
    pub forcerange: [f64; 2],
    pub actlimited: i32,
    pub actrange: [f64; 2],
    pub group: i32,
    pub nsample: i32,
    pub interp: i32,
    pub delay: f64,
    pub userdata: Vec<f64>,
}

impl Default for ActuatorSpec {
    fn default() -> Self {
        let mut gainprm = [0.0; NGAIN];
        gainprm[0] = 1.0;
        let mut dynprm = [0.0; NDYN];
        dynprm[0] = 1.0;
        let mut gear = [0.0; 6];
        gear[0] = 1.0;
        Self {
            name: None,
            r#type: ACTUATOR_GENERAL,
            gaintype: GAIN_FIXED,
            gainprm,
            biastype: BIAS_NONE,
            biasprm: [0.0; NBIAS],
            dyntype: DYN_NONE,
            dynprm,
            actdim: -1,
            ctrlspec: 0,
            velrange: [0.0; 2],
            ffrange: [0.0; 2],
            actearly: false,
            trntype: TRN_UNDEFINED,
            gear,
            target: None,
            refsite: None,
            slidersite: None,
            cranklength: 0.0,
            lengthrange: [0.0; 2],
            inheritrange: 0.0,
            damping: [0.0; NPOLY + 1],
            armature: 0.0,
            ctrllimited: 2,
            ctrlrange: [0.0; 2],
            forcelimited: 2,
            forcerange: [0.0; 2],
            actlimited: 2,
            actrange: [0.0; 2],
            group: 0,
            nsample: 0,
            interp: 0,
            delay: 0.0,
            userdata: Vec::new(),
        }
    }
}

/// The DC motor's parameters as `mjs_setToDCMotor` takes them; `None` is C's null pointer.
#[derive(Clone, Debug, Default)]
pub struct DcMotorSpec {
    pub motorconst: Option<[f64; 2]>,
    pub resistance: f64,
    pub nominal: Option<[f64; 3]>,
    pub saturation: Option<[f64; 3]>,
    pub inductance: Option<[f64; 2]>,
    pub cogging: Option<[f64; 3]>,
    pub controller: Option<[f64; 6]>,
    pub thermal: Option<[f64; 6]>,
    pub lugre: Option<[f64; 5]>,
    pub ctrlspec: i32,
}

fn refuse(err: &str) -> Result<(), String> {
    if err.is_empty() { Ok(()) } else { Err(err.into()) }
}

impl ActuatorSpec {
    /// A default actuator on this transmission (`trntype`, `target`).
    pub fn on(trntype: i32, target: &str) -> Self {
        Self { trntype, target: Some(target.into()), ..Self::default() }
    }

    // SetControl: a shortcut owns the control model and its activation layout.
    fn set_control(&mut self, r#type: i32) {
        self.gainprm = [0.0; NGAIN];
        self.biasprm = [0.0; NBIAS];
        self.dynprm = [0.0; NDYN];
        self.gainprm[0] = 1.0;
        self.dynprm[0] = 1.0;
        self.gaintype = GAIN_FIXED;
        self.biastype = BIAS_NONE;
        self.dyntype = DYN_NONE;
        self.ctrlspec = 0;
        self.actdim = -1;
        self.actearly = false;
        self.actlimited = 2;
        self.r#type = r#type;
    }

    /// mjs_setToMotor.
    pub fn set_to_motor(&mut self) -> Result<(), String> {
        self.set_control(ACTUATOR_MOTOR);
        Ok(())
    }

    /// mjs_setToPosition.
    pub fn set_to_position(&mut self, kp: f64, kv: Option<f64>, dampratio: Option<f64>, timeconst: Option<f64>, inheritrange: f64) -> Result<(), String> {
        self.set_control(ACTUATOR_POSITION);
        self.gainprm[0] = kp;
        self.biasprm[1] = -kp;
        if dampratio.is_some() && kv.is_some() { return refuse("kv and dampratio cannot both be defined"); }
        if let Some(kv) = kv {
            if kv < 0.0 { return refuse("kv cannot be negative"); }
            self.biasprm[2] = -kv;
        }
        if let Some(dampratio) = dampratio {
            if dampratio < 0.0 { return refuse("dampratio cannot be negative"); }
            self.biasprm[2] = dampratio;
        }
        if let Some(timeconst) = timeconst {
            if timeconst < 0.0 { return refuse("timeconst cannot be negative"); }
            self.dynprm[0] = timeconst;
            self.dyntype = if timeconst == 0.0 { DYN_NONE } else { DYN_FILTEREXACT };
        }
        self.inheritrange = inheritrange;
        if inheritrange > 0.0 && (self.ctrlrange[0] != 0.0 || self.ctrlrange[1] != 0.0) {
            return refuse("ctrlrange and inheritrange cannot both be defined");
        }
        self.ctrlspec = INPUT_POS;
        self.gaintype = GAIN_FIXED;
        self.biastype = BIAS_AFFINE;
        Ok(())
    }

    /// mjs_setToIntVelocity.
    pub fn set_to_int_velocity(&mut self, kp: f64, kv: Option<f64>, dampratio: Option<f64>, timeconst: Option<f64>, inheritrange: f64) -> Result<(), String> {
        // inheritrange sets actrange, not ctrlrange: skip the position range check
        self.set_to_position(kp, kv, dampratio, timeconst, 0.0)?;
        self.ctrlspec = INPUT_VEL;
        self.dyntype = DYN_INTEGRATOR;
        self.r#type = ACTUATOR_INTVELOCITY;
        self.inheritrange = inheritrange;
        if inheritrange > 0.0 && (self.actrange[0] != 0.0 || self.actrange[1] != 0.0) {
            return refuse("actrange and inheritrange cannot both be defined");
        }
        Ok(())
    }

    /// mjs_setToOrientation.
    pub fn set_to_orientation(&mut self, kp: f64, kv: Option<f64>, dampratio: Option<f64>, ctrlspec: i32) -> Result<(), String> {
        if kv.is_some() && dampratio.is_some() { return refuse("kv and dampratio cannot both be defined"); }
        self.set_control(ACTUATOR_ORIENTATION);
        self.gainprm[0] = kp;
        self.biasprm[1] = -kp;
        if let Some(kv) = kv {
            if kv < 0.0 { return refuse("kv cannot be negative"); }
            self.biasprm[2] = -kv;
        }
        if let Some(dampratio) = dampratio {
            if dampratio < 0.0 { return refuse("dampratio cannot be negative"); }
            self.biasprm[2] = dampratio;
        }
        self.ctrlspec = ctrlspec;
        self.gaintype = GAIN_SO3;
        self.biastype = BIAS_SO3;
        self.dyntype = DYN_NONE;
        Ok(())
    }

    /// mjs_setToPID.
    pub fn set_to_pid(&mut self, kp: f64, kv: Option<f64>, dampratio: Option<f64>, ki: Option<f64>, imax: Option<f64>, slewmax: Option<f64>, inheritrange: f64, ctrlspec: i32) -> Result<(), String> {
        if kv.is_some() && dampratio.is_some() { return refuse("kv and dampratio cannot both be defined"); }
        self.set_control(ACTUATOR_PID);
        self.biasprm[1] = -kp;
        if let Some(kv) = kv {
            if kv < 0.0 { return refuse("kv cannot be negative"); }
            self.biasprm[2] = -kv;
        }
        if let Some(dampratio) = dampratio {
            if dampratio < 0.0 { return refuse("dampratio cannot be negative"); }
            self.biasprm[2] = dampratio;
        }
        // controller states: ki in gainprm[0], imax in dynprm[0], slewmax in dynprm[1]
        let ki_value = ki.unwrap_or(0.0);
        let slew_value = slewmax.unwrap_or(0.0);
        if slew_value < 0.0 { return refuse("slewmax cannot be negative"); }
        self.gainprm[0] = ki_value;
        self.dynprm[1] = slew_value;
        self.dyntype = if ki_value != 0.0 || slew_value != 0.0 { DYN_PID } else { DYN_NONE };
        self.dynprm[0] = match imax { Some(imax) if ki_value != 0.0 => imax, _ => 0.0 };
        self.inheritrange = inheritrange;
        if inheritrange > 0.0 && (self.ctrlrange[0] != 0.0 || self.ctrlrange[1] != 0.0) {
            return refuse("posrange and inheritrange cannot both be defined");
        }
        self.ctrlspec = ctrlspec;
        self.gaintype = GAIN_PID;
        self.biastype = BIAS_AFFINE;
        Ok(())
    }

    /// mjs_setToVelocity.
    pub fn set_to_velocity(&mut self, kv: f64) -> Result<(), String> {
        self.set_control(ACTUATOR_VELOCITY);
        self.gainprm[0] = kv;
        self.biasprm[2] = -kv;
        self.ctrlspec = INPUT_VEL;
        self.biastype = BIAS_AFFINE;
        Ok(())
    }

    /// mjs_setToDamper.
    pub fn set_to_damper(&mut self, kv: f64) -> Result<(), String> {
        self.set_control(ACTUATOR_DAMPER);
        self.gainprm[0] = 0.0;
        self.gainprm[2] = -kv;
        self.ctrllimited = 1;
        self.gaintype = GAIN_AFFINE;
        if kv < 0.0 { return refuse("damping coefficient cannot be negative"); }
        if self.ctrlrange[0] < 0.0 || self.ctrlrange[1] < 0.0 { return refuse("damper control range cannot be negative"); }
        Ok(())
    }

    /// mjs_setToCylinder.
    pub fn set_to_cylinder(&mut self, timeconst: f64, bias: f64, area: f64, diameter: f64) -> Result<(), String> {
        self.set_control(ACTUATOR_CYLINDER);
        self.dynprm[0] = timeconst;
        self.biasprm[0] = bias;
        self.gainprm[0] = area;
        if diameter >= 0.0 { self.gainprm[0] = std::f64::consts::PI / 4.0 * diameter * diameter; }
        self.ctrlspec = INPUT_PRESSURE;
        self.dyntype = DYN_FILTER;
        self.biastype = BIAS_AFFINE;
        Ok(())
    }

    /// mjs_setToAdhesion.
    pub fn set_to_adhesion(&mut self, gain: f64) -> Result<(), String> {
        self.set_control(ACTUATOR_ADHESION);
        self.gainprm[0] = gain;
        self.ctrllimited = 1;
        if gain < 0.0 { return refuse("adhesion gain cannot be negative"); }
        if self.ctrlrange[0] < 0.0 || self.ctrlrange[1] < 0.0 { return refuse("adhesion control range cannot be negative"); }
        Ok(())
    }

    /// mjs_setToDCMotor.
    pub fn set_to_dc_motor(&mut self, p: &DcMotorSpec) -> Result<(), String> {
        let mut r = p.resistance; // electrical resistance
        let kt = p.motorconst.map_or(0.0, |k| k[0]); // torque constant
        let mut ke = p.motorconst.map_or(0.0, |k| k[1]); // back-EMF constant
        let vn = p.nominal.map_or(0.0, |n| n[0]); // nominal voltage
        let tau0 = p.nominal.map_or(0.0, |n| n[1]); // stall torque
        let omega0 = p.nominal.map_or(0.0, |n| n[2]); // no-load speed
        if kt < 0.0 || ke < 0.0 { return refuse("DC motor: motor constants must be non-negative"); }
        // derive Ke from nominal: omega0 = vn*Ke / (Ke^2 + R*B)
        if vn > 0.0 && ke <= 0.0 && omega0 > 0.0 {
            let b = self.damping[0];
            if b > 0.0 && r > 0.0 {
                let disc = vn * vn - 4.0 * r * b * omega0 * omega0;
                ke = if disc > 0.0 { (vn + disc.sqrt()) / (2.0 * omega0) } else { vn / omega0 };
            } else if b > 0.0 && tau0 > 0.0 {
                let ke_exact = vn / omega0 - vn * b / tau0;
                ke = if ke_exact > 0.0 { ke_exact } else { vn / omega0 };
            } else {
                ke = vn / omega0;
            }
        }
        // resolve effective motor constant K from [Kt, Ke]
        let k = if kt > 0.0 && ke > 0.0 { (kt * ke).sqrt() } else if kt > 0.0 { kt } else { ke };
        // derive R from nominal: tau0 = K*vn/R
        if r == 0.0 && vn > 0.0 && tau0 > 0.0 && k > 0.0 { r = k * vn / tau0; }
        // K = 0 decouples the electrical branch of a passive motor
        if k <= 0.0 && p.ctrlspec != INPUT_NONE { return refuse("DC motor: motor constant K must be positive"); }
        if r <= 0.0 { return refuse("DC motor: resistance R must be positive"); }
        self.set_control(ACTUATOR_DCMOTOR);
        self.dyntype = DYN_DCMOTOR;
        self.gaintype = GAIN_DCMOTOR;
        self.biastype = BIAS_DCMOTOR;
        // gainprm: [R, K, alpha, T0]
        self.gainprm[0] = r;
        self.gainprm[1] = k;
        // controller parameters: gainprm[4:6] for kp, ki, kd; dynprm[7,8] for slewmax, Imax
        let c = p.controller.unwrap_or([0.0; 6]);
        self.gainprm[4] = c[0];
        self.gainprm[5] = c[1];
        self.gainprm[6] = c[2];
        self.dynprm[7] = c[3];
        self.dynprm[8] = c[4];
        // controller parameters: gainprm[7] for v_max
        if c[5] > 0.0 { self.gainprm[7] = c[5]; }
        // saturation -> forcerange
        if let Some(sat) = p.saturation.filter(|s| s[0] > 0.0 || s[1] > 0.0) {
            let mut tau_max = sat[0];
            if tau_max == 0.0 && sat[1] > 0.0 {
                if k <= 0.0 { return refuse("DC motor: current limit requires a positive motor constant"); }
                tau_max = k * sat[1];
            }
            self.forcerange = [-tau_max, tau_max];
            self.forcelimited = 1;
        }
        // saturation: [tau_max, i_max, (di/dt)_max]
        if let Some(sat) = p.saturation.filter(|s| s[2] > 0.0) { self.dynprm[1] = sat[2]; }
        // cogging: [amplitude, periodicity, phase] -> biasprm[0:3]
        let cog = p.cogging.unwrap_or([0.0; 3]);
        self.biasprm[0] = cog[0];
        self.biasprm[1] = cog[1];
        self.biasprm[2] = cog[2];
        // count activation variables: slot order is slew, integral, temperature, bristle, current
        let mut actdim = 0;
        // inductance: [L, te]
        if p.inductance.is_some_and(|l| l[0] < 0.0) { return refuse("DC motor: inductance must be non-negative"); }
        if p.inductance.is_some_and(|l| l[1] < 0.0) { return refuse("DC motor: electrical time constant must be non-negative"); }
        let te = match p.inductance { Some(l) if l[0] > 0.0 => l[0] / r, Some(l) => l[1], None => 0.0 };
        self.dynprm[0] = te;
        if te > 0.0 { actdim += 1; }
        // controller states: slew rate limiting, integral
        if p.controller.is_some_and(|c| c[3] > 0.0) { actdim += 1; }
        if p.controller.is_some_and(|c| c[1] > 0.0) { actdim += 1; }
        // thermal -> temperature activation
        if let Some(th) = p.thermal.filter(|t| t[0] > 0.0 || t[1] > 0.0 || t[2] > 0.0) {
            let (mut rt, mut cap, tth, alpha, t0, ta) = (th[0], th[1], th[2], th[3], th[4], th[5]);
            // any two of RT, C, tth; tth takes precedence over C
            if tth > 0.0 && rt > 0.0 {
                cap = tth / rt;
            } else if tth > 0.0 && cap > 0.0 && rt == 0.0 {
                rt = tth / cap;
            }
            if rt <= 0.0 { return refuse("DC motor: thermal resistance must be positive"); }
            if cap <= 0.0 { return refuse("DC motor: thermal capacitance must be positive"); }
            self.dynprm[2] = rt;
            self.dynprm[3] = cap;
            self.dynprm[4] = ta;
            self.gainprm[2] = alpha;
            self.gainprm[3] = t0;
            actdim += 1;
        }
        // lugre: {stiffness, damping, coulomb, static, stribeck}
        if let Some(lu) = p.lugre.filter(|l| l[0] > 0.0) {
            self.dynprm[5] = lu[0];
            self.dynprm[6] = lu[1];
            self.biasprm[3] = lu[2];
            self.biasprm[4] = lu[3];
            self.biasprm[5] = lu[4];
            actdim += 1;
        }
        // set input mode and activation dimension
        self.gainprm[8] = 0.0;
        self.ctrlspec = p.ctrlspec;
        self.actdim = actdim;
        // enforce actlimited = 0; homogeneous bounds are invalid across DC motor states
        self.actlimited = 0;
        // DC motor always uses actearly
        self.actearly = true;
        Ok(())
    }
}

// mjs_defaultOrientation/Frame/Camera/Light/Flex/HField/Skin/Texture/Material,
// Sensor/Numeric/Text/Tuple/Key/Plugin are outside this subset.
