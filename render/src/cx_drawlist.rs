use crate::cx::*;

impl Cx{

    pub fn new_instance_layer(&mut self, shader_id:usize, instance_count:usize)->InstanceArea{
        let shc = &self.compiled_shaders[shader_id];

        let current_draw_list_id = *self.draw_list_stack.last().unwrap();

        let draw_list = &mut self.draw_lists[current_draw_list_id];
        // we need a new draw call
        let draw_call_id = draw_list.draw_calls_len;
        draw_list.draw_calls_len = draw_list.draw_calls_len + 1;
        
        // see if we need to add a new one
        if draw_call_id >= draw_list.draw_calls.len(){
            draw_list.draw_calls.push(DrawCall{
                draw_call_id:draw_call_id,
                draw_list_id:current_draw_list_id,
                redraw_id:self.redraw_id,
                sub_list_id:0,
                shader_id:shc.shader_id,
                uniforms_required:shc.named_uniform_props.total_slots,
                instance:Vec::new(),
                uniforms:Vec::new(),
                textures_2d:Vec::new(),
                current_instance_offset:0,
                instance_dirty:true,
                uniforms_dirty:true,
                platform:DrawCallPlatform{..Default::default()}
            });
            let dc = &mut draw_list.draw_calls[draw_call_id];
            return dc.get_current_instance_area(instance_count);
        }

        // reuse a draw
        let dc = &mut draw_list.draw_calls[draw_call_id];
        dc.shader_id = shc.shader_id;
        dc.uniforms_required = shc.named_uniform_props.total_slots;
        dc.sub_list_id = 0; // make sure its recognised as a draw call
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

    pub fn new_instance(&mut self, shader_id:usize, instance_count:usize)->InstanceArea{
        if !self.is_in_redraw_cycle{
            panic!("calling get_instance outside of redraw cycle is not possible!");
        }
        let current_draw_list_id = *self.draw_list_stack.last().unwrap();
        let draw_list = &mut self.draw_lists[current_draw_list_id];
        let shc = &self.compiled_shaders[shader_id];

        // find our drawcall to append to the current layer
        if draw_list.draw_calls_len > 0{
            for i in (0..draw_list.draw_calls_len).rev(){
                let dc = &mut draw_list.draw_calls[i];
                if dc.sub_list_id == 0 && dc.shader_id == shc.shader_id{
                    // reuse this drawcmd and add an instance
                    dc.current_instance_offset = dc.instance.len();
                    let slot_align = dc.instance.len() % shc.instance_slots;
                    if slot_align != 0{
                        panic!("Instance offset disaligned! shader: {} misalign: {} slots: {}", shader_id, slot_align, shc.instance_slots);
                    }
                    return dc.get_current_instance_area(instance_count);
                }
            }
        }

        self.new_instance_layer(shader_id,instance_count)
    }

    pub fn new_aligned_instance(&mut self, shader_id:usize, instance_count:usize)->AlignedInstance{
        let instance_area = self.new_instance(shader_id, instance_count);
        
        let align_index = self.align_list.len();
        self.align_list.push(Area::Instance(instance_area.clone()));
        AlignedInstance{
            inst:instance_area,
            index:align_index
        }
    }

    pub fn update_aligned_instance_count(&mut self,align:&AlignedInstance){
        if let Area::Instance(instance) = &mut self.align_list[align.index]{
            instance.instance_count = align.inst.instance_count;
        }
    }
}


#[derive(Clone)]
pub struct AlignedInstance{
    pub inst:InstanceArea,
    pub index:usize
}

#[derive(Default,Clone)]
pub struct DrawCall{
    pub draw_call_id:usize,
    pub draw_list_id:usize,
    pub redraw_id:u64,
    pub sub_list_id:usize, // if not 0, its a subnode
    pub shader_id:usize, // if shader_id changed, delete gl vao
    pub instance:Vec<f32>,
    pub current_instance_offset:usize, // offset of current instance
    pub uniforms:Vec<f32>,  // draw uniforms
    pub uniforms_required:usize,
    pub textures_2d:Vec<u32>,
    pub instance_dirty:bool,
    pub uniforms_dirty:bool,
    pub platform:DrawCallPlatform
}

impl DrawCall{
    pub fn need_uniforms_now(&self) ->bool{
        self.uniforms.len() < self.uniforms_required
    }

