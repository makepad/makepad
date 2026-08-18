#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Prec {
    Default = 0,
    F32 = 10,
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Ftype {
    Unknown = -1,
    AllF32 = 0,
    MostlyF16 = 1,
    MostlyQ4_0 = 2,
    MostlyQ4_1 = 3,
    MostlyQ4_1SomeF16 = 4,
    MostlyQ8_0 = 7,
    MostlyQ5_0 = 8,
    MostlyQ5_1 = 9,
    MostlyQ2K = 10,
    MostlyQ3K = 11,
    MostlyQ4K = 12,
    MostlyQ5K = 13,
    MostlyQ6K = 14,
    MostlyIq2Xxs = 15,
    MostlyIq2Xs = 16,
    MostlyIq3Xxs = 17,
    MostlyIq1S = 18,
    MostlyIq4Nl = 19,
    MostlyIq3S = 20,
    MostlyIq2S = 21,
    MostlyIq4Xs = 22,
    MostlyIq1M = 23,
    MostlyBf16 = 24,
    MostlyMxfp4 = 25,
    MostlyNvfp4 = 26,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Op {
    None = 0,
    Dup,
    Add,
    AddId,
    Add1,
    Acc,
    Sub,
    Mul,
    Div,
    Sqr,
    Sqrt,
    Log,
    Sin,
    Cos,
    Sum,
    SumRows,
    CumSum,
    Mean,
    ArgMax,
    CountEqual,
    Repeat,
    RepeatBack,
    Concat,
    SiluBack,
    Norm,
    RmsNorm,
    RmsNormBack,
    GroupNorm,
    L2Norm,
    MulMat,
    MulMatId,
    OutProd,
    Scale,
    Set,
    Cpy,
    Cont,
    Reshape,
    View,
    Permute,
    Transpose,
    GetRows,
    GetRowsBack,
    SetRows,
    Diag,
    DiagMaskInf,
    DiagMaskZero,
    SoftMax,
    SoftMaxBack,
    Rope,
    RopeBack,
    Clamp,
    ConvTranspose1d,
    Im2col,
    Im2colBack,
    Im2col3d,
    Conv2d,
    Conv3d,
    Conv2dDw,
    ConvTranspose2d,
    Pool1d,
    Pool2d,
    Pool2dBack,
    Upscale,
    Pad,
    PadReflect1d,
    Roll,
    Arange,
    TimestepEmbedding,
    Argsort,
    TopK,
    LeakyRelu,
    Tri,
    Fill,
    FlashAttnExt,
    FlashAttnBack,
    SsmConv,
    SsmScan,
    WinPart,
    WinUnpart,
    GetRelPos,
    AddRelPos,
    RwkvWkv6,
    GatedLinearAttn,
    RwkvWkv7,
    SolveTri,
    GatedDeltaNet,
    Unary,
    MapCustom1,
    MapCustom2,
    MapCustom3,
    Custom,
    CrossEntropyLoss,
    CrossEntropyLossBack,
    OptStepAdamw,
    OptStepSgd,
    Glu,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum UnaryOp {
    Abs = 0,
    Sgn,
    Neg,
    Step,
    Tanh,
    Elu,
    Relu,
    Sigmoid,
    Gelu,
    GeluQuick,
    Silu,
    Hardswish,
    Hardsigmoid,
    Exp,
    Expm1,
    SoftPlus,
    GeluErf,
    XiElu,
    Floor,
    Ceil,
    Round,
    Trunc,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum GluOp {
    Reglu = 0,
    Geglu,
    Swiglu,
    SwigluOai,
    GegluErf,
    GegluQuick,
}

const OP_NAMES: [&str; 96] = [
    "NONE",
    "DUP",
    "ADD",
    "ADD_ID",
    "ADD1",
    "ACC",
    "SUB",
    "MUL",
    "DIV",
    "SQR",
    "SQRT",
    "LOG",
    "SIN",
    "COS",
    "SUM",
    "SUM_ROWS",
    "CUMSUM",
    "MEAN",
    "ARGMAX",
    "COUNT_EQUAL",
    "REPEAT",
    "REPEAT_BACK",
    "CONCAT",
    "SILU_BACK",
    "NORM",
    "RMS_NORM",
    "RMS_NORM_BACK",
    "GROUP_NORM",
    "L2_NORM",
    "MUL_MAT",
    "MUL_MAT_ID",
    "OUT_PROD",
    "SCALE",
    "SET",
    "CPY",
    "CONT",
    "RESHAPE",
    "VIEW",
    "PERMUTE",
    "TRANSPOSE",
    "GET_ROWS",
    "GET_ROWS_BACK",
    "SET_ROWS",
    "DIAG",
    "DIAG_MASK_INF",
    "DIAG_MASK_ZERO",
    "SOFT_MAX",
    "SOFT_MAX_BACK",
    "ROPE",
    "ROPE_BACK",
    "CLAMP",
    "CONV_TRANSPOSE_1D",
    "IM2COL",
    "IM2COL_BACK",
    "IM2COL_3D",
    "CONV_2D",
    "CONV_3D",
    "CONV_2D_DW",
    "CONV_TRANSPOSE_2D",
    "POOL_1D",
    "POOL_2D",
    "POOL_2D_BACK",
    "UPSCALE",
    "PAD",
    "PAD_REFLECT_1D",
    "ROLL",
    "ARANGE",
    "TIMESTEP_EMBEDDING",
    "ARGSORT",
    "TOP_K",
    "LEAKY_RELU",
    "TRI",
    "FILL",
    "FLASH_ATTN_EXT",
    "FLASH_ATTN_BACK",
    "SSM_CONV",
    "SSM_SCAN",
    "WIN_PART",
    "WIN_UNPART",
    "GET_REL_POS",
    "ADD_REL_POS",
    "RWKV_WKV6",
    "GATED_LINEAR_ATTN",
    "RWKV_WKV7",
    "SOLVE_TRI",
    "GATED_DELTA_NET",
    "UNARY",
    "MAP_CUSTOM1",
    "MAP_CUSTOM2",
    "MAP_CUSTOM3",
    "CUSTOM",
    "CROSS_ENTROPY_LOSS",
    "CROSS_ENTROPY_LOSS_BACK",
    "OPT_STEP_ADAMW",
    "OPT_STEP_SGD",
    "GLU",
];

const OP_SYMBOLS: [&str; 96] = [
    "none",
    "x",
    "x+y",
    "x[i]+y",
    "x+y",
    "view(x,nb,offset)+=y->x",
    "x-y",
    "x*y",
    "x/y",
    "x^2",
    "√x",
    "log(x)",
    "sin(x)",
    "cos(x)",
    "Σx",
    "Σx_k",
    "cumsum(x)",
    "Σx/n",
    "argmax(x)",
    "count_equal(x)",
    "repeat(x)",
    "repeat_back(x)",
    "concat(x, y)",
    "silu_back(x)",
    "norm(x)",
    "rms_norm(x)",
    "rms_norm_back(x)",
    "group_norm(x)",
    "l2_norm(x)",
    "X*Y",
    "X[i]*Y",
    "X*Y",
    "x*v",
    "y-\\>view(x)",
    "x-\\>y",
    "cont(x)",
    "reshape(x)",
    "view(x)",
    "permute(x)",
    "transpose(x)",
    "get_rows(x)",
    "get_rows_back(x)",
    "set_rows(x)",
    "diag(x)",
    "diag_mask_inf(x)",
    "diag_mask_zero(x)",
    "soft_max(x)",
    "soft_max_back(x)",
    "rope(x)",
    "rope_back(x)",
    "clamp(x)",
    "conv_transpose_1d(x)",
    "im2col(x)",
    "im2col_back(x)",
    "im2col_3d(x)",
    "conv_2d(x)",
    "conv_3d(x)",
    "conv_2d_dw(x)",
    "conv_transpose_2d(x)",
    "pool_1d(x)",
    "pool_2d(x)",
    "pool_2d_back(x)",
    "upscale(x)",
    "pad(x)",
    "pad_reflect_1d(x)",
    "roll(x)",
    "arange(start, stop, step)",
    "timestep_embedding(timesteps, dim, max_period)",
    "argsort(x)",
    "top_k(x)",
    "leaky_relu(x)",
    "tri(x)",
    "fill(x, c)",
    "flash_attn_ext(x)",
    "flash_attn_back(x)",
    "ssm_conv(x)",
    "ssm_scan(x)",
    "win_part(x)",
    "win_unpart(x)",
    "get_rel_pos(x)",
    "add_rel_pos(x)",
    "rwkv_wkv6(k, v, r, tf, td, s)",
    "gated_linear_attn(k, v, q, gate, s)",
    "rwkv_wkv7(r, w, k, v, a, b, s)",
    "A X = B, A triangular, solve X",
    "gated_delta_net(q, k, v, g, beta, s)",
    "unary(x)",
    "map_custom(x)",
    "map_custom(x,y)",
    "map_custom(x,y,z)",
    "custom(x)",
    "cross_entropy_loss(x,y)",
    "cross_entropy_loss_back(x,y)",
    "adamw(x)",
    "sgd(x)",
    "glu(x)",
];

const UNARY_NAMES: [&str; 22] = [
    "ABS",
    "SGN",
    "NEG",
    "STEP",
    "TANH",
    "ELU",
    "RELU",
    "SIGMOID",
    "GELU",
    "GELU_QUICK",
    "SILU",
    "HARDSWISH",
    "HARDSIGMOID",
    "EXP",
    "EXPM1",
    "SOFTPLUS",
    "GELU_ERF",
    "XIELU",
    "FLOOR",
    "CEIL",
    "ROUND",
    "TRUNC",
];

const GLU_NAMES: [&str; 6] = [
    "REGLU",
    "GEGLU",
    "SWIGLU",
    "SWIGLU_OAI",
    "GEGLU_ERF",
    "GEGLU_QUICK",
];

impl Op {
    pub fn from_u32(value: u32) -> Option<Self> {
        if value < OP_NAMES.len() as u32 {
            Some(unsafe { std::mem::transmute(value) })
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        OP_NAMES[self as usize]
    }

    pub fn symbol(self) -> &'static str {
        OP_SYMBOLS[self as usize]
    }
}

impl UnaryOp {
    pub fn from_u32(value: u32) -> Option<Self> {
        if value < UNARY_NAMES.len() as u32 {
            Some(unsafe { std::mem::transmute(value) })
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        UNARY_NAMES[self as usize]
    }
}

impl GluOp {
    pub fn from_u32(value: u32) -> Option<Self> {
        if value < GLU_NAMES.len() as u32 {
            Some(unsafe { std::mem::transmute(value) })
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        GLU_NAMES[self as usize]
    }
}

pub fn ggml_op_name(op: Op) -> &'static str {
    op.name()
}

pub fn ggml_op_symbol(op: Op) -> &'static str {
    op.symbol()
}

pub fn ggml_unary_op_name(op: UnaryOp) -> &'static str {
    op.name()
}

pub fn ggml_glu_op_name(op: GluOp) -> &'static str {
    op.name()
}
