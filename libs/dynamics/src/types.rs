//! Model and data constants.
//!
//! Translated from `mjtype.h`, `mjmodel.h`, `mjdata.h` and the main header of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

pub const MINVAL: f64 = 1E-15;       // minimum value in any denominator
pub const MAXVAL: f64 = 1E+10;       // maximum value in qpos, qvel, qacc
pub const PI: f64 = 3.14159265358979323846;

// mjtDisableBit
pub const DSBL_CONSTRAINT: i32 = 1<<0;
pub const DSBL_EQUALITY: i32 = 1<<1;
pub const DSBL_FRICTIONLOSS: i32 = 1<<2;
pub const DSBL_LIMIT: i32 = 1<<3;
pub const DSBL_CONTACT: i32 = 1<<4;
pub const DSBL_SPRING: i32 = 1<<5;
pub const DSBL_DAMPER: i32 = 1<<6;
pub const DSBL_GRAVITY: i32 = 1<<7;
pub const DSBL_CLAMPCTRL: i32 = 1<<8;
pub const DSBL_WARMSTART: i32 = 1<<9;
pub const DSBL_FILTERPARENT: i32 = 1<<10;
pub const DSBL_ACTUATION: i32 = 1<<11;
pub const DSBL_REFSAFE: i32 = 1<<12;
pub const DSBL_SENSOR: i32 = 1<<13;
pub const DSBL_MIDPHASE: i32 = 1<<14;
pub const DSBL_EULERDAMP: i32 = 1<<15;
pub const DSBL_AUTORESET: i32 = 1<<16;
pub const DSBL_NATIVECCD: i32 = 1<<17;
pub const DSBL_ISLAND: i32 = 1<<18;
pub const DSBL_MULTICCD: i32 = 1<<19;
pub const NDISABLE: i32 = 20;

// mjtEnableBit
pub const ENBL_OVERRIDE: i32 = 1<<0;
pub const ENBL_ENERGY: i32 = 1<<1;
pub const ENBL_FWDINV: i32 = 1<<2;
pub const ENBL_INVDISCRETE: i32 = 1<<3;
pub const ENBL_SLEEP: i32 = 1<<4;
pub const ENBL_DIAGEXACT: i32 = 1<<5;
pub const ENBL_IPC: i32 = 1<<6;
pub const ENBL_SPECULATIVE: i32 = 1<<7;
pub const NENABLE: i32 = 8;

// mjtJoint
pub const JNT_FREE: i32 = 0;
pub const JNT_BALL: i32 = JNT_FREE + 1;
pub const JNT_SLIDE: i32 = JNT_BALL + 1;
pub const JNT_HINGE: i32 = JNT_SLIDE + 1;

// mjtGeom
pub const GEOM_PLANE: i32 = 0;
pub const GEOM_HFIELD: i32 = GEOM_PLANE + 1;
pub const GEOM_SPHERE: i32 = GEOM_HFIELD + 1;
pub const GEOM_CAPSULE: i32 = GEOM_SPHERE + 1;
pub const GEOM_ELLIPSOID: i32 = GEOM_CAPSULE + 1;
pub const GEOM_CYLINDER: i32 = GEOM_ELLIPSOID + 1;
pub const GEOM_BOX: i32 = GEOM_CYLINDER + 1;
pub const GEOM_MESH: i32 = GEOM_BOX + 1;
pub const GEOM_SDF: i32 = GEOM_MESH + 1;
pub const NGEOMTYPES: i32 = GEOM_SDF + 1;
pub const GEOM_ARROW: i32 = 100;
pub const GEOM_ARROW1: i32 = GEOM_ARROW + 1;
pub const GEOM_ARROW2: i32 = GEOM_ARROW1 + 1;
pub const GEOM_LINE: i32 = GEOM_ARROW2 + 1;
pub const GEOM_LINEBOX: i32 = GEOM_LINE + 1;
pub const GEOM_FLEX: i32 = GEOM_LINEBOX + 1;
pub const GEOM_SKIN: i32 = GEOM_FLEX + 1;
pub const GEOM_LABEL: i32 = GEOM_SKIN + 1;
pub const GEOM_TRIANGLE: i32 = GEOM_LABEL + 1;
pub const GEOM_NONE: i32 = 1001;

