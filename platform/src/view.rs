pub use {
    std::{
        rc::Rc,
        cell::RefCell
    },
    crate::{
        makepad_live_compiler::*,
        platform::{
            CxPlatformDrawCall,
            CxPlatformView,
        },
        cx::{
            Cx,
        },
        area::{Area, ViewArea, InstanceArea},
        live_traits::*,
        turtle::{Layout, Width, Height, Walk, Rect},
        cx_draw_shaders::{
            CxDrawShaderOptions,
            CxDrawShaderMapping,
            CxDrawShader,
            DrawShader,
        },
        draw_vars::{
            DrawVars,
            DRAW_CALL_USER_UNIFORMS,
            DRAW_CALL_TEXTURE_SLOTS
        },
        geometry::Geometry
    }
};

pub type ViewRedraw = Result<(), ()>;

#[derive(Debug)]
pub struct View { // draw info per UI element
    pub view_id: usize, //Option<usize>,
    pub redraw_id: u64,
    pub views_free: Rc<RefCell<Vec<usize >> >,
}

impl Drop for View {
    fn drop(&mut self) {
        self.views_free.borrow_mut().push(self.view_id)
    }
}

impl LiveHook for View {}
impl LiveNew for View {
    fn new(cx: &mut Cx) -> Self {
        let views_free = cx.views_free.clone();
        let view_id = if let Some(view_id) = views_free.borrow_mut().pop() {
            cx.views[view_id].alloc_generation += 1;
            view_id
        }
        else {
            let view_id = cx.views.len();
            cx.views.push(CxView::new());
            view_id
        };
        
        Self {
            views_free: views_free,
            redraw_id: 0,
            view_id,
        }
    }
    
    fn live_type_info(_cx:&mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            fields: Vec::new(),
            //kind: LiveTypeKind::Object,
            type_name: LiveId::from_str("View").unwrap()
        }
    }
}
impl LiveApply for View {
    //fn type_id(&self) -> std::any::TypeId {std::any::TypeId::of::<Self>()}
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, id!(View));
            return nodes.skip_node(start_index);
        }
        cx.views[self.view_id].debug_id = nodes[start_index].id;
        let mut index = start_index + 1;
        loop {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            match nodes[index].id {
                id!(debug_id) => cx.views[self.view_id].debug_id = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes),
                id!(is_clipped) => cx.views[self.view_id].is_clipped = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes),
                id!(is_overlay) => cx.views[self.view_id].is_overlay = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes),
                id!(always_redraw) => cx.views[self.view_id].always_redraw = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes),
                id!(layout) => cx.views[self.view_id].layout = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes),
                _ => {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    index = nodes.skip_node(index);
                }
            }
        }
        return index;
    }
}

impl View {
    
    pub fn set_is_clipped(&self, cx: &mut Cx, is_clipped: bool) {cx.views[self.view_id].is_clipped = is_clipped;}
    pub fn set_is_overlay(&self, cx: &mut Cx, is_overlay: bool) {cx.views[self.view_id].is_overlay = is_overlay;}
    pub fn set_always_redraw(&self, cx: &mut Cx, always_redraw: bool) {cx.views[self.view_id].always_redraw = always_redraw;}
    
    pub fn set_layout(&self, cx: &mut Cx, layout: Layout) {
        cx.views[self.view_id].layout = layout;
    }
    
    pub fn layout(&self, cx: &mut Cx) -> Layout {
        cx.views[self.view_id].layout
    }
    
    
    pub fn lock_view_transform(&self, cx: &mut Cx, mat: &Mat4) {
        let cxview = &mut cx.views[self.view_id];
        cxview.uniform_view_transform(mat);
        return cxview.locked_view_transform = true;
    }
    
