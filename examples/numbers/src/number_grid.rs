use {
    crate::{
        makepad_draw_2d::*,
        makepad_component::*,
    },
    std::fmt::Write,
};

live_register!{
    import makepad_draw_2d::shader::std::*;
    DrawBg: {{DrawBg}} {
        instance hover: float
        instance focus: float
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            // lets darw a rect
            sdf.box(0,0,self.rect_size.x, self.rect_size.y,2);
            
            let base = #3;
            let up = 0.0;
            if self.last_number < self.number{
                // lets draw a tiny down arrow
                base = #a00
                up = -PI;
            }
            else if self.last_number > self.number{
                base = #0a0
                up = 0
            }
            
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
    
    DrawLabel: {{DrawLabel}} {
        instance hover: float
        instance focus: float

        fn get_color(self) -> vec4 {
            return #fff
        }
    }
    
    NumberBox: {{NumberBox}} {
       
       layout:{padding:{left:14,top:1, bottom:1, right:5}}
       
        label_align: {
            y: 0.0
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        label: {hover: 0.0}
                        bg: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        label: {hover: 1.0}
                        bg: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        label: {focus: 0.0}
                        bg: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        label: {focus: 1.0}
                        bg: {focus: 1.0}
                    }
                }
            }
        }
    }
    
    NumberGrid: {{NumberGrid}} {
        number_box: NumberBox {}
        walk: {
            width: Size::Fill,
            height: Size::Fill
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawBg {
    draw_super: DrawQuad,
    last_number: f32,
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
    bg: DrawBg,
    label: DrawLabel,

    layout: Layout,
    state: State,
    
    label_align: Align,
}

#[derive(Live, FrameComponent)]
#[live_register(frame_component!(NumberGrid))]
pub struct NumberGrid {
    scroll_bars: ScrollBars,
    walk: Walk,
    layout: Layout,
    seed: u32,
    number_box: Option<LivePtr>,
    regen_animate: bool,
    #[rust] number_boxes: ComponentMap<NumberBoxId, NumberBox>,
}

impl NumberBox {
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, NumberBoxAction)) {
        self.state_handle_event(cx, event);
        
        match event.hits(cx, self.bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animate_state(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, ids!(hover.off));
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
        // lets pass the numbers into the gpu?
        self.bg.last_number = self.bg.number;
        self.bg.number = number;
        self.label.last_number = self.label.number;
        self.label.number = number;
        // ok so. we draw a box at position X, Y
        self.bg.begin(cx, Walk::fit().with_abs_pos(pos), self.layout);
        self.label.draw_walk(cx, Walk::fit(),self.label_align, fmt);
        self.bg.end(cx);
    }
}

#[derive(Clone, FrameAction)]
pub enum NumberGridAction {
    None
}

pub enum NumberBoxAction {
    None
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct NumberBoxId(pub LiveId);

impl LiveHook for NumberGrid {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        for number_box in self.number_boxes.values_mut() {
            if let Some(index) = nodes.child_by_name(index, id!(number_box).as_field()) {
                number_box.apply(cx, from, index, nodes);
            }
        }
    }
}

fn random_bit(seed:&mut u32)->u32{
    *seed += (*seed * *seed) | 5;
    return *seed >> 31;
}

fn random_f32(seed:&mut u32)->f32{
    let mut out = 0;
    for i in 0..32{
        out |= random_bit(seed);
        out <<=1;
    }
    out as f32 / std::u32::MAX as f32
}

impl NumberGrid{
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.scroll_bars.begin(cx, Walk::default(), self.layout);

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
                number_box.draw_abs(cx, pos, number, &buf);
            }
        }
        self.scroll_bars.end(cx);
        self.number_boxes.retain_visible();
        if self.regen_animate{
           self.scroll_bars.redraw(cx);
        }
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, NumberGridAction),
    ) {
        self.scroll_bars.handle_event(cx, event, &mut |_,_|{});
        
        match event{
            Event::KeyDown(fe) if fe.key_code == KeyCode::Space=>{
                self.regen_animate = true;
                self.scroll_bars.redraw(cx);
            }
            Event::KeyUp(fe) if fe.key_code == KeyCode::Space=>{
                self.regen_animate = false;
            }
            _=>()
        }
        
        let mut actions = Vec::new();
        for (box_id, number_box) in self.number_boxes.iter_mut() {
            number_box.handle_event(cx, event, &mut | _, action | {
                actions.push((*box_id, action))
            });
        }
        
        for (_node_id, action) in actions {
            match action {
                NumberBoxAction::None=>()
            }
        }
    }
}

