//! The generative-PBR channel contract.
//!
//! A material set carries five semantic channels: albedo (base color), normal,
//! roughness, metallic, and ambient occlusion. Every channel states its origin
//! explicitly; a model that cannot generate a channel produces an *honest
//! absence* with a reason, never a fabricated map. Consumers (GLB writer,
//! asset server, viewers) rely on:
//!
//! * albedo is sRGB; every other stored map is linear;
//! * the packed texture is **ORM** (Asset Server `FileRole::Orm`):
//!   R = occlusion, G = roughness, B = metallic — the common glTF layout where
//!   `occlusionTexture` (R) shares one image with `metallicRoughnessTexture`
//!   (G/B). When occlusion is absent, R is uniformly [`NEUTRAL_OCCLUSION`];
//! * normal maps are tangent-space with +Y up (OpenGL/glTF convention);
//! * maps target the mesh's UV0 atlas, assumed non-overlapping in `[0,1]`.

use crate::digest;

/// Neutral ambient-occlusion value used for the packed R channel when the
/// occlusion channel is honestly absent: 1.0 (fully unoccluded), the identity
/// for the multiplicative AO term.
pub const NEUTRAL_OCCLUSION: u8 = 255;

/// Public map inputs are deliberately bounded before byte-count arithmetic or
/// allocation. 8K is already larger than the current Paint execution profiles
/// and keeps malformed standalone contract values from requesting huge buffers.
pub const MAX_MAP_DIMENSION: u32 = 8_192;
pub const MAX_MAP_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PbrChannel {
    Albedo,
    Normal,
    Roughness,
    Metallic,
    Occlusion,
}

impl PbrChannel {
    pub fn all() -> [PbrChannel; 5] {
        [
            PbrChannel::Albedo,
            PbrChannel::Normal,
            PbrChannel::Roughness,
            PbrChannel::Metallic,
            PbrChannel::Occlusion,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            PbrChannel::Albedo => "albedo",
            PbrChannel::Normal => "normal",
            PbrChannel::Roughness => "roughness",
            PbrChannel::Metallic => "metallic",
            PbrChannel::Occlusion => "occlusion",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    Srgb,
    Linear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Gray8,
    Rgb8,
    Rgba8,
}

impl PixelFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Gray8 => 1,
            PixelFormat::Rgb8 => 3,
            PixelFormat::Rgba8 => 4,
        }
    }
}

/// Where a channel's content came from. `Absent` is a first-class outcome:
/// the honest statement that neither the model nor geometry produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelOrigin {
    /// Produced by a generative model (identified by backend/model id).
    Generated { model: String },
    /// Derived deterministically from mesh geometry (e.g. baked AO, mesh normals).
    GeometryDerived { method: String },
    /// Not produced. The reason is part of the contract and surfaces to consumers.
    Absent { reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PbrMap {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub color_space: ColorSpace,
    pub data: Vec<u8>,
}

impl PbrMap {
    pub fn expected_len(&self) -> Result<usize, MapLayoutError> {
        checked_map_len(self.width, self.height, self.format)
    }

    pub fn digest_hex(&self) -> String {
        digest::sha256_hex(&self.data)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapLayoutError {
    ZeroDimension,
    DimensionTooLarge { width: u32, height: u32 },
    ByteLengthOverflow,
    ByteLengthTooLarge { bytes: usize },
    AllocationFailed { bytes: usize },
}

impl std::fmt::Display for MapLayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroDimension => write!(f, "map dimensions must be nonzero"),
            Self::DimensionTooLarge { width, height } => write!(
                f,
                "map dimensions {width}x{height} exceed {MAX_MAP_DIMENSION} per axis"
            ),
            Self::ByteLengthOverflow => write!(f, "map byte length overflows address space"),
            Self::ByteLengthTooLarge { bytes } => {
                write!(f, "map byte length {bytes} exceeds limit {MAX_MAP_BYTES}")
            }
            Self::AllocationFailed { bytes } => {
                write!(f, "could not allocate {bytes} map bytes")
            }
        }
    }
}

