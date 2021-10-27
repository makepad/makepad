use crate::cx::*;
use std::collections::HashMap;
use makepad_shader_compiler::shaderast::DrawShaderDef;
use makepad_shader_compiler::shaderast::DrawShaderPtr;
use makepad_shader_compiler::shaderast::DrawShaderConstTable;
use makepad_live_parser::Span;

#[derive(Clone)]
pub struct ViewTexture {
    sample_count: usize,
    has_depth_stencil: bool,
    fixed_size: Option<Vec2>
}

pub type ViewRedraw = Result<(), ()>;

#[derive(Debug, Clone)]
pub struct View { // draw info per UI element
    pub view_id: Option<usize>,
    pub redraw_id: u64,
    pub is_clipped: bool,
    pub is_overlay: bool, // this view is an overlay, rendered last
    pub always_redraw: bool,
}

impl View {
    pub fn proto_overlay(_cx: &mut Cx) -> Self {
        Self {
            redraw_id: 0,
            is_clipped: true,
            is_overlay: true,
            always_redraw: false,
            view_id: None,
        }
    }
    
    pub fn new() -> Self {
        Self {
            redraw_id: 0,
            is_clipped: true,
            is_overlay: false,
            always_redraw: false,
            view_id: None,
        }
    }
    
    pub fn with_is_clipped(self, is_clipped: bool) -> Self {Self {is_clipped, ..self}}
    pub fn with_is_overlay(self, is_overlay: bool) -> Self {Self {is_overlay, ..self}}
    pub fn with_always_redraw(self, always_redraw: bool) -> Self {Self {always_redraw, ..self}}
    
    pub fn lock_view_transform(&self, cx: &mut Cx, mat: &Mat4) {
        if let Some(view_id) = self.view_id {
            let cxview = &mut cx.views[view_id];
            cxview.uniform_view_transform(mat);
            return cxview.locked_view_transform = true;
        }
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
        
        if let Some(view_id) = &self.view_id { // we need a draw_list_id
            set_view_transform_recur(*view_id, cx, mat);
        }
        
    }
    
    pub fn begin_view(&mut self, cx: &mut Cx, layout: Layout) -> ViewRedraw {
        
        if !cx.in_redraw_cycle {
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
                cx.views.push(CxView {do_v_scroll: true, do_h_scroll: true, ..Default::default()});
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
            if is_root_for_pass {
                view_id
            }
            else if let Some(last_view_id) = cx.view_stack.last() {
                *last_view_id
            }
            else { // we have no parent
                view_id
            }
        };
        
        // push ourselves up the parent draw_stack
        if view_id != parent_view_id {
            // copy the view transform
            
            if !cx.views[view_id].locked_view_transform {
                for i in 0..16 {
                    cx.views[view_id].view_uniforms.view_transform[i] =
                    cx.views[parent_view_id].view_uniforms.view_transform[i];
                }
            }
            
            // we need a new draw
            let parent_cxview = &mut cx.views[parent_view_id];
            
            let id = parent_cxview.draw_items_len;
            parent_cxview.draw_items_len = parent_cxview.draw_items_len + 1;
            
            // see if we need to add a new one
            if parent_cxview.draw_items_len > parent_cxview.draw_items.len() {
                parent_cxview.draw_items.push({
                    DrawItem {
                        view_id: parent_view_id,
                        draw_item_id: parent_cxview.draw_items.len(),
                        redraw_id: cx.redraw_id,
                        sub_view_id: Some(view_id),
                        draw_call: None
                    }
                })
            }
            else { // or reuse a sub list node
                let draw_item = &mut parent_cxview.draw_items[id];
                draw_item.sub_view_id = Some(view_id);
                draw_item.redraw_id = cx.redraw_id;
            }
        }
        
        // set nesting draw list id for incremental repaint scanning
        cx.views[view_id].nesting_view_id = nesting_view_id;
        
        if !self.always_redraw && cx.views[view_id].draw_items_len != 0 && !cx.view_will_redraw(view_id) {
            
            // walk the turtle because we aren't drawing
            let w = Width::Fix(cx.views[view_id].rect.size.x);
            let h = Height::Fix(cx.views[view_id].rect.size.y);
            cx.walk_turtle(Walk {width: w, height: h, margin: override_layout.walk.margin});
            return Err(());
        }
        
        // prepare drawlist for drawing
        let cxview = &mut cx.views[view_id];
        
        // update drawlist ids
        let last_redraw_id = cxview.redraw_id;
        self.redraw_id = cx.redraw_id;
        cxview.redraw_id = cx.redraw_id;
        cxview.draw_items_len = 0;
        
        cx.view_stack.push(view_id);
        
        let old_area = Area::View(ViewArea {view_id: view_id, redraw_id: last_redraw_id});
        let new_area = Area::View(ViewArea {view_id: view_id, redraw_id: cx.redraw_id});
        cx.update_area_refs(old_area, new_area);
        
        cx.begin_turtle(override_layout, new_area);
        
        if is_root_for_pass {
            cx.passes[pass_id].paint_dirty = true;
        }
        
        Ok(())
    }
    
