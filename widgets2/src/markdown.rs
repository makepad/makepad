use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    text_flow::{TextFlow, FlowBlockType},
    link_label::LinkLabel,
    WidgetMatchEvent,
};

#[cfg(feature = "markdown")]
use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};

script_mod!{
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    
    mod.widgets.MarkdownLinkBase = #(MarkdownLink::register_widget(vm))
    
    mod.widgets.MarkdownBase = #(Markdown::register_widget(vm))
    
    mod.widgets.MarkdownLink = mod.std.set_type_default() do mod.widgets.MarkdownLinkBase{
        width: Fit height: Fit
        align: Align{x: 0. y: 0.}
        
        label_walk: Walk{width: Fit height: Fit}
        
        draw_icon +: {
            hover: instance(0.0)
            pressed: instance(0.0)

            get_color: fn() {
                return mix(
                    mix(
                        theme.color_label_inner,
                        theme.color_label_inner_hover,
                        self.hover
                    ),
                    theme.color_label_inner_down,
                    self.pressed
                )
            }
        }
        
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0 hover: 0.0}
                        draw_icon: {pressed: 0.0 hover: 0.0}
                        draw_text: {pressed: 0.0 hover: 0.0}
                    }
                }
                
                on: AnimatorState{
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0 hover: snap(1.0)}
                        draw_icon: {pressed: 0.0 hover: snap(1.0)}
                        draw_text: {pressed: 0.0 hover: snap(1.0)}
                    }
                }
                
                pressed: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: snap(1.0) hover: 1.0}
                        draw_icon: {pressed: snap(1.0) hover: 1.0}
                        draw_text: {pressed: snap(1.0) hover: 1.0}
                    }
                }
            }
        }
        
        draw_bg +: {
            pressed: instance(0.0)
            hover: instance(0.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let offset_y = 1.0
                sdf.move_to(0. (self.rect_size.y - offset_y))
                sdf.line_to(self.rect_size.x (self.rect_size.y - offset_y))
                return sdf.stroke(mix(
                    theme.color_label_inner,
                    theme.color_label_inner_down,
                    self.pressed
                ), mix(0.0, 0.8, self.hover))
            }
        }
        
        draw_text +: {
            pressed: instance(0.0)
            hover: instance(0.0)

            color_hover: uniform(theme.color_label_inner_hover)
            color_pressed: uniform(theme.color_label_inner_down)

            wrap: Wrapping.Word
            color: theme.color_label_inner
            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
            get_color: fn() {
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
    
    mod.widgets.Markdown = mod.std.set_type_default() do mod.widgets.MarkdownBase{
        width: Fill height: Fit
        flow: Flow.right_wrap()
        padding: theme.mspace_1
                
        font_size: theme.font_size_p
        font_color: theme.color_label_inner
        
        paragraph_spacing: 16
        pre_code_spacing: 8
        inline_code_padding: theme.mspace_1
        inline_code_margin: theme.mspace_1
        heading_base_scale: 1.8
                
        draw_normal +: {
            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
            color: theme.color_label_inner
        }
        
        draw_italic +: {
            text_style: theme.font_italic{
                font_size: theme.font_size_p
            }
            color: theme.color_label_inner
        }
        
        draw_bold +: {
            text_style: theme.font_bold{
                font_size: theme.font_size_p
            }
            color: theme.color_label_inner
        }
        
        draw_bold_italic +: {
            text_style: theme.font_bold_italic{
                font_size: theme.font_size_p
            }
            color: theme.color_label_inner
        }
        
        draw_fixed +: {
            temp_y_shift: 0.25
            text_style: theme.font_code{
                font_size: theme.font_size_p
            }
            color: theme.color_label_inner
        }
        
        code_layout: Layout{
            flow: Flow.right_wrap()
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: 10}
        }
        code_walk: Walk{width: Fill height: Fit}
        
        quote_layout: Layout{
            flow: Flow.right_wrap()
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: theme.space_2}
        }
        quote_walk: Walk{width: Fill height: Fit}
        
        list_item_layout: Layout{
            flow: Flow.right_wrap()
            padding: theme.mspace_1
        }
        list_item_walk: Walk{
            height: Fit width: Fill
        }
        
        sep_walk: Walk{
            width: Fill height: 4.
            margin: theme.mspace_v_1
        }
        
        draw_block +: {
            line_color: theme.color_label_inner
            sep_color: theme.color_shadow
            quote_bg_color: theme.color_bg_highlight
            quote_fg_color: theme.color_label_inner
            code_color: theme.color_bg_highlight
            
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                match self.block_type {
                    FlowBlockType.Quote => {
                        sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                        sdf.fill(self.quote_bg_color)
                        sdf.box(theme.space_1 theme.space_1 theme.space_1 (self.rect_size.y - theme.space_2) 1.5)
                        sdf.fill(self.quote_fg_color)
                        return sdf.result
                    }
                    FlowBlockType.Sep => {
                        sdf.box(0. 1. (self.rect_size.x - 1.) (self.rect_size.y - 2.) 2.)
                        sdf.fill(self.sep_color)
                        return sdf.result
                    }
                    FlowBlockType.Code => {
                        sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                        sdf.fill(self.code_color)
                        return sdf.result
                    }
                    FlowBlockType.InlineCode => {
                        sdf.box(1. 1. self.rect_size.x (self.rect_size.y - 2.) 2.)
                        sdf.fill(self.code_color)
                        return sdf.result
                    }
                    FlowBlockType.Underline => {
                        sdf.box(0. (self.rect_size.y - 2.) self.rect_size.x 2.0 0.5)
                        sdf.fill(self.line_color)
                        return sdf.result
                    }
                    FlowBlockType.Strikethrough => {
                        sdf.box(0. (self.rect_size.y * 0.45) self.rect_size.x 2.0 0.5)
                        sdf.fill(self.line_color)
                        return sdf.result
                    }
                }
                return #f00
            }
        }
        
        link: mod.widgets.MarkdownLink{}
    }
}

