//! The generator contract shared by all PBR backends, plus a deterministic,
//! dependency-free test backend. The test backend exists so service plumbing,
//! UI, and asset-server integration can be built and regression-tested without
//! GPUs or checkpoints: same seed, same bytes, on every platform.

use crate::contract::{
    pack_orm, ChannelSlot, ColorSpace, PbrMap, PbrMaterialSet, PbrMeta, PixelFormat,
};
use crate::mesh::TriMesh;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PbrError {
    Cancelled,
    InvalidParams(String),
    /// Fail-closed: native CUDA runtime or pinned checkpoints are not present
    /// on this host. Never silently substituted.
    Unavailable(String),
    Internal(String),
}

impl std::fmt::Display for PbrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PbrError::Cancelled => write!(f, "cancelled"),
            PbrError::InvalidParams(m) => write!(f, "invalid params: {m}"),
            PbrError::Unavailable(m) => write!(f, "unavailable (fail-closed): {m}"),
            PbrError::Internal(m) => write!(f, "internal: {m}"),
        }
    }
}

impl std::error::Error for PbrError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PbrStage {
    Prepare,
    ViewSelect,
    GeometryRender,
    Encode,
    Denoise,
    Decode,
    Upscale,
    Bake,
    Inpaint,
    Finalize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PbrProgress {
    pub stage: PbrStage,
    pub current: u32,
    pub total: u32,
}

#[derive(Clone, Debug)]
pub struct PbrJobParams {
    pub seed: u64,
    pub texture_size: u32,
    /// Target mesh; optional for procedural backends, required by mesh-conditioned ones.
    pub mesh: Option<TriMesh>,
    /// Opaque reference image bytes (PNG); consumed by image-conditioned backends.
    pub reference_image_png: Option<Vec<u8>>,
    pub requested_views: u32,
}

impl Default for PbrJobParams {
    fn default() -> Self {
        Self {
            seed: 0,
            texture_size: 256,
            mesh: None,
            reference_image_png: None,
            requested_views: 6,
        }
    }
}

/// A PBR generator. The progress callback returns `false` to cancel; backends
/// must honor it promptly and return [`PbrError::Cancelled`].
pub trait PbrGenerator {
    fn model_id(&self) -> &'static str;
    fn generate(
        &mut self,
        params: &PbrJobParams,
        progress: &mut dyn FnMut(PbrProgress) -> bool,
    ) -> Result<PbrMaterialSet, PbrError>;
}

/// Seeded procedural PBR generator with byte-stable output (integer math only
/// in pixel synthesis). Generates albedo/roughness/metallic; normal and
/// occlusion are honestly absent, and the packed ORM carries neutral R.
pub struct DeterministicTestPbr;

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

fn pixel_hash(x: u32, y: u32, seed: u64) -> u64 {
    let mut state = seed
        ^ (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (y as u64).wrapping_mul(0xc2b2ae3d27d4eb4f);
    splitmix64(&mut state)
}

impl DeterministicTestPbr {
    const MODEL_ID: &'static str = "pbr-testpattern-v1";

    fn palette(seed: u64) -> [[u8; 3]; 4] {
        let mut state = seed;
        let mut palette = [[0u8; 3]; 4];
        for color in &mut palette {
            let bits = splitmix64(&mut state);
            // Mid-range colors so jitter never clips.
            color[0] = 64 + ((bits >> 8) & 0x7f) as u8;
            color[1] = 64 + ((bits >> 24) & 0x7f) as u8;
            color[2] = 64 + ((bits >> 40) & 0x7f) as u8;
        }
        palette
    }
}

impl PbrGenerator for DeterministicTestPbr {
    fn model_id(&self) -> &'static str {
        Self::MODEL_ID
    }

    fn generate(
        &mut self,
        params: &PbrJobParams,
        progress: &mut dyn FnMut(PbrProgress) -> bool,
    ) -> Result<PbrMaterialSet, PbrError> {
        let size = params.texture_size;
        if !(8..=8192).contains(&size) {
            return Err(PbrError::InvalidParams(format!(
                "texture_size {size} outside 8..=8192"
            )));
        }
        let emit = |progress: &mut dyn FnMut(PbrProgress) -> bool,
                    stage: PbrStage,
                    current: u32,
                    total: u32|
         -> Result<(), PbrError> {
            if progress(PbrProgress { stage, current, total }) {
                Ok(())
            } else {
                Err(PbrError::Cancelled)
            }
        };

        emit(progress, PbrStage::Prepare, 1, 1)?;
        let w = size;
        let h = size;
        let palette = Self::palette(params.seed);

        let mut albedo = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let cell = ((x / 32) + (y / 32)) & 3;
                let base = palette[cell as usize];
                let bits = pixel_hash(x, y, params.seed);
                for (c, b) in base.iter().enumerate() {
                    let jitter = ((bits >> (c * 8)) & 0xf) as i32 - 8;
                    albedo.push((*b as i32 + jitter).clamp(0, 255) as u8);
                }
            }
        }
        emit(progress, PbrStage::Denoise, 1, 3)?;

