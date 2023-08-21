use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawSplitter= {{DrawSplitter}} {
        uniform border_radius: 1.0
        uniform splitter_pad: 1.0
        uniform splitter_grabber: 110.0

        instance pressed: 0.0
        instance hover: 0.0
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.clear(COLOR_BG_APP);
            
            if self.is_vertical > 0.5 {
                sdf.box(
                    self.splitter_pad,
                    self.rect_size.y * 0.5 - self.splitter_grabber * 0.5,
                    self.rect_size.x - 2.0 * self.splitter_pad,
                    self.splitter_grabber,
                    self.border_radius
                );
            }
            else {
                sdf.box(
                    self.rect_size.x * 0.5 - self.splitter_grabber * 0.5,
                    self.splitter_pad,
                    self.splitter_grabber,
                    self.rect_size.y - 2.0 * self.splitter_pad,
                    self.border_radius
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
    
    Splitter= {{Splitter}} {
        split_bar_size: (DIM_SPLITTER_SIZE)
        min_horizontal: (DIM_SPLITTER_MIN_HORIZONTAL)
        max_horizontal: (DIM_SPLITTER_MAX_HORIZONTAL)
        min_vertical: (DIM_SPLITTER_MIN_VERTICAL)
        max_vertical: (DIM_SPLITTER_MAX_VERTICAL)
        
        state: {
            hover = {
                default: off
                off = {
                    cursor: Default,
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_splitter: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        state_down: Forward {duration: 0.01}
                    }
                    cursor: EwResize,
                    apply: {
                        draw_splitter: {
                            pressed: 0.0,
                            hover: [{time: 0.0, value: 1.0}],
                        }
                    }
                }
                
                pressed = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_splitter: {
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
    #[deref] draw_super: DrawQuad,
    #[live] is_vertical: f32,
}

#[derive(Live)]
pub struct Splitter {
    #[live(Axis::Horizontal)] pub axis: Axis,
    #[live(SplitterAlign::Weighted(0.5))] pub align: SplitterAlign,
    #[rust] rect: Rect,
    #[rust] position: f64,
    #[rust] drag_start_align: Option<SplitterAlign>,
    #[rust] area_a: Area,
    #[rust] area_b: Area,
    #[state] state: LiveState,
    
    #[live] min_vertical: f64,
    #[live] max_vertical: f64,
    #[live] min_horizontal: f64,
    #[live] max_horizontal: f64,
    
    #[live] draw_splitter: DrawSplitter,
    #[live] split_bar_size: f64,
    
    // framecomponent mode
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[live] a: WidgetRef,
    #[live] b: WidgetRef,
    #[live] walk: Walk,
}

impl LiveHook for Splitter{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx,Splitter)
    }
}

#[derive(Clone)]
enum DrawState {
    DrawA,
    DrawSplit,
    DrawB
}

impl Widget for Splitter {
   fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let mut redraw = false;
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid));
            redraw = true;
        });
        self.a.handle_widget_event_with(cx, event, dispatch_action);
        self.b.handle_widget_event_with(cx, event, dispatch_action);
        if redraw {
            self.a.redraw(cx);
            self.b.redraw(cx);
        }
    }
    
    fn get_walk(&self) -> Walk {
        self.walk
    }
    
    fn redraw(&mut self, cx:&mut Cx){
        self.draw_splitter.redraw(cx)
    }
    
    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results:&mut WidgetSet) {
        self.a.find_widgets(path, cached, results);
        self.b.find_widgets(path, cached, results);
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, DrawState::DrawA) {
            self.begin(cx, walk);
        }
        if let Some(DrawState::DrawA) = self.draw_state.get() {
            self.a.draw_widget(cx) ?;
            self.draw_state.set(DrawState::DrawSplit);
        }
        if let Some(DrawState::DrawSplit) = self.draw_state.get() {
            self.middle(cx);
            self.draw_state.set(DrawState::DrawB)
        }
        if let Some(DrawState::DrawB) = self.draw_state.get() {
            self.b.draw_widget(cx) ?;
            self.end(cx);
            self.draw_state.end();
        }
        WidgetDraw::done()
    }
}

impl Splitter {
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        // we should start a fill box in the layout direction of choice
        match self.axis {
            Axis::Horizontal => {
                cx.begin_box(walk, Layout::flow_right());
            }
            Axis::Vertical => {
                cx.begin_box(walk, Layout::flow_down());
            }
        }
        
