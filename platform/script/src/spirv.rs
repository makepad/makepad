//! A SPIR-V binary module builder: instruction encoding, the module's
//! logical sections, and interning of types and constants. The shader
//! backend (`shader_spirv`) drives it; nothing here knows about shaders.
//!
//! Opcode, enumerant and GLSL.std.450 numbers are those of the SPIR-V 1.3
//! unified specification and the GLSL.std.450 extended instruction set.

use std::collections::HashMap;

pub const SPIRV_MAGIC: u32 = 0x0723_0203;
/// SPIR-V 1.3 (Vulkan 1.1): view_index and StorageBuffer are core there.
pub const SPIRV_VERSION: u32 = 0x0001_0300;

pub const OP_NAME: u32 = 5;
pub const OP_EXTENSION: u32 = 10;
pub const OP_EXT_INST_IMPORT: u32 = 11;
pub const OP_EXT_INST: u32 = 12;
pub const OP_MEMORY_MODEL: u32 = 14;
pub const OP_ENTRY_POINT: u32 = 15;
pub const OP_EXECUTION_MODE: u32 = 16;
pub const OP_CAPABILITY: u32 = 17;
pub const OP_TYPE_VOID: u32 = 19;
pub const OP_TYPE_BOOL: u32 = 20;
pub const OP_TYPE_INT: u32 = 21;
pub const OP_TYPE_FLOAT: u32 = 22;
pub const OP_TYPE_VECTOR: u32 = 23;
pub const OP_TYPE_MATRIX: u32 = 24;
pub const OP_TYPE_IMAGE: u32 = 25;
pub const OP_TYPE_SAMPLER: u32 = 26;
pub const OP_TYPE_SAMPLED_IMAGE: u32 = 27;
pub const OP_TYPE_ARRAY: u32 = 28;
pub const OP_TYPE_RUNTIME_ARRAY: u32 = 29;
pub const OP_TYPE_STRUCT: u32 = 30;
pub const OP_TYPE_POINTER: u32 = 32;
pub const OP_TYPE_FUNCTION: u32 = 33;
pub const OP_CONSTANT_TRUE: u32 = 41;
pub const OP_CONSTANT_FALSE: u32 = 42;
pub const OP_CONSTANT: u32 = 43;
pub const OP_CONSTANT_COMPOSITE: u32 = 44;
pub const OP_CONSTANT_NULL: u32 = 46;
pub const OP_FUNCTION: u32 = 54;
pub const OP_FUNCTION_PARAMETER: u32 = 55;
pub const OP_FUNCTION_END: u32 = 56;
pub const OP_FUNCTION_CALL: u32 = 57;
pub const OP_VARIABLE: u32 = 59;
pub const OP_LOAD: u32 = 61;
pub const OP_STORE: u32 = 62;
pub const OP_ACCESS_CHAIN: u32 = 65;
pub const OP_ARRAY_LENGTH: u32 = 68;
pub const OP_DECORATE: u32 = 71;
pub const OP_MEMBER_DECORATE: u32 = 72;
pub const OP_VECTOR_EXTRACT_DYNAMIC: u32 = 77;
pub const OP_VECTOR_SHUFFLE: u32 = 79;
pub const OP_COMPOSITE_CONSTRUCT: u32 = 80;
pub const OP_COMPOSITE_EXTRACT: u32 = 81;
pub const OP_COPY_OBJECT: u32 = 83;
pub const OP_TRANSPOSE: u32 = 84;
pub const OP_SAMPLED_IMAGE: u32 = 86;
pub const OP_IMAGE_SAMPLE_IMPLICIT_LOD: u32 = 87;
pub const OP_IMAGE_SAMPLE_EXPLICIT_LOD: u32 = 88;
pub const OP_IMAGE_SAMPLE_DREF_IMPLICIT_LOD: u32 = 89;
pub const OP_IMAGE_SAMPLE_DREF_EXPLICIT_LOD: u32 = 90;
pub const OP_IMAGE_FETCH: u32 = 95;
pub const OP_IMAGE_GATHER: u32 = 96;
pub const OP_IMAGE_DREF_GATHER: u32 = 97;
pub const OP_IMAGE_QUERY_SIZE_LOD: u32 = 103;
pub const OP_IMAGE_QUERY_LEVELS: u32 = 106;
pub const OP_CONVERT_F_TO_U: u32 = 109;
pub const OP_CONVERT_F_TO_S: u32 = 110;
pub const OP_CONVERT_S_TO_F: u32 = 111;
pub const OP_CONVERT_U_TO_F: u32 = 112;
pub const OP_F_CONVERT: u32 = 115;
pub const OP_BITCAST: u32 = 124;
pub const OP_S_NEGATE: u32 = 126;
pub const OP_F_NEGATE: u32 = 127;
pub const OP_I_ADD: u32 = 128;
pub const OP_F_ADD: u32 = 129;
pub const OP_I_SUB: u32 = 130;
pub const OP_F_SUB: u32 = 131;
pub const OP_I_MUL: u32 = 132;
pub const OP_F_MUL: u32 = 133;
pub const OP_U_DIV: u32 = 134;
pub const OP_S_DIV: u32 = 135;
pub const OP_F_DIV: u32 = 136;
pub const OP_U_MOD: u32 = 137;
pub const OP_S_REM: u32 = 138;
pub const OP_F_REM: u32 = 140;
pub const OP_VECTOR_TIMES_SCALAR: u32 = 142;
pub const OP_MATRIX_TIMES_SCALAR: u32 = 143;
pub const OP_VECTOR_TIMES_MATRIX: u32 = 144;
pub const OP_MATRIX_TIMES_VECTOR: u32 = 145;
pub const OP_MATRIX_TIMES_MATRIX: u32 = 146;
pub const OP_OUTER_PRODUCT: u32 = 147;
pub const OP_DOT: u32 = 148;
pub const OP_ANY: u32 = 154;
pub const OP_ALL: u32 = 155;
pub const OP_IS_NAN: u32 = 156;
pub const OP_IS_INF: u32 = 157;
pub const OP_LOGICAL_EQUAL: u32 = 164;
pub const OP_LOGICAL_NOT_EQUAL: u32 = 165;
pub const OP_LOGICAL_OR: u32 = 166;
pub const OP_LOGICAL_AND: u32 = 167;
pub const OP_LOGICAL_NOT: u32 = 168;
pub const OP_SELECT: u32 = 169;
pub const OP_I_EQUAL: u32 = 170;
pub const OP_I_NOT_EQUAL: u32 = 171;
pub const OP_U_GREATER_THAN: u32 = 172;
pub const OP_S_GREATER_THAN: u32 = 173;
pub const OP_U_GREATER_THAN_EQUAL: u32 = 174;
pub const OP_S_GREATER_THAN_EQUAL: u32 = 175;
pub const OP_U_LESS_THAN: u32 = 176;
pub const OP_S_LESS_THAN: u32 = 177;
pub const OP_U_LESS_THAN_EQUAL: u32 = 178;
pub const OP_S_LESS_THAN_EQUAL: u32 = 179;
pub const OP_F_ORD_EQUAL: u32 = 180;
pub const OP_F_ORD_NOT_EQUAL: u32 = 182;
pub const OP_F_ORD_LESS_THAN: u32 = 184;
pub const OP_F_ORD_GREATER_THAN: u32 = 186;
pub const OP_F_ORD_LESS_THAN_EQUAL: u32 = 188;
pub const OP_F_ORD_GREATER_THAN_EQUAL: u32 = 190;
pub const OP_SHIFT_RIGHT_LOGICAL: u32 = 194;
pub const OP_SHIFT_RIGHT_ARITHMETIC: u32 = 195;
pub const OP_SHIFT_LEFT_LOGICAL: u32 = 196;
pub const OP_BITWISE_OR: u32 = 197;
pub const OP_BITWISE_XOR: u32 = 198;
pub const OP_BITWISE_AND: u32 = 199;
pub const OP_NOT: u32 = 200;
pub const OP_BIT_FIELD_INSERT: u32 = 201;
pub const OP_BIT_FIELD_S_EXTRACT: u32 = 202;
pub const OP_BIT_FIELD_U_EXTRACT: u32 = 203;
pub const OP_BIT_REVERSE: u32 = 204;
pub const OP_BIT_COUNT: u32 = 205;
pub const OP_DPDX: u32 = 207;
pub const OP_DPDY: u32 = 208;
pub const OP_FWIDTH: u32 = 209;
pub const OP_DPDX_FINE: u32 = 210;
pub const OP_DPDY_FINE: u32 = 211;
pub const OP_FWIDTH_FINE: u32 = 212;
pub const OP_DPDX_COARSE: u32 = 213;
pub const OP_DPDY_COARSE: u32 = 214;
pub const OP_FWIDTH_COARSE: u32 = 215;
pub const OP_LOOP_MERGE: u32 = 246;
pub const OP_SELECTION_MERGE: u32 = 247;
pub const OP_LABEL: u32 = 248;
pub const OP_BRANCH: u32 = 249;
pub const OP_BRANCH_CONDITIONAL: u32 = 250;
pub const OP_KILL: u32 = 252;
pub const OP_RETURN: u32 = 253;
pub const OP_RETURN_VALUE: u32 = 254;
pub const OP_UNREACHABLE: u32 = 255;

