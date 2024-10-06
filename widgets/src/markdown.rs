use crate::{
    makepad_markdown::*,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    text_flow::TextFlow,
    link_label::LinkLabel,
    WidgetMatchEvent,
};

live_design!{
    MarkdownLinkBase = {{MarkdownLink}} {
        link = {
            draw_text:{
                // other blue hyperlink colors: #1a0dab, // #0969da  // #0c50d1
                color: #1a0dab
            }
        }
    }

    MarkdownBase = {{Markdown}} {
        // ok so we can use one drawtext
        // change to italic, change bold (SDF), strikethrough
    }
} 

#[derive(Live, Widget)]
pub struct Markdown{
    #[deref] text_flow: TextFlow,
    #[live] body: ArcStringMut,
    #[live] paragraph_spacing: f64,
    #[live] pre_code_spacing: f64,
    #[live(false)] use_code_block_widget:bool,
    #[rust] in_code_block: bool,
    #[rust] code_block_string: String,
    #[rust] doc: MarkdownDoc,
    #[rust] auto_id: u64
}

// alright lets parse the HTML
impl LiveHook for Markdown{
    fn after_apply_from(&mut self, _cx: &mut Cx, _apply:&mut Apply) {
        self.parse_text();
    }
}
 
impl Widget for Markdown {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.text_flow.handle_event(cx, event, scope);
    } 
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk:Walk)->DrawStep{
        self.auto_id = 0;
        self.begin(cx, walk);
        let mut doc = MarkdownDoc::default();
        std::mem::swap(&mut doc, &mut self.doc);
        self.process_markdown_doc(&mut doc, cx);
        std::mem::swap(&mut doc, &mut self.doc);
        self.end(cx);
        DrawStep::done()
    }
     
    fn text(&self)->String{
        self.body.as_ref().to_string()
    } 
    
    fn set_text(&mut self, v:&str){
        if self.body.as_ref() != v{
            self.body.set(v);
            self.parse_text();
        }
    }
}

impl Markdown {
    
    fn parse_text(&mut self) {
        let new_doc = parse_markdown(self.body.as_ref());
        if new_doc != self.doc{
            self.doc = new_doc;
            //self.text_flow.clear_items();
        }
    }
    
    fn process_markdown_doc(&mut self, doc:&MarkdownDoc, cx: &mut Cx2d){
        let tf = &mut self.text_flow;
        for node in &doc.nodes{
            match node{
                MarkdownNode::BeginHead{level}=>{
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
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
                MarkdownNode::NewLine{paragraph: true}=>{
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                },
                MarkdownNode::NewLine{paragraph: false}=>{
                    if self.in_code_block{
                        self.code_block_string.push_str("\n");
                    }
                    else{
                        cx.turtle_new_line();
                    }
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
                            &doc.decoded[*start..*end]
                        }
                                                                        
                    };
                    tf.begin_list_item(cx, str, 1.5);
                },
                MarkdownNode::EndListItem=>{
                    tf.end_list_item(cx);
                },
                MarkdownNode::Link{start, url_start, end}=>{
                    self.auto_id += 1;
                    let item = tf.item(cx, LiveId(self.auto_id), live_id!(link));
                    item.set_text(&doc.decoded[*start..*url_start]);
                    item.as_markdown_link()
                    .set_href(&doc.decoded[*url_start..*end]);
                    item.draw_all_unscoped(cx);
                },
                MarkdownNode::Image{start, url_start, end}=>{
                    tf.draw_text(cx, "Image[name:");
                    tf.draw_text(cx, &doc.decoded[*start..*url_start]);
                    tf.draw_text(cx, ", url:");
                    tf.draw_text(cx, &doc.decoded[*url_start..*end]);
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
                    const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                    tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                    tf.fixed.push();
                    tf.inline_code.push();     
                },
                MarkdownNode::EndInlineCode=>{
                    tf.font_sizes.pop();
                    tf.fixed.pop();
                    tf.inline_code.pop();                 
                },
                MarkdownNode::BeginCode{lang_start:_, lang_end:_}=>{
                    if self.use_code_block_widget{
                        self.in_code_block = true;
                        self.code_block_string.clear();
                        cx.turtle_new_line_with_spacing(self.pre_code_spacing);
                    }
                    else{
                        const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                        tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                        // alright lets check if we need to use a widget
                        cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                        tf.combine_spaces.push(false);
                        tf.fixed.push();
                                
                        // This adjustment is necesary to do not add too much spacing
                        // between lines inside the code block.
                        // tf.top_drop.push(0.2);
                                
                        tf.begin_code(cx);
                    }
                },
                MarkdownNode::EndCode=>{
                    if self.in_code_block{
                        self.in_code_block = false;
                        let entry_id = tf.new_counted_id();
                        let cbs = &self.code_block_string;
                        tf.item_with(cx, entry_id, live_id!(code_block), |cx, item, _tf|{
                            item.widget(id!(code_view)).set_text(cbs);
                            item.draw_all_unscoped(cx);
                        });
                    }
                    else{
                        tf.font_sizes.pop();
                        //tf.top_drop.pop();
                        tf.fixed.pop();
                        tf.combine_spaces.pop();
                        tf.end_code(cx);
                    }
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
                    if self.in_code_block{
                        self.code_block_string.push_str(&doc.decoded[*start..*end]);
                    }
                    else{
                        tf.draw_text(cx, &doc.decoded[*start..*end]);
                    }
                }
            }
        }
    }
}

impl MarkdownRef {
    pub fn set_text(&mut self, v:&str) {
        let Some(mut inner) = self.borrow_mut() else { return };
        inner.set_text(v)
    }
}

#[derive(Live, LiveHook, Widget)]
struct MarkdownLink {
    #[deref]
    link: LinkLabel,
    #[live]
    href: String,
}

impl WidgetMatchEvent for MarkdownLink {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, scope: &mut Scope) {
        if self.link.clicked(actions) {
            cx.widget_action(
                self.widget_uid(),
                &scope.path,
                MarkdownAction::LinkNavigated(self.href.clone()),
            );
        }
    }
}

impl Widget for MarkdownLink {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.link.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope)
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.link.draw_walk(cx, scope, walk)
    }

    fn text(&self) -> String {
        self.link.text()
    }

    fn set_text(&mut self, v: &str) {
        self.link.set_text(v);
    }
}

impl MarkdownLinkRef {
    pub fn set_href(&mut self, v: &str) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.href = v.to_string();
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum MarkdownAction {
    None,
    LinkNavigated(String),
}
 