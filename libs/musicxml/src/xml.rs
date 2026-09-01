use crate::{XmlError, XmlErrorKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlDeclaration {
    pub version: String,
    pub encoding: Option<String>,
    pub standalone: Option<bool>,
}

impl Default for XmlDeclaration {
    fn default() -> Self {
        Self {
            version: "1.0".to_string(),
            encoding: Some("UTF-8".to_string()),
            standalone: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlDocument {
    pub declaration: XmlDeclaration,
    /// Contents of the declaration, without `<!DOCTYPE` and the final `>`.
    pub doctype: Option<String>,
    pub before_root: Vec<XmlNode>,
    pub root: XmlElement,
    pub after_root: Vec<XmlNode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlAttribute {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlElement {
    pub name: String,
    pub attributes: Vec<XmlAttribute>,
    pub children: Vec<XmlNode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XmlNode {
    Element(XmlElement),
    Text(String),
    CData(String),
    Comment(String),
    ProcessingInstruction { target: String, data: String },
}

impl XmlElement {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            attributes: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn with_attribute(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.push(XmlAttribute {
            name: name.into(),
            value: value.into(),
        });
        self
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .map(|attribute| attribute.value.as_str())
    }

    pub fn attr_mut(&mut self, name: &str) -> Option<&mut String> {
        self.attributes
            .iter_mut()
            .find(|attribute| attribute.name == name)
            .map(|attribute| &mut attribute.value)
    }

    pub fn set_attr(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        if let Some(attribute) = self
            .attributes
            .iter_mut()
            .find(|attribute| attribute.name == name)
        {
            attribute.value = value;
        } else {
            self.attributes.push(XmlAttribute { name, value });
        }
    }

    pub fn remove_attr(&mut self, name: &str) -> Option<XmlAttribute> {
        let index = self
            .attributes
            .iter()
            .position(|attribute| attribute.name == name)?;
        Some(self.attributes.remove(index))
    }

    pub fn id(&self) -> Option<&str> {
        self.attr("id")
    }

    pub fn child_elements(&self) -> impl Iterator<Item = &XmlElement> {
        self.children.iter().filter_map(XmlNode::as_element)
    }

    pub fn child_elements_mut(&mut self) -> impl Iterator<Item = &mut XmlElement> {
        self.children.iter_mut().filter_map(XmlNode::as_element_mut)
    }

    pub fn children_named<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a XmlElement> + 'a {
        let name = name.to_string();
        self.child_elements()
            .filter(move |element| element.name == name)
    }

    pub fn first_child(&self, name: &str) -> Option<&XmlElement> {
        self.children_named(name).next()
    }

    pub fn first_child_mut(&mut self, name: &str) -> Option<&mut XmlElement> {
        self.child_elements_mut()
            .find(|element| element.name == name)
    }

    /// Concatenates direct text and CDATA children, excluding descendant text.
    pub fn direct_text(&self) -> String {
        let mut text = String::new();
        for child in &self.children {
            match child {
                XmlNode::Text(value) | XmlNode::CData(value) => text.push_str(value),
                _ => {}
            }
        }
        text
    }

    /// Concatenates all descendant text and CDATA in document order.
    pub fn text_content(&self) -> String {
        fn append(nodes: &[XmlNode], out: &mut String) {
            for node in nodes {
                match node {
                    XmlNode::Element(element) => append(&element.children, out),
                    XmlNode::Text(text) | XmlNode::CData(text) => out.push_str(text),
                    XmlNode::Comment(_) | XmlNode::ProcessingInstruction { .. } => {}
                }
            }
        }
        let mut out = String::new();
        append(&self.children, &mut out);
        out
    }

    pub fn push_element(&mut self, element: XmlElement) {
        self.children.push(XmlNode::Element(element));
    }
}

impl XmlNode {
    pub fn as_element(&self) -> Option<&XmlElement> {
        match self {
            Self::Element(element) => Some(element),
            _ => None,
        }
    }

    pub fn as_element_mut(&mut self) -> Option<&mut XmlElement> {
        match self {
            Self::Element(element) => Some(element),
            _ => None,
        }
    }
}

pub fn parse_xml(source: &str) -> Result<XmlDocument, XmlError> {
    if let Some((offset, character)) = source
        .char_indices()
        .find(|(_, character)| !valid_xml_codepoint(*character as u32))
    {
        return Err(XmlError::at(
            XmlErrorKind::UnexpectedToken,
            format!("character U+{:04X} is not allowed in XML", character as u32),
            source,
            offset,
        ));
    }
    let normalized;
    let source = if source.contains('\r') {
        normalized = source.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_str()
    } else {
        source
    };
    Parser::new(source).parse_document()
}

pub fn write_xml(document: &XmlDocument) -> Result<String, XmlError> {
    validate_document(document)?;
    let mut out = String::new();
    out.push_str("<?xml version=\"");
    escape_attribute_into(&document.declaration.version, &mut out);
    out.push('"');
    if let Some(encoding) = &document.declaration.encoding {
        out.push_str(" encoding=\"");
        escape_attribute_into(encoding, &mut out);
        out.push('"');
    }
    if let Some(standalone) = document.declaration.standalone {
        out.push_str(if standalone {
            " standalone=\"yes\""
        } else {
            " standalone=\"no\""
        });
    }
    out.push_str("?>\n");
    if let Some(doctype) = &document.doctype {
        out.push_str("<!DOCTYPE ");
        out.push_str(doctype);
        out.push_str(">\n");
    }
    for node in &document.before_root {
        write_node(node, &mut out);
    }
    write_element(&document.root, &mut out);
    for node in &document.after_root {
        write_node(node, &mut out);
    }
    Ok(out)
}

fn write_element(element: &XmlElement, out: &mut String) {
    out.push('<');
    out.push_str(&element.name);
    for attribute in &element.attributes {
        out.push(' ');
        out.push_str(&attribute.name);
        out.push_str("=\"");
        escape_attribute_into(&attribute.value, out);
        out.push('"');
    }
    if element.children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in &element.children {
        write_node(child, out);
    }
    out.push_str("</");
    out.push_str(&element.name);
    out.push('>');
}

fn write_node(node: &XmlNode, out: &mut String) {
    match node {
        XmlNode::Element(element) => write_element(element, out),
        XmlNode::Text(text) => escape_text_into(text, out),
        XmlNode::CData(text) => {
            out.push_str("<![CDATA[");
            out.push_str(&text.replace("]]>", "]]]]><![CDATA[>"));
            out.push_str("]]>");
        }
        XmlNode::Comment(text) => {
            out.push_str("<!--");
            out.push_str(text);
            out.push_str("-->");
        }
        XmlNode::ProcessingInstruction { target, data } => {
            out.push_str("<?");
            out.push_str(target);
            if !data.is_empty() {
                out.push(' ');
                out.push_str(data);
            }
            out.push_str("?>");
        }
    }
}

fn escape_text_into(value: &str, out: &mut String) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(character),
        }
    }
}

fn escape_attribute_into(value: &str, out: &mut String) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            _ => out.push(character),
        }
    }
}

