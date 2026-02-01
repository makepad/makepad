use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_html::*,
    text_flow::TextFlow,
    widget::*,
    animator::{Animator, AnimatorImpl, AnimatorAction},
    WidgetMatchEvent,
};

const BULLET: &str = "•";

script_mod!{
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    
    mod.widgets.HtmlLinkBase = #(HtmlLink::register_widget(vm))
    
    mod.widgets.HtmlBase = #(Html::register_widget(vm))
    
    mod.widgets.HtmlLink = mod.std.set_type_default() do mod.widgets.HtmlLinkBase{
        width: Fit height: Fit
        align: Align{x: 0. y: 0.}
        
        color: #x0000EE
        hover_color: #x00EE00
        pressed_color: #xEE0000
        
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: 0.0
                        pressed: 0.0
                    }
                }
                
                on: AnimatorState{
                    redraw: true
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        hovered: snap(1.0)
                        pressed: snap(1.0)
                    }
                }
                
                pressed: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: snap(1.0)
                        pressed: snap(1.0)
                    }
                }
            }
        }
    }
    
    mod.widgets.Html = mod.std.set_type_default() do mod.widgets.HtmlBase{
        width: Fill height: Fit
        flow: Flow.Right{wrap: true}
        padding: theme.mspace_1
        
        ul_markers: ["•", "-"]
        ol_separator: "."
        
        heading_margin: Inset{top: 1.0, bottom: 0.1}
        paragraph_margin: Inset{top: 0.33, bottom: 0.33}
        
        font_size: theme.font_size_p
        font_color: theme.color_label_inner
        
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
            temp_y_shift: 0.24
            text_style: theme.font_code{
                font_size: theme.font_size_p
            }
            color: theme.color_label_inner
        }
        
        code_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: theme.space_2}
        }
        code_walk: Walk{width: Fill height: Fit}
        
        quote_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: theme.space_2}
        }
        quote_walk: Walk{width: Fill height: Fit}
        
        list_item_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: theme.mspace_1
        }
        list_item_walk: Walk{
            height: Fit width: Fill
        }
        
        inline_code_padding: theme.mspace_1
        inline_code_margin: theme.mspace_1
        
        sep_walk: Walk{
            width: Fill height: 4.
            margin: theme.mspace_v_1
        }
        
        $a: mod.widgets.HtmlLink{}
        
        draw_block +: {
            line_color: theme.color_label_inner
            sep_color: theme.color_shadow
            quote_bg_color: theme.color_bg_highlight
            quote_fg_color: theme.color_label_inner
            code_color: theme.color_bg_highlight
            space_1: uniform(theme.space_1)
            space_2: uniform(theme.space_2)
            
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                match self.block_type {
                    FlowBlockType.Quote => {
                        sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                        sdf.fill(self.quote_bg_color)
                        sdf.box(self.space_1 self.space_1 self.space_1 self.rect_size.y-self.space_2 1.5)
                        sdf.fill(self.quote_fg_color)
                        return sdf.result
                    }
                    FlowBlockType.Sep => {
                        sdf.box(0. 1. self.rect_size.x-1. self.rect_size.y-2. 2.)
                        sdf.fill(self.sep_color)
                        return sdf.result
                    }
                    FlowBlockType.Code => {
                        sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                        sdf.fill(self.code_color)
                        return sdf.result
                    }
                    FlowBlockType.InlineCode => {
                        sdf.box(1. 1. self.rect_size.x-2. self.rect_size.y-2. 2.)
                        sdf.fill(self.code_color)
                        return sdf.result
                    }
                    FlowBlockType.Underline => {
                        sdf.box(0. self.rect_size.y-2. self.rect_size.x 2.0 0.5)
                        sdf.fill(self.line_color)
                        return sdf.result
                    }
                    FlowBlockType.Strikethrough => {
                        sdf.box(0. self.rect_size.y * 0.45 self.rect_size.x 2.0 0.5)
                        sdf.fill(self.line_color)
                        return sdf.result
                    }
                }
                return #f00
            }
        }
    }
}