    pub fn get_current_instance_area(&self, instance_count:usize)->InstanceArea{
        InstanceArea{
            draw_list_id:self.draw_list_id,
            draw_call_id:self.draw_call_id,
            redraw_id:self.redraw_id,
            instance_offset:self.current_instance_offset,
            instance_count:instance_count
        }
    }
}

// CX and DL uniforms
const DL_UNI_SCROLL:usize = 0;
const DL_UNI_DPI_FACTOR:usize = 2;
const DL_UNI_DPI_DILATE:usize = 3;
const DL_UNI_CLIP:usize = 4;
const DL_UNI_SIZE:usize = 8;

#[derive(Default,Clone)]
pub struct DrawList{
    pub nesting_draw_list_id:usize, // the id of the parent we nest in, codeflow wise
    pub redraw_id:u64,
    pub draw_calls:Vec<DrawCall>,
    pub draw_calls_len: usize,
    pub uniforms:Vec<f32>, // cmdlist uniforms
    pub platform:DrawListPlatform,
    pub rect:Rect,
    pub clipped:bool
}

impl DrawList{
    pub fn initialize(&mut self, clipped:bool, redraw_id:u64){
        self.clipped = clipped;
        self.redraw_id = redraw_id;
        self.uniforms.resize(DL_UNI_SIZE, 0.0);
    }

    pub fn set_clipping_uniforms(&mut self){
        if self.clipped{
            //println!("SET CLIPPING {} {} {} {} {}", self.draw_list_id, self.rect.x, self.rect.y, self.rect.x+self.rect.w, self.rect.y+self.rect.h);
            self.uniform_draw_list_clip(self.rect.x, self.rect.y, self.rect.x+self.rect.w, self.rect.y+self.rect.h);
        }
        else{
            self.uniform_draw_list_clip(-50000.0,-50000.0,50000.0,50000.0);
        }
    }

    pub fn def_uniforms(sh:&mut Shader){
        sh.add_ast(shader_ast!({
            let draw_list_scroll:vec2<UniformDl>;
            let draw_list_clip:vec4<UniformDl>;
            let dpi_dilate:float<UniformDl>;
        }));
    }

    pub fn set_dpi_factor(&mut self, dpi_factor:f32){
        let dpi_dilate = (2. - dpi_factor).max(0.).min(1.);
        self.uniforms[DL_UNI_DPI_FACTOR+0] = dpi_factor;
        self.uniforms[DL_UNI_DPI_DILATE+0] = dpi_dilate;
    }
    
    pub fn set_scroll_x(&mut self, x:f32){
        self.uniforms[DL_UNI_SCROLL+0] = x;
    }

    pub fn set_scroll_y(&mut self, y:f32){
        self.uniforms[DL_UNI_SCROLL+1] = y;
    }

    pub fn get_scroll_pos(&self)->Vec2{
        return Vec2{x:self.uniforms[DL_UNI_SCROLL+0], y:self.uniforms[DL_UNI_SCROLL+1]}
    }

    pub fn clip_and_scroll_rect(&self, x:f32, y:f32, w:f32, h:f32)->Rect{
        let mut x1 = x - self.uniforms[DL_UNI_SCROLL+0];
        let mut y1 = y - self.uniforms[DL_UNI_SCROLL+1];
        let mut x2 = x1 + w;
        let mut y2 = y1 + h; 
        let min_x = self.uniforms[DL_UNI_CLIP+0];
        let min_y = self.uniforms[DL_UNI_CLIP+1];
        let max_x = self.uniforms[DL_UNI_CLIP+2];
        let max_y = self.uniforms[DL_UNI_CLIP+3];
        x1 = min_x.max(x1).min(max_x);
        y1 = min_y.max(y1).min(max_y);
        x2 = min_x.max(x2).min(max_x);
        y2 = min_y.max(y2).min(max_y);
        return Rect{x:x1, y:y1, w:x2-x1, h:y2-y1};
    }

    pub fn uniform_draw_list_clip(&mut self, min_x:f32, min_y:f32, max_x:f32, max_y:f32){
        
        self.uniforms[DL_UNI_CLIP+0] = min_x;
        self.uniforms[DL_UNI_CLIP+1] = min_y;
        self.uniforms[DL_UNI_CLIP+2] = max_x;
        self.uniforms[DL_UNI_CLIP+3] = max_y;
    }
}
