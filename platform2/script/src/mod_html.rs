use crate::handle::*;
use crate::heap::*;
use crate::makepad_live_id::live_id::*;
use crate::makepad_live_id_macros::*;
use crate::native::*;
use crate::value::*;
use crate::*;
use makepad_html::{parse_html, HtmlNode};
use makepad_live_id::live_id::InternLiveId;
use std::rc::Rc;

// Shared backing store — the parsed HTML data is Rc'd so query results
// just reference ranges into it without copying strings or nodes.
struct HtmlBacking {
    decoded: String,
    nodes: Vec<HtmlNode>,
}

pub struct ScriptHtmlDoc {
    backing: Rc<HtmlBacking>,
    // Element ranges as (open_tag_index, close_tag_index) into backing.nodes.
    ranges: Vec<(u32, u32)>,
    // true = root document (empty ranges means whole doc), false = query result (empty = no matches)
    is_root: bool,
    handle: ScriptHandle,
}

impl ScriptHandleGc for ScriptHtmlDoc {
    fn gc(&mut self) {}
    fn set_handle(&mut self, handle: ScriptHandle) {
        self.handle = handle;
    }
}

impl ScriptHtmlDoc {
    fn decoded(&self) -> &str {
        &self.backing.decoded
    }

    fn nodes(&self) -> &[HtmlNode] {
        &self.backing.nodes
    }
}

// ---- Query parsing ----

enum Combinator {
    Descendant,
    Child,
}

enum Terminal {
    None,
    Attr(LiveId),
    Text,
    Index(usize),
}

struct QueryStep {
    tag: Option<LiveId>,
    id_filter: Option<LiveId>,
    class_filter: Option<LiveId>,
    combinator: Combinator,
}

struct ParsedQuery {
    steps: Vec<QueryStep>,
    terminal: Terminal,
}

fn parse_query(sel: &str) -> ParsedQuery {
    let sel = sel.trim();
    let mut steps = Vec::new();
    let mut terminal = Terminal::None;
    let mut next_combinator = Combinator::Descendant;

    let mut i = 0;
    let bytes = sel.as_bytes();

    while i < bytes.len() {
        // Skip whitespace
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }

        if bytes[i] == b'>' {
            next_combinator = Combinator::Child;
            i += 1;
            continue;
        }

        // Read a token (until whitespace or >)
        let tok_start = i;
        while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'>' {
            i += 1;
        }
        let token = &sel[tok_start..i];

        let mut tag_part = token;
        let mut id_filter = None;
        let mut class_filter = None;

        // @attr terminal
        if let Some(at) = tag_part.find('@') {
            terminal = Terminal::Attr(LiveId::from_str_lc(&tag_part[at + 1..]));
            tag_part = &tag_part[..at];
            if tag_part.is_empty() {
                tag_part = "*";
            }
        }

        // [N] index terminal
        if let Some(b) = tag_part.find('[') {
            if let Some(e) = tag_part.find(']') {
                if let Ok(idx) = tag_part[b + 1..e].parse::<usize>() {
                    terminal = Terminal::Index(idx);
                }
                tag_part = &tag_part[..b];
            }
        }

        // #id filter
        if let Some(h) = tag_part.find('#') {
            id_filter = Some(LiveId::from_str_lc(&tag_part[h + 1..]));
            tag_part = &tag_part[..h];
            if tag_part.is_empty() {
                tag_part = "*";
            }
        }

        // .class or .text
        if let Some(d) = tag_part.find('.') {
            let suffix = &tag_part[d + 1..];
            if suffix == "text" {
                terminal = Terminal::Text;
            } else {
                class_filter = Some(LiveId::from_str_lc(suffix));
            }
            tag_part = &tag_part[..d];
            if tag_part.is_empty() {
                tag_part = "*";
            }
        }

        let tag = if tag_part == "*" {
            None
        } else {
            Some(LiveId::from_str_lc(tag_part))
        };

        steps.push(QueryStep {
            tag,
            id_filter,
            class_filter,
            combinator: std::mem::replace(&mut next_combinator, Combinator::Descendant),
        });
    }

    ParsedQuery { steps, terminal }
}

// ---- Query execution (zero-copy, operates on backing) ----

