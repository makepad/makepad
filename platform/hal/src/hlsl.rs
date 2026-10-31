//! `PipelineCreate`'s code payload for dialect::HLSL: the one HLSL source Cx's shader compiler
//! emitted (`vertex_main` for vs_5_0, `pixel_main` for ps_5_0; registers as
//! platform/shader/src/shader_ir_hlsl.rs documents) and the resource facts D3D11 needs from the
//! compile, which HLSL text does not state in a form the HAL may parse: each texture's shape
//! (the shader-resource-view dimension its declaration takes) and each sampler's state.
//! No source text is inspected or rewritten here or in the backend.

use crate::spirv::SpirvSampler;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HlslPipeline {
    pub source: String,
    /// Texture shapes in register order (t0..): 0 = 2D, 1 = cube, 2 = array, 3 = depth,
    /// 4 = depth array, 5 = uint.
    pub textures: Vec<u32>,
    /// Sampler states in register order (s0..).
    pub samplers: Vec<SpirvSampler>,
}

const MAGIC: u32 = 0x4c53_4c48; // "HLSL"

impl HlslPipeline {
    pub fn encode(&self) -> Vec<u8> {
        let mut words = vec![MAGIC, self.textures.len() as u32, self.samplers.len() as u32, self.source.len() as u32];
        words.extend_from_slice(&self.textures);
        for s in &self.samplers {
            words.extend([s.filter, s.address, u32::from(s.compare), u32::from(s.pixel)]);
        }
        let mut bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        bytes.extend_from_slice(self.source.as_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let word = |i: usize| bytes.get(i * 4..i * 4 + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        if word(0) != Some(MAGIC) {
            return Err("invalid HAL HLSL payload header".into());
        }
        let (nt, ns, nsrc) = (word(1).unwrap_or(0) as usize, word(2).unwrap_or(0) as usize, word(3).unwrap_or(0) as usize);
        let head = ns.checked_mul(4).and_then(|n| n.checked_add(4 + nt)).ok_or("invalid HAL HLSL payload lengths")?;
        if head.checked_mul(4).and_then(|n| n.checked_add(nsrc)) != Some(bytes.len()) {
            return Err("invalid HAL HLSL payload lengths".into());
        }
        let textures: Vec<u32> = (4..4 + nt).map(|i| word(i).unwrap()).collect();
        if textures.iter().any(|t| *t > 5) {
            return Err("invalid HLSL texture shape".into());
        }
        let mut samplers = Vec::with_capacity(ns);
        for k in 0..ns {
            let at = 4 + nt + k * 4;
            let s = [word(at).unwrap(), word(at + 1).unwrap(), word(at + 2).unwrap(), word(at + 3).unwrap()];
            if s[0] > 1 || s[1] > 3 || s[2] > 1 || s[3] > 1 {
                return Err("invalid HLSL sampler".into());
            }
            samplers.push(SpirvSampler { filter: s[0], address: s[1], compare: s[2] != 0, pixel: s[3] != 0 });
        }
        let source = std::str::from_utf8(&bytes[head * 4..]).map_err(|_| "HLSL source is not UTF-8")?.to_owned();
        Ok(Self { source, textures, samplers })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_round_trips() {
        let p = HlslPipeline {
            source: "float4 pixel_main() : SV_TARGET0 { return 1; }".into(),
            textures: vec![0, 1, 2, 3],
            samplers: vec![SpirvSampler { filter: 1, address: 1, compare: true, pixel: false }],
        };
        assert_eq!(HlslPipeline::decode(&p.encode()).unwrap(), p);
        assert!(HlslPipeline::decode(&p.encode()[..10]).is_err());
    }
}
