// Invoked by model_file's unit-test module to exercise its private container decoder.
#[macro_export]
macro_rules! model_file_tests {
    ($d:tt) => {
use crate::model::{make_model, MODEL_SIZES, VISUAL_BYTES};
use crate::model_file::{load_model_buffer, model_field_at, save_model, size_model};
use crate::model_fields;
use super::decode_model_buffer;

macro_rules! fill_arrays {
    (($d m:ident, $d counter:ident) sizes { $d($d size:ident,)* }
        arrays { $d($d name:ident: $d ty:ident [$d nr:ident, $d($d nc:tt)+],)* }) => {
        $d(assert!(!$d m.$d name.is_empty(), concat!(stringify!($d name), " must be exercised"));
        for value in &mut $d m.$d name {
            *value = $d counter as $d ty;
            $d counter += 1;
        })*
    };
}

#[test]
fn model_file_roundtrip() {
    let mut sizes = vec![1_i64; MODEL_SIZES.len()];
    let mut m = make_model(&sizes).unwrap();
    assert_eq!(m.opt.timestep, 0.002, "C option default");
    assert_eq!(m.opt.o_solref, [0.02, 1.0], "C solver reference default");
    assert_eq!(m.stat.extent, 2.0, "C statistic default");
    assert_eq!(m.vis.len(), VISUAL_BYTES, "opaque visual record size");
    let mut counter = 1_u64;
    model_fields!(fill_arrays(m, counter));
    m.opt.timestep = counter as f64;
    counter += 1;
    m.opt.impratio = counter as f64;
    counter += 1;
    m.opt.tolerance = counter as f64;
    counter += 1;
    m.opt.ls_tolerance = counter as f64;
    counter += 1;
    m.opt.noslip_tolerance = counter as f64;
    counter += 1;
    m.opt.ccd_tolerance = counter as f64;
    counter += 1;
    m.opt.sleep_tolerance = counter as f64;
    counter += 1;
    for value in &mut m.opt.gravity {
        *value = counter as f64;
        counter += 1;
    }
    for value in &mut m.opt.wind {
        *value = counter as f64;
        counter += 1;
    }
    for value in &mut m.opt.magnetic {
        *value = counter as f64;
        counter += 1;
    }
    m.opt.density = counter as f64;
    counter += 1;
    m.opt.viscosity = counter as f64;
    counter += 1;
    m.opt.o_margin = counter as f64;
    counter += 1;
    for value in &mut m.opt.o_solref {
        *value = counter as f64;
        counter += 1;
    }
    for value in &mut m.opt.o_solimp {
        *value = counter as f64;
        counter += 1;
    }
    for value in &mut m.opt.o_friction {
        *value = counter as f64;
        counter += 1;
    }
    m.opt.integrator = counter as i32;
    counter += 1;
    m.opt.cone = counter as i32;
    counter += 1;
    m.opt.jacobian = counter as i32;
    counter += 1;
    m.opt.solver = counter as i32;
    counter += 1;
    m.opt.iterations = counter as i32;
    counter += 1;
    m.opt.ls_iterations = counter as i32;
    counter += 1;
    m.opt.noslip_iterations = counter as i32;
    counter += 1;
    m.opt.ccd_iterations = counter as i32;
    counter += 1;
    m.opt.disableflags = counter as i32;
    counter += 1;
    m.opt.enableflags = counter as i32;
    counter += 1;
    m.opt.disableactuator = counter as i32;
    counter += 1;
    m.opt.sdf_initpoints = counter as i32;
    counter += 1;
    m.opt.sdf_iterations = counter as i32;
    counter += 1;
    m.stat.meaninertia = counter as f64;
    counter += 1;
    m.stat.meanmass = counter as f64;
    counter += 1;
    m.stat.meansize = counter as f64;
    counter += 1;
    m.stat.extent = counter as f64;
    counter += 1;
    for value in &mut m.stat.center {
        *value = counter as f64;
        counter += 1;
    }
    for value in &mut m.vis {
        *value = counter as u8;
        counter += 1;
    }
    assert!(counter > 1, "all fields received deterministic data");
    m.flg_gravcomp = 1;
    m.flg_surfacevel = 1;
    m.flg_adhesion = 1;
    let bytes = save_model(&m);
    let loaded = decode_model_buffer(&bytes).unwrap();
    assert_eq!(load_model_buffer(&bytes).unwrap_err(), "Invalid model: body_parentid out of bounds.");
    assert_eq!(save_model(&loaded), bytes, "save/load/save preserves bytes");
    assert_eq!(loaded, m, "every field roundtrips");
    assert_eq!(size_model(&m), bytes.len(), "size_model counts packed file bytes");
    assert_eq!(model_field_at(&m, 0), Some("header.id"));
    assert_eq!(model_field_at(&m, bytes.len() - 1), Some("map_d2m"));
    assert_eq!(model_field_at(&m, bytes.len()), None);
    let option_start = 20 + MODEL_SIZES.len() * 8;
    assert_eq!(&bytes[option_start + 300..option_start + 304], &[0; 4], "C option padding is zero");

    let mut corrupt = bytes.clone();
    for (word, message) in [
        (0, "Model missing header ID"),
        (1, "Model and executable have different floating point precision"),
        (2, "Model and executable have different number of sizes in mjModel"),
        (3, "Model and executable use different engine versions"),
        (4, "Model and executable have different number of pointers in mjModel"),
    ] {
        corrupt[word * 4] ^= 1;
        assert_eq!(load_model_buffer(&corrupt).unwrap_err(), message);
        corrupt[word * 4] ^= 1;
    }
    for (end, message) in [
        (19, "Model file has an incomplete header"),
        (option_start - 1, "Truncated model file - ran out of data while reading sizes"),
        (option_start, "Truncated model file - ran out of data while reading structs"),
        (bytes.len() - 1, "Truncated model file - ran out of data while reading map_d2m"),
    ] {
        assert_eq!(load_model_buffer(&bytes[..end]).unwrap_err(), message);
    }
    let nbuffer_start = 20 + (MODEL_SIZES.len() - 1) * 8;
    corrupt[nbuffer_start] ^= 1;
    assert_eq!(load_model_buffer(&corrupt).unwrap_err(), "Corrupted model, wrong nbuffer field");
    corrupt[nbuffer_start] ^= 1;
    corrupt.extend_from_slice(&[0]);
    assert_eq!(load_model_buffer(&corrupt).unwrap_err(), "Model file is too large");

    sizes[0] = -1;
    assert_eq!(make_model(&sizes).unwrap_err(), "Invalid model: nq is negative (-1).");
    sizes[0] = i32::MAX as i64;
    assert_eq!(make_model(&sizes).unwrap_err(),
        "Invalid model: nq is too large. Expected < 2147483647. Got 2147483647.");
    sizes[0] = 1;
    sizes[6] = 0;
    assert_eq!(make_model(&sizes).unwrap_err(), "Invalid model: nbody == 0");
}

    };
}
