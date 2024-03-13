use {
    crate::{
        makepad_html::*,
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        text_flow::TextFlow,
    },
    std::rc::Rc,
};

live_design!{
    HtmlBase = {{Html}} {
        // ok so we can use one drawtext
        // change to italic, change bold (SDF), strikethrough
    }
}

#[derive(Live, Widget)]
pub struct Html{
    #[deref] pub text_flow: TextFlow,
    #[live] pub body: Rc<String>,
    #[rust] pub doc: HtmlDoc
}

// alright lets parse the HTML
impl LiveHook for Html{
    fn after_apply_from(&mut self, _cx: &mut Cx, _apply:&mut Apply) {
        let mut errors = Some(Vec::new());
        self.doc = parse_html(&*self.body, &mut errors);
        if errors.as_ref().unwrap().len()>0{
            log!("HTML parser returned errors {:?}", errors)
        }
    }
}

impl Html{
    pub fn handle_custom_widget(cx: &mut Cx2d, scope:&mut Scope, tf:&mut TextFlow, node:&mut HtmlWalker, auto_id: &mut u64){
        let id = if let Some(id) = node.find_attr_lc(live_id!(id)){
            LiveId::from_str(id)
        } 
        else{
            *auto_id += 1;
            LiveId(*auto_id) 
        }; 
        let template = node.open_tag_nc().unwrap();
        if let Some(item) = tf.item(cx, id, template){
            item.set_text(node.find_text().unwrap_or(""));
            item.draw_all(cx, scope);
        }
        *node = node.jump_to_close();
    }
    
    pub fn handle_open_tag(cx: &mut Cx2d, tf:&mut TextFlow, node:&mut HtmlWalker)->Option<LiveId>{
        match node.open_tag_lc(){
            some_id!(a)=>{
                log!("{:?}", node.find_attr_lc(live_id!(href)));
                log!("{:?}", node.find_text());
                *node = node.jump_to_close();
            }
            some_id!(h1)=>{
                tf.push_size_abs_scale(1.5)
            },
            some_id!(code)=>{
                tf.push_fixed();
                tf.begin_code(cx);
            } 
            some_id!(block_quote)=>tf.begin_quote(cx),
            some_id!(br)=>cx.turtle_new_line(),
            some_id!(sep)=>tf.sep(cx),
            some_id!(u)=>tf.push_underline(),
            some_id!(s)=>tf.push_strikethrough(),
            some_id!(b)=>tf.push_bold(),
            some_id!(i)=>tf.push_italic(),
            some_id!(li)=>tf.begin_list_item(cx,"\u{2022}",1.0),
            Some(x)=>{return Some(x)}
            _=>()
        } 
        None
    }
    
    pub fn handle_close_tag(cx: &mut Cx2d, tf:&mut TextFlow, node:&mut HtmlWalker)->Option<LiveId>{
        match node.close_tag_lc(){
            some_id!(block_quote)=>tf.end_quote(cx),
            some_id!(code)=>{
                tf.pop_fixed();
                tf.end_code(cx); 
            }
            some_id!(li)=>tf.end_list_item(cx),
            some_id!(h1)=>tf.pop_size(),
            some_id!(b)=>tf.pop_bold(),
            some_id!(u)=>tf.pop_underline(),
            some_id!(s)=>tf.pop_strikethrough(),
            some_id!(i)=>tf.pop_italic(),
            Some(x)=>{return Some(x)}
            _=>()
        }
        None
    }
    
    pub fn handle_text_node(cx: &mut Cx2d, tf:&mut TextFlow, node:&mut HtmlWalker)->bool{
        if let Some(text) = node.text(){
            tf.draw_text(cx, text);
            true
        }
        else{
            false
        }
    }
}

impl Widget for Html {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.text_flow.handle_event(cx, event, scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk:Walk)->DrawStep{
        let tf = &mut self.text_flow;
        tf.begin(cx, walk); 
        // alright lets iterate the html doc and draw it
        let mut node = self.doc.walk();
        let mut auto_id = 0;
        while !node.empty(){
            match Self::handle_open_tag(cx, tf, &mut node){
                Some(_)=>{
                    Self::handle_custom_widget(cx, scope, tf, &mut node, &mut auto_id); 
                }
                _=>()
            }
            match Self::handle_close_tag(cx, tf, &mut  node){
                _=>()
            }
            Self::handle_text_node(cx, tf, &mut node);
            node = node.walk();
        }
        tf.end(cx);
        DrawStep::done()
    }  
     
    fn text(&self)->String{
        self.body.as_ref().to_string()
    }
    
    fn set_text(&mut self, v:&str){
        self.body = Rc::new(v.to_string());
        let mut errors = Some(Vec::new());
        self.doc = parse_html(&*self.body, &mut errors);
        if errors.as_ref().unwrap().len()>0{
            log!("HTML parser returned errors {:?}", errors)
        }
    }
} 
 