/// Whether to trim leading and trailing whitespace in the text body of an HTML tag.
///
/// Currently, *all* Unicode whitespace characters are trimmed, not just ASCII whitespace.
///
/// The default is to keep all whitespace.
#[derive(Copy, Clone, PartialEq, Default)]
pub enum TrimWhitespaceInText {
    /// Leading and trailing whitespace will be preserved in the text.
    #[default]
    Keep,
    /// Leading and trailing whitespace will be trimmed from the text.
    Trim,
}

#[derive(Script, Widget)]
pub struct Html {
    #[deref] pub text_flow: TextFlow,
    #[live] pub body: ArcStringMut,
    #[rust] pub doc: HtmlDoc,
    
    /// Markers used for unordered lists, indexed by the list's nesting level.
    /// The marker can be an arbitrary string, such as a bullet point or a custom icon.
    #[live] ul_markers: Vec<String>,
    /// Markers used for ordered lists, indexed by the list's nesting level.
    #[rust] ol_markers: Vec<OrderedListType>,
    /// The character used to separate an ordered list's item number from the content.
    #[live] ol_separator: String,

    /// The stack of list levels encountered so far, used to track nested lists.
    #[rust] list_stack: Vec<ListLevel>,
}

impl ScriptHook for Html {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        // Initialize ol_markers with default values
        if self.ol_markers.is_empty() {
            self.ol_markers = vec![
                OrderedListType::Numbers,
                OrderedListType::LowerAlpha,
                OrderedListType::LowerRoman,
            ];
        }
    }
    
    fn on_after_apply(&mut self, _vm: &mut ScriptVm, _apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        let mut errors = Some(Vec::new());
        let new_doc = parse_html(self.body.as_ref(), &mut errors, InternLiveId::No);
        if new_doc != self.doc {
            self.doc = new_doc;
            self.text_flow.clear_items();
        }
        if errors.as_ref().unwrap().len() > 0 {
            log!("HTML parser returned errors {:?}", errors)
        }
    }
}