// mjtProjection
pub const PROJ_PERSPECTIVE: i32 = 0;
pub const PROJ_ORTHOGRAPHIC: i32 = PROJ_PERSPECTIVE + 1;

// mjtCamLight
pub const CAMLIGHT_FIXED: i32 = 0;
pub const CAMLIGHT_TRACK: i32 = CAMLIGHT_FIXED + 1;
pub const CAMLIGHT_TRACKCOM: i32 = CAMLIGHT_TRACK + 1;
pub const CAMLIGHT_TARGETBODY: i32 = CAMLIGHT_TRACKCOM + 1;
pub const CAMLIGHT_TARGETBODYCOM: i32 = CAMLIGHT_TARGETBODY + 1;

// mjtLightType
pub const LIGHT_SPOT: i32 = 0;
pub const LIGHT_DIRECTIONAL: i32 = LIGHT_SPOT + 1;
pub const LIGHT_POINT: i32 = LIGHT_DIRECTIONAL + 1;
pub const LIGHT_IMAGE: i32 = LIGHT_POINT + 1;

// mjtTexture
pub const TEXTURE_2D: i32 = 0;
pub const TEXTURE_CUBE: i32 = TEXTURE_2D + 1;
pub const TEXTURE_SKYBOX: i32 = TEXTURE_CUBE + 1;

// mjtTextureRole
pub const TEXROLE_USER: i32 = 0;
pub const TEXROLE_RGB: i32 = TEXROLE_USER + 1;
pub const TEXROLE_OCCLUSION: i32 = TEXROLE_RGB + 1;
pub const TEXROLE_ROUGHNESS: i32 = TEXROLE_OCCLUSION + 1;
pub const TEXROLE_METALLIC: i32 = TEXROLE_ROUGHNESS + 1;
pub const TEXROLE_NORMAL: i32 = TEXROLE_METALLIC + 1;
pub const TEXROLE_OPACITY: i32 = TEXROLE_NORMAL + 1;
pub const TEXROLE_EMISSIVE: i32 = TEXROLE_OPACITY + 1;
pub const TEXROLE_RGBA: i32 = TEXROLE_EMISSIVE + 1;
pub const TEXROLE_ORM: i32 = TEXROLE_RGBA + 1;
pub const NTEXROLE: i32 = TEXROLE_ORM + 1;

// mjtColorSpace
pub const COLORSPACE_AUTO: i32 = 0;
pub const COLORSPACE_LINEAR: i32 = COLORSPACE_AUTO + 1;
pub const COLORSPACE_SRGB: i32 = COLORSPACE_LINEAR + 1;

// mjtIntegrator
pub const INT_EULER: i32 = 0;
pub const INT_RK4: i32 = INT_EULER + 1;
pub const INT_IMPLICIT: i32 = INT_RK4 + 1;
pub const INT_IMPLICITFAST: i32 = INT_IMPLICIT + 1;
pub const INT_DISCRETE: i32 = INT_IMPLICITFAST + 1;

// mjtCone
pub const CONE_PYRAMIDAL: i32 = 0;
pub const CONE_ELLIPTIC: i32 = CONE_PYRAMIDAL + 1;

// mjtJacobian
pub const JAC_DENSE: i32 = 0;
pub const JAC_SPARSE: i32 = JAC_DENSE + 1;
pub const JAC_AUTO: i32 = JAC_SPARSE + 1;

// mjtSolver
pub const SOL_PGS: i32 = 0;
pub const SOL_CG: i32 = SOL_PGS + 1;
pub const SOL_NEWTON: i32 = SOL_CG + 1;

