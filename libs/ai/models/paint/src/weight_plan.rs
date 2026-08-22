//! Deterministic fp16 weight-upload ordering and bounded residency planning
//! for the Hunyuan3D-Paint executor, sized against the three authorized
//! reference device classes (measured free VRAM, otherwise idle): RTX 4090
//! 23,425 MiB, RTX 5090 31,984 MiB, RTX PRO 6000 96,141 MiB.
//!
//! Two kinds of numbers live here and are never mixed silently:
//! * UNet weight footprints are exact because their name/dtype/shape inventory
//!   is pinned and verified;
//! * VAE/DINO weight footprints are unverified archive-derived estimates until
//!   their exact inventories and dtypes are pinned, and activation envelopes
//!   are estimates until a native CUDA canary measures them. Neither kind of
//!   estimate is an exact fit/readiness claim; service admission keeps using
//!   the frozen torch oracle numbers (see [`crate::pipeline`]) until then.
//!
//! Upload order is archive-offset order (sequential reads of the checkpoint
//! file), not name order; the load driver reports per-tensor progress,
//! honors cancellation between tensors, and releases partially-loaded
//! groups on unwind.

use crate::safetensors::SafeTensorIndex;
use crate::test_backend::PbrError;
use crate::torch_bin::{TorchBinIndex, TorchDtype};
use crate::unet_keys;
use std::collections::BTreeSet;
use std::io::{Read, Seek};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WeightGroup {
    UnetMain,
    UnetDual,
    Vae,
    Dino,
}

impl WeightGroup {
    pub fn all() -> [WeightGroup; 4] {
        [
            WeightGroup::UnetMain,
            WeightGroup::UnetDual,
            WeightGroup::Vae,
            WeightGroup::Dino,
        ]
    }
}

/// Pinned whole-archive byte sizes. These are not exact tensor payload sizes:
/// the VAE/DINO dtype and tensor inventories are not pinned yet.
pub const VAE_ARCHIVE_BYTES: u64 = 334_707_217;
pub const DINO_ARCHIVE_BYTES: u64 = 4_546_005_432;

/// Classify a checkpoint tensor name into its residency group.
pub fn unet_group_of(name: &str) -> Option<WeightGroup> {
    if name.starts_with("unet_dual.") {
        Some(WeightGroup::UnetDual)
    } else if name.starts_with("unet.") {
        Some(WeightGroup::UnetMain)
    } else {
        None
    }
}