    pub fn set_view_transform(&self, cx: &mut Cx, mat: &Mat4) {
        
        fn set_view_transform_recur(view_id: usize, cx: &mut Cx, mat: &Mat4) {
            if cx.views[view_id].locked_view_transform {
                return
            }
            cx.views[view_id].uniform_view_transform(mat);
            let draw_items_len = cx.views[view_id].draw_items_len;
            for draw_item_id in 0..draw_items_len {
                if let Some(sub_view_id) = cx.views[view_id].draw_items[draw_item_id].sub_view_id {
                    set_view_transform_recur(sub_view_id, cx, mat);
                }
            }
        }
        
        set_view_transform_recur(self.view_id, cx, mat);
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d) -> ViewRedraw {
        
        if !cx.in_redraw_cycle {
            panic!("calling begin_view outside of redraw cycle is not possible!");
        }
        
        // check if we have a pass id parent
        let pass_id = cx.draw_pass_id.expect("No pass found when begin_view");
        cx.views[self.view_id].pass_id = pass_id;
        
        let codeflow_parent_id = cx.view_stack.last();
        
        let view_will_redraw = cx.view_will_redraw(self.view_id);
        
        let cxpass = &mut cx.passes[pass_id];
        
        if cxpass.main_view_id.is_none() {
            cxpass.main_view_id = Some(self.view_id);
            cx.views[self.view_id].layout.abs_origin = Some(Vec2 {x: 0., y: 0.});
            cx.views[self.view_id].layout.abs_size = Some(cxpass.pass_size);
        }
        
        let cxpass = &mut cx.passes[pass_id];
        
        // find the parent draw list id
        let parent_view_id = if cx.views[self.view_id].is_overlay {
            if cxpass.main_view_id.is_none() {
                panic!("Cannot make overlay inside window without root view")
            };
            let main_view_id = cxpass.main_view_id.unwrap();
            Some(main_view_id)
        }
        else {
            cx.view_stack.last().cloned()
        };
        
        // push ourselves up the parent draw_stack
        if let Some(parent_view_id) = parent_view_id {
            // copy the view transform
            
            if !cx.views[self.view_id].locked_view_transform {
                for i in 0..16 {
                    cx.views[self.view_id].view_uniforms.view_transform[i] =
                    cx.views[parent_view_id].view_uniforms.view_transform[i];
                }
            }
            
            let parent = &mut cx.views[parent_view_id];
            
            // see if we need to add a new one
            if parent.draw_items_len >= parent.draw_items.len() {
                parent.draw_items.push({
                    DrawItem {
                        view_id: parent_view_id,
                        draw_item_id: parent.draw_items.len(),
                        redraw_id: cx.redraw_id,
                        sub_view_id: Some(self.view_id),
                        draw_call: None
                    }
                });
                parent.draw_items_len += 1;
            }
            else { // or reuse a sub list node
                let draw_item = &mut parent.draw_items[parent.draw_items_len];
                draw_item.sub_view_id = Some(self.view_id);
                draw_item.redraw_id = cx.redraw_id;
                parent.draw_items_len += 1;
            }
        }
        
        // set nesting draw list id for incremental repaint scanning
        cx.views[self.view_id].codeflow_parent_id = codeflow_parent_id.cloned();
        
        // check redraw status
        if !cx.views[self.view_id].always_redraw
            && cx.views[self.view_id].draw_items_len != 0
            && !view_will_redraw {
            
            // walk the turtle because we aren't drawing
            let w = Width::Fixed(cx.views[self.view_id].rect.size.x);
            let h = Height::Fixed(cx.views[self.view_id].rect.size.y);
            cx.walk_turtle(Walk {width: w, height: h, margin: cx.views[self.view_id].layout.walk.margin});
            return Err(());
        }
        
        if cxpass.main_view_id.unwrap() == self.view_id {
            cx.passes[pass_id].paint_dirty = true;
        }
        
        let cxview = &mut cx.views[self.view_id];
        
        // update redarw id
        let last_redraw_id = cxview.redraw_id;
        self.redraw_id = cx.redraw_id;
        cxview.redraw_id = cx.redraw_id;
        
        cxview.draw_items_len = 0;
        
        cx.view_stack.push(self.view_id);
        
        let old_area = Area::View(ViewArea {view_id: self.view_id, redraw_id: last_redraw_id});
        let new_area = Area::View(ViewArea {view_id: self.view_id, redraw_id: cx.redraw_id});
        
        cx.update_area_refs(old_area, new_area);
        cx.begin_turtle_with_guard(cx.views[self.view_id].layout, new_area);
        
        Ok(())
    }
    
    
    pub fn view_will_redraw(&self, cx: &mut Cx) -> bool {
        cx.view_will_redraw(self.view_id)
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) -> Area {
        // let view_id = self.view_id.unwrap();
        let view_area = Area::View(ViewArea {view_id: self.view_id, redraw_id: cx.redraw_id});
        let rect = cx.end_turtle_with_guard(view_area);
        let cxview = &mut cx.views[self.view_id];
        cxview.rect = rect;
        cx.view_stack.pop();
        view_area
    }
    
