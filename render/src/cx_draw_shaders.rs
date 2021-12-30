use {
    std::{
        ops::{Index, IndexMut},
        collections::{
            HashMap,
            HashSet,
            BTreeSet,
        },
    },
    makepad_shader_compiler::makepad_live_compiler::{
        LiveNode,
    },
    makepad_shader_compiler::{
        DrawShaderPtr,
    },
    crate::{
        draw_vars::{
            CxDrawShader,
            DrawShaderFingerprint,
        },
        cx::Cx
    }
};

#[derive(Default)]
pub struct CxDrawShaders{
    pub shaders: Vec<CxDrawShader>,
    pub generation: u64,
    pub ptr_to_id: HashMap<DrawShaderPtr, usize>,
    pub compile_set: BTreeSet<DrawShaderPtr>,
    pub fingerprints: Vec<DrawShaderFingerprint>,
    pub error_set: HashSet<DrawShaderPtr>,
    pub error_fingerprints: Vec<Vec<LiveNode>>,
} 

impl Cx{
    pub fn flush_draw_shaders(&mut self){
        self.draw_shaders.generation += 1;
        self.shader_registry.flush_registry();
        self.draw_shaders.shaders.clear();
        self.draw_shaders.ptr_to_id.clear();
        self.draw_shaders.fingerprints.clear();
        self.draw_shaders.error_set.clear();
        self.draw_shaders.error_fingerprints.clear();
    }
}

impl Index<usize> for CxDrawShaders {
    type Output = CxDrawShader;
    fn index(&self, index: usize) -> &Self::Output {
        &self.shaders[index]
    }
}

impl IndexMut<usize> for CxDrawShaders {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.shaders[index]
    }
}