// mjtEq
pub const EQ_CONNECT: i32 = 0;
pub const EQ_WELD: i32 = EQ_CONNECT + 1;
pub const EQ_JOINT: i32 = EQ_WELD + 1;
pub const EQ_TENDON: i32 = EQ_JOINT + 1;
pub const EQ_FLEX: i32 = EQ_TENDON + 1;
pub const EQ_FLEXVERT: i32 = EQ_FLEX + 1;
pub const EQ_FLEXSTRAIN: i32 = EQ_FLEXVERT + 1;
pub const EQ_DISTANCE: i32 = EQ_FLEXSTRAIN + 1;

// mjtTendon
pub const TENDON_FIXED: i32 = 0;
pub const TENDON_SPATIAL: i32 = TENDON_FIXED + 1;

// mjtWrap
pub const WRAP_NONE: i32 = 0;
pub const WRAP_JOINT: i32 = WRAP_NONE + 1;
pub const WRAP_PULLEY: i32 = WRAP_JOINT + 1;
pub const WRAP_SITE: i32 = WRAP_PULLEY + 1;
pub const WRAP_SPHERE: i32 = WRAP_SITE + 1;
pub const WRAP_CYLINDER: i32 = WRAP_SPHERE + 1;

// mjtTrn
pub const TRN_JOINT: i32 = 0;
pub const TRN_JOINTINPARENT: i32 = TRN_JOINT + 1;
pub const TRN_SLIDERCRANK: i32 = TRN_JOINTINPARENT + 1;
pub const TRN_TENDON: i32 = TRN_SLIDERCRANK + 1;
pub const TRN_SITE: i32 = TRN_TENDON + 1;
pub const TRN_BODY: i32 = TRN_SITE + 1;
pub const TRN_SO3: i32 = TRN_BODY + 1;
pub const TRN_UNDEFINED: i32 = 1000;

// mjtDyn
pub const DYN_NONE: i32 = 0;
pub const DYN_INTEGRATOR: i32 = DYN_NONE + 1;
pub const DYN_FILTER: i32 = DYN_INTEGRATOR + 1;
pub const DYN_FILTEREXACT: i32 = DYN_FILTER + 1;
pub const DYN_MUSCLE: i32 = DYN_FILTEREXACT + 1;
pub const DYN_DCMOTOR: i32 = DYN_MUSCLE + 1;
pub const DYN_PID: i32 = DYN_DCMOTOR + 1;
pub const DYN_USER: i32 = DYN_PID + 1;

// mjtGain
pub const GAIN_FIXED: i32 = 0;
pub const GAIN_AFFINE: i32 = GAIN_FIXED + 1;
pub const GAIN_MUSCLE: i32 = GAIN_AFFINE + 1;
pub const GAIN_DCMOTOR: i32 = GAIN_MUSCLE + 1;
pub const GAIN_SO3: i32 = GAIN_DCMOTOR + 1;
pub const GAIN_PID: i32 = GAIN_SO3 + 1;
pub const GAIN_USER: i32 = GAIN_PID + 1;

// mjtBias
pub const BIAS_NONE: i32 = 0;
pub const BIAS_AFFINE: i32 = BIAS_NONE + 1;
pub const BIAS_MUSCLE: i32 = BIAS_AFFINE + 1;
pub const BIAS_DCMOTOR: i32 = BIAS_MUSCLE + 1;
pub const BIAS_SO3: i32 = BIAS_DCMOTOR + 1;
pub const BIAS_USER: i32 = BIAS_SO3 + 1;

// mjtCtrlChart
pub const CHART_EXPMAP: i32 = 1;
pub const CHART_QUAT: i32 = 2;

