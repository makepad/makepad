use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    },
}; 
    
live_design!{
    DrawFlowBlock = {{DrawFlowBlock}} {}
    TextFlowBase = {{TextFlow}} {
        // ok so we can use one drawtext
        // change to italic, change bold (SDF), strikethrough
        font_size: 8,
        flow: RightWrap,
    }
}  

#[derive(Live, LiveHook)]
#[live_ignore]
#[repr(u32)]
pub enum FlowBlockType {
    #[pick] Quote = shader_enum(1),
    Sep = shader_enum(2),
    Code = shader_enum(3),
    InlineCode = shader_enum(4),
    Underline = shader_enum(5),
    Strikethrough = shader_enum(6)
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawFlowBlock {
    #[deref] draw_super: DrawQuad,
    #[live] block_type: FlowBlockType
} 

      
// this widget has a retained and an immediate mode api
#[derive(Live, Widget)]
pub struct TextFlow {
    #[live] draw_normal: DrawText,
    #[live] draw_italic: DrawText,
    #[live] draw_bold: DrawText,
    #[live] draw_bold_italic: DrawText,
    #[live] draw_fixed: DrawText,
    
    #[live] draw_block: DrawFlowBlock,
    
    #[live] font_size: f64,
    #[walk] walk: Walk,
    
    #[rust] bold_counter: usize,
    #[rust] italic_counter: usize,
    #[rust] fixed_counter: usize,
    #[rust] underline_counter: usize,
    #[rust] strikethrough_counter: usize,
    
    #[rust] font_size_stack: FontSizeStack,
    #[rust] area_stack: AreaStack,
    #[layout] layout: Layout,
    
    #[live] quote_layout: Layout,
    #[live] quote_walk: Walk,
    #[live] code_layout: Layout,
    #[live] code_walk: Walk,
    #[live] sep_walk: Walk, 
    #[live] list_item_layout: Layout,
    #[live] list_item_walk: Walk,
    #[live] inline_code_layout: Layout,
    #[live] inline_code_walk: Walk,
    
    #[redraw] #[rust] area:Area,
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] items: ComponentMap<(LiveId,LiveId), WidgetRef>,
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
}

impl LiveHook for TextFlow{
    // hook the apply flow to collect our templates and apply to instanced childnodes
    fn apply_value_instance(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        let id = nodes[index].id;
        match apply.from {
            ApplyFrom::NewFromDoc {file_id} | ApplyFrom::UpdateFromDoc {file_id} => {
                if nodes[index].origin.has_prop_type(LivePropType::Instance) {
                    let live_ptr = cx.live_registry.borrow().file_id_index_to_live_ptr(file_id, index);
                    self.templates.insert(id, live_ptr);
                    // lets apply this thing over all our childnodes with that template
                    for ((_, templ_id), node) in self.items.iter_mut() {
                        if *templ_id == id {
                            node.apply(cx, apply, index, nodes);
                        }
                    }
                }
                else {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                }
            }
            _ => ()
        }
        nodes.skip_node(index)
    }
} 

#[derive(Default)]
struct AreaStack{
    pub array_len: usize,
    pub stack_array: [Area;2],
    pub stack_vec: Vec<Area>
}

impl AreaStack{
    fn push(&mut self, v:Area){
        if self.array_len < self.stack_array.len(){
            self.stack_array[self.array_len] = v;
            self.array_len += 1;
        }
        else{
            self.stack_vec.push(v);
        }
    }
    fn pop(&mut self)->Area{
        if self.stack_vec.len()>0{
            self.stack_vec.pop().unwrap()
        }
        else if self.array_len > 0{
            self.array_len -=1;
            self.stack_array[self.array_len]
        }
        else{
            panic!()
        }
    }
    
    fn clear(&mut self){
        self.stack_vec.clear();
        self.array_len = 0;
    }
}


#[derive(Default)]
struct FontSizeStack{
    pub array_len: usize,
    pub stack_array: [f64;8],
    pub stack_vec: Vec<f64>
}

impl FontSizeStack{
    fn value(&self, def:f64)->f64{
        if self.stack_vec.len()>0{
            *self.stack_vec.last().unwrap()
        }
        else if self.array_len > 0{
            self.stack_array[self.array_len - 1]
        }
        else{
            def
        }
    }
    
    fn clear(&mut self){
        self.stack_vec.clear();
        self.array_len = 0;
    }
    