// GLSL.std.450 extended instructions.
pub const GL_ROUND_EVEN: u32 = 2;
pub const GL_TRUNC: u32 = 3;
pub const GL_F_ABS: u32 = 4;
pub const GL_S_ABS: u32 = 5;
pub const GL_F_SIGN: u32 = 6;
pub const GL_S_SIGN: u32 = 7;
pub const GL_FLOOR: u32 = 8;
pub const GL_CEIL: u32 = 9;
pub const GL_FRACT: u32 = 10;
pub const GL_RADIANS: u32 = 11;
pub const GL_DEGREES: u32 = 12;
pub const GL_SIN: u32 = 13;
pub const GL_COS: u32 = 14;
pub const GL_TAN: u32 = 15;
pub const GL_ASIN: u32 = 16;
pub const GL_ACOS: u32 = 17;
pub const GL_ATAN: u32 = 18;
pub const GL_SINH: u32 = 19;
pub const GL_COSH: u32 = 20;
pub const GL_TANH: u32 = 21;
pub const GL_ASINH: u32 = 22;
pub const GL_ACOSH: u32 = 23;
pub const GL_ATANH: u32 = 24;
pub const GL_ATAN2: u32 = 25;
pub const GL_POW: u32 = 26;
pub const GL_EXP: u32 = 27;
pub const GL_LOG: u32 = 28;
pub const GL_EXP2: u32 = 29;
pub const GL_LOG2: u32 = 30;
pub const GL_SQRT: u32 = 31;
pub const GL_INVERSE_SQRT: u32 = 32;
pub const GL_DETERMINANT: u32 = 33;
pub const GL_MATRIX_INVERSE: u32 = 34;
pub const GL_F_MIN: u32 = 37;
pub const GL_U_MIN: u32 = 38;
pub const GL_S_MIN: u32 = 39;
pub const GL_F_MAX: u32 = 40;
pub const GL_U_MAX: u32 = 41;
pub const GL_S_MAX: u32 = 42;
pub const GL_F_CLAMP: u32 = 43;
pub const GL_F_MIX: u32 = 46;
pub const GL_STEP: u32 = 48;
pub const GL_SMOOTH_STEP: u32 = 49;
pub const GL_FMA: u32 = 50;
pub const GL_LDEXP: u32 = 53;
pub const GL_PACK_SNORM_4X8: u32 = 54;
pub const GL_PACK_UNORM_4X8: u32 = 55;
pub const GL_PACK_SNORM_2X16: u32 = 56;
pub const GL_PACK_UNORM_2X16: u32 = 57;
pub const GL_PACK_HALF_2X16: u32 = 58;
pub const GL_UNPACK_SNORM_2X16: u32 = 60;
pub const GL_UNPACK_UNORM_2X16: u32 = 61;
pub const GL_UNPACK_HALF_2X16: u32 = 62;
pub const GL_UNPACK_SNORM_4X8: u32 = 63;
pub const GL_UNPACK_UNORM_4X8: u32 = 64;
pub const GL_LENGTH: u32 = 66;
pub const GL_DISTANCE: u32 = 67;
pub const GL_CROSS: u32 = 68;
pub const GL_NORMALIZE: u32 = 69;
pub const GL_FACE_FORWARD: u32 = 70;
pub const GL_REFLECT: u32 = 71;
pub const GL_REFRACT: u32 = 72;
pub const GL_FIND_I_LSB: u32 = 73;
pub const GL_FIND_S_MSB: u32 = 74;
pub const GL_FIND_U_MSB: u32 = 75;