// mjtCtrlInput
pub const INPUT_POS: i32 = 1;
pub const INPUT_VEL: i32 = 2;
pub const INPUT_FF: i32 = 4;
pub const INPUT_VOLTAGE: i32 = 8;
pub const INPUT_NONE: i32 = 16;
pub const INPUT_PRESSURE: i32 = 32;

// mjtObj
pub const OBJ_UNKNOWN: i32 = 0;
pub const OBJ_BODY: i32 = OBJ_UNKNOWN + 1;
pub const OBJ_XBODY: i32 = OBJ_BODY + 1;
pub const OBJ_JOINT: i32 = OBJ_XBODY + 1;
pub const OBJ_DOF: i32 = OBJ_JOINT + 1;
pub const OBJ_GEOM: i32 = OBJ_DOF + 1;
pub const OBJ_SITE: i32 = OBJ_GEOM + 1;
pub const OBJ_CAMERA: i32 = OBJ_SITE + 1;
pub const OBJ_LIGHT: i32 = OBJ_CAMERA + 1;
pub const OBJ_FLEX: i32 = OBJ_LIGHT + 1;
pub const OBJ_MESH: i32 = OBJ_FLEX + 1;
pub const OBJ_SKIN: i32 = OBJ_MESH + 1;
pub const OBJ_HFIELD: i32 = OBJ_SKIN + 1;
pub const OBJ_TEXTURE: i32 = OBJ_HFIELD + 1;
pub const OBJ_MATERIAL: i32 = OBJ_TEXTURE + 1;
pub const OBJ_PAIR: i32 = OBJ_MATERIAL + 1;
pub const OBJ_EXCLUDE: i32 = OBJ_PAIR + 1;
pub const OBJ_EQUALITY: i32 = OBJ_EXCLUDE + 1;
pub const OBJ_TENDON: i32 = OBJ_EQUALITY + 1;
pub const OBJ_ACTUATOR: i32 = OBJ_TENDON + 1;
pub const OBJ_SENSOR: i32 = OBJ_ACTUATOR + 1;
pub const OBJ_NUMERIC: i32 = OBJ_SENSOR + 1;
pub const OBJ_TEXT: i32 = OBJ_NUMERIC + 1;
pub const OBJ_TUPLE: i32 = OBJ_TEXT + 1;
pub const OBJ_KEY: i32 = OBJ_TUPLE + 1;
pub const OBJ_PLUGIN: i32 = OBJ_KEY + 1;
pub const NOBJECT: i32 = OBJ_PLUGIN + 1;
pub const OBJ_FRAME: i32 = 100;
pub const OBJ_DEFAULT: i32 = OBJ_FRAME + 1;
pub const OBJ_MODEL: i32 = OBJ_DEFAULT + 1;

