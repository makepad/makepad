
use crate::os::linux::openxr_sys::LibOpenXr;

#[derive(Default)]
pub struct CxAndroidOpenXr{
    openxr: Option<LibOpenXr>
}

impl CxAndroidOpenXr{
    pub fn init(&mut self){
        crate::log!("INITIALISING!!");
        self.openxr = LibOpenXr::try_load()
    }
}