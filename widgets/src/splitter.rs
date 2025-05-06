use crate::{
    makepad_derive_widget::*,
    makepad_micro_serde::*,
    makepad_draw::*,
    widget::*,
};

live_design!{
    link widgets;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    pub DrawSplitter= {{DrawSplitter}} {}
    pub SplitterBase = {{Splitter}} {}
    pub Splitter = <SplitterBase> {
        draw_bg: {
            instance drag: 0.0
            instance hover: 0.0
            
            uniform size: 110.0

            uniform color: (THEME_COLOR_D_HIDDEN),
            uniform color_hover: (THEME_COLOR_OUTSET_HOVER),
            uniform color_drag: (THEME_COLOR_OUTSET_DRAG)
            
            uniform border_radius: 1.0
            uniform splitter_pad: 1.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.clear(THEME_COLOR_BG_APP); // TODO: This should be a transparent color instead.
                
                if self.is_vertical > 0.5 {
                    sdf.box(
                        self.splitter_pad,
                        self.rect_size.y * 0.5 - self.size * 0.5,
                        self.rect_size.x - 2.0 * self.splitter_pad,
                        self.size,
                        self.border_radius
                    );
                }
                else {
                    sdf.box(
                        self.rect_size.x * 0.5 - self.size * 0.5,
                        self.splitter_pad,
                        self.size,
                        self.rect_size.y - 2.0 * self.splitter_pad,
                        self.border_radius
                    );
                }

                return sdf.fill_keep(
                    mix(
                        self.color,
                        mix(
                            self.color_hover,
                            self.color_drag,
                            self.drag
                        ),
                        self.hover
                    )
                );
            }
        }

        size: (THEME_SPLITTER_SIZE)
        min_horizontal: (THEME_SPLITTER_MIN_HORIZONTAL)
        max_horizontal: (THEME_SPLITTER_MAX_HORIZONTAL)
        min_vertical: (THEME_SPLITTER_MIN_VERTICAL)
        max_vertical: (THEME_SPLITTER_MAX_VERTICAL)
        
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {drag: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        state_drag: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {
                            drag: 0.0,
                            hover: [{time: 0.0, value: 1.0}],
                        }
                    }
                }
                
                drag = {
                    from: { all: Forward { duration: 0.1 }}
                    apply: {
                        draw_bg: {
                            drag: [{time: 0.0, value: 1.0}],
                            hover: 1.0,
                        }
                    }
                }
            }
        }
    }
    
}


#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawSplitter {
    #[deref] draw_super: DrawQuad,
    #[live] is_vertical: f32,
}

#[derive(Copy, Clone, Debug, Live, LiveHook, SerRon, DeRon)]
#[live_ignore]
pub enum SplitterAxis {
    #[pick] Horizontal,
    Vertical
}

impl Default for SplitterAxis {
    fn default() -> Self {
        SplitterAxis::Horizontal
    }
}


#[derive(Clone, Copy, Debug, Live, LiveHook, SerRon, DeRon)]
#[live_ignore]
pub enum SplitterAlign {
    #[live(50.0)] FromA(f64),
    #[live(50.0)] FromB(f64),
    #[pick(0.5)] Weighted(f64),
}

