#![allow(unused)]
use makepad_live_id::*;
use crate::value::*;
use crate::heap::*;
use crate::value::*;
use crate::trap::*;
use crate::mod_pod::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPodField{
    pub name: LiveId,
    pub default: ScriptValue,
    pub ty: ScriptPodTypeInline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPodEnum{
    pub name: LiveId,
    pub variant: ScriptPodEnumVariant
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptPodEnumVariant{
    Bare,
    Tuple{
        items: Vec<ScriptPodTypeInline>,
    },
    Named{
        fields: Vec<ScriptPodField>
    }
}

// we're going to try to follow std140 datamapping for wgsl
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct ScriptPodTypeData{
    pub name: Option<LiveId>,
    pub object: ScriptObject,
    pub default: ScriptValue,
    //pub cached_align_of2: usize,
    pub ty: ScriptPodTy
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct ScriptPodTypeInline{
    pub self_ref: ScriptPodType,
    pub data: ScriptPodTypeData
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ScriptPodVec{
    Vec2f,
    Vec3f,
    Vec4f,
    Vec2h,
    Vec3h,
    Vec4h,
    Vec2u,
    Vec3u,
    Vec4u,
    Vec2i,
    Vec3i,
    Vec4i,
    Vec2b,
    Vec3b,
    Vec4b,
}
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ScriptPodMat{
    Mat2x2f,
    Mat3x2f,
    Mat4x2f,
    Mat2x3f,
    Mat3x3f,
    Mat4x3f,
    Mat2x4f,
    Mat3x4f,
    Mat4x4f,
}

impl ScriptPodVec{
    pub fn elem_size(&self)->usize{
        match self{
            Self::Vec2h|Self::Vec3h|Self::Vec4h=>2,
            _=>4,
        }
    }
    
    pub fn elem_ty(&self)->ScriptPodTy{
        match self{
            Self::Vec2f | Self::Vec3f | Self::Vec4f=>ScriptPodTy::F32,
            Self::Vec2h | Self::Vec3h | Self::Vec4h=>ScriptPodTy::F16,
            Self::Vec2u | Self::Vec3u | Self::Vec4u=>ScriptPodTy::U32,
            Self::Vec2i | Self::Vec3i | Self::Vec4i=>ScriptPodTy::I32,
            Self::Vec2b | Self::Vec3b | Self::Vec4b=>ScriptPodTy::Bool,
        }
    }
    
    pub fn name(&self)->LiveId{
        match self{
            Self::Vec2f=>id!(vec2f),
            Self::Vec3f=>id!(vec3f),
            Self::Vec4f=>id!(vec4f),
            Self::Vec2h=>id!(vec2h),
            Self::Vec3h=>id!(vec3h),
            Self::Vec4h=>id!(vec4h),
            Self::Vec2u=>id!(vec2u),
            Self::Vec3u=>id!(vec3u),
            Self::Vec4u=>id!(vec4u),
            Self::Vec2i=>id!(vec2i),
            Self::Vec3i=>id!(vec3i),
            Self::Vec4i=>id!(vec4i),
            Self::Vec2b=>id!(vec2b),
            Self::Vec3b=>id!(vec3b),
            Self::Vec4b=>id!(vec4b),
        }
    }
    
    pub fn swizzle_type(&self, lanes:usize, builtin: &ScriptPodBuiltins)->ScriptPodType{
        match self{
            Self::Vec2f=>match lanes{2=>builtin.pod_vec2f,3=>builtin.pod_vec3f,4=>builtin.pod_vec4f,_=>builtin.pod_f32},
            Self::Vec3f=>match lanes{2=>builtin.pod_vec2f,3=>builtin.pod_vec3f,4=>builtin.pod_vec4f,_=>builtin.pod_f32},
            Self::Vec4f=>match lanes{2=>builtin.pod_vec2f,3=>builtin.pod_vec3f,4=>builtin.pod_vec4f,_=>builtin.pod_f32},
            Self::Vec2h=>match lanes{2=>builtin.pod_vec2h,3=>builtin.pod_vec3h,4=>builtin.pod_vec4h,_=>builtin.pod_f16},
            Self::Vec3h=>match lanes{2=>builtin.pod_vec2h,3=>builtin.pod_vec3h,4=>builtin.pod_vec4h,_=>builtin.pod_f16},
            Self::Vec4h=>match lanes{2=>builtin.pod_vec2h,3=>builtin.pod_vec3h,4=>builtin.pod_vec4h,_=>builtin.pod_f16},
            Self::Vec2u=>match lanes{2=>builtin.pod_vec2u,3=>builtin.pod_vec3u,4=>builtin.pod_vec4u,_=>builtin.pod_u32},
            Self::Vec3u=>match lanes{2=>builtin.pod_vec2u,3=>builtin.pod_vec3u,4=>builtin.pod_vec4u,_=>builtin.pod_u32},
            Self::Vec4u=>match lanes{2=>builtin.pod_vec2u,3=>builtin.pod_vec3u,4=>builtin.pod_vec4u,_=>builtin.pod_u32},
            Self::Vec2i=>match lanes{2=>builtin.pod_vec2i,3=>builtin.pod_vec3i,4=>builtin.pod_vec4i,_=>builtin.pod_i32},
            Self::Vec3i=>match lanes{2=>builtin.pod_vec2i,3=>builtin.pod_vec3i,4=>builtin.pod_vec4i,_=>builtin.pod_i32},
            Self::Vec4i=>match lanes{2=>builtin.pod_vec2i,3=>builtin.pod_vec3i,4=>builtin.pod_vec4i,_=>builtin.pod_i32},
            Self::Vec2b=>match lanes{2=>builtin.pod_vec2b,3=>builtin.pod_vec3b,4=>builtin.pod_vec4b,_=>builtin.pod_bool},
            Self::Vec3b=>match lanes{2=>builtin.pod_vec2b,3=>builtin.pod_vec3b,4=>builtin.pod_vec4b,_=>builtin.pod_bool},
            Self::Vec4b=>match lanes{2=>builtin.pod_vec2b,3=>builtin.pod_vec3b,4=>builtin.pod_vec4b,_=>builtin.pod_bool},
        }
    }
    
    pub fn builtin(&self, builtin: &ScriptPodBuiltins)->ScriptPodType{
        match self{
            Self::Vec2f=>builtin.pod_vec2f,
            Self::Vec3f=>builtin.pod_vec3f,
            Self::Vec4f=>builtin.pod_vec4f,
            Self::Vec2h=>builtin.pod_vec2h,
            Self::Vec3h=>builtin.pod_vec3h,
            Self::Vec4h=>builtin.pod_vec4h,
            Self::Vec2u=>builtin.pod_vec2u,
            Self::Vec3u=>builtin.pod_vec3u,
            Self::Vec4u=>builtin.pod_vec4u,
            Self::Vec2i=>builtin.pod_vec2i,
            Self::Vec3i=>builtin.pod_vec3i,
            Self::Vec4i=>builtin.pod_vec4i,
            Self::Vec2b=>builtin.pod_vec2b,
            Self::Vec3b=>builtin.pod_vec3b,
            Self::Vec4b=>builtin.pod_vec4b,
        }
    }
    
    pub fn dims(&self)->usize{
        match self{
            Self::Vec2f|Self::Vec2h|Self::Vec2u|Self::Vec2i|Self::Vec2b=>2,
            Self::Vec3f|Self::Vec3h|Self::Vec3u|Self::Vec3i|Self::Vec3b=>3,
            Self::Vec4f|Self::Vec4h|Self::Vec4u|Self::Vec4i|Self::Vec4b=>4,
        }
    }
    pub fn align_of(&self)->usize{
        match self{
            Self::Vec2f=>8,
            Self::Vec3f=>16,
            Self::Vec4f=>8,
            Self::Vec2h=>4,
            Self::Vec3h=>8,
            Self::Vec4h=>16,
            Self::Vec2u=>8,
            Self::Vec3u=>16,
            Self::Vec4u=>16,
            Self::Vec2i=>8,
            Self::Vec3i=>16,
            Self::Vec4i=>16,
            Self::Vec2b=>8,
            Self::Vec3b=>16,
            Self::Vec4b=>16,
        }
    }
    pub fn size_of(&self)->usize{
        match self{
            Self::Vec2f=>8,
            Self::Vec3f=>12,
            Self::Vec4f=>16,
            Self::Vec2h=>4,
            Self::Vec3h=>6,
            Self::Vec4h=>8,
            Self::Vec2u=>8,
            Self::Vec3u=>12,
            Self::Vec4u=>16,
            Self::Vec2i=>8,
            Self::Vec3i=>12,
            Self::Vec4i=>16,
            Self::Vec2b=>8,
            Self::Vec3b=>12,
            Self::Vec4b=>16,
        }
    }
}
impl ScriptPodMat{
    pub fn elem_size(&self)->usize{
        match self{
            _=>4,
        }
    }
    pub fn name(&self)->LiveId{
        match self{
            Self::Mat2x2f=>id!(mat2x2f),
            Self::Mat3x2f=>id!(mat3x2f),
            Self::Mat4x2f=>id!(mat4x2f),
            Self::Mat2x3f=>id!(mat2x3f),
            Self::Mat3x3f=>id!(mat3x3f),
            Self::Mat4x3f=>id!(mat4x3f),
            Self::Mat2x4f=>id!(mat2x4f),
            Self::Mat3x4f=>id!(mat3x4f),
            Self::Mat4x4f=>id!(mat4x4f),
        }
    }
    
    pub fn builtin(&self, builtin: &ScriptPodBuiltins)->ScriptPodType{
        match self{
            Self::Mat2x2f=>builtin.pod_mat2x2f,
            Self::Mat3x2f=>builtin.pod_mat3x2f,
            Self::Mat4x2f=>builtin.pod_mat4x2f,
            Self::Mat2x3f=>builtin.pod_mat2x3f,
            Self::Mat3x3f=>builtin.pod_mat3x3f,
            Self::Mat4x3f=>builtin.pod_mat4x3f,
            Self::Mat2x4f=>builtin.pod_mat2x4f,
            Self::Mat3x4f=>builtin.pod_mat3x4f,
            Self::Mat4x4f=>builtin.pod_mat4x4f,
        }
    }
    
    pub fn dim(&self)->usize{let (x,y) = self.dims(); x*y }
            
    pub fn dims(&self)->(usize,usize){
        match self{
            Self::Mat2x2f=>(2,2),
            Self::Mat3x2f=>(3,2),
            Self::Mat4x2f=>(4,2),
            Self::Mat2x3f=>(2,3),
            Self::Mat3x3f=>(3,3),
            Self::Mat4x3f=>(4,3),
            Self::Mat2x4f=>(2,4),
            Self::Mat3x4f=>(3,4),
            Self::Mat4x4f=>(4,4),
        }
    }
    
    
    pub fn align_of(&self)->usize{
        match self{
            Self::Mat2x2f=>8,
            Self::Mat3x2f=>8,
            Self::Mat4x2f=>8,
            Self::Mat2x3f=>16,
            Self::Mat3x3f=>16,
            Self::Mat4x3f=>16,
            Self::Mat2x4f=>16,
            Self::Mat3x4f=>16,
            Self::Mat4x4f=>16,
        }
    }
    pub fn size_of(&self)->usize{
        match self{
            Self::Mat2x2f=>16,
            Self::Mat3x2f=>24,
            Self::Mat4x2f=>32,
            Self::Mat2x3f=>32,
            Self::Mat3x3f=>48,
            Self::Mat4x3f=>64,
            Self::Mat2x4f=>32,
            Self::Mat3x4f=>48,
            Self::Mat4x4f=>64,
        }
    }
}
    

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum ScriptPodTy{
    #[default]
    Void,
    ArrayBuilder,
    UndefinedStruct,
    // limited to the types WGSL supports
    F32,
    F16,
    U32,
    I32,
    Bool,
    AtomicU32,
    AtomicI32,
    Vec(ScriptPodVec),
    Mat(ScriptPodMat),
    Struct{
        align_of: usize,
        size_of: usize,
        fields:Vec<ScriptPodField>
    },
    Enum{
        align_of: usize,
        size_of: usize,
        variants:Vec<ScriptPodEnum>
    },
    FixedArray{
        align_of: usize,
        size_of: usize,
        len: usize,
        ty: Box<ScriptPodTypeInline>,
    },
    VariableArray{
        align_of: usize,
        ty: Box<ScriptPodTypeInline>,
    }
}

impl ScriptPodTy{
    pub fn is_number(&self)->bool{
        match self{
            Self::F32 | Self::F16 | Self::U32 | Self::I32 => true,
            _ => false
        }
    }
    
    /// Calculates the struct layout (align_of and size_of) from a slice of fields.
    /// Returns (align_of, size_of) tuple.
    /// 
    /// The alignment is the maximum alignment of all fields.
    /// The size is calculated by walking through fields with proper alignment,
    /// then aligning the final size to the struct alignment.
    pub fn calculate_struct_layout(fields: &[ScriptPodField]) -> (usize, usize) {
        // Calculate max alignment from all field types
        let mut align_of = 0usize;
        for field in fields {
            align_of = align_of.max(field.ty.data.ty.align_of());
        }
        
        // Calculate size by walking through fields with proper alignment
        let mut offset_of = 0usize;
        for field in fields {
            let field_align = field.ty.data.ty.align_of();
            let field_size = field.ty.data.ty.size_of();
            
            // Align the offset to field alignment
            let rem = offset_of % field_align;
            if rem != 0 {
                offset_of += field_align - rem;
            }
            
            offset_of += field_size;
        }
        
        // Align final size to struct alignment
        if align_of > 0 {
            let rem = offset_of % align_of;
            if rem != 0 {
                offset_of += align_of - rem;
            }
        }
        
        (align_of, offset_of)
    }
    
    /// Creates a new Struct variant with layout calculated from fields.
    pub fn new_struct(fields: Vec<ScriptPodField>) -> Self {
        let (align_of, size_of) = Self::calculate_struct_layout(&fields);
        Self::Struct { align_of, size_of, fields }
    }
}

#[derive(Debug, Default)]
pub struct ScriptPodOffset{
    pub offset_of: usize,
    pub field_index: usize
}

impl ScriptPodTy{
    pub fn align_of(&self)->usize{
        match self{
            Self::Void | Self::ArrayBuilder | Self::UndefinedStruct => 0,
            Self::F32 => 4,
            Self::F16 => 2,
            Self::U32 => 4,
            Self::I32 => 4,
            Self::Bool => 4,
            Self::AtomicU32 => 4,
            Self::AtomicI32 => 4,
            Self::Vec(bt)=>bt.align_of(),
            Self::Mat(bt)=>bt.align_of(),
            Self::Struct{align_of,..}=>*align_of,
            Self::Enum{align_of,..}=>*align_of,
            Self::FixedArray{align_of,..}=>*align_of,
            Self::VariableArray{align_of,..}=>*align_of,
        }
    }
    
    pub fn size_of(&self)->usize{
        match self{
            Self::Void | Self::ArrayBuilder | Self::UndefinedStruct  => 0,
            Self::F32 => 4,
            Self::F16 => 2,
            Self::U32 => 4,
            Self::I32 => 4,
            Self::Bool => 4,
            Self::AtomicU32 => 4,
            Self::AtomicI32 => 4,
            Self::Vec(bt)=>bt.size_of(),
            Self::Mat(bt)=>bt.size_of(),
            Self::Struct{size_of,..}=>*size_of,
            Self::Enum{size_of,..}=>*size_of,
            Self::FixedArray{size_of, ..}=>*size_of,
            Self::VariableArray{..}=>0,
        }
    }
}

#[derive(Default, Debug)]
pub struct ScriptPodTag(u64);

impl ScriptPodTag{
    const MARK:u64 = 0x1<<60;
    const ALLOCED:u64 = 0x2<<60;
    const ARRAY_BUILDER: u64 = 0x4<<60;
    pub fn set_array_builder(&mut self, ty: ScriptPodType){
        self.0 |= Self::ARRAY_BUILDER;
        self.0 |= (ty.index as u64);
    }
    
    pub fn as_array_builder(&self)->Option<ScriptPodType>{
        if self.0 & Self::ARRAY_BUILDER != 0{
            return Some(ScriptPodType{index: (self.0&0xffff_ffff) as u32})
        }
        None
    }
    
    pub fn is_marked(&self)->bool{
        self.0 & Self::MARK != 0
    }
                
    pub fn set_mark(&mut self){
        self.0 |= Self::MARK
    }
                
    pub fn clear_mark(&mut self){
        self.0 &= !Self::MARK
    }
    
    pub fn is_alloced(&self)->bool{
        return self.0 & Self::ALLOCED != 0
    }
            
    pub fn set_alloced(&mut self){
        self.0 |= Self::ALLOCED
    }
            
    pub fn clear(&mut self){
        self.0 = 0;
    }
}

#[derive(Default)]
pub struct ScriptPodData{
    pub tag: ScriptPodTag,
    pub ty: ScriptPodType,
    pub data: Vec<u32>
}

impl ScriptPodData{
    pub fn clear(&mut self){
        self.tag.clear();
        self.data.clear();
    }
}