fn mib(bytes: u64) -> u32 {
    ((bytes + (1 << 20) - 1) >> 20) as u32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeightFootprintEstimate {
    pub mib_estimate: u32,
    /// True only when exact tensor names, dtypes, and shapes back the number.
    pub verified_exact: bool,
    pub basis: &'static str,
}

/// Estimated fp16 device footprint of a group's weights, in MiB. Only the
/// dump-pinned UNet groups currently carry `verified_exact = true`.
pub fn group_fp16_footprint_estimate(group: WeightGroup) -> WeightFootprintEstimate {
    match group {
        WeightGroup::UnetMain | WeightGroup::UnetDual => {
            let params: usize = unet_keys::expected_keys()
                .iter()
                .filter(|(name, _)| unet_group_of(name) == Some(group))
                .map(|(_, shape)| shape.iter().product::<usize>())
                .sum();
            WeightFootprintEstimate {
                mib_estimate: mib(params as u64 * 2),
                verified_exact: true,
                basis: "exact pinned UNet name/dtype/shape inventory",
            }
        }
        // A deliberately rough archive/2 estimate: likely f32 converted to
        // fp16, but ZIP metadata and any mixed dtypes make this non-exact.
        WeightGroup::Vae => WeightFootprintEstimate {
            mib_estimate: mib(VAE_ARCHIVE_BYTES / 2),
            verified_exact: false,
            basis: "unverified whole-archive/2 estimate; VAE inventory and dtypes not pinned",
        },
        WeightGroup::Dino => WeightFootprintEstimate {
            mib_estimate: mib(DINO_ARCHIVE_BYTES / 2),
            verified_exact: false,
            basis: "unverified whole-archive/2 estimate; DINO inventory and dtypes not pinned",
        },
    }
}

/// Measured-free device budgets (root-idle, from the hardware authorization).
#[derive(Clone, Copy, Debug)]
pub struct DeviceBudget {
    pub name: &'static str,
    pub free_mib: u32,
}

pub const DEV_24G_4090: DeviceBudget = DeviceBudget { name: "rtx4090-24g", free_mib: 23_425 };
pub const DEV_32G_5090: DeviceBudget = DeviceBudget { name: "rtx5090-32g", free_mib: 31_984 };
pub const DEV_96G_PRO6000: DeviceBudget = DeviceBudget { name: "rtxpro6000-96g", free_mib: 96_141 };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidencyMode {
    /// Load each phase's groups, release when the phase ends.
    Staged,
    /// Everything resident for the whole job (and across jobs).
    AllResident,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecPhase {
    DinoEncode,
    VaeEncodeCond,
    Denoise,
    VaeDecode,
}

/// Activation envelope estimates per phase, MiB — to be canary-verified on a
/// supported CUDA host before any admission decision trusts them.
pub const ACT_EST_DINO_MIB: u32 = 1_024;
pub const ACT_EST_VAE_ENC_MIB: u32 = 1_024;
pub const ACT_EST_DENOISE_MIB: u32 = 6_144;
pub const ACT_EST_VAE_DEC_MIB: u32 = 1_536;

#[derive(Clone, Debug)]
pub struct PhasePlan {
    pub phase: ExecPhase,
    pub load: Vec<WeightGroup>,
    pub resident: Vec<WeightGroup>,
    pub release_after: Vec<WeightGroup>,
    pub weights_mib_est: u32,
    /// False if any resident group is backed only by an archive-derived
    /// estimate rather than a pinned name/dtype/shape inventory.
    pub weight_footprints_verified_exact: bool,
    pub unverified_weight_groups: Vec<WeightGroup>,
    pub activation_mib_est: u32,
    pub peak_mib_est: u32,
    /// True while either weights or activations contain unverified estimates.
    pub estimated: bool,
}

fn phase_plan(
    phase: ExecPhase,
    load: Vec<WeightGroup>,
    resident: Vec<WeightGroup>,
    release_after: Vec<WeightGroup>,
    activation_mib_est: u32,
) -> PhasePlan {
    let footprints: Vec<_> = resident
        .iter()
        .map(|group| (*group, group_fp16_footprint_estimate(*group)))
        .collect();
    let weights_mib_est = footprints
        .iter()
        .try_fold(0u32, |sum, (_, footprint)| {
            sum.checked_add(footprint.mib_estimate)
        })
        .unwrap_or(u32::MAX);
    let unverified_weight_groups: Vec<_> = footprints
        .iter()
        .filter_map(|(group, footprint)| (!footprint.verified_exact).then_some(*group))
        .collect();
    let weight_footprints_verified_exact = unverified_weight_groups.is_empty();
    PhasePlan {
        phase,
        load,
        resident,
        release_after,
        weights_mib_est,
        weight_footprints_verified_exact,
        unverified_weight_groups,
        activation_mib_est,
        peak_mib_est: weights_mib_est
            .checked_add(activation_mib_est)
            .unwrap_or(u32::MAX),
        estimated: true,
    }
}

/// The bounded residency schedule for one job.
pub fn residency_plan(mode: ResidencyMode) -> Vec<PhasePlan> {
    use ExecPhase::*;
    use WeightGroup::*;
    match mode {
        ResidencyMode::Staged => vec![
            phase_plan(DinoEncode, vec![Dino], vec![Dino], vec![Dino], ACT_EST_DINO_MIB),
            // The roughly 160 MiB archive-derived VAE estimate is small
            // enough that the planning sketch keeps it across encode/decode;
            // this is not an exact footprint claim.
            phase_plan(VaeEncodeCond, vec![Vae], vec![Vae], vec![], ACT_EST_VAE_ENC_MIB),
            phase_plan(
                Denoise,
                vec![UnetMain, UnetDual],
                vec![UnetMain, UnetDual, Vae],
                vec![UnetMain, UnetDual],
                ACT_EST_DENOISE_MIB,
            ),
            phase_plan(VaeDecode, vec![], vec![Vae], vec![], ACT_EST_VAE_DEC_MIB),
        ],
        ResidencyMode::AllResident => {
            let all = vec![Dino, Vae, UnetMain, UnetDual];
            vec![
                phase_plan(DinoEncode, all.clone(), all.clone(), vec![], ACT_EST_DINO_MIB),
                phase_plan(VaeEncodeCond, vec![], all.clone(), vec![], ACT_EST_VAE_ENC_MIB),
                phase_plan(Denoise, vec![], all.clone(), vec![], ACT_EST_DENOISE_MIB),
                phase_plan(VaeDecode, vec![], all, vec![], ACT_EST_VAE_DEC_MIB),
            ]
        }
    }
}

pub fn plan_peak_mib_estimate(plan: &[PhasePlan]) -> u32 {
    plan.iter().map(|p| p.peak_mib_est).max().unwrap_or(0)
}

/// Whether the numerical estimate fits the measured-free budget. The name is
/// intentionally explicit: this is a planning sketch, never an exact
/// fit/readiness or service-admission decision.
pub fn estimated_fits(budget: DeviceBudget, plan: &[PhasePlan]) -> bool {
    plan_peak_mib_estimate(plan) <= budget.free_mib
}

// ---------------------------------------------------------------------------
// Upload ordering + load driver
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct OrderedUpload {
    pub name: String,
    pub group: WeightGroup,
    pub dtype: TorchDtype,
    pub shape: Vec<usize>,
    pub byte_len: usize,
    pub archive_offset: usize,
}

/// Sequential-read upload order for the UNet checkpoint: all tensors of the
/// requested groups, sorted by archive byte offset (name as tiebreaker).
pub fn upload_order(
    index: &TorchBinIndex,
    groups: &[WeightGroup],
) -> Result<Vec<OrderedUpload>, PbrError> {
    let mut items = Vec::new();
    let mut names = BTreeSet::new();
    for record in &index.tensors {
        let Some(group) = unet_group_of(&record.name) else {
            continue;
        };
        if !groups.contains(&group) {
            continue;
        }
        if !names.insert(record.name.clone()) {
            return Err(PbrError::Internal(format!(
                "duplicate tensor {} in upload inventory",
                record.name
            )));
        }
        let byte_len = record
            .numel
            .checked_mul(record.dtype.elem_size())
            .ok_or_else(|| PbrError::Internal(format!("{} byte length overflow", record.name)))?;
        let archive_offset = index
            .archive_offset(record)
            .map_err(|error| PbrError::Internal(error.to_string()))?;
        items.push(OrderedUpload {
            name: record.name.clone(),
            group,
            dtype: record.dtype,
            shape: record.shape.clone(),
            byte_len,
            archive_offset,
        });
    }
    items.sort_by(|a, b| {
        a.archive_offset
            .cmp(&b.archive_offset)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(items)
}

/// Archive-order plan for a single torch checkpoint group. UNet archives are
/// prefix-partitioned; the standalone VAE archive belongs wholly to `Vae`.
pub fn torch_upload_order(
    index: &TorchBinIndex,
    group: WeightGroup,
) -> Result<Vec<OrderedUpload>, PbrError> {
    let mut items = Vec::new();
    let mut names = BTreeSet::new();
    for record in &index.tensors {
        let selected = match group {
            WeightGroup::UnetMain | WeightGroup::UnetDual => {
                unet_group_of(&record.name) == Some(group)
            }
            WeightGroup::Vae => true,
            WeightGroup::Dino => false,
        };
        if !selected {
            continue;
        }
        if !names.insert(record.name.clone()) {
            return Err(PbrError::Internal(format!(
                "duplicate tensor {} in upload inventory",
                record.name
            )));
        }
        let byte_len = record
            .numel
            .checked_mul(record.dtype.elem_size())
            .ok_or_else(|| PbrError::Internal(format!("{} byte length overflow", record.name)))?;
        let archive_offset = index
            .archive_offset(record)
            .map_err(|error| PbrError::Internal(error.to_string()))?;
        items.push(OrderedUpload {
            name: record.name.clone(),
            group,
            dtype: record.dtype,
            shape: record.shape.clone(),
            byte_len,
            archive_offset,
        });
    }
    items.sort_by(|a, b| {
        a.archive_offset
            .cmp(&b.archive_offset)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(items)
}

/// Archive-order plan for the standalone DINO safetensors checkpoint.
pub fn safetensor_upload_order(
    index: &SafeTensorIndex,
) -> Result<Vec<OrderedUpload>, PbrError> {
    let mut items = Vec::new();
    let mut names = BTreeSet::new();
    for record in &index.tensors {
        if !names.insert(record.name.clone()) {
            return Err(PbrError::Internal(format!(
                "duplicate tensor {} in upload inventory",
                record.name
            )));
        }
        let byte_len = record
            .data_end
            .checked_sub(record.data_start)
            .ok_or_else(|| PbrError::Internal(format!("{} has reversed data range", record.name)))?;
        let archive_offset = index
            .archive_offset(record)
            .map_err(|error| PbrError::Internal(error.to_string()))?;
        items.push(OrderedUpload {
            name: record.name.clone(),
            group: WeightGroup::Dino,
            dtype: record.dtype,
            shape: record.shape.clone(),
            byte_len,
            archive_offset,
        });
    }
    items.sort_by(|a, b| {
        a.archive_offset
            .cmp(&b.archive_offset)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(items)
}

fn validate_unet_inventory(index: &TorchBinIndex) -> Result<(), PbrError> {
    let inventory: Vec<(String, Vec<usize>)> = index
        .tensors
        .iter()
        .map(|record| (record.name.clone(), record.shape.clone()))
        .collect();
    let report = unet_keys::verify_inventory(&inventory);
    if !report.clean() || report.matched != unet_keys::expected_keys().len() {
        return Err(PbrError::Unavailable(format!(
            "UNet checkpoint inventory is not exact: matched {}, missing {}, mismatched {}, duplicate {}, processor extras {}, dual extras {}, unexpected {}",
            report.matched,
            report.missing.len(),
            report.shape_mismatch.len(),
            report.duplicates.len(),
            report.processor_extras.len(),
            report.dual_extras.len(),
            report.unexpected.len()
        )));
    }
    if let Some(record) = index
        .tensors
        .iter()
        .find(|record| record.dtype != TorchDtype::F16)
    {
        return Err(PbrError::Unavailable(format!(
            "UNet tensor {} has {:?}, pinned inventory requires F16",
            record.name, record.dtype
        )));
    }
    for record in &index.tensors {
        let computed_numel = record.shape.iter().try_fold(1usize, |product, dimension| {
            product.checked_mul(*dimension).ok_or_else(|| {
                PbrError::Internal(format!("{} shape product overflow", record.name))
            })
        })?;
        if computed_numel != record.numel || record.shape.len() != record.stride.len() {
            return Err(PbrError::Internal(format!(
                "{} has inconsistent shape/stride/numel metadata",
                record.name
            )));
        }
        index
            .archive_offset(record)
            .map_err(|error| PbrError::Internal(error.to_string()))?;
    }
    Ok(())
}

/// The device-upload seam the CUDA executor implements; tests use a mock.
pub trait UploadSink {
    fn upload(&mut self, item: &OrderedUpload, data: &[u8]) -> Result<(), String>;
    fn release_group(&mut self, group: WeightGroup);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    NotLoaded,
    Loading {
        group: WeightGroup,
        done_bytes: u64,
        total_bytes: u64,
    },
    Resident(Vec<WeightGroup>),
}

pub struct LoadDriver {
    pub state: LoadState,
    resident: Vec<WeightGroup>,
}

impl Default for LoadDriver {
    fn default() -> Self {
        Self {
            state: LoadState::NotLoaded,
            resident: Vec::new(),
        }
    }
}

impl LoadDriver {
    pub fn resident_groups(&self) -> &[WeightGroup] {
        &self.resident
    }

    /// Load one group from the checkpoint archive through the sink.
    /// `progress(done_bytes, total_bytes)` returns false to cancel; on
    /// cancellation the partially-uploaded group is released and the driver
    /// returns to its prior resident set.
    pub fn load_group(
        &mut self,
        archive: &[u8],
        index: &TorchBinIndex,
        group: WeightGroup,
        sink: &mut dyn UploadSink,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<(), PbrError> {
        if !matches!(group, WeightGroup::UnetMain | WeightGroup::UnetDual) {
            return Err(PbrError::Unavailable(format!(
                "{group:?} has no pinned exact inventory in this experimental slice"
            )));
        }
        validate_unet_inventory(index)?;
        let items = upload_order(index, &[group])?;
        let expected_count = unet_keys::expected_keys()
            .iter()
            .filter(|(name, _)| unet_group_of(name) == Some(group))
            .count();
        self.load_ordered(group, items, expected_count, sink, progress, |item, sink| {
            let record = index
                .find(&item.name)
                .ok_or_else(|| format!("index lost tensor {}", item.name))?;
            let data = index.tensor_bytes(archive, record).map_err(|error| error.to_string())?;
            sink.upload(item, data)
        })
    }

    /// Streaming load path for the dump-pinned UNet checkpoint. VAE admission
    /// fails closed until its exact name/dtype/shape inventory is pinned.
    pub fn load_torch_group_from<R: Read + Seek>(
        &mut self,
        archive: &mut R,
        index: &TorchBinIndex,
        group: WeightGroup,
        sink: &mut dyn UploadSink,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<(), PbrError> {
        if !matches!(group, WeightGroup::UnetMain | WeightGroup::UnetDual) {
            return Err(PbrError::Unavailable(format!(
                "{group:?} streaming load is disabled until an exact pinned inventory is present"
            )));
        }
        validate_unet_inventory(index)?;
        let items = torch_upload_order(index, group)?;
        let expected_count = unet_keys::expected_keys()
            .iter()
            .filter(|(name, _)| unet_group_of(name) == Some(group))
            .count();
        let mut buffer = Vec::new();
        self.load_ordered(group, items, expected_count, sink, progress, |item, sink| {
            let record = index
                .find(&item.name)
                .ok_or_else(|| format!("index lost tensor {}", item.name))?;
            index
                .read_tensor_into(archive, record, &mut buffer)
                .map_err(|error| error.to_string())?;
            sink.upload(item, &buffer)
        })
    }

    /// DINO loading remains fail-closed until its exact name/dtype/shape
    /// inventory is pinned in source. Parsing and deterministic planning are
    /// available independently, but cannot mark the group resident.
    pub fn load_dino_group_from<R: Read + Seek>(
        &mut self,
        archive: &mut R,
        index: &SafeTensorIndex,
        sink: &mut dyn UploadSink,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<(), PbrError> {
        let _ = (archive, index, sink, progress);
        Err(PbrError::Unavailable(
            "DINO streaming load is disabled until an exact pinned inventory is present"
                .to_string(),
        ))
    }

    pub fn release_group(&mut self, group: WeightGroup, sink: &mut dyn UploadSink) {
        if let Some(at) = self.resident.iter().position(|g| *g == group) {
            self.resident.remove(at);
            sink.release_group(group);
        }
        self.state = if self.resident.is_empty() {
            LoadState::NotLoaded
        } else {
            LoadState::Resident(self.resident.clone())
        };
    }

    pub fn unload_all(&mut self, sink: &mut dyn UploadSink) {
        for group in std::mem::take(&mut self.resident) {
            sink.release_group(group);
        }
        self.state = LoadState::NotLoaded;
    }

    fn load_ordered<F>(
        &mut self,
        group: WeightGroup,
        items: Vec<OrderedUpload>,
        expected_count: usize,
        sink: &mut dyn UploadSink,
        progress: &mut dyn FnMut(u64, u64) -> bool,
        mut upload_one: F,
    ) -> Result<(), PbrError>
    where
        F: FnMut(&OrderedUpload, &mut dyn UploadSink) -> Result<(), String>,
    {
        if self.resident.contains(&group) {
            return Ok(());
        }
        if items.is_empty() || items.len() != expected_count {
            return Err(PbrError::Internal(format!(
                "incomplete tensor inventory for {group:?}: planned {}, required {expected_count}",
                items.len()
            )));
        }
        let mut names = BTreeSet::new();
        for item in &items {
            if item.group != group || item.byte_len == 0 || !names.insert(item.name.as_str()) {
                return Err(PbrError::Internal(format!(
                    "invalid or duplicate upload item {} for {group:?}",
                    item.name
                )));
            }
        }
        let total_bytes = items.iter().try_fold(0u64, |total, item| {
            let item_bytes = u64::try_from(item.byte_len)
                .map_err(|_| PbrError::Internal("weight byte length exceeds u64".to_string()))?;
            total
                .checked_add(item_bytes)
                .ok_or_else(|| PbrError::Internal("weight byte total overflow".to_string()))
        })?;
        let mut done_bytes = 0u64;
        for item in &items {
            if !progress(done_bytes, total_bytes) {
                self.rollback_loading(group, sink);
                return Err(PbrError::Cancelled);
            }
            self.state = LoadState::Loading {
                group,
                done_bytes,
                total_bytes,
            };
            if let Err(error) = upload_one(item, sink) {
                self.rollback_loading(group, sink);
                return Err(PbrError::Internal(error));
            }
            let item_bytes = u64::try_from(item.byte_len)
                .map_err(|_| PbrError::Internal("weight byte length exceeds u64".to_string()))?;
            done_bytes = done_bytes
                .checked_add(item_bytes)
                .ok_or_else(|| PbrError::Internal("weight progress overflow".to_string()))?;
        }
        if !progress(done_bytes, total_bytes) {
            self.rollback_loading(group, sink);
            return Err(PbrError::Cancelled);
        }
        if done_bytes != total_bytes {
            self.rollback_loading(group, sink);
            return Err(PbrError::Internal(format!(
                "incomplete upload for {group:?}: {done_bytes}/{total_bytes} bytes"
            )));
        }
        self.resident.push(group);
        self.state = LoadState::Resident(self.resident.clone());
        Ok(())
    }

    fn rollback_loading(&mut self, group: WeightGroup, sink: &mut dyn UploadSink) {
        sink.release_group(group);
        self.state = if self.resident.is_empty() {
            LoadState::NotLoaded
        } else {
            LoadState::Resident(self.resident.clone())
        };
    }

    #[cfg(test)]
    fn load_structural_group_for_test(
        &mut self,
        archive: &[u8],
        index: &TorchBinIndex,
        group: WeightGroup,
        sink: &mut dyn UploadSink,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<(), PbrError> {
        let items = upload_order(index, &[group])?;
        let expected_count = items.len();
        self.load_ordered(group, items, expected_count, sink, progress, |item, sink| {
            let record = index
                .find(&item.name)
                .ok_or_else(|| format!("index lost tensor {}", item.name))?;
            let data = index
                .tensor_bytes(archive, record)
                .map_err(|error| error.to_string())?;
            sink.upload(item, data)
        })
    }

    #[cfg(test)]
    fn load_structural_torch_from_for_test<R: Read + Seek>(
        &mut self,
        archive: &mut R,
        index: &TorchBinIndex,
        group: WeightGroup,
        sink: &mut dyn UploadSink,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<(), PbrError> {
        let items = torch_upload_order(index, group)?;
        let expected_count = items.len();
        let mut buffer = Vec::new();
        self.load_ordered(group, items, expected_count, sink, progress, |item, sink| {
            let record = index
                .find(&item.name)
                .ok_or_else(|| format!("index lost tensor {}", item.name))?;
            index
                .read_tensor_into(archive, record, &mut buffer)
                .map_err(|error| error.to_string())?;
            sink.upload(item, &buffer)
        })
    }

    #[cfg(test)]
    fn load_structural_dino_from_for_test<R: Read + Seek>(
        &mut self,
        archive: &mut R,
        index: &SafeTensorIndex,
        sink: &mut dyn UploadSink,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<(), PbrError> {
        let items = safetensor_upload_order(index)?;
        let expected_count = items.len();
        let mut buffer = Vec::new();
        self.load_ordered(
            WeightGroup::Dino,
            items,
            expected_count,
            sink,
            progress,
            |item, sink| {
                let record = index
                    .find(&item.name)
                    .ok_or_else(|| format!("index lost tensor {}", item.name))?;
                index
                    .read_tensor_into(archive, record, &mut buffer)
                    .map_err(|error| error.to_string())?;
                sink.upload(item, &buffer)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safetensors::read_index_from as read_safe_index_from;
    use crate::torch_bin::{
        read_index, read_index_from as read_torch_index_from, test_fixture::FixtureWriter,
    };

    #[test]
    fn group_footprints_distinguish_exact_inventory_from_archive_estimates() {
        let main = group_fp16_footprint_estimate(WeightGroup::UnetMain);
        let dual = group_fp16_footprint_estimate(WeightGroup::UnetDual);
        let vae = group_fp16_footprint_estimate(WeightGroup::Vae);
        let dino = group_fp16_footprint_estimate(WeightGroup::Dino);
        assert!(main.verified_exact && dual.verified_exact);
        assert!(!vae.verified_exact && !dino.verified_exact);
        assert!(main.basis.contains("exact pinned"));
        assert!(vae.basis.contains("unverified"));
        assert!(dino.basis.contains("unverified"));
        assert!((2_000..2_300).contains(&main.mib_estimate), "main {main:?}");
        assert!((1_600..1_750).contains(&dual.mib_estimate), "dual {dual:?}");
        assert!((155..165).contains(&vae.mib_estimate), "vae {vae:?}");
        assert!((2_150..2_180).contains(&dino.mib_estimate), "dino {dino:?}");
        let all: u32 = WeightGroup::all()
            .iter()
            .map(|group| group_fp16_footprint_estimate(*group).mib_estimate)
            .sum();
        assert!(all < 6_500, "all-resident weights {all} MiB");
    }

    #[test]
    fn plans_fit_their_devices() {
        let staged = residency_plan(ResidencyMode::Staged);
        let resident = residency_plan(ResidencyMode::AllResident);
        // Numerical sketches fit, but VAE/DINO weight values and every
        // activation envelope remain explicitly unverified estimates.
        for budget in [DEV_24G_4090, DEV_32G_5090, DEV_96G_PRO6000] {
            assert!(
                estimated_fits(budget, &staged),
                "staged estimate must fit {}",
                budget.name
            );
            assert!(
                estimated_fits(budget, &resident),
                "all-resident estimate must fit {} (admission still gates on the torch oracle until canary)",
                budget.name
            );
        }
        // Staged peak is the denoise phase; both UNets resident there.
        let denoise = staged.iter().find(|p| p.phase == ExecPhase::Denoise).unwrap();
        assert!(denoise.resident.contains(&WeightGroup::UnetMain));
        assert!(denoise.resident.contains(&WeightGroup::UnetDual));
        assert_eq!(plan_peak_mib_estimate(&staged), denoise.peak_mib_est);
        // Staged releases DINO before denoise and keeps the VAE for decode.
        let dino_phase = &staged[0];
        assert_eq!(dino_phase.release_after, vec![WeightGroup::Dino]);
        let decode = staged.iter().find(|p| p.phase == ExecPhase::VaeDecode).unwrap();
        assert_eq!(decode.resident, vec![WeightGroup::Vae]);
        assert!(staged.iter().all(|p| p.estimated));
        assert!(staged
            .iter()
            .any(|phase| !phase.weight_footprints_verified_exact));
        assert_eq!(decode.unverified_weight_groups, vec![WeightGroup::Vae]);

        let exact_unet_weights = phase_plan(
            ExecPhase::Denoise,
            vec![WeightGroup::UnetMain, WeightGroup::UnetDual],
            vec![WeightGroup::UnetMain, WeightGroup::UnetDual],
            vec![],
            ACT_EST_DENOISE_MIB,
        );
        assert!(exact_unet_weights.weight_footprints_verified_exact);
        assert!(exact_unet_weights.unverified_weight_groups.is_empty());
        assert!(
            exact_unet_weights.estimated,
            "activation estimate still prevents an exact readiness claim"
        );
    }

    fn two_group_fixture() -> (Vec<u8>, crate::torch_bin::TorchBinIndex) {
        let mut fixture = FixtureWriter::new();
        // Added in an order whose names sort OPPOSITE to archive order, so
        // the offset-order requirement is actually exercised.
        let a: Vec<u8> = (0..12u16).flat_map(|v| v.to_le_bytes()).collect();
        fixture.add_tensor("unet.z_first.weight", "HalfStorage", "0", &a, 2, &[12], &[1], 0);
        let b: Vec<u8> = (0..8u16).flat_map(|v| v.to_le_bytes()).collect();
        fixture.add_tensor("unet.a_second.weight", "HalfStorage", "1", &b, 2, &[8], &[1], 0);
        let c: Vec<u8> = (0..4u16).flat_map(|v| v.to_le_bytes()).collect();
        fixture.add_tensor("unet_dual.only.weight", "HalfStorage", "2", &c, 2, &[4], &[1], 0);
        let archive = fixture.finish(false);
        let index = read_index(&archive).unwrap();
        (archive, index)
    }

    #[test]
    fn upload_order_is_archive_offset_order() {
        let (_, index) = two_group_fixture();
        let order = upload_order(&index, &[WeightGroup::UnetMain]).unwrap();
        assert_eq!(order.len(), 2);
        assert_eq!(order[0].name, "unet.z_first.weight");
        assert_eq!(order[1].name, "unet.a_second.weight");
        assert!(order[0].archive_offset < order[1].archive_offset);
        let both =
            upload_order(&index, &[WeightGroup::UnetMain, WeightGroup::UnetDual]).unwrap();
        assert_eq!(both.len(), 3);
    }

    #[test]
    fn planners_reject_overflow_missing_storage_and_reversed_ranges() {
        let (_, mut index) = two_group_fixture();
        index.tensors[0].numel = usize::MAX;
        assert!(upload_order(&index, &[WeightGroup::UnetMain]).is_err());

        let (_, mut index) = two_group_fixture();
        index.tensors[0].storage_key = "missing".to_string();
        assert!(upload_order(&index, &[WeightGroup::UnetMain]).is_err());

        let archive = safe_archive(
            r#"{"a":{"dtype":"F16","shape":[2],"data_offsets":[0,4]}}"#,
            &[0; 4],
        );
        let mut cursor = std::io::Cursor::new(archive);
        let mut safe_index = read_safe_index_from(&mut cursor).unwrap();
        safe_index.tensors[0].data_start = 4;
        safe_index.tensors[0].data_end = 0;
        assert!(safetensor_upload_order(&safe_index).is_err());
    }

    #[derive(Default)]
    struct MockSink {
        uploads: Vec<(String, usize)>,
        releases: Vec<WeightGroup>,
        fail_on_upload: Option<usize>,
    }

    impl UploadSink for MockSink {
        fn upload(&mut self, item: &OrderedUpload, data: &[u8]) -> Result<(), String> {
            assert_eq!(item.byte_len, data.len());
            if self.fail_on_upload == Some(self.uploads.len()) {
                return Err(format!("injected upload failure for {}", item.name));
            }
            self.uploads.push((item.name.clone(), data.len()));
            Ok(())
        }
        fn release_group(&mut self, group: WeightGroup) {
            self.releases.push(group);
        }
    }

    #[test]
    fn driver_loads_reports_and_becomes_resident() {
        let (archive, index) = two_group_fixture();
        let mut driver = LoadDriver::default();
        let mut sink = MockSink::default();
        let mut ticks = Vec::new();
        driver
            .load_structural_group_for_test(
                &archive,
                &index,
                WeightGroup::UnetMain,
                &mut sink,
                &mut |d, t| {
                ticks.push((d, t));
                true
            },
            )
            .unwrap();
        assert_eq!(sink.uploads.len(), 2);
        assert_eq!(sink.uploads[0].0, "unet.z_first.weight");
        assert_eq!(driver.resident_groups(), &[WeightGroup::UnetMain]);
        assert!(matches!(driver.state, LoadState::Resident(_)));
        // Progress is monotonic and ends complete.
        assert!(ticks.windows(2).all(|w| w[0].0 <= w[1].0));
        assert_eq!(ticks.last().unwrap().0, ticks.last().unwrap().1);
        // Idempotent for an already-resident group.
        driver
            .load_structural_group_for_test(
                &archive,
                &index,
                WeightGroup::UnetMain,
                &mut sink,
                &mut |_, _| true,
            )
            .unwrap();
        assert_eq!(sink.uploads.len(), 2);
    }

    #[test]
    fn public_loaders_never_mark_incomplete_or_unpinned_groups_resident() {
        let (archive, index) = two_group_fixture();
        let mut driver = LoadDriver::default();
        let mut sink = MockSink::default();
        let mut progress_calls = 0;
        let error = driver
            .load_group(
                &archive,
                &index,
                WeightGroup::UnetMain,
                &mut sink,
                &mut |_, _| {
                    progress_calls += 1;
                    true
                },
            )
            .unwrap_err();
        assert!(matches!(error, PbrError::Unavailable(_)));
        assert_eq!(progress_calls, 0);
        assert!(driver.resident_groups().is_empty());
        assert!(sink.uploads.is_empty());

        let mut cursor = std::io::Cursor::new(archive);
        let error = driver
            .load_torch_group_from(
                &mut cursor,
                &index,
                WeightGroup::Vae,
                &mut sink,
                &mut |_, _| true,
            )
            .unwrap_err();
        assert!(matches!(error, PbrError::Unavailable(_)));
        assert!(driver.resident_groups().is_empty());
    }

    #[test]
    fn driver_cancel_releases_partial_group() {
        let (archive, index) = two_group_fixture();
        let mut driver = LoadDriver::default();
        let mut sink = MockSink::default();
        let mut calls = 0;
        let result = driver.load_structural_group_for_test(
            &archive,
            &index,
            WeightGroup::UnetMain,
            &mut sink,
            &mut |_, _| {
                calls += 1;
                calls < 2
            },
        );
        assert_eq!(result.unwrap_err(), PbrError::Cancelled);
        assert_eq!(sink.releases, vec![WeightGroup::UnetMain]);
        assert_eq!(driver.state, LoadState::NotLoaded);
        assert!(driver.resident_groups().is_empty());
    }

    #[test]
    fn driver_streams_torch_vae_in_offset_order() {
        let mut fixture = FixtureWriter::new();
        let first = vec![1u8; 12];
        let second = vec![2u8; 8];
        fixture.add_tensor("decoder.z.weight", "FloatStorage", "0", &first, 4, &[3], &[1], 0);
        fixture.add_tensor("encoder.a.bias", "FloatStorage", "1", &second, 4, &[2], &[1], 0);
        let archive = fixture.finish(false);
        let mut cursor = std::io::Cursor::new(archive);
        let index = read_torch_index_from(&mut cursor).unwrap();
        let mut driver = LoadDriver::default();
        let mut sink = MockSink::default();
        driver
            .load_structural_torch_from_for_test(
                &mut cursor,
                &index,
                WeightGroup::Vae,
                &mut sink,
                &mut |_, _| true,
            )
            .unwrap();
        assert_eq!(
            sink.uploads,
            vec![
                ("decoder.z.weight".to_string(), 12),
                ("encoder.a.bias".to_string(), 8),
            ]
        );
        assert_eq!(driver.resident_groups(), &[WeightGroup::Vae]);
    }

    fn safe_archive(header: &str, data: &[u8]) -> Vec<u8> {
        let mut header = header.as_bytes().to_vec();
        while header.len() % 8 != 0 {
            header.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(&(header.len() as u64).to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn driver_streams_dino_safetensors_in_offset_order() {
        let archive = safe_archive(
            r#"{"later":{"dtype":"F32","shape":[2],"data_offsets":[4,12]},"first":{"dtype":"F16","shape":[2],"data_offsets":[0,4]}}"#,
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        );
        let mut cursor = std::io::Cursor::new(archive);
        let index = read_safe_index_from(&mut cursor).unwrap();
        let order = safetensor_upload_order(&index).unwrap();
        assert_eq!(order[0].name, "first");
        assert_eq!(order[1].name, "later");

        let mut driver = LoadDriver::default();
        let mut sink = MockSink::default();
        driver
            .load_structural_dino_from_for_test(
                &mut cursor,
                &index,
                &mut sink,
                &mut |_, _| true,
            )
            .unwrap();
        assert_eq!(
            sink.uploads,
            vec![("first".to_string(), 4), ("later".to_string(), 8)]
        );
        assert_eq!(driver.resident_groups(), &[WeightGroup::Dino]);
    }

    #[test]
    fn upload_error_releases_partial_group_and_preserves_prior_residency() {
        let (archive, index) = two_group_fixture();
        let mut driver = LoadDriver::default();
        let mut sink = MockSink::default();
        driver
            .load_structural_group_for_test(
                &archive,
                &index,
                WeightGroup::UnetDual,
                &mut sink,
                &mut |_, _| true,
            )
            .unwrap();
        sink.fail_on_upload = Some(sink.uploads.len() + 1);
        let error = driver
            .load_structural_group_for_test(
                &archive,
                &index,
                WeightGroup::UnetMain,
                &mut sink,
                &mut |_, _| true,
            )
            .unwrap_err();
        assert!(matches!(error, PbrError::Internal(message) if message.contains("injected")));
        assert_eq!(driver.resident_groups(), &[WeightGroup::UnetDual]);
        assert_eq!(driver.state, LoadState::Resident(vec![WeightGroup::UnetDual]));
        assert_eq!(sink.releases, vec![WeightGroup::UnetMain]);
    }

    #[test]
    fn cancellation_at_completion_rolls_back() {
        let (archive, index) = two_group_fixture();
        let mut driver = LoadDriver::default();
        let mut sink = MockSink::default();
        let error = driver
            .load_structural_group_for_test(
                &archive,
                &index,
                WeightGroup::UnetDual,
                &mut sink,
                &mut |done, total| done < total,
            )
            .unwrap_err();
        assert_eq!(error, PbrError::Cancelled);
        assert_eq!(driver.state, LoadState::NotLoaded);
        assert_eq!(sink.releases, vec![WeightGroup::UnetDual]);
    }

    #[test]
    fn unload_releases_everything() {
        let (archive, index) = two_group_fixture();
        let mut driver = LoadDriver::default();
        let mut sink = MockSink::default();
        for group in [WeightGroup::UnetMain, WeightGroup::UnetDual] {
            driver
                .load_structural_group_for_test(
                    &archive,
                    &index,
                    group,
                    &mut sink,
                    &mut |_, _| true,
                )
                .unwrap();
        }
        assert_eq!(driver.resident_groups().len(), 2);
        driver.unload_all(&mut sink);
        assert_eq!(driver.state, LoadState::NotLoaded);
        assert_eq!(sink.releases, vec![WeightGroup::UnetMain, WeightGroup::UnetDual]);
    }
}