    pub fn get_rect(&self, cx: &Cx) -> Rect {
        let cxview = &cx.views[self.view_id];
        return cxview.rect
    }
    
    pub fn get_view_transform(&self, cx: &Cx) -> Mat4 {
        let cxview = &cx.views[self.view_id];
        return cxview.get_view_transform()
    }
    
    pub fn set_view_debug(&self, cx: &mut Cx, view_debug: CxViewDebug) {
        let cxview = &mut cx.views[self.view_id];
        cxview.debug = Some(view_debug);
    }
    
    pub fn redraw(&self, cx: &mut Cx) {
        cx.redraw_view_of(self.area());
    }
    
    pub fn redraw_view_and_children(&self, cx: &mut Cx) {
        cx.redraw_view_and_children_of(self.area());
    }
    
    pub fn area(&self) -> Area {
        Area::View(ViewArea {view_id: self.view_id, redraw_id: self.redraw_id})
    }
}


impl Cx {
    
    pub fn new_draw_call(&mut self, draw_vars: &DrawVars) -> Option<&mut DrawItem> {
        return self.get_draw_call(false, draw_vars);
    }
    
    pub fn append_to_draw_call(&mut self, draw_vars: &DrawVars) -> Option<&mut DrawItem> {
        return self.get_draw_call(true, draw_vars);
    }
    
    pub fn get_draw_call(&mut self, append: bool, draw_vars: &DrawVars) -> Option<&mut DrawItem> {
        
        if draw_vars.draw_shader.is_none(){
            return None
        }
        let draw_shader = draw_vars.draw_shader.unwrap();
        
        if draw_shader.draw_shader_generation != self.draw_shaders.generation{
            return None
        }
        
        let sh = &self.draw_shaders[draw_shader.draw_shader_id];
        
        let current_view_id = *self.view_stack.last().unwrap();
        let cxview = &mut self.views[current_view_id];
        let draw_item_id = cxview.draw_items_len;
        
        if append && !sh.mapping.flags.draw_call_always {
            if let Some(index) = cxview.find_appendable_drawcall(sh, draw_vars) {
                return Some(&mut cxview.draw_items[index]);
            }
        }
        
        // add one
        cxview.draw_items_len += 1;
        
        // see if we need to add a new one
        if draw_item_id >= cxview.draw_items.len() {
            cxview.draw_items.push(DrawItem {
                draw_item_id: draw_item_id,
                view_id: current_view_id,
                redraw_id: self.redraw_id,
                sub_view_id: None,
                draw_call: Some(DrawCall::new(&sh.mapping, draw_vars))
            });
            return Some(&mut cxview.draw_items[draw_item_id]);
        }
        // reuse an older one, keeping all GPU resources attached
        let mut draw_item = &mut cxview.draw_items[draw_item_id];
        draw_item.sub_view_id = None;
        draw_item.redraw_id = self.redraw_id;
        if let Some(dc) = &mut draw_item.draw_call {
            dc.reuse_in_place(&sh.mapping, draw_vars);
        }
        else {
            draw_item.draw_call = Some(DrawCall::new(&sh.mapping, draw_vars))
        }
        return Some(draw_item);
    }
    
    pub fn begin_many_instances(&mut self, draw_vars: &DrawVars) -> Option<ManyInstances> {
        
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none(){
            return None
        }
        let draw_item = draw_item.unwrap();
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        let mut instances = None;
        
        std::mem::swap(&mut instances, &mut draw_call.instances);
        Some(ManyInstances {
            instance_area: InstanceArea {
                view_id: draw_item.view_id,
                draw_item_id: draw_item.draw_item_id,
                instance_count: 0,
                instance_offset: instances.as_ref().unwrap().len(),
                redraw_id: draw_item.redraw_id
            },
            aligned: None,
            instances: instances.unwrap()
        })
    }
    
    pub fn begin_many_aligned_instances(&mut self, draw_vars: &DrawVars) -> Option<ManyInstances> {
        let mut li = self.begin_many_instances(draw_vars);
        if li.is_none(){
            return None;
        }
        li.as_mut().unwrap().aligned = Some(self.align_list.len());
        self.align_list.push(Area::Empty);
        li
    }
    
