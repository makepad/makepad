//! Deterministic MJCF export of the rigid-body model specification.
//!
//! Attribute spellings and enum mappings follow `src/xml/xml_native_reader.cc`
//! and its generated tables of the engine named in CREDITS.md (Apache-2.0,
//! Copyright 2021 DeepMind Technologies Limited); changed: Rust writer.

use crate::model::Opt;
use crate::model_spec::*;
use crate::types::*;
use std::fmt::{Debug, Write};

struct Xml(String);

impl Xml {
    fn start(&mut self, tag: &str) {
        write!(self.0, "<{tag}").unwrap();
    }

    fn text(&mut self, key: &str, value: &str) {
        write!(self.0, " {key}=\"").unwrap();
        for c in value.chars() {
            let escaped = match c {
                '&' => "&amp;", '<' => "&lt;", '>' => "&gt;", '\"' => "&quot;",
                '\'' => "&apos;", '\n' => "&#10;", '\r' => "&#13;", '\t' => "&#9;",
                _ => { self.0.push(c); continue; }
            };
            self.0.push_str(escaped);
        }
        self.0.push('"');
    }

    fn name(&mut self, key: &str, value: &Option<String>) {
        if let Some(value) = value { self.text(key, value); }
    }

    fn number<T: Debug>(&mut self, key: &str, value: T) {
        write!(self.0, " {key}=\"{value:?}\"").unwrap();
    }

    fn array<T: Debug>(&mut self, key: &str, values: &[T]) {
        write!(self.0, " {key}=\"").unwrap();
        for (i, value) in values.iter().enumerate() {
            if i != 0 { self.0.push(' '); }
            write!(self.0, "{value:?}").unwrap();
        }
        self.0.push('"');
    }

    fn end(&mut self) { self.0.push_str("/>\n"); }
    fn open(&mut self) { self.0.push_str(">\n"); }
    fn close(&mut self, tag: &str) { writeln!(self.0, "</{tag}>").unwrap(); }
}

fn keyword<'a>(value: i32, words: &[&'a str]) -> &'a str {
    *words.get(usize::try_from(value).expect("negative enum value"))
        .expect("enum value outside MJCF subset")
}

fn shape(value: i32) -> &'static str {
    match value {
        GEOM_PLANE => "plane", GEOM_SPHERE => "sphere", GEOM_CAPSULE => "capsule",
        GEOM_ELLIPSOID => "ellipsoid", GEOM_CYLINDER => "cylinder", GEOM_BOX => "box",
        GEOM_MESH => "mesh",
        _ => panic!("geom type outside the model specification"),
    }
}

