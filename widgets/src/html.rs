use {
    crate::{
        makepad_html::*,
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        text_flow::TextFlow,
        link_label::LinkLabel,
    },
    std::rc::Rc,
};

const BULLET: &str = "•";

live_design!{
    import makepad_widgets::link_label::LinkLabelBase;

    HtmlBase = {{Html}} {
        // ok so we can use one drawtext
        // change to italic, change bold (SDF), strikethrough

        ul_markers: ["•", "-"],
        ol_markers: [Numbers, LowerAlpha, LowerRoman],
        ol_separator: ".",
    }

    // other blue hyperlink colors: #1a0dab, // #0969da  // #0c50d1
    const LINK_COLOR = #x155EEF

    HtmlLink = {{HtmlLink}}<LinkLabelBase> {
        width: Fit,
        draw_text: {
            color: (LINK_COLOR),
        }
    }
}

#[derive(Live, Widget)]
pub struct Html {
    #[deref] pub text_flow: TextFlow,
    #[live] pub body: Rc<String>,
    #[rust] pub doc: HtmlDoc,

    /// Markers used for unordered lists, indexed by the list's nesting level.
    /// The marker can be an arbitrary string, such as a bullet point or a custom icon.
    #[live] ul_markers: Vec<String>,
    /// Markers used for ordered lists, indexed by the list's nesting level.
    #[live] ol_markers: Vec<OrderedListType>,
    /// The character used to separate an ordered list's item number from the content.
    #[live] ol_separator: String,

    /// The stack of list levels encountered so far, used to track nested lists.
    #[rust] list_stack: Vec<ListLevel>,
}

