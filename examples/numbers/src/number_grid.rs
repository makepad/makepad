use {
    crate::{
        makepad_widgets::*,
    },
    std::fmt::Write,
};

live_design!{
    import makepad_draw::shader::std::*;
    DrawBg= {{DrawBg}} {
        instance hover: float
        instance focus: float
        
        fn pixel(self) -> vec4 {
            
            let base = #3;
            let up = 0.0;
            if self.last_number < self.number{
                base = #a00
                up = -PI;
            }
            else if self.last_number > self.number{
                base = #0a0
                up = 0
            }
            
            if self.fast_path > 0.0{
                return base
            }
            
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            
            sdf.box(0,0,self.rect_size.x, self.rect_size.y,2);
            
            sdf.fill(mix(base, #8, self.hover));
            
             let sz = 3.;
            let c = vec2(7.0, 0.5 * self.rect_size.y);
            sdf.rotate(up, c.x, c.y);
            sdf.move_to(c.x - sz, c.y + sz);
            sdf.line_to(c.x, c.y - sz);
            sdf.line_to(c.x + sz, c.y + sz);
            sdf.close_path();
            sdf.fill(mix(#0, #f, self.hover));
            
            return sdf.result
        }
    }
    
    DrawLabel= {{DrawLabel}} {
        instance hover: float
        instance focus: float

        fn get_color(self) -> vec4 {
            return #fff
        }
    }
    
    NumberBox= {{NumberBox}} {
       
       layout:{padding:{left:14,top:1, bottom:1, right:5}}
       
        label_align: {
            y: 0.0
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        label: {hover: 0.0}
                        draw_bg: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        label: {hover: 1.0}
                        draw_bg: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        label: {focus: 0.0}
                        draw_bg: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        label: {focus: 1.0}
                        draw_bg: {focus: 1.0}
                    }
                }
            }
        }
    }
    
    NumberGrid= {{NumberGrid}} {
        number_box: <NumberBox> {}
        walk: {
            width: Fill,
            height: Fill
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawBg {
    draw_super: DrawQuad,
    last_number: f32,
    fast_path: f32,
    number: f32
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawLabel {
    draw_super: DrawText,
    last_number: f32,
    number: f32
}

#[derive(Live, LiveHook)]
pub struct NumberBox {
    draw_bg: DrawBg,
    label: DrawLabel,

    layout: Layout,
    state: State,
    
    label_align: Align,
}

impl Widget for NumberGrid {
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid));
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx)
    }
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
    
    
}

#[derive(Live)]
#[live_design_with{
    widget_factory!(cx, NumberGrid)
}]
pub struct NumberGrid {
    scroll_bars: ScrollBars,
    walk: Walk,
    layout: Layout,
    seed: u32,
    fast_path: bool,
    number_box: Option<LivePtr>,
    regen_animate: bool,
    #[rust] number_boxes: ComponentMap<NumberBoxId, NumberBox>,
}

impl NumberBox {
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, NumberBoxAction)) {
        self.state_handle_event(cx, event);
        
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
            },
            Hit::FingerUp(_fe) => {
            }
            Hit::FingerMove(_fe) => {
                
            }
            _ => ()
        }
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos:DVec2, number:f32, fmt:&str) {
        
        self.draw_bg.last_number = self.draw_bg.number;
        self.draw_bg.number = number;
        self.label.last_number = self.label.number;
        self.label.number = number;
        
        self.draw_bg.begin(cx, Walk::fit().with_abs_pos(pos), self.layout);
        self.label.draw_walk(cx, Walk::fit(),self.label_align, fmt);
        self.draw_bg.end(cx);
    }
}

#[derive(Clone, WidgetAction)]
pub enum NumberGridAction {
    None
}

pub enum NumberBoxAction {
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct NumberBoxId(pub LiveId);

impl LiveHook for NumberGrid {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        for number_box in self.number_boxes.values_mut() {
            if let Some(index) = nodes.child_by_name(index, live_id!(number_box).as_field()) {
                number_box.apply(cx, from, index, nodes);
            }
        }
    }
}

fn random_bit(seed:&mut u32)->u32{
    *seed = seed.overflowing_add((seed.overflowing_mul(*seed)).0 | 5).0;
    return *seed >> 31;
}

fn random_f32(seed:&mut u32)->f32{
    let mut out = 0;
    for _ in 0..32{
        out |= random_bit(seed);
        out <<=1;
    }
    out as f32 / std::u32::MAX as f32
}

impl NumberGrid{
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.scroll_bars.begin(cx, walk, self.layout);

        let start_pos = cx.turtle().pos(); //+ vec2(10., 10.);
        let number_box = self.number_box;

        let mut buf = String::new();

        for y in 0..70{
            for x in 0..35{
                let box_id = LiveId(x*100+y).into();
                let number_box = self.number_boxes.get_or_insert(cx, box_id, | cx | {
                    NumberBox::new_from_ptr(cx, number_box)
                });
                let number = random_f32(&mut self.seed);
                buf.clear();
                write!(buf,"{:.3}", number).unwrap();
                let pos = start_pos + dvec2(x as f64 * 55.0,y as f64*15.0);
                number_box.draw_bg.fast_path = if self.fast_path{1.0}else{0.0};
                number_box.draw_abs(cx, pos, number, &buf);
            }
        }
        self.scroll_bars.end(cx);
        self.number_boxes.retain_visible();
        if self.regen_animate{
           self.scroll_bars.redraw(cx);
        }
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, NumberGridAction),
    ) {
        self.scroll_bars.handle_event_with(cx, event, &mut |_,_|{});
        
        match event{
            Event::KeyDown(fe) if fe.key_code == KeyCode::Space=>{
                self.regen_animate = true;
                self.scroll_bars.redraw(cx);
            }
            Event::KeyUp(fe) if fe.key_code == KeyCode::Space=>{
                self.regen_animate = false;
            }
            Event::KeyDown(fe) if fe.key_code == KeyCode::ReturnKey =>{
                self.fast_path = !self.fast_path;
                self.scroll_bars.redraw(cx);
            }
            _=>()
        }
        
        let mut actions = Vec::new();
        for (box_id, number_box) in self.number_boxes.iter_mut() {
            number_box.handle_event_with(cx, event, &mut | _, action | {
                actions.push((*box_id, action))
            });
        }
        
        for (_node_id, action) in actions {
            match action {
            }
        }
    }
}

