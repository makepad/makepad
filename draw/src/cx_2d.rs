use {
    std::{
        ops::Deref,
        ops::DerefMut
    },
    crate::{ 
        makepad_math::DVec2,
        makepad_platform::{
            DrawEvent,
            Area,
            DrawListId,
            PassId,
            Pass,
            CxPassParent,
            CxPassRect,
            Cx
        },
        nav::{
            CxNavTreeRc,
        },
        font::{
            CxFontsAtlasRc,
        },
        view::View,
        turtle::{Turtle, TurtleWalk},
    }
};

pub struct PassStackItem{
    pub pass_id: PassId,
    dpi_factor: f64, 
    draw_list_stack_len: usize,
    turtles_len: usize
}

pub struct Cx2d<'a> {
    pub cx: &'a mut Cx,
    pub (crate) draw_event: &'a DrawEvent,
    pub (crate) pass_stack: Vec<PassStackItem>,
    pub (crate) overlay_id: Option<DrawListId>,
    //pub (crate) overlay_sweep_lock: Option<Rc<RefCell<Area>>>,
    pub draw_list_stack: Vec<DrawListId>,
    pub (crate) turtles: Vec<Turtle>,
    pub (crate) turtle_walks: Vec<TurtleWalk>,
    pub (crate) align_list: Vec<Area>,
    pub fonts_atlas_rc: CxFontsAtlasRc,
    pub nav_tree_rc: CxNavTreeRc,
}

impl<'a> Deref for Cx2d<'a> {type Target = Cx; fn deref(&self) -> &Self::Target {self.cx}}
impl<'a> DerefMut for Cx2d<'a> {fn deref_mut(&mut self) -> &mut Self::Target {self.cx}}

impl<'a> Drop for Cx2d<'a>{
    fn drop(&mut self){
        self.draw_font_atlas();
    }
}

impl<'a> Cx2d<'a> {
    /*pub fn set_sweep_lock(&mut self, lock:Area){
        *self.overlay_sweep_lock.as_ref().unwrap().borrow_mut() = lock;
    }
    
    pub fn clear_sweep_lock(&mut self, lock:Area){
        let mut sweep_lock = self.overlay_sweep_lock.as_ref().unwrap().borrow_mut();
        if *sweep_lock == lock{
            *sweep_lock = Area::Empty
        }
    }*/
    
    pub fn new(cx: &'a mut Cx, draw_event: &'a DrawEvent) -> Self {
        Self::lazy_construct_font_atlas(cx);
        Self::lazy_construct_nav_tree(cx);
        cx.redraw_id += 1;
        let fonts_atlas_rc = cx.get_global::<CxFontsAtlasRc>().clone();
        let nav_tree_rc = cx.get_global::<CxNavTreeRc>().clone();

        Self {
            overlay_id: None,
            fonts_atlas_rc,
            cx: cx,
            draw_event,
           // overlay_sweep_lock: None,
            pass_stack: Vec::new(),
            draw_list_stack: Vec::new(),
            turtle_walks: Vec::new(),
            turtles: Vec::new(),
            align_list: Vec::new(),
            nav_tree_rc,
        }
    }
    
    pub fn current_dpi_factor(&self) -> f64 {
        self.pass_stack.last().unwrap().dpi_factor
    }
    
    pub fn make_child_pass(&mut self, pass: &Pass) {
        let pass_id = self.pass_stack.last().unwrap().pass_id;
        let cxpass = &mut self.passes[pass.pass_id()];
        cxpass.parent = CxPassParent::Pass(pass_id);
    }
    
    pub fn begin_pass(&mut self, pass: &Pass) {
        let cxpass = &mut self.passes[pass.pass_id()];
        
        cxpass.main_draw_list_id = None;
        
        let dpi_factor = match cxpass.parent {
            CxPassParent::Window(window_id) => {
                self.passes[pass.pass_id()].pass_rect = Some(CxPassRect::Size(self.windows[window_id].get_inner_size()));
                self.get_delegated_dpi_factor(pass.pass_id())
            }
            CxPassParent::Pass(pass_id) => {
                self.passes[pass.pass_id()].pass_rect = self.passes[pass_id].pass_rect.clone();
                self.get_delegated_dpi_factor(pass_id)
            }
            _ => {
                cxpass.override_dpi_factor = Some(1.0);
                1.0
            }
        };
        
        self.pass_stack.push(PassStackItem{
            dpi_factor ,
            draw_list_stack_len: self.draw_list_stack.len(),
            turtles_len:self.turtles.len(),
            pass_id:pass.pass_id()
        });
    }
    
    pub fn end_pass(&mut self, pass: &Pass) {
        let stack_item = self.pass_stack.pop().unwrap();
        if stack_item.pass_id != pass.pass_id() {
            panic!();
        }

        if self.draw_list_stack.len() != stack_item.draw_list_stack_len {
            panic!("Draw list stack disaligned, forgot an end_view(cx)");
        }
        if self.turtles.len() != stack_item.turtles_len {
            panic!("Turtle stack disaligned, forgot an end_turtle()");
        }
        
        
    }
    
 
    
    pub fn set_pass_area(&mut self, pass:&Pass, area: Area){
        self.passes[pass.pass_id()].pass_rect = Some(CxPassRect::Area(area));
    }
    
    pub fn current_pass_size(&self) -> DVec2 {
        self.cx.get_pass_rect(self.pass_stack.last().unwrap().pass_id, self.current_dpi_factor()).unwrap().size
    }
    
    pub fn view_will_redraw(&self, view: &View) -> bool {
        self.draw_event.draw_list_will_redraw(self, view.draw_list.id())
    }
}