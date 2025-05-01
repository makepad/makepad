use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    text_flow::TextFlow,
    link_label::LinkLabel,
    WidgetMatchEvent,
};

use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};

live_design!{
    link widgets;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    use crate::link_label::LinkLabelBase;
    
    pub MarkdownLinkBase = {{MarkdownLink}}<LinkLabelBase> {
        /*link = {
            draw_text:{
                // other blue hyperlink colors: #1a0dab, // #0969da  // #0c50d1
                color: #1a0dab
            }
        }*/
    }

    pub MarkdownBase = {{Markdown}} {
        // ok so we can use one drawtext
        // change to italic, change bold (SDF), strikethrough
    }
    
    pub MarkdownLink = <MarkdownLinkBase> {
        width: Fit, height: Fit,
        align: {x: 0., y: 0.}
        
        label_walk: { width: Fit, height: Fit }
        
        draw_icon: {
            instance hover: 0.0
            instance pressed: 0.0

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_LABEL_INNER,
                        THEME_COLOR_LABEL_INNER_HOVER,
                        self.hover
                    ),
                    THEME_COLOR_LABEL_INNER_DOWN,
                    self.pressed
                )
            }
        }
        
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0, hover: 0.0}
                        draw_icon: {pressed: 0.0, hover: 0.0}
                        draw_text: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_icon: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
        }
        
        draw_bg: {
            instance pressed: 0.0
            instance hover: 0.0

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let offset_y = 1.0
                sdf.move_to(0., self.rect_size.y - offset_y);
                sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);
                return sdf.stroke(mix(
                    THEME_COLOR_LABEL_INNER,
                    THEME_COLOR_LABEL_INNER_DOWN,
                    self.pressed
                ), mix(0.0, 0.8, self.hover));
            }
        }
        
        draw_text: {
            instance pressed: 0.0
            instance hover: 0.0

            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER),
            uniform color_pressed: (THEME_COLOR_LABEL_INNER_DOWN),

            wrap: Word
            color: (THEME_COLOR_LABEL_INNER),
            text_style: <THEME_FONT_REGULAR>{
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        self.color_hover,
                        self.hover
                    ),
                    self.color_pressed,
                    self.pressed
                )
            }
        }
    }
    
    pub Markdown = <MarkdownBase> {
        width:Fill, height:Fit,
        flow: RightWrap,
        padding: <THEME_MSPACE_1> {}
                
        font_size: (THEME_FONT_SIZE_P),
        font_color: (THEME_COLOR_LABEL_INNER),
        
        paragraph_spacing: 16,
        pre_code_spacing: 8,
        inline_code_padding: <THEME_MSPACE_1> {},
        inline_code_margin: <THEME_MSPACE_1> {},
                
        draw_normal: {
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_LABEL_INNER)
        }
        
        draw_italic: {
            text_style: <THEME_FONT_ITALIC> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_LABEL_INNER)
        }
        
        draw_bold: {
            text_style: <THEME_FONT_BOLD> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_LABEL_INNER)
        }
        
        draw_bold_italic: {
            text_style: <THEME_FONT_BOLD_ITALIC> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_LABEL_INNER)
        }
        
        draw_fixed: {
            text_style: <THEME_FONT_CODE> {
                font_size: (THEME_FONT_SIZE_P)
            }
            color: (THEME_COLOR_LABEL_INNER)
        }
        
        code_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_2> { left: (THEME_SPACE_3), right: (THEME_SPACE_3) }
        }
        code_walk: { width: Fill, height: Fit }
        
        quote_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_2> { left: (THEME_SPACE_3), right: (THEME_SPACE_3) }
        }
        quote_walk: { width: Fill, height: Fit, }
        
        list_item_layout: {
            flow: RightWrap,
            padding: <THEME_MSPACE_1> {}
        }
        list_item_walk: {
            height: Fit, width: Fill,
        }
        
        sep_walk: {
            width: Fill, height: 4.
            margin: <THEME_MSPACE_V_1> {}
        }
        
        draw_block: {
            line_color: (THEME_COLOR_LABEL_INNER)
            sep_color: (THEME_COLOR_SHADOW)
            quote_bg_color: (THEME_COLOR_BG_HIGHLIGHT)
            quote_fg_color: (THEME_COLOR_LABEL_INNER)
            code_color: (THEME_COLOR_BG_HIGHLIGHT)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                match self.block_type {
                    FlowBlockType::Quote => {
                        sdf.box(
                            0.,
                            0.,
                            self.rect_size.x,
                            self.rect_size.y,
                            2.
                        );
                        sdf.fill(self.quote_bg_color)
                        sdf.box(
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            THEME_SPACE_1,
                            self.rect_size.y - THEME_SPACE_2,
                            1.5
                        );
                        sdf.fill(self.quote_fg_color)
                        return sdf.result;
                    }
                    FlowBlockType::Sep => {
                        sdf.box(
                            0.,
                            1.,
                            self.rect_size.x-1,
                            self.rect_size.y-2.,
                            2.
                        );
                        sdf.fill(self.sep_color);
                        return sdf.result;
                    }
                    FlowBlockType::Code => {
                        sdf.box(
                            0.,
                            0.,
                            self.rect_size.x,
                            self.rect_size.y,
                            2.
                        );
                        sdf.fill(self.code_color);
                        return sdf.result;
                    }
                    FlowBlockType::InlineCode => {
                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x,
                            self.rect_size.y - 2.,
                            2.
                        );
                        sdf.fill(self.code_color);
                        return sdf.result;
                    }
                    FlowBlockType::Underline => {
                        sdf.box(
                            0.,
                            self.rect_size.y-2,
                            self.rect_size.x,
                            2.0,
                            0.5
                        );
                        sdf.fill(self.line_color);
                        return sdf.result;
                    }
                    FlowBlockType::Strikethrough => {
                        sdf.box(
                            0.,
                            self.rect_size.y * 0.45,
                            self.rect_size.x,
                            2.0,
                            0.5
                        );
                        sdf.fill(self.line_color);
                        return sdf.result;
                    }
                }
                return #f00
            }
        }
        
        link = <MarkdownLink> {}
    }
    
} 