// alright lets parse the HTML
impl LiveHook for Html {
    fn after_apply_from(&mut self, _cx: &mut Cx, _apply:&mut Apply) {
        let mut errors = Some(Vec::new());
        self.doc = parse_html(&*self.body, &mut errors);
        if errors.as_ref().unwrap().len()>0{
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
    ) -> Option<LiveId> {

        fn open_header_tag(cx: &mut Cx2d, tf: &mut TextFlow, scale: f64) {
            tf.push_bold();
            tf.push_size_abs_scale(scale);
            cx.turtle_new_line();
        }
            
        match node.open_tag_lc() {
            some_id!(h1) => open_header_tag(cx, tf, 2.0),
            some_id!(h2) => open_header_tag(cx, tf, 1.5),
            some_id!(h3) => open_header_tag(cx, tf, 1.17),
            some_id!(h4) => open_header_tag(cx, tf, 1.0),
            some_id!(h5) => open_header_tag(cx, tf, 0.83),
            some_id!(h6) => open_header_tag(cx, tf, 0.67),

            some_id!(p) => {
                // there's probably a better way to do this by setting margins...
                cx.turtle_new_line();
                cx.turtle_new_line();
            }
            some_id!(code) => {
                tf.push_size_abs_scale(0.85);
                tf.push_fixed();
                // tf.begin_inline_code(cx); // inline code block doesn't work properly
            }
            some_id!(pre) => {
                cx.turtle_new_line();
                tf.push_fixed();
                tf.begin_code(cx);
            }
            some_id!(blockquote) => {
                cx.turtle_new_line();
                tf.begin_quote(cx);
            }
            some_id!(br) => cx.turtle_new_line(),
            some_id!(hr)
            | some_id!(sep) => {
                cx.turtle_new_line();
                tf.sep(cx);
                cx.turtle_new_line();
            }
            some_id!(u) => tf.push_underline(),
            some_id!(del)
            | some_id!(s)
            | some_id!(strike) => tf.push_strikethrough(),

            some_id!(b)
            | some_id!(strong) => tf.push_bold(),
            some_id!(i)
            | some_id!(em) => tf.push_italic(),

            some_id!(sub) => {
                // TODO: adjust baseline downwards
                tf.push_size_abs_scale(0.6);
            }
            some_id!(sup) => {
                tf.push_size_abs_scale(0.6);
            }
            some_id!(ul) => {
                list_stack.push(ListLevel {
                    list_kind: ListKind::Unordered,
                    numbering_type: None,
                    li_count: 1,
                    padding: 2.5,
                });
            }
            some_id!(ol) => { 
                // Handle the "start" attribute
                let start_attr = node.find_attr_lc(live_id!(start));
                let start: i32 = start_attr
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1);

                // Handle the "type" attribute
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
                let indent_level = list_stack.len();
                let index = indent_level.saturating_sub(1);
                // log!("indent_level: {indent_level}, index: {index}, list_stack: {list_stack:?}");
                let marker_and_pad = list_stack.last_mut().map(|ll| {
                    let marker = match ll.list_kind {
                        ListKind::Unordered => {
                            ul_markers.get(index).cloned()
                                .unwrap_or_else(|| BULLET.into()) // default to bullet point
                        }
                        ListKind::Ordered => {
                            // Handle the "value" attribute, only relevant to <ol>.
                            let value_attr = node.find_attr_lc(live_id!(value));
                            let value: i32 = value_attr
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(ll.li_count);

                            // Handle the "type" attribute, only relevant to <ol>.
                            let type_attr = node.find_attr_lc(live_id!(type));
                            let numbering_type = type_attr.and_then(OrderedListType::from_type_attribute);

                            // Generate this <li> marker string using either:
                            // * the <li> element's numbering type, otherwise,
                            // * the outer <ol>'s numbering type, otherwise,
                            // * the DSL-specified numbering type for the current nesting level,
                            // * otherwise a literal "#" character, which indicates malformed HTML.
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
                
                // Now, actually emit the list item.
                // log!("marker: {marker}, pad: {pad}");
                cx.turtle_new_line();
                tf.begin_list_item(cx, marker, pad);
            }
            Some(x) => { return Some(x) }
            _ => ()
        } 
        None
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
                tf.pop_size();
                tf.pop_bold();
                cx.turtle_new_line();
            }
            some_id!(b)
            | some_id!(strong) => tf.pop_bold(),
            some_id!(i)
            | some_id!(em) => tf.pop_italic(),
            some_id!(p) => {
                cx.turtle_new_line();
                cx.turtle_new_line();
            }
            some_id!(blockquote) => tf.end_quote(cx),
            some_id!(code) => {
                tf.pop_size();
                tf.pop_fixed();
                // tf.end_inline_code(cx); // doesn't work properly
            }
            some_id!(pre) => {
                tf.pop_fixed();
                tf.end_code(cx);     
            }
            some_id!(sub)
            | some_id!(sup) => {
                tf.pop_size();
            }
            some_id!(ul)
            | some_id!(ol) => {
                list_stack.pop();
            }
            some_id!(li) => tf.end_list_item(cx),
            some_id!(u) => tf.pop_underline(),
            some_id!(del)
            | some_id!(s)
            | some_id!(strike) => tf.pop_strikethrough(),
            _ => ()
        }
        None
    }
    
    pub fn handle_text_node(cx: &mut Cx2d, tf: &mut TextFlow, node: &mut HtmlWalker) -> bool {
        if let Some(text) = node.text() {
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
        // log!("HTML WIDGET EVENT: {:?}", event);
        self.text_flow.handle_event(cx, event, scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let tf = &mut self.text_flow;
        tf.begin(cx, walk);
        // alright lets iterate the html doc and draw it
        let mut node = self.doc.new_walker();
        let mut auto_id = 0;
        while !node.done() {
            match Self::handle_open_tag(cx, tf, &mut node, &mut self.list_stack, &self.ul_markers, &self.ol_markers, &self.ol_separator) {
                Some(_)=>{
                    handle_custom_widget(cx, scope, tf, &mut node, &mut auto_id); 
                }
                _=>()
            }
            match Self::handle_close_tag(cx, tf, &mut node, &mut self.list_stack) {
                _ => ()
            }
            Self::handle_text_node(cx, tf, &mut node);
            node.walk();
        }
        tf.end(cx);
        DrawStep::done()
    }  
     
    fn text(&self) -> String {
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


fn handle_custom_widget(
    cx: &mut Cx2d,
    scope: &mut Scope,
    tf: &mut TextFlow,
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
    // Obtain all attributes for the current open tag so we can pass them
    // to the new widget item via the Scope `props` field.
    // The custom widget can access these attributes in `LiveHook::after_apply()`.
    let attrs = node.collect_attributes();
    let mut scope_with_attrs = Scope::with_props(&attrs);
    // log!("FOUND CUSTOM WIDGET! template: {template:?}, id: {id:?}, attrs: {attrs:?}");

    if let Some(item) = tf.item_with_scope(cx, &mut scope_with_attrs, id, template) {
        item.set_text(node.find_text().unwrap_or(""));
        item.draw_all(cx, scope);
    }

    node.jump_to_close();
}



#[derive(Live, Widget)]
struct HtmlLink {
    #[deref] link: LinkLabel,
    #[live] href: String,
}

impl LiveHook for HtmlLink {
    // After an HtmlLink instance has been instantiated ("applied"),
    // populate its struct fields from the `<a>` tag's attributes.
    fn after_apply(&mut self, _cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        log!("HtmlLink::after_apply(): apply.from: {:?}, apply.scope exists: {:?}", apply.from, apply.scope.is_some());
        match apply.from {
            ApplyFrom::New | ApplyFrom::NewFromDoc {..} | ApplyFrom::UpdateFromDoc {..} => {
                let scope_attrs: Option<&Vec<HtmlAttribute>> = apply.scope.as_ref()
                    .and_then(|scope| scope.props.get());
                log!("HtmlLink::after_apply(): SCOPE ATTRS: {:?}", scope_attrs);
                if let Some(attrs) = scope_attrs {
                    if let Some(html_attr) = attrs.iter().find(|attr| attr.lc == live_id!(href)) {
                        log!("HtmlLink::after_apply(): found href attr: {:?}", html_attr);
                        self.href = String::from(&html_attr.value)
                    }
                }
            }
            _ => ()
        }
    }
}

impl MatchEvent for HtmlLink {
    fn handle_actions(&mut self, _cx: &mut Cx, actions: &Actions) {
        if self.link.clicked(actions) {
            log!("HtmlLink::handle_actions(): clicked! href: {:?}", self.href);
        }

        if self.link.released(actions) {
            log!("HtmlLink::handle_actions(): released! href: {:?}", self.href);
        }
    }
}

impl Widget for HtmlLink {
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
    ) {
        log!("HtmlLink::handle_event(): event: {:?}", event);
        self.link.handle_event(cx, event, scope);
        self.match_event(cx, event)
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.link.draw_walk(cx, scope, walk)
    }
    
    fn text(&self)->String{
        self.link.text()
    }
    
    fn set_text(&mut self, v:&str){
        self.link.set_text(v);
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
#[derive(Copy, Clone, Debug, Live, LiveHook, LiveRegister)]
pub enum OrderedListType {
    #[pick] // must be the top-most attribute
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
impl Default for OrderedListType {
    fn default() -> Self {
        OrderedListType::Numbers
    }
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
            // TODO: fix alpha implementations
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
