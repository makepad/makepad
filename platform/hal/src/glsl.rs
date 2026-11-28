//! Complete GLES sources and reflection emitted by the client compiler. Names are data:
//! the executor never inspects, patches or manufactures shader source.

use crate::spirv::SpirvSampler;

const GLSL_PIPELINE_MAGIC: u32 = 0x474c_0003;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    pub offset: u32,
    /// The frame builder's DrawShaderAttrFormat discriminant.
    pub format: u32,
    pub lanes: u32,
    pub instance: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    pub name: String,
    /// Logical texture slot in Draw::tex.
    pub slot: u32,
    /// 0 = 2D, 1 = cube, 2 = array, 3 = depth, 4 = depth array, 7 = volume.
    pub shape: u32,
    /// Bit 0 is vertex, bit 1 is fragment.
    pub stages: u8,
    pub sampler: SpirvSampler,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickTwin {
    pub name: String,
    /// Logical slot in Draw::pick_tex and bit in mp_pick.y.
    pub slot: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlslPipeline {
    pub vertex: String,
    pub fragment: String,
    /// draw, pass, list, dynamic, scope, then the custom blocks in Draw::ubufs order.
    pub blocks: Vec<String>,
    pub attributes: Vec<Attribute>,
    pub textures: Vec<Texture>,
    pub pick_twins: Vec<PickTwin>,
    pub vertex_stride: u32,
    pub instance_stride: u32,
    pub instance_texture: bool,
    pub pick: bool,
}

impl GlslPipeline {
    pub fn encode(&self) -> Vec<u8> {
        fn word(out: &mut Vec<u8>, n: u32) { out.extend(n.to_le_bytes()); }
        fn string(out: &mut Vec<u8>, s: &str) { word(out, s.len() as u32); out.extend(s.as_bytes()); }
        let mut out = Vec::new();
        for n in [GLSL_PIPELINE_MAGIC, self.vertex_stride, self.instance_stride, self.instance_texture as u32,
            self.pick as u32, self.blocks.len() as u32, self.attributes.len() as u32,
            self.textures.len() as u32, self.pick_twins.len() as u32] { word(&mut out, n); }
        string(&mut out, &self.vertex); string(&mut out, &self.fragment);
        for b in &self.blocks { string(&mut out, b); }
        for a in &self.attributes {
            string(&mut out, &a.name);
            for n in [a.offset, a.format, a.lanes, a.instance as u32] { word(&mut out, n); }
        }
        for t in &self.textures {
            string(&mut out, &t.name);
            for n in [t.slot, t.shape, t.stages as u32, t.sampler.filter, t.sampler.address, t.sampler.compare as u32,
                t.sampler.pixel as u32, t.sampler.anisotropy] { word(&mut out, n); }
        }
        for twin in &self.pick_twins {
            string(&mut out, &twin.name);
            word(&mut out, twin.slot);
        }
        out
    }

    pub fn decode(mut bytes: &[u8]) -> Result<Self, String> {
        fn word(b: &mut &[u8]) -> Result<u32, String> {
            let n = b.get(..4).ok_or("truncated GLSL payload")?;
            let n = u32::from_le_bytes(n.try_into().unwrap()); *b = &b[4..]; Ok(n)
        }
        fn string(b: &mut &[u8]) -> Result<String, String> {
            let n = word(b)? as usize;
            let s = std::str::from_utf8(b.get(..n).ok_or("truncated GLSL string")?).map_err(|_| "GLSL payload is not UTF-8")?;
            if s.contains('\0') { return Err("NUL in GLSL payload".into()); }
            let s = s.to_owned(); *b = &b[n..]; Ok(s)
        }
        if word(&mut bytes)? != GLSL_PIPELINE_MAGIC { return Err("invalid GLSL payload version".into()); }
        let vertex_stride = word(&mut bytes)?;
        let instance_stride = word(&mut bytes)?;
        let instance_texture = word(&mut bytes)?;
        let pick = word(&mut bytes)?;
        let nb = word(&mut bytes)?; let na = word(&mut bytes)?; let nt = word(&mut bytes)?;
        let twins = word(&mut bytes)?;
        if instance_texture > 1 || pick > 1 || !(5..=9).contains(&nb) { return Err("invalid GLSL reflection".into()); }
        let vertex = string(&mut bytes)?; let fragment = string(&mut bytes)?;
        let mut blocks = Vec::new(); let mut attributes = Vec::new(); let mut textures = Vec::new();
        for _ in 0..nb { blocks.push(string(&mut bytes)?); }
        for _ in 0..na {
            let name = string(&mut bytes)?;
            let offset = word(&mut bytes)?; let format = word(&mut bytes)?;
            let lanes = word(&mut bytes)?; let instance = word(&mut bytes)?;
            if format > 13 || !(1..=4).contains(&lanes) || instance > 1 { return Err("invalid GL attribute".into()); }
            attributes.push(Attribute { name, offset, format, lanes, instance: instance != 0 });
        }
        for _ in 0..nt {
            let name = string(&mut bytes)?;
            let slot = word(&mut bytes)?; let shape = word(&mut bytes)?; let stages = word(&mut bytes)?;
            if !(1..=3).contains(&stages) { return Err("invalid GLSL texture stages".into()); }
            let filter = word(&mut bytes)?; let address = word(&mut bytes)?;
            let compare = word(&mut bytes)?; let pixel = word(&mut bytes)?; let anisotropy = word(&mut bytes)?;
            if !matches!(shape, 0..=4 | crate::TEXTURE_SHAPE_3D) || filter > 1 || address > 3 || compare > 1 || pixel > 1 || anisotropy == 0 { return Err("invalid GL texture/sampler".into()); }
            textures.push(Texture { name, slot, shape, stages: stages as u8, sampler: SpirvSampler { filter, address, compare: compare != 0, pixel: pixel != 0, anisotropy } });
        }
        let mut pick_twins = Vec::new();
        let mut mask = 0u32;
        for _ in 0..twins {
            let name = string(&mut bytes)?;
            let slot = word(&mut bytes)?;
            if pick == 0 || slot >= 32 || mask & (1 << slot) != 0 {
                return Err("invalid GLSL pick twin slot".into());
            }
            mask |= 1 << slot;
            pick_twins.push(PickTwin { name, slot });
        }
        if !bytes.is_empty() { return Err("trailing GLSL payload bytes".into()); }
        Ok(Self { vertex, fragment, blocks, attributes, textures, pick_twins, vertex_stride, instance_stride,
            instance_texture: instance_texture != 0, pick: pick != 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glsl_pipeline_v3_round_trips_multi_sampler_and_pick_twins() {
        let mut p = GlslPipeline {
            vertex: "void main() {}".into(),
            fragment: "void main() {}".into(),
            blocks: ["draw_callUniforms", "passUniforms", "draw_listUniforms", "userUniforms", "liveUniforms", "my_Uniforms"].map(String::from).to_vec(),
            attributes: vec![Attribute { name: "geom_pos".into(), offset: 8, format: 4, lanes: 1, instance: false }],
            textures: vec![
                Texture { name: "tex_image".into(), slot: 0, shape: 0, stages: 3, sampler: SpirvSampler { filter: 1, address: 3, compare: false, pixel: false, anisotropy: 1 } },
                Texture { name: "mp_s1_image".into(), slot: 0, shape: 0, stages: 2, sampler: SpirvSampler { filter: 1, address: 0, compare: false, pixel: false, anisotropy: 16 } },
                Texture { name: "tex_volume".into(), slot: 3, shape: crate::TEXTURE_SHAPE_3D, stages: 1, sampler: SpirvSampler { filter: 0, address: 1, compare: false, pixel: false, anisotropy: 1 } },
            ],
            pick_twins: vec![PickTwin { name: "mp_pick_tex_image".into(), slot: 0 }],
            vertex_stride: 12,
            instance_stride: 64,
            instance_texture: true,
            pick: true,
        };
        let mut bytes = p.encode();
        assert_eq!(bytes[..4], 0x474c_0003u32.to_le_bytes());
        assert_eq!(GlslPipeline::decode(&bytes).unwrap(), p);
        assert!(GlslPipeline::decode(&bytes[..20]).is_err());
        bytes[..4].copy_from_slice(&0x474c_0002u32.to_le_bytes());
        assert_eq!(GlslPipeline::decode(&bytes).unwrap_err(), "invalid GLSL payload version");
        for invalid in [0, 4] {
            p.textures[0].stages = invalid;
            assert_eq!(GlslPipeline::decode(&p.encode()).unwrap_err(), "invalid GLSL texture stages");
        }
    }
}
