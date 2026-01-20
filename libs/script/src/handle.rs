use crate::value::*;
use crate::heap::*;
use std::any::TypeId;
use std::fmt::Debug;
use std::fmt;

impl ScriptHeap{
     
    pub fn new_handle(&mut self, ty:ScriptHandleType, mut hgc:Box<dyn ScriptHandleGc>)->ScriptHandle{
        if let Some(mut handle) = self.handles_free.pop(){
            hgc.set_handle(handle);
            self.handles[handle.index as usize] = Some(ScriptHandleData{
                tag: Default::default(),
                handle:hgc
            });
            handle.ty = ty;
            handle
        }
        else{
            let index = self.handles.len();
            let handle = ScriptHandle{ty, index: index as _};
            hgc.set_handle(handle);
            self.handles.push(Some(ScriptHandleData{
                tag: Default::default(),
                handle:hgc
            }));
            handle
        }
    }
    
    pub fn handle_ref<T: ScriptHandleGc + 'static>(&self, handle: ScriptHandle) -> Option<&T> {
        self.handles.get(handle.index as usize)
            .and_then(|h| h.as_ref())
            .and_then(|h| h.handle.downcast_ref::<T>())
    }
    
    pub fn handle_mut<T: ScriptHandleGc + 'static>(&mut self, handle: ScriptHandle) -> Option<&mut T> {
        self.handles.get_mut(handle.index as usize)
            .and_then(|h| h.as_mut())
            .and_then(|h| h.handle.downcast_mut::<T>())
    }
}

#[derive(Default)]
pub struct HandleTag(u64);

impl HandleTag{
    const MARK:u64 = 0x1;
    
    pub fn is_marked(&self)->bool{
        self.0 & Self::MARK != 0
    }
            
    pub fn set_mark(&mut self){
        self.0 |= Self::MARK
    }
            
    pub fn clear_mark(&mut self){
        self.0 &= !Self::MARK
    }
}

pub struct ScriptHandleData{
    pub tag: HandleTag,
    pub handle: Box<dyn ScriptHandleGc>
}

impl ScriptHandleData{
    pub fn gc(mut self){
        self.handle.gc()
    }
}

pub trait ScriptHandleGc{
    fn gc(&mut self);
    fn set_handle(&mut self, _handle:ScriptHandle){}
    fn ref_cast_type_id(&self) -> TypeId where Self: 'static {TypeId::of::<Self>()}
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result{
        write!(f, "ScriptHandleGc: No debug format")
    }
}

impl Debug for dyn ScriptHandleGc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result{
        self.debug_fmt(f)
    }
}

impl dyn ScriptHandleGc{
    pub fn is<T: ScriptHandleGc + 'static> (&self)->bool{
        let t = TypeId::of::<T>();
        let concrete = self.ref_cast_type_id();
        t == concrete
    }
    pub fn downcast_ref<T: ScriptHandleGc + 'static>(&self) -> Option<&T>{
        if self.is::<T>(){
            Some(unsafe{&*(self as *const dyn ScriptHandleGc as *const T)})
        }
        else{
            None
        }
    }
    pub fn downcast_mut<T: ScriptHandleGc + 'static>(&mut self) -> Option<&mut T>{
        if self.is::<T>(){
            Some(unsafe{&mut *(self as *const dyn ScriptHandleGc as *mut T)})
        }
        else{
            None
        }
    }
}
