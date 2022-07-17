use crate::{
    makepad_platform::*,
    frame_component::*,
};


live_register!{
    use makepad_platform::shader::std::*;
    use makepad_component::theme::*;
    
    DrawSplitter: {{DrawSplitter}} {
        const BORDER_RADIUS: 1.0
        const SPLITER_PAD: 1.0
        const SPLITER_GRABBER: 110.0
        instance pressed: 0.0
        instance hover: 0.0
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.clear(COLOR_BG_APP);
            if self.is_vertical > 0.5 {
                sdf.box(
                    SPLITER_PAD,
                    self.rect_size.y * 0.5 - SPLITER_GRABBER * 0.5,
                    self.rect_size.x - 2.0 * SPLITER_PAD,
                    SPLITER_GRABBER,
                    BORDER_RADIUS
                );
            }
            else {
                sdf.box(
                    self.rect_size.x * 0.5 - SPLITER_GRABBER * 0.5,
                    SPLITER_PAD,
                    SPLITER_GRABBER,
                    self.rect_size.y - 2.0 * SPLITER_PAD,
                    BORDER_RADIUS
                );
            }
            return sdf.fill_keep(mix(
                COLOR_BG_APP,
                mix(
                    COLOR_CONTROL_HOVER,
                    COLOR_CONTROL_PRESSED,
                    self.pressed
                ),
                self.hover
            ));
        }
    }
    
    Splitter: {{Splitter}} {
        split_bar_size: (DIM_SPLITTER_SIZE)
        min_horizontal: (DIM_SPLITTER_MIN_HORIZONTAL)
        max_horizontal: (DIM_SPLITTER_MAX_HORIZONTAL)
        min_vertical: (DIM_SPLITTER_MIN_VERTICAL)
        max_vertical: (DIM_SPLITTER_MAX_VERTICAL)
        
        state:{
            hover = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        bar_quad: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Play::Forward {duration: 0.1}
                        state_down: Play::Forward {duration: 0.01}
                    }
                    apply: {
                        bar_quad: {
                            pressed: 0.0,
                            hover: [{time: 0.0, value: 1.0}],
                        }
                    }
                }
                
                pressed = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        bar_quad: {
                            pressed: [{time: 0.0, value: 1.0}],
                            hover: 1.0,
                        }
                    }
                }
            }
        }
    }
}


#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawSplitter {
    draw_super: DrawQuad,
    is_vertical: f32,
}

#[derive(Live, LiveHook)]
#[live_register(frame_component!(Splitter))]
pub struct Splitter {
    #[live(Axis::Horizontal)] pub axis: Axis,
    #[live(SplitterAlign::Weighted(0.5))] pub align: SplitterAlign,
    #[rust] rect: Rect,
    #[rust] position: f32,
    #[rust] drag_start_align: Option<SplitterAlign>,

    state: State,
    
    min_vertical: f32,
    max_vertical: f32,
    min_horizontal: f32,
    max_horizontal: f32,
    
    bar_quad: DrawSplitter,
    split_bar_size: f32,
    
    // framecomponent mode
    #[rust] draw_state: DrawStateWrap<DrawState>,
    a: FrameComponentRef,
    b: FrameComponentRef,
    walk: Walk,
}

#[derive(Clone)]
enum DrawState{
    DrawA,
    DrawSplit,
    DrawB
}

