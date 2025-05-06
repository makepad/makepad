use crate::makepad_platform::*;
use std::cell::RefCell;
use std::rc::Rc;

// pointer into the main document
struct DesignState{
    live_ptr: Option<LivePtr>
}

// lets query a document for the top level items
#[derive(Default, Live)]
struct DesignView{
}

// this is the main designview.
// you point it at a livefile and it should get going
struct DesignViewGlobal(Rc<RefCell<DesignView>>);

impl LiveHook for DesignView{
    fn after_new_from_doc(&mut self, _cx:&mut Cx){
        println!("HERE!");
    }
}

impl DesignView{
    pub fn handle_event(&mut self, cx:&mut Cx, event:&Event){
        let live_registry_cp = cx.live_registry.clone();
        let live_registry = live_registry_cp.borrow();
        
    }
    
    pub fn run_live_design(cx:&mut Cx, event:&Event){
        if !cx.has_global::<DesignViewGlobal>(){
            let new = DesignView::new(cx);
            cx.set_global(DesignViewGlobal(Rc::new(RefCell::new(new))));
        }
        cx.get_global::<DesignViewGlobal>().0.clone().borrow_mut().handle_event(cx, event);
    }
}