fn validate_document(document: &XmlDocument) -> Result<(), XmlError> {
    let synthetic = "";
    let valid_string = |value: &str| {
        value
            .chars()
            .all(|character| valid_xml_codepoint(character as u32))
    };
    if !valid_string(&document.declaration.version)
        || document
            .declaration
            .encoding
            .as_deref()
            .is_some_and(|value| !valid_string(value))
        || document
            .doctype
            .as_deref()
            .is_some_and(|value| !valid_string(value))
    {
        return Err(XmlError::at(
            XmlErrorKind::UnexpectedToken,
            "XML metadata contains a forbidden character",
            synthetic,
            0,
        ));
    }
    if !valid_name(&document.root.name) {
        return Err(XmlError::at(
            XmlErrorKind::InvalidName,
            format!("invalid root element name {:?}", document.root.name),
            synthetic,
            0,
        ));
    }
    fn validate_element(element: &XmlElement) -> Result<(), String> {
        if !valid_name(&element.name) {
            return Err(format!("invalid element name {:?}", element.name));
        }
        for (index, attribute) in element.attributes.iter().enumerate() {
            if !valid_name(&attribute.name) {
                return Err(format!("invalid attribute name {:?}", attribute.name));
            }
            if element.attributes[..index]
                .iter()
                .any(|other| other.name == attribute.name)
            {
                return Err(format!("duplicate attribute {:?}", attribute.name));
            }
            if !attribute
                .value
                .chars()
                .all(|character| valid_xml_codepoint(character as u32))
            {
                return Err(format!(
                    "attribute {:?} contains a forbidden XML character",
                    attribute.name
                ));
            }
        }
        for child in &element.children {
            match child {
                XmlNode::Element(child) => validate_element(child)?,
                XmlNode::Text(text) | XmlNode::CData(text)
                    if !text
                        .chars()
                        .all(|character| valid_xml_codepoint(character as u32)) =>
                {
                    return Err("text contains a forbidden XML character".to_string())
                }
                XmlNode::Comment(text) if text.contains("--") || text.ends_with('-') => {
                    return Err("comment contains an illegal -- sequence".to_string())
                }
                XmlNode::Comment(text)
                    if !text
                        .chars()
                        .all(|character| valid_xml_codepoint(character as u32)) =>
                {
                    return Err("comment contains a forbidden XML character".to_string())
                }
                XmlNode::ProcessingInstruction { target, data } => {
                    if !valid_name(target)
                        || target.eq_ignore_ascii_case("xml")
                        || data.contains("?>")
                        || !data
                            .chars()
                            .all(|character| valid_xml_codepoint(character as u32))
                    {
                        return Err("invalid processing instruction".to_string());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    validate_element(&document.root)
        .map_err(|message| XmlError::at(XmlErrorKind::UnexpectedToken, message, synthetic, 0))?;
    for node in document
        .before_root
        .iter()
        .chain(document.after_root.iter())
    {
        match node {
            XmlNode::Element(_) => {
                return Err(XmlError::at(
                    XmlErrorKind::MultipleRoots,
                    "top-level side content cannot contain an element",
                    synthetic,
                    0,
                ))
            }
            XmlNode::Text(text) if !text.chars().all(char::is_whitespace) => {
                return Err(XmlError::at(
                    XmlErrorKind::UnexpectedToken,
                    "non-whitespace text outside the root element",
                    synthetic,
                    0,
                ))
            }
            XmlNode::Text(text) if !valid_string(text) => {
                return Err(XmlError::at(
                    XmlErrorKind::UnexpectedToken,
                    "top-level text contains a forbidden XML character",
                    synthetic,
                    0,
                ))
            }
            XmlNode::CData(_) => {
                return Err(XmlError::at(
                    XmlErrorKind::InvalidCData,
                    "CDATA is not allowed outside the root element",
                    synthetic,
                    0,
                ))
            }
            XmlNode::Comment(text)
                if text.contains("--") || text.ends_with('-') || !valid_string(text) =>
            {
                return Err(XmlError::at(
                    XmlErrorKind::InvalidComment,
                    "invalid top-level comment",
                    synthetic,
                    0,
                ))
            }
            XmlNode::ProcessingInstruction { target, data }
                if !valid_name(target)
                    || target.eq_ignore_ascii_case("xml")
                    || data.contains("?>")
                    || !valid_string(data) =>
            {
                return Err(XmlError::at(
                    XmlErrorKind::UnexpectedToken,
                    "invalid top-level processing instruction",
                    synthetic,
                    0,
                ))
            }
            _ => {}
        }
    }
    Ok(())
}

struct Parser<'a> {
    source: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: source.strip_prefix('\u{feff}').unwrap_or(source),
            pos: 0,
        }
    }

    fn parse_document(mut self) -> Result<XmlDocument, XmlError> {
        let mut declaration = XmlDeclaration::default();
        let mut doctype = None;
        let mut before_root = Vec::new();
        let mut after_root = Vec::new();
        let mut root = None;

        while !self.eof() {
            if self.starts_xml_declaration() && root.is_none() && before_root.is_empty() {
                declaration = self.parse_declaration()?;
            } else if self.starts_with("<!DOCTYPE") {
                if root.is_some() || doctype.is_some() {
                    return Err(self.error(
                        XmlErrorKind::InvalidDoctype,
                        "DOCTYPE must occur once before the root element",
                    ));
                }
                doctype = Some(self.parse_doctype()?);
            } else if self.starts_with("<!--") {
                let node = XmlNode::Comment(self.parse_comment()?);
                if root.is_some() {
                    after_root.push(node);
                } else {
                    before_root.push(node);
                }
            } else if self.starts_with("<?") {
                let node = self.parse_processing_instruction()?;
                if root.is_some() {
                    after_root.push(node);
                } else {
                    before_root.push(node);
                }
            } else if self.starts_with("<") {
                if root.is_some() {
                    return Err(self.error(XmlErrorKind::MultipleRoots, "multiple root elements"));
                }
                root = Some(self.parse_element()?);
            } else {
                let start = self.pos;
                let text = self.take_until('<');
                if !text.chars().all(char::is_whitespace) {
                    return Err(XmlError::at(
                        XmlErrorKind::UnexpectedToken,
                        "non-whitespace text outside the root element",
                        self.source,
                        start,
                    ));
                }
            }
        }

        let root = root.ok_or_else(|| {
            self.error(XmlErrorKind::MissingRoot, "document has no root element")
        })?;
        Ok(XmlDocument {
            declaration,
            doctype,
            before_root,
            root,
            after_root,
        })
    }

    fn parse_declaration(&mut self) -> Result<XmlDeclaration, XmlError> {
        let start = self.pos;
        self.expect("<?xml", XmlErrorKind::InvalidDeclaration)?;
        let body_start = self.pos;
        let end = self.source[self.pos..]
            .find("?>")
            .map(|relative| self.pos + relative)
            .ok_or_else(|| {
                XmlError::at(
                    XmlErrorKind::InvalidDeclaration,
                    "unterminated XML declaration",
                    self.source,
                    start,
                )
            })?;
        let body = &self.source[body_start..end];
        self.pos = end + 2;
        let attributes = parse_pseudo_attributes(body, self.source, body_start)?;
        let version = attributes
            .iter()
            .find(|attribute| attribute.name == "version")
            .map(|attribute| attribute.value.clone())
            .ok_or_else(|| {
                XmlError::at(
                    XmlErrorKind::InvalidDeclaration,
                    "XML declaration has no version",
                    self.source,
                    start,
                )
            })?;
        let encoding = attributes
            .iter()
            .find(|attribute| attribute.name == "encoding")
            .map(|attribute| attribute.value.clone());
        let standalone = match attributes
            .iter()
            .find(|attribute| attribute.name == "standalone")
            .map(|attribute| attribute.value.as_str())
        {
            None => None,
            Some("yes") => Some(true),
            Some("no") => Some(false),
            Some(_) => {
                return Err(XmlError::at(
                    XmlErrorKind::InvalidDeclaration,
                    "standalone must be yes or no",
                    self.source,
                    start,
                ))
            }
        };
        if attributes.iter().any(|attribute| {
            attribute.name != "version"
                && attribute.name != "encoding"
                && attribute.name != "standalone"
        }) {
            return Err(XmlError::at(
                XmlErrorKind::InvalidDeclaration,
                "unknown XML declaration attribute",
                self.source,
                start,
            ));
        }
        Ok(XmlDeclaration {
            version,
            encoding,
            standalone,
        })
    }

    fn parse_doctype(&mut self) -> Result<String, XmlError> {
        let start = self.pos;
        self.expect("<!DOCTYPE", XmlErrorKind::InvalidDoctype)?;
        let content_start = self.pos;
        let bytes = self.source.as_bytes();
        let mut quote = None;
        let mut subset_depth = 0usize;
        while self.pos < bytes.len() {
            if quote.is_none() && self.starts_with("<!--") {
                let comment_end = self.source[self.pos + 4..]
                    .find("-->")
                    .map(|relative| self.pos + 4 + relative + 3)
                    .ok_or_else(|| {
                        XmlError::at(
                            XmlErrorKind::InvalidDoctype,
                            "unterminated comment in DOCTYPE",
                            self.source,
                            self.pos,
                        )
                    })?;
                self.pos = comment_end;
                continue;
            }
            let byte = bytes[self.pos];
            match quote {
                Some(delimiter) if byte == delimiter => quote = None,
                Some(_) => {}
                None if byte == b'\'' || byte == b'"' => quote = Some(byte),
                None if byte == b'[' => subset_depth += 1,
                None if byte == b']' => subset_depth = subset_depth.saturating_sub(1),
                None if byte == b'>' && subset_depth == 0 => {
                    let value = self.source[content_start..self.pos].trim().to_string();
                    self.pos += 1;
                    if value.is_empty() {
                        return Err(XmlError::at(
                            XmlErrorKind::InvalidDoctype,
                            "empty DOCTYPE",
                            self.source,
                            start,
                        ));
                    }
                    return Ok(value);
                }
                _ => {}
            }
            self.pos += 1;
        }
        Err(XmlError::at(
            XmlErrorKind::InvalidDoctype,
            "unterminated DOCTYPE",
            self.source,
            start,
        ))
    }

    fn parse_element(&mut self) -> Result<XmlElement, XmlError> {
        let start = self.pos;
        self.expect("<", XmlErrorKind::UnexpectedToken)?;
        if self.starts_with("/") || self.starts_with("!") || self.starts_with("?") {
            return Err(self.error(XmlErrorKind::UnexpectedToken, "expected an opening tag"));
        }
        let name = self.parse_name()?.to_string();
        let mut attributes = Vec::new();
        loop {
            self.skip_whitespace();
            if self.consume("/>") {
                return Ok(XmlElement {
                    name,
                    attributes,
                    children: Vec::new(),
                });
            }
            if self.consume(">") {
                break;
            }
            if self.eof() {
                return Err(XmlError::at(
                    XmlErrorKind::UnexpectedEnd,
                    "unterminated opening tag",
                    self.source,
                    start,
                ));
            }
            let attribute_start = self.pos;
            let attribute_name = self.parse_name()?.to_string();
            if attributes
                .iter()
                .any(|attribute: &XmlAttribute| attribute.name == attribute_name)
            {
                return Err(XmlError::at(
                    XmlErrorKind::DuplicateAttribute,
                    format!("duplicate attribute {attribute_name:?}"),
                    self.source,
                    attribute_start,
                ));
            }
            self.skip_whitespace();
            self.expect("=", XmlErrorKind::InvalidAttribute)?;
            self.skip_whitespace();
            let value = self.parse_quoted_value()?;
            attributes.push(XmlAttribute {
                name: attribute_name,
                value,
            });
        }

        let mut children = Vec::new();
        loop {
            if self.eof() {
                return Err(XmlError::at(
                    XmlErrorKind::UnexpectedEnd,
                    format!("element {name:?} has no closing tag"),
                    self.source,
                    start,
                ));
            }
            if self.consume("</") {
                let close_start = self.pos;
                let closing = self.parse_name()?.to_string();
                self.skip_whitespace();
                self.expect(">", XmlErrorKind::UnexpectedToken)?;
                if closing != name {
                    return Err(XmlError::at(
                        XmlErrorKind::MismatchedClosingTag,
                        format!("expected </{name}> but found </{closing}>"),
                        self.source,
                        close_start,
                    ));
                }
                break;
            } else if self.starts_with("<!--") {
                children.push(XmlNode::Comment(self.parse_comment()?));
            } else if self.starts_with("<![CDATA[") {
                children.push(XmlNode::CData(self.parse_cdata()?));
            } else if self.starts_with("<?") {
                children.push(self.parse_processing_instruction()?);
            } else if self.starts_with("<!DOCTYPE") {
                return Err(self.error(
                    XmlErrorKind::InvalidDoctype,
                    "DOCTYPE is not allowed inside an element",
                ));
            } else if self.starts_with("<") {
                children.push(XmlNode::Element(self.parse_element()?));
            } else {
                let text_start = self.pos;
                let raw = self.take_until('<');
                let text = decode_entities(raw, self.source, text_start)?;
                if !text.is_empty() {
                    children.push(XmlNode::Text(text));
                }
            }
        }
        Ok(XmlElement {
            name,
            attributes,
            children,
        })
    }

    fn parse_comment(&mut self) -> Result<String, XmlError> {
        let start = self.pos;
        self.expect("<!--", XmlErrorKind::InvalidComment)?;
        let end = self.source[self.pos..]
            .find("-->")
            .map(|relative| self.pos + relative)
            .ok_or_else(|| {
                XmlError::at(
                    XmlErrorKind::InvalidComment,
                    "unterminated comment",
                    self.source,
                    start,
                )
            })?;
        let value = &self.source[self.pos..end];
        if value.contains("--") || value.ends_with('-') {
            return Err(XmlError::at(
                XmlErrorKind::InvalidComment,
                "comment contains an illegal -- sequence",
                self.source,
                start,
            ));
        }
        self.pos = end + 3;
        Ok(value.to_string())
    }

    fn parse_cdata(&mut self) -> Result<String, XmlError> {
        let start = self.pos;
        self.expect("<![CDATA[", XmlErrorKind::InvalidCData)?;
        let end = self.source[self.pos..]
            .find("]]>")
            .map(|relative| self.pos + relative)
            .ok_or_else(|| {
                XmlError::at(
                    XmlErrorKind::InvalidCData,
                    "unterminated CDATA section",
                    self.source,
                    start,
                )
            })?;
        let value = self.source[self.pos..end].to_string();
        self.pos = end + 3;
        Ok(value)
    }

    fn parse_processing_instruction(&mut self) -> Result<XmlNode, XmlError> {
        let start = self.pos;
        self.expect("<?", XmlErrorKind::UnexpectedToken)?;
        let target = self.parse_name()?.to_string();
        if target.eq_ignore_ascii_case("xml") {
            return Err(XmlError::at(
                XmlErrorKind::InvalidDeclaration,
                "xml processing instruction is only allowed as the declaration",
                self.source,
                start,
            ));
        }
        let end = self.source[self.pos..]
            .find("?>")
            .map(|relative| self.pos + relative)
            .ok_or_else(|| {
                XmlError::at(
                    XmlErrorKind::UnexpectedEnd,
                    "unterminated processing instruction",
                    self.source,
                    start,
                )
            })?;
        let data = self.source[self.pos..end].trim().to_string();
        self.pos = end + 2;
        Ok(XmlNode::ProcessingInstruction { target, data })
    }

    fn parse_quoted_value(&mut self) -> Result<String, XmlError> {
        let start = self.pos;
        let delimiter = match self.peek_byte() {
            Some(b'\'') => b'\'',
            Some(b'"') => b'"',
            _ => {
                return Err(self.error(
                    XmlErrorKind::InvalidAttribute,
                    "attribute value must be quoted",
                ))
            }
        };
        self.pos += 1;
        let content_start = self.pos;
        while let Some(byte) = self.peek_byte() {
            if byte == delimiter {
                let raw = &self.source[content_start..self.pos];
                let decoded = decode_entities(raw, self.source, content_start)?;
                self.pos += 1;
                return Ok(decoded);
            }
            if byte == b'<' {
                return Err(XmlError::at(
                    XmlErrorKind::InvalidAttribute,
                    "attribute value contains <",
                    self.source,
                    self.pos,
                ));
            }
            self.advance_char();
        }
        Err(XmlError::at(
            XmlErrorKind::UnexpectedEnd,
            "unterminated attribute value",
            self.source,
            start,
        ))
    }

    fn parse_name(&mut self) -> Result<&'a str, XmlError> {
        let start = self.pos;
        let first = self.peek_char().ok_or_else(|| {
            self.error(XmlErrorKind::UnexpectedEnd, "expected an XML name")
        })?;
        if !is_name_start(first) {
            return Err(self.error(XmlErrorKind::InvalidName, "invalid start of XML name"));
        }
        self.advance_char();
        while let Some(character) = self.peek_char() {
            if !is_name_continue(character) {
                break;
            }
            self.advance_char();
        }
        Ok(&self.source[start..self.pos])
    }

    fn skip_whitespace(&mut self) {
        while self.peek_char().is_some_and(char::is_whitespace) {
            self.advance_char();
        }
    }

    fn take_until(&mut self, delimiter: char) -> &'a str {
        let start = self.pos;
        if let Some(relative) = self.source[self.pos..].find(delimiter) {
            self.pos += relative;
        } else {
            self.pos = self.source.len();
        }
        &self.source[start..self.pos]
    }

    fn starts_with(&self, value: &str) -> bool {
        self.source[self.pos..].starts_with(value)
    }

    fn starts_xml_declaration(&self) -> bool {
        let Some(tail) = self.source[self.pos..].strip_prefix("<?xml") else {
            return false;
        };
        tail.chars().next().is_some_and(char::is_whitespace)
    }

    fn consume(&mut self, value: &str) -> bool {
        if self.starts_with(value) {
            self.pos += value.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, value: &str, kind: XmlErrorKind) -> Result<(), XmlError> {
        if self.consume(value) {
            Ok(())
        } else {
            Err(self.error(kind, format!("expected {value:?}")))
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.source.len()
    }

    fn peek_byte(&self) -> Option<u8> {
        self.source.as_bytes().get(self.pos).copied()
    }

    fn peek_char(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    fn advance_char(&mut self) {
        if let Some(character) = self.peek_char() {
            self.pos += character.len_utf8();
        }
    }

    fn error(&self, kind: XmlErrorKind, message: impl Into<String>) -> XmlError {
        XmlError::at(kind, message, self.source, self.pos)
    }
}

fn parse_pseudo_attributes(
    source: &str,
    whole_source: &str,
    base_offset: usize,
) -> Result<Vec<XmlAttribute>, XmlError> {
    let wrapped = format!("<declaration{source}/>");
    let parsed = parse_xml(&wrapped).map_err(|mut error| {
        error.offset = base_offset + error.offset.saturating_sub("<declaration".len());
        let corrected = XmlError::at(error.kind, error.message, whole_source, error.offset);
        corrected
    })?;
    Ok(parsed.root.attributes)
}

fn decode_entities(raw: &str, source: &str, base_offset: usize) -> Result<String, XmlError> {
    if !raw.contains('&') {
        return Ok(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut pos = 0;
    while let Some(relative) = raw[pos..].find('&') {
        let entity_start = pos + relative;
        out.push_str(&raw[pos..entity_start]);
        let semi = raw[entity_start + 1..]
            .find(';')
            .map(|relative| entity_start + 1 + relative)
            .ok_or_else(|| {
                XmlError::at(
                    XmlErrorKind::InvalidEntity,
                    "unterminated entity reference",
                    source,
                    base_offset + entity_start,
                )
            })?;
        let entity = &raw[entity_start + 1..semi];
        let character = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "apos" => '\'',
            "quot" => '"',
            value if value.starts_with("#x") || value.starts_with("#X") => {
                parse_character_reference(&value[2..], 16, source, base_offset + entity_start)?
            }
            value if value.starts_with('#') => {
                parse_character_reference(&value[1..], 10, source, base_offset + entity_start)?
            }
            _ => {
                return Err(XmlError::at(
                    XmlErrorKind::InvalidEntity,
                    format!("unknown XML entity &{entity};"),
                    source,
                    base_offset + entity_start,
                ))
            }
        };
        out.push(character);
        pos = semi + 1;
    }
    out.push_str(&raw[pos..]);
    Ok(out)
}

fn parse_character_reference(
    digits: &str,
    radix: u32,
    source: &str,
    offset: usize,
) -> Result<char, XmlError> {
    let value = u32::from_str_radix(digits, radix).ok();
    value
        .filter(|value| valid_xml_codepoint(*value))
        .and_then(char::from_u32)
        .ok_or_else(|| {
            XmlError::at(
                XmlErrorKind::InvalidCharacterReference,
                "invalid XML character reference",
                source,
                offset,
            )
        })
}

fn valid_xml_codepoint(value: u32) -> bool {
    matches!(value, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(is_name_start) && chars.all(is_name_continue)
}

fn is_name_start(character: char) -> bool {
    matches!(character,
        ':' | '_' | 'A'..='Z' | 'a'..='z' |
        '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{f6}' | '\u{f8}'..='\u{2ff}' |
        '\u{370}'..='\u{37d}' | '\u{37f}'..='\u{1fff}' | '\u{200c}'..='\u{200d}' |
        '\u{2070}'..='\u{218f}' | '\u{2c00}'..='\u{2fef}' | '\u{3001}'..='\u{d7ff}' |
        '\u{f900}'..='\u{fdcf}' | '\u{fdf0}'..='\u{fffd}' | '\u{10000}'..='\u{effff}'
    )
}

fn is_name_continue(character: char) -> bool {
    is_name_start(character)
        || character == '-'
        || character == '.'
        || character.is_ascii_digit()
        || character == '\u{b7}'
        || matches!(character, '\u{300}'..='\u{36f}' | '\u{203f}'..='\u{2040}')
}
