//! Versioned canonical binary checkpoint. All integers are little-endian;
//! negative zero has one encoding (+0). Unknown flags, duplicate identities,
//! noncanonical order, invalid lengths and trailing bytes are errors.
use crate::{mesh::storage_estimate, *};

const MAGIC: &[u8; 8] = b"MPMESH01";
const PIN_MAGIC: &[u8; 8] = b"MPMESH02";

impl Mesh {
    pub fn to_bytes(&self, ctx: &mut Context<'_>) -> Result<Vec<u8>> {
        self.check_structure(ctx)?;
        let size = 32usize
            .saturating_add(self.vertices.len().saturating_mul(36))
            .saturating_add(self.faces.len().saturating_mul(20))
            .saturating_add(self.corners.len().saturating_mul(33))
            .saturating_add(
                self.corners
                    .iter()
                    .filter(|c| c.normal.is_some())
                    .count()
                    .saturating_mul(24),
            )
            .saturating_add(self.edge_data.len().saturating_mul(26))
            .saturating_add(
                self.vertices
                    .iter()
                    .map(|v| v.weights.len().saturating_mul(12))
                    .sum::<usize>(),
            );
        ctx.bytes(self.estimated_bytes().saturating_add(size))?;
        let mut w = Writer(Vec::with_capacity(size));
        w.0.extend_from_slice(if self.uv_pins.is_empty(){MAGIC}else{PIN_MAGIC});
        w.u64(self.next_id);
        for n in [
            self.vertices.len(),
            self.faces.len(),
            self.corners.len(),
            self.edge_data.len(),
        ] {
            w.u32(n as u32);
        }
        for v in &self.vertices {
            ctx.checkpoint(1)?;
            w.u64(v.id.0);
            for p in v.position {
                w.f64(p);
            }
            w.u32(v.weights.len() as u32);
            for weight in &v.weights {
                ctx.checkpoint(1)?;
                w.u32(weight.joint);
                w.f64(weight.weight);
            }
        }
        for f in &self.faces {
            ctx.checkpoint(1)?;
            w.u64(f.id.0);
            w.u32(f.first_corner);
            w.u32(f.corner_count);
            w.u32(f.material);
        }
        for c in &self.corners {
            ctx.checkpoint(1)?;
            w.u64(c.id.0);
            w.u64(c.vertex.0);
            for uv in c.uv {
                w.f64(uv);
            }
            w.0.push(c.normal.is_some() as u8 | ((self.uv_pins.contains(&c.id) as u8)<<1));
            if let Some(normal) = c.normal {
                for n in normal {
                    w.f64(n);
                }
            }
        }
        for (key, data) in &self.edge_data {
            ctx.checkpoint(1)?;
            w.u64(key.0 .0);
            w.u64(key.1 .0);
            w.0.push(data.attributes.seam as u8);
            w.f64(data.attributes.crease);
            w.0.push(data.loose as u8);
        }
        debug_assert_eq!(w.0.len(), size);
        Ok(w.0)
    }

