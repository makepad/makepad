use crate::core::{TensorFlag, GGML_MAX_DIMS, GGML_MAX_NAME, GGML_MAX_OP_PARAMS, GGML_MAX_SRC};
use crate::op::{Ftype, GluOp, Op, UnaryOp};
use crate::quant::*;

pub type TensorId = usize;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TensorType {
    F32 = GGML_TYPE_F32,
    F16 = GGML_TYPE_F16,
    Q4_0 = GGML_TYPE_Q4_0,
    Q4_1 = GGML_TYPE_Q4_1,
    Q5_0 = GGML_TYPE_Q5_0,
    Q5_1 = GGML_TYPE_Q5_1,
    Q8_0 = GGML_TYPE_Q8_0,
    Q8_1 = GGML_TYPE_Q8_1,
    Q2K = GGML_TYPE_Q2_K,
    Q3K = GGML_TYPE_Q3_K,
    Q4K = GGML_TYPE_Q4_K,
    Q5K = GGML_TYPE_Q5_K,
    Q6K = GGML_TYPE_Q6_K,
    Q8K = GGML_TYPE_Q8_K,
    IQ2Xxs = GGML_TYPE_IQ2_XXS,
    IQ2Xs = GGML_TYPE_IQ2_XS,
    IQ3Xxs = GGML_TYPE_IQ3_XXS,
    IQ1S = GGML_TYPE_IQ1_S,
    IQ4Nl = GGML_TYPE_IQ4_NL,
    IQ3S = GGML_TYPE_IQ3_S,
    IQ2S = GGML_TYPE_IQ2_S,
    IQ4Xs = GGML_TYPE_IQ4_XS,
    I8 = GGML_TYPE_I8,
    I16 = GGML_TYPE_I16,
    I32 = GGML_TYPE_I32,
    I64 = GGML_TYPE_I64,
    F64 = GGML_TYPE_F64,
    IQ1M = GGML_TYPE_IQ1_M,
    BF16 = GGML_TYPE_BF16,
    TQ1_0 = GGML_TYPE_TQ1_0,
    TQ2_0 = GGML_TYPE_TQ2_0,
    MXFP4 = GGML_TYPE_MXFP4,
    NVFP4 = GGML_TYPE_NVFP4,
}

impl TensorType {
    pub fn from_ggml_type(id: u32) -> Option<Self> {
        Some(match id {
            GGML_TYPE_F32 => Self::F32,
            GGML_TYPE_F16 => Self::F16,
            GGML_TYPE_Q4_0 => Self::Q4_0,
            GGML_TYPE_Q4_1 => Self::Q4_1,
            GGML_TYPE_Q5_0 => Self::Q5_0,
            GGML_TYPE_Q5_1 => Self::Q5_1,
            GGML_TYPE_Q8_0 => Self::Q8_0,
            GGML_TYPE_Q8_1 => Self::Q8_1,
            GGML_TYPE_Q2_K => Self::Q2K,
            GGML_TYPE_Q3_K => Self::Q3K,
            GGML_TYPE_Q4_K => Self::Q4K,
            GGML_TYPE_Q5_K => Self::Q5K,
            GGML_TYPE_Q6_K => Self::Q6K,
            GGML_TYPE_Q8_K => Self::Q8K,
            GGML_TYPE_IQ2_XXS => Self::IQ2Xxs,
            GGML_TYPE_IQ2_XS => Self::IQ2Xs,
            GGML_TYPE_IQ3_XXS => Self::IQ3Xxs,
            GGML_TYPE_IQ1_S => Self::IQ1S,
            GGML_TYPE_IQ4_NL => Self::IQ4Nl,
            GGML_TYPE_IQ3_S => Self::IQ3S,
            GGML_TYPE_IQ2_S => Self::IQ2S,
            GGML_TYPE_IQ4_XS => Self::IQ4Xs,
            GGML_TYPE_I8 => Self::I8,
            GGML_TYPE_I16 => Self::I16,
            GGML_TYPE_I32 => Self::I32,
            GGML_TYPE_I64 => Self::I64,
            GGML_TYPE_F64 => Self::F64,
            GGML_TYPE_IQ1_M => Self::IQ1M,
            GGML_TYPE_BF16 => Self::BF16,
            GGML_TYPE_TQ1_0 => Self::TQ1_0,
            GGML_TYPE_TQ2_0 => Self::TQ2_0,
            GGML_TYPE_MXFP4 => Self::MXFP4,
            GGML_TYPE_NVFP4 => Self::NVFP4,
            _ => return None,
        })
    }

