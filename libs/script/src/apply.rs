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
// Apply - Source of apply operation
// ============================================================================

#[derive(Debug, Clone, Copy, Default)]
pub enum Apply {
    New,
    Reload,
    Update,
    Animate,
    Eval,
    Default(usize),
    #[default]
    Over,
}

impl Apply {
    pub fn is_from_script(&self) -> bool {
        match self {
            Self::New => true,
            Self::Reload => true,
            Self::Update => true,
            _ => false
        }
    }
    
    pub fn is_from_rust(&self) -> bool {
        match self {
            Self::Animate => true,
            Self::Eval => true,
            Self::Over => true,
            _ => false
        }
    }

    pub fn is_new(&self) -> bool {
        match self {
            Self::New => true,
            _ => false
        }
    }
    
    pub fn is_update(&self) -> bool {
        match self {
            Self::Update => true,
            _ => false
        }
    }
    
    pub fn is_reload(&self) -> bool {
        match self {
            Self::Reload => true,
            _ => false
        }
    }
    
    pub fn is_animate(&self) -> bool {
        match self {
            Self::Animate => true,
            _ => false
        }
    }
    
    pub fn is_eval(&self) -> bool {
        match self {
            Self::Eval => true,
            _ => false
        }
    }
    
    pub fn is_over(&self) -> bool {
        match self {
            Self::Over => true,
            _ => false
        }
    }
    
    pub fn as_default(&self) -> Option<usize> {
        match self {
            Self::Default(u) => Some(*u),
            _ => None
        }
    }
    
        
    pub fn is_default(&self) -> bool {
        match self {
            Self::Default(_) => true,
            _ => false
        }
    }
}
