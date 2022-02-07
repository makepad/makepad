pub use {
    std::{
        rc::Rc,
        cell::RefCell
    },
    crate::{
        makepad_live_compiler::*,
        makepad_math::*,
        platform::{
            CxPlatformDrawCall,
            CxPlatformView,
        },
        cx::{
            Cx,
        },
        area::{Area, DrawListArea, InstanceArea},
        live_traits::*,
        draw_2d::turtle::{Layout, Width, Height, Walk, Rect},
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
    
    pub fn get_local_scroll(&self) -> Vec4 {
        self.draw_scroll
    }
    
    pub fn set_zbias(&mut self, zbias: f32) {
        self.draw_zbias = zbias;
    }
    
    pub fn set_clip(&mut self, clip: (Vec2, Vec2)) {
        self.draw_clip_x1 = clip.0.x;
        self.draw_clip_y1 = clip.0.y;
        self.draw_clip_x2 = clip.1.x;
        self.draw_clip_y2 = clip.1.y;
    }
    
    pub fn clip_and_scroll_rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        let mut x1 = x - self.draw_scroll.x;
        let mut y1 = y - self.draw_scroll.y;
        let mut x2 = x1 + w;
        let mut y2 = y1 + h;
        x1 = self.draw_clip_x1.max(x1).min(self.draw_clip_x2);
        y1 = self.draw_clip_y1.max(y1).min(self.draw_clip_y2);
        x2 = self.draw_clip_x1.max(x2).min(self.draw_clip_x2);
        y2 = self.draw_clip_y1.max(y2).min(self.draw_clip_y2);
        return Rect {pos: vec2(x1, y1), size: vec2(x2 - x1, y2 - y1)};
    }
    
    pub fn set_local_scroll(&mut self, scroll: Vec2, local_scroll: Vec2, options: &CxDrawShaderOptions) {
        self.draw_scroll.x = scroll.x;
        if !options.no_h_scroll {
            self.draw_scroll.x += local_scroll.x;
        }
        self.draw_scroll.y = scroll.y;
        if !options.no_v_scroll {
            self.draw_scroll.y += local_scroll.y;
        }
        self.draw_scroll.z = local_scroll.x;
        self.draw_scroll.w = local_scroll.y;
    }
    
}

pub struct DrawItem {
    pub draw_item_id: usize,
    pub draw_list_id: usize,
    pub redraw_id: u64,
    
    pub sub_view_id: Option<usize>,
    pub draw_call: Option<DrawCall>,
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
    
}

#[derive(Default, Clone)]
#[repr(C)]
pub struct DrawListUniforms {
    pub view_transform: [f32; 16],
}

impl DrawListUniforms {
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<DrawListUniforms>()] {
        unsafe {std::mem::transmute(self)}
    }
}

#[derive(Clone)]
pub enum DrawListDebug {
    DrawTree,
    Instances
}

#[derive(Default)]
pub struct DrawList {
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
    
    pub draw_list_uniforms: DrawListUniforms,
    pub platform: CxPlatformView,
    
    pub rect: Rect,
    pub is_clipped: bool,
    
    pub debug: Option<DrawListDebug>
}

impl DrawList {
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
                            if diff {continue}
                            for i in 0..sh.mapping.textures.len() {
                                if draw_call.texture_slots[i] != draw_vars.texture_slots[i] {
                                    diff = true;
                                    break;
                                }
                            }
                            if diff {continue}
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
            self.draw_list_uniforms.view_transform[i] = v.v[i];
        }
    }
    
    pub fn get_view_transform(&self) -> Mat4 {
        //dump in uniforms
        let mut m = Mat4::default();
        for i in 0..16 {
            m.v[i] = self.draw_list_uniforms.view_transform[i];
        }
        m
    }
}