    pub fn end_many_instances(&mut self, many_instances: ManyInstances) -> Area {
        let mut ia = many_instances.instance_area;
        let cxview = &mut self.views[ia.view_id];
        let draw_item = &mut cxview.draw_items[ia.draw_item_id];
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        
        let mut instances = Some(many_instances.instances);
        std::mem::swap(&mut instances, &mut draw_call.instances);
        ia.instance_count = (draw_call.instances.as_ref().unwrap().len() - ia.instance_offset) / draw_call.total_instance_slots;
        if let Some(aligned) = many_instances.aligned {
            self.align_list[aligned] = ia.clone().into();
        }
        ia.into()
    }
    
    pub fn add_instance(&mut self, draw_vars: &DrawVars) -> Area {
        let data = draw_vars.as_slice();
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none(){
            return Area::Empty
        }
        let draw_item = draw_item.unwrap();
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        let instance_count = data.len() / draw_call.total_instance_slots;
        let check = data.len() % draw_call.total_instance_slots;
        if check > 0 {
            panic!("Data not multiple of total slots");
        }
        let ia = InstanceArea {
            view_id: draw_item.view_id,
            draw_item_id: draw_item.draw_item_id,
            instance_count: instance_count,
            instance_offset: draw_call.instances.as_ref().unwrap().len(),
            redraw_id: draw_item.redraw_id
        };
        draw_call.instances.as_mut().unwrap().extend_from_slice(data);
        ia.into()
    }
    
    pub fn add_aligned_instance(&mut self, draw_vars: &DrawVars) -> Area {
        let data = draw_vars.as_slice();
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none(){
            return Area::Empty
        }
        let draw_item = draw_item.unwrap();
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        let instance_count = data.len() / draw_call.total_instance_slots;
        let check = data.len() % draw_call.total_instance_slots;
        if check > 0 {
            println!("Data not multiple of total slots");
            return Area::Empty
        }
        let ia: Area = (InstanceArea {
            view_id: draw_item.view_id,
            draw_item_id: draw_item.draw_item_id,
            instance_count: instance_count,
            instance_offset: draw_call.instances.as_ref().unwrap().len(),
            redraw_id: draw_item.redraw_id
        }).into();
        draw_call.instances.as_mut().unwrap().extend_from_slice(data);
        self.align_list.push(ia.clone());
        ia
    }
    
}

#[derive(Debug)]
pub struct ManyInstances {
    pub instance_area: InstanceArea,
    pub aligned: Option<usize>,
    pub instances: Vec<f32>
}

#[derive(Clone)]
pub struct AlignedInstance {
    pub inst: InstanceArea,
    pub index: usize
}

#[derive(Default, Clone)]
#[repr(C)]
pub struct DrawUniforms {
    pub draw_clip_x1: f32,
    pub draw_clip_y1: f32,
    pub draw_clip_x2: f32,
    pub draw_clip_y2: f32,
    pub draw_scroll: Vec4,
    pub draw_zbias: f32,
    pub pad1: f32,
    pub pad2: f32,
    pub pad3: f32
}

impl DrawUniforms {
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<DrawUniforms>()] {
        unsafe {std::mem::transmute(self)}
    }
}

pub struct DrawItem {
    pub draw_item_id: usize,
    pub view_id: usize,
    pub redraw_id: u64,
    
    pub sub_view_id: Option<usize>,
    pub draw_call: Option<DrawCall>,
}

pub struct UserUniforms {
    
}

pub struct DrawCall {
    pub draw_shader: DrawShader, // if shader_id changed, delete gl vao
    
    pub options: CxDrawShaderOptions,
    
    pub instances: Option<Vec<f32 >>,
    pub total_instance_slots: usize,
    
    pub draw_uniforms: DrawUniforms, // draw uniforms
    pub geometry_id: Option<usize>,
    pub user_uniforms: [f32; DRAW_CALL_USER_UNIFORMS], // user uniforms
    
    pub texture_slots: [Option<usize>; DRAW_CALL_TEXTURE_SLOTS],
    pub instance_dirty: bool,
    pub uniforms_dirty: bool,
    pub platform: CxPlatformDrawCall
}

impl DrawCall {
    