#[derive(Live, LiveHook, Widget)]
pub struct Markdown{
    #[deref] text_flow: TextFlow,
    #[live] body: ArcStringMut,
    #[live] paragraph_spacing: f64,
    #[live] pre_code_spacing: f64,
    #[live(false)] use_code_block_widget:bool,
    #[rust] in_code_block: bool,
    #[rust] code_block_string: String,
    #[rust] auto_id: u64
}

impl Widget for Markdown {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.text_flow.handle_event(cx, event, scope);
    } 
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk:Walk)->DrawStep{
        self.auto_id = 0;
        self.begin(cx, walk);
        self.process_markdown_doc(cx);
        self.end(cx);
        DrawStep::done()
    }
     
    fn text(&self)->String{
        self.body.as_ref().to_string()
    } 
    
    fn set_text(&mut self, cx:&mut Cx, v:&str){
        if self.body.as_ref() != v{
            self.body.set(v);
            self.redraw(cx);
        }
    }
}

impl Markdown {
    fn process_markdown_doc(&mut self, cx: &mut Cx2d) {
        let tf = &mut self.text_flow;
        // Track state for nested formatting
        let mut list_stack = Vec::new();

        let parser = Parser::new_ext(self.body.as_ref(), Options::ENABLE_TABLES);        
        
        for event in parser.into_iter() {
            match event {
                MdEvent::Start(Tag::Heading { level, .. }) => {
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    let levelf64 = match level {
                        HeadingLevel::H1 => 1.0,
                        HeadingLevel::H2 => 2.0,
                        HeadingLevel::H3 => 3.0,
                        HeadingLevel::H4 => 4.0,
                        HeadingLevel::H5 => 5.0,
                        HeadingLevel::H6 => 6.0,
                    };
                    tf.push_size_abs_scale(4.5 / levelf64);
                    tf.bold.push();
                }
                MdEvent::End(TagEnd::Heading(_level)) => {
                    tf.bold.pop();
                    tf.font_sizes.pop();
                    cx.turtle_new_line();
                }
                MdEvent::Start(Tag::Paragraph) => {
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                }
                MdEvent::End(TagEnd::Paragraph) => {
                    // No special handling needed
                }
                MdEvent::Start(Tag::BlockQuote(_)) => {
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    tf.begin_quote(cx);
                }
                MdEvent::End(TagEnd::BlockQuote(_quote_kind)) => {
                    tf.end_quote(cx);
                }
                MdEvent::Start(Tag::List(first_number)) => {
                    list_stack.push(first_number);
                }
                MdEvent::End(TagEnd::List(_is_ordered)) => {
                    list_stack.pop();
                }
                MdEvent::Start(Tag::Item) => {
                    cx.turtle_new_line();
                    let marker = if let Some(Some(n)) = list_stack.last() {
                        format!("{}.", n)
                    } else {
                        "•".to_string()
                    };
                    tf.begin_list_item(cx, &marker, 1.5);
                }
                MdEvent::End(TagEnd::Item) => {
                    tf.end_list_item(cx);
                }
                MdEvent::Start(Tag::Emphasis) => {
                    tf.italic.push();
                }
                MdEvent::End(TagEnd::Emphasis) => {
                    tf.italic.pop();
                }
                MdEvent::Start(Tag::Strong) => {
                    tf.bold.push();
                }
                MdEvent::End(TagEnd::Strong) => {
                    tf.bold.pop();
                }
                MdEvent::Start(Tag::Strikethrough) => {
                    tf.underline.push();
                }
                MdEvent::End(TagEnd::Strikethrough) => {
                    tf.underline.pop();
                }
                MdEvent::Start(Tag::Link { dest_url, .. }) => {
                    self.auto_id += 1;
                    let item = tf.item(cx, LiveId(self.auto_id), live_id!(link));
                    item.as_markdown_link().set_href(&dest_url);
                    item.draw_all_unscoped(cx);
                }
                MdEvent::End(TagEnd::Link) => {
                    // Link handling is done in Start event
                }
                MdEvent::Start(Tag::Image { dest_url, title, .. }) => {
                    tf.draw_text(cx, "Image[name:");
                    tf.draw_text(cx, &title);
                    tf.draw_text(cx, ", url:");
                    tf.draw_text(cx, &dest_url);
                    tf.draw_text(cx, "]");
                }
                MdEvent::Start(Tag::CodeBlock(_kind)) => {
                    if self.use_code_block_widget {
                        self.in_code_block = true;
                        self.code_block_string.clear();
                        cx.turtle_new_line_with_spacing(self.pre_code_spacing);
                        
                        // TODO: Handle language info if available for syntax highlighting
                        // if let CodeBlockKind::Fenced(lang) = kind {
                        // }
                    } else {
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
                }
                MdEvent::End(TagEnd::CodeBlock) => {
                    if self.in_code_block {
                        self.in_code_block = false;
                        let entry_id = tf.new_counted_id();
                        let cbs = &self.code_block_string;
                        tf.item_with(cx, entry_id, live_id!(code_block), |cx, item, _tf|{
                            item.widget(id!(code_view)).set_text(cx, cbs);
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
                }
                // Inline code
                MdEvent::Code(text) => {
                    const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                    tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                    tf.fixed.push();
                    tf.inline_code.push();
                    tf.draw_text(cx, &text);
                    tf.font_sizes.pop();
                    tf.fixed.pop();
                    tf.inline_code.pop();
                }
                MdEvent::Text(text) => {
                    if self.in_code_block {
                        self.code_block_string.push_str(&text);
                    } else {
                        tf.draw_text(cx, &text);
                    }
                }
                MdEvent::SoftBreak => {
                    if self.in_code_block {
                        self.code_block_string.push('\n');
                    } else {
                        cx.turtle_new_line();
                    }
                }
                MdEvent::HardBreak => {
                    if self.in_code_block {
                        self.code_block_string.push('\n');
                    } else {
                        cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    }
                }
                MdEvent::Rule => {
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    tf.sep(cx);
                }
                MdEvent::TaskListMarker(_) => {
                    // TODO: Implement task list markers
                }
                MdEvent::Start(Tag::Table(_)) => {
                    // TODO: Implement table support
                }
                MdEvent::End(TagEnd::Table) => {
                    // TODO: Implement table support
                }
                MdEvent::Start(Tag::TableHead) => {
                    // TODO: Implement table header support
                }
                MdEvent::End(TagEnd::TableHead) => {
                    // TODO: Implement table header support
                }
                MdEvent::Start(Tag::TableRow) => {
                    // TODO: Implement table row support
                }
                MdEvent::End(TagEnd::TableRow) => {
                    // TODO: Implement table row support
                }
                MdEvent::Start(Tag::TableCell) => {
                    // TODO: Implement table cell support
                }
                MdEvent::End(TagEnd::TableCell) => {
                    // TODO: Implement table cell support
                }
                _ => {} // Unimplemented or unneceary events
            }
        }
    }
}

impl MarkdownRef {
    pub fn set_text(&mut self, cx:&mut Cx, v:&str) {
        let Some(mut inner) = self.borrow_mut() else { return };
        inner.set_text(cx, v)
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

    fn set_text(&mut self, cx:&mut Cx, v: &str) {
        self.link.set_text(cx, v);
    }
}

impl MarkdownLinkRef {
    pub fn set_href(&self, v: &str) {
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
 