fn find_close_tag(nodes: &[HtmlNode], open_idx: usize) -> usize {
    let mut depth = 0u32;
    for i in (open_idx + 1)..nodes.len() {
        match &nodes[i] {
            HtmlNode::OpenTag { .. } => depth += 1,
            HtmlNode::CloseTag { .. } => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    nodes.len().saturating_sub(1)
}

fn element_matches(decoded: &str, nodes: &[HtmlNode], idx: usize, step: &QueryStep) -> bool {
    let lc = match &nodes[idx] {
        HtmlNode::OpenTag { lc, .. } => *lc,
        _ => return false,
    };
    if let Some(tag_id) = step.tag {
        if lc != tag_id {
            return false;
        }
    }
    if step.id_filter.is_none() && step.class_filter.is_none() {
        return true;
    }

    // Scan attributes (they immediately follow the open tag)
    let mut id_ok = step.id_filter.is_none();
    let mut class_ok = step.class_filter.is_none();
    for j in (idx + 1)..nodes.len() {
        match &nodes[j] {
            HtmlNode::Attribute { lc, start, end, .. } => {
                if !id_ok && *lc == live_id!(id) {
                    if let Some(ref want) = step.id_filter {
                        id_ok = LiveId::from_str_lc(&decoded[*start..*end]) == *want;
                    }
                }
                if !class_ok && *lc == live_id!(class) {
                    if let Some(ref want) = step.class_filter {
                        let cls = &decoded[*start..*end];
                        class_ok = cls
                            .split_whitespace()
                            .any(|c| LiveId::from_str_lc(c) == *want);
                    }
                }
            }
            _ => break,
        }
    }
    id_ok && class_ok
}

fn find_elements(
    decoded: &str,
    nodes: &[HtmlNode],
    step: &QueryStep,
    range_start: usize,
    range_end: usize,
    recurse: bool,
    out: &mut Vec<(u32, u32)>,
) {
    let mut i = range_start;
    let mut depth = 0u32;
    while i < range_end {
        match &nodes[i] {
            HtmlNode::OpenTag { .. } => {
                if (recurse || depth == 0) && element_matches(decoded, nodes, i, step) {
                    let close = find_close_tag(nodes, i);
                    out.push((i as u32, close as u32));
                    if !recurse {
                        i = close + 1;
                        continue;
                    }
                }
                depth += 1;
            }
            HtmlNode::CloseTag { .. } => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn execute_query(
    decoded: &str,
    nodes: &[HtmlNode],
    ranges: &[(u32, u32)],
    steps: &[QueryStep],
) -> Vec<(u32, u32)> {
    if steps.is_empty() {
        return ranges.to_vec();
    }

    // First step: search within provided ranges
    let mut matches = Vec::new();
    if ranges.is_empty() {
        // Whole document
        find_elements(
            decoded,
            nodes,
            &steps[0],
            0,
            nodes.len(),
            true,
            &mut matches,
        );
    } else {
        for &(s, e) in ranges {
            find_elements(
                decoded,
                nodes,
                &steps[0],
                s as usize,
                e as usize,
                true,
                &mut matches,
            );
        }
    }

    // Subsequent steps
    for step in &steps[1..] {
        let prev = std::mem::take(&mut matches);
        let is_child = matches!(step.combinator, Combinator::Child);
        for &(s, e) in &prev {
            let child_start = s as usize + 1;
            if child_start < e as usize {
                find_elements(
                    decoded,
                    nodes,
                    step,
                    child_start,
                    e as usize,
                    !is_child,
                    &mut matches,
                );
            }
        }
    }

    // Deduplicate (same element can be found via multiple ancestor paths)
    matches.sort_unstable();
    matches.dedup();

    matches
}

fn collect_text<'a>(decoded: &'a str, nodes: &[HtmlNode], start: usize, end: usize) -> &'a str {
    // Fast path: if there's exactly one text node, return a slice (no alloc)
    let mut first_text: Option<(usize, usize)> = None;
    let mut count = 0;
    for i in start..=end.min(nodes.len().saturating_sub(1)) {
        if let HtmlNode::Text {
            start: s,
            end: e,
            all_ws,
        } = &nodes[i]
        {
            if !all_ws {
                count += 1;
                if count == 1 {
                    first_text = Some((*s, *e));
                }
            }
        }
    }
    if count == 1 {
        if let Some((s, e)) = first_text {
            return &decoded[s..e];
        }
    }
    // For zero or multiple text nodes we'll need the caller to handle it
    ""
}

fn collect_text_owned(decoded: &str, nodes: &[HtmlNode], start: usize, end: usize) -> String {
    let mut text = String::new();
    for i in start..=end.min(nodes.len().saturating_sub(1)) {
        if let HtmlNode::Text {
            start: s,
            end: e,
            all_ws,
        } = &nodes[i]
        {
            if !all_ws || (text.is_empty() && *all_ws) {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&decoded[*s..*e]);
            }
        }
    }
    text
}

fn get_attr<'a>(
    decoded: &'a str,
    nodes: &[HtmlNode],
    open_idx: usize,
    attr_id: LiveId,
) -> Option<&'a str> {
    for j in (open_idx + 1)..nodes.len() {
        match &nodes[j] {
            HtmlNode::Attribute { lc, start, end, .. } if *lc == attr_id => {
                return Some(&decoded[*start..*end]);
            }
            HtmlNode::OpenTag { .. } | HtmlNode::CloseTag { .. } | HtmlNode::Text { .. } => break,
            _ => {}
        }
    }
    None
}