impl SplitterAlign {
    fn to_position(self, axis: SplitterAxis, rect: Rect) -> f64 {
        match axis {
            SplitterAxis::Horizontal => match self {
                Self::FromA(position) => position,
                Self::FromB(position) => rect.size.x - position,
                Self::Weighted(weight) => weight * rect.size.x,
            },
            SplitterAxis::Vertical => match self {
                Self::FromA(position) => position,
                Self::FromB(position) => rect.size.y - position,
                Self::Weighted(weight) => weight * rect.size.y,
            },
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct Splitter {
    #[live(SplitterAxis::Horizontal)] pub axis: SplitterAxis,
    #[live(SplitterAlign::Weighted(0.5))] pub align: SplitterAlign,
    #[rust] rect: Rect,
    #[rust] position: f64,
    #[rust] drag_start_align: Option<SplitterAlign>,
    #[rust] area_a: Area,
    #[rust] area_b: Area,
    #[animator] animator: Animator,
    
    #[live] min_vertical: f64,
    #[live] max_vertical: f64,
    #[live] min_horizontal: f64,
    #[live] max_horizontal: f64,
    
    #[redraw] #[live] draw_bg: DrawSplitter,
    #[live] size: f64,
    
    // framecomponent mode
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[find] #[live] a: WidgetRef,
    #[find] #[live] b: WidgetRef,
    #[walk] walk: Walk,
}

#[derive(Clone)]
enum DrawState {
    DrawA,
    DrawSplit,
    DrawB
}

impl Widget for Splitter {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        
        self.animator_handle_event(cx, event);
        match event.hits_with_options(cx, self.draw_bg.area(), HitOptions::new().with_margin(self.margin())) {
            Hit::FingerHoverIn(_) => {
                match self.axis {
                    SplitterAxis::Horizontal => cx.set_cursor(MouseCursor::ColResize),
                    SplitterAxis::Vertical => cx.set_cursor(MouseCursor::RowResize),
                }
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(_) => {
                match self.axis {
                    SplitterAxis::Horizontal => cx.set_cursor(MouseCursor::ColResize),
                    SplitterAxis::Vertical => cx.set_cursor(MouseCursor::RowResize),
                }
                self.animator_play(cx, id!(hover.drag));
                self.drag_start_align = Some(self.align);
            }
            Hit::FingerUp(f) => {
                self.drag_start_align = None;
                if f.is_over && f.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                }
            }
            Hit::FingerMove(f) => {
                if let Some(drag_start_align) = self.drag_start_align {
                    let delta = match self.axis {
                        SplitterAxis::Horizontal => f.abs.x - f.abs_start.x,
                        SplitterAxis::Vertical => f.abs.y - f.abs_start.y,
                    };
                    let new_position =
                    drag_start_align.to_position(self.axis, self.rect) + delta;
                    self.align = match self.axis {
                        SplitterAxis::Horizontal => {
                            let center = self.rect.size.x / 2.0;
                            if new_position < center - 30.0 {
                                SplitterAlign::FromA(new_position.max(self.min_vertical))
                            } else if new_position > center + 30.0 {
                                SplitterAlign::FromB((self.rect.size.x - new_position).max(self.max_vertical))
                            } else {
                                SplitterAlign::Weighted(new_position / self.rect.size.x)
                            }
                        }
                        SplitterAxis::Vertical => {
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
                    self.draw_bg.redraw(cx);
                    cx.widget_action(uid, &scope.path, SplitterAction::Changed {axis: self.axis, align: self.align});
                    
                    self.a.redraw(cx);
                    self.b.redraw(cx);
                }
            }
            _ => {}
        }
        self.a.handle_event(cx, event, scope);
        self.b.handle_event(cx, event, scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, DrawState::DrawA) {
            self.begin(cx, walk);
        }
        if let Some(DrawState::DrawA) = self.draw_state.get() {
            self.a.draw(cx, scope) ?;
            self.draw_state.set(DrawState::DrawSplit);
        }
        if let Some(DrawState::DrawSplit) = self.draw_state.get() {
            self.middle(cx);
            self.draw_state.set(DrawState::DrawB)
        }
        if let Some(DrawState::DrawB) = self.draw_state.get() {
            self.b.draw(cx, scope) ?;
            self.end(cx);
            self.draw_state.end();
        }
        DrawStep::done()
    }
}

impl Splitter {
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        // we should start a fill turtle in the layout direction of choice
        match self.axis {
            SplitterAxis::Horizontal => {
                cx.begin_turtle(walk, Layout::flow_right());
            }
            SplitterAxis::Vertical => {
                cx.begin_turtle(walk, Layout::flow_down());
            }
        }
        
        self.rect = cx.turtle().padded_rect();
        self.position = self.align.to_position(self.axis, self.rect);
        
        let walk = match self.axis {
            SplitterAxis::Horizontal => Walk::size(Size::Fixed(self.position), Size::Fill),
            SplitterAxis::Vertical => Walk::size(Size::Fill, Size::Fixed(self.position)),
        };
        cx.begin_turtle(walk, Layout::flow_down());
    }
    
    pub fn middle(&mut self, cx: &mut Cx2d) {
        cx.end_turtle_with_area(&mut self.area_a);
        match self.axis {
            SplitterAxis::Horizontal => {
                self.draw_bg.is_vertical = 1.0;
                self.draw_bg.draw_walk(cx, Walk::size(Size::Fixed(self.size), Size::Fill));
            }
            SplitterAxis::Vertical => {
                self.draw_bg.is_vertical = 0.0;
                self.draw_bg.draw_walk(cx, Walk::size(Size::Fill, Size::Fixed(self.size)));
            }
        }
        cx.begin_turtle(Walk::default(), Layout::flow_down());
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle_with_area(&mut self.area_b);
        cx.end_turtle();
    }
    
    pub fn axis(&self) -> SplitterAxis {
        self.axis
    }

    pub fn area_a(&self) -> Area {
        self.area_a
    }
    
    pub fn area_b(&self) -> Area {
        self.area_b
    }
    
    pub fn set_axis(&mut self, axis: SplitterAxis) {
        self.axis = axis;
    }
    
    pub fn align(&self) -> SplitterAlign {
        self.align
    }
    
    pub fn set_align(&mut self, align: SplitterAlign) {
        self.align = align;
    }
    
    fn margin(&self) -> Margin {
        match self.axis {
            SplitterAxis::Horizontal => Margin {
                left: 3.0,
                top: 0.0,
                right: 3.0,
                bottom: 0.0,
            },
            SplitterAxis::Vertical => Margin {
                left: 0.0,
                top: 3.0,
                right: 0.0,
                bottom: 3.0,
            },
        }
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum SplitterAction {
    None,
    Changed {axis: SplitterAxis, align: SplitterAlign},
}