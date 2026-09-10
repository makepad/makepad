use makepad_live_id::*;

#[derive(Debug)]
pub struct HtmlError {
    pub message: String,
    pub position: usize,
}

#[derive(Default, PartialEq)]
pub struct HtmlDoc {
    pub decoded: String,
    pub nodes: Vec<HtmlNode>,
}

#[derive(Debug, PartialEq)]
pub enum HtmlNode {
    OpenTag {
        lc: LiveId,
        nc: LiveId,
    },
    CloseTag {
        lc: LiveId,
        nc: LiveId,
    },
    Attribute {
        lc: LiveId,
        nc: LiveId,
        start: usize,
        end: usize,
    },
    Text {
        start: usize,
        end: usize,
        all_ws: bool,
    },
}
/*
/// A standalone owned copy of an HTML attribute.
#[derive(Debug, Clone)]
pub struct HtmlAttribute {
    /// The LiveID of this attribute's key converted to lowercase.
    pub lc: LiveId,
    /// The LiveID of this attribute's key in its original normal case.
    pub nc: LiveId,
    /// The value of this attribute.
    pub value: String,
}*/

pub struct HtmlWalker<'a> {
    decoded: &'a str,
    pub nodes: &'a [HtmlNode],
    pub index: usize,
}

impl<'a> HtmlWalker<'a> {
    pub fn index(&self) -> usize {
        self.index
    }

    pub fn walk(&mut self) {
        if self.index < self.nodes.len() {
            for i in self.index + 1..self.nodes.len() {
                // we skip attributes
                if let HtmlNode::Attribute { .. } = &self.nodes[i] {
                } else {
                    self.index = i;
                    return;
                }
            }
            self.index = self.nodes.len();
        }
    }

    /// Advances to the close tag matching the open tag the walker is on.
    ///
    /// Only tags with the *same* id affect nesting depth. Counting every
    /// open tag would over-consume, because a void element written without a
    /// slash (`<br>`, `<img src=x>`, `<hr>`) emits an `OpenTag` with no
    /// matching `CloseTag`, so each one would leave the depth permanently
    /// unbalanced and swallow the rest of the document.
    ///
    /// The walker does not move when the element has no close tag of its own
    /// — it is void, or unclosed — so the caller's own `walk()` still visits
    /// the following nodes and any enclosing close tag is still delivered.
    pub fn jump_to_close(&mut self) {
        let Some(open_lc) = self.open_tag_lc() else {
            return;
        };
        let mut depth = 0u32;
        for i in self.index + 1..self.nodes.len() {
            match &self.nodes[i] {
                HtmlNode::OpenTag { lc, .. } if *lc == open_lc => {
                    depth += 1;
                }
                HtmlNode::CloseTag { lc, .. } if *lc == open_lc => {
                    if depth == 0 {
                        self.index = i;
                        return;
                    }
                    depth -= 1;
                }
                _ => (),
            }
        }
    }

    pub fn done(&self) -> bool {
        self.index >= self.nodes.len()
    }
    /*
    /// Iterates over and returns a list of all attributes for the current open HTML tag.
    pub fn collect_attributes(&self) -> Vec<HtmlAttribute> {
        let mut attrs = Vec::new();
        for node in &self.nodes[self.index ..] {
            match node {
                HtmlNode::Attribute { lc, nc, start, end } => {
                    attrs.push(HtmlAttribute {
                        lc: *lc,
                        nc: *nc,
                        value: String::from(&self.decoded[*start..*end]),
                    });
                }
                HtmlNode::CloseTag { .. } => break,
                _ => continue,
            }
        }
        attrs
    }
    */
    /// Returns the first attribute of the currently-opened Html tag
    /// whose key matches the given `flc` LiveId, which should be all lowercase.
    ///
    /// Matching is done after converting all attribute keys to lowercase.
    pub fn find_attr_lc(&self, flc: LiveId) -> Option<&'a str> {
        for i in (self.index + 1)..self.nodes.len() {
            match &self.nodes[i] {
                HtmlNode::OpenTag { .. } | HtmlNode::CloseTag { .. } => return None,
                HtmlNode::Attribute {
                    lc,
                    nc: _,
                    start,
                    end,
                } if *lc == flc => return Some(&self.decoded[*start..*end]),
                _ => (),
            }
        }
        None
    }

    pub fn while_attr_lc(&mut self) -> Option<(LiveId, &'a str)> {
        if self.index < self.nodes.len() {
            match &self.nodes[self.index] {
                HtmlNode::Attribute {
                    lc,
                    nc: _,
                    start,
                    end,
                } => {
                    self.index += 1;
                    return Some((*lc, &self.decoded[*start..*end]));
                }
                _ => (),
            }
        }
        None
    }

    /// Returns the first attribute of the currently-opened Html tag
    /// whose key matches the given `fnc` LiveId, which is case-sensitive.
    ///
    /// Matching is done in a case-sensitive manner.
    pub fn find_attr_nc(&self, fnc: LiveId) -> Option<&'a str> {
        for i in (self.index + 1)..self.nodes.len() {
            match &self.nodes[i] {
                HtmlNode::OpenTag { .. } | HtmlNode::CloseTag { .. } => return None,
                HtmlNode::Attribute {
                    lc: _,
                    nc,
                    start,
                    end,
                } if *nc == fnc => return Some(&self.decoded[*start..*end]),
                _ => (),
            }
        }
        None
    }

    /// Returns the first non-empty text at or after the current position,
    /// stopping at the first close tag.
    ///
    /// The parser emits a zero-length `Text` node in front of every tag, so
    /// those are skipped: otherwise `<a href="x"><b>label</b></a>` reports no
    /// text at all rather than `label`.
    pub fn find_text(&self) -> Option<&'a str> {
        for i in self.index..self.nodes.len() {
            match &self.nodes[i] {
                HtmlNode::CloseTag { .. } => return None,
                HtmlNode::Text { start, end, .. } if start != end => {
                    return Some(&self.decoded[*start..*end]);
                }
                _ => (),
            }
        }
        None
    }

    /// Returns the text directly inside the next `tag` open tag at or after
    /// the current position. `tag` is matched against the lowercased tag id,
    /// since HTML tag names are case-insensitive.
    pub fn find_tag_text(&self, tag: LiveId) -> Option<&'a str> {
        for i in self.index..self.nodes.len() {
            match &self.nodes[i] {
                HtmlNode::OpenTag { lc, .. } if *lc == tag => {
                    // Attributes sit between the open tag and its text, and
                    // the parser emits a zero-length text node before any
                    // nested markup; skip both to reach the real content.
                    for candidate in &self.nodes[i + 1..] {
                        match candidate {
                            HtmlNode::Attribute { .. } => (),
                            HtmlNode::Text { start, end, .. } if start == end => (),
                            HtmlNode::Text { start, end, .. } => {
                                return Some(&self.decoded[*start..*end]);
                            }
                            _ => break,
                        }
                    }
                }
                _ => (),
            }
        }
        None
    }

    pub fn text(&self) -> Option<&'a str> {
        match self.nodes.get(self.index) {
            Some(HtmlNode::Text { start, end, .. }) => Some(&self.decoded[*start..*end]),
            _ => None,
        }
    }

    pub fn text_is_all_ws(&self) -> bool {
        match self.nodes.get(self.index) {
            Some(HtmlNode::Text { all_ws, .. }) => *all_ws,
            _ => false,
        }
    }

    pub fn open_tag_lc(&self) -> Option<LiveId> {
        match self.nodes.get(self.index) {
            Some(HtmlNode::OpenTag { lc, nc: _ }) => Some(*lc),
            _ => None,
        }
    }
    pub fn open_tag_nc(&self) -> Option<LiveId> {
        match self.nodes.get(self.index) {
            Some(HtmlNode::OpenTag { lc: _, nc }) => Some(*nc),
            _ => None,
        }
    }
    pub fn open_tag(&self) -> Option<(LiveId, LiveId)> {
        match self.nodes.get(self.index) {
            Some(HtmlNode::OpenTag { lc, nc }) => Some((*lc, *nc)),
            _ => None,
        }
    }

    pub fn close_tag_lc(&self) -> Option<LiveId> {
        match self.nodes.get(self.index) {
            Some(HtmlNode::CloseTag { lc, nc: _ }) => Some(*lc),
            _ => None,
        }
    }
    pub fn close_tag_nc(&self) -> Option<LiveId> {
        match self.nodes.get(self.index) {
            Some(HtmlNode::CloseTag { lc: _, nc }) => Some(*nc),
            _ => None,
        }
    }
    pub fn close_tag(&self) -> Option<(LiveId, LiveId)> {
        match self.nodes.get(self.index) {
            Some(HtmlNode::CloseTag { lc, nc }) => Some((*lc, *nc)),
            _ => None,
        }
    }
}

impl HtmlDoc {
    pub fn new_walker(&self) -> HtmlWalker<'_> {
        HtmlWalker {
            decoded: &self.decoded,
            index: 0,
            nodes: &self.nodes,
        }
    }

    pub fn new_walker_with_index(&self, index: usize) -> HtmlWalker<'_> {
        HtmlWalker {
            decoded: &self.decoded,
            index,
            nodes: &self.nodes,
        }
    }
}

/// The five characters HTML treats as whitespace.
///
/// `char::is_whitespace` follows Unicode, which also covers U+00A0 and
/// U+3000. Using it here collapsed `&nbsp;` away and ate the full-width
/// spaces in CJK text, neither of which HTML permits.
fn is_html_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}

