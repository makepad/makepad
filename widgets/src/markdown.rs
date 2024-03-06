use {
    crate::{
        makepad_markdown::*,
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        text_flow::TextFlow,
    },
    std::rc::Rc,
};

live_design!{
    MarkdownBase = {{Markdown}} {
        // ok so we can use one drawtext
        // change to italic, change bold (SDF), strikethrough
    }
}

#[derive(Live, Widget)]
pub struct Markdown{
    #[deref] text_flow: TextFlow,
    #[live] body: Rc<String>,
    #[rust] doc: MarkdownDoc
}

// alright lets parse the HTML
impl LiveHook for Markdown{
    fn after_apply_from(&mut self, _cx: &mut Cx, _apply:&mut Apply) {
       self.doc = parse_markdown(&*self.body);
    }
}
 
impl Widget for Markdown {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.text_flow.handle_event(cx, event, scope);
    } 
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk:Walk)->DrawStep{
        let tf = &mut self.text_flow;
        tf.begin(cx, walk);
        // alright lets walk the markdown
        for node in &self.doc.nodes{
            match node{ 
                MarkdownNode::BeginHead{level}=>{
                    tf.push_size_abs_scale(3.0 / *level as f64);
                },
                MarkdownNode::EndHead=>{
                    tf.pop_size();
                    cx.turtle_new_line();
                },
                MarkdownNode::BeginNormal=>{
                    
                },
                MarkdownNode::EndNormal=>{
                    cx.turtle_new_line();
                },
                MarkdownNode::BeginItem{count:_}=>{
                    
                },
                MarkdownNode::EndItem=>{
                    
                },
                MarkdownNode::Link{name_start:_, name_end:_, url_start:_, url_end:_}=>{
                    
                },
                MarkdownNode::Image{name_start:_, name_end:_, url_start:_, url_end:_}=>{
                    
                },
                MarkdownNode::BeginQuote=>{
                    tf.begin_quote(cx);
                },
                MarkdownNode::EndQuote=>{
                    tf.end_quote(cx);
                },
                MarkdownNode::BeginInlineCode=>{
                    tf.push_fixed();
                    tf.begin_inline_code(cx);     
                },
                MarkdownNode::EndInlineCode=>{
                    tf.pop_fixed();
                    tf.end_inline_code(cx);                 
                },
                MarkdownNode::BeginCode=>{
                    tf.push_fixed();
                    tf.begin_code(cx);     
                },
                MarkdownNode::EndCode=>{
                    tf.pop_fixed();
                    tf.end_code(cx);
                    cx.turtle_new_line();
                },
                MarkdownNode::BeginBold=>{
                    tf.push_bold();
                },
                MarkdownNode::BeginItalic=>{
                    tf.push_italic();
                },
                MarkdownNode::EndBold=>{
                    tf.pop_bold();          
                },
                MarkdownNode::EndItalic=>{
                    tf.pop_italic();
                },
                MarkdownNode::Text{start, end}=>{
                    tf.draw_text(cx, &self.doc.decoded[*start..*end]);
                }
            }
        }
        
        tf.end(cx);
        DrawStep::done()
    }
     
    fn text(&self)->String{
        self.body.as_ref().to_string()
    } 
    
    fn set_text(&mut self, v:&str){
        self.body = Rc::new(v.to_string())
    }
} 
 