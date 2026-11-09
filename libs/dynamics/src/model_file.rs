//! Little-endian model files, preserving the C record layout.
//!
//! Translated from `src/engine/engine_io.c` and the file-read portion of
//! `src/xml/xml_api.cc` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust; std::fs replaces the resource layer.

use crate::model::{columns, make_model, Model, Opt, Statistic, MODEL_POINTERS, MODEL_SIZES, VISUAL_BYTES};
use crate::model_fields;
use crate::types::*;
use std::path::Path;

const ID: i32 = 54321;
const NHEADER: usize = 5;
const OPTION_BYTES: usize = 304;
const STATISTIC_BYTES: usize = 56;
const STRUCT_BYTES: usize = OPTION_BYTES + VISUAL_BYTES + STATISTIC_BYTES + 3;
const FIXED_BYTES: usize = NHEADER * 4 + MODEL_SIZES.len() * 8 + STRUCT_BYTES;

// C x86-64/aarch64 layout (offsets in bytes):
// Opt:
//   timestep: 0 (8 bytes)
//   impratio: 8 (8 bytes)
//   tolerance: 16 (8 bytes)
//   ls_tolerance: 24 (8 bytes)
//   noslip_tolerance: 32 (8 bytes)
//   ccd_tolerance: 40 (8 bytes)
//   sleep_tolerance: 48 (8 bytes)
//   gravity: 56 (24 bytes)
//   wind: 80 (24 bytes)
//   magnetic: 104 (24 bytes)
//   density: 128 (8 bytes)
//   viscosity: 136 (8 bytes)
//   o_margin: 144 (8 bytes)
//   o_solref: 152 (16 bytes)
//   o_solimp: 168 (40 bytes)
//   o_friction: 208 (40 bytes)
//   integrator: 248 (4 bytes)
//   cone: 252 (4 bytes)
//   jacobian: 256 (4 bytes)
//   solver: 260 (4 bytes)
//   iterations: 264 (4 bytes)
//   ls_iterations: 268 (4 bytes)
//   noslip_iterations: 272 (4 bytes)
//   ccd_iterations: 276 (4 bytes)
//   disableflags: 280 (4 bytes)
//   enableflags: 284 (4 bytes)
//   disableactuator: 288 (4 bytes)
//   sdf_initpoints: 292 (4 bytes)
//   sdf_iterations: 296 (4 bytes)
//   trailing padding: 300..304; alignment 8, total 304.
// Statistic:
//   meaninertia: 0 (8 bytes)
//   meanmass: 8 (8 bytes)
//   meansize: 16 (8 bytes)
//   extent: 24 (8 bytes)
//   center: 32 (24 bytes)
//   no padding; alignment 8, total 56.

impl Opt {
    fn write(&self, out: &mut Vec<u8>) {
        self.timestep.write(out);
        self.impratio.write(out);
        self.tolerance.write(out);
        self.ls_tolerance.write(out);
        self.noslip_tolerance.write(out);
        self.ccd_tolerance.write(out);
        self.sleep_tolerance.write(out);
        for value in &self.gravity { value.write(out); }
        for value in &self.wind { value.write(out); }
        for value in &self.magnetic { value.write(out); }
        self.density.write(out);
        self.viscosity.write(out);
        self.o_margin.write(out);
        for value in &self.o_solref { value.write(out); }
        for value in &self.o_solimp { value.write(out); }
        for value in &self.o_friction { value.write(out); }
        self.integrator.write(out);
        self.cone.write(out);
        self.jacobian.write(out);
        self.solver.write(out);
        self.iterations.write(out);
        self.ls_iterations.write(out);
        self.noslip_iterations.write(out);
        self.ccd_iterations.write(out);
        self.disableflags.write(out);
        self.enableflags.write(out);
        self.disableactuator.write(out);
        self.sdf_initpoints.write(out);
        self.sdf_iterations.write(out);
        out.extend_from_slice(&[0; 4]);
    }