impl FrameComponent for Splitter {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, self_id: LiveId) -> FrameComponentActionRef {
        let mut actions = Vec::new();
        let mut redraw = false;
        self.handle_event_with_fn(cx, event, &mut |_,action|{
            actions.merge(self_id,action.into()); 
            redraw = true;
        });
        if let Some(child) = self.a.as_mut(){
            if redraw{
                child.redraw(cx);
            }
            actions.merge(id!(a), child.handle_component_event(cx, event, id!(a)));
        }
        if let Some(child) = self.b.as_mut(){
            if redraw{
                child.redraw(cx);
            }
            actions.merge(id!(b), child.handle_component_event(cx, event, id!(b)));
        }
        FrameActions::from_vec(actions).into()
    }
    
    fn get_walk(&self) -> Walk {
        self.walk
    }
    
    fn find_child(&self, id: &[LiveId]) -> Option<&Box<dyn FrameComponent >> {
        find_child_impl!(id, self.a, self.b)
    }
    
    fn find_child_mut(&mut self, id: &[LiveId]) -> Option<&mut Box<dyn FrameComponent >> {
        find_child_mut_impl!(id, self.a, self.b)
    }

    fn create_child(&mut self, cx:&mut Cx, at:CreateAt, id:LiveId, path: &[LiveId], nodes:&[LiveNode]) -> Option<&mut Box<dyn FrameComponent >> {
        create_child_impl!(cx, at, id, path, nodes, self.a, self.b)
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk) -> Result<(), LiveId> {
        if self.draw_state.begin(cx, DrawState::DrawA){
            self.begin(cx, walk);
        }
        if let DrawState::DrawA = self.draw_state.get(){
            if let Some(child) = self.a.as_mut(){
                child.draw_walk_component(cx)?;
            }
            self.draw_state.set(DrawState::DrawSplit);
        }
        if let DrawState::DrawSplit = self.draw_state.get(){
            self.middle(cx);
            self.draw_state.set(DrawState::DrawB)
        }
        if let DrawState::DrawB = self.draw_state.get(){
            if let Some(child) = self.b.as_mut(){
                child.draw_walk_component(cx)?;
            }
            self.end(cx);
            self.draw_state.end();
        }
        Ok(())
    }
}

impl Splitter {
    pub fn begin(&mut self, cx: &mut Cx2d, walk:Walk) {
        // we should start a fill turtle in the layout direction of choice
        match self.axis {
            Axis::Horizontal => {
                cx.begin_turtle(walk, Layout::flow_right());
            }
            Axis::Vertical => {
                cx.begin_turtle(walk, Layout::flow_down());
            }
        }
        
        self.rect = cx.turtle().padded_rect();
        self.position = self.align.to_position(self.axis, self.rect);

        let walk = match self.axis {
            Axis::Horizontal => Walk::size(Size::Fixed(self.position), Size::Fill),
            Axis::Vertical => Walk::size(Size::Fill, Size::Fixed(self.position)),
        };
        cx.begin_turtle(walk, Layout::flow_down());
    }
    
    pub fn middle(&mut self, cx: &mut Cx2d) {
        cx.end_turtle();
        match self.axis {
            Axis::Horizontal => {
                self.bar_quad.is_vertical = 1.0;
                self.bar_quad.draw_walk(cx, Walk::size(Size::Fixed(self.split_bar_size), Size::Fill));
            }
            Axis::Vertical => {
                self.bar_quad.is_vertical = 0.0;
                self.bar_quad.draw_walk(cx, Walk::size(Size::Fill, Size::Fixed(self.split_bar_size)));
            }
        }
        cx.begin_turtle(Walk::default(), Layout::flow_down());
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle();
        cx.end_turtle();
    }
    
    pub fn axis(&self) -> Axis {
        self.axis
    }
    
    pub fn set_axis(&mut self, axis: Axis) {
        self.axis = axis;
    }
    
    pub fn align(&self) -> SplitterAlign {
        self.align
    }
    
    pub fn set_align(&mut self, align: SplitterAlign) {
        self.align = align;
    }
    
