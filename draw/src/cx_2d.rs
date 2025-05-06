use {
    std::{
        ops::Deref,
        ops::DerefMut,
    },
    crate::{
        cx_draw::CxDraw,
        makepad_math::{DVec2,Vec2Index},
        makepad_platform::{
            DrawListId,
        },
        draw_list_2d::DrawList2d,
        turtle::{Turtle, TurtleWalk, Walk, AlignEntry},
    },
};

pub struct Cx2d<'a, 'b> {
    pub cx: &'b mut CxDraw<'a>,
    pub (crate) overlay_id: Option<DrawListId>,
    
    //pub (crate) overlay_sweep_lock: Option<Rc<RefCell<Area>>>,
    pub (crate) turtles: Vec<Turtle>,
    pub (crate) turtle_walks: Vec<TurtleWalk>,
    pub (crate) turtle_clips: Vec<(DVec2, DVec2)>,
    pub (crate) align_list: Vec<AlignEntry>,
}

impl<'a, 'b> Deref for Cx2d<'a,'b> {type Target = CxDraw<'a>; fn deref(&self) -> &Self::Target {self.cx}}
impl<'a, 'b> DerefMut for Cx2d<'a,'b> {fn deref_mut(&mut self) -> &mut Self::Target {self.cx}}

impl<'a, 'b> Cx2d<'a, 'b> {
    pub fn new(cx: &'b mut CxDraw<'a>)->Self{
        Self {
            overlay_id: None,
            cx: cx,
            turtle_clips: Vec::with_capacity(1024),
            turtle_walks: Vec::with_capacity(1024),
            turtles: Vec::with_capacity(64),
            align_list: Vec::with_capacity(4096),
        }
    }
    
    pub fn will_redraw(&self, draw_list_2d: &mut DrawList2d, walk: Walk) -> bool {
        // ok so we need to check if our turtle position has changed since last time.
        // if it did, we redraw
        let rect = self.peek_walk_turtle(walk);
        if draw_list_2d.dirty_check_rect != rect {
            draw_list_2d.dirty_check_rect = rect;
            return true;
        }
        self.draw_event.draw_list_will_redraw(self, draw_list_2d.draw_list.id())
    }
        
    pub fn will_redraw_check_axis(&self, draw_list_2d: &mut DrawList2d, size:f64, axis:Vec2Index) -> bool {
        // ok so we need to check if our turtle position has changed since last time.
        // if it did, we redraw
        if draw_list_2d.dirty_check_rect.size.index(axis) != size {
            draw_list_2d.dirty_check_rect.size.set_index(axis, size);
            return true;
        }
        self.draw_event.draw_list_will_redraw(self, draw_list_2d.draw_list.id())
    }
}