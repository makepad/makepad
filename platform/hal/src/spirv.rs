//! `PipelineCreate`'s binary code payload for dialect::SPIRV. Both stages and the resource
//! metadata come from Cx's existing draw SPIR-V compiler. No source compilation lives in HAL.

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpirvPipeline {
    pub vertex: Vec<u32>,
    pub fragment: Vec<u32>,
    pub texture_base: u32,
    pub sampler_base: u32,
    pub depth_binding: u32,
    pub instance_binding: u32,
    /// Optional pick uniforms and the base of pick twins; NONE for an ordinary pipeline.
    pub pick_binding: u32,
    pub pick_texture_base: u32,
    /// Texture shapes: 0 = 2D, 1 = cube, 2 = array, 3 = depth, 4 = depth array, 5 = uint.
    pub textures: Vec<u32>,
    pub samplers: Vec<SpirvSampler>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpirvSampler {
    /// 0 = nearest, 1 = linear.
    pub filter: u32,
    /// 0 = repeat, 1 = edge, 2 = transparent border, 3 = mirrored repeat.
    pub address: u32,
    pub compare: bool,
    pub pixel: bool,
}

impl SpirvPipeline {
    pub fn encode(&self) -> Vec<u8> {
        let mut words = vec![0x48564b31, self.vertex.len() as u32, self.fragment.len() as u32,
            self.texture_base, self.sampler_base, self.depth_binding, self.instance_binding,
            self.pick_binding, self.pick_texture_base, self.textures.len() as u32, self.samplers.len() as u32];
        words.extend_from_slice(&self.textures);
        for s in &self.samplers { words.extend([s.filter, s.address, u32::from(s.compare), u32::from(s.pixel)]); }
        words.extend_from_slice(&self.vertex);
        words.extend_from_slice(&self.fragment);
        words.into_iter().flat_map(u32::to_le_bytes).collect()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() % 4 != 0 { return Err("SPIR-V payload is not word aligned".into()); }
        let words: Vec<_> = bytes.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
        if words.len() < 11 || words[0] != 0x48564b31 { return Err("invalid HAL SPIR-V payload header".into()); }
        let (nv, nf, nt, ns) = (words[1] as usize, words[2] as usize, words[9] as usize, words[10] as usize);
        let end = ns.checked_mul(4).and_then(|n| n.checked_add(11)).and_then(|n| n.checked_add(nt))
            .and_then(|n| n.checked_add(nv)).and_then(|n| n.checked_add(nf));
        if end != Some(words.len()) || nv < 5 || nf < 5 { return Err("invalid HAL SPIR-V payload lengths".into()); }
        let textures = words[11..11 + nt].to_vec();
        if textures.iter().any(|t| *t > 5) { return Err("invalid SPIR-V texture shape".into()); }
        let mut samplers = Vec::with_capacity(ns);
        for s in words[11 + nt..11 + nt + ns * 4].chunks_exact(4) {
            if s[0] > 1 || s[1] > 3 || s[2] > 1 || s[3] > 1 { return Err("invalid SPIR-V sampler".into()); }
            samplers.push(SpirvSampler { filter: s[0], address: s[1], compare: s[2] != 0, pixel: s[3] != 0 });
        }
        let start = 11 + nt + ns * 4;
        let vertex = words[start..start + nv].to_vec();
        let fragment = words[start + nv..].to_vec();
        for stage in [&vertex, &fragment] {
            if stage[0] != 0x07230203 { return Err("invalid SPIR-V magic".into()); }
            let mut at = 5;
            while at < stage.len() {
                let count = (stage[at] >> 16) as usize;
                if count == 0 || count > stage.len() - at { return Err("truncated SPIR-V instruction".into()); }
                at += count;
            }
        }
        Ok(Self { vertex, fragment, textures, samplers, texture_base: words[3], sampler_base: words[4],
            depth_binding: words[5], instance_binding: words[6], pick_binding: words[7], pick_texture_base: words[8] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shader may declare every texture slot a makepad draw has (up to 17): the payload must
    /// carry more than eight texture shapes (it was capped at eight, so such pipelines failed).
    #[test]
    fn a_payload_with_twelve_textures_round_trips() {
        let stage = vec![0x07230203, 0x00010000, 0, 1, 0];
        let p = SpirvPipeline {
            vertex: stage.clone(), fragment: stage, texture_base: 2, sampler_base: 20, depth_binding: 0,
            instance_binding: 1, pick_binding: u32::MAX, pick_texture_base: 40,
            textures: (0..12).map(|i| i % 3).collect(),
            samplers: vec![SpirvSampler { filter: 1, address: 0, compare: false, pixel: false }],
        };
        let back = SpirvPipeline::decode(&p.encode()).expect("twelve textures decode");
        assert_eq!(back.textures, p.textures);
        assert_eq!(back.texture_base, 2);
    }
}