        let mut rough = Vec::with_capacity((w * h) as usize);
        for _y in 0..h {
            for x in 0..w {
                rough.push(((x as u64 * 255) / (w as u64 - 1)) as u8);
            }
        }
        emit(progress, PbrStage::Denoise, 2, 3)?;

        let mut metal = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for _x in 0..w {
                metal.push(if (y / 32) % 2 == 0 { 25 } else { 230 });
            }
        }
        emit(progress, PbrStage::Denoise, 3, 3)?;

        let albedo = PbrMap {
            width: w,
            height: h,
            format: PixelFormat::Rgb8,
            color_space: ColorSpace::Srgb,
            data: albedo,
        };
        let rough = PbrMap {
            width: w,
            height: h,
            format: PixelFormat::Gray8,
            color_space: ColorSpace::Linear,
            data: rough,
        };
        let metal = PbrMap {
            width: w,
            height: h,
            format: PixelFormat::Gray8,
            color_space: ColorSpace::Linear,
            data: metal,
        };
        emit(progress, PbrStage::Bake, 1, 1)?;
        let packed = pack_orm(None, &rough, &metal).map_err(|e| PbrError::Internal(e.to_string()))?;

        let set = PbrMaterialSet {
            albedo: ChannelSlot::generated(Self::MODEL_ID, albedo),
            normal: ChannelSlot::absent("test backend does not synthesize normal detail"),
            roughness: ChannelSlot::generated(Self::MODEL_ID, rough),
            metallic: ChannelSlot::generated(Self::MODEL_ID, metal),
            occlusion: ChannelSlot::absent(
                "test backend does not synthesize occlusion; packed ORM R is neutral 255",
            ),
            packed_orm: Some(packed),
            meta: PbrMeta::new(Self::MODEL_ID, "none", params.seed, 0),
        };
        set.validate().map_err(|e| PbrError::Internal(e.to_string()))?;
        emit(progress, PbrStage::Finalize, 1, 1)?;
        Ok(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ChannelOrigin, NEUTRAL_OCCLUSION};
    use crate::digest;

    fn run(seed: u64, size: u32) -> PbrMaterialSet {
        let mut backend = DeterministicTestPbr;
        let params = PbrJobParams {
            seed,
            texture_size: size,
            ..Default::default()
        };
        backend.generate(&params, &mut |_| true).unwrap()
    }

    fn combined_digest(set: &PbrMaterialSet) -> String {
        let mut h = digest::Sha256::new();
        h.update(&set.albedo.map.as_ref().unwrap().data);
        h.update(&set.roughness.map.as_ref().unwrap().data);
        h.update(&set.metallic.map.as_ref().unwrap().data);
        h.update(&set.packed_orm.as_ref().unwrap().data);
        h.update(set.manifest_json().as_bytes());
        digest::hex(&h.finalize())
    }

    #[test]
    fn deterministic_across_runs() {
        let a = run(42, 64);
        let b = run(42, 64);
        assert_eq!(combined_digest(&a), combined_digest(&b));
        assert_eq!(a.manifest_json(), b.manifest_json());
    }

    #[test]
    fn golden_digest_seed42_64() {
        // Pinned golden digest: any byte change in the procedural output or
        // the manifest format is a deliberate, reviewed change.
        assert_eq!(
            combined_digest(&run(42, 64)),
            "1b1ea6c681f442759ddb63a5080f9f63aee0912d02c873791e3fa8337464f58e"
        );
    }

    #[test]
    fn seeds_differ() {
        let a = run(1, 64);
        let b = run(2, 64);
        assert_ne!(
            a.albedo.map.as_ref().unwrap().data,
            b.albedo.map.as_ref().unwrap().data
        );
    }

    #[test]
    fn honest_absence_and_neutral_r() {
        let set = run(7, 64);
        set.validate().unwrap();
        assert!(matches!(set.normal.origin, ChannelOrigin::Absent { .. }));
        assert!(matches!(set.occlusion.origin, ChannelOrigin::Absent { .. }));
        let orm = set.packed_orm.as_ref().unwrap();
        assert!(orm.data.chunks_exact(3).all(|px| px[0] == NEUTRAL_OCCLUSION));
    }

    #[test]
    fn cancellation_honored() {
        let mut backend = DeterministicTestPbr;
        let params = PbrJobParams {
            seed: 5,
            texture_size: 64,
            ..Default::default()
        };
        let mut calls = 0;
        let result = backend.generate(&params, &mut |_| {
            calls += 1;
            calls < 2
        });
        assert_eq!(result.unwrap_err(), PbrError::Cancelled);
        assert_eq!(calls, 2);
    }

    #[test]
    fn invalid_size_rejected() {
        let mut backend = DeterministicTestPbr;
        let params = PbrJobParams {
            texture_size: 4,
            ..Default::default()
        };
        assert!(matches!(
            backend.generate(&params, &mut |_| true),
            Err(PbrError::InvalidParams(_))
        ));
    }
}
