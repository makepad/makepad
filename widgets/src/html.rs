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
    import makepad_widgets::label::LabelBase;
    
    HtmlBase = {{Html}} {
        // ok so we can use one drawtext
        // change to italic, change bold (SDF), strikethrough

        Unsupported = <LabelBase> {
            draw_text: {
                color: #f00
            }
            text = "<Unsupported HTML tag>"
        }
    }
}

#[derive(Live, Widget)]
pub struct Html{
    #[deref] text_flow: TextFlow,
    #[live] html: Rc<String>,
    #[rust] doc: HtmlDoc
}

// alright lets parse the HTML
impl LiveHook for Html{
    fn after_apply_from(&mut self, _cx: &mut Cx, _apply:&mut Apply) {
        let mut errors = Some(Vec::new());
        self.doc = parse_html(&*self.html, &mut errors);
        if errors.as_ref().unwrap().len()>0{
            log!("HTML parser returned errors {:?}", errors)
        }
    }
    // hook the apply flow to add `Unsupported` to the templates in this Html `TextFlow`.
    fn apply_value_instance(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        self.text_flow.apply_value_instance(cx, apply, index, nodes)
    }
}
 
impl Widget for Html {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.text_flow.handle_event(cx, event, scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk:Walk)->DrawStep{
        let tf = &mut self.text_flow;
        tf.begin(cx, walk);
        let mut auto_id = 0;
        // alright lets iterate the html doc and draw it
        let mut node = self.doc.walk();
        while !node.empty(){
            
            match node.open_tag_lc(){
                some_id!(a)=>{
                    node = node.jump_to_close();
                }
                some_id!(h1) => open_header_tag(cx, tf, 2.0),
                some_id!(h2) => open_header_tag(cx, tf, 1.5),
                some_id!(h3) => open_header_tag(cx, tf, 1.17),
                some_id!(h4) => open_header_tag(cx, tf, 1.0),
                some_id!(h5) => open_header_tag(cx, tf, 0.83),
                some_id!(h6) => open_header_tag(cx, tf, 0.67),
                some_id!(br)=>cx.turtle_new_line(1.0),
                some_id!(b)=>tf.push_bold(),
                some_id!(strong)=>tf.push_strong(),
                some_id!(i)=>tf.push_italic(),
                some_id!(em)=>tf.push_emphasis(),
                some_id!(p)=> {
                    // there's probably a better way to do this by setting margins...
                    cx.turtle_new_line(2.0);
                }
                Some(_custom)=>{ // custom widget
                    let mut id_str = None;
                    let id = if let Some(id) = node.find_attr_lc(live_id!(id)){
                        id_str = Some(id.to_owned());
                        LiveId::from_str(id)
                    }
                    else{
                        auto_id += 1;
                        log!("Using auto_id {auto_id}");
                        LiveId(auto_id)
                    };
                    log!("_custom widget {_custom:?}, id_str {id_str:?}");
                    let template = node.open_tag_nc()
                        .expect(&format!("Failed to find template for HTML node {id_str:?}"));
                    if let Some(item) = tf.item(cx, id, template){
                        item.set_text(node.find_text().unwrap_or(""));
                        item.draw_all(cx, scope);
                    } else {
                        log!("Failed to create item for HTML node {id_str:?}, id {id:?}, template {template:?}");
                        let item = tf.item(cx, id, live_id!(Unsupported))
                            .expect("Failed to create Unsupported Html item");
                        item.set_text(node.find_text().unwrap_or(""));
                        let text = item.text();
                        log!("Unsupported item text: {:?}", text);
                        item.draw_all(cx, scope);
                        tf.draw_text(cx, text.trim());

                    }
                    node = node.jump_to_close();
                }
                _=>()
            } 
            match node.close_tag_lc(){
                some_id!(h1)
                | some_id!(h2)
                | some_id!(h3)
                | some_id!(h4)
                | some_id!(h5)
                | some_id!(h6) => {
                    tf.pop_size();
                    tf.pop_bold();
                    cx.turtle_new_line(1.0);
                }
                some_id!(b)=>tf.pop_bold(),
                some_id!(strong)=>tf.pop_strong(),
                some_id!(i)=>tf.pop_italic(),
                some_id!(em)=>tf.pop_emphasis(),
                some_id!(p)=> {
                    cx.turtle_new_line(2.0);
                }
                _=>()
            }
            if let Some(text) = node.text(){
                log!("Node text: {text}");
                tf.draw_text(cx, text.trim());
            }
            node = node.walk();
        }
        tf.end(cx);
        DrawStep::done()
    }  
     
    fn text(&self)->String{
        self.html.as_ref().to_string()
    } 
    
    fn set_text(&mut self, v:&str){
        self.html = Rc::new(v.to_string())
    }
} 

fn open_header_tag(cx: &mut Cx2d, tf: &mut TextFlow, scale: f64) {
    tf.push_bold();
    tf.push_scale(scale);
    cx.turtle_new_line(1.0 + (scale * 2.0));
}
