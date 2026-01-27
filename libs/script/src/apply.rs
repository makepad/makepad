use makepad_live_id::*;
use smallvec::*;
use std::any::Any;
use std::fmt::{Debug, Formatter};

// ============================================================================
// HeapLiveIdPath - A path of LiveIds for tracking scope hierarchy
// ============================================================================

#[derive(Default, Clone)]
pub struct HeapLiveIdPath{
    pub data: SmallVec<[LiveId;16]>, 
}

impl HeapLiveIdPath{
    pub fn last(&self)->LiveId{
        *self.data.last().unwrap_or(&LiveId(0))
    }
    
    pub fn from_end(&self, pos:usize)->LiveId{
        *self.data.iter().rev().nth(pos).unwrap_or(&LiveId(0))
    }
    
    pub fn push(&mut self, id:LiveId){
        self.data.push(id);
    }
    pub fn pop(&mut self){
        self.data.pop();
    }
}

impl Debug for HeapLiveIdPath {
    fn fmt(&self, f: &mut Formatter) -> core::fmt::Result {
        for i in 0..self.data.len(){
            if i!=0{
                let _ = write!(f, ".");
            }
            let _ = write!(f, "{}", self.data[i]);
        }
        Ok(())
    }
}

// ============================================================================
// ScopeDataRef / ScopeDataMut - Type-erased data containers for scope
// ============================================================================

#[derive(Default)]
pub struct ScopeDataRef<'a>(Option<&'a dyn Any>);

#[derive(Default)]
pub struct ScopeDataMut<'a>(Option<&'a mut dyn Any>);

impl <'a> ScopeDataRef<'a>{
    pub fn get<T: Any>(&self) -> Option<&T> {
        self.0.as_ref().and_then(|r| r.downcast_ref())
    }
}

impl <'a> ScopeDataMut<'a>{
    pub fn get<T: Any>(&mut self) -> Option<&T> {
        self.0.as_ref().and_then(|r| r.downcast_ref())
    }
                    
    pub fn get_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.0.as_mut().and_then(|r| r.downcast_mut())
    }
}

// ============================================================================
// Scope - Context passed during apply operations
// ============================================================================

#[derive(Default)]
pub struct Scope<'a,'b>{
    pub path: HeapLiveIdPath,
    pub data: ScopeDataMut<'a>,
    pub props: ScopeDataRef<'b>,
    pub index: usize
}

impl<'a,'b> Scope<'a,'b>{
    pub fn with_data<T: Any>(v: &'a mut T)->Self{
        Self{
            path:HeapLiveIdPath::default(),
            data:ScopeDataMut(Some(v)),
            props:ScopeDataRef(None),
            index: 0
        }
    }
        
    pub fn with_data_props<T: Any + Sized, U: Any + Sized>(v: &'a mut T, w: &'b U)->Self{
        Self{
            path:HeapLiveIdPath::default(),
            data:ScopeDataMut(Some(v)),
            props:ScopeDataRef(Some(w)),
            index: 0
        }
    }
        
    pub fn with_props<T: Any>(w: &'b T)->Self{
        Self{
            path:HeapLiveIdPath::default(),
            data:ScopeDataMut(None),
            props:ScopeDataRef(Some(w)),
            index: 0
        }
    }
    
    pub fn with_data_index<T: Any>(v:&'a mut T, index:usize)->Self{
        Self{
            path:HeapLiveIdPath::default(),
            data:ScopeDataMut(Some(v)),
            props:ScopeDataRef(None),
            index
        }
    }
            
    pub fn with_data_props_index<T: Any>(v:&'a mut T, w:&'b T, index:usize)->Self{
        Self{
            path:HeapLiveIdPath::default(),
            data:ScopeDataMut(Some(v)),
            props:ScopeDataRef(Some(w)),
            index
        }
    }
            
    pub fn with_props_index<T: Any>( w:&'b T, index:usize)->Self{
        Self{
            path:HeapLiveIdPath::default(),
            data:ScopeDataMut(None),
            props:ScopeDataRef(Some(w)),
            index
        }
    }
    
