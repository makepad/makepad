//! Complete GLES sources and reflection emitted by the client compiler. Names are data:
//! the executor never inspects, patches or manufactures shader source.

use crate::spirv::SpirvSampler;

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
    /// 0 = 2D, 1 = cube, 2 = array, 3 = depth, 4 = depth array.
    pub shape: u32,
    pub sampler: SpirvSampler,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlslPipeline {
    pub vertex: String,
    pub fragment: String,
    /// draw, pass, list, dynamic, scope, then the custom blocks in Draw::ubufs order.
    pub blocks: Vec<String>,
    pub attributes: Vec<Attribute>,
    pub textures: Vec<Texture>,
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
        for n in [0x474c_0001, self.vertex_stride, self.instance_stride, self.instance_texture as u32,
            self.pick as u32, self.blocks.len() as u32, self.attributes.len() as u32, self.textures.len() as u32] { word(&mut out, n); }
        string(&mut out, &self.vertex); string(&mut out, &self.fragment);
        for b in &self.blocks { string(&mut out, b); }
        for a in &self.attributes {
            string(&mut out, &a.name);
            for n in [a.offset, a.format, a.lanes, a.instance as u32] { word(&mut out, n); }
        }
        for t in &self.textures {
            string(&mut out, &t.name);
            for n in [t.shape, t.sampler.filter, t.sampler.address, t.sampler.compare as u32, t.sampler.pixel as u32] { word(&mut out, n); }
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
        if word(&mut bytes)? != 0x474c_0001 { return Err("invalid GLSL payload version".into()); }
        let vertex_stride = word(&mut bytes)?;
        let instance_stride = word(&mut bytes)?;
        let instance_texture = word(&mut bytes)?;
        let pick = word(&mut bytes)?;
        let nb = word(&mut bytes)?; let na = word(&mut bytes)?; let nt = word(&mut bytes)?;
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
            let shape = word(&mut bytes)?; let filter = word(&mut bytes)?; let address = word(&mut bytes)?;
            let compare = word(&mut bytes)?; let pixel = word(&mut bytes)?;
            if shape > 4 || filter > 1 || address > 3 || compare > 1 || pixel > 1 { return Err("invalid GL texture/sampler".into()); }
            textures.push(Texture { name, shape, sampler: SpirvSampler { filter, address, compare: compare != 0, pixel: pixel != 0 } });
        }
        if !bytes.is_empty() { return Err("trailing GLSL payload bytes".into()); }
        Ok(Self { vertex, fragment, blocks, attributes, textures, vertex_stride, instance_stride,
            instance_texture: instance_texture != 0, pick: pick != 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_round_trips() {
        let p = GlslPipeline {
            vertex: "void main() {}".into(),
            fragment: "void main() {}".into(),
            blocks: ["draw_callUniforms", "passUniforms", "draw_listUniforms", "userUniforms", "liveUniforms", "my_Uniforms"].map(String::from).to_vec(),
            attributes: vec![Attribute { name: "geom_pos".into(), offset: 8, format: 4, lanes: 1, instance: false }],
            textures: vec![Texture { name: "tex_image".into(), shape: 2, sampler: SpirvSampler { filter: 1, address: 3, compare: false, pixel: false } }],
            vertex_stride: 12,
            instance_stride: 64,
            instance_texture: true,
            pick: true,
        };
        assert_eq!(GlslPipeline::decode(&p.encode()).unwrap(), p);
        assert!(GlslPipeline::decode(&p.encode()[..20]).is_err());
    }
}