        self.rect = cx.r#box().padded_rect();
        self.position = self.align.to_position(self.axis, self.rect);
        
        let walk = match self.axis {
            Axis::Horizontal => Walk::size(Length::Fixed(self.position), Length::Fill),
            Axis::Vertical => Walk::size(Length::Fill, Length::Fixed(self.position)),
        };
        cx.begin_box(walk, Layout::flow_down());
    }
    
    pub fn middle(&mut self, cx: &mut Cx2d) {
        cx.end_box_with_area(&mut self.area_a);
        match self.axis {
            Axis::Horizontal => {
                self.draw_splitter.is_vertical = 1.0;
                self.draw_splitter.draw_walk(cx, Walk::size(Length::Fixed(self.split_bar_size), Length::Fill));
            }
            Axis::Vertical => {
                self.draw_splitter.is_vertical = 0.0;
                self.draw_splitter.draw_walk(cx, Walk::size(Length::Fill, Length::Fixed(self.split_bar_size)));
            }
        }
        cx.begin_box(Walk::default(), Layout::flow_down());
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_box_with_area(&mut self.area_b);
        cx.end_box();
    }
    
    pub fn axis(&self) -> Axis {
        self.axis
    }

    pub fn area_a(&self) -> Area {
        self.area_a
    }
    
    pub fn area_b(&self) -> Area {
        self.area_b
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
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, SplitterAction),
    ) {
        self.state_handle_event(cx, event);
        match event.hits_with_options(cx, self.draw_splitter.area(), HitOptions::new().with_margin(self.margin())) {
        Hit::FingerHoverIn(_) => {
            match self.axis {
                Axis::Horizontal => cx.set_cursor(MouseCursor::ColResize),
                Axis::Vertical => cx.set_cursor(MouseCursor::RowResize),
            }
            self.animate_state(cx, id!(hover.on));
        }
        Hit::FingerHoverOut(_) => {
            self.animate_state(cx, id!(hover.off));
        },
        Hit::FingerDown(_) => {
            match self.axis {
                Axis::Horizontal => cx.set_cursor(MouseCursor::ColResize),
                Axis::Vertical => cx.set_cursor(MouseCursor::RowResize),
            }
            self.animate_state(cx, id!(hover.pressed));
            self.drag_start_align = Some(self.align);
        }
        Hit::FingerUp(f) => {
            self.drag_start_align = None;
            if f.is_over && f.device.has_hovers() {
                self.animate_state(cx, id!(hover.on));
            }
            else {
                self.animate_state(cx, id!(hover.off));
            }
        }
        Hit::FingerMove(f) => {
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
                            SplitterAlign::FromA(new_position.max(self.min_vertical))
                        } else if new_position > center + 30.0 {
                            SplitterAlign::FromB((self.rect.size.x - new_position).max(self.max_vertical))
                        } else {
                            SplitterAlign::Weighted(new_position / self.rect.size.x)
                        }
                    }
                    Axis::Vertical => {
                        let center = self.rect.size.y / 2.0;
                        if new_position < center - 30.0 {
                            SplitterAlign::FromA(new_position.max(self.min_horizontal))
                        } else if new_position > center + 30.0 {
                            SplitterAlign::FromB((self.rect.size.y - new_position).max(self.max_horizontal))
                        } else {
                            SplitterAlign::Weighted(new_position / self.rect.size.y)
                        }
                    }
                };
                self.draw_splitter.redraw(cx);
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
#[live_ignore]
pub enum SplitterAlign {
    #[live(50.0)] FromA(f64),
    #[live(50.0)] FromB(f64),
    #[pick(0.5)] Weighted(f64),
}

impl SplitterAlign {
    fn to_position(self, axis: Axis, rect: Rect) -> f64 {
        match axis {
            Axis::Horizontal => match self {
                Self::FromA(position) => position,
                Self::FromB(position) => rect.size.x - position,
                Self::Weighted(weight) => weight * rect.size.x,
            },
            Axis::Vertical => match self {
                Self::FromA(position) => position,
                Self::FromB(position) => rect.size.y - position,
                Self::Weighted(weight) => weight * rect.size.y,
            },
        }
    }
}

#[derive(Clone, WidgetAction)]
pub enum SplitterAction {
    None,
    Changed {axis: Axis, align: SplitterAlign},
}
