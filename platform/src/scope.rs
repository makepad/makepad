use crate::makepad_live_id::*;
use std::any::Any;
use std::fmt::{Debug, Formatter};

#[derive(Default, Clone)]
pub struct HeapLiveIdPath{
    static_path:[LiveId;16], 
    static_len:usize, 
    dyn_path:Vec<LiveId>,
}

impl HeapLiveIdPath{
    pub fn get(&self, id:usize)->LiveId{
        if id >= self.dyn_path.len(){
            if id >= self.static_len{
                return LiveId(0)
            }
            else{
                let idx = self.static_len - id - 1;
                self.static_path[idx]
            }
        }
        else{
            let idx = self.dyn_path.len() - id - 1;
            *self.dyn_path.get(idx).unwrap_or(&LiveId(0))
        }
    }
    pub fn push(&mut self, id:LiveId){
        if self.static_len < self.static_path.len(){
            self.static_path[self.static_len] = id;
            self.static_len += 1;
        }
        else{
            self.dyn_path.push(id);
        }
    }
    pub fn pop(&mut self){
        if self.dyn_path.len()>0{
            self.dyn_path.pop();
        }
        else if self.static_len>0{
            self.static_len -= 1;
        }
    }
}

impl Debug for HeapLiveIdPath {
    fn fmt(&self, f: &mut Formatter) -> core::fmt::Result {
        for i in 0..self.static_len{
            if i!=0{
                let _ = write!(f, ".");
            }
            let _ = write!(f, "{}", self.static_path[i]);
        }
        for i in 0..self.dyn_path.len(){
            let _ = write!(f, ".");
            let _ = write!(f, "{}", self.dyn_path[i]);
        }
        Ok(())
    }
}


#[derive(Default)]
pub struct Scope<'a,'b>{
    pub path: HeapLiveIdPath,
    pub data: ScopeDataMut<'a>,
    pub props: ScopeDataRef<'b>,
    pub index: usize
}

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

impl<'a,'b> Scope<'a,'b>{
    pub fn with_data<T: Any>(v: &'a mut T)->Self{
        Self{
            path:HeapLiveIdPath::default(),
            data:ScopeDataMut(Some(v)),
            props:ScopeDataRef(None),
            index: 0
        }
    }
        
    pub fn with_data_props<T: Any>(v: &'a mut T, w: &'b T)->Self{
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
}