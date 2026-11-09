//! Builder inputs for the rigid-body subset, with explicit body ownership.
//!
//! Translated from `mjspec.h` and `src/user/user_init.c` of
//! the engine named in CREDITS.md (Apache-2.0, Copyright 2021 DeepMind
//! Technologies Limited; mjspec.h Copyright 2024 DeepMind Technologies Limited);
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
    pub meshes: Vec<MeshSpec>,
    pub bodies: Vec<BodySpec>,
    pub equalities: Vec<EqualitySpec>,
    pub pairs: Vec<PairSpec>,
    pub excludes: Vec<ExcludeSpec>,
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
            meshes: Vec::new(),
            bodies: vec![BodySpec::default()],
            equalities: Vec::new(),
            pairs: Vec::new(),
            excludes: Vec::new(),
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

// mjs_defaultOrientation/Frame/Camera/Light/Flex/HField/Skin/Texture/Material,
// Tendon/Actuator/Sensor/Numeric/Text/Tuple/Key/Plugin are outside this subset.
