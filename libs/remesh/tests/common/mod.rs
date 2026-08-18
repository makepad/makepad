//! Reader for the FCDUMP1 fixture/dump format written by
//! local/faithc_ref/convert_dumps.py (std-only, little endian).
//!
//! Shared between several test binaries; each uses a subset of the API.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

#[derive(Clone)]
pub enum Arr {
    F32(Vec<f32>),
    I64(Vec<i64>),
    I8(Vec<i8>),
    F64(Vec<f64>),
}

pub struct FcDump {
    pub arrays: HashMap<String, (Vec<usize>, Arr)>,
}

impl FcDump {
    pub fn load(path: &std::path::Path) -> std::io::Result<FcDump> {
        let mut f = std::fs::File::open(path)?;
        let mut data = Vec::new();
        f.read_to_end(&mut data)?;
        let mut p = 0usize;
        assert_eq!(&data[0..8], b"FCDUMP1\x00");
        p += 8;
        let rd_u32 = |p: &mut usize| {
            let v = u32::from_le_bytes(data[*p..*p + 4].try_into().unwrap());
            *p += 4;
            v
        };
        let rd_u64 = |p: &mut usize| {
            let v = u64::from_le_bytes(data[*p..*p + 8].try_into().unwrap());
            *p += 8;
            v
        };
        let count = rd_u32(&mut p);
        let mut arrays = HashMap::new();
        for _ in 0..count {
            let name_len = rd_u32(&mut p) as usize;
            let name = String::from_utf8(data[p..p + name_len].to_vec()).unwrap();
            p += name_len;
            let dtype = data[p];
            p += 1;
            let ndim = rd_u32(&mut p) as usize;
            let mut dims = Vec::with_capacity(ndim);
            for _ in 0..ndim {
                dims.push(rd_u64(&mut p) as usize);
            }
            let byte_len = rd_u64(&mut p) as usize;
            let raw = &data[p..p + byte_len];
            p += byte_len;
            let arr = match dtype {
                0 => Arr::F32(
                    raw.chunks_exact(4)
                        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                ),
                1 => Arr::I64(
                    raw.chunks_exact(8)
                        .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                ),
                2 => Arr::I8(raw.iter().map(|&b| b as i8).collect()),
                3 => Arr::F64(
                    raw.chunks_exact(8)
                        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                ),
                _ => panic!("bad dtype {dtype}"),
            };
            arrays.insert(name, (dims, arr));
        }
        Ok(FcDump { arrays })
    }

    pub fn f32(&self, name: &str) -> (&[usize], &[f32]) {
        match &self.arrays[name] {
            (d, Arr::F32(v)) => (d, v),
            _ => panic!("{name}: not f32"),
        }
    }
    pub fn i64(&self, name: &str) -> (&[usize], &[i64]) {
        match &self.arrays[name] {
            (d, Arr::I64(v)) => (d, v),
            _ => panic!("{name}: not i64"),
        }
    }
    pub fn i8(&self, name: &str) -> (&[usize], &[i8]) {
        match &self.arrays[name] {
            (d, Arr::I8(v)) => (d, v),
            _ => panic!("{name}: not i8"),
        }
    }

    pub fn v3s(&self, name: &str) -> Vec<[f32; 3]> {
        let (dims, v) = self.f32(name);
        assert_eq!(*dims.last().unwrap(), 3);
        v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
    }

    pub fn tris(&self, name: &str) -> Vec<[[f32; 3]; 3]> {
        let (dims, v) = self.f32(name);
        assert_eq!(&dims[1..], &[3, 3]);
        v.chunks_exact(9)
            .map(|c| {
                [
                    [c[0], c[1], c[2]],
                    [c[3], c[4], c[5]],
                    [c[6], c[7], c[8]],
                ]
            })
            .collect()
    }
}

/// local/faithc_ref relative to the workspace root.
pub fn faithc_ref_dir() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .ok()?;
    let dir = root.join("local/faithc_ref");
    if dir.is_dir() {
        Some(dir)
    } else {
        None
    }
}

/// Mesh (positions + faces) from a dump -> triangle soup.
pub fn dump_tri_soup(dump: &FcDump) -> Vec<[[f32; 3]; 3]> {
    let verts = dump.v3s("mesh_vertices");
    let (_, faces) = dump.i64("mesh_faces");
    faces
        .chunks_exact(3)
        .map(|f| {
            [
                verts[f[0] as usize],
                verts[f[1] as usize],
                verts[f[2] as usize],
            ]
        })
        .collect()
}