// Capabilities.
pub const CAP_SHADER: u32 = 1;
pub const CAP_FLOAT16: u32 = 9;
pub const CAP_SAMPLED_CUBE_ARRAY: u32 = 45;
pub const CAP_IMAGE_QUERY: u32 = 50;
pub const CAP_DERIVATIVE_CONTROL: u32 = 51;
pub const CAP_MULTI_VIEW: u32 = 4439;

// Execution models and modes.
pub const EXEC_MODEL_VERTEX: u32 = 0;
pub const EXEC_MODEL_FRAGMENT: u32 = 4;
pub const EXEC_MODE_ORIGIN_UPPER_LEFT: u32 = 7;
pub const EXEC_MODE_DEPTH_REPLACING: u32 = 12;

// Storage classes.
pub const SC_UNIFORM_CONSTANT: u32 = 0;
pub const SC_INPUT: u32 = 1;
pub const SC_UNIFORM: u32 = 2;
pub const SC_OUTPUT: u32 = 3;
pub const SC_PRIVATE: u32 = 6;
pub const SC_FUNCTION: u32 = 7;
pub const SC_STORAGE_BUFFER: u32 = 12;

// Decorations.
pub const DECO_BLOCK: u32 = 2;
pub const DECO_COL_MAJOR: u32 = 5;
pub const DECO_ARRAY_STRIDE: u32 = 6;
pub const DECO_MATRIX_STRIDE: u32 = 7;
pub const DECO_BUILT_IN: u32 = 11;
pub const DECO_FLAT: u32 = 14;
pub const DECO_INVARIANT: u32 = 18;
pub const DECO_NON_WRITABLE: u32 = 24;
pub const DECO_LOCATION: u32 = 30;
pub const DECO_BINDING: u32 = 33;
pub const DECO_DESCRIPTOR_SET: u32 = 34;
pub const DECO_OFFSET: u32 = 35;

