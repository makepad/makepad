use crate::cx::*;

pub trait ScrollBarLike<ScrollBarT> {
    fn draw_scroll_bar(&mut self, cx: &mut Cx, axis: Axis, view_area: Area, view_rect: Rect, total_size: Vec2) -> f32;
    fn handle_scroll_bar(&mut self, cx: &mut Cx, event: &mut Event) -> ScrollBarEvent;
    fn set_scroll_view_total(&mut self, cx: &mut Cx, view_total: f32);
    fn get_scroll_view_total(&self) -> f32;
    fn set_scroll_pos(&mut self, cx: &mut Cx, scroll_pos: f32) -> bool;
    fn get_scroll_pos(&self) -> f32;
    fn scroll_into_view(&mut self, cx: &mut Cx, pos: f32, size: f32);
    fn get_scroll_target(&mut self) -> f32;
    fn set_scroll_target(&mut self, cx: &mut Cx, scroll_pos_target: f32) -> bool;
}

#[derive(Clone, PartialEq, Debug)]
pub enum ScrollBarEvent {
    None,
    Scroll {scroll_pos: f32, view_total: f32, view_visible: f32},
    ScrollDone
} 

#[derive(Clone)]
pub struct ViewTexture{
    sample_count:usize,
    has_depth_stencil:bool,
    fixed_size:Option<Vec2>
}

#[derive(Clone)]
pub struct View<TScrollBar>
where TScrollBar: ScrollBarLike<TScrollBar> + Clone
{ // draw info per UI element
    pub view_id: Option<usize>,
    pub is_clipped: bool,
    pub is_overlay: bool, // this view is an overlay, rendered last
    pub scroll_h: Option<TScrollBar>,
    pub scroll_v: Option<TScrollBar>,
}

impl<TScrollBar> Style for View<TScrollBar>
where TScrollBar: ScrollBarLike<TScrollBar> + Clone
{
    fn style(_cx: &mut Cx) -> Self {
        Self {
            is_clipped: true,
            is_overlay: false,
            view_id: None,
            scroll_h: None,
            scroll_v: None
        }
    }
}

#[derive(Clone)]
pub struct NoScrollBar {
}

impl NoScrollBar {
    fn handle_no_scroll_bar(&mut self, _cx: &mut Cx, _event: &mut Event) -> ScrollBarEvent {
        ScrollBarEvent::None
    }
}

impl ScrollBarLike<NoScrollBar> for NoScrollBar {
    fn draw_scroll_bar(&mut self, _cx: &mut Cx, _axis: Axis, _view_area: Area, _view_rect: Rect, _total_size: Vec2) -> f32 {0.}
    fn handle_scroll_bar(&mut self, _cx: &mut Cx, _event: &mut Event) -> ScrollBarEvent {
        ScrollBarEvent::None
    }
    fn set_scroll_view_total(&mut self, _cx: &mut Cx, _view_total: f32) {}
    fn get_scroll_view_total(&self) -> f32 {0.}
    fn set_scroll_pos(&mut self, _cx: &mut Cx, _scroll_pos: f32) -> bool {false}
    fn get_scroll_pos(&self) -> f32 {0.}
    fn scroll_into_view(&mut self, _cx: &mut Cx, _pos: f32, _size: f32) {}
    fn set_scroll_target(&mut self, _cx: &mut Cx, _scroll_target: f32) -> bool {false}
    fn get_scroll_target(&mut self) -> f32 {0.}
}

pub type ViewRedraw = Result<(), ()>;