// mjtSensor
pub const SENS_TOUCH: i32 = 0;
pub const SENS_ACCELEROMETER: i32 = SENS_TOUCH + 1;
pub const SENS_VELOCIMETER: i32 = SENS_ACCELEROMETER + 1;
pub const SENS_GYRO: i32 = SENS_VELOCIMETER + 1;
pub const SENS_FORCE: i32 = SENS_GYRO + 1;
pub const SENS_TORQUE: i32 = SENS_FORCE + 1;
pub const SENS_MAGNETOMETER: i32 = SENS_TORQUE + 1;
pub const SENS_RANGEFINDER: i32 = SENS_MAGNETOMETER + 1;
pub const SENS_CAMPROJECTION: i32 = SENS_RANGEFINDER + 1;
pub const SENS_JOINTPOS: i32 = SENS_CAMPROJECTION + 1;
pub const SENS_JOINTVEL: i32 = SENS_JOINTPOS + 1;
pub const SENS_TENDONPOS: i32 = SENS_JOINTVEL + 1;
pub const SENS_TENDONVEL: i32 = SENS_TENDONPOS + 1;
pub const SENS_ACTUATORPOS: i32 = SENS_TENDONVEL + 1;
pub const SENS_ACTUATORVEL: i32 = SENS_ACTUATORPOS + 1;
pub const SENS_ACTUATORFRC: i32 = SENS_ACTUATORVEL + 1;
pub const SENS_JOINTACTFRC: i32 = SENS_ACTUATORFRC + 1;
pub const SENS_TENDONACTFRC: i32 = SENS_JOINTACTFRC + 1;
pub const SENS_BALLQUAT: i32 = SENS_TENDONACTFRC + 1;
pub const SENS_BALLANGVEL: i32 = SENS_BALLQUAT + 1;
pub const SENS_JOINTLIMITPOS: i32 = SENS_BALLANGVEL + 1;
pub const SENS_JOINTLIMITVEL: i32 = SENS_JOINTLIMITPOS + 1;
pub const SENS_JOINTLIMITFRC: i32 = SENS_JOINTLIMITVEL + 1;
pub const SENS_TENDONLIMITPOS: i32 = SENS_JOINTLIMITFRC + 1;
pub const SENS_TENDONLIMITVEL: i32 = SENS_TENDONLIMITPOS + 1;
pub const SENS_TENDONLIMITFRC: i32 = SENS_TENDONLIMITVEL + 1;
pub const SENS_FRAMEPOS: i32 = SENS_TENDONLIMITFRC + 1;
pub const SENS_FRAMEQUAT: i32 = SENS_FRAMEPOS + 1;
pub const SENS_FRAMEXAXIS: i32 = SENS_FRAMEQUAT + 1;
pub const SENS_FRAMEYAXIS: i32 = SENS_FRAMEXAXIS + 1;
pub const SENS_FRAMEZAXIS: i32 = SENS_FRAMEYAXIS + 1;
pub const SENS_FRAMELINVEL: i32 = SENS_FRAMEZAXIS + 1;
pub const SENS_FRAMEANGVEL: i32 = SENS_FRAMELINVEL + 1;
pub const SENS_FRAMELINACC: i32 = SENS_FRAMEANGVEL + 1;
pub const SENS_FRAMEANGACC: i32 = SENS_FRAMELINACC + 1;
pub const SENS_SUBTREECOM: i32 = SENS_FRAMEANGACC + 1;
pub const SENS_SUBTREELINVEL: i32 = SENS_SUBTREECOM + 1;
pub const SENS_SUBTREEANGMOM: i32 = SENS_SUBTREELINVEL + 1;
pub const SENS_INSIDESITE: i32 = SENS_SUBTREEANGMOM + 1;
pub const SENS_GEOMDIST: i32 = SENS_INSIDESITE + 1;
pub const SENS_GEOMNORMAL: i32 = SENS_GEOMDIST + 1;
pub const SENS_GEOMFROMTO: i32 = SENS_GEOMNORMAL + 1;
pub const SENS_CONTACT: i32 = SENS_GEOMFROMTO + 1;
pub const SENS_E_POTENTIAL: i32 = SENS_CONTACT + 1;
pub const SENS_E_KINETIC: i32 = SENS_E_POTENTIAL + 1;
pub const SENS_CLOCK: i32 = SENS_E_KINETIC + 1;
pub const SENS_TACTILE: i32 = SENS_CLOCK + 1;
pub const SENS_PLUGIN: i32 = SENS_TACTILE + 1;
pub const SENS_USER: i32 = SENS_PLUGIN + 1;

// mjtStage
pub const STAGE_NONE: i32 = 0;
pub const STAGE_POS: i32 = STAGE_NONE + 1;
pub const STAGE_VEL: i32 = STAGE_POS + 1;
pub const STAGE_ACC: i32 = STAGE_VEL + 1;

// mjtDataType
pub const DATATYPE_REAL: i32 = 0;
pub const DATATYPE_POSITIVE: i32 = DATATYPE_REAL + 1;
pub const DATATYPE_AXIS: i32 = DATATYPE_POSITIVE + 1;
pub const DATATYPE_QUATERNION: i32 = DATATYPE_AXIS + 1;