impl std::error::Error for MapLayoutError {}

pub fn checked_map_len(
    width: u32,
    height: u32,
    format: PixelFormat,
) -> Result<usize, MapLayoutError> {
    if width == 0 || height == 0 {
        return Err(MapLayoutError::ZeroDimension);
    }
    if width > MAX_MAP_DIMENSION || height > MAX_MAP_DIMENSION {
        return Err(MapLayoutError::DimensionTooLarge { width, height });
    }
    let pixels = usize::try_from(width)
        .ok()
        .and_then(|width| usize::try_from(height).ok().and_then(|height| width.checked_mul(height)))
        .ok_or(MapLayoutError::ByteLengthOverflow)?;
    let bytes = pixels
        .checked_mul(format.bytes_per_pixel())
        .ok_or(MapLayoutError::ByteLengthOverflow)?;
    if bytes > MAX_MAP_BYTES {
        return Err(MapLayoutError::ByteLengthTooLarge { bytes });
    }
    Ok(bytes)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChannelSlot {
    pub origin: ChannelOrigin,
    pub map: Option<PbrMap>,
}

impl ChannelSlot {
    pub fn absent(reason: &str) -> Self {
        Self {
            origin: ChannelOrigin::Absent {
                reason: reason.to_string(),
            },
            map: None,
        }
    }

    pub fn generated(model: &str, map: PbrMap) -> Self {
        Self {
            origin: ChannelOrigin::Generated {
                model: model.to_string(),
            },
            map: Some(map),
        }
    }

    pub fn geometry_derived(method: &str, map: PbrMap) -> Self {
        Self {
            origin: ChannelOrigin::GeometryDerived {
                method: method.to_string(),
            },
            map: Some(map),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PbrMeta {
    /// Backend/model identity that produced this set.
    pub generator: String,
    /// Pinned checkpoint revision, or "none" for procedural backends.
    pub checkpoint_revision: String,
    pub seed: u64,
    /// Number of diffusion views used (0 for procedural backends).
    pub views_used: u32,
    pub uv_set: &'static str,
    pub uv_assumption: &'static str,
    pub normal_convention: &'static str,
    pub orm_packing: &'static str,
    /// Backend-specific provenance pairs (pinned revisions, license identity,
    /// operator-visible facts). Rendered into the manifest when non-empty.
    pub extra: Vec<(String, String)>,
}

impl PbrMeta {
    pub fn new(generator: &str, checkpoint_revision: &str, seed: u64, views_used: u32) -> Self {
        Self {
            generator: generator.to_string(),
            checkpoint_revision: checkpoint_revision.to_string(),
            seed,
            views_used,
            uv_set: "UV0",
            uv_assumption: "single non-overlapping atlas in [0,1], no mirroring",
            normal_convention: "tangent-space, +Y up (OpenGL/glTF)",
            orm_packing: "ORM: R=occlusion (neutral 255 when absent), G=roughness, B=metallic",
            extra: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PbrMaterialSet {
    pub albedo: ChannelSlot,
    pub normal: ChannelSlot,
    pub roughness: ChannelSlot,
    pub metallic: ChannelSlot,
    pub occlusion: ChannelSlot,
    /// Packed ORM texture (linear Rgb8): R = occlusion, G = roughness,
    /// B = metallic. If the occlusion channel is absent, R must be uniformly
    /// [`NEUTRAL_OCCLUSION`]; if standalone maps are present, the packed
    /// channels must equal them byte-for-byte.
    pub packed_orm: Option<PbrMap>,
    pub meta: PbrMeta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractError {
    AbsentWithMap(PbrChannel),
    PresentWithoutMap(PbrChannel),
    ZeroDimension(PbrChannel),
    DataLengthMismatch(PbrChannel),
    WrongColorSpace(PbrChannel),
    WrongFormat(PbrChannel),
    MapLayout(PbrChannel, MapLayoutError),
    AtlasDimensionMismatch(PbrChannel),
    PackedOrmMapLayout(MapLayoutError),
    PackedOrmWrongFormat,
    PackedOrmDataLengthMismatch,
    PackedOrmDimensionMismatch,
    PackedOrmOcclusionMismatch,
    PackedOrmRoughnessMismatch,
    PackedOrmMetallicMismatch,
    PackedOrmNeutralRViolation,
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContractError::AbsentWithMap(c) => write!(f, "channel {} marked absent but carries a map", c.name()),
            ContractError::PresentWithoutMap(c) => write!(f, "channel {} marked present but has no map", c.name()),
            ContractError::ZeroDimension(c) => write!(f, "channel {} has a zero dimension", c.name()),
            ContractError::DataLengthMismatch(c) => write!(f, "channel {} data length mismatch", c.name()),
            ContractError::WrongColorSpace(c) => write!(f, "channel {} violates the color-space law", c.name()),
            ContractError::WrongFormat(c) => write!(f, "channel {} has a disallowed pixel format", c.name()),
            ContractError::MapLayout(c, error) => {
                write!(f, "channel {} has invalid map layout: {error}", c.name())
            }
            ContractError::AtlasDimensionMismatch(c) => write!(
                f,
                "channel {} dimensions differ from the material atlas",
                c.name()
            ),
            ContractError::PackedOrmMapLayout(error) => {
                write!(f, "packed ORM has invalid map layout: {error}")
            }
            ContractError::PackedOrmWrongFormat => write!(f, "packed ORM map must be linear Rgb8"),
            ContractError::PackedOrmDataLengthMismatch => write!(f, "packed ORM data length mismatch"),
            ContractError::PackedOrmDimensionMismatch => {
                write!(f, "packed ORM dimensions differ from standalone maps")
            }
            ContractError::PackedOrmOcclusionMismatch => {
                write!(f, "packed ORM R channel differs from occlusion map")
            }
            ContractError::PackedOrmRoughnessMismatch => {
                write!(f, "packed ORM G channel differs from roughness map")
            }
            ContractError::PackedOrmMetallicMismatch => {
                write!(f, "packed ORM B channel differs from metallic map")
            }
            ContractError::PackedOrmNeutralRViolation => write!(
                f,
                "occlusion is absent but packed ORM R channel is not uniformly the neutral value"
            ),
        }
    }
}

impl std::error::Error for ContractError {}

impl PbrMaterialSet {
    pub fn slot(&self, channel: PbrChannel) -> &ChannelSlot {
        match channel {
            PbrChannel::Albedo => &self.albedo,
            PbrChannel::Normal => &self.normal,
            PbrChannel::Roughness => &self.roughness,
            PbrChannel::Metallic => &self.metallic,
            PbrChannel::Occlusion => &self.occlusion,
        }
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        let mut atlas_dimensions = None;
        for channel in PbrChannel::all() {
            let slot = self.slot(channel);
            let absent = matches!(slot.origin, ChannelOrigin::Absent { .. });
            match (&slot.map, absent) {
                (Some(_), true) => return Err(ContractError::AbsentWithMap(channel)),
                (None, false) => return Err(ContractError::PresentWithoutMap(channel)),
                (None, true) => continue,
                (Some(map), false) => {
                    let expected_len = map.expected_len().map_err(|error| match error {
                        MapLayoutError::ZeroDimension => ContractError::ZeroDimension(channel),
                        error => ContractError::MapLayout(channel, error),
                    })?;
                    if map.data.len() != expected_len {
                        return Err(ContractError::DataLengthMismatch(channel));
                    }
                    match atlas_dimensions {
                        Some(dimensions) if dimensions != (map.width, map.height) => {
                            return Err(ContractError::AtlasDimensionMismatch(channel));
                        }
                        None => atlas_dimensions = Some((map.width, map.height)),
                        _ => {}
                    }
                    let want_space = match channel {
                        PbrChannel::Albedo => ColorSpace::Srgb,
                        _ => ColorSpace::Linear,
                    };
                    if map.color_space != want_space {
                        return Err(ContractError::WrongColorSpace(channel));
                    }
                    let format_ok = match channel {
                        PbrChannel::Albedo => matches!(map.format, PixelFormat::Rgb8 | PixelFormat::Rgba8),
                        PbrChannel::Normal => matches!(map.format, PixelFormat::Rgb8),
                        PbrChannel::Roughness | PbrChannel::Metallic | PbrChannel::Occlusion => {
                            matches!(map.format, PixelFormat::Gray8)
                        }
                    };
                    if !format_ok {
                        return Err(ContractError::WrongFormat(channel));
                    }
                }
            }
        }
        if let Some(orm) = &self.packed_orm {
            let expected_len = orm
                .expected_len()
                .map_err(ContractError::PackedOrmMapLayout)?;
            if orm.format != PixelFormat::Rgb8 || orm.color_space != ColorSpace::Linear {
                return Err(ContractError::PackedOrmWrongFormat);
            }
            if orm.data.len() != expected_len {
                return Err(ContractError::PackedOrmDataLengthMismatch);
            }
            if atlas_dimensions.is_some_and(|dimensions| dimensions != (orm.width, orm.height)) {
                return Err(ContractError::PackedOrmDimensionMismatch);
            }
            let check = |map: &Option<PbrMap>,
                         channel_offset: usize,
                         mismatch: ContractError|
             -> Result<(), ContractError> {
                if let Some(map) = map {
                    if map.width != orm.width || map.height != orm.height {
                        return Err(ContractError::PackedOrmDimensionMismatch);
                    }
                    for (i, v) in map.data.iter().enumerate() {
                        if orm.data[i * 3 + channel_offset] != *v {
                            return Err(mismatch);
                        }
                    }
                }
                Ok(())
            };
            check(&self.occlusion.map, 0, ContractError::PackedOrmOcclusionMismatch)?;
            check(&self.roughness.map, 1, ContractError::PackedOrmRoughnessMismatch)?;
            check(&self.metallic.map, 2, ContractError::PackedOrmMetallicMismatch)?;
            if self.occlusion.map.is_none() {
                // Honest absence still publishes a well-defined R: the neutral value.
                if orm.data.chunks_exact(3).any(|px| px[0] != NEUTRAL_OCCLUSION) {
                    return Err(ContractError::PackedOrmNeutralRViolation);
                }
            }
        }
        Ok(())
    }

    /// Deterministic JSON manifest describing the set: per-channel origin,
    /// dimensions and content digest, plus generation metadata. Field order is
    /// fixed so the manifest itself is byte-stable for golden tests.
    pub fn manifest_json(&self) -> String {
        let mut s = String::with_capacity(1024);
        s.push_str("{\n  \"contract\": \"pbr-material-set-v1\",\n");
        s.push_str(&format!("  \"generator\": \"{}\",\n", esc(&self.meta.generator)));
        s.push_str(&format!(
            "  \"checkpoint_revision\": \"{}\",\n",
            esc(&self.meta.checkpoint_revision)
        ));
        s.push_str(&format!("  \"seed\": {},\n", self.meta.seed));
        s.push_str(&format!("  \"views_used\": {},\n", self.meta.views_used));
        s.push_str(&format!("  \"uv_set\": \"{}\",\n", esc(self.meta.uv_set)));
        s.push_str(&format!("  \"uv_assumption\": \"{}\",\n", esc(self.meta.uv_assumption)));
        s.push_str(&format!(
            "  \"normal_convention\": \"{}\",\n",
            esc(self.meta.normal_convention)
        ));
        s.push_str(&format!("  \"orm_packing\": \"{}\",\n", esc(self.meta.orm_packing)));
        if !self.meta.extra.is_empty() {
            s.push_str("  \"provenance\": {\n");
            for (i, (k, v)) in self.meta.extra.iter().enumerate() {
                let comma = if i + 1 == self.meta.extra.len() { "" } else { "," };
                s.push_str(&format!("    \"{}\": \"{}\"{}\n", esc(k), esc(v), comma));
            }
            s.push_str("  },\n");
        }
        s.push_str("  \"channels\": {\n");
        let all = PbrChannel::all();
        for (idx, channel) in all.iter().enumerate() {
            let slot = self.slot(*channel);
            s.push_str(&format!("    \"{}\": {{ ", channel.name()));
            match &slot.origin {
                ChannelOrigin::Generated { model } => {
                    s.push_str(&format!("\"origin\": \"generated\", \"model\": \"{}\"", esc(model)));
                }
                ChannelOrigin::GeometryDerived { method } => {
                    s.push_str(&format!(
                        "\"origin\": \"geometry_derived\", \"method\": \"{}\"",
                        esc(method)
                    ));
                }
                ChannelOrigin::Absent { reason } => {
                    s.push_str(&format!("\"origin\": \"absent\", \"reason\": \"{}\"", esc(reason)));
                }
            }
            if let Some(map) = &slot.map {
                s.push_str(&format!(
                    ", \"width\": {}, \"height\": {}, \"sha256\": \"{}\"",
                    map.width,
                    map.height,
                    map.digest_hex()
                ));
            }
            s.push_str(" }");
            if idx + 1 != all.len() || self.packed_orm.is_some() {
                s.push(',');
            }
            s.push('\n');
        }
        if let Some(orm) = &self.packed_orm {
            s.push_str(&format!(
                "    \"packed_orm\": {{ \"origin\": \"packing\", \"neutral_occlusion\": {}, \"width\": {}, \"height\": {}, \"sha256\": \"{}\" }}\n",
                if self.occlusion.map.is_none() { NEUTRAL_OCCLUSION as i32 } else { -1 },
                orm.width,
                orm.height,
                orm.digest_hex()
            ));
        }
        s.push_str("  }\n}\n");
        s
    }
}

/// Build the packed ORM map from present standalone channels, applying the
/// neutral-R law when occlusion is absent. Roughness and metallic must be
/// present and share dimensions.
pub fn pack_orm(
    occlusion: Option<&PbrMap>,
    roughness: &PbrMap,
    metallic: &PbrMap,
) -> Result<PbrMap, ContractError> {
    let validate_scalar = |map: &PbrMap| -> Result<usize, ContractError> {
        if map.format != PixelFormat::Gray8 || map.color_space != ColorSpace::Linear {
            return Err(ContractError::PackedOrmWrongFormat);
        }
        let expected = map
            .expected_len()
            .map_err(ContractError::PackedOrmMapLayout)?;
        if map.data.len() != expected {
            return Err(ContractError::PackedOrmDataLengthMismatch);
        }
        Ok(expected)
    };
    let count = validate_scalar(roughness)?;
    validate_scalar(metallic)?;
    if roughness.width != metallic.width || roughness.height != metallic.height {
        return Err(ContractError::PackedOrmDimensionMismatch);
    }
    if let Some(occ) = occlusion {
        validate_scalar(occ)?;
        if occ.width != roughness.width || occ.height != roughness.height {
            return Err(ContractError::PackedOrmDimensionMismatch);
        }
    }
    let output_len = count
        .checked_mul(PixelFormat::Rgb8.bytes_per_pixel())
        .ok_or(ContractError::PackedOrmMapLayout(
            MapLayoutError::ByteLengthOverflow,
        ))?;
    if output_len > MAX_MAP_BYTES {
        return Err(ContractError::PackedOrmMapLayout(
            MapLayoutError::ByteLengthTooLarge { bytes: output_len },
        ));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(output_len).map_err(|_| {
        ContractError::PackedOrmMapLayout(MapLayoutError::AllocationFailed {
            bytes: output_len,
        })
    })?;
    for i in 0..count {
        data.push(match occlusion {
            Some(occ) => occ.data[i],
            None => NEUTRAL_OCCLUSION,
        });
        data.push(roughness.data[i]);
        data.push(metallic.data[i]);
    }
    Ok(PbrMap {
        width: roughness.width,
        height: roughness.height,
        format: PixelFormat::Rgb8,
        color_space: ColorSpace::Linear,
        data,
    })
}

fn esc(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(width: u32, height: u32, value: u8) -> PbrMap {
        PbrMap {
            width,
            height,
            format: PixelFormat::Gray8,
            color_space: ColorSpace::Linear,
            data: vec![value; (width * height) as usize],
        }
    }

    fn valid_set() -> PbrMaterialSet {
        let w = 4;
        let h = 4;
        let albedo = PbrMap {
            width: w,
            height: h,
            format: PixelFormat::Rgb8,
            color_space: ColorSpace::Srgb,
            data: vec![200; (w * h * 3) as usize],
        };
        let rough = gray(w, h, 180);
        let metal = gray(w, h, 20);
        let packed = pack_orm(None, &rough, &metal).unwrap();
        PbrMaterialSet {
            albedo: ChannelSlot::generated("test-model", albedo),
            normal: ChannelSlot::absent("model does not generate normal detail"),
            roughness: ChannelSlot::generated("test-model", rough),
            metallic: ChannelSlot::generated("test-model", metal),
            occlusion: ChannelSlot::absent("AO comes from the engine geometry baker"),
            packed_orm: Some(packed),
            meta: PbrMeta::new("test-model", "none", 7, 0),
        }
    }

    #[test]
    fn valid_set_passes() {
        valid_set().validate().unwrap();
    }

    #[test]
    fn map_lengths_are_checked_and_bounded() {
        let zero = gray(0, 1, 0);
        assert_eq!(zero.expected_len(), Err(MapLayoutError::ZeroDimension));
        let oversized = PbrMap {
            width: u32::MAX,
            height: u32::MAX,
            format: PixelFormat::Rgba8,
            color_space: ColorSpace::Linear,
            data: Vec::new(),
        };
        assert!(matches!(
            oversized.expected_len(),
            Err(MapLayoutError::DimensionTooLarge { .. })
        ));
    }

    #[test]
    fn pack_orm_rejects_malformed_public_maps_without_panicking() {
        let valid = gray(2, 2, 128);
        for malformed_channel in 0..3 {
            let mut occurrence = valid.clone();
            let mut roughness = valid.clone();
            let mut metallic = valid.clone();
            match malformed_channel {
                0 => occurrence.data.pop(),
                1 => roughness.data.pop(),
                _ => metallic.data.pop(),
            };
            let result = std::panic::catch_unwind(|| {
                pack_orm(Some(&occurrence), &roughness, &metallic)
            });
            assert!(result.is_ok(), "malformed map must return an error, not panic");
            assert_eq!(
                result.unwrap(),
                Err(ContractError::PackedOrmDataLengthMismatch)
            );
        }

        let mut wrong_format = valid.clone();
        wrong_format.format = PixelFormat::Rgb8;
        assert_eq!(
            pack_orm(None, &wrong_format, &valid),
            Err(ContractError::PackedOrmWrongFormat)
        );
        let huge = PbrMap {
            width: u32::MAX,
            height: 1,
            format: PixelFormat::Gray8,
            color_space: ColorSpace::Linear,
            data: Vec::new(),
        };
        assert!(matches!(
            pack_orm(None, &huge, &huge),
            Err(ContractError::PackedOrmMapLayout(
                MapLayoutError::DimensionTooLarge { .. }
            ))
        ));
    }

    #[test]
    fn all_present_channels_share_one_atlas_size() {
        let mut set = valid_set();
        set.normal = ChannelSlot::geometry_derived(
            "test",
            PbrMap {
                width: 2,
                height: 2,
                format: PixelFormat::Rgb8,
                color_space: ColorSpace::Linear,
                data: vec![128; 12],
            },
        );
        assert_eq!(
            set.validate(),
            Err(ContractError::AtlasDimensionMismatch(PbrChannel::Normal))
        );
    }

    #[test]
    fn zero_sized_packed_orm_is_rejected() {
        let mut set = valid_set();
        set.packed_orm = Some(PbrMap {
            width: 0,
            height: 4,
            format: PixelFormat::Rgb8,
            color_space: ColorSpace::Linear,
            data: Vec::new(),
        });
        assert_eq!(
            set.validate(),
            Err(ContractError::PackedOrmMapLayout(
                MapLayoutError::ZeroDimension
            ))
        );
    }

    #[test]
    fn absent_occlusion_packs_neutral_r() {
        let set = valid_set();
        let orm = set.packed_orm.as_ref().unwrap();
        assert!(orm.data.chunks_exact(3).all(|px| px[0] == NEUTRAL_OCCLUSION));
    }

    #[test]
    fn non_neutral_r_with_absent_occlusion_rejected() {
        let mut set = valid_set();
        set.packed_orm.as_mut().unwrap().data[0] = 128;
        assert_eq!(set.validate(), Err(ContractError::PackedOrmNeutralRViolation));
    }

    #[test]
    fn present_occlusion_must_match_r() {
        let mut set = valid_set();
        let occ = gray(4, 4, 90);
        set.packed_orm = Some(pack_orm(Some(&occ), set.roughness.map.as_ref().unwrap(), set.metallic.map.as_ref().unwrap()).unwrap());
        set.occlusion = ChannelSlot::geometry_derived("engine-ao-bake", occ);
        set.validate().unwrap();
        // Now corrupt one R byte: must be caught as occlusion mismatch.
        set.packed_orm.as_mut().unwrap().data[3] ^= 0xff;
        assert_eq!(set.validate(), Err(ContractError::PackedOrmOcclusionMismatch));
    }

    #[test]
    fn albedo_must_be_srgb() {
        let mut set = valid_set();
        set.albedo.map.as_mut().unwrap().color_space = ColorSpace::Linear;
        assert_eq!(set.validate(), Err(ContractError::WrongColorSpace(PbrChannel::Albedo)));
    }

    #[test]
    fn absent_with_map_rejected() {
        let mut set = valid_set();
        set.occlusion.map = Some(gray(4, 4, 255));
        assert_eq!(set.validate(), Err(ContractError::AbsentWithMap(PbrChannel::Occlusion)));
    }

    #[test]
    fn present_without_map_rejected() {
        let mut set = valid_set();
        set.roughness.map = None;
        assert_eq!(
            set.validate(),
            Err(ContractError::PresentWithoutMap(PbrChannel::Roughness))
        );
    }

    #[test]
    fn packed_roughness_mismatch_rejected() {
        let mut set = valid_set();
        set.packed_orm.as_mut().unwrap().data[1] ^= 0xff;
        assert_eq!(set.validate(), Err(ContractError::PackedOrmRoughnessMismatch));
    }

    #[test]
    fn data_length_checked() {
        let mut set = valid_set();
        set.metallic.map.as_mut().unwrap().data.pop();
        assert_eq!(
            set.validate(),
            Err(ContractError::DataLengthMismatch(PbrChannel::Metallic))
        );
    }

    #[test]
    fn provenance_extra_rendered_and_escaped() {
        let mut set = valid_set();
        assert!(!set.manifest_json().contains("\"provenance\""));
        set.meta.extra = vec![
            ("weights_revision".to_string(), "abc123".to_string()),
            ("note".to_string(), "quote \" and \\ slash".to_string()),
        ];
        let m = set.manifest_json();
        assert!(m.contains("\"provenance\": {"));
        assert!(m.contains("\"weights_revision\": \"abc123\","));
        assert!(m.contains("\"note\": \"quote \\\" and \\\\ slash\"\n"));
    }

    #[test]
    fn manifest_is_stable_and_descriptive() {
        let a = valid_set().manifest_json();
        let b = valid_set().manifest_json();
        assert_eq!(a, b);
        assert!(a.contains("\"pbr-material-set-v1\""));
        assert!(a.contains("\"albedo\""));
        assert!(a.contains("\"origin\": \"absent\""));
        assert!(a.contains("AO comes from the engine geometry baker"));
        assert!(a.contains("\"packed_orm\""));
        assert!(a.contains("\"neutral_occlusion\": 255"));
        assert!(a.contains("R=occlusion (neutral 255 when absent), G=roughness, B=metallic"));
    }
}