/// The state of a list at a given nesting level.
#[cfg(feature = "markdown")]
struct ListState {
    // Current item number for ordered lists.
    current_number: u64,         
    // Start number for ordered lists, None for unordered.
    start_number: Option<u64>,  
}

#[derive(Script, ScriptHook, Widget)]
pub struct Markdown{
    #[deref] text_flow: TextFlow,
    #[live] body: ArcStringMut,
    #[live] paragraph_spacing: f64,
    #[live] pre_code_spacing: f64,
    #[live(false)] use_code_block_widget:bool,
    #[rust] in_code_block: bool,
    #[rust] code_block_string: String,
    #[live(false)] use_math_widget: bool,
    #[rust] auto_id: u64,
    #[live] heading_base_scale: f64,
}

impl Widget for Markdown {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.text_flow.handle_event(cx, event, scope);
    } 
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk:Walk)->DrawStep{
        self.auto_id = 0;
        self.begin(cx, walk);
        #[cfg(feature = "markdown")]
        self.process_markdown_doc(cx);
        #[cfg(not(feature = "markdown"))]
        {
            self.text_flow.draw_text(cx, "Markdown feature not enabled");
        }
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
    #[cfg(feature = "markdown")]
    fn process_markdown_doc(&mut self, cx: &mut Cx2d) {
        let tf = &mut self.text_flow;
        // Track state for nested formatting
        let mut list_stack: Vec<ListState> = Vec::new();
        let mut is_first_block = true;

        let parser = Parser::new_ext(self.body.as_ref(), Options::ENABLE_TABLES | Options::ENABLE_MATH);        
        
        for event in parser.into_iter() {
            match event {
                MdEvent::Start(Tag::Heading { level, .. }) => {
                    if !is_first_block {
                        cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    }
                    is_first_block = false; 
                    let heading_base = self.heading_base_scale;
                    let scale = match level {
                        HeadingLevel::H1 => heading_base,
                        HeadingLevel::H2 => heading_base * 0.75,
                        HeadingLevel::H3 => heading_base * 0.58,
                        HeadingLevel::H4 => heading_base * 0.5, 
                        HeadingLevel::H5 => heading_base * 0.42,
                        HeadingLevel::H6 => heading_base * 0.33,
                    };
                    tf.push_size_abs_scale(scale);
                    tf.bold.push();
                }
                MdEvent::End(TagEnd::Heading(_level)) => {
                    tf.bold.pop();
                    tf.font_sizes.pop();
                    cx.turtle_new_line();
                }
                MdEvent::Start(Tag::Paragraph) => {
                    if !is_first_block {
                         cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    }
                    is_first_block = false;
                }
                MdEvent::End(TagEnd::Paragraph) => {
                    // No special handling needed, turtle position is managed by content/following blocks
                }
                MdEvent::Start(Tag::BlockQuote(_)) => {
                     if !is_first_block {
                        cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    }
                    is_first_block = false;
                    tf.begin_quote(cx);
                }
                MdEvent::End(TagEnd::BlockQuote(_quote_kind)) => {
                    tf.end_quote(cx);
                }
                MdEvent::Start(Tag::List(first_number)) => {
                    list_stack.push(ListState {
                        start_number: first_number,
                        current_number: first_number.unwrap_or(1),
                    });
                }
                MdEvent::End(TagEnd::List(_is_ordered)) => {
                    list_stack.pop();
                }
                MdEvent::Start(Tag::Item) => {
                     if !is_first_block {
                         cx.turtle_new_line();
                     }
                     is_first_block = false;
                    let marker = if let Some(state) = list_stack.last_mut() {
                        if state.start_number.is_some() {
                            // Ordered list - use and increment the counter
                            let num = state.current_number;
                            state.current_number += 1;
                            format!("{}.", num)
                        } else {
                            // Unordered list - use bullet
                            "•".to_string()
                        }
                    } else {
                        "•".to_string()
                    };
                    tf.begin_list_item(cx, &marker, 2.5);
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
                    if !is_first_block {
                         cx.turtle_new_line_with_spacing(self.pre_code_spacing);
                    }
                    is_first_block = false;
                    if self.use_code_block_widget {
                        self.in_code_block = true;
                        self.code_block_string.clear();
                    } else {
                        const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                        tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                        tf.combine_spaces.push(false);
                        tf.fixed.push();
                        tf.begin_code(cx);
                    }
                }
                MdEvent::End(TagEnd::CodeBlock) => {
                    if self.in_code_block {
                        self.in_code_block = false;
                        let entry_id = tf.new_counted_id();
                        let cbs = &self.code_block_string;
                        
                        tf.item_with(cx, entry_id, live_id!(code_block), |cx, item, _tf|{
                            item.widget(ids!(code_view)).set_text(cx, cbs);
                            item.draw_all_unscoped(cx);
                        });
                    }
                    else{
                        tf.font_sizes.pop();
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
                // Inline math ($...$)
                MdEvent::InlineMath(text) => {
                    if self.use_math_widget {
                        let entry_id = tf.new_counted_id();
                        tf.item_with(cx, entry_id, live_id!(inline_math), |cx, item, _tf| {
                            item.set_text(cx, &text);
                            item.draw_all_unscoped(cx);
                        });
                    } else {
                        // Fallback: render as inline code style
                        const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                        tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                        tf.fixed.push();
                        tf.inline_code.push();
                        tf.draw_text(cx, &text);
                        tf.font_sizes.pop();
                        tf.fixed.pop();
                        tf.inline_code.pop();
                    }
                }
                // Display math ($$...$$)
                MdEvent::DisplayMath(text) => {
                    if !is_first_block {
                        cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    }
                    is_first_block = false;

                    if self.use_math_widget {
                        let entry_id = tf.new_counted_id();
                        tf.item_with(cx, entry_id, live_id!(display_math), |cx, item, _tf| {
                            item.set_text(cx, &text);
                            item.draw_all_unscoped(cx);
                        });
                    } else {
                        // Fallback: render as code block style
                        tf.begin_code(cx);
                        tf.fixed.push();
                        tf.draw_text(cx, &text);
                        tf.fixed.pop();
                        tf.end_code(cx);
                    }
                }
                MdEvent::Text(text) => {
                    if self.in_code_block {
                        self.code_block_string.push_str(&text);
                    } else {
                        tf.draw_text(cx, &text.trim_end_matches("\n"));
                    }
                }
                MdEvent::SoftBreak => {
                    if self.in_code_block {
                        self.code_block_string.push('\n');
                    } else {
                        tf.draw_text(cx, " ");
                    }
                }
                MdEvent::HardBreak => {
                    if self.in_code_block {
                        self.code_block_string.push('\n');
                    } else {
                         cx.turtle_new_line();
                    }
                }
                MdEvent::Rule => {
                     if !is_first_block {
                        cx.turtle_new_line_with_spacing(self.paragraph_spacing);
                    }
                     is_first_block = false;
                    tf.sep(cx);
                    cx.turtle_new_line_with_spacing(self.paragraph_spacing);
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
                _ => {} // Unimplemented or unnecessary events
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

#[derive(Script, ScriptHook, Widget)]
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
