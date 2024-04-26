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
    #[live] paragraph_spacing: f64,
    #[rust] doc: MarkdownDoc
}

// alright lets parse the HTML
impl LiveHook for Markdown{
    fn after_apply_from(&mut self, _cx: &mut Cx, _apply:&mut Apply) {
        let new_doc = parse_markdown(&*self.body);
        if new_doc != self.doc{
            self.doc = new_doc;
            self.text_flow.clear_items();
        }
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
                    tf.push_size_abs_scale(4.5 / *level as f64);
                    tf.bold.push();
                },
                MarkdownNode::Separator=>{
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    tf.sep(cx);
                }
                MarkdownNode::EndHead=>{
                    tf.bold.pop();
                    tf.font_sizes.pop();
                    cx.turtle_new_line();
                },
                MarkdownNode::NewLine=>{
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                },
                MarkdownNode::BeginNormal=>{
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                },
                MarkdownNode::EndNormal=>{
                    
                },
                MarkdownNode::BeginListItem{label}=>{
                    cx.turtle_new_line();
                    let str = match label{
                        MarkdownListLabel::Plus=>"+",
                        MarkdownListLabel::Minus=>"-",
                        MarkdownListLabel::Star=>"*",
                        MarkdownListLabel::Number{start,end,..}=>{
                            &self.doc.decoded[*start..*end]
                        }
                                                
                    };
                    tf.begin_list_item(cx, str, 1.5);
                },
                MarkdownNode::EndListItem=>{
                    tf.end_list_item(cx);
                },
                MarkdownNode::Link{start, url_start, end}=>{
                    tf.draw_text(cx, "Link[name:");
                    tf.draw_text(cx, &self.doc.decoded[*start..*url_start]);
                    tf.draw_text(cx, ", url:");
                    tf.draw_text(cx, &self.doc.decoded[*url_start..*end]);
                    tf.draw_text(cx, " ]");
                },
                MarkdownNode::Image{start, url_start, end}=>{
                    tf.draw_text(cx, "Image[name:");
                    tf.draw_text(cx, &self.doc.decoded[*start..*url_start]);
                    tf.draw_text(cx, ", url:");
                    tf.draw_text(cx, &self.doc.decoded[*url_start..*end]);
                    tf.draw_text(cx, " ]");
                },
                MarkdownNode::BeginQuote=>{
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    tf.begin_quote(cx);
                },
                MarkdownNode::EndQuote=>{
                    tf.end_quote(cx);
                },
                MarkdownNode::BeginUnderline=>{
                    tf.underline.push();
                },
                MarkdownNode::EndUnderline=>{
                    tf.underline.pop();
                },
                MarkdownNode::BeginInlineCode=>{
                    tf.fixed.push();
                    tf.inline_code.push();     
                },
                MarkdownNode::EndInlineCode=>{
                    tf.fixed.pop();
                    tf.inline_code.pop();                 
                },
                MarkdownNode::BeginCode=>{
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    tf.combine_spaces.push(false);
                    tf.fixed.push();

                    // This adjustment is necesary to do not add too much spacing
                    // between lines inside the code block.
                    tf.top_drop.push(0.2);

                    tf.begin_code(cx);
                },
                MarkdownNode::EndCode=>{
                    tf.top_drop.pop();
                    tf.fixed.pop();
                    tf.combine_spaces.pop();
                    tf.end_code(cx);
                },
                MarkdownNode::BeginBold=>{
                    tf.bold.push();
                },
                MarkdownNode::BeginItalic=>{
                    tf.italic.push();
                },
                MarkdownNode::EndBold=>{
                    tf.bold.pop();          
                },
                MarkdownNode::EndItalic=>{
                    tf.italic.pop();
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
        self.body = Rc::new(v.to_string());

        let new_doc = parse_markdown(&*self.body);
        if new_doc != self.doc{
            self.doc = new_doc;
            self.text_flow.clear_items();
        }
    }
}

impl MarkdownRef {
    pub fn set_text(&mut self, v:&str) {
        let Some(mut inner) = self.borrow_mut() else { return };
        inner.set_text(v)
    }
}
 