// Built-ins.
pub const BUILTIN_POSITION: u32 = 0;
pub const BUILTIN_POINT_SIZE: u32 = 1;
pub const BUILTIN_FRAG_COORD: u32 = 15;
pub const BUILTIN_FRONT_FACING: u32 = 17;
pub const BUILTIN_FRAG_DEPTH: u32 = 22;
pub const BUILTIN_SAMPLE_ID: u32 = 18;
pub const BUILTIN_SAMPLE_MASK: u32 = 20;
pub const BUILTIN_VERTEX_INDEX: u32 = 42;
pub const BUILTIN_INSTANCE_INDEX: u32 = 43;
pub const BUILTIN_VIEW_INDEX: u32 = 4440;

// Image dimensions and operands.
pub const DIM_2D: u32 = 1;
pub const DIM_3D: u32 = 2;
pub const DIM_CUBE: u32 = 3;
pub const IMAGE_FORMAT_UNKNOWN: u32 = 0;
pub const IMAGE_OPERAND_BIAS: u32 = 0x1;
pub const IMAGE_OPERAND_LOD: u32 = 0x2;
pub const IMAGE_OPERAND_GRAD: u32 = 0x4;
pub const IMAGE_OPERAND_CONST_OFFSET: u32 = 0x8;

/// Appends one instruction: the word count and opcode, then the operands.
pub fn spv_inst(out: &mut Vec<u32>, op: u32, operands: &[u32]) {
    out.push(((operands.len() as u32 + 1) << 16) | op);
    out.extend_from_slice(operands);
}

/// A literal string operand: UTF-8, nul-terminated, padded to a word.
pub fn spv_string(out: &mut Vec<u32>, text: &str) {
    let bytes = text.as_bytes();
    let mut word = 0u32;
    let mut shift = 0;
    for b in bytes {
        word |= (*b as u32) << shift;
        shift += 8;
        if shift == 32 {
            out.push(word);
            word = 0;
            shift = 0;
        }
    }
    // The terminating nul (and padding) is the rest of this word, or a
    // whole zero word when the text filled the last one.
    out.push(word);
}

/// The logical sections of a module, filled in any order and written out
/// in the order the specification requires.
#[derive(Default)]
pub struct SpvBuilder {
    pub bound: u32,
    capabilities: Vec<u32>,
    extensions: Vec<String>,
    pub glsl_ext: u32,
    pub entry_points: Vec<u32>,
    pub execution_modes: Vec<u32>,
    pub annotations: Vec<u32>,
    /// Types, constants and global variables, in definition order.
    pub globals: Vec<u32>,
    pub functions: Vec<u32>,
    interned: HashMap<Vec<u32>, u32>,
}

impl SpvBuilder {
    pub fn new() -> Self {
        let mut b = SpvBuilder { bound: 1, ..Default::default() };
        b.glsl_ext = b.id();
        b.capability(CAP_SHADER);
        b
    }

    pub fn id(&mut self) -> u32 {
        let id = self.bound;
        self.bound += 1;
        id
    }