    pub fn handle_event_with_fn(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, SplitterAction),
    ) {
        self.state_handle_event(cx, event);
        match event.hits_with_options(
            cx,
            self.bar_quad.area(),
            HitOptions {
                margin: Some(self.margin()),
                ..HitOptions::default()
            },
        ) {
            HitEvent::FingerHover(f) => {
                match self.axis {
                    Axis::Horizontal => cx.set_hover_mouse_cursor(MouseCursor::ColResize),
                    Axis::Vertical => cx.set_hover_mouse_cursor(MouseCursor::RowResize),
                }
                match f.hover_state {
                    HoverState::In => if !f.any_down {
                        self.animate_state(cx, ids!(hover.on));
                    },
                    HoverState::Out => if !f.any_down {
                        self.animate_state(cx, ids!(hover.off));
                    },
                    _ => ()
                }
            },
            HitEvent::FingerDown(_) => {
                match self.axis {
                    Axis::Horizontal => cx.set_down_mouse_cursor(MouseCursor::ColResize),
                    Axis::Vertical => cx.set_down_mouse_cursor(MouseCursor::RowResize),
                }
                self.animate_state(cx, ids!(hover.pressed));
                self.drag_start_align = Some(self.align);
            }
            HitEvent::FingerUp(f) => {
                self.drag_start_align = None;
                if f.is_over && f.input_type.has_hovers() {
                    self.animate_state(cx, ids!(hover.on));
                }
                else {
                    self.animate_state(cx, ids!(hover.off));
                }
            }
            HitEvent::FingerMove(f) => {
                if let Some(drag_start_align) = self.drag_start_align {
                    let delta = match self.axis {
                        Axis::Horizontal => f.abs.x - f.abs_start.x,
                        Axis::Vertical => f.abs.y - f.abs_start.y,
                    };
                    let new_position =
                    drag_start_align.to_position(self.axis, self.rect) + delta;
                    self.align = match self.axis {
                        Axis::Horizontal => {
                            let center = self.rect.size.x / 2.0;
                            if new_position < center - 30.0 {
                                SplitterAlign::FromStart(new_position.max(self.min_vertical))
                            } else if new_position > center + 30.0 {
                                SplitterAlign::FromEnd((self.rect.size.x - new_position).max(self.max_vertical))
                            } else {
                                SplitterAlign::Weighted(new_position / self.rect.size.x)
                            }
                        }
                        Axis::Vertical => {
                            let center = self.rect.size.y / 2.0;
                            if new_position < center - 30.0 {
                                SplitterAlign::FromStart(new_position.max(self.min_horizontal))
                            } else if new_position > center + 30.0 {
                                SplitterAlign::FromEnd((self.rect.size.y - new_position).max(self.max_horizontal))
                            } else {
                                SplitterAlign::Weighted(new_position / self.rect.size.y)
                            }
                        }
                    };
                    self.bar_quad.area().redraw(cx);
                    dispatch_action(cx, SplitterAction::Changed {axis: self.axis, align: self.align});
                }
            }
            _ => {}
        }
    }
    
    fn margin(&self) -> Margin {
        match self.axis {
            Axis::Horizontal => Margin {
                left: 3.0,
                top: 0.0,
                right: 3.0,
                bottom: 0.0,
            },
            Axis::Vertical => Margin {
                left: 0.0,
                top: 3.0,
                right: 0.0,
                bottom: 3.0,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Live, LiveHook)]
pub enum SplitterAlign {
    #[live(50.0)] FromStart(f32),
    #[live(50.0)] FromEnd(f32),
    #[pick(0.5)] Weighted(f32),
}

impl SplitterAlign {
    fn to_position(self, axis: Axis, rect: Rect) -> f32 {
        match axis {
            Axis::Horizontal => match self {
                Self::FromStart(position) => position,
                Self::FromEnd(position) => rect.size.x - position,
                Self::Weighted(weight) => weight * rect.size.x,
            },
            Axis::Vertical => match self {
                Self::FromStart(position) => position,
                Self::FromEnd(position) => rect.size.y - position,
                Self::Weighted(weight) => weight * rect.size.y,
            },
        }
    }
}

#[derive(Clone, FrameComponentAction)]
pub enum SplitterAction {
    None,
    Changed {axis: Axis, align: SplitterAlign},
}