    pub fn empty()->Self{
        Self{
            path:HeapLiveIdPath::default(),
            data:ScopeDataMut(None),
            props:ScopeDataRef(None),
            index: 0
        }
    }
        
    pub fn with_id<F, R>(&mut self, id:LiveId, f: F) -> R where F: FnOnce(&mut Scope) -> R{
        self.path.push(id);
        let r = f(self);
        self.path.pop();
        r
    }
    
    pub fn override_props<T:Any, F, R>(&mut self, props:&'b T, f: F) -> R where F: FnOnce(&mut Scope) -> R{
        let mut props = ScopeDataRef(Some(props));
        std::mem::swap(&mut self.props, &mut props);
        let r = f(self);
        std::mem::swap(&mut self.props, &mut props);
        r
    }
    
    pub fn override_props_index<T:Any, F, R>(&mut self, props:&'b T, index:usize, f: F) -> R where F: FnOnce(&mut Scope) -> R{
        let mut props = ScopeDataRef(Some(props));
        let old_index = self.index;
        self.index = index;
        std::mem::swap(&mut self.props, &mut props);
        let r = f(self);
        std::mem::swap(&mut self.props, &mut props);
        self.index = old_index;
        r
    }
}

// ============================================================================
// ApplyFrom - Source of apply operation
// ============================================================================

/// File identifier for live DSL documents (wraps u16 index)
#[derive(Clone, Copy, Default, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ApplyFileId(pub u16);

#[derive(Debug, Clone, Copy)]
pub enum ApplyFrom {
    NewFromDoc {file_id: ApplyFileId},
    UpdateFromDoc {file_id: ApplyFileId},
    New,
    Animate,
    AnimatorInit,
    Over,
}

impl Default for ApplyFrom {
    fn default() -> Self {
        ApplyFrom::Over
    }
}

impl ApplyFrom {
    pub fn is_from_doc(&self) -> bool {
        match self {
            Self::NewFromDoc {..} => true,
            Self::UpdateFromDoc {..} => true,
            _ => false
        }
    }

    pub fn is_new_from_doc(&self) -> bool {
        match self {
            Self::NewFromDoc {..} => true,
            _ => false
        }
    }
    
    pub fn should_apply_reset(&self) -> bool {
        match self {
            Self::UpdateFromDoc{..}  => true,
            _ => false
        }
    }
    
    pub fn is_update_from_doc(&self) -> bool {
        match self {
            Self::UpdateFromDoc {..} => true,
            _ => false
        }
    }
        
    pub fn file_id(&self) -> Option<ApplyFileId> {
        match self {
            Self::NewFromDoc {file_id} => Some(*file_id),
            Self::UpdateFromDoc {file_id,..} => Some(*file_id),
            _ => None
        }
    }
    
    pub fn with_scope<'a, 'b, 'c>(self, scope:&'c mut Scope<'a,'b>)->Apply<'a, 'b, 'c>{
        Apply{
            from: self,
            scope: Some(scope)
        }
    }
}

// ============================================================================
// Apply - Context for apply operations including source and scope
// ============================================================================

pub struct Apply<'a,'b,'c> {
    pub from: ApplyFrom,
    pub scope: Option<&'c mut Scope<'a,'b>>,
}

impl Default for Apply<'_, '_, '_> {
    fn default() -> Self {
        Self {
            from: ApplyFrom::default(),
            scope: None,
        }
    }
}

impl<'a,'b, 'c> From<ApplyFrom> for Apply<'a,'b,'c> {
    fn from(from: ApplyFrom) -> Self {
        Self {
            from,
            scope: None,
        }
    }
}

impl <'a,'b,'c> Apply<'a,'b,'c>{
    pub fn override_from<F, R>(&mut self, from:ApplyFrom, f: F) -> R where F: FnOnce(&mut Apply) -> R{
        if let Some(scope) = &mut self.scope{
            f(&mut Apply{
                from: from,
                scope: Some(*scope)
            })
        }
        else{
            f(&mut Apply{
                from: from,
                scope: None
            })
            
        }
    }
}