pub fn parse_html(
    body: &str,
    errors: &mut Option<Vec<HtmlError>>,
    intern: InternLiveId,
) -> HtmlDoc {
    enum State {
        Text {
            start: usize,
            dec_start: usize,
            last_non_whitespace: usize,
            collapse_ws: bool,
        },
        ElementName(usize),
        ElementClose(usize),
        ElementAttrs,
        ElementCloseScanSpaces,
        /// Discarding a malformed tag up to its `>`.
        BogusTag,
        ElementSelfClose,
        AttribName(usize),
        AttribValueEq(LiveId, LiveId),
        AttribValueStart(LiveId, LiveId),
        AttribValueSq(LiveId, LiveId, usize),
        AttribValueDq(LiveId, LiveId, usize),
        AttribValueBare(LiveId, LiveId, usize),
        AttribValueBareSlash(LiveId, LiveId, usize),
        CommentStartDash1,
        CommentStartDash2,
        CommentEndDash,
        CommentEnd,
        DocType,
        HeaderQuestion,
        HeaderAngle,
        CommentBody,
    }

    /// The longest name in `match_entity` is `DownLeftRightVector`. A run of
    /// characters longer than this cannot become a named entity, so scanning
    /// stops there rather than buffering unbounded text.
    const MAX_ENTITY_NAME: usize = "DownLeftRightVector".len();
    /// Numeric references (`&#38;`, `&#x26;`) may be zero-padded, so they get
    /// a longer budget than a name would.
    const MAX_ENTITY_NUMERIC: usize = 32;

    /// State for an entity currently being scanned.
    #[derive(Copy, Clone)]
    struct InEntity {
        /// Index in `decoded` of the opening `&`.
        start: usize,
        /// `last_non_whitespace` as it was *before* the `&` was appended.
        /// The entity's own characters are appended to `decoded` while it is
        /// being scanned and then truncated away again on a match, so the
        /// index has to be restored rather than recomputed.
        saved_last_non_whitespace: usize,
    }

    fn process_entity(
        c: char,
        in_entity: &mut Option<InEntity>,
        decoded: &mut String,
        last_non_whitespace: &mut usize,
        collapse_ws: bool,
    ) {
        if let Some(InEntity { start, saved_last_non_whitespace }) = *in_entity {
            // scan entity
            if c == ';' {
                // potential end of entity
                match match_entity(&decoded[start + 1..]) {
                    Err(_e) => {
                        *in_entity = None;
                    }
                    Ok(entity) => {
                        *in_entity = None;
                        // `match_entity` only yields Unicode scalar values,
                        // but fall back to leaving the text as-is rather than
                        // panicking if that ever stops holding.
                        if let Some(ch) = char::from_u32(entity) {
                            decoded.truncate(start);
                            decoded.push(ch);
                            *last_non_whitespace = if is_html_whitespace(ch) {
                                saved_last_non_whitespace
                            } else {
                                decoded.len()
                            };
                            return;
                        }
                    }
                }
            }
            // definitely not an entity
            else {
                let scanned = decoded.len() - start;
                let budget = if decoded.as_bytes().get(start + 1) == Some(&b'#') {
                    MAX_ENTITY_NUMERIC
                } else {
                    MAX_ENTITY_NAME
                };
                if is_html_whitespace(c) || scanned > budget {
                    *in_entity = None;
                }
            }
        }
        if c == '&' {
            *in_entity = Some(InEntity {
                start: decoded.len(),
                saved_last_non_whitespace: *last_non_whitespace,
            });
        }
        if collapse_ws && is_html_whitespace(c) {
            if *last_non_whitespace == decoded.len() {
                decoded.push(' ');
            }
        } else {
            decoded.push(c);
            if !is_html_whitespace(c) {
                *last_non_whitespace = decoded.len();
            }
        }
    }

    /// `<pre>` and `<code>` keep their whitespace verbatim.
    fn preserves_whitespace(lc: LiveId) -> bool {
        lc == live_id!(pre) || lc == live_id!(code)
    }

    let mut nodes = Vec::new();
    // How many whitespace-preserving elements are currently open. A single
    // flag would be wrong: any tag nested inside `<pre>` used to reset
    // collapsing for the rest of the element, so `<pre>a  <b>b  b</b>  c</pre>`
    // lost every space after the first text run.
    let mut pre_depth = 0usize;
    let mut state = State::Text {
        start: 0,
        dec_start: 0,
        last_non_whitespace: 0,
        collapse_ws: true,
    };
    let mut decoded = String::new();
    let mut in_entity = None;

    for (i, c) in body.char_indices() {
        state = match state {
            State::DocType => {
                if c == '>' {
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else {
                    State::DocType
                }
            }
            State::HeaderQuestion => {
                if c == '?' {
                    State::HeaderAngle
                } else {
                    State::HeaderQuestion
                }
            }
            State::HeaderAngle => {
                if c == '>' {
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else {
                    State::HeaderQuestion
                }
            }
            State::Text {
                start,
                dec_start,
                last_non_whitespace,
                collapse_ws,
            } => {
                if c == '<' {
                    if let Some(InEntity { start, .. }) = in_entity {
                        if let Some(errors) = errors {
                            errors.push(HtmlError {
                                message: "Unterminated entity".into(),
                                position: start,
                            })
                        };
                    }
                    // An entity cannot span this boundary. Dropping it here is
                    // what keeps a later `;` from truncating `decoded` back
                    // past text that has already been committed to a node.
                    in_entity = None;
                    nodes.push(HtmlNode::Text {
                        start: dec_start,
                        end: decoded.len(),
                        all_ws: dec_start == last_non_whitespace,
                    });
                    State::ElementName(i + 1)
                } else {
                    let mut last_non_whitespace = last_non_whitespace;
                    process_entity(
                        c,
                        &mut in_entity,
                        &mut decoded,
                        &mut last_non_whitespace,
                        collapse_ws,
                    );
                    State::Text {
                        start,
                        dec_start,
                        last_non_whitespace,
                        collapse_ws,
                    }
                }
            }
            State::ElementName(start) => {
                if c == '/' && i == start {
                    State::ElementClose(i + 1)
                } else if c == '!' && i == start {
                    State::CommentStartDash1
                } else if c == '?' && i == start {
                    State::HeaderQuestion
                } else if i == start && !c.is_ascii_alphabetic() {
                    // A tag name has to start with an ASCII letter. Anything
                    // else means the `<` was never markup — `5<10`, `a < b`,
                    // `<>` — so put it back as literal text instead of
                    // parsing a bogus element and losing the rest of the
                    // line to it.
                    let dec_start = decoded.len();
                    decoded.push('<');
                    let mut last_non_whitespace = decoded.len();
                    process_entity(
                        c,
                        &mut in_entity,
                        &mut decoded,
                        &mut last_non_whitespace,
                        pre_depth == 0,
                    );
                    State::Text {
                        start,
                        dec_start,
                        last_non_whitespace,
                        collapse_ws: pre_depth == 0,
                    }
                } else if is_html_whitespace(c) {
                    let lc = LiveId::from_str_lc(&body[start..i]);
                    if preserves_whitespace(lc) {
                        pre_depth += 1;
                    }
                    nodes.push(HtmlNode::OpenTag {
                        lc,
                        nc: LiveId::from_str_with_intern(&body[start..i], intern),
                    });
                    State::ElementAttrs
                } else if c == '/' {
                    let lc = LiveId::from_str_lc(&body[start..i]);
                    if preserves_whitespace(lc) {
                        pre_depth += 1;
                    }
                    nodes.push(HtmlNode::OpenTag {
                        lc,
                        nc: LiveId::from_str_with_intern(&body[start..i], intern),
                    });
                    State::ElementSelfClose
                } else if c == '>' {
                    let lc = LiveId::from_str_lc(&body[start..i]);
                    let nc = LiveId::from_str_with_intern(&body[start..i], intern);
                    if preserves_whitespace(lc) {
                        pre_depth += 1;
                    }
                    nodes.push(HtmlNode::OpenTag { lc, nc });
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else {
                    State::ElementName(start)
                }
            }
            State::ElementClose(start) => {
                if i == start && !c.is_ascii_alphabetic() {
                    // `</3`, `</ ` — not a close tag, so keep it as text.
                    let dec_start = decoded.len();
                    decoded.push('<');
                    decoded.push('/');
                    let mut last_non_whitespace = decoded.len();
                    process_entity(
                        c,
                        &mut in_entity,
                        &mut decoded,
                        &mut last_non_whitespace,
                        pre_depth == 0,
                    );
                    State::Text {
                        start,
                        dec_start,
                        last_non_whitespace,
                        collapse_ws: pre_depth == 0,
                    }
                } else if c == '>' {
                    let lc = LiveId::from_str_lc(&body[start..i]);
                    let nc = LiveId::from_str_with_intern(&body[start..i], intern);
                    if preserves_whitespace(lc) {
                        pre_depth = pre_depth.saturating_sub(1);
                    }
                    nodes.push(HtmlNode::CloseTag { lc, nc });
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else if is_html_whitespace(c) {
                    let lc = LiveId::from_str_lc(&body[start..i]);
                    if preserves_whitespace(lc) {
                        pre_depth = pre_depth.saturating_sub(1);
                    }
                    nodes.push(HtmlNode::CloseTag {
                        lc,
                        nc: LiveId::from_str_with_intern(&body[start..i], intern),
                    });
                    State::ElementCloseScanSpaces
                } else {
                    State::ElementClose(start)
                }
            }
            State::ElementCloseScanSpaces => {
                if c == '>' {
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else if !is_html_whitespace(c) {
                    if let Some(errors) = errors {
                        errors.push(HtmlError{message:"Unexpected character after whitespace whilst looking for closing tag >".into(), position:i})
                    };
                    State::BogusTag
                } else {
                    State::ElementCloseScanSpaces
                }
            }
            State::ElementSelfClose => {
                let malformed = c != '>';
                if malformed {
                    if let Some(errors) = errors {
                        errors.push(HtmlError {
                            message: "Expected > after / self closed tag".into(),
                            position: i,
                        })
                    };
                }
                // look backwards to the OpenTag
                let begin = nodes.iter().rev().find_map(|v| {
                    if let HtmlNode::OpenTag { lc, nc } = v {
                        Some((*lc, *nc))
                    } else {
                        None
                    }
                });
                // Always present in a well-formed transition into this state,
                // but a parser fed arbitrary input should not carry an
                // `unwrap` that a future edit could make reachable.
                if let Some((lc, nc)) = begin {
                    if preserves_whitespace(lc) {
                        pre_depth = pre_depth.saturating_sub(1);
                    }
                    nodes.push(HtmlNode::CloseTag { lc, nc });
                }
                if malformed {
                    // Drop the rest of the malformed tag instead of resuming
                    // text inside it, which leaked its `>` into the output.
                    State::BogusTag
                } else {
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                }
            }
            State::BogusTag => {
                if c == '>' {
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else {
                    State::BogusTag
                }
            }
            State::ElementAttrs => {
                if c == '/' {
                    //nodes.push(HtmlNode::BeginElement(HtmlId::new(&body, start, i)));
                    State::ElementSelfClose
                } else if c == '>' {
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else if !is_html_whitespace(c) {
                    State::AttribName(i)
                } else {
                    State::ElementAttrs
                }
            }
            State::AttribName(start) => {
                if is_html_whitespace(c) {
                    State::AttribValueEq(
                        LiveId::from_str_lc(&body[start..i]),
                        LiveId::from_str_with_intern(&body[start..i], intern),
                    )
                } else if c == '=' {
                    State::AttribValueStart(
                        LiveId::from_str_lc(&body[start..i]),
                        LiveId::from_str_with_intern(&body[start..i], intern),
                    )
                } else if c == '/' {
                    nodes.push(HtmlNode::Attribute {
                        lc: LiveId::from_str_lc(&body[start..i]),
                        nc: LiveId::from_str_with_intern(&body[start..i], intern),
                        start: 0,
                        end: 0,
                    });
                    State::ElementSelfClose
                } else if c == '>' {
                    nodes.push(HtmlNode::Attribute {
                        lc: LiveId::from_str_lc(&body[start..i]),
                        nc: LiveId::from_str_with_intern(&body[start..i], intern),
                        start: 0,
                        end: 0,
                    });
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else {
                    State::AttribName(start)
                }
            }
            State::AttribValueEq(lc, nc) => {
                if c == '/' {
                    nodes.push(HtmlNode::Attribute {
                        lc,
                        nc,
                        start: 0,
                        end: 0,
                    });
                    State::ElementSelfClose
                } else if c == '>' {
                    nodes.push(HtmlNode::Attribute {
                        lc,
                        nc,
                        start: 0,
                        end: 0,
                    });
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else if c == '=' {
                    State::AttribValueStart(lc, nc)
                } else if !is_html_whitespace(c) {
                    nodes.push(HtmlNode::Attribute {
                        lc,
                        nc,
                        start: 0,
                        end: 0,
                    });
                    State::AttribName(i)
                } else {
                    State::AttribValueEq(lc, nc)
                }
            }
            State::AttribValueStart(lc, nc) => {
                if c == '>' {
                    // An empty unquoted value: the tag ends here.
                    nodes.push(HtmlNode::Attribute {
                        lc,
                        nc,
                        start: 0,
                        end: 0,
                    });
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else if c == '\"' {
                    // double quoted attrib
                    State::AttribValueDq(lc, nc, decoded.len())
                } else if c == '\'' {
                    // single quoted attrib
                    State::AttribValueSq(lc, nc, decoded.len())
                } else if !is_html_whitespace(c) {
                    // Route the first character through `process_entity` too,
                    // otherwise a value starting with `&` never begins an
                    // entity scan and `&amp;x` decodes as literal `&amp;x`.
                    let start = decoded.len();
                    process_entity(c, &mut in_entity, &mut decoded, &mut 0, false);
                    State::AttribValueBare(lc, nc, start)
                } else {
                    State::AttribValueStart(lc, nc)
                }
            }
            State::AttribValueSq(lc, nc, start) => {
                if c == '\'' {
                    if let Some(InEntity { start, .. }) = in_entity {
                        if let Some(errors) = errors {
                            errors.push(HtmlError {
                                message: "Unterminated entity".into(),
                                position: start,
                            })
                        };
                    }
                    // An entity cannot span this boundary. Dropping it here is
                    // what keeps a later `;` from truncating `decoded` back
                    // past text that has already been committed to a node.
                    in_entity = None;
                    nodes.push(HtmlNode::Attribute {
                        lc,
                        nc,
                        start,
                        end: decoded.len(),
                    });
                    State::ElementAttrs
                } else {
                    process_entity(c, &mut in_entity, &mut decoded, &mut 0, false);
                    State::AttribValueSq(lc, nc, start)
                }
            }
            State::AttribValueDq(lc, nc, start) => {
                if c == '\"' {
                    if let Some(InEntity { start, .. }) = in_entity {
                        if let Some(errors) = errors {
                            errors.push(HtmlError {
                                message: "Unterminated entity".into(),
                                position: start,
                            })
                        };
                    }
                    // An entity cannot span this boundary. Dropping it here is
                    // what keeps a later `;` from truncating `decoded` back
                    // past text that has already been committed to a node.
                    in_entity = None;
                    nodes.push(HtmlNode::Attribute {
                        lc,
                        nc,
                        start,
                        end: decoded.len(),
                    });
                    State::ElementAttrs
                } else {
                    process_entity(c, &mut in_entity, &mut decoded, &mut 0, false);
                    State::AttribValueDq(lc, nc, start)
                }
            }
            State::AttribValueBareSlash(lc, nc, start) => {
                if c == '>' {
                    // The slash really was the self-closing marker.
                    in_entity = None;
                    nodes.push(HtmlNode::Attribute {
                        lc,
                        nc,
                        start,
                        end: decoded.len(),
                    });
                    let begin = nodes.iter().rev().find_map(|v| {
                        if let HtmlNode::OpenTag { lc, nc } = v {
                            Some((*lc, *nc))
                        } else {
                            None
                        }
                    });
                    if let Some((lc, nc)) = begin {
                        if preserves_whitespace(lc) {
                            pre_depth = pre_depth.saturating_sub(1);
                        }
                        nodes.push(HtmlNode::CloseTag { lc, nc });
                    }
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else if is_html_whitespace(c) {
                    // `<a href=x/ y=2>`: the slash belongs to the value.
                    decoded.push('/');
                    in_entity = None;
                    nodes.push(HtmlNode::Attribute {
                        lc,
                        nc,
                        start,
                        end: decoded.len(),
                    });
                    State::ElementAttrs
                } else {
                    // `<a href=http://host/path>`: an ordinary value character.
                    decoded.push('/');
                    process_entity(c, &mut in_entity, &mut decoded, &mut 0, false);
                    State::AttribValueBare(lc, nc, start)
                }
            }
            State::AttribValueBare(lc, nc, start) => {
                if c == '/' {
                    // Only a slash immediately before `>` closes the tag, so
                    // decide once the next character is known.
                    State::AttribValueBareSlash(lc, nc, start)
                } else if c == '>' {
                    in_entity = None;
                    nodes.push(HtmlNode::Attribute {
                        lc,
                        nc,
                        start,
                        end: decoded.len(),
                    });
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else if is_html_whitespace(c) {
                    in_entity = None;
                    nodes.push(HtmlNode::Attribute {
                        lc,
                        nc,
                        start,
                        end: decoded.len(),
                    });
                    State::ElementAttrs
                } else {
                    process_entity(c, &mut in_entity, &mut decoded, &mut 0, false);
                    State::AttribValueBare(lc, nc, start)
                }
            }
            State::CommentStartDash1 => {
                if c != '-' {
                    // we should scan for >
                    State::DocType
                    // if let Some(errors) = errors{errors.push(HtmlError{message:"Unexpected //character looking for - after <!".into(), position:i})};
                } else {
                    State::CommentStartDash2
                }
            }
            State::CommentStartDash2 => {
                if c != '-' {
                    if let Some(errors) = errors {
                        errors.push(HtmlError {
                            message: "Unexpected character looking for - after <!-".into(),
                            position: i,
                        })
                    };
                    State::CommentBody
                } else {
                    // `<!--` is complete: a `>` now closes an empty comment.
                    State::CommentEnd
                }
            }
            State::CommentBody => {
                if c == '-' {
                    State::CommentEndDash
                } else {
                    State::CommentBody
                }
            }
            State::CommentEndDash => {
                if c == '-' {
                    State::CommentEnd
                } else {
                    State::CommentBody
                }
            }
            State::CommentEnd => {
                if c == '-' {
                    // A run of dashes keeps `>` able to close the comment.
                    State::CommentEnd
                } else if c == '>' {
                    State::Text {
                        start: i + 1,
                        dec_start: decoded.len(),
                        last_non_whitespace: decoded.len(),
                        collapse_ws: pre_depth == 0,
                    }
                } else {
                    State::CommentBody
                }
            }
        }
    }
    if let State::Text {
        start: _,
        dec_start,
        last_non_whitespace,
        collapse_ws: _,
    } = state
    {
        nodes.push(HtmlNode::Text {
            start: dec_start,
            end: decoded.len(),
            all_ws: dec_start == last_non_whitespace,
        });
    } else {
        // if we didnt end in text state something is wrong
        if let Some(errors) = errors {
            errors.push(HtmlError {
                message: "HTML Parsing endstate is not HtmlNode::Text".into(),
                position: body.len(),
            })
        };
    }
    HtmlDoc { nodes, decoded }
}

pub fn match_entity(what: &str) -> Result<u32, String> {
    Ok(match what {
        "dollar" => 36,
        "DOLLAR" => 36,
        "cent" => 162,
        "CENT" => 162,
        "pound" => 163,
        "POUND" => 163,
        "curren" => 164,
        "CURREN" => 164,
        "yen" => 165,
        "YEN" => 165,
        "copy" => 169,
        "COPY" => 169,
        "reg" => 174,
        "REG" => 174,
        "trade" => 8482,
        "TRADE" => 8482,
        "commat" => 64,
        "COMMAT" => 64,
        "Copf" => 8450,
        "copf" => 120148,
        "COPF" => 8450,
        "incare" => 8453,
        "INCARE" => 8453,
        "gscr" => 8458,
        "GSCR" => 8458,
        "hamilt" => 8459,
        "HAMILT" => 8459,
        "Hfr" => 8460,
        "hfr" => 120101,
        "HFR" => 8460,
        "Hopf" => 8461,
        "hopf" => 120153,
        "HOPF" => 8461,
        "planckh" => 8462,
        "PLANCKH" => 8462,
        "planck" => 8463,
        "PLANCK" => 8463,
        "Iscr" => 8464,
        "iscr" => 119998,
        "ISCR" => 8464,
        "image" => 8465,
        "IMAGE" => 8465,
        "Lscr" => 8466,
        "lscr" => 120001,
        "LSCR" => 8466,
        "ell" => 8467,
        "ELL" => 8467,
        "Nopf" => 8469,
        "nopf" => 120159,
        "NOPF" => 8469,
        "numero" => 8470,
        "NUMERO" => 8470,
        "copysr" => 8471,
        "COPYSR" => 8471,
        "weierp" => 8472,
        "WEIERP" => 8472,
        "Popf" => 8473,
        "popf" => 120161,
        "POPF" => 8473,
        "Qopf" => 8474,
        "qopf" => 120162,
        "QOPF" => 8474,
        "Rscr" => 8475,
        "rscr" => 120007,
        "RSCR" => 8475,
        "real" => 8476,
        "REAL" => 8476,
        "Ropf" => 8477,
        "ropf" => 120163,
        "ROPF" => 8477,
        "rx" => 8478,
        "RX" => 8478,
        "Zopf" => 8484,
        "zopf" => 120171,
        "ZOPF" => 8484,
        "mho" => 8487,
        "MHO" => 8487,
        "Zfr" => 8488,
        "zfr" => 120119,
        "ZFR" => 8488,
        "iiota" => 8489,
        "IIOTA" => 8489,
        "bernou" => 8492,
        "BERNOU" => 8492,
        "Cfr" => 8493,
        "cfr" => 120096,
        "CFR" => 8493,
        "escr" => 8495,
        "ESCR" => 8495,
        "Escr" => 8496,
        "Fscr" => 8497,
        "fscr" => 119995,
        "FSCR" => 8497,
        "Mscr" => 8499,
        "mscr" => 120002,
        "MSCR" => 8499,
        "oscr" => 8500,
        "OSCR" => 8500,
        "alefsym" => 8501,
        "ALEFSYM" => 8501,
        "beth" => 8502,
        "BETH" => 8502,
        "gimel" => 8503,
        "GIMEL" => 8503,
        "daleth" => 8504,
        "DALETH" => 8504,
        "DD" => 8517,
        "dd" => 8518,
        "ee" => 8519,
        "EE" => 8519,
        "ii" => 8520,
        "II" => 8520,
        "starf" => 9733,
        "STARF" => 9733,
        "star" => 9734,
        "STAR" => 9734,
        "phone" => 9742,
        "PHONE" => 9742,
        "female" => 9792,
        "FEMALE" => 9792,
        "male" => 9794,
        "MALE" => 9794,
        "spades" => 9824,
        "SPADES" => 9824,
        "clubs" => 9827,
        "CLUBS" => 9827,
        "hearts" => 9829,
        "HEARTS" => 9829,
        "diams" => 9830,
        "DIAMS" => 9830,
        "loz" => 9674,
        "LOZ" => 9674,
        "sung" => 9834,
        "SUNG" => 9834,
        "flat" => 9837,
        "FLAT" => 9837,
        "natural" => 9838,
        "NATURAL" => 9838,
        "sharp" => 9839,
        "SHARP" => 9839,
        "check" => 10003,
        "CHECK" => 10003,
        "cross" => 10007,
        "CROSS" => 10007,
        "malt" => 10016,
        "MALT" => 10016,
        "sext" => 10038,
        "SEXT" => 10038,
        "VerticalSeparator" => 10072,
        "verticalseparator" => 10072,
        "VERTICALSEPARATOR" => 10072,
        "lbbrk" => 10098,
        "LBBRK" => 10098,
        "rbbrk" => 10099,
        "RBBRK" => 10099,
        "excl" => 33,
        "EXCL" => 33,
        "num" => 35,
        "NUM" => 35,
        "percnt" => 37,
        "PERCNT" => 37,
        "amp" => 38,
        "AMP" => 38,
        "lpar" => 40,
        "LPAR" => 40,
        "rpar" => 41,
        "RPAR" => 41,
        "ast" => 42,
        "AST" => 42,
        "comma" => 44,
        "COMMA" => 44,
        "period" => 46,
        "PERIOD" => 46,
        "sol" => 47,
        "SOL" => 47,
        "colon" => 58,
        "COLON" => 58,
        "semi" => 59,
        "SEMI" => 59,
        "quest" => 63,
        "QUEST" => 63,
        "lbrack" => 91,
        "LBRACK" => 91,
        "bsol" => 92,
        "BSOL" => 92,
        "rbrack" => 93,
        "RBRACK" => 93,
        "Hat" => 94,
        "hat" => 94,
        "HAT" => 94,
        "lowbar" => 95,
        "LOWBAR" => 95,
        "grave" => 96,
        "GRAVE" => 96,
        "lbrace" => 123,
        "LBRACE" => 123,
        "vert" => 124,
        "VERT" => 124,
        "rbrace" => 125,
        "RBRACE" => 125,
        "tilde" => 732,
        "TILDE" => 732,
        "circ" => 710,
        "CIRC" => 710,
        "nbsp" => 160,
        "NBSP" => 160,
        "ensp" => 8194,
        "ENSP" => 8194,
        "emsp" => 8195,
        "EMSP" => 8195,
        "thinsp" => 8201,
        "THINSP" => 8201,
        "zwnj" => 8204,
        "ZWNJ" => 8204,
        "zwj" => 8205,
        "ZWJ" => 8205,
        "lrm" => 8206,
        "LRM" => 8206,
        "rlm" => 8207,
        "RLM" => 8207,
        "iexcl" => 161,
        "IEXCL" => 161,
        "brvbar" => 166,
        "BRVBAR" => 166,
        "sect" => 167,
        "SECT" => 167,
        "uml" => 168,
        "UML" => 168,
        "ordf" => 170,
        "ORDF" => 170,
        "not" => 172,
        "NOT" => 172,
        "shy" => 173,
        "SHY" => 173,
        "macr" => 175,
        "MACR" => 175,
        "sup2" => 178,
        "SUP2" => 178,
        "sup3" => 179,
        "SUP3" => 179,
        "acute" => 180,
        "ACUTE" => 180,
        "micro" => 181,
        "MICRO" => 181,
        "para" => 182,
        "PARA" => 182,
        "middot" => 183,
        "MIDDOT" => 183,
        "cedil" => 184,
        "CEDIL" => 184,
        "sup1" => 185,
        "SUP1" => 185,
        "ordm" => 186,
        "ORDM" => 186,
        "iquest" => 191,
        "IQUEST" => 191,
        "hyphen" => 8208,
        "HYPHEN" => 8208,
        "ndash" => 8211,
        "NDASH" => 8211,
        "mdash" => 8212,
        "MDASH" => 8212,
        "horbar" => 8213,
        "HORBAR" => 8213,
        "Vert" => 8214,
        "dagger" => 8224,
        "DAGGER" => 8224,
        "Dagger" => 8225,
        "bull" => 8226,
        "BULL" => 8226,
        "nldr" => 8229,
        "NLDR" => 8229,
        "hellip" => 8230,
        "HELLIP" => 8230,
        "pertenk" => 8241,
        "PERTENK" => 8241,
        "prime" => 8242,
        "PRIME" => 8242,
        "Prime" => 8243,
        "tprime" => 8244,
        "TPRIME" => 8244,
        "bprime" => 8245,
        "BPRIME" => 8245,
        "oline" => 8254,
        "OLINE" => 8254,
        "caret" => 8257,
        "CARET" => 8257,
        "hybull" => 8259,
        "HYBULL" => 8259,
        "frasl" => 8260,
        "FRASL" => 8260,
        "bsemi" => 8271,
        "BSEMI" => 8271,
        "qprime" => 8279,
        "QPRIME" => 8279,
        "quot" => 34,
        "QUOT" => 34,
        "apos" => 39,
        "APOS" => 39,
        "laquo" => 171,
        "LAQUO" => 171,
        "raquo" => 187,
        "RAQUO" => 187,
        "lsquo" => 8216,
        "LSQUO" => 8216,
        "rsquo" => 8217,
        "RSQUO" => 8217,
        "sbquo" => 8218,
        "SBQUO" => 8218,
        "ldquo" => 8220,
        "LDQUO" => 8220,
        "rdquo" => 8221,
        "RDQUO" => 8221,
        "bdquo" => 8222,
        "BDQUO" => 8222,
        "lsaquo" => 8249,
        "LSAQUO" => 8249,
        "rsaquo" => 8250,
        "RSAQUO" => 8250,
        "frac14" => 188,
        "FRAC14" => 188,
        "frac12" => 189,
        "FRAC12" => 189,
        "frac34" => 190,
        "FRAC34" => 190,
        "frac13" => 8531,
        "FRAC13" => 8531,
        "frac23" => 8532,
        "FRAC23" => 8532,
        "frac15" => 8533,
        "FRAC15" => 8533,
        "frac25" => 8534,
        "FRAC25" => 8534,
        "frac35" => 8535,
        "FRAC35" => 8535,
        "frac45" => 8536,
        "FRAC45" => 8536,
        "frac16" => 8537,
        "FRAC16" => 8537,
        "frac56" => 8538,
        "FRAC56" => 8538,
        "frac18" => 8539,
        "FRAC18" => 8539,
        "frac38" => 8540,
        "FRAC38" => 8540,
        "frac58" => 8541,
        "FRAC58" => 8541,
        "frac78" => 8542,
        "FRAC78" => 8542,
        "plus" => 43,
        "PLUS" => 43,
        "minus" => 8722,
        "MINUS" => 8722,
        "times" => 215,
        "TIMES" => 215,
        "divide" => 247,
        "DIVIDE" => 247,
        "equals" => 61,
        "EQUALS" => 61,
        "ne" => 8800,
        "NE" => 8800,
        "plusmn" => 177,
        "PLUSMN" => 177,
        "lt" => 60,
        "LT" => 60,
        "gt" => 62,
        "GT" => 62,
        "deg" => 176,
        "DEG" => 176,
        "fnof" => 402,
        "FNOF" => 402,
        "permil" => 8240,
        "PERMIL" => 8240,
        "forall" => 8704,
        "FORALL" => 8704,
        "comp" => 8705,
        "COMP" => 8705,
        "part" => 8706,
        "PART" => 8706,
        "exist" => 8707,
        "EXIST" => 8707,
        "nexist" => 8708,
        "NEXIST" => 8708,
        "empty" => 8709,
        "EMPTY" => 8709,
        "nabla" => 8711,
        "NABLA" => 8711,
        "isin" => 8712,
        "ISIN" => 8712,
        "notin" => 8713,
        "NOTIN" => 8713,
        "ni" => 8715,
        "NI" => 8715,
        "notni" => 8716,
        "NOTNI" => 8716,
        "prod" => 8719,
        "PROD" => 8719,
        "coprod" => 8720,
        "COPROD" => 8720,
        "sum" => 8721,
        "SUM" => 8721,
        "mnplus" => 8723,
        "MNPLUS" => 8723,
        "plusdo" => 8724,
        "PLUSDO" => 8724,
        "setminus" => 8726,
        "SETMINUS" => 8726,
        "lowast" => 8727,
        "LOWAST" => 8727,
        "compfn" => 8728,
        "COMPFN" => 8728,
        "radic" => 8730,
        "RADIC" => 8730,
        "prop" => 8733,
        "PROP" => 8733,
        "infin" => 8734,
        "INFIN" => 8734,
        "angrt" => 8735,
        "ANGRT" => 8735,
        "ang" => 8736,
        "ANG" => 8736,
        "angmsd" => 8737,
        "ANGMSD" => 8737,
        "angsph" => 8738,
        "ANGSPH" => 8738,
        "mid" => 8739,
        "MID" => 8739,
        "nmid" => 8740,
        "NMID" => 8740,
        "parallel" => 8741,
        "PARALLEL" => 8741,
        "npar" => 8742,
        "NPAR" => 8742,
        "and" => 8743,
        "AND" => 8743,
        "or" => 8744,
        "OR" => 8744,
        "cap" => 8745,
        "CAP" => 8745,
        "cup" => 8746,
        "CUP" => 8746,
        "int" => 8747,
        "INT" => 8747,
        "Int" => 8748,
        "iiint" => 8749,
        "IIINT" => 8749,
        "conint" => 8750,
        "CONINT" => 8750,
        "Conint" => 8751,
        "Cconint" => 8752,
        "cconint" => 8752,
        "CCONINT" => 8752,
        "cwint" => 8753,
        "CWINT" => 8753,
        "cwconint" => 8754,
        "CWCONINT" => 8754,
        "awconint" => 8755,
        "AWCONINT" => 8755,
        "there4" => 8756,
        "THERE4" => 8756,
        "because" => 8757,
        "BECAUSE" => 8757,
        "ratio" => 8758,
        "RATIO" => 8758,
        "Colon" => 8759,
        "minusd" => 8760,
        "MINUSD" => 8760,
        "mDDot" => 8762,
        "mddot" => 8762,
        "MDDOT" => 8762,
        "homtht" => 8763,
        "HOMTHT" => 8763,
        "sim" => 8764,
        "SIM" => 8764,
        "bsim" => 8765,
        "BSIM" => 8765,
        "ac" => 8766,
        "AC" => 8766,
        "acd" => 8767,
        "ACD" => 8767,
        "wreath" => 8768,
        "WREATH" => 8768,
        "nsim" => 8769,
        "NSIM" => 8769,
        "esim" => 8770,
        "ESIM" => 8770,
        "sime" => 8771,
        "SIME" => 8771,
        "nsime" => 8772,
        "NSIME" => 8772,
        "cong" => 8773,
        "CONG" => 8773,
        "simne" => 8774,
        "SIMNE" => 8774,
        "ncong" => 8775,
        "NCONG" => 8775,
        "asymp" => 8776,
        "ASYMP" => 8776,
        "nap" => 8777,
        "NAP" => 8777,
        "approxeq" => 8778,
        "APPROXEQ" => 8778,
        "apid" => 8779,
        "APID" => 8779,
        "bcong" => 8780,
        "BCONG" => 8780,
        "asympeq" => 8781,
        "ASYMPEQ" => 8781,
        "bump" => 8782,
        "BUMP" => 8782,
        "bumpe" => 8783,
        "BUMPE" => 8783,
        "esdot" => 8784,
        "ESDOT" => 8784,
        "eDot" => 8785,
        "edot" => 279,
        "EDOT" => 8785,
        "efDot" => 8786,
        "efdot" => 8786,
        "EFDOT" => 8786,
        "erDot" => 8787,
        "erdot" => 8787,
        "ERDOT" => 8787,
        "colone" => 8788,
        "COLONE" => 8788,
        "ecolon" => 8789,
        "ECOLON" => 8789,
        "ecir" => 8790,
        "ECIR" => 8790,
        "cire" => 8791,
        "CIRE" => 8791,
        "wedgeq" => 8793,
        "WEDGEQ" => 8793,
        "veeeq" => 8794,
        "VEEEQ" => 8794,
        "trie" => 8796,
        "TRIE" => 8796,
        "equest" => 8799,
        "EQUEST" => 8799,
        "equiv" => 8801,
        "EQUIV" => 8801,
        "nequiv" => 8802,
        "NEQUIV" => 8802,
        "le" => 8804,
        "LE" => 8804,
        "ge" => 8805,
        "GE" => 8805,
        "lE" => 8806,
        "gE" => 8807,
        "lnE" => 8808,
        "lne" => 10887,
        "LNE" => 8808,
        "gnE" => 8809,
        "gne" => 10888,
        "GNE" => 8809,
        "Lt" => 8810,
        "Gt" => 8811,
        "between" => 8812,
        "BETWEEN" => 8812,
        "NotCupCap" => 8813,
        "notcupcap" => 8813,
        "NOTCUPCAP" => 8813,
        "nlt" => 8814,
        "NLT" => 8814,
        "ngt" => 8815,
        "NGT" => 8815,
        "nle" => 8816,
        "NLE" => 8816,
        "nge" => 8817,
        "NGE" => 8817,
        "lsim" => 8818,
        "LSIM" => 8818,
        "gsim" => 8819,
        "GSIM" => 8819,
        "nlsim" => 8820,
        "NLSIM" => 8820,
        "ngsim" => 8821,
        "NGSIM" => 8821,
        "lg" => 8822,
        "LG" => 8822,
        "gl" => 8823,
        "GL" => 8823,
        "ntlg" => 8824,
        "NTLG" => 8824,
        "ntgl" => 8825,
        "NTGL" => 8825,
        "pr" => 8826,
        "PR" => 8826,
        "sc" => 8827,
        "SC" => 8827,
        "prcue" => 8828,
        "PRCUE" => 8828,
        "sccue" => 8829,
        "SCCUE" => 8829,
        "prsim" => 8830,
        "PRSIM" => 8830,
        "scsim" => 8831,
        "SCSIM" => 8831,
        "npr" => 8832,
        "NPR" => 8832,
        "nsc" => 8833,
        "NSC" => 8833,
        "sub" => 8834,
        "SUB" => 8834,
        "sup" => 8835,
        "SUP" => 8835,
        "nsub" => 8836,
        "NSUB" => 8836,
        "nsup" => 8837,
        "NSUP" => 8837,
        "sube" => 8838,
        "SUBE" => 8838,
        "supe" => 8839,
        "SUPE" => 8839,
        "nsube" => 8840,
        "NSUBE" => 8840,
        "nsupe" => 8841,
        "NSUPE" => 8841,
        "subne" => 8842,
        "SUBNE" => 8842,
        "supne" => 8843,
        "SUPNE" => 8843,
        "cupdot" => 8845,
        "CUPDOT" => 8845,
        "uplus" => 8846,
        "UPLUS" => 8846,
        "sqsub" => 8847,
        "SQSUB" => 8847,
        "sqsup" => 8848,
        "SQSUP" => 8848,
        "sqsube" => 8849,
        "SQSUBE" => 8849,
        "sqsupe" => 8850,
        "SQSUPE" => 8850,
        "sqcap" => 8851,
        "SQCAP" => 8851,
        "sqcup" => 8852,
        "SQCUP" => 8852,
        "oplus" => 8853,
        "OPLUS" => 8853,
        "ominus" => 8854,
        "OMINUS" => 8854,
        "otimes" => 8855,
        "OTIMES" => 8855,
        "osol" => 8856,
        "OSOL" => 8856,
        "odot" => 8857,
        "ODOT" => 8857,
        "ocir" => 8858,
        "OCIR" => 8858,
        "oast" => 8859,
        "OAST" => 8859,
        "odash" => 8861,
        "ODASH" => 8861,
        "plusb" => 8862,
        "PLUSB" => 8862,
        "minusb" => 8863,
        "MINUSB" => 8863,
        "timesb" => 8864,
        "TIMESB" => 8864,
        "sdotb" => 8865,
        "SDOTB" => 8865,
        "vdash" => 8866,
        "VDASH" => 8866,
        "dashv" => 8867,
        "DASHV" => 8867,
        "top" => 8868,
        "TOP" => 8868,
        "perp" => 8869,
        "PERP" => 8869,
        "models" => 8871,
        "MODELS" => 8871,
        "vDash" => 8872,
        "Vdash" => 8873,
        "Vvdash" => 8874,
        "vvdash" => 8874,
        "VVDASH" => 8874,
        "VDash" => 8875,
        "nvdash" => 8876,
        "NVDASH" => 8876,
        "nvDash" => 8877,
        "nVdash" => 8878,
        "nVDash" => 8879,
        "prurel" => 8880,
        "PRUREL" => 8880,
        "vltri" => 8882,
        "VLTRI" => 8882,
        "vrtri" => 8883,
        "VRTRI" => 8883,
        "ltrie" => 8884,
        "LTRIE" => 8884,
        "rtrie" => 8885,
        "RTRIE" => 8885,
        "origof" => 8886,
        "ORIGOF" => 8886,
        "imof" => 8887,
        "IMOF" => 8887,
        "mumap" => 8888,
        "MUMAP" => 8888,
        "hercon" => 8889,
        "HERCON" => 8889,
        "intcal" => 8890,
        "INTCAL" => 8890,
        "veebar" => 8891,
        "VEEBAR" => 8891,
        "barvee" => 8893,
        "BARVEE" => 8893,
        "angrtvb" => 8894,
        "ANGRTVB" => 8894,
        "lrtri" => 8895,
        "LRTRI" => 8895,
        "xwedge" => 8896,
        "XWEDGE" => 8896,
        "xvee" => 8897,
        "XVEE" => 8897,
        "xcap" => 8898,
        "XCAP" => 8898,
        "xcup" => 8899,
        "XCUP" => 8899,
        "diamond" => 8900,
        "DIAMOND" => 8900,
        "sdot" => 8901,
        "SDOT" => 8901,
        "Star" => 8902,
        "divonx" => 8903,
        "DIVONX" => 8903,
        "bowtie" => 8904,
        "BOWTIE" => 8904,
        "ltimes" => 8905,
        "LTIMES" => 8905,
        "rtimes" => 8906,
        "RTIMES" => 8906,
        "lthree" => 8907,
        "LTHREE" => 8907,
        "rthree" => 8908,
        "RTHREE" => 8908,
        "bsime" => 8909,
        "BSIME" => 8909,
        "cuvee" => 8910,
        "CUVEE" => 8910,
        "cuwed" => 8911,
        "CUWED" => 8911,
        "Sub" => 8912,
        "Sup" => 8913,
        "Cap" => 8914,
        "Cup" => 8915,
        "fork" => 8916,
        "FORK" => 8916,
        "epar" => 8917,
        "EPAR" => 8917,
        "ltdot" => 8918,
        "LTDOT" => 8918,
        "gtdot" => 8919,
        "GTDOT" => 8919,
        "Ll" => 8920,
        "ll" => 8810,
        "LL" => 8920,
        "Gg" => 8921,
        "gg" => 8811,
        "GG" => 8921,
        "leg" => 8922,
        "LEG" => 8922,
        "gel" => 8923,
        "GEL" => 8923,
        "cuepr" => 8926,
        "CUEPR" => 8926,
        "cuesc" => 8927,
        "CUESC" => 8927,
        "nprcue" => 8928,
        "NPRCUE" => 8928,
        "nsccue" => 8929,
        "NSCCUE" => 8929,
        "nsqsube" => 8930,
        "NSQSUBE" => 8930,
        "nsqsupe" => 8931,
        "NSQSUPE" => 8931,
        "lnsim" => 8934,
        "LNSIM" => 8934,
        "gnsim" => 8935,
        "GNSIM" => 8935,
        "prnsim" => 8936,
        "PRNSIM" => 8936,
        "scnsim" => 8937,
        "SCNSIM" => 8937,
        "nltri" => 8938,
        "NLTRI" => 8938,
        "nrtri" => 8939,
        "NRTRI" => 8939,
        "nltrie" => 8940,
        "NLTRIE" => 8940,
        "nrtrie" => 8941,
        "NRTRIE" => 8941,
        "vellip" => 8942,
        "VELLIP" => 8942,
        "ctdot" => 8943,
        "CTDOT" => 8943,
        "utdot" => 8944,
        "UTDOT" => 8944,
        "dtdot" => 8945,
        "DTDOT" => 8945,
        "disin" => 8946,
        "DISIN" => 8946,
        "isinsv" => 8947,
        "ISINSV" => 8947,
        "isins" => 8948,
        "ISINS" => 8948,
        "isindot" => 8949,
        "ISINDOT" => 8949,
        "notinvc" => 8950,
        "NOTINVC" => 8950,
        "notinvb" => 8951,
        "NOTINVB" => 8951,
        "isinE" => 8953,
        "isine" => 8953,
        "ISINE" => 8953,
        "nisd" => 8954,
        "NISD" => 8954,
        "xnis" => 8955,
        "XNIS" => 8955,
        "nis" => 8956,
        "NIS" => 8956,
        "notnivc" => 8957,
        "NOTNIVC" => 8957,
        "notnivb" => 8958,
        "NOTNIVB" => 8958,
        "lceil" => 8968,
        "LCEIL" => 8968,
        "rceil" => 8969,
        "RCEIL" => 8969,
        "lfloor" => 8970,
        "LFLOOR" => 8970,
        "rfloor" => 8971,
        "RFLOOR" => 8971,
        "lang" => 10216,
        "LANG" => 10216,
        "rang" => 10217,
        "RANG" => 10217,
        "Alpha" => 913,
        "alpha" => 945,
        "ALPHA" => 913,
        "Beta" => 914,
        "beta" => 946,
        "BETA" => 914,
        "Gamma" => 915,
        "gamma" => 947,
        "GAMMA" => 915,
        "Delta" => 916,
        "delta" => 948,
        "DELTA" => 916,
        "Epsilon" => 917,
        "epsilon" => 949,
        "EPSILON" => 917,
        "Zeta" => 918,
        "zeta" => 950,
        "ZETA" => 918,
        "Eta" => 919,
        "eta" => 951,
        "ETA" => 919,
        "Theta" => 920,
        "theta" => 952,
        "THETA" => 920,
        "Iota" => 921,
        "iota" => 953,
        "IOTA" => 921,
        "Kappa" => 922,
        "kappa" => 954,
        "KAPPA" => 922,
        "Lambda" => 923,
        "lambda" => 955,
        "LAMBDA" => 923,
        "Mu" => 924,
        "mu" => 956,
        "MU" => 924,
        "Nu" => 925,
        "nu" => 957,
        "NU" => 925,
        "Xi" => 926,
        "xi" => 958,
        "XI" => 926,
        "Omicron" => 927,
        "omicron" => 959,
        "OMICRON" => 927,
        "Pi" => 928,
        "pi" => 960,
        "PI" => 928,
        "Rho" => 929,
        "rho" => 961,
        "RHO" => 929,
        "Sigma" => 931,
        "sigma" => 963,
        "SIGMA" => 931,
        "Tau" => 932,
        "tau" => 964,
        "TAU" => 932,
        "Upsilon" => 933,
        "upsilon" => 965,
        "UPSILON" => 933,
        "Phi" => 934,
        "phi" => 966,
        "PHI" => 934,
        "Chi" => 935,
        "chi" => 967,
        "CHI" => 935,
        "Psi" => 936,
        "psi" => 968,
        "PSI" => 936,
        "Omega" => 937,
        "omega" => 969,
        "OMEGA" => 937,
        "sigmaf" => 962,
        "SIGMAF" => 962,
        "thetasym" => 977,
        "THETASYM" => 977,
        "upsih" => 978,
        "UPSIH" => 978,
        "piv" => 982,
        "PIV" => 982,
        "Agrave" => 192,
        "agrave" => 224,
        "AGRAVE" => 192,
        "Aacute" => 193,
        "aacute" => 225,
        "AACUTE" => 193,
        "Acirc" => 194,
        "acirc" => 226,
        "ACIRC" => 194,
        "Atilde" => 195,
        "atilde" => 227,
        "ATILDE" => 195,
        "Auml" => 196,
        "auml" => 228,
        "AUML" => 196,
        "Aring" => 197,
        "aring" => 229,
        "ARING" => 197,
        "AElig" => 198,
        "aelig" => 230,
        "AELIG" => 198,
        "Ccedil" => 199,
        "ccedil" => 231,
        "CCEDIL" => 199,
        "Egrave" => 200,
        "egrave" => 232,
        "EGRAVE" => 200,
        "Eacute" => 201,
        "eacute" => 233,
        "EACUTE" => 201,
        "Ecirc" => 202,
        "ecirc" => 234,
        "ECIRC" => 202,
        "Euml" => 203,
        "euml" => 235,
        "EUML" => 203,
        "Igrave" => 204,
        "Iacute" => 205,
        "Lacute" => 313,
        "lacute" => 314,
        "LACUTE" => 313,
        "Icirc" => 206,
        "Iuml" => 207,
        "ETH" => 208,
        "eth" => 240,
        "Ntilde" => 209,
        "ntilde" => 241,
        "NTILDE" => 209,
        "Ograve" => 210,
        "ograve" => 242,
        "OGRAVE" => 210,
        "Oacute" => 211,
        "oacute" => 243,
        "OACUTE" => 211,
        "Ocirc" => 212,
        "ocirc" => 244,
        "OCIRC" => 212,
        "Otilde" => 213,
        "otilde" => 245,
        "OTILDE" => 213,
        "Ouml" => 214,
        "ouml" => 246,
        "OUML" => 214,
        "Oslash" => 216,
        "oslash" => 248,
        "OSLASH" => 216,
        "Ugrave" => 217,
        "ugrave" => 249,
        "UGRAVE" => 217,
        "Uacute" => 218,
        "uacute" => 250,
        "UACUTE" => 218,
        "Ucirc" => 219,
        "ucirc" => 251,
        "UCIRC" => 219,
        "Uuml" => 220,
        "uuml" => 252,
        "UUML" => 220,
        "Yacute" => 221,
        "yacute" => 253,
        "YACUTE" => 221,
        "THORN" => 222,
        "thorn" => 254,
        "szlig" => 223,
        "SZLIG" => 223,
        "igrave" => 236,
        "IGRAVE" => 236,
        "iacute" => 237,
        "IACUTE" => 237,
        "icirc" => 238,
        "ICIRC" => 238,
        "iuml" => 239,
        "IUML" => 239,
        "yuml" => 255,
        "YUML" => 255,
        "Amacr" => 256,
        "amacr" => 257,
        "AMACR" => 256,
        "Abreve" => 258,
        "abreve" => 259,
        "ABREVE" => 258,
        "Aogon" => 260,
        "aogon" => 261,
        "AOGON" => 260,
        "Cacute" => 262,
        "cacute" => 263,
        "CACUTE" => 262,
        "Ccirc" => 264,
        "ccirc" => 265,
        "CCIRC" => 264,
        "Cdot" => 266,
        "cdot" => 267,
        "CDOT" => 266,
        "Ccaron" => 268,
        "ccaron" => 269,
        "CCARON" => 268,
        "Dcaron" => 270,
        "dcaron" => 271,
        "DCARON" => 270,
        "Dstrok" => 272,
        "dstrok" => 273,
        "DSTROK" => 272,
        "Emacr" => 274,
        "emacr" => 275,
        "EMACR" => 274,
        "Edot" => 278,
        "Eogon" => 280,
        "eogon" => 281,
        "EOGON" => 280,
        "Ecaron" => 282,
        "ecaron" => 283,
        "ECARON" => 282,
        "Gcirc" => 284,
        "gcirc" => 285,
        "GCIRC" => 284,
        "Gbreve" => 286,
        "gbreve" => 287,
        "GBREVE" => 286,
        "Gdot" => 288,
        "gdot" => 289,
        "GDOT" => 288,
        "Gcedil" => 290,
        "gcedil" => 290,
        "GCEDIL" => 290,
        "Hcirc" => 292,
        "hcirc" => 293,
        "HCIRC" => 292,
        "Hstrok" => 294,
        "hstrok" => 295,
        "HSTROK" => 294,
        "Itilde" => 296,
        "itilde" => 297,
        "ITILDE" => 296,
        "Imacr" => 298,
        "imacr" => 299,
        "IMACR" => 298,
        "Iogon" => 302,
        "iogon" => 303,
        "IOGON" => 302,
        "Idot" => 304,
        "idot" => 304,
        "IDOT" => 304,
        "imath" => 305,
        "IMATH" => 305,
        "IJlig" => 306,
        "ijlig" => 307,
        "IJLIG" => 306,
        "Jcirc" => 308,
        "jcirc" => 309,
        "JCIRC" => 308,
        "Kcedil" => 310,
        "kcedil" => 311,
        "KCEDIL" => 310,
        "kgreen" => 312,
        "KGREEN" => 312,
        "Lcedil" => 315,
        "lcedil" => 316,
        "LCEDIL" => 315,
        "Lcaron" => 317,
        "lcaron" => 318,
        "LCARON" => 317,
        "Lmidot" => 319,
        "lmidot" => 320,
        "LMIDOT" => 319,
        "Lstrok" => 321,
        "lstrok" => 322,
        "LSTROK" => 321,
        "Nacute" => 323,
        "nacute" => 324,
        "NACUTE" => 323,
        "Ncedil" => 325,
        "ncedil" => 326,
        "NCEDIL" => 325,
        "Ncaron" => 327,
        "ncaron" => 328,
        "NCARON" => 327,
        "napos" => 329,
        "NAPOS" => 329,
        "ENG" => 330,
        "eng" => 331,
        "Omacr" => 332,
        "omacr" => 333,
        "OMACR" => 332,
        "Odblac" => 336,
        "odblac" => 337,
        "ODBLAC" => 336,
        "OElig" => 338,
        "oelig" => 339,
        "OELIG" => 338,
        "Racute" => 340,
        "racute" => 341,
        "RACUTE" => 340,
        "Rcedil" => 342,
        "rcedil" => 343,
        "RCEDIL" => 342,
        "Rcaron" => 344,
        "rcaron" => 345,
        "RCARON" => 344,
        "Sacute" => 346,
        "sacute" => 347,
        "SACUTE" => 346,
        "Scirc" => 348,
        "scirc" => 349,
        "SCIRC" => 348,
        "Scedil" => 350,
        "scedil" => 351,
        "SCEDIL" => 350,
        "Scaron" => 352,
        "scaron" => 353,
        "SCARON" => 352,
        "Tcedil" => 354,
        "tcedil" => 355,
        "TCEDIL" => 354,
        "Tcaron" => 356,
        "tcaron" => 357,
        "TCARON" => 356,
        "Tstrok" => 358,
        "tstrok" => 359,
        "TSTROK" => 358,
        "Utilde" => 360,
        "utilde" => 361,
        "UTILDE" => 360,
        "Umacr" => 362,
        "umacr" => 363,
        "UMACR" => 362,
        "Ubreve" => 364,
        "ubreve" => 365,
        "UBREVE" => 364,
        "Uring" => 366,
        "uring" => 367,
        "URING" => 366,
        "Udblac" => 368,
        "udblac" => 369,
        "UDBLAC" => 368,
        "Uogon" => 370,
        "uogon" => 371,
        "UOGON" => 370,
        "Wcirc" => 372,
        "wcirc" => 373,
        "WCIRC" => 372,
        "Ycirc" => 374,
        "ycirc" => 375,
        "YCIRC" => 374,
        "Yuml" => 376,
        "Zacute" => 377,
        "zacute" => 378,
        "ZACUTE" => 377,
        "Zdot" => 379,
        "zdot" => 380,
        "ZDOT" => 379,
        "Zcaron" => 381,
        "zcaron" => 382,
        "ZCARON" => 381,
        "DownBreve" => 785,
        "downbreve" => 785,
        "DOWNBREVE" => 785,
        "olarr" => 8634,
        "OLARR" => 8634,
        "orarr" => 8635,
        "ORARR" => 8635,
        "lharu" => 8636,
        "LHARU" => 8636,
        "lhard" => 8637,
        "LHARD" => 8637,
        "uharr" => 8638,
        "UHARR" => 8638,
        "uharl" => 8639,
        "UHARL" => 8639,
        "rharu" => 8640,
        "RHARU" => 8640,
        "rhard" => 8641,
        "RHARD" => 8641,
        "dharr" => 8642,
        "DHARR" => 8642,
        "dharl" => 8643,
        "DHARL" => 8643,
        "rlarr" => 8644,
        "RLARR" => 8644,
        "udarr" => 8645,
        "UDARR" => 8645,
        "lrarr" => 8646,
        "LRARR" => 8646,
        "llarr" => 8647,
        "LLARR" => 8647,
        "uuarr" => 8648,
        "UUARR" => 8648,
        "rrarr" => 8649,
        "RRARR" => 8649,
        "ddarr" => 8650,
        "DDARR" => 8650,
        "lrhar" => 8651,
        "LRHAR" => 8651,
        "rlhar" => 8652,
        "RLHAR" => 8652,
        "nlArr" => 8653,
        "nlarr" => 8602,
        "NLARR" => 8653,
        "nhArr" => 8654,
        "nharr" => 8622,
        "NHARR" => 8654,
        "nrArr" => 8655,
        "nrarr" => 8603,
        "NRARR" => 8655,
        "lArr" => 8656,
        "larr" => 8592,
        "LARR" => 8656,
        "uArr" => 8657,
        "uarr" => 8593,
        "UARR" => 8657,
        "rArr" => 8658,
        "rarr" => 8594,
        "RARR" => 8658,
        "dArr" => 8659,
        "darr" => 8595,
        "DARR" => 8659,
        "hArr" => 8660,
        "harr" => 8596,
        "HARR" => 8660,
        "vArr" => 8661,
        "varr" => 8597,
        "VARR" => 8661,
        "nwArr" => 8662,
        "nwarr" => 8598,
        "NWARR" => 8662,
        "neArr" => 8663,
        "nearr" => 8599,
        "NEARR" => 8663,
        "seArr" => 8664,
        "searr" => 8600,
        "SEARR" => 8664,
        "swArr" => 8665,
        "swarr" => 8601,
        "SWARR" => 8665,
        "lAarr" => 8666,
        "laarr" => 8666,
        "LAARR" => 8666,
        "rAarr" => 8667,
        "raarr" => 8667,
        "RAARR" => 8667,
        "ziglarr" => 8668,
        "ZIGLARR" => 8668,
        "zigrarr" => 8669,
        "ZIGRARR" => 8669,
        "larrb" => 8676,
        "LARRB" => 8676,
        "rarrb" => 8677,
        "RARRB" => 8677,
        "duarr" => 8693,
        "DUARR" => 8693,
        "hoarr" => 8703,
        "HOARR" => 8703,
        "loarr" => 8701,
        "LOARR" => 8701,
        "roarr" => 8702,
        "ROARR" => 8702,
        "xlarr" => 10229,
        "XLARR" => 10229,
        "xrarr" => 10230,
        "XRARR" => 10230,
        "xharr" => 10231,
        "XHARR" => 10231,
        "xlArr" => 10232,
        "xrArr" => 10233,
        "xhArr" => 10234,
        "dzigrarr" => 10239,
        "DZIGRARR" => 10239,
        "xmap" => 10236,
        "XMAP" => 10236,
        "nvlArr" => 10498,
        "nvlarr" => 10498,
        "NVLARR" => 10498,
        "nvrArr" => 10499,
        "nvrarr" => 10499,
        "NVRARR" => 10499,
        "nvHarr" => 10500,
        "nvharr" => 10500,
        "NVHARR" => 10500,
        "Map" => 10501,
        "map" => 8614,
        "MAP" => 10501,
        "lbarr" => 10508,
        "LBARR" => 10508,
        "rbarr" => 10509,
        "RBARR" => 10509,
        "lBarr" => 10510,
        "rBarr" => 10511,
        "RBarr" => 10512,
        "DDotrahd" => 10513,
        "ddotrahd" => 10513,
        "DDOTRAHD" => 10513,
        "UpArrowBar" => 10514,
        "uparrowbar" => 10514,
        "UPARROWBAR" => 10514,
        "DownArrowBar" => 10515,
        "downarrowbar" => 10515,
        "DOWNARROWBAR" => 10515,
        "Rarrtl" => 10518,
        "rarrtl" => 8611,
        "RARRTL" => 10518,
        "latail" => 10521,
        "LATAIL" => 10521,
        "ratail" => 10522,
        "RATAIL" => 10522,
        "lAtail" => 10523,
        "rAtail" => 10524,
        "larrfs" => 10525,
        "LARRFS" => 10525,
        "rarrfs" => 10526,
        "RARRFS" => 10526,
        "larrbfs" => 10527,
        "LARRBFS" => 10527,
        "rarrbfs" => 10528,
        "RARRBFS" => 10528,
        "nwarhk" => 10531,
        "NWARHK" => 10531,
        "nearhk" => 10532,
        "NEARHK" => 10532,
        "searhk" => 10533,
        "SEARHK" => 10533,
        "swarhk" => 10534,
        "SWARHK" => 10534,
        "nwnear" => 10535,
        "NWNEAR" => 10535,
        "nesear" => 10536,
        "NESEAR" => 10536,
        "seswar" => 10537,
        "SESWAR" => 10537,
        "swnwar" => 10538,
        "SWNWAR" => 10538,
        "cudarrr" => 10549,
        "CUDARRR" => 10549,
        "ldca" => 10550,
        "LDCA" => 10550,
        "rdca" => 10551,
        "RDCA" => 10551,
        "cudarrl" => 10552,
        "CUDARRL" => 10552,
        "larrpl" => 10553,
        "LARRPL" => 10553,
        "curarrm" => 10556,
        "CURARRM" => 10556,
        "cularrp" => 10557,
        "CULARRP" => 10557,
        "rarrpl" => 10565,
        "RARRPL" => 10565,
        "harrcir" => 10568,
        "HARRCIR" => 10568,
        "Uarrocir" => 10569,
        "uarrocir" => 10569,
        "UARROCIR" => 10569,
        "lurdshar" => 10570,
        "LURDSHAR" => 10570,
        "ldrushar" => 10571,
        "LDRUSHAR" => 10571,
        "RightUpDownVector" => 10575,
        "rightupdownvector" => 10575,
        "RIGHTUPDOWNVECTOR" => 10575,
        "DownLeftRightVector" => 10576,
        "downleftrightvector" => 10576,
        "DOWNLEFTRIGHTVECTOR" => 10576,
        "LeftUpDownVector" => 10577,
        "leftupdownvector" => 10577,
        "LEFTUPDOWNVECTOR" => 10577,
        "LeftVectorBar" => 10578,
        "leftvectorbar" => 10578,
        "LEFTVECTORBAR" => 10578,
        "RightVectorBar" => 10579,
        "rightvectorbar" => 10579,
        "RIGHTVECTORBAR" => 10579,
        "RightUpVectorBar" => 10580,
        "rightupvectorbar" => 10580,
        "RIGHTUPVECTORBAR" => 10580,
        "RightDownVectorBar" => 10581,
        "rightdownvectorbar" => 10581,
        "RIGHTDOWNVECTORBAR" => 10581,
        "DownLeftVectorBar" => 10582,
        "downleftvectorbar" => 10582,
        "DOWNLEFTVECTORBAR" => 10582,
        "DownRightVectorBar" => 10583,
        "downrightvectorbar" => 10583,
        "DOWNRIGHTVECTORBAR" => 10583,
        "LeftUpVectorBar" => 10584,
        "leftupvectorbar" => 10584,
        "LEFTUPVECTORBAR" => 10584,
        "LeftDownVectorBar" => 10585,
        "leftdownvectorbar" => 10585,
        "LEFTDOWNVECTORBAR" => 10585,
        "LeftTeeVector" => 10586,
        "leftteevector" => 10586,
        "LEFTTEEVECTOR" => 10586,
        "RightTeeVector" => 10587,
        "rightteevector" => 10587,
        "RIGHTTEEVECTOR" => 10587,
        "RightUpTeeVector" => 10588,
        "rightupteevector" => 10588,
        "RIGHTUPTEEVECTOR" => 10588,
        "RightDownTeeVector" => 10589,
        "rightdownteevector" => 10589,
        "RIGHTDOWNTEEVECTOR" => 10589,
        "DownLeftTeeVector" => 10590,
        "downleftteevector" => 10590,
        "DOWNLEFTTEEVECTOR" => 10590,
        "DownRightTeeVector" => 10591,
        "downrightteevector" => 10591,
        "DOWNRIGHTTEEVECTOR" => 10591,
        "LeftUpTeeVector" => 10592,
        "leftupteevector" => 10592,
        "LEFTUPTEEVECTOR" => 10592,
        "LeftDownTeeVector" => 10593,
        "leftdownteevector" => 10593,
        "LEFTDOWNTEEVECTOR" => 10593,
        "lHar" => 10594,
        "lhar" => 10594,
        "LHAR" => 10594,
        "uHar" => 10595,
        "uhar" => 10595,
        "UHAR" => 10595,
        "rHar" => 10596,
        "rhar" => 10596,
        "RHAR" => 10596,
        "dHar" => 10597,
        "dhar" => 10597,
        "DHAR" => 10597,
        "luruhar" => 10598,
        "LURUHAR" => 10598,
        "ldrdhar" => 10599,
        "LDRDHAR" => 10599,
        "ruluhar" => 10600,
        "RULUHAR" => 10600,
        "rdldhar" => 10601,
        "RDLDHAR" => 10601,
        "lharul" => 10602,
        "LHARUL" => 10602,
        "llhard" => 10603,
        "LLHARD" => 10603,
        "rharul" => 10604,
        "RHARUL" => 10604,
        "lrhard" => 10605,
        "LRHARD" => 10605,
        "udhar" => 10606,
        "UDHAR" => 10606,
        "duhar" => 10607,
        "DUHAR" => 10607,
        "RoundImplies" => 10608,
        "roundimplies" => 10608,
        "ROUNDIMPLIES" => 10608,
        "erarr" => 10609,
        "ERARR" => 10609,
        "simrarr" => 10610,
        "SIMRARR" => 10610,
        "larrsim" => 10611,
        "LARRSIM" => 10611,
        "rarrsim" => 10612,
        "RARRSIM" => 10612,
        "rarrap" => 10613,
        "RARRAP" => 10613,
        "ltlarr" => 10614,
        "LTLARR" => 10614,
        "gtrarr" => 10616,
        "GTRARR" => 10616,
        "subrarr" => 10617,
        "SUBRARR" => 10617,
        "suplarr" => 10619,
        "SUPLARR" => 10619,
        "lfisht" => 10620,
        "LFISHT" => 10620,
        "rfisht" => 10621,
        "RFISHT" => 10621,
        "ufisht" => 10622,
        "UFISHT" => 10622,
        "dfisht" => 10623,
        "DFISHT" => 10623,
        x => {
            // Not a named entity: it may be a numeric character reference.
            // Everything here is validated rather than cast, because the
            // caller turns the result into a `char`: an out-of-range value,
            // a surrogate, or a negative number would otherwise panic.
            let Some(digits) = x.strip_prefix('#') else {
                return Err("unknown html entity".into());
            };
            // `&#x..;` / `&#X..;` are hex, anything else is decimal. Parsing
            // as `u32` (not `i64`) rejects a leading `-` or `+` outright.
            let num = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16)
                    .map_err(|_| String::from("Cannot parse hex html entity"))?,
                None => digits
                    .parse::<u32>()
                    .map_err(|_| String::from("Cannot parse digit html entity"))?,
            };
            // Reject anything that is not a Unicode scalar value: beyond
            // U+10FFFF, or a UTF-16 surrogate half.
            if char::from_u32(num).is_none() {
                return Err("Html entity is not a valid unicode scalar".into());
            }
            num
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concatenated text of every text node, in document order.
    fn text_of(body: &str) -> String {
        let doc = parse_html(body, &mut None, InternLiveId::No);
        let mut walker = doc.new_walker();
        let mut out = String::new();
        while !walker.done() {
            if let Some(text) = walker.text() {
                out.push_str(text);
            }
            walker.walk();
        }
        out
    }

    /// Every node's byte range must be in bounds, ordered, and on a char
    /// boundary of `decoded`, and `all_ws` must match the text it describes.
    fn assert_nodes_consistent(body: &str) {
        let doc = parse_html(body, &mut None, InternLiveId::No);
        let mut highest_text_end = 0;
        for (i, node) in doc.nodes.iter().enumerate() {
            let (start, end) = match node {
                HtmlNode::Text { start, end, .. } | HtmlNode::Attribute { start, end, .. } => {
                    (*start, *end)
                }
                _ => continue,
            };
            assert!(start <= end, "{body:?}: node {i} has start {start} > end {end}");
            assert!(
                end <= doc.decoded.len(),
                "{body:?}: node {i} ends at {end}, past decoded len {}",
                doc.decoded.len()
            );
            assert!(
                doc.decoded.is_char_boundary(start) && doc.decoded.is_char_boundary(end),
                "{body:?}: node {i} range {start}..{end} splits a character"
            );
            if let HtmlNode::Text { all_ws, .. } = node {
                let really_all_ws = doc.decoded[start..end].chars().all(is_html_whitespace);
                assert_eq!(
                    *all_ws, really_all_ws,
                    "{body:?}: node {i} all_ws={all_ws} but text is {:?}",
                    &doc.decoded[start..end]
                );
                assert!(
                    start >= highest_text_end,
                    "{body:?}: node {i} overlaps an earlier text node"
                );
                highest_text_end = end;
            }
        }
    }

    /// Numeric character references that do not name a Unicode scalar value
    /// used to reach `char::from_u32(..).unwrap()` and abort the process. Any
    /// of these is reachable from a hostile chat message.
    #[test]
    fn out_of_range_numeric_entities_are_left_as_text() {
        for body in [
            "&#xD800;",           // high surrogate
            "&#xDFFF;",           // low surrogate
            "&#55296;",           // the same, in decimal
            "&#x110000;",         // one past the last scalar value
            "&#-1;",              // negative
            "&#x-1;",
            "&#99999999999999;",  // overflows every integer width
            "&#;",
            "&#x;",
        ] {
            assert_eq!(text_of(body), body, "{body:?} should survive as literal text");
            assert_nodes_consistent(body);
        }
    }

    /// A `&` that never terminates must not stay pending across a tag or a
    /// closing quote: a later `;` would truncate `decoded` and retroactively
    /// invalidate the byte ranges of nodes already emitted.
    #[test]
    fn an_unterminated_entity_does_not_span_a_tag_boundary() {
        for body in [
            "<p>&am<b>p;</b></p>",
            "<p>&am</p><b>p;</b>",
            "&<>;",
            "&#<>1;",
            "<a href='&am'>p;</a>",
            "<a href=\"&am\">p;</a>",
            "<a href=&am>p;</a>",
        ] {
            assert_nodes_consistent(body);
        }
        assert_eq!(text_of("<p>&am<b>p;</b></p>"), "&amp;");
    }

    /// `&;` is not an entity. It decoded to `‰` because the table carried an
    /// empty-string key where `permil` belonged.
    #[test]
    fn an_empty_entity_name_is_not_an_entity() {
        assert_eq!(text_of("&;"), "&;");
        assert_eq!(match_entity("").is_err(), true);
        assert_eq!(text_of("&permil;"), "\u{2030}");
    }

    /// The table was generated by folding names case-insensitively, so every
    /// entity whose name differs from another only by case took the other's
    /// code point: `&eacute;` rendered `É`, `&alpha;` rendered `Α`.
    #[test]
    fn case_distinct_entities_keep_their_own_code_points() {
        for (body, expected) in [
            ("&aacute;", "\u{e1}"),
            ("&Aacute;", "\u{c1}"),
            ("&eacute;", "\u{e9}"),
            ("&Eacute;", "\u{c9}"),
            ("&alpha;", "\u{3b1}"),
            ("&Alpha;", "\u{391}"),
            ("&omega;", "\u{3c9}"),
            ("&Omega;", "\u{3a9}"),
            ("&larr;", "\u{2190}"),
            ("&lArr;", "\u{21d0}"),
            ("&rarr;", "\u{2192}"),
            ("&rArr;", "\u{21d2}"),
            ("&copf;", "\u{1d554}"),
            ("&Copf;", "\u{2102}"),
        ] {
            assert_eq!(text_of(body), expected, "{body:?}");
        }
    }

    /// `Igrave`/`Icirc`/`Iuml` had been transcribed as `Lgrave`/`Lcirc`/`Luml`,
    /// and `Iacute` was missing outright.
    #[test]
    fn capital_i_accents_are_reachable() {
        assert_eq!(text_of("&Igrave;"), "\u{cc}");
        assert_eq!(text_of("&Iacute;"), "\u{cd}");
        assert_eq!(text_of("&Icirc;"), "\u{ce}");
        assert_eq!(text_of("&Iuml;"), "\u{cf}");
        // the invented l-accent spellings are not entities
        assert_eq!(text_of("&Lgrave;"), "&Lgrave;");
        assert_eq!(text_of("&lcirc;"), "&lcirc;");
    }

    /// Entities that already worked must keep working.
    #[test]
    fn common_entities_are_unchanged() {
        for (body, expected) in [
            ("&amp;", "&"),
            ("&lt;", "<"),
            ("&gt;", ">"),
            ("&quot;", "\""),
            ("&apos;", "'"),
            ("&nbsp;", "\u{a0}"),
            ("&mdash;", "\u{2014}"),
            ("&hellip;", "\u{2026}"),
            ("&#38;", "&"),
            ("&#x26;", "&"),
            ("&#X26;", "&"),
            ("&amp;lt;", "&lt;"),
            ("&notanentity;", "&notanentity;"),
            ("&DownLeftRightVector;", "\u{2950}"),
        ] {
            assert_eq!(text_of(body), expected, "{body:?}");
        }
    }

    /// `<pre>` and `<code>` keep their whitespace. A single flag meant any
    /// nested tag cancelled that for the rest of the element, so a
    /// syntax-highlighted code block lost its indentation.
    #[test]
    fn whitespace_is_preserved_for_the_whole_pre_element() {
        assert_eq!(text_of("<pre>a    b</pre>"), "a    b");
        assert_eq!(
            text_of("<pre>a    <b>b    b</b>    c</pre>"),
            "a    b    b    c"
        );
        assert_eq!(
            text_of("<pre><code><span>fn</span>  main()</code></pre>"),
            "fn  main()"
        );
        assert_eq!(text_of("<pre><code>x\n    y</code></pre>"), "x\n    y");
        // and collapsing resumes once the element closes
        assert_eq!(text_of("<pre>a  b</pre>c    d"), "a  bc d");
        assert_eq!(text_of("<p>a    b</p>"), "a b");
    }

    /// A void element written without a slash emits an open tag and no close
    /// tag. Counting every open tag as a nesting level therefore left the
    /// depth permanently unbalanced and swallowed the rest of the document.
    #[test]
    fn jump_to_close_steps_over_void_elements() {
        fn tail_after_first_element(body: &str) -> String {
            let doc = parse_html(body, &mut None, InternLiveId::No);
            let mut walker = doc.new_walker();
            while !walker.done() && walker.open_tag_lc().is_none() {
                walker.walk();
            }
            walker.jump_to_close();
            let mut out = String::new();
            while !walker.done() {
                if let Some(text) = walker.text() {
                    out.push_str(text);
                }
                walker.walk();
            }
            out
        }
        assert_eq!(tail_after_first_element("<div><b>x</b></div>AFTER"), "AFTER");
        assert_eq!(tail_after_first_element("<div><br>x</div>AFTER"), "AFTER");
        assert_eq!(tail_after_first_element("<div><br/>x</div>AFTER"), "AFTER");
        assert_eq!(
            tail_after_first_element("<a href='u'><img src='i'>caption</a>AFTER"),
            "AFTER"
        );
        assert_eq!(tail_after_first_element("<b>x<b>y</b>z</b>AFTER"), "AFTER");
        // an element with no close tag of its own leaves the walker where it
        // is, so the caller still sees everything that follows
        assert_eq!(tail_after_first_element("<img src='i'>AFTER"), "AFTER");
        assert_eq!(tail_after_first_element("<p>unclosed"), "unclosed");
    }

    /// Unquoted attribute values were the only value form that never decoded
    /// entities, and a value beginning with a multi-byte character recorded a
    /// byte range that split that character.
    #[test]
    fn unquoted_attribute_values_are_decoded_and_stay_on_char_boundaries() {
        fn href(body: &str) -> Option<String> {
            let doc = parse_html(body, &mut None, InternLiveId::No);
            let mut walker = doc.new_walker();
            while !walker.done() && walker.open_tag_lc().is_none() {
                walker.walk();
            }
            walker.find_attr_lc(live_id!(href)).map(str::to_string)
        }
        assert_eq!(href("<a href=a&amp;b>t</a>").as_deref(), Some("a&b"));
        assert_eq!(href("<a href=&amp;x>t</a>").as_deref(), Some("&x"));
        assert_eq!(href("<a href=\"a&amp;b\">t</a>").as_deref(), Some("a&b"));
        assert_eq!(href("<a href='a&amp;b'>t</a>").as_deref(), Some("a&b"));
        assert_eq!(href("<a href=émile>t</a>").as_deref(), Some("émile"));
        assert_nodes_consistent("<a href=émile>t</a>");
        assert_nodes_consistent("<a href=漢字 title=🙂>t</a>");
    }

    /// `<a href=>` took `>` as the first character of the value, so the tag
    /// never closed and its content leaked out as literal text.
    #[test]
    fn an_empty_unquoted_attribute_value_still_closes_the_tag() {
        assert_eq!(text_of("<a href=>hello</a><p>after</p>"), "helloafter");
        let doc = parse_html("<a href=>hello</a>", &mut None, InternLiveId::No);
        let mut walker = doc.new_walker();
        while !walker.done() && walker.open_tag_lc().is_none() {
            walker.walk();
        }
        assert_eq!(walker.find_attr_lc(live_id!(href)), Some(""));
    }

    /// The parser emits a zero-length text node in front of every tag, so
    /// `find_text` reported no text for any element whose content starts with
    /// markup, and `find_tag_text` missed any tag carrying an attribute.
    #[test]
    fn text_lookups_skip_parser_artifacts() {
        let doc = parse_html("<a href='x'><b>label</b></a>", &mut None, InternLiveId::No);
        let mut walker = doc.new_walker();
        while !walker.done() && walker.open_tag_lc() != Some(live_id!(a)) {
            walker.walk();
        }
        assert_eq!(walker.find_text(), Some("label"));

        let doc = parse_html("<p class='x'>Hello</p>", &mut None, InternLiveId::No);
        assert_eq!(doc.new_walker().find_tag_text(live_id!(p)), Some("Hello"));
        // tag names are case-insensitive
        let doc = parse_html("<P>Hello</P>", &mut None, InternLiveId::No);
        assert_eq!(doc.new_walker().find_tag_text(live_id!(p)), Some("Hello"));
    }

    /// Whitespace introduced by an entity has to leave `all_ws` describing the
    /// text that is actually there.
    /// HTML's whitespace set is the five ASCII characters, not Unicode's.
    /// Treating U+00A0 and U+3000 as collapsible ate `&nbsp;` runs and the
    /// full-width spaces in CJK text.
    #[test]
    fn only_ascii_whitespace_collapses() {
        assert_eq!(text_of("<p>a&nbsp; b</p>"), "a\u{a0} b");
        assert_eq!(text_of("<p>&nbsp;&nbsp;</p>"), "\u{a0}\u{a0}");
        assert_eq!(text_of("<p>\u{3000}x\u{3000}</p>"), "\u{3000}x\u{3000}");
        assert_eq!(text_of("<p>a    b</p>"), "a b");
        assert_eq!(text_of("<p>a\t\n\r b</p>"), "a b");
        // a cell holding only &nbsp; is content, not collapsible whitespace
        let doc = parse_html("<td>&nbsp;</td>", &mut None, InternLiveId::No);
        let all_ws: Vec<bool> = doc.nodes.iter().filter_map(|n| match n {
            HtmlNode::Text { all_ws, start, end } if start != end => Some(*all_ws),
            _ => None,
        }).collect();
        assert_eq!(all_ws, vec![false]);
    }

    /// A `/` only closes the tag when `>` follows it, so unquoted URLs keep
    /// their path.
    #[test]
    fn an_unquoted_value_keeps_slashes_that_are_not_the_self_closing_marker() {
        fn attr(body: &str, key: LiveId) -> Option<String> {
            let doc = parse_html(body, &mut None, InternLiveId::No);
            let mut walker = doc.new_walker();
            while !walker.done() && walker.open_tag_lc().is_none() {
                walker.walk();
            }
            walker.find_attr_lc(key).map(str::to_string)
        }
        assert_eq!(
            attr("<a href=http://example.com/page>x</a>", live_id!(href)).as_deref(),
            Some("http://example.com/page")
        );
        assert_eq!(text_of("<a href=http://example.com/page>x</a> rest"), "x rest");
        assert_eq!(attr("<img src=/media/pic.png>", live_id!(src)).as_deref(), Some("/media/pic.png"));
        // and the self-closing form still self-closes
        assert_eq!(attr("<img src=x/>", live_id!(src)).as_deref(), Some("x"));
        let doc = parse_html("<img src=x/>", &mut None, InternLiveId::No);
        assert!(doc.nodes.iter().any(|n| matches!(n, HtmlNode::CloseTag { lc, .. } if *lc == live_id!(img))));
        assert_eq!(attr("<a href=x/ y=2>t</a>", live_id!(href)).as_deref(), Some("x/"));
    }

    /// A `<` that cannot begin a tag is literal text, the way a browser
    /// treats it. Parsing it as an element used to consume the rest of the
    /// line as a tag name and attributes, and drop it.
    #[test]
    fn a_less_than_that_is_not_a_tag_stays_text() {
        assert_eq!(text_of("5<10 and 6<12"), "5<10 and 6<12");
        assert_eq!(text_of("a < b"), "a < b");
        assert_eq!(text_of("<>"), "<>");
        assert_eq!(text_of("<1a>x"), "<1a>x");
        assert_eq!(text_of("i <3 you"), "i <3 you");
        assert_eq!(text_of("<é>text"), "<é>text");
        // real tags still parse
        assert_eq!(text_of("<p>x</p>"), "x");
        assert_eq!(text_of("<P>x</P>"), "x");
        for body in ["5<10 and 6<12", "a < b", "<>", "<1a>x", "i <3 you", "<é>t"] {
            assert_nodes_consistent(body);
        }
    }

    /// Junk inside a tag is discarded up to its `>` instead of resuming text
    /// in the middle of it, and `</` that cannot name an element is text.
    #[test]
    fn malformed_tags_do_not_leak_their_markup_into_the_text() {
        assert_eq!(text_of("a</p x>b"), "ab");
        assert_eq!(text_of("a<br/x>b"), "ab");
        assert_eq!(text_of("<p>x</p junk>y"), "xy");
        assert_eq!(text_of("</3 you"), "</3 you");
        assert_eq!(text_of("i </3 u"), "i </3 u");
        assert_eq!(text_of("a</ b"), "a</ b");
        // well-formed close tags are unaffected
        assert_eq!(text_of("<p>x</p >y"), "xy");
        assert_eq!(text_of("<br/>ok"), "ok");
        assert_eq!(text_of("<a href=u>t</a>tail"), "ttail");
        for body in ["a</p x>b", "a<br/x>b", "</3 you", "a</ b", "a</>b"] {
            assert_nodes_consistent(body);
        }
    }

    /// `<!-->` is a complete comment rather than the start of an
    /// unterminated one that eats the rest of the document.
    #[test]
    fn declarations_and_comments_do_not_swallow_the_document() {
        assert_eq!(text_of("5<10? yes"), "5<10? yes");
        assert_eq!(text_of("a<!-->b"), "ab");
        assert_eq!(text_of("a<!--->b"), "ab");
        assert_eq!(text_of("a<!----->b"), "ab");
        assert_eq!(text_of("a<!--c-->b"), "ab");
        assert_eq!(text_of("a<!--c--->b"), "ab");
        assert_eq!(text_of("a<?xml version='1'?>b"), "ab");
    }

    #[test]
    fn all_ws_matches_the_decoded_text() {
        for body in [
            " &#11;",
            "&nbsp;",
            "&nbsp;&nbsp;",
            " &nbsp;",
            "&#32;",
            "&#32;x",
            "<td>&nbsp;</td>",
            "<td>&nbsp;&nbsp;</td>",
        ] {
            assert_nodes_consistent(body);
        }
    }

    /// Malformed input must still produce a walkable document rather than a
    /// panic or a corrupted node range.
    #[test]
    fn malformed_input_stays_walkable() {
        for body in [
            "", "<", ">", "</", "/>", "<>", "</>", "<//>", "< />", "<a", "<a ",
            "<a href", "<a href=", "<a href='", "<a href=\"", "<!--", "<!---",
            "<!-->", "<!--->", "<!", "<!DOCTYPE html>", "<?xml version='1'?>",
            "<?", "<a?b>", "&", "&#", "&#x", "&amp", "<p>&", "</p></p></p>",
            "<b><i></b></i>", "<a href=x/ y=2>t</a>", "<a//b>", "\u{feff}<p>x</p>",
            "<é>text</é>", "<p>🙂<br>👨‍👩‍👧</p>", "<p>\r\n\tx</p>",
        ] {
            assert_nodes_consistent(body);
            // walking the whole document must not panic
            let doc = parse_html(body, &mut None, InternLiveId::No);
            let mut walker = doc.new_walker();
            while !walker.done() {
                let _ = walker.text();
                let _ = walker.open_tag();
                let _ = walker.close_tag();
                let _ = walker.find_attr_lc(live_id!(href));
                let _ = walker.find_text();
                let _ = walker.text_is_all_ws();
                let before = walker.index();
                let mut probe = doc.new_walker_with_index(before);
                probe.jump_to_close();
                assert!(probe.index() >= before, "{body:?}: jump_to_close moved backwards");
                walker.walk();
            }
        }
    }

    /// Interning changes how names are stored, never the shape of the document.
    #[test]
    fn interning_does_not_change_the_document() {
        for body in ["<p class='a'>x</p>", "<a href=y>&amp;</a>", "<pre>a  b</pre>"] {
            let plain = parse_html(body, &mut None, InternLiveId::No);
            let interned = parse_html(body, &mut None, InternLiveId::Yes);
            assert_eq!(plain.decoded, interned.decoded, "{body:?}");
            assert_eq!(plain.nodes.len(), interned.nodes.len(), "{body:?}");
        }
    }
}