    pub fn view_will_redraw(&self, cx: &mut Cx) -> bool {
        if let Some(view_id) = self.view_id {
            cx.view_will_redraw(view_id)
        }
        else {
            true
        }
    }
    
    pub fn end_view(&mut self, cx: &mut Cx) -> Area {
        let view_id = self.view_id.unwrap();
        let view_area = Area::View(ViewArea {view_id: view_id, redraw_id: cx.redraw_id});
        let rect = cx.end_turtle(view_area);
        let cxview = &mut cx.views[view_id];
        cxview.rect = rect;
        cx.view_stack.pop();
        view_area
    }
    
    pub fn get_rect(&self, cx: &Cx) -> Rect {
        if let Some(view_id) = self.view_id {
            let cxview = &cx.views[view_id];
            return cxview.rect
        }
        Rect::default()
    }
    
    pub fn get_view_transform(&self, cx: &Cx) -> Mat4 {
        if let Some(view_id) = self.view_id {
            let cxview = &cx.views[view_id];
            return cxview.get_view_transform()
        }
        Mat4::default()
    }
    
    
    pub fn set_view_debug(&self, cx: &mut Cx, view_debug: CxViewDebug) {
        if let Some(view_id) = self.view_id {
            let cxview = &mut cx.views[view_id];
            cxview.debug = Some(view_debug);
        }
    }
    
    pub fn redraw_view(&self, cx: &mut Cx) {
        if let Some(view_id) = self.view_id {
            let cxview = &cx.views[view_id];
            let area = Area::View(ViewArea {view_id: view_id, redraw_id: cxview.redraw_id});
            cx.redraw_child_area(area);
        }
        else {
            cx.redraw_child_area(Area::All)
        }
    }
    
    pub fn redraw_view_parent(&self, cx: &mut Cx) {
        if let Some(view_id) = self.view_id {
            let cxview = &cx.views[view_id];
            let area = Area::View(ViewArea {view_id: view_id, redraw_id: cxview.redraw_id});
            cx.redraw_parent_area(area);
        }
        else {
            cx.redraw_parent_area(Area::All)
        }
    }
    
    pub fn area(&self) -> Area {
        if let Some(view_id) = self.view_id {
            Area::View(ViewArea {view_id: view_id, redraw_id: self.redraw_id})
        }
        else {
            Area::Empty
        }
    }
}


impl Cx {
    
    pub fn new_draw_call(&mut self, draw_call_vars: &DrawCallVars) -> &mut DrawItem {
        return self.get_draw_call(false, draw_call_vars);
    }
    
    pub fn append_to_draw_call(&mut self, draw_call_vars: &DrawCallVars) -> &mut DrawItem {
        return self.get_draw_call(true, draw_call_vars);
    }
    