    fn push(&mut self, v:f64){
        if self.array_len < self.stack_array.len(){
            self.stack_array[self.array_len] = v;
            self.array_len += 1;
        }
        else{
            self.stack_vec.push(v);
        }
    }
    fn pop(&mut self){
        if self.stack_vec.len()>0{
            self.stack_vec.pop();
        }
        else if self.array_len > 0{
            self.array_len -=1
        }
    }
}

#[derive(Clone)]
enum DrawState {
    Begin,
    Drawing,
}

impl Widget for TextFlow {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk:Walk)->DrawStep{
        //self.draw_text.draw_walk(cx, walk.with_add_padding(self.padding), self.align, self.text.as_ref());
        if self.draw_state.begin(cx, DrawState::Begin) {
            self.begin(cx, walk);
            return DrawStep::make_step()
        }
        if let Some(_) = self.draw_state.get() {
            self.end(cx);
            self.draw_state.end();
        }
        DrawStep::done()
    }
    
    fn text(&self)->String{
        "".into()
        //self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, _v:&str){
        //self.text.as_mut_empty().push_str(v);
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        for ((id,_),entry) in self.items.iter_mut(){
            scope.with_id(*id, |scope| {
                entry.handle_event(cx, event, scope);
            });
            entry.handle_event(cx, event, scope);
        }
    }
}

impl TextFlow{
    pub fn begin(&mut self, cx: &mut Cx2d, walk:Walk){
        // lets begin a turtle 
        // well know if we have a known width to wrap
        // if we dont we just dont wrap
        cx.begin_turtle(walk, self.layout);
        self.draw_state.set(DrawState::Drawing);
        self.draw_block.append_to_draw_call(cx);
        self.font_size_stack.clear();
        self.area_stack.clear();
        self.bold_counter = 0;
        self.italic_counter = 0;
        self.fixed_counter = 0;
        self.underline_counter = 0;
        self.strikethrough_counter = 0;
    }
    
    pub fn end(&mut self, cx: &mut Cx2d){
        // lets end the turtle with how far we walked
        cx.end_turtle_with_area(&mut self.area);
        self.items.retain_visible();
    } 
    
    pub fn push_bold(&mut self){
        self.bold_counter += 1;
    }
    
    pub fn pop_bold(&mut self){
        if self.bold_counter>0{
            self.bold_counter -= 1;
        }
    } 
    
    pub fn push_underline(&mut self){
        self.underline_counter += 1;
    }
        
    pub fn pop_underline(&mut self){
        if self.underline_counter>0{
            self.underline_counter -= 1;
        }
    } 
    
    pub fn push_strikethrough(&mut self){
        self.strikethrough_counter += 1;
    }
            
    pub fn pop_strikethrough(&mut self){
        if self.strikethrough_counter>0{
            self.strikethrough_counter -= 1;
        }
    } 
    
    pub fn push_fixed(&mut self){
        self.fixed_counter += 1;
    }
        
    pub fn pop_fixed(&mut self){
        if self.fixed_counter>0{
            self.fixed_counter -= 1;
        }
    } 
    
    pub fn push_italic(&mut self){
        self.italic_counter += 1;
    }
    
    pub fn pop_italic(&mut self){
        if self.italic_counter>0{
            self.italic_counter -= 1;
        }
    }
    
    pub fn push_size(&mut self, size: f64){
        self.font_size_stack.push(size);
    }
    
    pub fn pop_size(&mut self){
        self.font_size_stack.pop();
    }
    
    pub fn push_size_rel_scale(&mut self, scale: f64){
        self.font_size_stack.push(
            self.font_size_stack.value(self.font_size) * scale
        );
    }
    
    pub fn push_size_abs_scale(&mut self, scale: f64){
        self.font_size_stack.push(
            self.font_size * scale
        );
    }
    

    
    pub fn begin_code(&mut self, cx:&mut Cx2d){
        // alright we are going to push a block with a layout and a walk
        self.draw_block.block_type = FlowBlockType::Code;
        self.draw_block.begin(cx, self.code_walk, self.code_layout);
        self.area_stack.push(self.draw_block.draw_vars.area);
    }
    
    pub fn end_code(&mut self, cx:&mut Cx2d){
        self.draw_block.draw_vars.area = self.area_stack.pop();
        self.draw_block.end(cx);
    }
    