fn option(x: &mut Xml, o: &Opt) {
    x.start("option");
    x.number("timestep", o.timestep);
    x.number("impratio", o.impratio);
    x.number("tolerance", o.tolerance);
    x.number("ls_tolerance", o.ls_tolerance);
    x.number("noslip_tolerance", o.noslip_tolerance);
    x.number("ccd_tolerance", o.ccd_tolerance);
    x.number("sleep_tolerance", o.sleep_tolerance);
    x.number("density", o.density);
    x.number("viscosity", o.viscosity);
    x.number("o_margin", o.o_margin);
    x.number("iterations", o.iterations);
    x.number("ls_iterations", o.ls_iterations);
    x.number("noslip_iterations", o.noslip_iterations);
    x.number("ccd_iterations", o.ccd_iterations);
    x.number("sdf_initpoints", o.sdf_initpoints);
    x.number("sdf_iterations", o.sdf_iterations);
    x.array("gravity", &o.gravity);
    x.array("wind", &o.wind);
    x.array("magnetic", &o.magnetic);
    x.array("o_solref", &o.o_solref);
    x.array("o_solimp", &o.o_solimp);
    x.array("o_friction", &o.o_friction);
    x.text("integrator", keyword(o.integrator, &["Euler", "RK4", "implicit", "implicitfast", "discrete"]));
    x.text("cone", keyword(o.cone, &["pyramidal", "elliptic"]));
    x.text("jacobian", keyword(o.jacobian, &["dense", "sparse", "auto"]));
    x.text("solver", keyword(o.solver, &["PGS", "CG", "Newton"]));
    assert!(o.disableactuator >= 0, "actuator groups are limited to bits 0..30 by MJCF");
    let groups: Vec<_> = (0..31).filter(|i| o.disableactuator & (1 << i) != 0).collect();
    x.array("actuatorgroupdisable", &groups);
    x.open();
    x.start("flag");
    assert_eq!(o.disableflags & !((1 << 20) - 1), 0, "unknown disableflags");
    x.text("constraint", if o.disableflags & DSBL_CONSTRAINT == 0 { "enable" } else { "disable" });
    x.text("equality", if o.disableflags & DSBL_EQUALITY == 0 { "enable" } else { "disable" });
    x.text("frictionloss", if o.disableflags & DSBL_FRICTIONLOSS == 0 { "enable" } else { "disable" });
    x.text("limit", if o.disableflags & DSBL_LIMIT == 0 { "enable" } else { "disable" });
    x.text("contact", if o.disableflags & DSBL_CONTACT == 0 { "enable" } else { "disable" });
    x.text("spring", if o.disableflags & DSBL_SPRING == 0 { "enable" } else { "disable" });
    x.text("damper", if o.disableflags & DSBL_DAMPER == 0 { "enable" } else { "disable" });
    x.text("gravity", if o.disableflags & DSBL_GRAVITY == 0 { "enable" } else { "disable" });
    x.text("clampctrl", if o.disableflags & DSBL_CLAMPCTRL == 0 { "enable" } else { "disable" });
    x.text("warmstart", if o.disableflags & DSBL_WARMSTART == 0 { "enable" } else { "disable" });
    x.text("filterparent", if o.disableflags & DSBL_FILTERPARENT == 0 { "enable" } else { "disable" });
    x.text("actuation", if o.disableflags & DSBL_ACTUATION == 0 { "enable" } else { "disable" });
    x.text("refsafe", if o.disableflags & DSBL_REFSAFE == 0 { "enable" } else { "disable" });
    x.text("sensor", if o.disableflags & DSBL_SENSOR == 0 { "enable" } else { "disable" });
    x.text("midphase", if o.disableflags & DSBL_MIDPHASE == 0 { "enable" } else { "disable" });
    x.text("eulerdamp", if o.disableflags & DSBL_EULERDAMP == 0 { "enable" } else { "disable" });
    x.text("autoreset", if o.disableflags & DSBL_AUTORESET == 0 { "enable" } else { "disable" });
    x.text("nativeccd", if o.disableflags & DSBL_NATIVECCD == 0 { "enable" } else { "disable" });
    x.text("island", if o.disableflags & DSBL_ISLAND == 0 { "enable" } else { "disable" });
    x.text("multiccd", if o.disableflags & DSBL_MULTICCD == 0 { "enable" } else { "disable" });
    assert_eq!(o.enableflags & !((1 << 8) - 1), 0, "unknown enableflags");
    x.text("override", if o.enableflags & ENBL_OVERRIDE != 0 { "enable" } else { "disable" });
    x.text("energy", if o.enableflags & ENBL_ENERGY != 0 { "enable" } else { "disable" });
    x.text("fwdinv", if o.enableflags & ENBL_FWDINV != 0 { "enable" } else { "disable" });
    x.text("invdiscrete", if o.enableflags & ENBL_INVDISCRETE != 0 { "enable" } else { "disable" });
    x.text("sleep", if o.enableflags & ENBL_SLEEP != 0 { "enable" } else { "disable" });
    x.text("diagexact", if o.enableflags & ENBL_DIAGEXACT != 0 { "enable" } else { "disable" });
    x.text("ipc", if o.enableflags & ENBL_IPC != 0 { "enable" } else { "disable" });
    x.text("speculative", if o.enableflags & ENBL_SPECULATIVE != 0 { "enable" } else { "disable" });
    x.end();
    x.close("option");
}