// mjtConDataField
pub const CONDATA_FOUND: i32 = 0;
pub const CONDATA_FORCE: i32 = CONDATA_FOUND + 1;
pub const CONDATA_TORQUE: i32 = CONDATA_FORCE + 1;
pub const CONDATA_DIST: i32 = CONDATA_TORQUE + 1;
pub const CONDATA_POS: i32 = CONDATA_DIST + 1;
pub const CONDATA_NORMAL: i32 = CONDATA_POS + 1;
pub const CONDATA_TANGENT: i32 = CONDATA_NORMAL + 1;
pub const CONDATA_LINVEL: i32 = CONDATA_TANGENT + 1;
pub const CONDATA_ANGVEL: i32 = CONDATA_LINVEL + 1;
pub const NCONDATA: i32 = CONDATA_ANGVEL + 1;

// mjtRayDataField
pub const RAYDATA_DIST: i32 = 0;
pub const RAYDATA_DIR: i32 = RAYDATA_DIST + 1;
pub const RAYDATA_ORIGIN: i32 = RAYDATA_DIR + 1;
pub const RAYDATA_POINT: i32 = RAYDATA_ORIGIN + 1;
pub const RAYDATA_NORMAL: i32 = RAYDATA_POINT + 1;
pub const RAYDATA_DEPTH: i32 = RAYDATA_NORMAL + 1;
pub const NRAYDATA: i32 = RAYDATA_DEPTH + 1;

// mjtCamOutBit
pub const CAMOUT_RGB: i32 = 1<<0;
pub const CAMOUT_DEPTH: i32 = 1<<1;
pub const CAMOUT_DIST: i32 = 1<<2;
pub const CAMOUT_NORMAL: i32 = 1<<3;
pub const CAMOUT_SEG: i32 = 1<<4;
pub const NCAMOUT: i32 = 5;

// mjtSameFrame
pub const SAMEFRAME_NONE: i32 = 0;
pub const SAMEFRAME_BODY: i32 = SAMEFRAME_NONE + 1;
pub const SAMEFRAME_INERTIA: i32 = SAMEFRAME_BODY + 1;
pub const SAMEFRAME_BODYROT: i32 = SAMEFRAME_INERTIA + 1;
pub const SAMEFRAME_INERTIAROT: i32 = SAMEFRAME_BODYROT + 1;

// mjtSleepPolicy
pub const SLEEP_AUTO: i32 = 0;
pub const SLEEP_AUTO_NEVER: i32 = SLEEP_AUTO + 1;
pub const SLEEP_AUTO_ALLOWED: i32 = SLEEP_AUTO_NEVER + 1;
pub const SLEEP_NEVER: i32 = SLEEP_AUTO_ALLOWED + 1;
pub const SLEEP_ALLOWED: i32 = SLEEP_NEVER + 1;
pub const SLEEP_INIT: i32 = SLEEP_ALLOWED + 1;

// mjtLRMode
pub const LRMODE_NONE: i32 = 0;
pub const LRMODE_MUSCLE: i32 = LRMODE_NONE + 1;
pub const LRMODE_MUSCLEUSER: i32 = LRMODE_MUSCLE + 1;
pub const LRMODE_ALL: i32 = LRMODE_MUSCLEUSER + 1;

// mjtFlexSelf
pub const FLEXSELF_NONE: i32 = 0;
pub const FLEXSELF_NARROW: i32 = FLEXSELF_NONE + 1;
pub const FLEXSELF_BVH: i32 = FLEXSELF_NARROW + 1;
pub const FLEXSELF_SAP: i32 = FLEXSELF_BVH + 1;
pub const FLEXSELF_AUTO: i32 = FLEXSELF_SAP + 1;

