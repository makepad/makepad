use std::any::TypeId;
use std::fmt::Debug;
use std::fmt;

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
    fn gc(&mut self){}
    fn ref_cast_type_id(&self) -> TypeId where Self: 'static {TypeId::of::<Self>()}
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

impl<T: 'static + Debug + ?Sized > ScriptHandleGc for T {
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result{
        self.fmt(f)
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