    fn read(reader: &mut Reader<'_>) -> Self {
        let record = Self {
            timestep: reader.scalar(),
            impratio: reader.scalar(),
            tolerance: reader.scalar(),
            ls_tolerance: reader.scalar(),
            noslip_tolerance: reader.scalar(),
            ccd_tolerance: reader.scalar(),
            sleep_tolerance: reader.scalar(),
            gravity: std::array::from_fn(|_| reader.scalar()),
            wind: std::array::from_fn(|_| reader.scalar()),
            magnetic: std::array::from_fn(|_| reader.scalar()),
            density: reader.scalar(),
            viscosity: reader.scalar(),
            o_margin: reader.scalar(),
            o_solref: std::array::from_fn(|_| reader.scalar()),
            o_solimp: std::array::from_fn(|_| reader.scalar()),
            o_friction: std::array::from_fn(|_| reader.scalar()),
            integrator: reader.scalar(),
            cone: reader.scalar(),
            jacobian: reader.scalar(),
            solver: reader.scalar(),
            iterations: reader.scalar(),
            ls_iterations: reader.scalar(),
            noslip_iterations: reader.scalar(),
            ccd_iterations: reader.scalar(),
            disableflags: reader.scalar(),
            enableflags: reader.scalar(),
            disableactuator: reader.scalar(),
            sdf_initpoints: reader.scalar(),
            sdf_iterations: reader.scalar(),
        };
        reader.offset += 4;
        record
    }
}

impl Statistic {
    fn write(&self, out: &mut Vec<u8>) {
        self.meaninertia.write(out);
        self.meanmass.write(out);
        self.meansize.write(out);
        self.extent.write(out);
        for value in &self.center { value.write(out); }
    }

    fn read(reader: &mut Reader<'_>) -> Self {
        let record = Self {
            meaninertia: reader.scalar(),
            meanmass: reader.scalar(),
            meansize: reader.scalar(),
            extent: reader.scalar(),
            center: std::array::from_fn(|_| reader.scalar()),
        };
        record
    }
}

fn record_field_at(record: &str, offset: usize) -> Option<&'static str> {
    match record {
        "opt" => match offset {
            0..=7 => Some("opt.timestep"),
            8..=15 => Some("opt.impratio"),
            16..=23 => Some("opt.tolerance"),
            24..=31 => Some("opt.ls_tolerance"),
            32..=39 => Some("opt.noslip_tolerance"),
            40..=47 => Some("opt.ccd_tolerance"),
            48..=55 => Some("opt.sleep_tolerance"),
            56..=79 => Some("opt.gravity"),
            80..=103 => Some("opt.wind"),
            104..=127 => Some("opt.magnetic"),
            128..=135 => Some("opt.density"),
            136..=143 => Some("opt.viscosity"),
            144..=151 => Some("opt.o_margin"),
            152..=167 => Some("opt.o_solref"),
            168..=207 => Some("opt.o_solimp"),
            208..=247 => Some("opt.o_friction"),
            248..=251 => Some("opt.integrator"),
            252..=255 => Some("opt.cone"),
            256..=259 => Some("opt.jacobian"),
            260..=263 => Some("opt.solver"),
            264..=267 => Some("opt.iterations"),
            268..=271 => Some("opt.ls_iterations"),
            272..=275 => Some("opt.noslip_iterations"),
            276..=279 => Some("opt.ccd_iterations"),
            280..=283 => Some("opt.disableflags"),
            284..=287 => Some("opt.enableflags"),
            288..=291 => Some("opt.disableactuator"),
            292..=295 => Some("opt.sdf_initpoints"),
            296..=299 => Some("opt.sdf_iterations"),
            300..=303 => Some("opt.padding"),
            _ => None,
        },
        "stat" => match offset {
            0..=7 => Some("stat.meaninertia"),
            8..=15 => Some("stat.meanmass"),
            16..=23 => Some("stat.meansize"),
            24..=31 => Some("stat.extent"),
            32..=55 => Some("stat.center"),
            _ => None,
        },
        _ => None,
    }
}

trait Scalar: Sized {
    const BYTES: usize;
    fn write(&self, out: &mut Vec<u8>);
    fn read(bytes: &[u8]) -> Self;
}