    pub fn capability(&mut self, cap: u32) {
        if !self.capabilities.contains(&cap) {
            self.capabilities.push(cap);
        }
    }

    pub fn extension(&mut self, name: &str) {
        for e in &self.extensions {
            if e == name {
                return;
            }
        }
        self.extensions.push(name.to_string());
    }

    /// A type instruction (`op result operands...`), one id per distinct
    /// instruction.
    pub fn intern_type(&mut self, op: u32, operands: &[u32]) -> u32 {
        let mut key = Vec::with_capacity(operands.len() + 2);
        key.push(op);
        key.push(0);
        key.extend_from_slice(operands);
        if let Some(id) = self.interned.get(&key) {
            return *id;
        }
        let id = self.id();
        let mut words = Vec::with_capacity(operands.len() + 1);
        words.push(id);
        words.extend_from_slice(operands);
        spv_inst(&mut self.globals, op, &words);
        self.interned.insert(key, id);
        id
    }

    /// A constant instruction (`op type result operands...`), one id per
    /// distinct instruction.
    pub fn intern_const(&mut self, op: u32, ty: u32, operands: &[u32]) -> u32 {
        let mut key = Vec::with_capacity(operands.len() + 2);
        key.push(op);
        key.push(ty);
        key.extend_from_slice(operands);
        if let Some(id) = self.interned.get(&key) {
            return *id;
        }
        let id = self.id();
        let mut words = Vec::with_capacity(operands.len() + 2);
        words.push(ty);
        words.push(id);
        words.extend_from_slice(operands);
        spv_inst(&mut self.globals, op, &words);
        self.interned.insert(key, id);
        id
    }

    pub fn type_void(&mut self) -> u32 {
        self.intern_type(OP_TYPE_VOID, &[])
    }
    pub fn type_bool(&mut self) -> u32 {
        self.intern_type(OP_TYPE_BOOL, &[])
    }
    pub fn type_f32(&mut self) -> u32 {
        self.intern_type(OP_TYPE_FLOAT, &[32])
    }
    pub fn type_f16(&mut self) -> u32 {
        self.capability(CAP_FLOAT16);
        self.intern_type(OP_TYPE_FLOAT, &[16])
    }
    pub fn type_i32(&mut self) -> u32 {
        self.intern_type(OP_TYPE_INT, &[32, 1])
    }
    pub fn type_u32(&mut self) -> u32 {
        self.intern_type(OP_TYPE_INT, &[32, 0])
    }
    pub fn type_vector(&mut self, component: u32, count: u32) -> u32 {
        self.intern_type(OP_TYPE_VECTOR, &[component, count])
    }
    pub fn type_matrix(&mut self, column: u32, count: u32) -> u32 {
        self.intern_type(OP_TYPE_MATRIX, &[column, count])
    }
    pub fn type_pointer(&mut self, storage: u32, base: u32) -> u32 {
        self.intern_type(OP_TYPE_POINTER, &[storage, base])
    }
    pub fn type_function(&mut self, ret: u32, params: &[u32]) -> u32 {
        let mut ops = Vec::with_capacity(params.len() + 1);
        ops.push(ret);
        ops.extend_from_slice(params);
        self.intern_type(OP_TYPE_FUNCTION, &ops)
    }
    pub fn type_sampler(&mut self) -> u32 {
        self.intern_type(OP_TYPE_SAMPLER, &[])
    }
    pub fn type_sampled_image(&mut self, image: u32) -> u32 {
        self.intern_type(OP_TYPE_SAMPLED_IMAGE, &[image])
    }
    pub fn type_image(&mut self, sampled: u32, dim: u32, depth: bool, arrayed: bool) -> u32 {
        self.intern_type(
            OP_TYPE_IMAGE,
            &[sampled, dim, depth as u32, arrayed as u32, 0, 1, IMAGE_FORMAT_UNKNOWN],
        )
    }
    /// A sized array, decorated with its stride on first use.
    pub fn type_array(&mut self, element: u32, len: u32, stride: u32) -> u32 {
        let u32_ty = self.type_u32();
        let len_id = self.intern_const(OP_CONSTANT, u32_ty, &[len]);
        let key = vec![OP_TYPE_ARRAY, 0, element, len_id];
        if let Some(id) = self.interned.get(&key) {
            return *id;
        }
        let id = self.intern_type(OP_TYPE_ARRAY, &[element, len_id]);
        self.decorate(id, DECO_ARRAY_STRIDE, &[stride]);
        id
    }
    pub fn type_runtime_array(&mut self, element: u32, stride: u32) -> u32 {
        let key = vec![OP_TYPE_RUNTIME_ARRAY, 0, element];
        if let Some(id) = self.interned.get(&key) {
            return *id;
        }
        let id = self.intern_type(OP_TYPE_RUNTIME_ARRAY, &[element]);
        self.decorate(id, DECO_ARRAY_STRIDE, &[stride]);
        id
    }