impl<TScrollBar> View<TScrollBar>
where TScrollBar: ScrollBarLike<TScrollBar> + Clone
{
    
    pub fn begin_view(&mut self, cx: &mut Cx, layout: Layout) -> ViewRedraw {
        
        if !cx.is_in_redraw_cycle {
            panic!("calling begin_view outside of redraw cycle is not possible!");
        }
        
        // check if we have a pass id parent
        let pass_id = *cx.pass_stack.last().expect("No pass found when begin_view");
        
        if self.view_id.is_none() { // we need a draw_list_id
            if cx.views_free.len() != 0 {
                self.view_id = Some(cx.views_free.pop().unwrap());
            }
            else {
                self.view_id = Some(cx.views.len());
                cx.views.push(CxView {..Default::default()});
            }
            let cxview = &mut cx.views[self.view_id.unwrap()];
            cxview.initialize(pass_id, self.is_clipped, cx.redraw_id);
        }
        
        let view_id = self.view_id.unwrap();
        
        let nesting_view_id = if cx.view_stack.len() > 0 {
            *cx.view_stack.last().unwrap()
        }
        else { // return the root draw list
            0
        };
        
        let (override_layout, is_root_for_pass) = if cx.passes[pass_id].main_view_id.is_none() {
            // we are the first view on a window
            let cxpass = &mut cx.passes[pass_id];
            cxpass.main_view_id = Some(view_id);
            // we should take the window geometry and abs position as our turtle layout
            (Layout {
                abs_origin: Some(Vec2 {x: 0., y: 0.}),
                abs_size: Some(cxpass.pass_size),
                ..layout
            }, true)
        }
        else {
            (layout, false)
        };
        
        let cxpass = &mut cx.passes[pass_id];
        // find the parent draw list id
        let parent_view_id = if self.is_overlay {
            if cxpass.main_view_id.is_none() {
                panic!("Cannot make overlay inside window without root view")
            };
            let main_view_id = cxpass.main_view_id.unwrap();
            main_view_id
        }
        else {
            if let Some(last_view_id) = cx.view_stack.last() {
                *last_view_id
            }
            else { // we have no parent
                view_id
            }
        };
        
        // push ourselves up the parent draw_stack
        if view_id != parent_view_id {
            // we need a new draw
            let parent_cxview = &mut cx.views[parent_view_id];
            
            let id = parent_cxview.draw_calls_len;
            parent_cxview.draw_calls_len = parent_cxview.draw_calls_len + 1;
            
            // see if we need to add a new one
            if parent_cxview.draw_calls_len > parent_cxview.draw_calls.len() {
                parent_cxview.draw_calls.push({
                    DrawCall {
                        view_id: parent_view_id,
                        draw_call_id: parent_cxview.draw_calls.len(),
                        redraw_id: cx.redraw_id,
                        sub_view_id: view_id,
                        ..Default::default()
                    }
                })
            }
            else { // or reuse a sub list node
                let draw = &mut parent_cxview.draw_calls[id];
                draw.sub_view_id = view_id;
                draw.redraw_id = cx.redraw_id;
            }
        }
        
        // set nesting draw list id for incremental repaint scanning
        cx.views[view_id].nesting_view_id = nesting_view_id;
        
        if cx.views[view_id].draw_calls_len != 0 && !cx.view_will_redraw(view_id) {
            
            // walk the turtle because we aren't drawing
            let w = Bounds::Fix(cx.views[view_id].rect.w);
            let h = Bounds::Fix(cx.views[view_id].rect.h);
            cx.walk_turtle(w, h, override_layout.margin, None);
            return Err(());
        }
        
        // prepare drawlist for drawing
        let cxview = &mut cx.views[view_id];
        
        // update drawlist ids
        cxview.redraw_id = cx.redraw_id;
        cxview.draw_calls_len = 0;
        
        cx.view_stack.push(view_id);
        
        cx.begin_turtle(&override_layout, Area::View(ViewArea {
            view_id: view_id,
            redraw_id: cx.redraw_id
        }));
        
        if is_root_for_pass {
            cx.passes[pass_id].paint_dirty = true;
        }
        
        Ok(())
    }
    
    pub fn view_will_redraw(&mut self, cx: &mut Cx)->bool{
        if let Some(view_id) = self.view_id{
            cx.view_will_redraw(view_id)
        }
        else{
            true
        }
    }
    
    pub fn handle_scroll_bars(&mut self, cx: &mut Cx, event: &mut Event) -> bool {
        let mut ret_h = ScrollBarEvent::None;
        let mut ret_v = ScrollBarEvent::None;
        
        if let Some(scroll_h) = &mut self.scroll_h {
            ret_h = scroll_h.handle_scroll_bar(cx, event);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            ret_v = scroll_v.handle_scroll_bar(cx, event);
        }
        match ret_h {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                cx.set_view_scroll_x(self.view_id.unwrap(), scroll_pos);
            },
            _ => ()
        };
        match ret_v {
            ScrollBarEvent::None => (),
            ScrollBarEvent::Scroll {scroll_pos, ..} => {
                cx.set_view_scroll_y(self.view_id.unwrap(), scroll_pos);
            },
            _ => ()
        };
        ret_h != ScrollBarEvent::None || ret_v != ScrollBarEvent::None
    }
    
    pub fn get_scroll_pos(&self, cx: &Cx) -> Vec2 {
        if let Some(view_id) = self.view_id {
            let cxview = &cx.views[view_id];
            cxview.unsnapped_scroll
        }
        else {
            Vec2::zero()
        }
    }
    
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, pos: Vec2) -> bool {
        let view_id = self.view_id.unwrap();
        //let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:cx.redraw_id});
        let mut changed = false;
        if let Some(scroll_h) = &mut self.scroll_h {
            if scroll_h.set_scroll_pos(cx, pos.x) {
                let scroll_pos = scroll_h.get_scroll_pos();
                cx.set_view_scroll_x(view_id, scroll_pos);
                changed = true;
            }
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            if scroll_v.set_scroll_pos(cx, pos.y) {
                let scroll_pos = scroll_v.get_scroll_pos();
                cx.set_view_scroll_y(view_id, scroll_pos);
                changed = true;
            }
        }
        changed
    }
    
    pub fn set_scroll_view_total(&mut self, cx: &mut Cx, view_total: Vec2) {
        if let Some(scroll_h) = &mut self.scroll_h {
            scroll_h.set_scroll_view_total(cx, view_total.x)
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            scroll_v.set_scroll_view_total(cx, view_total.y)
        }
    }
    
    pub fn get_scroll_view_total(&mut self) -> Vec2 {
        Vec2 {
            x: if let Some(scroll_h) = &mut self.scroll_h {
                scroll_h.get_scroll_view_total()
            }else {0.},
            y: if let Some(scroll_v) = &mut self.scroll_v {
                scroll_v.get_scroll_view_total()
            }else {0.}
        }
    }
    
    pub fn scroll_into_view(&mut self, cx: &mut Cx, rect: Rect) {
        if let Some(scroll_h) = &mut self.scroll_h {
            scroll_h.scroll_into_view(cx, rect.x, rect.w);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            scroll_v.scroll_into_view(cx, rect.y, rect.h);
        }
    }
    
    pub fn set_scroll_target(&mut self, cx: &mut Cx, pos: Vec2) {
        if let Some(scroll_h) = &mut self.scroll_h {
            scroll_h.set_scroll_target(cx, pos.x);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            scroll_v.set_scroll_target(cx, pos.y);
        }
    }
    
    pub fn end_view(&mut self, cx: &mut Cx) -> Area {
        let view_id = self.view_id.unwrap();
        let view_area = Area::View(ViewArea {view_id: view_id, redraw_id: cx.redraw_id});
        
        // lets ask the turtle our actual bounds
        let view_total = cx.get_turtle_bounds();
        let mut rect_now = cx.get_turtle_rect();
        if rect_now.h.is_nan() {
            rect_now.h = view_total.y;
        }
        if rect_now.w.is_nan() {
            rect_now.w = view_total.x;
        }
        
        if let Some(scroll_h) = &mut self.scroll_h {
            let scroll_pos = scroll_h.draw_scroll_bar(cx, Axis::Horizontal, view_area, rect_now, view_total);
            cx.set_view_scroll_x(view_id, scroll_pos);
        }
        if let Some(scroll_v) = &mut self.scroll_v {
            //println!("SET SCROLLBAR {} {}", rect_now.h, view_total.y);
            let scroll_pos = scroll_v.draw_scroll_bar(cx, Axis::Vertical, view_area, rect_now, view_total);
            cx.set_view_scroll_y(view_id, scroll_pos);
        }
        
        let rect = cx.end_turtle(view_area);
        
        let cxview = &mut cx.views[view_id];
        
        cxview.rect = rect;
        
        cx.view_stack.pop();
        
        return view_area
    }
    
    pub fn get_rect(&mut self, cx: &Cx) -> Rect {
        if let Some(view_id) = self.view_id {
            let cxview = &cx.views[view_id];
            return cxview.rect
        }
        Rect::zero()
    }
    
    
    pub fn redraw_view_area(&self, cx: &mut Cx) {
        if let Some(view_id) = self.view_id {
            let cxview = &cx.views[view_id];
            let area = Area::View(ViewArea {view_id: view_id, redraw_id: cxview.redraw_id});
            cx.redraw_child_area(area);
        }
        else {
            cx.redraw_child_area(Area::All)
        }
    }
    
    pub fn get_view_area(&self, cx: &Cx) -> Area {
        if let Some(view_id) = self.view_id {
            let cxview = &cx.views[view_id];
            Area::View(ViewArea {view_id: view_id, redraw_id: cxview.redraw_id})
        }
        else {
            Area::Empty
        }
    }
}