    pub fn new(mapping: &CxDrawShaderMapping, draw_vars: &DrawVars) -> Self {

        DrawCall {
            geometry_id: draw_vars.geometry_id,
            options: draw_vars.options.clone(),
            draw_shader: draw_vars.draw_shader.unwrap(),
            instances: Some(Vec::new()),
            total_instance_slots: mapping.instances.total_slots,
            draw_uniforms: DrawUniforms::default(),
            user_uniforms: draw_vars.user_uniforms,
            texture_slots: draw_vars.texture_slots,
            instance_dirty: true,
            uniforms_dirty: true,
            platform: CxPlatformDrawCall::default()
        }
    }
    
    pub fn reuse_in_place(&mut self, mapping: &CxDrawShaderMapping, draw_vars: &DrawVars) {
        self.draw_shader = draw_vars.draw_shader.unwrap();
        self.geometry_id = draw_vars.geometry_id;
        self.instances.as_mut().unwrap().clear();
        self.total_instance_slots = mapping.instances.total_slots;
        for i in 0..mapping.user_uniforms.total_slots {
            self.user_uniforms[i] = draw_vars.user_uniforms[i];
        }
        for i in 0..mapping.textures.len() {
            self.texture_slots[i] = draw_vars.texture_slots[i];
        }
        self.instance_dirty = true;
        self.uniforms_dirty = true;
        self.options = draw_vars.options.clone();
    }
    
    pub fn set_local_scroll(&mut self, scroll: Vec2, local_scroll: Vec2) {
        self.draw_uniforms.draw_scroll.x = scroll.x;
        if !self.options.no_h_scroll {
            self.draw_uniforms.draw_scroll.x += local_scroll.x;
        }
        self.draw_uniforms.draw_scroll.y = scroll.y;
        if !self.options.no_v_scroll {
            self.draw_uniforms.draw_scroll.y += local_scroll.y;
        }
        self.draw_uniforms.draw_scroll.z = local_scroll.x;
        self.draw_uniforms.draw_scroll.w = local_scroll.y;
    }
    
    pub fn get_local_scroll(&self) -> Vec4 {
        self.draw_uniforms.draw_scroll
    }
    
    pub fn set_zbias(&mut self, zbias: f32) {
        self.draw_uniforms.draw_zbias = zbias;
    }
    
    pub fn set_clip(&mut self, clip: (Vec2, Vec2)) {
        self.draw_uniforms.draw_clip_x1 = clip.0.x;
        self.draw_uniforms.draw_clip_y1 = clip.0.y;
        self.draw_uniforms.draw_clip_x2 = clip.1.x;
        self.draw_uniforms.draw_clip_y2 = clip.1.y;
    }
    
    pub fn clip_and_scroll_rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        let mut x1 = x - self.draw_uniforms.draw_scroll.x;
        let mut y1 = y - self.draw_uniforms.draw_scroll.y;
        let mut x2 = x1 + w;
        let mut y2 = y1 + h;
        x1 = self.draw_uniforms.draw_clip_x1.max(x1).min(self.draw_uniforms.draw_clip_x2);
        y1 = self.draw_uniforms.draw_clip_y1.max(y1).min(self.draw_uniforms.draw_clip_y2);
        x2 = self.draw_uniforms.draw_clip_x1.max(x2).min(self.draw_uniforms.draw_clip_x2);
        y2 = self.draw_uniforms.draw_clip_y1.max(y2).min(self.draw_uniforms.draw_clip_y2);
        return Rect {pos: vec2(x1, y1), size: vec2(x2 - x1, y2 - y1)};
    }
}

#[derive(Default, Clone)]
#[repr(C)]
pub struct ViewUniforms {
    view_transform: [f32; 16],
}

impl ViewUniforms {
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<ViewUniforms>()] {
        unsafe {std::mem::transmute(self)}
    }
}

#[derive(Clone)]
pub enum CxViewDebug {
    DrawTree,
    Instances
}

#[derive(Default)]
pub struct CxView {
    pub debug_id: LiveId,
    
    pub alloc_generation: u64,
    
    pub codeflow_parent_id: Option<usize>, // the id of the parent we nest in, codeflow wise
    
    pub redraw_id: u64,
    
    pub pass_id: usize,
    
    pub locked_view_transform: bool,
    pub no_v_scroll: bool, // this means we
    pub no_h_scroll: bool,
    pub parent_scroll: Vec2,
    pub unsnapped_scroll: Vec2,
    pub snapped_scroll: Vec2,
    
