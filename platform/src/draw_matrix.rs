#![allow(dead_code)]
use {
    crate::{
        makepad_math::Mat4,
        makepad_error_log::*,
        id_pool::*,
    }
};

#[derive(Debug)]
pub struct DrawMatrix(PoolId);

#[derive(Default, Clone, Debug, PartialEq, Copy, Hash, Ord, PartialOrd, Eq)]
pub struct DrawMatrixId(usize, u64);

#[derive(Default)]
pub struct CxDrawMatrix{
    parent: DrawMatrixId,
    parent_version: u64,
    
    local_version: u64,
    local: Mat4,
    
    object_to_world: Mat4,
    world_to_object: Option<Mat4>
}

impl DrawMatrixId{
    pub fn index(&self)->usize{self.0}
    pub fn generation(&self)->u64{self.1}
}

impl DrawMatrix {
    pub fn id(&self) -> DrawMatrixId {DrawMatrixId(self.0.id, self.0.generation)}
    pub fn identity()->DrawMatrixId{
        DrawMatrixId(0, 0)
    }
}

pub struct CxDrawMatrixPool{
    pool: IdPool<CxDrawMatrix>,
    identity: DrawMatrix
}

impl Default for CxDrawMatrixPool{
    fn default()->Self{
        let mut pool = IdPool::default();
        let identity = DrawMatrix(pool.alloc());
        
        return Self{
            pool,
            identity
        }
    }
}

impl CxDrawMatrixPool {
    pub fn alloc(&mut self) -> DrawMatrix {
        DrawMatrix(self.pool.alloc())
    }
    
    pub fn update_local(&mut self, index:DrawMatrixId, new_mat:&Mat4){
        let d = &mut self.pool.pool[index.0];
        if d.local != *new_mat{
            d.local_version += 1;
            d.local = *new_mat;
        }
    }
    
    pub fn update_parent(&mut self, index:DrawMatrixId){
        fn update_parent(_index:usize, _pool:&mut Vec<IdPoolItem<CxDrawMatrix>>){
            
        }
        update_parent(index.0, &mut self.pool.pool);
        let d = &mut self.pool.pool[index.0];
        if d.parent.0 != 0{ // not the identity
           // self.update_parent(d.parent);
            // ok now we compare the parent version to our parent version
           // if self.pool.pool[index.0].parent_version != 
           //    self.pool.pool[]
        }
    }
    
    /*pub fn get_object_to_world(&mut self, index:DrawMatrixId)->&Mat4{
        // ok this on demand updates the matrix stack
        
    }*/
}

impl std::ops::Index<DrawMatrixId> for CxDrawMatrixPool {
    
    type Output = CxDrawMatrix;
    fn index(&self, index: DrawMatrixId) -> &Self::Output {
        let d = &self.pool.pool[index.0];
        if d.generation != index.1 {
            error!("MatrixNode id generation wrong {} {} {}", index.0, d.generation, index.1)
        }
        &d.item
    }
}

impl std::ops::IndexMut<DrawMatrixId> for CxDrawMatrixPool {
    fn index_mut(&mut self, index: DrawMatrixId) -> &mut Self::Output {
        let d = &mut self.pool.pool[index.0];
        if d.generation != index.1 {
            error!("MatrixNode id generation wrong {} {} {}", index.0, d.generation, index.1)
        }
        &mut d.item
        
    }
}