    pub fn get_draw_call(&mut self, append: bool, draw_call_vars: &DrawCallVars) -> &mut DrawItem {
        let sh = &self.draw_shaders[draw_call_vars.draw_shader.unwrap().draw_shader_id];
        
        let current_view_id = *self.view_stack.last().unwrap();
        let cxview = &mut self.views[current_view_id];
        let draw_item_id = cxview.draw_items_len;
        
        if append && !sh.mapping.flags.draw_call_always {
            if let Some(index) = cxview.find_appendable_drawcall(sh, draw_call_vars) {
                return &mut cxview.draw_items[index];
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
                draw_call: Some(DrawCall::new(&sh.mapping, draw_call_vars))
            });
            return &mut cxview.draw_items[draw_item_id];
        }
        // reuse an older one, keeping all GPU resources attached
        let mut draw_item = &mut cxview.draw_items[draw_item_id];
        draw_item.sub_view_id = None;
        draw_item.redraw_id = self.redraw_id;
        if let Some(dc) = &mut draw_item.draw_call {
            dc.update(&sh.mapping, draw_call_vars);
        }
        else {
            draw_item.draw_call = Some(DrawCall::new(&sh.mapping, draw_call_vars))
        }
        return draw_item;
    }
    
    pub fn begin_many_instances(&mut self, draw_call_vars: &DrawCallVars) -> ManyInstances {
        let draw_item = self.append_to_draw_call(draw_call_vars);
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        let mut instances = Vec::new();
        if draw_call.in_many_instances {
            panic!("please call end_many_instances before calling begin_many_instances again")
        }
        draw_call.in_many_instances = true;
        std::mem::swap(&mut instances, &mut draw_call.instances);
        ManyInstances {
            instance_area: InstanceArea {
                view_id: draw_item.view_id,
                draw_item_id: draw_item.draw_item_id,
                instance_count: 0,
                instance_offset: instances.len(),
                redraw_id: draw_item.redraw_id
            },
            aligned: None,
            instances
        }
    }
    
    pub fn begin_many_aligned_instances(&mut self, draw_call_vars: &DrawCallVars) -> ManyInstances {
        let mut li = self.begin_many_instances(draw_call_vars);
        li.aligned = Some(self.align_list.len());
        self.align_list.push(Area::Empty);
        li
    }
    
    pub fn end_many_instances(&mut self, mut many_instances: ManyInstances) -> Area {
        let mut ia = many_instances.instance_area;
        let cxview = &mut self.views[ia.view_id];
        let draw_item = &mut cxview.draw_items[ia.draw_item_id];
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        
        if !draw_call.in_many_instances {
            panic!("please call begin_many_instances before calling end_many_instances")
        }
        draw_call.in_many_instances = false;
        std::mem::swap(&mut many_instances.instances, &mut draw_call.instances);
        ia.instance_count = (draw_call.instances.len() - ia.instance_offset) / draw_call.total_instance_slots;
        if let Some(aligned) = many_instances.aligned {
            self.align_list[aligned] = ia.clone().into();
        }
        ia.into()
    }
    
    pub fn add_instance(&mut self, draw_call_vars: &DrawCallVars) -> Area {
        let data = draw_call_vars.instance_slice();
        let draw_item = self.append_to_draw_call(draw_call_vars);
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
            instance_offset: draw_call.instances.len(),
            redraw_id: draw_item.redraw_id
        };
        draw_call.instances.extend_from_slice(data);
        ia.into()
    }
    
    pub fn add_aligned_instance(&mut self, draw_call_vars: &DrawCallVars) -> Area {
        let data = draw_call_vars.instance_slice();
        let draw_item = self.append_to_draw_call(draw_call_vars);
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        let instance_count = data.len() / draw_call.total_instance_slots;
        let check = data.len() % draw_call.total_instance_slots;
        if check > 0 {
            panic!("Data not multiple of total slots");
        }
        let ia: Area = (InstanceArea {
            view_id: draw_item.view_id,
            draw_item_id: draw_item.draw_item_id,
            instance_count: instance_count,
            instance_offset: draw_call.instances.len(),
            redraw_id: draw_item.redraw_id
        }).into();
        draw_call.instances.extend_from_slice(data);
        self.align_list.push(ia.clone());
        ia
    }
    
    pub fn set_view_scroll_x(&mut self, view_id: usize, scroll_pos: f32) {
        let fac = self.get_delegated_dpi_factor(self.views[view_id].pass_id);
        let cxview = &mut self.views[view_id];
        cxview.unsnapped_scroll.x = scroll_pos;
        let snapped = scroll_pos - scroll_pos % (1.0 / fac);
        if cxview.snapped_scroll.x != snapped {
            cxview.snapped_scroll.x = snapped;
            self.passes[cxview.pass_id].paint_dirty = true;
        }
    }
    
    
    pub fn set_view_scroll_y(&mut self, view_id: usize, scroll_pos: f32) {
        let fac = self.get_delegated_dpi_factor(self.views[view_id].pass_id);
        let cxview = &mut self.views[view_id];
        cxview.unsnapped_scroll.y = scroll_pos;
        let snapped = scroll_pos - scroll_pos % (1.0 / fac);
        if cxview.snapped_scroll.y != snapped {
            cxview.snapped_scroll.y = snapped;
            self.passes[cxview.pass_id].paint_dirty = true;
        }
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
    pub draw_scroll_x: f32,
    pub draw_scroll_y: f32,
    pub draw_scroll_z: f32,
    pub draw_scroll_w: f32,
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

#[derive(Clone)]
pub struct DrawItem {
    pub draw_item_id: usize,
    pub view_id: usize,
    pub redraw_id: u64,
    
    pub sub_view_id: Option<usize>,
    pub draw_call: Option<DrawCall>,
}

pub struct UserUniforms {
    
}

#[derive(Clone)]
pub struct DrawCall {
    pub draw_shader: DrawShader, // if shader_id changed, delete gl vao
    
    pub in_many_instances: bool,
    pub instances: Vec<f32>,
    pub total_instance_slots: usize,
    //pub current_instance_offset: usize, // offset of current instance
    
    pub draw_uniforms: DrawUniforms, // draw uniforms
    pub geometry: Option<Geometry>,
    pub user_uniforms: [f32; DRAW_CALL_USER_UNIFORMS], // user uniforms
    
    pub do_v_scroll: bool,
    pub do_h_scroll: bool,
    
    pub texture_slots: [Option<Texture>; DRAW_CALL_TEXTURE_SLOTS],
    pub instance_dirty: bool,
    pub uniforms_dirty: bool,
    pub platform: CxPlatformDrawCall
}

impl DrawCall {
    
    pub fn new(mapping: &CxDrawShaderMapping, draw_call_vars: &DrawCallVars) -> Self {
        DrawCall {
            geometry: draw_call_vars.geometry,
            do_h_scroll: true,
            do_v_scroll: true,
            in_many_instances: false,
            draw_shader: draw_call_vars.draw_shader.unwrap(),
            instances: Vec::new(),
            total_instance_slots: mapping.instances.total_slots,
            draw_uniforms: DrawUniforms::default(),
            user_uniforms: draw_call_vars.user_uniforms,
            texture_slots: draw_call_vars.texture_slots,
            //current_instance_offset: 0,
            instance_dirty: true,
            uniforms_dirty: true,
            platform: CxPlatformDrawCall::default()
        }
    }
    
    pub fn update(&mut self, mapping: &CxDrawShaderMapping, draw_call_vars: &DrawCallVars) {
        self.draw_shader = draw_call_vars.draw_shader.unwrap();
        self.geometry = draw_call_vars.geometry;
        self.instances.truncate(0);
        self.total_instance_slots = mapping.instances.total_slots;
        for i in 0..mapping.user_uniforms.total_slots {
            self.user_uniforms[i] = draw_call_vars.user_uniforms[i];
        }
        for i in 0..mapping.textures.len() {
            self.texture_slots[i] = draw_call_vars.texture_slots[i];
        }
        self.instance_dirty = true;
        self.uniforms_dirty = true;
        self.do_h_scroll = true;
        self.do_v_scroll = true;
    }
    
    pub fn set_local_scroll(&mut self, scroll: Vec2, local_scroll: Vec2) {
        self.draw_uniforms.draw_scroll_x = scroll.x;
        if self.do_h_scroll {
            self.draw_uniforms.draw_scroll_x += local_scroll.x;
        }
        self.draw_uniforms.draw_scroll_y = scroll.y;
        if self.do_v_scroll {
            self.draw_uniforms.draw_scroll_y += local_scroll.y;
        }
        self.draw_uniforms.draw_scroll_z = local_scroll.x;
        self.draw_uniforms.draw_scroll_w = local_scroll.y;
    }
    
    pub fn get_local_scroll(&self) -> Vec4 {
        Vec4 {
            x: self.draw_uniforms.draw_scroll_x,
            y: self.draw_uniforms.draw_scroll_y,
            z: self.draw_uniforms.draw_scroll_z,
            w: self.draw_uniforms.draw_scroll_w
        }
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
    /*
    pub fn into_area(&self) -> Area {
        Area::Instance(InstanceArea {
            view_id: self.view_id,
            draw_call_id: self.draw_call_id,
            redraw_id: self.redraw_id,
            instance_offset: 0,
            instance_count: 0
        })
    }*/
    /*
    pub fn get_current_instance_area(&self, instance_count: usize) -> InstanceArea {
        InstanceArea {
            view_id: self.view_id,
            draw_call_id: self.draw_call_id,
            redraw_id: self.redraw_id,
            instance_offset: self.current_instance_offset,
            instance_count: instance_count
        }
    }*/
    
    pub fn clip_and_scroll_rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        let mut x1 = x - self.draw_uniforms.draw_scroll_x;
        let mut y1 = y - self.draw_uniforms.draw_scroll_y;
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

#[derive(Default, Clone)]
pub struct CxView {
    pub nesting_view_id: usize, // the id of the parent we nest in, codeflow wise
    pub redraw_id: u64,
    pub pass_id: usize,
    pub locked_view_transform: bool,
    pub do_v_scroll: bool, // this means we
    pub do_h_scroll: bool,
    pub draw_items: Vec<DrawItem>,
    pub draw_items_len: usize,
    pub parent_scroll: Vec2,
    pub view_uniforms: ViewUniforms,
    pub unsnapped_scroll: Vec2,
    pub snapped_scroll: Vec2,
    pub platform: CxPlatformView,
    pub rect: Rect,
    pub clipped: bool,
    pub debug: Option<CxViewDebug>
}

impl CxView {
    pub fn initialize(&mut self, pass_id: usize, clipped: bool, redraw_id: u64) {
        self.clipped = clipped;
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
        if self.clipped {
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
    
    pub fn find_appendable_drawcall(&mut self, sh: &CxDrawShader, draw_call_vars: &DrawCallVars) -> Option<usize> {
        // find our drawcall to append to the current layer
        if self.draw_items_len > 0 {
            for i in (0..self.draw_items_len).rev() {
                let draw_item = &mut self.draw_items[i];
                if let Some(draw_call) = &draw_item.draw_call {
                    if draw_item.sub_view_id.is_none() && draw_call.draw_shader == draw_call_vars.draw_shader.unwrap() {
                        // lets compare uniforms and textures..
                        if sh.mapping.flags.draw_call_compare {
                            if draw_call.geometry != draw_call_vars.geometry {
                                return None
                            }
                            for i in 0..sh.mapping.user_uniforms.total_slots {
                                if draw_call.user_uniforms[i] != draw_call_vars.user_uniforms[i] {
                                    return None
                                }
                            }
                            for i in 0..sh.mapping.textures.len() {
                                if draw_call.texture_slots[i] != draw_call_vars.texture_slots[i] {
                                    return None
                                }
                            }
                        }
                        return Some(i)
                    }
                }
            }
        }
        None
    }
    /*
    pub fn set_clipping_uniforms(&mut self) {
        if self.clipped {
           self.uniform_view_clip(self.rect.x, self.rect.y, self.rect.x + self.rect.w, self.rect.y + self.rect.h);
        }
        else {
            self.uniform_view_clip(-50000.0, -50000.0, 50000.0, 50000.0);
        }
    }*/
    
    pub fn get_local_scroll(&self) -> Vec2 {
        let xs = if self.do_v_scroll {self.snapped_scroll.x}else {0.};
        let ys = if self.do_h_scroll {self.snapped_scroll.y}else {0.};
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


const DRAW_CALL_USER_UNIFORMS: usize = 32;
const DRAW_CALL_TEXTURE_SLOTS: usize = 16;
const DRAW_CALL_VAR_INSTANCES: usize = 32;

#[cfg(any(target_os = "linux", target_arch = "wasm32", test))]
const DRAW_SHADER_INPUT_PACKING: DrawShaderInputPacking = DrawShaderInputPacking::UniformGLSL;
#[cfg(any(target_os = "macos", test))]
const DRAW_SHADER_INPUT_PACKING: DrawShaderInputPacking = DrawShaderInputPacking::UniformsMetal;
#[cfg(any(target_os = "windows", test))]
const DRAW_SHADER_INPUT_PACKING: DrawShaderInputPacking = DrawShaderInputPacking::UniformsHLSL;


#[derive(Default)]
pub struct DrawCallVars {
    pub var_instance_start: usize,
    pub var_instance_slots: usize,
    pub draw_shader: Option<DrawShader>,
    pub geometry: Option<Geometry>,
    pub user_uniforms: [f32; DRAW_CALL_USER_UNIFORMS],
    pub texture_slots: [Option<Texture>; DRAW_CALL_TEXTURE_SLOTS],
    pub var_instances: [f32; DRAW_CALL_VAR_INSTANCES]
}

impl DrawCallVars {
    pub fn instance_slice<'a>(&'a self) -> &'a [f32] {
        unsafe {
            std::slice::from_raw_parts((&self.var_instances[self.var_instance_start - 1] as *const _ as *const f32).offset(1), self.var_instance_slots)
        }
    }
    
    pub fn init_shader(&mut self, cx: &mut Cx, draw_shader_ptr: DrawShaderPtr, geometry_fields: &dyn GeometryFields) {
        // lets first fetch the shader from live_ptr
        // if it doesn't exist, we should allocate and
        if let Some(draw_shader_id) = cx.draw_shader_ptr_to_id.get(&draw_shader_ptr) {
            self.draw_shader = Some(DrawShader {
                draw_shader_ptr,
                draw_shader_id: *draw_shader_id
            });
        }
        else {
            fn live_type_to_shader_ty(live_type: LiveType) -> Option<Ty> {
                if live_type == f32::live_type() {Some(Ty::Float)}
                else if live_type == Vec2::live_type() {Some(Ty::Vec2)}
                else if live_type == Vec3::live_type() {Some(Ty::Vec3)}
                else if live_type == Vec4::live_type() {Some(Ty::Vec4)}
                else {None}
            }
            // ok ! we have to compile it
            let live_factories = &cx.live_factories;
            let result = cx.shader_registry.analyse_draw_shader(draw_shader_ptr, | span, id, live_type, draw_shader_def | {
                if id == id!(rust_type) {
                    fn recur_expand(live_type: LiveType, live_factories: &HashMap<LiveType, Box<dyn LiveFactory >>, draw_shader_def: &mut DrawShaderDef, span: Span) {
                        if let Some(lf) = live_factories.get(&live_type) {
                            
                            let mut fields = Vec::new();
                            
                            lf.live_fields(&mut fields);
                            let mut slots = 0;
                            for field in fields {
                                if field.id == id!(deref_target) {
                                    recur_expand(field.live_type, live_factories, draw_shader_def, span);
                                    continue
                                }
                                // lets count sizes
                                if let Some(ty) = live_type_to_shader_ty(field.live_type) {
                                    slots += ty.slots();
                                    draw_shader_def.add_instance(field.id, ty, span);
                                };
                            }
                            // insert padding
                            if slots%2 == 1{
                                draw_shader_def.add_instance(Id(0), Ty::Float, span);
                            }
                        }
                    }
                    recur_expand(live_type, live_factories, draw_shader_def, span);
                }
                if id == id!(geometry) {
                    if let Some(lf) = live_factories.get(&live_type) {
                        if lf.live_type() == geometry_fields.live_type_check() {
                            let mut fields = Vec::new();
                            geometry_fields.geometry_fields(&mut fields);
                            for field in fields {
                                draw_shader_def.add_geometry(field.id, field.ty, span);
                            }
                        }
                        else {
                            eprintln!("lf.get_type() != geometry_fields.live_type_check()");
                        }
                    }
                }
            });
            // ok lets print an error
            match result {
                Err(e) => {
                    println!("Error {}", e.to_live_file_error("", ""));
                }
                Ok(draw_shader_def) => {
                    // OK! SO the shader parsed
                    let draw_shader_id = cx.draw_shaders.len();
                    
                    let const_table = DrawShaderConstTable::default();
                    //let mut const_table = cx.shader_registry.compute_const_table(&draw_shader_def, NONE)
                    
                    let mut mapping = CxDrawShaderMapping::from_draw_shader_def(
                        draw_shader_def,
                        const_table,
                        DRAW_SHADER_INPUT_PACKING
                    );
                    mapping.update_live_uniforms(&cx.shader_registry.live_registry);
                    
                    cx.draw_shaders.push(CxDrawShader {
                        name: "todo".to_string(),
                        platform: None,
                        mapping: mapping
                    });
                    // ok so. maybe we should fill the live_uniforms buffer?
                    
                    cx.draw_shader_ptr_to_id.insert(draw_shader_ptr, draw_shader_id);
                    cx.draw_shader_compile_set.insert(draw_shader_ptr);
                    // now we simply queue it somewhere somehow to compile.
                    self.draw_shader = Some(DrawShader {
                        draw_shader_id,
                        draw_shader_ptr
                    });
                    
                    self.geometry = Some(geometry_fields.get_geometry());
                    // also we should allocate it a Shader object
                }
            }
        }
    }
    
    pub fn update_var(&mut self, cx: &mut Cx, value_ptr: LivePtr, id: Id) {
        fn store_values(cx: &Cx, draw_shader: DrawShader, id: Id, values: &[f32], draw_call_vars: &mut DrawCallVars) {
            let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
            for input in &sh.mapping.user_uniforms.inputs {
                if input.id == id {
                    if values.len() == input.slots {
                        for i in 0..input.slots {
                            draw_call_vars.user_uniforms[input.offset + i] = values[i];
                        }
                    }
                }
            }
            for input in &sh.mapping.var_instances.inputs {
                if input.id == id {
                    if values.len() == input.slots {
                        for i in 0..input.slots {
                            let index = draw_call_vars.var_instances.len() - sh.mapping.var_instances.total_slots + input.offset + i;
                            draw_call_vars.var_instances[input.offset + index] = values[i];
                        }
                    }
                }
            }
        }
        if let Some(draw_shader) = self.draw_shader {
            let node = cx.shader_registry.live_registry.resolve_ptr(value_ptr);
            match node.value {
                LiveValue::Int(val) => {
                    store_values(&cx, draw_shader, id, &[val as f32], self);
                }
                LiveValue::Float(val) => {
                    store_values(&cx, draw_shader, id, &[val as f32], self);
                }
                LiveValue::Color(val) => {
                    let val = Vec4::from_u32(val);
                    store_values(&cx, draw_shader, id, &[val.x, val.y, val.z, val.w], self);
                }
                LiveValue::Vec2(val) => {
                    store_values(&cx, draw_shader, id, &[val.x, val.y], self);
                }
                LiveValue::Vec3(val) => {
                    store_values(&cx, draw_shader, id, &[val.x, val.y, val.z], self);
                }
                _ => ()
            }
        }
    }
    
    pub fn init_slicer(
        &mut self,
        cx: &mut Cx,
    ) {
        if let Some(draw_shader) = self.draw_shader {
            let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
            self.var_instance_start = self.var_instances.len() - sh.mapping.var_instances.total_slots;
            self.var_instance_slots = sh.mapping.instances.total_slots;
        }
    }
}