    pub draw_items: Vec<DrawItem>,
    pub draw_items_len: usize,
    
    pub view_uniforms: ViewUniforms,
    pub platform: CxPlatformView,
    
    pub rect: Rect,
    pub is_clipped: bool,
    pub is_overlay: bool,
    pub always_redraw: bool,
    
    pub layout: Layout,
    
    pub debug: Option<CxViewDebug>
}

impl CxView {
    pub fn new() -> Self {
        let mut ret = Self {
            is_clipped: true,
            no_v_scroll: false,
            no_h_scroll: false,
            ..Self::default()
        };
        ret.uniform_view_transform(&Mat4::identity());
        ret
    }
    
    pub fn initialize(&mut self, pass_id: usize, is_clipped: bool, redraw_id: u64) {
        self.is_clipped = is_clipped;
        self.redraw_id = redraw_id;
        self.pass_id = pass_id;
        self.uniform_view_transform(&Mat4::identity());
    }
    
    pub fn get_scrolled_rect(&self) -> Rect {
        Rect {
            pos: self.rect.pos + self.parent_scroll,
            size: self.rect.size
        }
    }
    
    pub fn get_inverse_scrolled_rect(&self) -> Rect {
        Rect {
            pos: self.rect.pos - self.parent_scroll,
            size: self.rect.size
        }
    }
    
    pub fn intersect_clip(&self, clip: (Vec2, Vec2)) -> (Vec2, Vec2) {
        if self.is_clipped {
            let min_x = self.rect.pos.x - self.parent_scroll.x;
            let min_y = self.rect.pos.y - self.parent_scroll.y;
            let max_x = self.rect.pos.x + self.rect.size.x - self.parent_scroll.x;
            let max_y = self.rect.pos.y + self.rect.size.y - self.parent_scroll.y;
            
            (Vec2 {
                x: min_x.max(clip.0.x),
                y: min_y.max(clip.0.y)
            }, Vec2 {
                x: max_x.min(clip.1.x),
                y: max_y.min(clip.1.y)
            })
        }
        else {
            clip
        }
    }
    
    pub fn find_appendable_drawcall(&mut self, sh: &CxDrawShader, draw_vars: &DrawVars) -> Option<usize> {
        // find our drawcall to append to the current layer
        if self.draw_items_len > 0 {
            for i in (0..self.draw_items_len).rev() {
                let draw_item = &mut self.draw_items[i];
                if let Some(draw_call) = &draw_item.draw_call {
                    if draw_item.sub_view_id.is_none() && draw_call.draw_shader == draw_vars.draw_shader.unwrap() {
                        // lets compare uniforms and textures..
                        if !sh.mapping.flags.draw_call_nocompare {
                            if draw_call.geometry_id != draw_vars.geometry_id {
                                continue
                            }
                            let mut diff = false;
                            for i in 0..sh.mapping.user_uniforms.total_slots {
                                if draw_call.user_uniforms[i] != draw_vars.user_uniforms[i] {
                                    diff = true;
                                    break;
                                }
                            }
                            if diff{continue}
                            for i in 0..sh.mapping.textures.len() {
                                if draw_call.texture_slots[i] != draw_vars.texture_slots[i] {
                                    diff = true;
                                    break;
                                }
                            }
                            if diff{continue}
                        }
                        if !draw_call.options.appendable_drawcall(&draw_vars.options) {
                            continue
                        }
                        return Some(i)
                    }
                }
            }
        }
        None
    }
    
    pub fn get_local_scroll(&self) -> Vec2 {
        let xs = if self.no_v_scroll {0.} else {self.snapped_scroll.x};
        let ys = if self.no_h_scroll {0.} else {self.snapped_scroll.y};
        Vec2 {x: xs, y: ys}
    }
    
    pub fn uniform_view_transform(&mut self, v: &Mat4) {
        //dump in uniforms
        for i in 0..16 {
            self.view_uniforms.view_transform[i] = v.v[i];
        }
    }
    
    pub fn get_view_transform(&self) -> Mat4 {
        //dump in uniforms
        let mut m = Mat4::default();
        for i in 0..16 {
            m.v[i] = self.view_uniforms.view_transform[i];
        }
        m
    }
}



