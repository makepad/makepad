use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    },
}; 
   
live_design!{
    TextFlowBase = {{TextFlow}} {
        // ok so we can use one drawtext
        // change to italic, change bold (SDF), strikethrough
        font_size: 8,
        flow: RightWrap
    }
}
   
// this widget has a retained and an immediate mode api
#[derive(Live, Widget)]
pub struct TextFlow {
    #[live] draw_normal: DrawText,
    #[live] draw_italic: DrawText,
    #[live] draw_bold: DrawText,
    #[live] draw_bold_italic: DrawText,
    #[live] font_size: f64,
    #[walk] walk: Walk,
    #[rust] bold_counter: usize,
    #[rust] italic_counter: usize,
    #[rust] font_size_stack: FontSizeStack,
    #[layout] layout: Layout,
    #[redraw] #[rust] area:Area,
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] items: ComponentMap<(u64,LiveId), WidgetRef>,
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
            scope.with_id(LiveId(*id), |scope| {
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
    
    pub fn push_scale(&mut self, scale: f64){
        self.font_size_stack.push(
            self.font_size_stack.value(self.font_size) * scale
        );
    }
    
    pub fn pop_size(&mut self){
        self.font_size_stack.pop();
    }
    
    
    pub fn item(&mut self, cx: &mut Cx, entry_id: u64, template: LiveId) -> Option<WidgetRef> {
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
            let dt = if self.bold_counter > 0{
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
            };
            let fs = self.font_size_stack.value(self.font_size);
            dt.text_style.font_size = fs;
            // the turtle is at pos X so we walk it.
            dt.draw_walk_word(cx, text);
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
