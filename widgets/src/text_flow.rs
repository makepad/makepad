use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
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
    #[live] line_color: Vec4,
    #[live] sep_color: Vec4,
    #[live] code_color: Vec4,
    #[live] quote_bg_color: Vec4,
    #[live] quote_fg_color: Vec4,
    #[live] block_type: FlowBlockType
}

#[derive(Default)]
pub struct StackCounter(usize);
impl StackCounter{
    pub fn push(&mut self){
        self.0 += 1;
    }
    pub fn pop(&mut self){
        if self.0 > 0{
            self.0 -=1;
        }
    }
    pub fn clear(&mut self){
        self.0 = 0
    }
    pub fn value(&self)->usize{
        self.0
    }
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
    
    #[rust] area_stack: SmallVec<[Area;4]>,
    #[rust] pub font_sizes: SmallVec<[f64;8]>,
   // #[rust] pub font: SmallVec<[Font;2]>,
    #[rust] pub top_drop: SmallVec<[f64;4]>,
    #[rust] pub combine_spaces: SmallVec<[bool;4]>,
    #[rust] pub ignore_newlines: SmallVec<[bool;4]>,
    #[rust] pub bold: StackCounter,
    #[rust] pub italic: StackCounter,
    #[rust] pub fixed: StackCounter,
    #[rust] pub underline: StackCounter,
    #[rust] pub strikethrough: StackCounter,
    #[rust] pub inline_code: StackCounter,
    
    #[rust] pub areas_tracker: RectAreasTracker,
    
    #[layout] layout: Layout,
    