impl Cx {
    
    pub fn new_instance_draw_call(&mut self, shader: &Shader, instance_count: usize) -> InstanceArea {
        let shader_id = shader.shader_id.unwrap();
        let sh = &self.shaders[shader_id];
        
        let current_view_id = *self.view_stack.last().unwrap();
        
        let draw_list = &mut self.views[current_view_id];
        // we need a new draw call
        let draw_call_id = draw_list.draw_calls_len;
        draw_list.draw_calls_len = draw_list.draw_calls_len + 1;
        
        // see if we need to add a new one
        if draw_call_id >= draw_list.draw_calls.len() {
            draw_list.draw_calls.push(DrawCall {
                draw_call_id: draw_call_id,
                view_id: current_view_id,
                redraw_id: self.redraw_id,
                sub_view_id: 0,
                shader_id: shader_id,
                uniforms_required: sh.mapping.named_uniform_props.total_slots,
                instance: Vec::new(),
                uniforms: Vec::new(),
                textures_2d: Vec::new(),
                current_instance_offset: 0,
                instance_dirty: true,
                uniforms_dirty: true,
                platform: PlatformDrawCall {..Default::default()}
            });
            let dc = &mut draw_list.draw_calls[draw_call_id];
            return dc.get_current_instance_area(instance_count);
        }
        
        // reuse a draw
        let dc = &mut draw_list.draw_calls[draw_call_id];
        dc.shader_id = shader_id;
        dc.uniforms_required = sh.mapping.named_uniform_props.total_slots;
        dc.sub_view_id = 0; // make sure its recognised as a draw call
        // truncate buffers and set update frame
        dc.redraw_id = self.redraw_id;
        dc.instance.truncate(0);
        dc.current_instance_offset = 0;
        dc.uniforms.truncate(0);
        dc.textures_2d.truncate(0);
        dc.instance_dirty = true;
        dc.uniforms_dirty = true;
        return dc.get_current_instance_area(instance_count);
    }
    