    pub fn from_bytes(bytes: &[u8], ctx: &mut Context<'_>) -> Result<Self> {
        ctx.checkpoint(0)?;
        ctx.bytes(bytes.len())?;
        let mut r = Reader { bytes, offset: 0 };
        let magic=r.take(8)?;
        if magic != MAGIC && magic != PIN_MAGIC {
            return Err(MeshError::CorruptData("unsupported mesh format"));
        }
        let next_id = r.u64()?;
        let nv = r.u32()? as usize;
        let nf = r.u32()? as usize;
        let nc = r.u32()? as usize;
        let ne = r.u32()? as usize;
        ctx.counts(nv, nf, nc, ne)?;
        let minimum = nv
            .saturating_mul(36)
            .saturating_add(nf.saturating_mul(20))
            .saturating_add(nc.saturating_mul(33))
            .saturating_add(ne.saturating_mul(26));
        if minimum > r.remaining() {
            return Err(MeshError::CorruptData("counts exceed input size"));
        }
        ctx.bytes(
            bytes
                .len()
                .saturating_add(storage_estimate(nv, nf, nc, ne, 0)),
        )?;
        let mut m = Self {
            vertices: Vec::with_capacity(nv),
            faces: Vec::with_capacity(nf),
            corners: Vec::with_capacity(nc),
            edge_data: Default::default(),
            uv_pins: Default::default(),
            next_id,
        };
        let mut total_weights = 0usize;
        for _ in 0..nv {
            ctx.checkpoint(1)?;
            let id = VertexId(r.u64()?);
            let position = [r.f64()?, r.f64()?, r.f64()?];
            let nw = r.u32()? as usize;
            ctx.limit(
                "weights per vertex",
                nw as u64,
                ctx.limits.max_weights_per_vertex as u64,
            )?;
            if nw.saturating_mul(12) > r.remaining() {
                return Err(MeshError::CorruptData("weight count exceeds input"));
            }
            total_weights = total_weights.saturating_add(nw);
            ctx.bytes(
                bytes
                    .len()
                    .saturating_add(storage_estimate(nv, nf, nc, ne, total_weights)),
            )?;
            let mut weights = Vec::with_capacity(nw);
            for _ in 0..nw {
                ctx.checkpoint(1)?;
                weights.push(JointWeight {
                    joint: r.u32()?,
                    weight: r.f64()?,
                });
            }
            m.vertices.push(Vertex {
                id,
                position,
                weights,
            });
        }
        for _ in 0..nf {
            ctx.checkpoint(1)?;
            m.faces.push(Face {
                id: FaceId(r.u64()?),
                first_corner: r.u32()?,
                corner_count: r.u32()?,
                material: r.u32()?,
            });
        }
        for _ in 0..nc {
            ctx.checkpoint(1)?;
            let id = CornerId(r.u64()?);
            let vertex = VertexId(r.u64()?);
            let uv = [r.f64()?, r.f64()?];
            let flags=r.take(1)?[0];
            if flags>if magic==MAGIC {1}else{3}{return Err(MeshError::CorruptData("invalid corner flags"));}
            if flags&2!=0 {m.uv_pins.insert(id);}
            let normal = if flags&1!=0 {
                Some([r.f64()?, r.f64()?, r.f64()?])
            } else {
                None
            };
            m.corners.push(Corner {
                id,
                vertex,
                uv,
                normal,
            });
        }
        let mut previous = None;
        for _ in 0..ne {
            ctx.checkpoint(1)?;
            let key = EdgeKey(VertexId(r.u64()?), VertexId(r.u64()?));
            if previous.is_some_and(|p| p >= key) {
                return Err(MeshError::CorruptData("edges are not in canonical order"));
            }
            previous = Some(key);
            let attributes = EdgeAttributes {
                seam: r.flag()?,
                crease: r.f64()?,
            };
            let loose = r.flag()?;
            m.edge_data.insert(key, EdgeData { attributes, loose });
        }
        if r.remaining() != 0 {
            return Err(MeshError::CorruptData("trailing bytes"));
        }
        if magic==PIN_MAGIC && m.uv_pins.is_empty(){return Err(MeshError::CorruptData("noncanonical empty pin extension"));}
        m.check_structure(ctx)?;
        m.check_faces(ctx)?;
        Ok(m)
    }
}
struct Writer(Vec<u8>);
impl Writer {
    fn u32(&mut self, n: u32) {
        self.0.extend_from_slice(&n.to_le_bytes());
    }
    fn u64(&mut self, n: u64) {
        self.0.extend_from_slice(&n.to_le_bytes());
    }
    fn f64(&mut self, n: f64) {
        self.u64(if n == 0. { 0 } else { n.to_bits() });
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(n)
            .ok_or(MeshError::CorruptData("length overflow"))?;
        let result = self
            .bytes
            .get(self.offset..end)
            .ok_or(MeshError::CorruptData("truncated input"))?;
        self.offset = end;
        Ok(result)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64> {
        let bits = self.u64()?;
        let value = f64::from_bits(bits);
        if !value.is_finite() || bits == 0x8000000000000000 {
            return Err(MeshError::CorruptData("noncanonical float"));
        }
        Ok(value)
    }
    fn flag(&mut self) -> Result<bool> {
        match self.take(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(MeshError::CorruptData("invalid boolean flag")),
        }
    }
}