impl Html {
    fn handle_open_tag(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        node: &mut HtmlWalker,
        list_stack: &mut Vec<ListLevel>,
        ul_markers: &Vec<String>,
        ol_markers: &Vec<OrderedListType>,
        ol_separator: &str,
    ) -> (Option<LiveId>, TrimWhitespaceInText) {

        let mut trim_whitespace_in_text = TrimWhitespaceInText::default();

        fn open_header_tag(cx: &mut Cx2d, tf: &mut TextFlow, scale: f64, trim: &mut TrimWhitespaceInText) {
            *trim = TrimWhitespaceInText::Trim;
            tf.bold.push();
            tf.push_size_abs_scale(scale);
            let fs = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f64;
            tf.new_line_collapsed_with_spacing(cx, fs * tf.heading_margin.top);
        }

        match node.open_tag_lc() {
            some_id!(h1) => open_header_tag(cx, tf, 2.0, &mut trim_whitespace_in_text),
            some_id!(h2) => open_header_tag(cx, tf, 1.5, &mut trim_whitespace_in_text),
            some_id!(h3) => open_header_tag(cx, tf, 1.17, &mut trim_whitespace_in_text),
            some_id!(h4) => open_header_tag(cx, tf, 1.0, &mut trim_whitespace_in_text),
            some_id!(h5) => open_header_tag(cx, tf, 0.83, &mut trim_whitespace_in_text),
            some_id!(h6) => open_header_tag(cx, tf, 0.67, &mut trim_whitespace_in_text),

            some_id!(p) => {
                let fs = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f64;
                tf.new_line_collapsed_with_spacing(cx, fs * tf.paragraph_margin.top);
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(code) => {
                const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                tf.combine_spaces.push(false);
                tf.fixed.push();
                tf.inline_code.push();
            }
            some_id!(pre) => {
                tf.new_line_collapsed(cx);
                tf.fixed.push();
                tf.ignore_newlines.push(false);
                tf.combine_spaces.push(false);
                tf.begin_code(cx);
            }
            some_id!(blockquote) => {
                tf.new_line_collapsed(cx);
                tf.ignore_newlines.push(false);
                tf.combine_spaces.push(false);
                tf.begin_quote(cx);
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(br) => {
                tf.new_line_collapsed(cx);
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(hr)
            | some_id!(sep) => {
                tf.new_line_collapsed(cx);
                tf.sep(cx);
                tf.new_line_collapsed(cx);
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(u) => tf.underline.push(),
            some_id!(del)
            | some_id!(s)
            | some_id!(strike) => tf.strikethrough.push(),

            some_id!(b)
            | some_id!(strong) => tf.bold.push(),
            some_id!(i)
            | some_id!(em) => tf.italic.push(),

            some_id!(sub) => {
                tf.push_size_rel_scale(0.7);
            }
            some_id!(sup) => {
                tf.push_size_rel_scale(0.7);
            }
            some_id!(ul) => {
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
                list_stack.push(ListLevel {
                    list_kind: ListKind::Unordered,
                    numbering_type: None,
                    li_count: 1,
                    padding: 2.5,
                });
            }
            some_id!(ol) => { 
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
                let start_attr = node.find_attr_lc(live_id!(start));
                let start: i32 = start_attr
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1);

                let type_attr = node.find_attr_lc(live_id!(type));
                let numbering_type = type_attr.and_then(OrderedListType::from_type_attribute);

                list_stack.push(ListLevel {
                    list_kind: ListKind::Ordered,
                    numbering_type, 
                    li_count: start,
                    padding: 2.5,
                });
            }
            some_id!(li) => {
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
                let indent_level = list_stack.len();
                let index = indent_level.saturating_sub(1);
                let marker_and_pad = list_stack.last_mut().map(|ll| {
                    let marker = match ll.list_kind {
                        ListKind::Unordered => {
                            ul_markers.get(index).cloned()
                                .unwrap_or_else(|| BULLET.into())
                        }
                        ListKind::Ordered => {
                            let value_attr = node.find_attr_lc(live_id!(value));
                            let value: i32 = value_attr
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(ll.li_count);

                            let type_attr = node.find_attr_lc(live_id!(type));
                            let numbering_type = type_attr.and_then(OrderedListType::from_type_attribute);

                            numbering_type.as_ref()
                                .or_else(|| ll.numbering_type.as_ref())
                                .or_else(|| ol_markers.get(index))
                                .map(|ol_type| ol_type.marker(value, ol_separator))
                                .unwrap_or_else(|| "#".into())
                        }
                    };
                    ll.li_count += 1;
                    (marker, ll.padding)
                });
                let (marker, pad) = marker_and_pad.as_ref()
                    .map(|(m, p)| (m.as_str(), *p))
                    .unwrap_or((BULLET, 2.5));
                
                tf.new_line_collapsed(cx);
                tf.begin_list_item(cx, marker, pad);
            }
            Some(x) => return (Some(x), trim_whitespace_in_text),
            _ => ()
        }
        (None, trim_whitespace_in_text)
    }
    
    fn handle_close_tag(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        node: &mut HtmlWalker,
        list_stack: &mut Vec<ListLevel>,
    ) -> Option<LiveId> {
        match node.close_tag_lc() {
            some_id!(h1)
            | some_id!(h2)
            | some_id!(h3)
            | some_id!(h4)
            | some_id!(h5)
            | some_id!(h6) => {
                let size = tf.font_sizes.pop();
                tf.bold.pop();
                tf.new_line_collapsed_with_spacing(cx, size.unwrap_or(0.0) as f64 * tf.heading_margin.bottom);
            }
            some_id!(b)
            | some_id!(strong) => tf.bold.pop(),
            some_id!(i)
            | some_id!(em) => tf.italic.pop(),
            some_id!(p) => {
                let fs = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f64;
                tf.new_line_collapsed_with_spacing(cx, fs * tf.paragraph_margin.bottom);
            }
            some_id!(blockquote) => {
                tf.ignore_newlines.pop();
                tf.combine_spaces.pop();
                tf.end_quote(cx);
            }
            some_id!(code) => {
                tf.inline_code.pop();
                tf.font_sizes.pop();
                tf.combine_spaces.pop();
                tf.fixed.pop(); 
            }
            some_id!(pre) => {
                tf.fixed.pop();
                tf.ignore_newlines.pop();
                tf.combine_spaces.pop();
                tf.end_code(cx);     
            }
            some_id!(sub) => {
                tf.font_sizes.pop();
            }
            some_id!(sup) => {
                tf.font_sizes.pop();
            }
            some_id!(ul)
            | some_id!(ol) => {
                list_stack.pop();
            }
            some_id!(li) => tf.end_list_item(cx),
            some_id!(u) => tf.underline.pop(),
            some_id!(del)
            | some_id!(s)
            | some_id!(strike) => tf.strikethrough.pop(),
            _ => ()
        }
        None
    }
    
    pub fn handle_text_node(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        node: &mut HtmlWalker,
        trim: TrimWhitespaceInText,    
    ) -> bool {
        if let Some(text) = node.text() {
            let text = if trim == TrimWhitespaceInText::Trim {
                text.trim_matches(char::is_whitespace)
            } else {
                text
            };
            tf.draw_text(cx, text);
            true
        }
        else {
            false
        }
    }
}

impl Widget for Html {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.text_flow.handle_event(cx, event, scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let tf = &mut self.text_flow;
        tf.begin(cx, walk);
        let mut node = self.doc.new_walker();
        let mut auto_id = 0;
        while !node.done() {
            let mut trim = TrimWhitespaceInText::default();
            match Self::handle_open_tag(cx, tf, &mut node, &mut self.list_stack, &self.ul_markers, &self.ol_markers, &self.ol_separator) {
                (Some(_), _tws) => {
                    handle_custom_widget(cx, scope, tf, &self.doc, &mut node, &mut auto_id); 
                }
                (None, tws) => {
                    trim = tws;
                }
            }
            match Self::handle_close_tag(cx, tf, &mut node, &mut self.list_stack) {
                _ => ()
            }
            Self::handle_text_node(cx, tf, &mut node, trim);
            node.walk();
        }
        tf.end(cx);
        DrawStep::done()
    }  
     
    fn text(&self) -> String {
        self.body.as_ref().to_string()
    }
    
    fn set_text(&mut self, cx:&mut Cx, v:&str){
        self.body.set(v);
        let mut errors = Some(Vec::new());
        self.doc = parse_html(self.body.as_ref(), &mut errors, InternLiveId::No);
        if errors.as_ref().unwrap().len() > 0 {
            log!("HTML parser returned errors {:?}", errors)
        }
        self.redraw(cx);
    }
} 


fn handle_custom_widget(
    cx: &mut Cx2d,
    _scope: &mut Scope,
    tf: &mut TextFlow,
    doc: &HtmlDoc,
    node: &mut HtmlWalker,
    auto_id: &mut u64,
) {
    let id = if let Some(id) = node.find_attr_lc(live_id!(id)) {
        LiveId::from_str(id)
    } else {
        *auto_id += 1;
        LiveId(*auto_id)
    };

    let template = node.open_tag_nc().unwrap();
    let mut scope_with_attrs = Scope::with_props_index(doc, node.index);

    if let Some(item) = tf.item_with_scope(cx, &mut scope_with_attrs, id, template) {
        item.set_text(cx, node.find_text().unwrap_or(""));
        let mut draw_scope = Scope::with_data(tf);
        item.draw_all(cx, &mut draw_scope);
    }

    node.jump_to_close();
}


#[derive(Clone, Debug, Default)]
pub enum HtmlLinkAction {
    #[default]
    None,
    Clicked {
        url: String,
        key_modifiers: KeyModifiers,
    },
    SecondaryClicked {
        url: String,
        key_modifiers: KeyModifiers,
    },
}

#[derive(Script, Widget, Animator)]
pub struct HtmlLink {
    #[source] source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,

    #[redraw] #[area] area: Area,

    #[walk] walk: Walk,
    #[layout] layout: Layout,

    #[rust] drawn_areas: SmallVec<[Area; 2]>,
    #[live(true)] grab_key_focus: bool,

    #[live] hovered: f32,
    #[live] pressed: f32,

    /// The default font color for the link when not hovered on or pressed.
    #[live] color: Option<Vec4f>,
    /// The font color used when the link is hovered on.
    #[live] hover_color: Option<Vec4f>,
    /// The font color used when the link is pressed.
    #[live] pressed_color: Option<Vec4f>,

    #[live] pub text: ArcStringMut,
    #[live] pub url: String,
}

impl ScriptHook for HtmlLink {
    fn on_after_new_scoped(&mut self, _vm: &mut ScriptVm, scope: &mut Scope) {
        // After an HtmlLink instance has been instantiated,
        // populate its struct fields from the `<a>` tag's attributes.
        if let Some(doc) = scope.props.get::<HtmlDoc>() {
            let mut walker = doc.new_walker_with_index(scope.index + 1);
            
            if let Some((lc, attr)) = walker.while_attr_lc() {
                match lc {
                    live_id!(href) => {
                        self.url = attr.into()
                    }
                    _ => ()
                }
            }
        }
    }
}

impl WidgetMatchEvent for HtmlLink {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions, _scope: &mut Scope) {
        // No actions needed for now
    }
}

impl Widget for HtmlLink {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            if let Some(tf) = scope.data.get_mut::<TextFlow>() {
                tf.redraw(cx);
            } else {
                self.drawn_areas.iter().for_each(|area| area.redraw(cx));
            }
        }
        
        self.widget_match_event(cx, event, scope);

        for area in self.drawn_areas.clone().into_iter() {
            match event.hits(cx, area) {
                Hit::FingerDown(fe) => {
                    if fe.is_primary_hit() {
                        if self.grab_key_focus {
                            cx.set_key_focus(self.area());
                        }
                        self.animator_play(cx, ids!(hover.pressed));
                    }
                    else if fe.mouse_button().is_some_and(|mb| mb.is_secondary()) {
                        cx.widget_action(
                            self.widget_uid(),
                            &scope.path,
                            HtmlLinkAction::SecondaryClicked {
                                url: self.url.clone(),
                                key_modifiers: fe.modifiers,
                            },
                        );
                    }
                }
                Hit::FingerHoverIn(_) => {
                    cx.set_cursor(MouseCursor::Hand);
                    self.animator_play(cx, ids!(hover.on));
                }
                Hit::FingerHoverOut(_) => {
                    self.animator_play(cx, ids!(hover.off));
                }
                Hit::FingerLongPress(_) => {
                    cx.widget_action(
                        self.widget_uid(),
                        &scope.path,
                        HtmlLinkAction::SecondaryClicked {
                            url: self.url.clone(),
                            key_modifiers: Default::default(),
                        },
                    );
                }
                Hit::FingerUp(fu) => {
                    if fu.is_over {
                        cx.set_cursor(MouseCursor::Hand);
                        self.animator_play(cx, ids!(hover.on));
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }

                    if fu.is_over
                        && fu.is_primary_hit()
                        && fu.was_tap()
                    {
                        cx.widget_action(
                            self.widget_uid(),
                            &scope.path,
                            HtmlLinkAction::Clicked {
                                url: self.url.clone(),
                                key_modifiers: fu.modifiers,
                            },
                        );
                    }
                }
                _ => (),
            }
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let Some(tf) = scope.data.get_mut::<TextFlow>() else {
            return DrawStep::done();
        };

        tf.underline.push();
        tf.areas_tracker.push_tracker();
        let mut pushed_color = false;
        if self.hovered > 0.0 {
            if let Some(color) = self.hover_color {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        } else if self.pressed > 0.0 {
            if let Some(color) = self.pressed_color {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        } else {
            if let Some(color) = self.color {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        }
        tf.draw_text(cx, self.text.as_ref());
        
        if pushed_color {
            tf.font_colors.pop();
        }
        tf.underline.pop();

        let (start, end) = tf.areas_tracker.pop_tracker();
        
        if self.drawn_areas.len() == end-start {
            for i in 0..end-start {
                self.drawn_areas[i] = cx.update_area_refs(self.drawn_areas[i], 
                    tf.areas_tracker.areas[i+start]);
            }
        }
        else {
            self.drawn_areas = SmallVec::from(
                &tf.areas_tracker.areas[start..end]
            );
        }

        DrawStep::done()
    }
    
    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx:&mut Cx, v: &str) {
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}

impl HtmlRef {
    pub fn set_text(&mut self, cx:&mut Cx, v:&str) {
        let Some(mut inner) = self.borrow_mut() else { return };
        inner.set_text(cx, v)
    }
}

impl HtmlLinkRef {
    pub fn set_url(&mut self, url: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.url = url.to_string();
        }
    }

    pub fn url(&self) -> Option<String> {
        if let Some(inner) = self.borrow() {
            Some(inner.url.clone())
        } else {
            None
        }
    }
}

/// The format and metadata of a list at a given nesting level.
#[derive(Debug)]
struct ListLevel {
    /// The kind of list, either ordered or unordered.
    list_kind: ListKind,
    /// The type of marker formatting for ordered lists,
    /// if overridden for this particular list level.
    numbering_type: Option<OrderedListType>,
    /// The number of list items encountered so far at this level of nesting.
    /// This is a 1-indexed value, so the default initial value should be 1.
    /// This is an integer because negative numbering values are supported.
    li_count: i32,
    /// The padding space inserted to the left of each list item,
    /// where the list marker is drawn.
    padding: f64,
}

/// List kinds: ordered (numbered) and unordered (bulleted).
#[derive(Debug)]
enum ListKind {
    Unordered,
    Ordered,
}

/// The type of marker used for ordered lists.
///
/// See the ["type" attribute docs](https://developer.mozilla.org/en-US/docs/Web/HTML/Element/ol#attributes)
/// for more info.
#[derive(Copy, Clone, Debug, Default)]
pub enum OrderedListType {
    #[default]
    /// Decimal integers: 1, 2, 3, 4, ...
    ///
    /// This *does* support negative integer values, e.g., -2, -1, 0, 1, 2 ...
    Numbers,
    /// Uppercase letters: A, B, C, D, ...
    UpperAlpha,
    /// Lowercase letters: a, b, c, d, ...
    LowerAlpha,
    /// Uppercase roman numerals: I, II, III, IV, ...
    UpperRoman,
    /// Lowercase roman numerals: i, ii, iii, iv, ...
    LowerRoman,
}

impl OrderedListType {
    /// Returns the marker for the given count and separator character.
    ///
    /// ## Notes on behavior
    /// * A negative or zero `count` will always return an integer number marker.
    /// * Currently, for `UpperApha` and `LowerAlpha`, a `count` higher than 25 will result in a wrong character.
    /// * Roman numerals >= 4000 will return an integer number marker.
    pub fn marker(&self, count: i32, separator: &str) -> String {
        let to_number = || format!("{count}{separator}");
        if count <= 0 { return to_number(); }

        match self {
            OrderedListType::Numbers => to_number(),
            OrderedListType::UpperAlpha => format!("{}{separator}", ('A' as u8 + count as u8 - 1) as char),
            OrderedListType::LowerAlpha => format!("{}{separator}", ('a' as u8 + count as u8 - 1) as char),
            OrderedListType::UpperRoman => to_roman_numeral(count)
                .map(|m| format!("{}{separator}", m))
                .unwrap_or_else(to_number),
            OrderedListType::LowerRoman => to_roman_numeral(count)
                .map(|m| format!("{}{separator}", m.to_lowercase()))
                .unwrap_or_else(to_number),
        }
    }

    /// Returns an ordered list type based on the given HTML `type` attribute value string `s`.
    ///
    /// Returns `None` if an invalid value is given.
    pub fn from_type_attribute(s: &str) -> Option<Self> {
        match s {
            "a" => Some(OrderedListType::LowerAlpha),
            "A" => Some(OrderedListType::UpperAlpha),
            "i" => Some(OrderedListType::LowerRoman),
            "I" => Some(OrderedListType::UpperRoman),
            "1" => Some(OrderedListType::Numbers),
            _ => None,
        }
    }
}

/// Converts an integer into an uppercase roman numeral string.
///
/// Returns `None` if the input is not between 1 and 3999 inclusive.
///
/// This code was adapted from the [`roman` crate](https://crates.io/crates/roman).
pub fn to_roman_numeral(mut count: i32) -> Option<String> {
    const MAX: i32 = 3999;
    static NUMERALS: &[(i32, &str)] = &[
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"),
        (100, "C"), (90, "XC"), (50, "L"), (40, "XL"),
        (10, "X"), (9, "IX"), (5, "V"), (4, "IV"),
        (1, "I")
    ];

    if count <= 0 || count > MAX { return None; }
    let mut output = String::new();
    for &(value, s) in NUMERALS.iter() {
        while count >= value {
            count -= value;
            output.push_str(s);
        }
    }
    if count == 0 {
        Some(output)
    } else {
        None
    }
}
