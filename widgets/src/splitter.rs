use crate::{
    makepad_derive_widget::*,
    makepad_draw_2d::*,
    widget::*,
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawSplitter= {{DrawSplitter}} {
        const BORDER_RADIUS = 1.0
        const SPLITER_PAD = 1.0
        const SPLITER_GRABBER = 110.0
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
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        bar: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        state_down: Forward {duration: 0.01}
                    }
                    apply: {
                        bar: {
                            pressed: 0.0,
                            hover: [{time: 0.0, value: 1.0}],
                        }
                    }
                }
                
                pressed = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        bar: {
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
#[live_design_fn(widget_factory!(Splitter))]
pub struct Splitter {
    #[live(Axis::Horizontal)] pub axis: Axis,
    #[live(SplitterAlign::Weighted(0.5))] pub align: SplitterAlign,
    #[rust] rect: Rect,
    #[rust] position: f64,
    #[rust] drag_start_align: Option<SplitterAlign>,
    
    state: State,
    
    min_vertical: f64,
    max_vertical: f64,
    min_horizontal: f64,
    max_horizontal: f64,
    
    bar: DrawSplitter,
    split_bar_size: f64,
    
    // framecomponent mode
    #[rust] draw_state: DrawStateWrap<DrawState>,
    a: WidgetRef,
    b: WidgetRef,
    walk: Walk,
}

#[derive(Clone)]
enum DrawState {
    DrawA,
    DrawSplit,
    DrawB
}

impl Widget for Splitter {
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}
    
    fn handle_widget_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let mut redraw = false;
        let uid = self.widget_uid();
        self.handle_event_fn(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid));
            redraw = true;
        });
        self.a.handle_widget_event_fn(cx, event, dispatch_action);
        self.b.handle_widget_event_fn(cx, event, dispatch_action);
        if redraw {
            self.a.redraw(cx);
            self.b.redraw(cx);
        }
    }
    
    fn get_walk(&self) -> Walk {
        self.walk
    }
    
    fn redraw(&mut self, cx:&mut Cx){
        self.bar.redraw(cx)
    }
    
    fn find_widget(&mut self, path: &[LiveId], cached: WidgetCache) -> WidgetResult {
        self.a.find_widget(path, cached) ?;
        self.b.find_widget(path, cached)
    }
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, DrawState::DrawA) {
            self.begin(cx, walk);
        }
        if let DrawState::DrawA = self.draw_state.get() {
            self.a.draw_walk_widget(cx) ?;
            self.draw_state.set(DrawState::DrawSplit);
        }
        if let DrawState::DrawSplit = self.draw_state.get() {
            self.middle(cx);
            self.draw_state.set(DrawState::DrawB)
        }
        if let DrawState::DrawB = self.draw_state.get() {
            self.b.draw_walk_widget(cx) ?;
            self.end(cx);
            self.draw_state.end();
        }
        WidgetDraw::done()
    }
}

impl Splitter {
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
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
                self.bar.is_vertical = 1.0;
                self.bar.draw_walk(cx, Walk::size(Size::Fixed(self.split_bar_size), Size::Fill));
            }
            Axis::Vertical => {
                self.bar.is_vertical = 0.0;
                self.bar.draw_walk(cx, Walk::size(Size::Fill, Size::Fixed(self.split_bar_size)));
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
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, SplitterAction),
    ) {
        self.state_handle_event(cx, event);
        match event.hits_with_options(cx, self.bar.area(), HitOptions::margin(self.margin())) {
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
            if f.is_over && f.digit.has_hovers() {
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
                self.bar.redraw(cx);
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
    #[live(50.0)] FromStart(f64),
    #[live(50.0)] FromEnd(f64),
    #[pick(0.5)] Weighted(f64),
}

impl SplitterAlign {
    fn to_position(self, axis: Axis, rect: Rect) -> f64 {
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

#[derive(Clone, WidgetAction)]
pub enum SplitterAction {
    None,
    Changed {axis: Axis, align: SplitterAlign},
}