fn joint(x: &mut Xml, j: &JointSpec) {
    if j.align != 2 {
        // MJCF exposes align only on freejoint, which has no physical attributes.
        // Refuse combinations that this shortcut would silently discard.
        let d = JointSpec::default();
        assert!(j.r#type == JNT_FREE && j.pos == d.pos && j.axis == d.axis
            && j.r#ref == d.r#ref && j.stiffness == d.stiffness
            && j.springref == d.springref && j.springyield == d.springyield
            && j.springdamper == d.springdamper && j.limited == d.limited
            && j.range == d.range && j.margin == d.margin
            && j.solref_limit == d.solref_limit && j.solimp_limit == d.solimp_limit
            && j.actfrclimited == d.actfrclimited && j.actfrcrange == d.actfrcrange
            && j.armature == d.armature && j.damping == d.damping
            && j.frictionloss == d.frictionloss && j.solref_friction == d.solref_friction
            && j.solimp_friction == d.solimp_friction && !j.actgravcomp
            && j.userdata.is_empty(), "per-joint align requires the default freejoint shortcut");
        x.start("freejoint");
        x.name("name", &j.name);
        x.number("group", j.group);
        x.text("align", keyword(j.align, &["false", "true", "auto"]));
        x.end();
        return;
    }
    x.start("joint");
    x.name("name", &j.name);
    x.text("type", keyword(j.r#type, &["free", "ball", "slide", "hinge"]));
    x.number("group", j.group);
    x.number("margin", j.margin);
    x.number("ref", j.r#ref);
    x.number("springref", j.springref);
    x.number("springyield", j.springyield);
    x.number("armature", j.armature);
    x.number("frictionloss", j.frictionloss);
    x.number("actuatorgravcomp", j.actgravcomp);
    x.array("pos", &j.pos);
    x.array("axis", &j.axis);
    x.array("springdamper", &j.springdamper);
    x.array("solreflimit", &j.solref_limit);
    x.array("solimplimit", &j.solimp_limit);
    x.array("solreffriction", &j.solref_friction);
    x.array("solimpfriction", &j.solimp_friction);
    x.array("stiffness", &j.stiffness);
    x.array("range", &j.range);
    x.array("actuatorfrcrange", &j.actfrcrange);
    x.array("damping", &j.damping);
    x.array("user", &j.userdata);
    x.text("limited", keyword(j.limited, &["false", "true", "auto"]));
    x.text("actuatorfrclimited", keyword(j.actfrclimited, &["false", "true", "auto"]));
    x.end();
}

fn frame(x: &mut Xml, pos: &[f64; 3], quat: &[f64; 4], fromto: &[f64; 6]) {
    x.array("pos", pos);
    x.array("quat", quat);
    // The reference accepts the default pos/quat alongside fromto, then
    // computes its frame from the endpoints. Nonzero pos is a compiler error.
    if !fromto[0].is_nan() { x.array("fromto", fromto); }
}

fn geom(x: &mut Xml, g: &GeomSpec) {
    x.start("geom");
    x.name("name", &g.name);
    x.text("type", shape(g.r#type));
    x.name("mesh", &g.meshname);
    frame(x, &g.pos, &g.quat, &g.fromto);
    x.number("contype", g.contype);
    x.number("conaffinity", g.conaffinity);
    x.number("condim", g.condim);
    x.number("priority", g.priority);
    x.number("solmix", g.solmix);
    x.number("margin", g.margin);
    x.number("gap", g.gap);
    x.number("adhesion", g.adhesion);
    x.number("group", g.group);
    x.number("castshadow", g.castshadow);
    x.array("size", &g.size);
    x.array("friction", &g.friction);
    x.array("solref", &g.solref);
    x.array("solimp", &g.solimp);
    x.array("surfacevel", &g.surfacevel);
    x.array("fluidcoef", &g.fluid_coefs);
    x.array("rgba", &g.rgba);
    x.array("user", &g.userdata);
    // NaN means absent, not a literal NaN: the XML reader warns on literal NaNs.
    // mass/density are alternatives; mass wins in the reference compiler too.
    if g.mass.is_nan() { x.number("density", g.density); }
    else { x.number("mass", g.mass); }
    x.text("shellinertia", keyword(g.typeinertia, &["false", "true"]));
    assert!(g.fluid_ellipsoid == 0.0 || g.fluid_ellipsoid == 1.0,
        "fluidshape has only none and ellipsoid values in MJCF");
    x.text("fluidshape", if g.fluid_ellipsoid == 0.0 { "none" } else { "ellipsoid" });
    x.end();
}

fn site(x: &mut Xml, s: &SiteSpec) {
    x.start("site");
    x.name("name", &s.name);
    x.text("type", shape(s.r#type));
    frame(x, &s.pos, &s.quat, &s.fromto);
    x.array("size", &s.size);
    x.number("group", s.group);
    x.array("rgba", &s.rgba);
    x.array("user", &s.userdata);
    x.end();
}

fn body(x: &mut Xml, b: &BodySpec) {
    x.start("body");
    x.name("name", &b.name);
    x.array("pos", &b.pos);
    x.array("quat", &b.quat);
    x.number("mocap", b.mocap);
    x.number("gravcomp", b.gravcomp);
    x.text("sleep", match b.sleep {
        SLEEP_AUTO => "auto", SLEEP_NEVER => "never",
        SLEEP_ALLOWED => "allowed", SLEEP_INIT => "init",
        _ => panic!("internal sleep state is not an authored policy"),
    });
    x.text("simple", keyword(b.simple as i32, &["false", "auto"]));
    x.text("fuse", keyword(b.fuse as i32, &["false", "auto"]));
    x.array("user", &b.userdata);
    x.open();
    x.start("inertial");
    // With inertiafromgeom=false, an unset ipos copies the body frame (mjCBody::Compile).
    let (ipos, iquat) = if b.ipos[0].is_nan() { (&b.pos, &b.quat) } else { (&b.ipos, &b.iquat) };
    x.array("pos", ipos);
    x.number("mass", b.mass);
    if b.fullinertia[0].is_nan() {
        x.array("quat", iquat);
        x.array("diaginertia", &b.inertia);
    } else {
        assert!(!b.ipos[0].is_nan(), "fullinertia requires an explicit inertial position");
        assert_eq!(*iquat, [1.0, 0.0, 0.0, 0.0], "MJCF fullinertia is in the body frame");
        assert_eq!(b.inertia, [0.0; 3], "full and diagonal inertia are mutually exclusive");
        x.array("fullinertia", &b.fullinertia);
    }
    x.end();
}

fn equality(x: &mut Xml, e: &EqualitySpec) {
    let tag = match e.r#type {
        EQ_CONNECT => "connect", EQ_WELD => "weld", EQ_JOINT => "joint",
        _ => panic!("equality type outside the rigid-body specification"),
    };
    x.start(tag);
    x.name("name", &e.name);
    x.number("active", e.active);
    x.array("solref", &e.solref);
    x.array("solimp", &e.solimp);
    if e.r#type == EQ_JOINT {
        x.name("joint1", &e.name1);
        x.name("joint2", &e.name2);
        x.array("polycoef", &e.data[..5]);
    } else {
        assert!(matches!(e.objtype, OBJ_UNKNOWN | OBJ_BODY | OBJ_SITE), "invalid equality object type");
        if e.objtype == OBJ_SITE {
            x.name("site1", &e.name1);
            x.name("site2", &e.name2);
        } else {
            x.name("body1", &e.name1);
            x.name("body2", &e.name2);
            x.array("anchor", &e.data[..3]);
            if e.r#type == EQ_WELD { x.array("relpose", &e.data[3..10]); }
        }
        if e.r#type == EQ_WELD { x.number("torquescale", e.data[10]); }
    }
    x.end();
}

fn tendon(x: &mut Xml, t: &TendonSpec) {
    // mjCTendon::Type: fixed when the path is non-empty and has only joints.
    let fixed = !t.path.is_empty() && t.path.iter().all(|w| w.r#type == WRAP_JOINT);
    let tag = if fixed { "fixed" } else { "spatial" };
    x.start(tag);
    x.name("name", &t.name);
    x.number("group", t.group);
    x.text("limited", keyword(t.limited, &["false", "true", "auto"]));
    x.text("actuatorfrclimited", keyword(t.actfrclimited, &["false", "true", "auto"]));
    x.array("range", &t.range);
    x.array("actuatorfrcrange", &t.actfrcrange);
    x.array("solreflimit", &t.solref_limit);
    x.array("solimplimit", &t.solimp_limit);
    x.array("solreffriction", &t.solref_friction);
    x.array("solimpfriction", &t.solimp_friction);
    x.number("frictionloss", t.frictionloss);
    x.array("springlength", &t.springlength);
    x.number("springyield", t.springyield);
    x.number("margin", t.margin);
    x.array("stiffness", &t.stiffness);
    x.array("damping", &t.damping);
    x.number("armature", t.armature);
    x.array("user", &t.userdata);
    if fixed {
        let d = TendonSpec::default();
        assert!(t.width == d.width && t.rgba == d.rgba, "MJCF has no width or rgba on a fixed tendon");
    } else {
        x.number("width", t.width);
        x.array("rgba", &t.rgba);
    }
    x.open();
    for w in &t.path {
        match w.r#type {
            WRAP_SITE => { x.start("site"); x.name("site", &w.name); }
            WRAP_SPHERE | WRAP_CYLINDER => {
                x.start("geom");
                x.name("geom", &w.name);
                if w.sidesite.as_deref().is_some_and(|s| !s.is_empty()) { x.name("sidesite", &w.sidesite); }
            }
            WRAP_PULLEY => { x.start("pulley"); x.number("divisor", w.prm); }
            WRAP_JOINT => { x.start("joint"); x.name("joint", &w.name); x.number("coef", w.prm); }
            _ => panic!("wrap type outside MJCF"),
        }
        x.end();
    }
    x.close(tag);
}

// The "input" attribute: an so3 chart keyword, "none" (an empty signature), or the input tokens
// in canonical order.
fn input(gaintype: i32, ctrlspec: i32) -> String {
    if gaintype == GAIN_SO3 { return keyword(ctrlspec, &["", "expmap", "quat"]).into(); }
    if ctrlspec == INPUT_NONE { return "none".into(); }
    let tokens = [(INPUT_POS, "pos"), (INPUT_VEL, "vel"), (INPUT_FF, "ff"), (INPUT_VOLTAGE, "voltage"), (INPUT_PRESSURE, "pressure")];
    assert!(ctrlspec & !(INPUT_POS | INPUT_VEL | INPUT_FF | INPUT_VOLTAGE | INPUT_PRESSURE) == 0, "input bits outside MJCF");
    tokens.iter().filter(|(bit, _)| ctrlspec & bit != 0).map(|(_, token)| *token).collect::<Vec<_>>().join(" ")
}

// Every actuator is written as <general>: the shortcut setters have already filled its rows.
fn actuator(x: &mut Xml, a: &ActuatorSpec) {
    x.start("general");
    x.name("name", &a.name);
    let target = &a.target;
    match a.trntype {
        TRN_JOINT => x.name("joint", target),
        TRN_JOINTINPARENT => x.name("jointinparent", target),
        TRN_TENDON => x.name("tendon", target),
        TRN_SLIDERCRANK => {
            x.name("cranksite", target);
            x.name("slidersite", &a.slidersite);
            x.number("cranklength", a.cranklength);
        }
        TRN_SITE => {
            x.name("site", target);
            if a.refsite.as_deref().is_some_and(|s| !s.is_empty()) { x.name("refsite", &a.refsite); }
        }
        TRN_BODY => x.name("body", target),
        _ => panic!("transmission type outside MJCF"),
    }
    x.number("group", a.group);
    x.text("ctrllimited", keyword(a.ctrllimited, &["false", "true", "auto"]));
    x.text("forcelimited", keyword(a.forcelimited, &["false", "true", "auto"]));
    x.text("actlimited", keyword(a.actlimited, &["false", "true", "auto"]));
    x.array("ctrlrange", &a.ctrlrange);
    x.array("forcerange", &a.forcerange);
    x.array("actrange", &a.actrange);
    x.array("lengthrange", &a.lengthrange);
    x.array("gear", &a.gear);
    x.array("damping", &a.damping);
    x.number("armature", a.armature);
    x.number("actdim", a.actdim);
    x.array("velrange", &a.velrange);
    x.array("ffrange", &a.ffrange);
    x.number("inheritrange", a.inheritrange);
    x.text("dyntype", keyword(a.dyntype, &["none", "integrator", "filter", "filterexact", "muscle", "dcmotor", "pid", "user"]));
    x.text("gaintype", keyword(a.gaintype, &["fixed", "affine", "muscle", "dcmotor", "so3", "pid", "user"]));
    x.text("biastype", keyword(a.biastype, &["none", "affine", "muscle", "dcmotor", "so3", "user"]));
    x.array("dynprm", &a.dynprm);
    x.array("gainprm", &a.gainprm);
    x.array("biasprm", &a.biasprm);
    x.text("actearly", if a.actearly { "true" } else { "false" });
    if a.ctrlspec != 0 { x.text("input", &input(a.gaintype, a.ctrlspec)); }
    x.number("nsample", a.nsample);
    x.text("interp", keyword(a.interp, &["zoh", "linear", "cubic"]));
    x.number("delay", a.delay);
    x.array("user", &a.userdata);
    x.end();
}

/// Export all applicable attributes, including defaults, using shortest round-trip
/// numeric spelling. Unset sentinels and mutually exclusive alternatives are omitted.
/// Panics on invalid topology or values/combinations MJCF cannot represent.
/// Names referenced by equalities/pairs/excludes/tendons/actuators must be supplied by the builder.
/// Traversal preserves sibling, joint, geom and site order and takes linear time.
pub fn write_mjcf(spec: &ModelSpec) -> String {
    assert!(!spec.bodies.is_empty(), "body zero must be the world");
    let world = &spec.bodies[0];
    assert_eq!(world.parent, 0, "world must parent itself");
    assert!(world.joints.is_empty(), "world cannot have joints");
    let mut children = vec![Vec::new(); spec.bodies.len()];
    for (i, b) in spec.bodies.iter().enumerate().skip(1) {
        assert!(b.parent < i, "body parent must precede child");
        children[b.parent].push(i);
    }
    let mut x = Xml(String::new());
    x.start("mujoco");
    x.name("model", &spec.modelname);
    x.open();
    x.start("compiler");
    x.text("angle", "radian");
    x.text("inertiafromgeom", "false");
    x.text("autolimits", "true");
    x.number("boundmass", spec.compiler.boundmass);
    x.number("boundinertia", spec.compiler.boundinertia);
    x.number("settotalmass", spec.compiler.settotalmass);
    x.number("balanceinertia", spec.compiler.balanceinertia);
    assert!(!spec.compiler.discardvisual && !spec.compiler.fusestatic && !spec.compiler.alignfree,
        "discardvisual, fusestatic and alignfree are outside the builder subset");
    x.text("discardvisual", "false");
    x.number("usethread", spec.compiler.usethread);
    x.text("fusestatic", "false");
    x.text("alignfree", "false");
    x.end();
    option(&mut x, &spec.option);
    x.start("statistic");
    if !spec.stat.extent.is_nan() { x.number("extent", spec.stat.extent); }
    if !spec.stat.meaninertia.is_nan() { x.number("meaninertia", spec.stat.meaninertia); }
    if !spec.stat.meanmass.is_nan() { x.number("meanmass", spec.stat.meanmass); }
    if !spec.stat.meansize.is_nan() { x.number("meansize", spec.stat.meansize); }
    if !spec.stat.center[0].is_nan() { x.array("center", &spec.stat.center); }
    x.end();
    x.start("size");
    assert!(spec.memory >= -1, "memory must be -1 (automatic) or nonnegative");
    if spec.memory != -1 { x.number("memory", spec.memory); }
    x.number("nuserdata", spec.nuserdata);
    x.number("nuser_body", spec.nuser_body);
    x.number("nuser_jnt", spec.nuser_jnt);
    x.number("nuser_geom", spec.nuser_geom);
    x.number("nuser_site", spec.nuser_site);
    x.number("nuser_tendon", spec.nuser_tendon);
    x.number("nuser_actuator", spec.nuser_actuator);
    x.end();
    if !spec.meshes.is_empty() {
        x.start("asset");
        x.open();
        for mesh in &spec.meshes {
            x.start("mesh");
            x.name("name", &mesh.name);
            x.array("vertex", &mesh.vert);
            x.array("scale", &mesh.scale);
            x.text("inertia", keyword(mesh.inertia, &["convex", "exact", "legacy", "shell"]));
            x.end();
        }
        x.close("asset");
    }
    x.start("worldbody");
    x.open();
    // Explicit stack avoids recursion depth limits for long articulated chains.
    let mut stack = vec![(0, false)];
    while let Some((i, closing)) = stack.pop() {
        if closing { x.close(if i == 0 { "worldbody" } else { "body" }); continue; }
        let b = &spec.bodies[i];
        if i != 0 { body(&mut x, b); }
        for j in &b.joints { joint(&mut x, j); }
        for g in &b.geoms { geom(&mut x, g); }
        for s in &b.sites { site(&mut x, s); }
        stack.push((i, true));
        for &child in children[i].iter().rev() { stack.push((child, false)); }
    }
    if !spec.tendons.is_empty() {
        x.start("tendon");
        x.open();
        for t in &spec.tendons { tendon(&mut x, t); }
        x.close("tendon");
    }
    x.start("equality");
    x.open();
    for e in &spec.equalities { equality(&mut x, e); }
    x.close("equality");
    x.start("contact");
    x.open();
    for p in &spec.pairs {
        x.start("pair");
        x.name("name", &p.name);
        x.name("geom1", &p.geomname1);
        x.name("geom2", &p.geomname2);
        x.number("condim", p.condim);
        x.number("margin", p.margin);
        x.number("gap", p.gap);
        x.number("adhesion", p.adhesion);
        x.array("friction", &p.friction);
        x.array("solref", &p.solref);
        x.array("solreffriction", &p.solreffriction);
        x.array("solimp", &p.solimp);
        x.end();
    }
    for e in &spec.excludes {
        x.start("exclude");
        x.name("name", &e.name);
        x.name("body1", &e.bodyname1);
        x.name("body2", &e.bodyname2);
        x.end();
    }
    x.close("contact");
    if !spec.actuators.is_empty() {
        x.start("actuator");
        x.open();
        for a in &spec.actuators { actuator(&mut x, a); }
        x.close("actuator");
    }
    x.close("mujoco");
    x.0
}