// mjtSDFType
pub const SDFTYPE_SINGLE: i32 = 0;
pub const SDFTYPE_INTERSECTION: i32 = SDFTYPE_SINGLE + 1;
pub const SDFTYPE_MIDSURFACE: i32 = SDFTYPE_INTERSECTION + 1;
pub const SDFTYPE_COLLISION: i32 = SDFTYPE_MIDSURFACE + 1;

// mjtState
pub const STATE_TIME: i32 = 1<<0;
pub const STATE_QPOS: i32 = 1<<1;
pub const STATE_QVEL: i32 = 1<<2;
pub const STATE_ACT: i32 = 1<<3;
pub const STATE_HISTORY: i32 = 1<<4;
pub const STATE_WARMSTART: i32 = 1<<5;
pub const STATE_CTRL: i32 = 1<<6;
pub const STATE_QFRC_APPLIED: i32 = 1<<7;
pub const STATE_XFRC_APPLIED: i32 = 1<<8;
pub const STATE_EQ_ACTIVE: i32 = 1<<9;
pub const STATE_MOCAP_POS: i32 = 1<<10;
pub const STATE_MOCAP_QUAT: i32 = 1<<11;
pub const STATE_USERDATA: i32 = 1<<12;
pub const STATE_PLUGIN: i32 = 1<<13;
pub const STATE_PLASTIC: i32 = 1<<14;
pub const NSTATE: i32 = 15;
pub const STATE_PHYSICS: i32 = STATE_QPOS | STATE_QVEL | STATE_ACT | STATE_HISTORY | STATE_PLASTIC;
pub const STATE_FULLPHYSICS: i32 = STATE_TIME | STATE_PHYSICS | STATE_PLUGIN;
pub const STATE_USER: i32 = STATE_CTRL | STATE_QFRC_APPLIED | STATE_XFRC_APPLIED | STATE_EQ_ACTIVE | STATE_MOCAP_POS | STATE_MOCAP_QUAT | STATE_USERDATA;
pub const STATE_INTEGRATION: i32 = STATE_FULLPHYSICS | STATE_USER | STATE_WARMSTART;

// mjtConstraint
pub const CNSTR_EQUALITY: i32 = 0;
pub const CNSTR_FRICTION_DOF: i32 = CNSTR_EQUALITY + 1;
pub const CNSTR_FRICTION_TENDON: i32 = CNSTR_FRICTION_DOF + 1;
pub const CNSTR_LIMIT_JOINT: i32 = CNSTR_FRICTION_TENDON + 1;
pub const CNSTR_LIMIT_TENDON: i32 = CNSTR_LIMIT_JOINT + 1;
pub const CNSTR_CONTACT_FRICTIONLESS: i32 = CNSTR_LIMIT_TENDON + 1;
pub const CNSTR_CONTACT_PYRAMIDAL: i32 = CNSTR_CONTACT_FRICTIONLESS + 1;
pub const CNSTR_CONTACT_ELLIPTIC: i32 = CNSTR_CONTACT_PYRAMIDAL + 1;

// mjtConstraintState
pub const CNSTRSTATE_SATISFIED: i32 = 0;
pub const CNSTRSTATE_QUADRATIC: i32 = CNSTRSTATE_SATISFIED + 1;
pub const CNSTRSTATE_LINEARNEG: i32 = CNSTRSTATE_QUADRATIC + 1;
pub const CNSTRSTATE_LINEARPOS: i32 = CNSTRSTATE_LINEARNEG + 1;
pub const CNSTRSTATE_CONE: i32 = CNSTRSTATE_LINEARPOS + 1;

// mjtWarning
pub const WARN_INERTIA: i32 = 0;
pub const WARN_CONTACTFULL: i32 = WARN_INERTIA + 1;
pub const WARN_CNSTRFULL: i32 = WARN_CONTACTFULL + 1;
pub const WARN_BADQPOS: i32 = WARN_CNSTRFULL + 1;
pub const WARN_BADQVEL: i32 = WARN_BADQPOS + 1;
pub const WARN_BADQACC: i32 = WARN_BADQVEL + 1;
pub const WARN_BADCTRL: i32 = WARN_BADQACC + 1;
pub const NWARNING: i32 = WARN_BADCTRL + 1;