    pub fn ggml_type(self) -> u32 {
        self as u32
    }

    pub fn name(self) -> &'static str {
        ggml_type_name(self.ggml_type())
    }

    pub fn block_size(self) -> usize {
        block_elements(self.ggml_type())
    }

    pub fn scalar_size_bytes(self) -> Option<usize> {
        match self {
            Self::F16 | Self::BF16 | Self::I16 => Some(2),
            Self::F32 | Self::I32 => Some(4),
            Self::F64 | Self::I64 => Some(8),
            Self::I8 => Some(1),
            Self::Q4_0
            | Self::Q4_1
            | Self::Q5_0
            | Self::Q5_1
            | Self::Q8_0
            | Self::Q8_1
            | Self::Q2K
            | Self::Q3K
            | Self::Q4K
            | Self::Q5K
            | Self::Q6K
            | Self::Q8K
            | Self::IQ2Xxs
            | Self::IQ2Xs
            | Self::IQ3Xxs
            | Self::IQ1S
            | Self::IQ4Nl
            | Self::IQ3S
            | Self::IQ2S
            | Self::IQ4Xs
            | Self::IQ1M
            | Self::TQ1_0
            | Self::TQ2_0
            | Self::MXFP4
            | Self::NVFP4 => None,
        }
    }

    pub fn is_quantized(self) -> bool {
        self.scalar_size_bytes().is_none()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferUsage {
    Weights,
    Activations,
    Scratch,
    State,
    Readback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TensorLayout {
    rank: usize,
    extents: [i64; 4],
    strides_bytes: [usize; 4],
}

impl TensorLayout {
    pub fn row_major(extents: &[i64], elem_size: usize) -> Result<Self, String> {
        if extents.is_empty() {
            return Err("tensor rank must be at least 1".to_string());
        }
        if extents.len() > 4 {
            return Err(format!("tensor rank {} exceeds ggml rank 4", extents.len()));
        }

        let mut padded_extents = [1_i64; 4];
        let mut padded_strides = [0usize; 4];

        for (i, &extent) in extents.iter().enumerate() {
            if extent < 0 {
                return Err(format!("negative extent at dimension {}: {}", i, extent));
            }
            padded_extents[i] = extent;
        }

        padded_strides[0] = elem_size;
        for i in 1..4 {
            padded_strides[i] = padded_strides[i - 1]
                .checked_mul(usize::try_from(padded_extents[i - 1]).map_err(|_| {
                    format!(
                        "extent {} at dimension {} does not fit in usize",
                        padded_extents[i - 1],
                        i - 1
                    )
                })?)
                .ok_or_else(|| "overflow computing row-major strides".to_string())?;
        }

        Ok(Self {
            rank: extents.len(),
            extents: padded_extents,
            strides_bytes: padded_strides,
        })
    }

    pub fn for_ggml(ty: TensorType, extents: &[i64]) -> Result<Self, String> {
        if extents.is_empty() {
            return Err("tensor rank must be at least 1".to_string());
        }
        if extents.len() > GGML_MAX_DIMS {
            return Err(format!("tensor rank {} exceeds ggml rank 4", extents.len()));
        }

        let mut padded_extents = [1_i64; GGML_MAX_DIMS];
        for (i, &extent) in extents.iter().enumerate() {
            if extent < 0 {
                return Err(format!("negative extent at dimension {}: {}", i, extent));
            }
            padded_extents[i] = extent;
        }

        let mut strides = [0_usize; GGML_MAX_DIMS];
        strides[0] = ggml_type_size_for_type(ty);
        if extents.len() > 1 {
            strides[1] = ggml_row_size_for_type(ty, padded_extents[0])?;
            for i in 2..GGML_MAX_DIMS {
                strides[i] = strides[i - 1]
                    .checked_mul(usize::try_from(padded_extents[i - 1]).map_err(|_| {
                        format!(
                            "extent {} at dimension {} does not fit in usize",
                            padded_extents[i - 1],
                            i - 1
                        )
                    })?)
                    .ok_or_else(|| "overflow computing ggml strides".to_string())?;
            }
        }

        Ok(Self {
            rank: extents.len(),
            extents: padded_extents,
            strides_bytes: strides,
        })
    }

    pub fn from_parts(
        rank: usize,
        extents: &[i64],
        strides_bytes: &[usize],
    ) -> Result<Self, String> {
        if !(1..=4).contains(&rank) {
            return Err(format!("invalid tensor rank {}", rank));
        }
        if extents.len() != rank {
            return Err(format!("rank {} but got {} extents", rank, extents.len()));
        }
        if strides_bytes.len() != rank {
            return Err(format!(
                "rank {} but got {} strides",
                rank,
                strides_bytes.len()
            ));
        }
        let mut padded_extents = [1_i64; 4];
        let mut padded_strides = [0_usize; 4];
        for i in 0..rank {
            padded_extents[i] = extents[i];
            padded_strides[i] = strides_bytes[i];
        }
        for i in rank..4 {
            padded_strides[i] = padded_strides[i - 1]
                .checked_mul(usize::try_from(padded_extents[i - 1]).map_err(|_| {
                    format!(
                        "extent {} at dimension {} does not fit in usize",
                        padded_extents[i - 1],
                        i - 1
                    )
                })?)
                .ok_or_else(|| "overflow computing padded strides".to_string())?;
        }
        Ok(Self {
            rank,
            extents: padded_extents,
            strides_bytes: padded_strides,
        })
    }

    pub fn rank(&self) -> usize {
        self.rank
    }

    pub fn extents(&self) -> &[i64] {
        &self.extents[..self.rank]
    }

    pub fn extents4(&self) -> [i64; 4] {
        self.extents
    }

    pub fn strides_bytes(&self) -> [usize; 4] {
        self.strides_bytes
    }

    pub fn num_elements(&self) -> i64 {
        self.extents().iter().copied().product()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TensorDesc {
    pub name: Option<String>,
    pub ty: TensorType,
    pub layout: TensorLayout,
    pub usage: BufferUsage,
}

impl TensorDesc {
    pub fn new(ty: TensorType, layout: TensorLayout, usage: BufferUsage) -> Self {
        Self {
            name: None,
            ty,
            layout,
            usage,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        let mut name = name.into();
        name.truncate(GGML_MAX_NAME.saturating_sub(1));
        self.name = Some(name);
        self
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct TensorFlags(pub u32);

impl TensorFlags {
    pub const EMPTY: Self = Self(0);
    pub const INPUT: Self = Self(TensorFlag::Input as u32);
    pub const OUTPUT: Self = Self(TensorFlag::Output as u32);
    pub const PARAM: Self = Self(TensorFlag::Param as u32);
    pub const LOSS: Self = Self(TensorFlag::Loss as u32);
    pub const COMPUTE: Self = Self(TensorFlag::Compute as u32);

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

#[derive(Clone, Debug)]
pub struct Tensor {
    pub id: TensorId,
    pub desc: TensorDesc,
    pub ne: [i64; GGML_MAX_DIMS],
    pub nb: [usize; GGML_MAX_DIMS],
    pub op: Op,
    pub op_params: [i32; GGML_MAX_OP_PARAMS / std::mem::size_of::<i32>()],
    pub flags: TensorFlags,
    pub src: [Option<TensorId>; GGML_MAX_SRC],
    pub view_src: Option<TensorId>,
    pub view_offs: usize,
    pub buffer_id: Option<usize>,
    pub data_offset: Option<usize>,
    pub extra: Option<String>,
}

impl Tensor {
    pub fn from_desc(id: TensorId, desc: TensorDesc) -> Self {
        Self {
            id,
            ne: desc.layout.extents4(),
            nb: desc.layout.strides_bytes(),
            desc,
            op: Op::None,
            op_params: [0; GGML_MAX_OP_PARAMS / std::mem::size_of::<i32>()],
            flags: TensorFlags::EMPTY,
            src: [None; GGML_MAX_SRC],
            view_src: None,
            view_offs: 0,
            buffer_id: None,
            data_offset: None,
            extra: None,
        }
    }

    pub fn name(&self) -> Option<&str> {
        self.desc.name.as_deref()
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        let mut name = name.into();
        name.truncate(GGML_MAX_NAME.saturating_sub(1));
        self.desc.name = Some(name);
    }

    pub fn set_input(&mut self) {
        self.flags.insert(TensorFlags::INPUT);
    }

    pub fn set_output(&mut self) {
        self.flags.insert(TensorFlags::OUTPUT);
    }

    pub fn set_param(&mut self) {
        self.flags.insert(TensorFlags::PARAM);
    }

    pub fn set_loss(&mut self) {
        self.flags.insert(TensorFlags::LOSS);
    }

    pub fn n_dims(&self) -> usize {
        for i in (1..GGML_MAX_DIMS).rev() {
            if self.ne[i] > 1 {
                return i + 1;
            }
        }
        1
    }

    pub fn nelements(&self) -> i64 {
        self.ne.iter().copied().product()
    }

    pub fn nrows(&self) -> i64 {
        self.ne[1] * self.ne[2] * self.ne[3]
    }

    pub fn nbytes(&self) -> usize {
        if self.ne.iter().any(|&ne| ne <= 0) {
            return 0;
        }

        let blck_size = ggml_blck_size_for_type(self.desc.ty);
        let mut nbytes = if blck_size == 1 {
            ggml_type_size_for_type(self.desc.ty)
        } else {
            usize::try_from(self.ne[0]).unwrap_or(0) * self.nb[0] / blck_size
        };

        let start = if blck_size == 1 { 0 } else { 1 };
        for i in start..GGML_MAX_DIMS {
            nbytes += usize::try_from(self.ne[i] - 1).unwrap_or(0) * self.nb[i];
        }

        nbytes
    }

    pub fn is_quantized(&self) -> bool {
        self.desc.ty.is_quantized()
    }

    pub fn is_scalar(&self) -> bool {
        self.ne == [1, 1, 1, 1]
    }

    pub fn is_vector(&self) -> bool {
        self.ne[1] == 1 && self.ne[2] == 1 && self.ne[3] == 1
    }

    pub fn is_matrix(&self) -> bool {
        self.ne[2] == 1 && self.ne[3] == 1
    }

    pub fn is_3d(&self) -> bool {
        self.ne[3] == 1
    }

    pub fn is_transposed(&self) -> bool {
        self.nb[0] > self.nb[1]
    }

    pub fn is_permuted(&self) -> bool {
        self.nb[0] > self.nb[1] || self.nb[1] > self.nb[2] || self.nb[2] > self.nb[3]
    }

    pub fn is_contiguous(&self) -> bool {
        self.is_contiguous_n(0)
    }

    pub fn is_contiguous_1(&self) -> bool {
        self.is_contiguous_n(1)
    }

    pub fn is_contiguous_2(&self) -> bool {
        self.is_contiguous_n(2)
    }

    pub fn is_contiguously_allocated(&self) -> bool {
        self.nbytes()
            == (self.nelements() as usize) * ggml_type_size_for_type(self.desc.ty)
                / ggml_blck_size_for_type(self.desc.ty)
    }

    pub fn is_contiguous_channels(&self) -> bool {
        self.nb[0] > self.nb[2]
            && self.nb[1] > self.nb[0]
            && self.nb[2] == ggml_type_size_for_type(self.desc.ty)
    }

    pub fn is_contiguous_rows(&self) -> bool {
        self.ne[0] == ggml_blck_size_for_type(self.desc.ty) as i64
            || self.nb[0] == ggml_type_size_for_type(self.desc.ty)
    }

    pub fn is_empty(&self) -> bool {
        self.ne.iter().any(|&ne| ne == 0)
    }

    pub fn is_view(&self) -> bool {
        self.view_src.is_some()
    }

    pub fn are_same_shape(&self, other: &Self) -> bool {
        self.ne == other.ne
    }

    pub fn are_same_stride(&self, other: &Self) -> bool {
        self.nb == other.nb
    }

    pub fn can_repeat(&self, other: &Self) -> bool {
        self.is_empty()
            .then_some(other.is_empty())
            .unwrap_or_else(|| {
                other.ne[0] % self.ne[0] == 0
                    && other.ne[1] % self.ne[1] == 0
                    && other.ne[2] % self.ne[2] == 0
                    && other.ne[3] % self.ne[3] == 0
            })
    }

    pub fn op_desc(&self) -> &'static str {
        match self.op {
            Op::Unary => self
                .unary_op()
                .map(UnaryOp::name)
                .unwrap_or_else(|| self.op.name()),
            Op::Glu => self
                .glu_op()
                .map(GluOp::name)
                .unwrap_or_else(|| self.op.name()),
            op => op.name(),
        }
    }

    pub fn set_op_param_i32(&mut self, index: usize, value: i32) {
        self.op_params[index] = value;
    }

    pub fn op_param_i32(&self, index: usize) -> i32 {
        self.op_params[index]
    }

    pub fn set_op_param_f32(&mut self, index: usize, value: f32) {
        self.op_params[index] = value.to_bits() as i32;
    }

    pub fn op_param_f32(&self, index: usize) -> f32 {
        f32::from_bits(self.op_params[index] as u32)
    }

    pub fn set_unary_op(&mut self, op: UnaryOp) {
        self.set_op_param_i32(0, op as i32);
    }

    pub fn unary_op(&self) -> Option<UnaryOp> {
        UnaryOp::from_u32(self.op_param_i32(0) as u32)
    }

    pub fn set_glu_op(&mut self, op: GluOp) {
        self.set_op_param_i32(0, op as i32);
    }

    pub fn glu_op(&self) -> Option<GluOp> {
        GluOp::from_u32(self.op_param_i32(0) as u32)
    }

    fn is_contiguous_n(&self, n: usize) -> bool {
        let mut next_nb = ggml_type_size_for_type(self.desc.ty);
        if self.ne[0] != ggml_blck_size_for_type(self.desc.ty) as i64 && self.nb[0] != next_nb {
            return false;
        }
        next_nb *= usize::try_from(self.ne[0]).unwrap_or(0) / ggml_blck_size_for_type(self.desc.ty);
        for i in 1..GGML_MAX_DIMS {
            if i > n {
                if self.ne[i] != 1 && self.nb[i] != next_nb {
                    return false;
                }
                next_nb *= usize::try_from(self.ne[i]).unwrap_or(0);
            } else {
                next_nb = usize::try_from(self.ne[i]).unwrap_or(0) * self.nb[i];
            }
        }
        true
    }
}

pub fn ggml_blck_size_for_type(ty: TensorType) -> usize {
    block_elements(ty.ggml_type())
}

pub fn ggml_type_size_for_type(ty: TensorType) -> usize {
    block_size(ty.ggml_type())
}

pub fn ggml_row_size_for_type(ty: TensorType, ne: i64) -> Result<usize, String> {
    if ne < 0 {
        return Err(format!("negative row extent {}", ne));
    }
    let block_elements = ggml_blck_size_for_type(ty) as i64;
    if ne % block_elements != 0 {
        return Err(format!(
            "row extent {} is not divisible by block size {} for type {}",
            ne,
            block_elements,
            ty.name()
        ));
    }
    Ok(ggml_type_size_for_type(ty) * usize::try_from(ne / block_elements).unwrap_or(0))
}

pub fn ggml_ftype_to_tensor_type(ftype: Ftype) -> Option<TensorType> {
    Some(match ftype {
        Ftype::AllF32 => TensorType::F32,
        Ftype::MostlyF16 => TensorType::F16,
        Ftype::MostlyBf16 => TensorType::BF16,
        Ftype::MostlyQ4_0 => TensorType::Q4_0,
        Ftype::MostlyQ4_1 => TensorType::Q4_1,
        Ftype::MostlyQ5_0 => TensorType::Q5_0,
        Ftype::MostlyQ5_1 => TensorType::Q5_1,
        Ftype::MostlyQ8_0 => TensorType::Q8_0,
        Ftype::MostlyMxfp4 => TensorType::MXFP4,
        Ftype::MostlyNvfp4 => TensorType::NVFP4,
        Ftype::MostlyQ2K => TensorType::Q2K,
        Ftype::MostlyQ3K => TensorType::Q3K,
        Ftype::MostlyQ4K => TensorType::Q4K,
        Ftype::MostlyQ5K => TensorType::Q5K,
        Ftype::MostlyQ6K => TensorType::Q6K,
        Ftype::MostlyIq2Xxs => TensorType::IQ2Xxs,
        Ftype::MostlyIq2Xs => TensorType::IQ2Xs,
        Ftype::MostlyIq3Xxs => TensorType::IQ3Xxs,
        Ftype::MostlyIq1S => TensorType::IQ1S,
        Ftype::MostlyIq1M => TensorType::IQ1M,
        Ftype::MostlyIq4Nl => TensorType::IQ4Nl,
        Ftype::MostlyIq4Xs => TensorType::IQ4Xs,
        Ftype::MostlyIq3S => TensorType::IQ3S,
        Ftype::MostlyIq2S => TensorType::IQ2S,
        Ftype::Unknown | Ftype::MostlyQ4_1SomeF16 => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::TensorLayout;

    #[test]
    fn from_parts_extrapolates_padded_strides_like_ggml() {
        let layout = TensorLayout::from_parts(3, &[16, 8, 4], &[4, 128, 1024]).unwrap();
        assert_eq!(layout.strides_bytes(), [4, 128, 1024, 4096]);
    }
}