    pub fn new_instance(&mut self, shader: &Shader, instance_count: usize) -> InstanceArea {
        let shader_id = shader.shader_id.unwrap();
        if !self.is_in_redraw_cycle {
            panic!("calling new_instance outside of redraw cycle is not possible!");
        }
        let current_view_id = *self.view_stack.last().unwrap();
        let draw_list = &mut self.views[current_view_id];
        let sh = &self.shaders[shader_id];
        // find our drawcall to append to the current layer
        if draw_list.draw_calls_len > 0 {
            for i in (0..draw_list.draw_calls_len).rev() {
                let dc = &mut draw_list.draw_calls[i];
                if dc.sub_view_id == 0 && dc.shader_id == shader_id {
                    // reuse this drawcmd and add an instance
                    dc.current_instance_offset = dc.instance.len();
                    let slot_align = dc.instance.len() % sh.mapping.instance_slots;
                    if slot_align != 0 {
                        panic!("Instance offset disaligned! shader: {} misalign: {} slots: {}", shader_id, slot_align, sh.mapping.instance_slots);
                    }
                    return dc.get_current_instance_area(instance_count);
                }
            }
        }
        
        self.new_instance_draw_call(shader, instance_count)
    }
    
    pub fn align_instance(&mut self, instance_area:InstanceArea) -> AlignedInstance {
        let align_index = self.align_list.len();
        self.align_list.push(Area::Instance(instance_area.clone()));
        AlignedInstance {
            inst: instance_area,
            index: align_index
        }
    }

    pub fn update_aligned_instance_count(&mut self, aligned_instance: &AlignedInstance) {
        if let Area::Instance(instance) = &mut self.align_list[aligned_instance.index] {
            instance.instance_count = aligned_instance.inst.instance_count;
        }
    }
    
    pub fn set_view_scroll_x(&mut self, view_id:usize, scroll_pos:f32){
        let fac = self.get_delegated_dpi_factor(self.views[view_id].pass_id);
        let cxview = &mut self.views[view_id];
        cxview.unsnapped_scroll.x = scroll_pos;
        let snapped = scroll_pos - scroll_pos % (1.0 / fac);
        if cxview.uniforms[VW_UNI_SCROLL + 0] != snapped{
            cxview.uniforms[VW_UNI_SCROLL + 0] = snapped;
            self.passes[cxview.pass_id].paint_dirty = true;
        }
    }


    pub fn set_view_scroll_y(&mut self, view_id:usize, scroll_pos: f32) {
        let fac = self.get_delegated_dpi_factor(self.views[view_id].pass_id);
        let cxview = &mut self.views[view_id];
        cxview.unsnapped_scroll.y = scroll_pos;
        let snapped = scroll_pos - scroll_pos % (1.0 / fac);
        if cxview.uniforms[VW_UNI_SCROLL + 1] != snapped{
            cxview.uniforms[VW_UNI_SCROLL + 1] = snapped;
            self.passes[cxview.pass_id].paint_dirty = true;
        }
    }
        
}


#[derive(Clone)]
pub struct AlignedInstance {
    pub inst: InstanceArea,
    pub index: usize
}

