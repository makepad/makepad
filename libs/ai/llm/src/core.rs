pub const GGML_FILE_MAGIC: u32 = 0x67676d6c;
pub const GGML_FILE_VERSION: u32 = 2;

pub const GGML_QNT_VERSION: u32 = 2;
pub const GGML_QNT_VERSION_FACTOR: u32 = 1000;

pub const GGML_MAX_DIMS: usize = 4;
pub const GGML_MAX_PARAMS: usize = 2048;
pub const GGML_MAX_SRC: usize = 10;
pub const GGML_MAX_N_THREADS: usize = 512;
pub const GGML_MAX_OP_PARAMS: usize = 64;
pub const GGML_MAX_NAME: usize = 64;
pub const GGML_DEFAULT_N_THREADS: usize = 4;
pub const GGML_DEFAULT_GRAPH_SIZE: usize = 2048;

#[cfg(target_pointer_width = "32")]
pub const GGML_MEM_ALIGN: usize = 4;
#[cfg(all(not(target_pointer_width = "32"), target_os = "emscripten"))]
pub const GGML_MEM_ALIGN: usize = 8;
#[cfg(all(not(target_pointer_width = "32"), not(target_os = "emscripten")))]
pub const GGML_MEM_ALIGN: usize = 16;

#[inline]
pub const fn ggml_pad(value: usize, alignment: usize) -> usize {
    ((value + alignment - 1) / alignment) * alignment
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Status {
    AllocFailed = -2,
    Failed = -1,
    Success = 0,
    Aborted = 1,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllocFailed => "alloc_failed",
            Self::Failed => "failed",
            Self::Success => "success",
            Self::Aborted => "aborted",
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ObjectType {
    Tensor = 0,
    Graph = 1,
    WorkBuffer = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum LogLevel {
    None = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
    Continue = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TensorFlag {
    Input = 1,
    Output = 2,
    Param = 4,
    Loss = 8,
    Compute = 16,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TriType {
    UpperDiag = 0,
    Upper = 1,
    LowerDiag = 2,
    Lower = 3,
}

pub const GGML_ROPE_TYPE_NORMAL: i32 = 0;
pub const GGML_ROPE_TYPE_NEOX: i32 = 2;
pub const GGML_ROPE_TYPE_MROPE: i32 = 8;
pub const GGML_ROPE_TYPE_VISION: i32 = 24;
pub const GGML_ROPE_TYPE_IMROPE: i32 = 40;
pub const GGML_MROPE_SECTIONS: usize = 4;

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PoolOp {
    Max = 0,
    Avg = 1,
}

impl PoolOp {
    pub fn from_i32(value: i32) -> Option<Self> {
        Some(match value {
            0 => Self::Max,
            1 => Self::Avg,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Max => "max",
            Self::Avg => "avg",
        }
    }
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ScaleMode {
    Nearest = 0,
    Bilinear = 1,
    Bicubic = 2,
}

impl ScaleMode {
    pub fn from_i32(value: i32) -> Option<Self> {
        Some(match value {
            0 => Self::Nearest,
            1 => Self::Bilinear,
            2 => Self::Bicubic,
            _ => return None,
        })
    }
}

pub const GGML_SCALE_FLAG_ALIGN_CORNERS: i32 = 1 << 8;
pub const GGML_SCALE_FLAG_ANTIALIAS: i32 = 1 << 9;

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SortOrder {
    Asc = 0,
    Desc = 1,
}

impl SortOrder {
    pub fn from_i32(value: i32) -> Option<Self> {
        Some(match value {
            0 => Self::Asc,
            1 => Self::Desc,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InitParams {
    pub mem_size: usize,
    pub mem_buffer: Option<Vec<u8>>,
    pub no_alloc: bool,
}