    pub fn begin_inline_code(&mut self, cx:&mut Cx2d){
        // alright we are going to push a block with a layout and a walk
        self.draw_block.block_type = FlowBlockType::InlineCode;
        self.draw_block.begin(cx, self.inline_code_walk, self.inline_code_layout);
        self.area_stack.push(self.draw_block.draw_vars.area);
    } 
        
    pub fn end_inline_code(&mut self, cx:&mut Cx2d){
        self.draw_block.draw_vars.area = self.area_stack.pop();
        self.draw_block.end(cx);
    }
      
    pub fn begin_list_item(&mut self, cx:&mut Cx2d, dot:&str, pad:f64){
        // alright we are going to push a block with a layout and a walk
        let pad = self.draw_normal.get_font_size() * pad;
        cx.begin_turtle(self.list_item_walk, Layout{
            padding:Padding{
                left: self.list_item_layout.padding.left + pad,
                ..self.list_item_layout.padding
            },
            ..self.list_item_layout
        });
        // lets draw the 'marker' at -x 
        // lets get the turtle position and abs draw 
        
        let pos = cx.turtle().pos() - dvec2(pad,0.0);
        let fs = self.font_size_stack.value(self.font_size);
        self.draw_normal.text_style.font_size = fs;
        self.draw_normal.draw_abs(cx, pos, dot);
        
        self.area_stack.push(self.draw_block.draw_vars.area);
    }
    
    pub fn end_list_item(&mut self, cx:&mut Cx2d){
        cx.end_turtle();
    }
    
    pub fn sep(&mut self, cx:&mut Cx2d){
        self.draw_block.block_type = FlowBlockType::Sep;
        self.draw_block.draw_walk(cx, self.sep_walk);
    }
    
    pub fn begin_quote(&mut self, cx:&mut Cx2d){
        // alright we are going to push a block with a layout and a walk
        self.draw_block.block_type = FlowBlockType::Quote;
        self.draw_block.begin(cx, self.quote_walk, self.quote_layout);
        self.area_stack.push(self.draw_block.draw_vars.area);
    }
        
    pub fn end_quote(&mut self, cx:&mut Cx2d){
        self.draw_block.draw_vars.area = self.area_stack.pop();
        self.draw_block.end(cx);
    }
    
    pub fn item(&mut self, cx: &mut Cx, entry_id: LiveId, template: LiveId) -> Option<WidgetRef> {
        if let Some(ptr) = self.templates.get(&template) {
            let entry = self.items.get_or_insert(cx, (entry_id, template), | cx | {
                WidgetRef::new_from_ptr(cx, Some(*ptr))
            });
            return Some(entry.clone())
        }
        None 
    }
     
    pub fn draw_text(&mut self, cx:&mut Cx2d, text:&str){
        if let Some(DrawState::Drawing) = self.draw_state.get(){
            
            let dt = if self.fixed_counter > 0{
                &mut self.draw_fixed
            }
            else {
                if self.bold_counter > 0{
                    if self.italic_counter > 0{
                        &mut self.draw_bold_italic
                    }
                    else{
                        &mut self.draw_bold
                    }
                }
                else{
                    if self.italic_counter>0{
                        &mut self.draw_italic
                    }
                    else{
                        &mut self.draw_normal
                    }
                }
            };
            let fs = self.font_size_stack.value(self.font_size);
            dt.text_style.font_size = fs;
            // the turtle is at pos X so we walk it.
            if self.strikethrough_counter > 0{
                let db = &mut self.draw_block;
                db.block_type = FlowBlockType::Strikethrough;
                dt.draw_walk_word_with(cx, text, |cx, rect|{
                    db.draw_abs(cx, rect);
                });
            }
            else if self.underline_counter > 0{
                let db = &mut self.draw_block;
                db.block_type = FlowBlockType::Underline;
                dt.draw_walk_word_with(cx, text, |cx, rect|{
                    db.draw_abs(cx, rect);
                });
            }
            else{
                dt.draw_walk_word(cx, text);
            }
        }
    }
}


/*
#[derive(Clone)]
pub struct HtmlId(pub HtmlString);
impl HtmlId{
    pub fn new(rc:&Rc<String>, start:usize, end:usize)->Self{
        Self(HtmlString(rc.clone(), start, end))
    }
    pub fn as_id(&self)->LiveId{
        LiveId::from_str(&self.0.0[self.0.1..self.0.2])
    }
}

impl fmt::Debug for HtmlId{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}
*/

// HTML Dom tree-flat vector