#[derive(Default, Clone)]
pub struct DrawCall {
    pub draw_call_id: usize,
    pub view_id: usize,
    pub redraw_id: u64,
    pub sub_view_id: usize, // if not 0, its a subnode
    pub shader_id: usize, // if shader_id changed, delete gl vao
    pub instance: Vec<f32>,
    pub current_instance_offset: usize, // offset of current instance
    pub uniforms: Vec<f32>, // draw uniforms
    pub uniforms_required: usize,
    pub textures_2d: Vec<u32>,
    pub instance_dirty: bool,
    pub uniforms_dirty: bool,
    pub platform: PlatformDrawCall
}

impl DrawCall {
    pub fn need_uniforms_now(&self) -> bool {
        self.uniforms.len() < self.uniforms_required
    }
    
    pub fn get_current_instance_area(&self, instance_count: usize) -> InstanceArea {
        InstanceArea {
            view_id: self.view_id,
            draw_call_id: self.draw_call_id,
            redraw_id: self.redraw_id,
            instance_offset: self.current_instance_offset,
            instance_count: instance_count
        }
    }
}

const VW_UNI_SCROLL: usize = 0;
const VW_UNI_CLIP: usize = 4;
const VW_VIEW_TRANSFORM: usize = 8;
const VW_UNI_SIZE: usize = 24;

#[derive(Default, Clone)]
pub struct CxView {
    pub nesting_view_id: usize, // the id of the parent we nest in, codeflow wise
    pub redraw_id: u64,
    pub pass_id: usize,
    pub draw_calls: Vec<DrawCall>,
    pub draw_calls_len: usize,
    pub uniforms: Vec<f32>, // cmdlist uniforms
    pub unsnapped_scroll: Vec2,
    pub platform: CxPlatformView,
    pub rect: Rect,
    pub clipped: bool
}

impl CxView {
    pub fn initialize(&mut self, pass_id:usize, clipped: bool, redraw_id: u64) {
        self.clipped = clipped;
        self.redraw_id = redraw_id;
        self.pass_id = pass_id;
        self.uniforms.resize(VW_UNI_SIZE, 0.0);
    }
    
    pub fn set_clipping_uniforms(&mut self) {
        if self.clipped {
           self.uniform_view_clip(self.rect.x, self.rect.y, self.rect.x + self.rect.w, self.rect.y + self.rect.h);
        }
        else {
            self.uniform_view_clip(-50000.0, -50000.0, 50000.0, 50000.0);
        }
    }
    
    pub fn def_uniforms(sg: ShaderGen)->ShaderGen{
        sg.compose(shader_ast!({
            let view_scroll: vec2<UniformVw>;
            let view_clip: vec4<UniformVw>;
            let view_transform: mat4<UniformVw>;
        }))
    }
    
    //pub fn get_scroll_pos(&self) -> Vec2 {
    //    return self.unsnapped_scroll
        //return Vec2 {x: self.uniforms[VW_UNI_SCROLL + 0], y: self.uniforms[VW_UNI_SCROLL + 1]}
    //}
    
    pub fn uniform_view_transform(&mut self, v: &Mat4) {
        //dump in uniforms
        for i in 0..16 {
            self.uniforms[VW_VIEW_TRANSFORM + i] = v.v[i];
        }
    }
        
    pub fn clip_and_scroll_rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        let mut x1 = x - self.uniforms[VW_UNI_SCROLL + 0];
        let mut y1 = y - self.uniforms[VW_UNI_SCROLL + 1];
        let mut x2 = x1 + w;
        let mut y2 = y1 + h;
        let min_x = self.uniforms[VW_UNI_CLIP + 0];
        let min_y = self.uniforms[VW_UNI_CLIP + 1];
        let max_x = self.uniforms[VW_UNI_CLIP + 2];
        let max_y = self.uniforms[VW_UNI_CLIP + 3];
        x1 = min_x.max(x1).min(max_x);
        y1 = min_y.max(y1).min(max_y);
        x2 = min_x.max(x2).min(max_x);
        y2 = min_y.max(y2).min(max_y);
        return Rect {x: x1, y: y1, w: x2 - x1, h: y2 - y1};
    }
    
    pub fn uniform_view_clip(&mut self, min_x: f32, min_y: f32, max_x: f32, max_y: f32) {
        
        self.uniforms[VW_UNI_CLIP + 0] = min_x;
        self.uniforms[VW_UNI_CLIP + 1] = min_y;
        self.uniforms[VW_UNI_CLIP + 2] = max_x;
        self.uniforms[VW_UNI_CLIP + 3] = max_y;
    }
}
