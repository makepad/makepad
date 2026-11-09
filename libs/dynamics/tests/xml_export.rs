use makepad_dynamics::model_spec::*;
use makepad_dynamics::types::*;
use makepad_dynamics::xml_export::write_mjcf;

#[test]
fn hinge_chain_mjcf() {
    let mut spec = ModelSpec {
        modelname: Some("chain & \"links\"".into()), ..ModelSpec::default()
    };
    spec.option.gravity = [0.0, 0.0, -9810.0];
    spec.option.timestep = 1.0 / 240.0;
    spec.bodies.push(BodySpec {
        name: Some("base".into()), pos: [0.0, 0.0, 600.0], ipos: [0.0; 3],
        mass: 1000.0, inertia: [10000.0; 3],
        geoms: vec![GeomSpec { size: [10.0; 3], ..GeomSpec::default() }],
        ..BodySpec::default()
    });
    for i in 0..5 {
        spec.bodies.push(BodySpec {
            name: Some(format!("link{i}")), parent: i + 1,
            pos: [0.0, 0.0, if i == 0 { 0.0 } else { -100.0 }],
            ipos: [0.0, 0.0, -50.0], mass: 100.0,
            inertia: [83333.0, 83333.0, 1250.0],
            joints: vec![JointSpec { axis: [0.0, 1.0, 0.0], ..JointSpec::default() }],
            geoms: vec![GeomSpec {
                r#type: GEOM_CAPSULE, size: [5.0, 0.0, 0.0], density: 0.001,
                fromto: [0.0, 0.0, 0.0, 0.0, 0.0, -100.0], ..GeomSpec::default()
            }],
            ..BodySpec::default()
        });
    }
    spec.bodies[6].sites.push(SiteSpec {
        name: Some("tip".into()), pos: [0.0, 0.0, -100.0], size: [2.0; 3],
        ..SiteSpec::default()
    });
    let xml = write_mjcf(&spec);
    assert!(xml.contains("model=\"chain &amp; &quot;links&quot;\""), "escape XML names");
    assert!(xml.contains("<compiler angle=\"radian\" inertiafromgeom=\"false\" autolimits=\"true\""), "fixed compiler contract");
    assert_eq!(xml.matches("<body ").count(), 6, "base plus five links");
    assert_eq!(xml.matches("<inertial ").count(), 6, "every nonworld body has inertia");
    assert_eq!(xml.matches("<joint ").count(), 5, "five hinges");
    assert_eq!(xml.matches("type=\"capsule\"").count(), 5, "five capsule links");
    assert_eq!(xml.matches("fromto=\"0.0 0.0 0.0 0.0 0.0 -100.0\"").count(), 5, "capsule endpoints");
    assert!(xml.contains("<site name=\"tip\""), "tip site");
    assert!(xml.contains("<flag constraint=\"enable\""), "explicit option flags");
    assert!(xml.contains("gravity=\"0.0 0.0 -9810.0\""), "millimetre gravity");
    assert!(xml.contains("<equality>\n</equality>"), "equality section");
    assert!(xml.contains("<contact>\n</contact>"), "contact section");
    assert!(!xml.contains("NaN"), "unset sentinels must not reach the reader");
    let timestep = xml.split("timestep=\"").nth(1).unwrap().split('"').next().unwrap();
    assert_eq!(timestep.parse::<f64>().unwrap().to_bits(), spec.option.timestep.to_bits(), "round-trip timestep bits");
    // Subnormal and signed-zero spellings are not covered by the ordinary fixture values.
    spec.option.wind = [f64::from_bits(1), -0.0, f64::from_bits(0x3ff0000000000001)];
    let xml = write_mjcf(&spec);
    let wind = xml.split("wind=\"").nth(1).unwrap().split('"').next().unwrap();
    for (value, expected) in wind.split_whitespace().zip(spec.option.wind) {
        assert_eq!(value.parse::<f64>().unwrap().to_bits(), expected.to_bits(), "round-trip wind bits");
    }
}