    #[live] quote_layout: Layout,
    #[live] quote_walk: Walk,
    #[live] code_layout: Layout,
    #[live] code_walk: Walk,
    #[live] sep_walk: Walk, 
    #[live] list_item_layout: Layout,
    #[live] list_item_walk: Walk,
    #[live] inline_code_padding: Padding,
    #[live] inline_code_margin: Margin,
        
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
            ApplyFrom::NewFromDoc {file_id} | ApplyFrom::UpdateFromDoc {file_id,..} => {
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
pub struct RectAreasTracker{
    pub areas: SmallVec<[Area;4]>,
    pos: usize,
    stack: SmallVec<[usize;2]>,
}

impl RectAreasTracker{
    fn clear_stack(&mut self){
        self.pos = 0;
        self.stack.clear();
    }
    
    pub fn push_tracker(&mut self){
        self.stack.push(self.pos);
    }
    
    // this returns the range in the area vec    
    pub fn pop_tracker(&mut self)->(usize, usize){
        return (self.stack.pop().unwrap(), self.pos)
    }
    
    pub fn track_rect(&mut self, cx:&mut Cx2d, rect:Rect){
        if self.stack.len() >0{
            if self.pos >= self.areas.len(){
                self.areas.push(Area::Empty);
            }
            cx.add_aligned_rect_area(&mut self.areas[self.pos], rect);
            self.pos += 1;
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
        self.clear_stacks();
    }
    
    fn clear_stacks(&mut self){
        self.areas_tracker.clear_stack();
        self.bold.clear();
        self.italic.clear();
        self.fixed.clear();
        self.underline.clear();
        self.strikethrough.clear();
        self.inline_code.clear();
        //self.font.clear();
        self.font_sizes.clear();
        self.area_stack.clear();
        self.top_drop.clear();
        self.combine_spaces.clear();
        self.ignore_newlines.clear();
    }
    
        
    pub fn push_size_rel_scale(&mut self, scale: f64){
        self.font_sizes.push(
            self.font_sizes.last().unwrap_or(&self.font_size) * scale
        );
    }
            
    pub fn push_size_abs_scale(&mut self, scale: f64){
        self.font_sizes.push(
            self.font_size * scale
        );
    }
    
    pub fn end(&mut self, cx: &mut Cx2d){
        // lets end the turtle with how far we walked
        cx.end_turtle_with_area(&mut self.area);
        self.items.retain_visible();
    } 

    pub fn begin_code(&mut self, cx:&mut Cx2d){
        // alright we are going to push a block with a layout and a walk
        self.draw_block.block_type = FlowBlockType::Code;
        self.draw_block.begin(cx, self.code_walk, self.code_layout);
        self.area_stack.push(self.draw_block.draw_vars.area);
    }
    
    pub fn end_code(&mut self, cx:&mut Cx2d){
        self.draw_block.draw_vars.area = self.area_stack.pop().unwrap();
        self.draw_block.end(cx);
    }
    
    pub fn begin_list_item(&mut self, cx:&mut Cx2d, dot:&str, pad:f64){
        // alright we are going to push a block with a layout and a walk
        let fs = self.font_sizes.last().unwrap_or(&self.font_size);
        self.draw_normal.text_style.font_size = *fs;
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
        self.draw_block.draw_vars.area = self.area_stack.pop().unwrap();
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
        
    pub fn clear_items(&mut self){
        self.items.clear();
    }
        

    pub fn item_with_scope(&mut self, cx: &mut Cx, scope: &mut Scope, entry_id: LiveId, template: LiveId) -> Option<WidgetRef> {
        if let Some(ptr) = self.templates.get(&template) {
            let entry = self.items.get_or_insert(cx, (entry_id, template), | cx | {
                WidgetRef::new_from_ptr_with_scope(cx, scope, Some(*ptr))
            });
            return Some(entry.clone())
        }
        None 
    }
     
    pub fn draw_text(&mut self, cx:&mut Cx2d, text:&str){
        if let Some(DrawState::Drawing) = self.draw_state.get(){
            
            let dt = if self.fixed.value() > 0{
                &mut self.draw_fixed
            }
            else if self.bold.value() > 0{
                if self.italic.value() > 0{
                    &mut self.draw_bold_italic
                }
                else{
                    &mut self.draw_bold
                }
            }
            else if self.italic.value() > 0{
                    &mut self.draw_italic
            }
            else{
                &mut self.draw_normal
            };

            let font_size = self.font_sizes.last().unwrap_or(&self.font_size);
            dt.text_style.top_drop = *self.top_drop.last().unwrap_or(&1.2);
            dt.text_style.font_size = *font_size;
            dt.ignore_newlines = *self.ignore_newlines.last().unwrap_or(&true);
            dt.combine_spaces = *self.combine_spaces.last().unwrap_or(&true);
            //if let Some(font) = self.font
            // the turtle is at pos X so we walk it.
           
            let areas_tracker = &mut self.areas_tracker;
            if self.inline_code.value() > 0{
                let db = &mut self.draw_block;
                db.block_type = FlowBlockType::InlineCode;
                let rect = cx.walk_turtle(Walk{
                    width: Size::Fixed(self.inline_code_margin.left),
                    height: Size::Fixed(0.0),
                    ..Default::default()
                });
                areas_tracker.track_rect(cx, rect);
                dt.draw_walk_resumable_with(cx, text, |cx, mut rect|{
                    rect.pos -= self.inline_code_padding.left_top();
                    rect.size += self.inline_code_padding.size();
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                });
                let rect = cx.walk_turtle(Walk{
                    width:Size::Fixed(self.inline_code_margin.right),
                    height:Size::Fixed(0.0),
                    ..Default::default()
                });
                areas_tracker.track_rect(cx, rect);
            }
            else if self.strikethrough.value() > 0{
                let db = &mut self.draw_block;
                db.block_type = FlowBlockType::Strikethrough;
                dt.draw_walk_resumable_with(cx, text, |cx, rect|{
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                });
            }
            else if self.underline.value() > 0{
                let db = &mut self.draw_block;
                db.block_type = FlowBlockType::Underline;
                dt.draw_walk_resumable_with(cx, text, |cx, rect|{
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                });
            }
            else{
                dt.draw_walk_resumable_with(cx, text, |cx, rect|{
                    areas_tracker.track_rect(cx, rect);
                });
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