// mjtTimer
pub const TIMER_STEP: i32 = 0;
pub const TIMER_FORWARD: i32 = TIMER_STEP + 1;
pub const TIMER_INVERSE: i32 = TIMER_FORWARD + 1;
pub const TIMER_POSITION: i32 = TIMER_INVERSE + 1;
pub const TIMER_VELOCITY: i32 = TIMER_POSITION + 1;
pub const TIMER_ACTUATION: i32 = TIMER_VELOCITY + 1;
pub const TIMER_CONSTRAINT: i32 = TIMER_ACTUATION + 1;
pub const TIMER_ADVANCE: i32 = TIMER_CONSTRAINT + 1;
pub const TIMER_POS_KINEMATICS: i32 = TIMER_ADVANCE + 1;
pub const TIMER_POS_INERTIA: i32 = TIMER_POS_KINEMATICS + 1;
pub const TIMER_POS_COLLISION: i32 = TIMER_POS_INERTIA + 1;
pub const TIMER_POS_MAKE: i32 = TIMER_POS_COLLISION + 1;
pub const TIMER_POS_PROJECT: i32 = TIMER_POS_MAKE + 1;
pub const TIMER_COL_BROAD: i32 = TIMER_POS_PROJECT + 1;
pub const TIMER_COL_NARROW: i32 = TIMER_COL_BROAD + 1;
pub const NTIMER: i32 = TIMER_COL_NARROW + 1;

// mjtSleepState
pub const S_STATIC: i32 = -1;
pub const S_ASLEEP: i32 = 0;
pub const S_AWAKE: i32 = 1;

// mjtLogLevel
pub const LOG_DEBUG: i32 = 0;
pub const LOG_INFO: i32 = LOG_DEBUG + 1;
pub const LOG_WARNING: i32 = LOG_INFO + 1;
pub const LOG_ERROR: i32 = LOG_WARNING + 1;

// mjtLogTopic
pub const TOPIC_NONE: i32 = 0;
pub const TOPIC_TIME_STP: i32 = 1;
pub const TOPIC_TIME_CMP: i32 = 2;
pub const TOPIC_SLEEP: i32 = 3;
pub const NTOPIC: i32 = 3;
pub const MINMU: f64 = 1E-5;      // minimum friction coefficient
pub const MINIMP: f64 = 0.0001;    // minimum constraint impedance
pub const MAXIMP: f64 = 0.9999;    // maximum constraint impedance
pub const MAXCONPAIR: usize = 50;        // maximum number of contacts per geom pair
pub const MAXTREEDEPTH: usize = 50;        // maximum bounding volume hierarchy depth
pub const MAXFLEXNODES: usize = 27;        // maximum number of flex nodes
pub const MINAWAKE: usize = 10;        // minimum number of timesteps before sleeping
pub const NEQDATA: usize = 12;        // number of eq_data fields
pub const NDYN: usize = 10;        // number of actuator dynamics parameters
pub const NGAIN: usize = 10;        // number of actuator gain parameters
pub const NBIAS: usize = 10;        // number of actuator bias parameters
pub const NFLUID: usize = 12;        // number of fluid interaction parameters
pub const NREF: usize = 2;         // number of solver reference parameters
pub const NIMP: usize = 5;         // number of solver impedance parameters
pub const NPOLY: usize = 2;         // number of high-order polynomial coefficients
pub const NSENS: usize = 3;         // number of sensor parameters
pub const NSOLVER: usize = 200;       // size of one mjData.solver array
pub const NISLAND: usize = 20;        // number of mjData.solver arrays
pub const VERSION_HEADER: usize = 4000000;