macro_rules! scalar {
    ($($ty:ty),*) => { $(
        impl Scalar for $ty {
            const BYTES: usize = std::mem::size_of::<Self>();
            fn write(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn read(bytes: &[u8]) -> Self {
                Self::from_le_bytes(bytes.try_into().unwrap())
            }
        }
    )* };
}
scalar!(f64, f32, i64, i32, u8);

// Every caller checks the complete record/array before reading any of its scalars.
struct Reader<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl Reader<'_> {
    fn scalar<T: Scalar>(&mut self) -> T {
        let value = T::read(&self.buf[self.offset..self.offset + T::BYTES]);
        self.offset += T::BYTES;
        value
    }

    fn check(&self, count: usize, message: &str) -> Result<(), String> {
        if count > self.buf.len() - self.offset {
            Err(message.to_string())
        } else {
            Ok(())
        }
    }
}

/// Borrowed values of one serialized Model field. Floating-point callers should
/// compare `to_bits()`, including signed zeros and NaN payloads. `vis` is the
/// opaque byte record retained by Model; padding and nonserialized caches are
/// not model fields. Sizes are scalar, all other variants are slices.
#[derive(Clone, Copy, Debug)]
pub enum ModelField<'a> {
    Size(usize),
    F64(&'a [f64]),
    F32(&'a [f32]),
    I64(&'a [i64]),
    I32(&'a [i32]),
    U8(&'a [u8]),
}

macro_rules! field_slice {
    ($values:expr, f64) => { ModelField::F64($values) };
    ($values:expr, f32) => { ModelField::F32($values) };
    ($values:expr, i64) => { ModelField::I64($values) };
    ($values:expr, i32) => { ModelField::I32($values) };
    ($values:expr, u8) => { ModelField::U8($values) };
}

macro_rules! file_arrays {
    (() sizes { $($size:ident,)* } arrays { $($name:ident: $ty:ident [$nr:ident, $($nc:tt)+],)* }) => {
        impl Model {
            fn write_arrays(&self, out: &mut Vec<u8>) {
                let m = self;
                $(let count = m.$nr * columns!(m; $($nc)+);
                for value in &m.$name[..count] { value.write(out); })*
            }

            fn read_arrays(&mut self, reader: &mut Reader<'_>) -> Result<(), String> {
                let m = self;
                $(let message = concat!("Truncated model file - ran out of data while reading ", stringify!($name));
                let count = m.$nr.checked_mul(columns!(m; $($nc)+))
                    .ok_or_else(|| message.to_string())?;
                let bytes = count.checked_mul(<$ty>::BYTES).ok_or_else(|| message.to_string())?;
                reader.check(bytes, message)?;
                // The serialized nnames_map is set after construction in C. A Vec
                // can safely follow that size even if the file changes it.
                if count > m.$name.len() {
                    m.$name.try_reserve_exact(count - m.$name.len())
                        .map_err(|_| "could not allocate the model buffer".to_string())?;
                }
                m.$name.resize(count, <$ty>::default());
                for value in &mut m.$name { *value = reader.scalar(); })*
                Ok(())
            }
        }

        /// Visit every serialized Model field in file order using the same table
        /// as load/save. Empty arrays are visited, so differing model sizes can be
        /// compared by field rather than by shifted file offsets.
        pub fn visit_model_fields<'a>(m: &'a Model, mut visit: impl FnMut(&'static str, ModelField<'a>)) {
            $(visit(stringify!($size), ModelField::Size(m.$size));)*
            visit("opt.timestep", ModelField::F64(std::slice::from_ref(&m.opt.timestep)));
            visit("opt.impratio", ModelField::F64(std::slice::from_ref(&m.opt.impratio)));
            visit("opt.tolerance", ModelField::F64(std::slice::from_ref(&m.opt.tolerance)));
            visit("opt.ls_tolerance", ModelField::F64(std::slice::from_ref(&m.opt.ls_tolerance)));
            visit("opt.noslip_tolerance", ModelField::F64(std::slice::from_ref(&m.opt.noslip_tolerance)));
            visit("opt.ccd_tolerance", ModelField::F64(std::slice::from_ref(&m.opt.ccd_tolerance)));
            visit("opt.sleep_tolerance", ModelField::F64(std::slice::from_ref(&m.opt.sleep_tolerance)));
            visit("opt.gravity", ModelField::F64(&m.opt.gravity));
            visit("opt.wind", ModelField::F64(&m.opt.wind));
            visit("opt.magnetic", ModelField::F64(&m.opt.magnetic));
            visit("opt.density", ModelField::F64(std::slice::from_ref(&m.opt.density)));
            visit("opt.viscosity", ModelField::F64(std::slice::from_ref(&m.opt.viscosity)));
            visit("opt.o_margin", ModelField::F64(std::slice::from_ref(&m.opt.o_margin)));
            visit("opt.o_solref", ModelField::F64(&m.opt.o_solref));
            visit("opt.o_solimp", ModelField::F64(&m.opt.o_solimp));
            visit("opt.o_friction", ModelField::F64(&m.opt.o_friction));
            visit("opt.integrator", ModelField::I32(std::slice::from_ref(&m.opt.integrator)));
            visit("opt.cone", ModelField::I32(std::slice::from_ref(&m.opt.cone)));
            visit("opt.jacobian", ModelField::I32(std::slice::from_ref(&m.opt.jacobian)));
            visit("opt.solver", ModelField::I32(std::slice::from_ref(&m.opt.solver)));
            visit("opt.iterations", ModelField::I32(std::slice::from_ref(&m.opt.iterations)));
            visit("opt.ls_iterations", ModelField::I32(std::slice::from_ref(&m.opt.ls_iterations)));
            visit("opt.noslip_iterations", ModelField::I32(std::slice::from_ref(&m.opt.noslip_iterations)));
            visit("opt.ccd_iterations", ModelField::I32(std::slice::from_ref(&m.opt.ccd_iterations)));
            visit("opt.disableflags", ModelField::I32(std::slice::from_ref(&m.opt.disableflags)));
            visit("opt.enableflags", ModelField::I32(std::slice::from_ref(&m.opt.enableflags)));
            visit("opt.disableactuator", ModelField::I32(std::slice::from_ref(&m.opt.disableactuator)));
            visit("opt.sdf_initpoints", ModelField::I32(std::slice::from_ref(&m.opt.sdf_initpoints)));
            visit("opt.sdf_iterations", ModelField::I32(std::slice::from_ref(&m.opt.sdf_iterations)));
            visit("vis", ModelField::U8(&m.vis));
            visit("stat.meaninertia", ModelField::F64(std::slice::from_ref(&m.stat.meaninertia)));
            visit("stat.meanmass", ModelField::F64(std::slice::from_ref(&m.stat.meanmass)));
            visit("stat.meansize", ModelField::F64(std::slice::from_ref(&m.stat.meansize)));
            visit("stat.extent", ModelField::F64(std::slice::from_ref(&m.stat.extent)));
            visit("stat.center", ModelField::F64(&m.stat.center));
            visit("flg_gravcomp", ModelField::U8(std::slice::from_ref(&m.flg_gravcomp)));
            visit("flg_surfacevel", ModelField::U8(std::slice::from_ref(&m.flg_surfacevel)));
            visit("flg_adhesion", ModelField::U8(std::slice::from_ref(&m.flg_adhesion)));
            $(let count = m.$nr * columns!(m; $($nc)+);
            visit(stringify!($name), field_slice!(&m.$name[..count], $ty));)*
        }

        /// Serialized size; file arrays are packed, unlike the aligned C buffer.
        pub fn size_model(m: &Model) -> usize {
            let mut size = FIXED_BYTES;
            $(size += <$ty>::BYTES * m.$nr * columns!(m; $($nc)+);)*
            size
        }

        /// Name of the serialized field containing an offset, for parity diagnostics.
        pub fn model_field_at(m: &Model, offset: usize) -> Option<&'static str> {
            let mut start = 0;
            for name in ["header.id", "header.precision", "header.nsize", "header.version", "header.nptr"] {
                if offset < start + 4 { return Some(name); }
                start += 4;
            }
            $(if offset < start + 8 { return Some(stringify!($size)); }
            start += 8;)*
            if offset < start + OPTION_BYTES { return record_field_at("opt", offset - start); }
            start += OPTION_BYTES;
            if offset < start + VISUAL_BYTES { return Some("vis"); }
            start += VISUAL_BYTES;
            if offset < start + STATISTIC_BYTES { return record_field_at("stat", offset - start); }
            start += STATISTIC_BYTES;
            for name in ["flg_gravcomp", "flg_surfacevel", "flg_adhesion"] {
                if offset == start { return Some(name); }
                start += 1;
            }
            $(let bytes = <$ty>::BYTES * m.$nr * columns!(m; $($nc)+);
            if offset < start + bytes { return Some(stringify!($name)); }
            start += bytes;)*
            debug_assert_eq!(start, size_model(m));
            None
        }
    };
}
model_fields!(file_arrays());

fn header() -> [i32; NHEADER] {
    [ID, 8, MODEL_SIZES.len() as i32, VERSION_HEADER as i32, MODEL_POINTERS as i32]
}

pub fn save_model(m: &Model) -> Vec<u8> {
    let mut buf = Vec::with_capacity(size_model(m));
    for value in header() { value.write(&mut buf); }
    for value in m.sizes() { value.write(&mut buf); }
    m.opt.write(&mut buf);
    assert_eq!(m.vis.len(), VISUAL_BYTES, "mjVisual size mismatch");
    buf.extend_from_slice(&m.vis);
    m.stat.write(&mut buf);
    buf.extend_from_slice(&[m.flg_gravcomp, m.flg_surfacevel, m.flg_adhesion]);
    m.write_arrays(&mut buf);
    buf
}

/// Decode a model file and validate its references, as mj_loadModelBuffer does.
pub fn load_model_buffer(buf: &[u8]) -> Result<Model, String> {
    let m = decode_model_buffer(buf)?;
    crate::io::validate_references(&m)?;
    crate::model::check_supported(&m)?;
    Ok(m)
}

fn decode_model_buffer(buf: &[u8]) -> Result<Model, String> {
    let mut reader = Reader { buf, offset: 0 };
    reader.check(NHEADER * 4, "Model file has an incomplete header")?;
    let messages = [
        "Model missing header ID",
        "Model and executable have different floating point precision",
        "Model and executable have different number of sizes in mjModel",
        "Model and executable use different engine versions",
        "Model and executable have different number of pointers in mjModel",
    ];
    for (expected, message) in header().into_iter().zip(messages) {
        if reader.scalar::<i32>() != expected { return Err(message.into()); }
    }
    reader.check(MODEL_SIZES.len() * 8,
        "Truncated model file - ran out of data while reading sizes")?;
    let sizes: [i64; MODEL_SIZES.len()] = std::array::from_fn(|_| reader.scalar());
    let mut m = make_model(&sizes).map_err(|_| "Invalid sizes, unable to load model".to_string())?;
    if m.nbuffer as i64 != sizes[MODEL_SIZES.len() - 1] {
        return Err("Corrupted model, wrong nbuffer field".into());
    }
    m.set_sizes(&sizes);
    reader.check(STRUCT_BYTES, "Truncated model file - ran out of data while reading structs")?;
    m.opt = Opt::read(&mut reader);
    m.vis.copy_from_slice(&buf[reader.offset..reader.offset + VISUAL_BYTES]);
    reader.offset += VISUAL_BYTES;
    m.stat = Statistic::read(&mut reader);
    m.flg_gravcomp = reader.scalar();
    m.flg_surfacevel = reader.scalar();
    m.flg_adhesion = reader.scalar();
    m.read_arrays(&mut reader)?;
    if reader.offset != buf.len() { return Err("Model file is too large".into()); }
    Ok(m)
}

pub fn load_model(path: &Path) -> Result<Model, String> {
    load_model_with(path, |_| Ok(()))
}

/// Decode and validate references, attach SDF fields, then check supported features.
pub fn load_model_with(
    path: &Path,
    attach: impl FnOnce(&mut Model) -> Result<(), String>,
) -> Result<Model, String> {
    let buf = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut m = decode_model_buffer(&buf)?;
    crate::io::validate_references(&m)?;
    attach(&mut m)?;
    crate::model::check_supported(&m)?;
    Ok(m)
}

#[cfg(test)]
#[path = "../tests/model_file.rs"]
mod test_source;

#[cfg(test)]
mod tests {
    crate::model_file_tests!($);
}