    pub fn const_u32(&mut self, v: u32) -> u32 {
        let ty = self.type_u32();
        self.intern_const(OP_CONSTANT, ty, &[v])
    }
    pub fn const_i32(&mut self, v: i32) -> u32 {
        let ty = self.type_i32();
        self.intern_const(OP_CONSTANT, ty, &[v as u32])
    }
    pub fn const_f32(&mut self, v: f32) -> u32 {
        let ty = self.type_f32();
        self.intern_const(OP_CONSTANT, ty, &[v.to_bits()])
    }
    pub fn const_bool(&mut self, v: bool) -> u32 {
        let ty = self.type_bool();
        self.intern_const(if v { OP_CONSTANT_TRUE } else { OP_CONSTANT_FALSE }, ty, &[])
    }
    pub fn const_null(&mut self, ty: u32) -> u32 {
        self.intern_const(OP_CONSTANT_NULL, ty, &[])
    }
    pub fn const_composite(&mut self, ty: u32, parts: &[u32]) -> u32 {
        self.intern_const(OP_CONSTANT_COMPOSITE, ty, parts)
    }

    pub fn decorate(&mut self, id: u32, decoration: u32, operands: &[u32]) {
        let mut words = Vec::with_capacity(operands.len() + 2);
        words.push(id);
        words.push(decoration);
        words.extend_from_slice(operands);
        spv_inst(&mut self.annotations, OP_DECORATE, &words);
    }

    pub fn member_decorate(&mut self, id: u32, member: u32, decoration: u32, operands: &[u32]) {
        let mut words = Vec::with_capacity(operands.len() + 3);
        words.push(id);
        words.push(member);
        words.push(decoration);
        words.extend_from_slice(operands);
        spv_inst(&mut self.annotations, OP_MEMBER_DECORATE, &words);
    }

    /// A global `OpVariable` (`init` 0 for none).
    pub fn global_variable(&mut self, ptr_ty: u32, storage: u32, init: u32) -> u32 {
        let id = self.id();
        if init != 0 {
            spv_inst(&mut self.globals, OP_VARIABLE, &[ptr_ty, id, storage, init]);
        } else {
            spv_inst(&mut self.globals, OP_VARIABLE, &[ptr_ty, id, storage]);
        }
        id
    }

    pub fn entry_point(&mut self, model: u32, function: u32, name: &str, interface: &[u32]) {
        let mut words = Vec::new();
        words.push(model);
        words.push(function);
        spv_string(&mut words, name);
        words.extend_from_slice(interface);
        spv_inst(&mut self.entry_points, OP_ENTRY_POINT, &words);
    }

    pub fn execution_mode(&mut self, function: u32, mode: u32) {
        spv_inst(&mut self.execution_modes, OP_EXECUTION_MODE, &[function, mode]);
    }

    /// The finished module's words.
    pub fn finish(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(
            32 + self.annotations.len() + self.globals.len() + self.functions.len(),
        );
        out.push(SPIRV_MAGIC);
        out.push(SPIRV_VERSION);
        out.push(0); // generator: unregistered
        out.push(self.bound);
        out.push(0);
        for cap in &self.capabilities {
            spv_inst(&mut out, OP_CAPABILITY, &[*cap]);
        }
        for ext in &self.extensions {
            let mut words = Vec::new();
            spv_string(&mut words, ext);
            spv_inst(&mut out, OP_EXTENSION, &words);
        }
        let mut words = vec![self.glsl_ext];
        spv_string(&mut words, "GLSL.std.450");
        spv_inst(&mut out, OP_EXT_INST_IMPORT, &words);
        // Logical addressing, GLSL450 memory model.
        spv_inst(&mut out, OP_MEMORY_MODEL, &[0, 1]);
        out.extend_from_slice(&self.entry_points);
        out.extend_from_slice(&self.execution_modes);
        out.extend_from_slice(&self.annotations);
        out.extend_from_slice(&self.globals);
        out.extend_from_slice(&self.functions);
        out
    }
}