fn count_top_level_elements(nodes: &[HtmlNode]) -> usize {
    let mut count = 0;
    let mut depth = 0u32;
    for node in nodes {
        match node {
            HtmlNode::OpenTag { .. } => {
                if depth == 0 {
                    count += 1;
                }
                depth += 1;
            }
            HtmlNode::CloseTag { .. } => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    count
}

fn top_level_ranges(nodes: &[HtmlNode]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < nodes.len() {
        if let HtmlNode::OpenTag { .. } = &nodes[i] {
            let close = find_close_tag(nodes, i);
            out.push((i as u32, close as u32));
            i = close + 1;
        } else {
            i += 1;
        }
    }
    out
}

pub fn define_html_module(heap: &mut ScriptHeap, native: &mut ScriptNative) {
    let html_type = native.new_handle_type(heap, id!(html));

    // "string".parse_html() -> html handle
    native.add_type_method(
        heap,
        ScriptValueType::REDUX_STRING,
        id!(parse_html),
        &[],
        move |vm, args| {
            let sself = script_value!(vm, args.self);
            let doc =
                if let Some(d) = vm.bx.heap.string_mut_self_with(sself, |_heap, s| {
                    parse_html(s, &mut None, InternLiveId::No)
                }) {
                    d
                } else {
                    return script_err_unexpected!(
                        vm.bx.threads.cur_ref().trap,
                        "parse_html called on non-string value"
                    );
                };
            let backing = Rc::new(HtmlBacking {
                decoded: doc.decoded,
                nodes: doc.nodes,
            });
            let html_doc = ScriptHtmlDoc {
                backing,
                ranges: Vec::new(),
                is_root: true,
                handle: ScriptHandle::ZERO,
            };
            vm.bx.heap.new_handle(html_type, Box::new(html_doc)).into()
        },
    );

    // html.query(selector) -> html handle, string, or array
    native.add_type_method(
        heap,
        html_type.to_redux(),
        id!(query),
        &[(id!(sel), NIL)],
        move |vm, args| {
            let sself = script_value!(vm, args.self);
            let sel_val = script_value!(vm, args.sel);

            let sel_str = if let Some(r) = vm.bx.heap.string_with(sel_val, |_, s| s.to_string()) {
                r
            } else {
                return script_err_type_mismatch!(
                    vm.bx.threads.cur_ref().trap,
                    "query() selector must be a string"
                );
            };

            let handle = if let Some(h) = sself.as_handle() {
                h
            } else {
                return NIL;
            };

            let parsed = parse_query(&sel_str);

            // Extract Rc backing + ranges (cheap clone of Rc + small vec)
            let (backing, ranges, is_root) =
                if let Some(doc) = vm.bx.heap.handle_ref::<ScriptHtmlDoc>(handle) {
                    (doc.backing.clone(), doc.ranges.clone(), doc.is_root)
                } else {
                    return NIL;
                };

            // Empty non-root = no matches from previous query, skip
            if !is_root && ranges.is_empty() {
                let sub = ScriptHtmlDoc {
                    backing: backing.clone(),
                    ranges: Vec::new(),
                    is_root: false,
                    handle: ScriptHandle::ZERO,
                };
                return vm.bx.heap.new_handle(html_type, Box::new(sub)).into();
            }

            let matches = execute_query(&backing.decoded, &backing.nodes, &ranges, &parsed.steps);

            match parsed.terminal {
                Terminal::Attr(attr_id) => {
                    if matches.len() == 1 {
                        if let Some(val) = get_attr(
                            &backing.decoded,
                            &backing.nodes,
                            matches[0].0 as usize,
                            attr_id,
                        ) {
                            return vm.bx.heap.new_string_from_str(val);
                        }
                        return NIL;
                    }
                    let arr = vm.bx.heap.new_array();
                    let trap = vm.bx.threads.cur_ref().trap.pass();
                    for &(s, _e) in &matches {
                        if let Some(val) =
                            get_attr(&backing.decoded, &backing.nodes, s as usize, attr_id)
                        {
                            let sv = vm.bx.heap.new_string_from_str(val);
                            vm.bx.heap.array_push(arr, sv, trap);
                        } else {
                            vm.bx.heap.array_push(arr, NIL, trap);
                        }
                    }
                    arr.into()
                }
                Terminal::Text => {
                    if matches.len() == 1 {
                        let slice = collect_text(
                            &backing.decoded,
                            &backing.nodes,
                            matches[0].0 as usize,
                            matches[0].1 as usize,
                        );
                        if !slice.is_empty() {
                            return vm.bx.heap.new_string_from_str(slice);
                        }
                        let owned = collect_text_owned(
                            &backing.decoded,
                            &backing.nodes,
                            matches[0].0 as usize,
                            matches[0].1 as usize,
                        );
                        return vm.bx.heap.new_string_from_str(&owned);
                    }
                    let arr = vm.bx.heap.new_array();
                    let trap = vm.bx.threads.cur_ref().trap.pass();
                    for &(s, e) in &matches {
                        let slice =
                            collect_text(&backing.decoded, &backing.nodes, s as usize, e as usize);
                        let sv = if !slice.is_empty() {
                            vm.bx.heap.new_string_from_str(slice)
                        } else {
                            let owned = collect_text_owned(
                                &backing.decoded,
                                &backing.nodes,
                                s as usize,
                                e as usize,
                            );
                            vm.bx.heap.new_string_from_str(&owned)
                        };
                        vm.bx.heap.array_push(arr, sv, trap);
                    }
                    arr.into()
                }
                Terminal::Index(idx) => {
                    if idx < matches.len() {
                        let sub = ScriptHtmlDoc {
                            backing: backing.clone(),
                            ranges: vec![matches[idx]],
                            is_root: false,
                            handle: ScriptHandle::ZERO,
                        };
                        vm.bx.heap.new_handle(html_type, Box::new(sub)).into()
                    } else {
                        NIL
                    }
                }
                Terminal::None => {
                    let sub = ScriptHtmlDoc {
                        backing: backing.clone(),
                        ranges: matches,
                        is_root: false,
                        handle: ScriptHandle::ZERO,
                    };
                    vm.bx.heap.new_handle(html_type, Box::new(sub)).into()
                }
            }
        },
    );

    // html.attr(name) -> string or nil
    native.add_type_method(
        heap,
        html_type.to_redux(),
        id!(attr),
        &[(id!(name), NIL)],
        move |vm, args| {
            let sself = script_value!(vm, args.self);
            let name_val = script_value!(vm, args.name);
            let attr_name = if let Some(r) = vm.bx.heap.string_with(name_val, |_, s| s.to_string())
            {
                r
            } else {
                return script_err_type_mismatch!(
                    vm.bx.threads.cur_ref().trap,
                    "attr() name must be a string"
                );
            };
            let handle = if let Some(h) = sself.as_handle() {
                h
            } else {
                return NIL;
            };
            let attr_id = LiveId::from_str_lc(&attr_name);
            let result = if let Some(doc) = vm.bx.heap.handle_ref::<ScriptHtmlDoc>(handle) {
                let open_idx = if doc.is_root && doc.ranges.is_empty() {
                    doc.nodes()
                        .iter()
                        .position(|n| matches!(n, HtmlNode::OpenTag { .. }))
                } else if !doc.ranges.is_empty() {
                    Some(doc.ranges[0].0 as usize)
                } else {
                    None
                };
                open_idx.and_then(|idx| {
                    get_attr(doc.decoded(), doc.nodes(), idx, attr_id).map(|v| v.to_string())
                })
            } else {
                None
            };
            if let Some(val) = result {
                return vm.bx.heap.new_string_from_str(&val);
            }
            NIL
        },
    );

    // html.array() -> array of html handles (one per top-level element)
    native.add_type_method(
        heap,
        html_type.to_redux(),
        id!(array),
        &[],
        move |vm, args| {
            let sself = script_value!(vm, args.self);
            let handle = if let Some(h) = sself.as_handle() {
                h
            } else {
                return NIL;
            };

            let (backing, ranges) =
                if let Some(doc) = vm.bx.heap.handle_ref::<ScriptHtmlDoc>(handle) {
                    let r = if doc.is_root && doc.ranges.is_empty() {
                        top_level_ranges(doc.nodes())
                    } else if !doc.ranges.is_empty() {
                        doc.ranges.clone()
                    } else {
                        Vec::new()
                    };
                    (doc.backing.clone(), r)
                } else {
                    return NIL;
                };

            let arr = vm.bx.heap.new_array();
            let trap = vm.bx.threads.cur_ref().trap.pass();
            for range in &ranges {
                let sub = ScriptHtmlDoc {
                    backing: backing.clone(),
                    ranges: vec![*range],
                    is_root: false,
                    handle: ScriptHandle::ZERO,
                };
                let h = vm.bx.heap.new_handle(html_type, Box::new(sub));
                vm.bx.heap.array_push(arr, h.into(), trap);
            }
            arr.into()
        },
    );

    // Getters: length, text, html
    native.set_type_getter(html_type.to_redux(), |vm, value, field| {
        let handle = if let Some(h) = value.as_handle() {
            h
        } else {
            return NIL;
        };

        if field == id!(length) {
            if let Some(doc) = vm.bx.heap.handle_ref::<ScriptHtmlDoc>(handle) {
                if !doc.is_root || !doc.ranges.is_empty() {
                    return ScriptValue::from_f64(doc.ranges.len() as f64);
                }
                return ScriptValue::from_f64(count_top_level_elements(doc.nodes()) as f64);
            }
            return NIL;
        }

        // For text and html we extract the string, drop the borrow, then allocate
        let result = if let Some(doc) = vm.bx.heap.handle_ref::<ScriptHtmlDoc>(handle) {
            if field == id!(text) {
                let mut text = String::new();
                if doc.is_root && doc.ranges.is_empty() {
                    text = collect_text_owned(
                        doc.decoded(),
                        doc.nodes(),
                        0,
                        doc.nodes().len().saturating_sub(1),
                    );
                } else {
                    for &(s, e) in &doc.ranges {
                        let t =
                            collect_text_owned(doc.decoded(), doc.nodes(), s as usize, e as usize);
                        if !t.is_empty() {
                            if !text.is_empty() {
                                text.push(' ');
                            }
                            text.push_str(&t);
                        }
                    }
                }
                Some(text)
            } else if field == id!(html) {
                let html = if doc.is_root && doc.ranges.is_empty() {
                    reconstruct_html(doc.decoded(), doc.nodes(), 0, doc.nodes().len())
                } else {
                    let mut out = String::new();
                    for &(s, e) in &doc.ranges {
                        reconstruct_html_into(
                            &mut out,
                            doc.decoded(),
                            doc.nodes(),
                            s as usize,
                            e as usize + 1,
                        );
                    }
                    out
                };
                Some(html)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(s) = result {
            return vm.bx.heap.new_string_from_str(&s);
        }
        NIL
    });
}

fn reconstruct_html(decoded: &str, nodes: &[HtmlNode], start: usize, end: usize) -> String {
    let mut out = String::new();
    reconstruct_html_into(&mut out, decoded, nodes, start, end);
    out
}

fn reconstruct_html_into(
    out: &mut String,
    decoded: &str,
    nodes: &[HtmlNode],
    start: usize,
    end: usize,
) {
    let mut i = start;
    while i < end && i < nodes.len() {
        match &nodes[i] {
            HtmlNode::OpenTag { nc, .. } => {
                out.push('<');
                out.push_str(&format!("{}", nc));
                let mut j = i + 1;
                while j < end && j < nodes.len() {
                    if let HtmlNode::Attribute { nc, start, end, .. } = &nodes[j] {
                        out.push(' ');
                        out.push_str(&format!("{}", nc));
                        let val = &decoded[*start..*end];
                        if !val.is_empty() {
                            out.push_str("=\"");
                            out.push_str(val);
                            out.push('"');
                        }
                        j += 1;
                    } else {
                        break;
                    }
                }
                out.push('>');
            }
            HtmlNode::CloseTag { nc, .. } => {
                out.push_str("</");
                out.push_str(&format!("{}", nc));
                out.push('>');
            }
            HtmlNode::Text { start, end, .. } => {
                out.push_str(&decoded[*start..*end]);
            }
            HtmlNode::Attribute { .. } => {}
        }
        i += 1;
    }
}
