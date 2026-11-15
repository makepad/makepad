//! Logical structure and tagged page text (ISO 32000-1, clauses 14.6–14.9).

use crate::content::{parse_content_stream, tm_pre_translate, PdfOp, TextArrayItem};
use crate::document::PdfDocument;
use crate::font::decode_codes;
use crate::lexer::{PdfError, PdfResult};
use crate::object::{ObjRef, PdfDict, PdfObj};
use crate::page::PdfPage;
use std::collections::{HashMap, HashSet};

/// A document's logical structure, extracted text and conformance diagnostics.
#[derive(Clone, Debug)]
pub struct StructTree {
    /// Catalog /MarkInfo /Marked is true.
    pub marked: bool,
    /// Catalog /Lang.
    pub lang: Option<String>,
    /// Catalog /ViewerPreferences /Direction (L2R or R2L).
    pub direction: Option<String>,
    /// The StructTreeRoot's /K elements, in order.
    pub roots: Vec<StructElement>,
    /// Per page, text of each outermost artifact sequence, in stream order.
    pub artifacts: Vec<Vec<String>>,
    /// Per page, text outside structure content items and artifacts.
    pub untagged: Vec<Vec<String>>,
    /// Breaches of required rules, prefixed with their section number.
    pub problems: Vec<String>,
    /// Departures from recommendations and unsupported content, by section.
    pub notes: Vec<String>,
}

/// A structure element and its children in logical reading order.
#[derive(Clone, Debug)]
pub struct StructElement {
    /// /S as written.
    pub kind: String,
    /// /S after following /RoleMap to a standard type.
    pub standard: String,
    /// The element's indirect reference, if present.
    pub object: Option<ObjRef>,
    /// The page index identified by this element's /Pg.
    pub page: Option<usize>,
    /// /Lang, without synthesising an inherited value.
    pub lang: Option<String>,
    /// /Alt alternate description.
    pub alt: Option<String>,
    /// /ActualText replacement for the entire subtree.
    pub actual_text: Option<String>,
    /// /E expansion of an abbreviation.
    pub expansion: Option<String>,
    /// /T title.
    pub title: Option<String>,
    /// /ID identifier.
    pub id: Option<String>,
    /// Attribute entries, with their owners, in /A order.
    pub attributes: Vec<StructAttribute>,
    /// Children in /K order.
    pub kids: Vec<StructKid>,
}

/// One entry of a structure attribute dictionary.
#[derive(Clone, Debug)]
pub struct StructAttribute {
    /// The dictionary's /O attribute owner.
    pub owner: String,
    /// Attribute name, without the leading slash.
    pub key: String,
    /// Resolved attribute value.
    pub value: PdfObj,
}

/// A child structure element, marked-content item or referenced object.
#[derive(Clone, Debug)]
pub enum StructKid {
    /// A nested structure element.
    Element(StructElement),
    /// A marked-content sequence and its extracted text.
    Content {
        /// Zero-based page index.
        page: usize,
        /// Marked-content identifier on that page.
        mcid: i64,
        /// Text after marked-content replacements and reversal.
        text: String,
    },
    /// An object reference dictionary (OBJR).
    Object {
        /// Page from the OBJR or its containing element, when supplied.
        page: Option<usize>,
        /// The referenced PDF object.
        object: ObjRef,
    },
}

impl StructElement {
    /// The subtree's text in /K order; /ActualText replaces its subtree (14.9.4).
    pub fn text(&self) -> String {
        let mut text = String::new();
        let mut pending = vec![TextPart::Element(self)];
        while let Some(part) = pending.pop() {
            match part {
                TextPart::Text(value) => text.push_str(value),
                TextPart::Element(element) => {
                    if let Some(actual) = &element.actual_text {
                        text.push_str(actual);
                    } else {
                        for kid in element.kids.iter().rev() {
                            match kid {
                                StructKid::Element(child) => pending.push(TextPart::Element(child)),
                                StructKid::Content { text, .. } => pending.push(TextPart::Text(text)),
                                StructKid::Object { .. } => {}
                            }
                        }
                    }
                }
            }
        }
        text
    }

    /// Find an /A attribute by owner and key, including revision-bearing arrays.
    pub fn attribute(&self, owner: &str, key: &str) -> Option<&PdfObj> {
        self.attributes.iter().rev()
            .find(|attribute| attribute.owner == owner && attribute.key == key)
            .map(|attribute| &attribute.value)
    }

    /// Iterate over direct child elements in logical order.
    pub fn elements(&self) -> impl Iterator<Item = &StructElement> {
        self.kids.iter().filter_map(|kid| match kid {
            StructKid::Element(element) => Some(element),
            _ => None,
        })
    }
}

impl Drop for StructElement {
    fn drop(&mut self) {
        // A corrupt or very deep tree must not overflow the call stack on drop.
        let mut pending = std::mem::take(&mut self.kids);
        while let Some(kid) = pending.pop() {
            if let StructKid::Element(mut child) = kid {
                pending.append(&mut child.kids);
            }
        }
    }
}

enum TextPart<'a> {
    Element(&'a StructElement),
    Text(&'a str),
}

// Use work lists for document-controlled tree depth, rather than recursive walks.
struct ElementNode {
    element: StructElement,
    kids: Vec<NodeKid>,
}

enum NodeKid {
    Element(usize),
    Value(StructKid),
}

enum Visit {
    Kid(PdfObj, Option<usize>),
    EndArray(ObjRef),
}

struct Reader<'a, 'data> {
    doc: &'a mut PdfDocument<'data>,
    problems: Vec<String>,
    notes: Vec<String>,
    roles: PdfDict,
    parents: HashMap<i64, PdfObj>,
    page_indices: HashMap<u32, usize>,
    // Page -> MCID -> containing element indices. Keep duplicates for diagnostics.
    references: Vec<HashMap<i64, Vec<usize>>>,
    objects: HashSet<ObjRef>,
    nodes: Vec<ElementNode>,
    roots: Vec<usize>,
}

impl StructTree {
    /// Read the structure, or return None when the catalog has no /StructTreeRoot.
    pub fn read(doc: &mut PdfDocument<'_>) -> PdfResult<Option<StructTree>> {
        let catalog = doc.catalog()?;
        let root_obj = match catalog.get("StructTreeRoot") {
            Some(value) if !matches!(value, PdfObj::Null) => value.clone(),
            _ => return Ok(None),
        };
        let root = resolve_dict(doc, &root_obj)?;
        let mark_info = optional_dict(doc, catalog.get("MarkInfo"))?;
        let marked = resolved_entry(doc, &mark_info, "Marked")?.as_bool() == Some(true);
        let lang = text_entry(doc, &catalog, "Lang")?;
        let preferences = optional_dict(doc, catalog.get("ViewerPreferences"))?;
        let direction = resolved_entry(doc, &preferences, "Direction")?
            .as_name().map(str::to_owned);
        let roles = optional_dict(doc, root.get("RoleMap"))?;
        let page_count = doc.page_count();
        let page_indices = (0..page_count).filter_map(|index| {
            doc.page_object_number(index).map(|number| (number, index))
        }).collect();
        let mut reader = Reader {
            doc,
            problems: Vec::new(),
            notes: Vec::new(),
            roles,
            parents: HashMap::new(),
            page_indices,
            references: vec![HashMap::new(); page_count],
            objects: HashSet::new(),
            nodes: Vec::new(),
            roots: Vec::new(),
        };
        if !marked {
            reader.problems.push("14.7.1 / 14.8.1: StructTreeRoot exists but catalog /MarkInfo /Marked is not true".into());
        }
        if resolved_entry(reader.doc, &root, "Type")?.as_name() != Some("StructTreeRoot") {
            reader.problems.push(format!("14.7.2: {} /Type is not /StructTreeRoot", location(root_obj.as_ref())));
        }
        if let Some(parent_tree) = root.get("ParentTree") {
            reader.read_parent_tree(parent_tree)?;
        }
        reader.read_elements(&root, root_obj.as_ref())?;

        let mut artifacts = Vec::with_capacity(page_count);
        let mut untagged = Vec::with_capacity(page_count);
        let mut texts = Vec::with_capacity(page_count);
        for index in 0..page_count {
            let page = reader.doc.page(index)?;
            let content = PageReader::read(
                &page, index, marked, &reader.references[index], &reader.objects,
                &mut reader.problems,
            )?;
            reader.check_page(index, &page, &content)?;
            artifacts.push(content.artifacts.into_iter().map(|text| text.text).collect());
            untagged.push(content.untagged.into_iter().map(|text| text.text).collect());
            texts.push(content.mcids);
        }
        for node in &mut reader.nodes {
            for kid in &mut node.kids {
                if let NodeKid::Value(StructKid::Content { page, mcid, text }) = kid {
                    if let Some(found) = texts.get(*page).and_then(|page| page.get(mcid)) {
                        *text = found.text.clone();
                    }
                }
            }
        }
        // Children were allocated after their parent. Assemble from the leaves up.
        let mut built: Vec<Option<StructElement>> = (0..reader.nodes.len()).map(|_| None).collect();
        for (index, node) in reader.nodes.into_iter().enumerate().rev() {
            let mut element = node.element;
            for kid in node.kids {
                match kid {
                    NodeKid::Element(child) => {
                        if let Some(value) = built.get_mut(child).and_then(Option::take) {
                            element.kids.push(StructKid::Element(value));
                        }
                    }
                    NodeKid::Value(value) => element.kids.push(value),
                }
            }
            check_child_types(&element, &mut reader.notes);
            built[index] = Some(element);
        }
        let roots = reader.roots.into_iter()
            .filter_map(|index| built.get_mut(index).and_then(Option::take)).collect();
        Ok(Some(Self {
            marked, lang, direction, roots, artifacts, untagged,
            problems: reader.problems, notes: reader.notes,
        }))
    }
}

impl Reader<'_, '_> {
    fn read_elements(&mut self, root: &PdfDict, root_ref: Option<ObjRef>) -> PdfResult<()> {
        let mut pending = Vec::new();
        if let Some(kids) = root.get("K") {
            pending.push(Visit::Kid(kids.clone(), None));
        }
        let mut seen = HashSet::new();
        if let Some(reference) = root_ref {
            seen.insert(reference);
        }
        let mut arrays = HashSet::new();
        while let Some(visit) = pending.pop() {
            let (obj, parent) = match visit {
                Visit::EndArray(reference) => {
                    arrays.remove(&reference);
                    continue;
                }
                Visit::Kid(obj, parent) => (obj, parent),
            };
            let reference = obj.as_ref();
            let resolved = resolve(self.doc, &obj)?;
            match resolved {
                PdfObj::Array(kids) => {
                    if let Some(reference) = reference {
                        if !arrays.insert(reference) {
                            self.problems.push(format!("14.7.2: cyclic /K array at object {}", reference.num));
                            continue;
                        }
                        pending.push(Visit::EndArray(reference));
                    }
                    pending.extend(kids.into_iter().rev().map(|kid| Visit::Kid(kid, parent)));
                }
                PdfObj::Int(mcid) => {
                    if let Some(parent) = parent {
                        let page = self.nodes[parent].element.page;
                        if let Some(page) = page {
                            self.add_content(parent, page, mcid);
                        } else {
                            self.problems.push(format!("14.7.4.2: integer MCID {mcid} in {} has no element /Pg", location(self.nodes[parent].element.object)));
                        }
                    } else {
                        self.problems.push(format!("14.7.2: StructTreeRoot /K contains integer MCID {mcid}, not an element"));
                    }
                }
                PdfObj::Dict(dict) => {
                    let type_obj = resolved_entry(self.doc, &dict, "Type")?;
                    match type_obj.as_name() {
                        Some("MCR") | Some("OBJR") => {
                            if let Some(parent) = parent {
                                self.read_item(parent, &dict, type_obj.as_name() == Some("MCR"))?;
                            } else {
                                self.problems.push("14.7.2: StructTreeRoot /K contains a content item, not an element".into());
                            }
                        }
                        _ => {
                            if let Some(reference) = reference {
                                if !seen.insert(reference) {
                                    self.problems.push(format!("14.7.2: element object {} reached twice (cycle or multiple parents)", reference.num));
                                    continue;
                                }
                            } else {
                                self.problems.push("14.7.2: structure element is not an indirect object".into());
                            }
                            let expected = parent.and_then(|index| self.nodes[index].element.object).or_else(|| {
                                if parent.is_none() { root_ref } else { None }
                            });
                            let where_ = location(reference);
                            if expected.is_none() || dict.get_ref("P") != expected {
                                self.problems.push(format!("14.7.2: {where_} /P is not an indirect reference to its containing parent {}", location(expected)));
                            }
                            if !matches!(type_obj, PdfObj::Null) && type_obj.as_name() != Some("StructElem") {
                                self.problems.push(format!("14.7.2: {where_} /Type is not /StructElem"));
                            }
                            let kind_obj = resolved_entry(self.doc, &dict, "S")?;
                            let kind = kind_obj.as_name().unwrap_or("").to_owned();
                            if kind.is_empty() {
                                self.problems.push(format!("14.7.2: {where_} missing a name-valued /S"));
                            }
                            let standard = self.standard_type(&kind, &where_)?;
                            let page = self.page_entry(&dict, "14.7.2", &where_)?;
                            let attributes = self.attributes(&dict, &where_)?;
                            let element = StructElement {
                                kind, standard, object: reference, page,
                                lang: text_entry(self.doc, &dict, "Lang")?,
                                alt: text_entry(self.doc, &dict, "Alt")?,
                                actual_text: text_entry(self.doc, &dict, "ActualText")?,
                                expansion: text_entry(self.doc, &dict, "E")?,
                                title: text_entry(self.doc, &dict, "T")?,
                                id: text_entry(self.doc, &dict, "ID")?,
                                attributes, kids: Vec::new(),
                            };
                            let index = self.nodes.len();
                            self.nodes.push(ElementNode { element, kids: Vec::new() });
                            if let Some(parent) = parent {
                                self.nodes[parent].kids.push(NodeKid::Element(index));
                            } else {
                                self.roots.push(index);
                            }
                            if let Some(kids) = dict.get("K") {
                                pending.push(Visit::Kid(kids.clone(), Some(index)));
                            }
                        }
                    }
                }
                PdfObj::Null => {}
                _ => self.problems.push(format!("14.7.2: invalid /K child in {}", location(parent.and_then(|index| self.nodes[index].element.object)))),
            }
        }
        Ok(())
    }

    fn page_entry(&mut self, dict: &PdfDict, section: &str, where_: &str) -> PdfResult<Option<usize>> {
        match dict.get("Pg") {
            None | Some(PdfObj::Null) => Ok(None),
            Some(PdfObj::Ref(reference)) => {
                let page = self.page_indices.get(&reference.num).copied();
                if page.is_none() {
                    self.problems.push(format!("{section}: {where_} /Pg object {} is not a page", reference.num));
                }
                Ok(page)
            }
            Some(_) => {
                self.problems.push(format!("{section}: {where_} /Pg is not an indirect page reference"));
                Ok(None)
            }
        }
    }

    fn read_item(&mut self, parent: usize, dict: &PdfDict, mcr: bool) -> PdfResult<()> {
        let element = &self.nodes[parent].element;
        let owner = element.object;
        let inherited_page = element.page;
        let where_ = format!("{} in {}", if mcr { "MCR" } else { "OBJR" }, location(owner));
        let page = self.page_entry(dict, "14.7.4.2", &where_)?.or(inherited_page);
        if mcr {
            let mcid = integer(&resolved_entry(self.doc, dict, "MCID")?);
            if mcid.is_none() {
                self.problems.push(format!("14.7.4.2: {where_} has no integer /MCID"));
            }
            if page.is_none() {
                self.problems.push(format!("14.7.4.2: {where_} has no page /Pg on it or its element"));
            }
            if dict.get("Stm").is_some_and(|obj| !matches!(obj, PdfObj::Null)) {
                self.notes.push(format!("14.7.4.2: {where_}: content in a stream other than a page's is not read"));
            } else if let (Some(page), Some(mcid)) = (page, mcid) {
                self.add_content(parent, page, mcid);
            }
        } else if let Some(object) = dict.get_ref("Obj") {
            // An OBJR may name a stream object (for example, a Form XObject).
            let target = resolve(self.doc, &PdfObj::Ref(object))?;
            let target = target.as_dict()
                .ok_or_else(|| PdfError::new(format!("14.7.4.3: OBJR object {} is not a dictionary or stream", object.num)))?;
            let key = integer(&resolved_entry(self.doc, target, "StructParent")?);
            match key {
                Some(key) if owner.is_some() && self.parents.get(&key).and_then(PdfObj::as_ref) == owner => {}
                Some(key) => self.problems.push(format!("14.7.4.4: {where_}, page {page:?}, object {}: parent-tree key {key} does not name its element", object.num)),
                None => self.problems.push(format!("14.7.4.4: {where_}, page {page:?}, object {} has no integer /StructParent", object.num)),
            }
            self.objects.insert(object);
            self.nodes[parent].kids.push(NodeKid::Value(StructKid::Object { page, object }));
        } else {
            self.problems.push(format!("14.7.4.3: {where_} has no indirect /Obj reference"));
        }
        Ok(())
    }

    fn add_content(&mut self, parent: usize, page: usize, mcid: i64) {
        if mcid < 0 {
            self.problems.push(format!("14.7.4.2: page {page}, negative MCID {mcid} in {}", location(self.nodes[parent].element.object)));
        }
        self.references[page].entry(mcid).or_default().push(parent);
        self.nodes[parent].kids.push(NodeKid::Value(StructKid::Content { page, mcid, text: String::new() }));
    }

    fn standard_type(&mut self, kind: &str, where_: &str) -> PdfResult<String> {
        let mut current = kind.to_owned();
        let mut visited = HashSet::new();
        while !standard_type(&current) && visited.insert(current.clone()) {
            let next = resolved_entry(self.doc, &self.roles, &current)?;
            match next.as_name() {
                Some(next) => current = next.to_owned(),
                None => break,
            }
        }
        if !standard_type(&current) {
            self.problems.push(format!("14.8.4: {where_} /S /{kind} does not map to a standard structure type (ended at /{current})"));
        }
        Ok(current)
    }

    fn attributes(&mut self, dict: &PdfDict, where_: &str) -> PdfResult<Vec<StructAttribute>> {
        let mut result = Vec::new();
        let value = resolved_entry(self.doc, dict, "A")?;
        let values = match value {
            PdfObj::Null => return Ok(result),
            PdfObj::Array(values) => values,
            value => vec![value],
        };
        let mut previous_dictionary = false;
        for value in values {
            match resolve(self.doc, &value)? {
                PdfObj::Int(revision) if revision >= 0 && previous_dictionary => {
                    previous_dictionary = false;
                }
                PdfObj::Dict(attribute) => {
                    previous_dictionary = true;
                    let owner = resolved_entry(self.doc, &attribute, "O")?.as_name().unwrap_or("").to_owned();
                    if owner.is_empty() {
                        self.problems.push(format!("14.7.5: {where_} attribute dictionary has no name-valued /O"));
                    }
                    let mut entries: Vec<_> = attribute.map.into_iter().collect();
                    entries.sort_by(|a, b| a.0.cmp(&b.0));
                    for (key, value) in entries {
                        if key == "O" { continue; }
                        let value = resolve(self.doc, &value)?;
                        if let Some(allowed) = attribute_values(&owner, &key) {
                            if !value.as_name().is_some_and(|name| allowed.contains(&name)) {
                                self.problems.push(format!("14.8.5: {where_} /{owner} /{key} has invalid value {value:?}"));
                            }
                        }
                        result.push(StructAttribute { owner: owner.clone(), key, value });
                    }
                }
                _ => {
                    previous_dictionary = false;
                    self.problems.push(format!("14.7.5: {where_} /A contains neither an attribute dictionary nor its revision number"));
                }
            }
        }
        Ok(result)
    }

    fn check_page(&mut self, index: usize, page: &PdfPage, content: &PageContent) -> PdfResult<()> {
        let references = &self.references[index];
        let mut mcids: Vec<_> = content.mcids.keys().copied().collect();
        mcids.sort_unstable();
        for mcid in mcids {
            if !references.contains_key(&mcid) {
                self.problems.push(format!("14.7.4.2: page {index}, MCID {mcid} has no structure content item"));
            }
        }
        let mut referenced: Vec<_> = references.iter().collect();
        referenced.sort_by_key(|(mcid, _)| **mcid);
        let parent_array = match page.struct_parents.and_then(|key| self.parents.get(&key)) {
            Some(value) => Some(resolve(self.doc, value)?),
            None => None,
        };
        if !content.mcids.is_empty() {
            match page.struct_parents {
                None => self.problems.push(format!("14.7.4.4: page {index} has MCID sequences but no /StructParents")),
                Some(key) if parent_array.as_ref().and_then(PdfObj::as_array).is_none() => {
                    self.problems.push(format!("14.7.4.4: page {index} parent-tree key {key} is missing or is not an array"));
                }
                _ => {}
            }
        }
        for (&mcid, owners) in referenced {
            if !content.mcids.contains_key(&mcid) {
                self.problems.push(format!("14.7.4.2: page {index}, MCID {mcid} is referenced but no sequence carries it"));
            }
            if owners.len() > 1 {
                self.problems.push(format!("14.7.4.2: page {index}, MCID {mcid} has {} structure content items", owners.len()));
            }
            for &owner in owners {
                let expected = self.nodes[owner].element.object;
                let actual = parent_array.as_ref().and_then(PdfObj::as_array)
                    .and_then(|array| usize::try_from(mcid).ok().and_then(|index| array.get(index)))
                    .and_then(PdfObj::as_ref);
                if expected.is_none() || actual != expected {
                    self.problems.push(format!("14.7.4.4: page {index}, MCID {mcid}, /StructParents {:?}: parent-tree array entry does not name {}", page.struct_parents, location(expected)));
                }
            }
        }
        Ok(())
    }

    fn read_parent_tree(&mut self, root: &PdfObj) -> PdfResult<()> {
        // Exit tasks propagate the actual descendant limits back to their parent.
        enum Task { Enter(PdfObj, Option<usize>), Exit(usize) }
        struct Node {
            parent: Option<usize>,
            limits: Option<(i64, i64)>,
            actual: Option<(i64, i64)>,
            where_: String,
        }
        let mut pending = vec![Task::Enter(root.clone(), None)];
        let mut nodes: Vec<Node> = Vec::new();
        let mut visited = HashSet::new();
        while let Some(task) = pending.pop() {
            let (obj, parent) = match task {
                Task::Exit(index) => {
                    let node = &nodes[index];
                    if node.limits.is_some() && node.limits != node.actual {
                        self.problems.push(format!("7.9.7: parent-tree {} /Limits do not match its descendant keys", node.where_));
                    }
                    if let (Some(parent), Some(actual)) = (node.parent, node.actual) {
                        if nodes[parent].actual.is_some_and(|previous| actual.0 <= previous.1) {
                            self.problems.push(format!("7.9.7: parent-tree {} child key ranges overlap or are out of order", nodes[parent].where_));
                        }
                        extend_limits(&mut nodes[parent].actual, actual);
                    }
                    continue;
                }
                Task::Enter(obj, parent) => (obj, parent),
            };
            let where_ = location(obj.as_ref());
            if let Some(reference) = obj.as_ref() {
                if !visited.insert(reference) {
                    self.problems.push(format!("7.9.7: parent-tree {where_} is reached twice"));
                    continue;
                }
            }
            let dict = resolve_dict(self.doc, &obj)?;
            let limits_obj = resolved_entry(self.doc, &dict, "Limits")?;
            let limits = limits_obj.as_array().and_then(|values| {
                if values.len() == 2 { Some((integer(&values[0])?, integer(&values[1])?)) } else { None }
            });
            if (parent.is_some() || !matches!(limits_obj, PdfObj::Null)) && limits.is_none() {
                self.problems.push(format!("7.9.7: parent-tree {where_} has no valid /Limits"));
            }
            let index = nodes.len();
            nodes.push(Node { parent, limits, actual: None, where_: where_.clone() });
            pending.push(Task::Exit(index));
            match resolved_entry(self.doc, &dict, "Nums")? {
                PdfObj::Array(values) => {
                    if values.len() % 2 != 0 {
                        self.problems.push(format!("7.9.7: parent-tree {where_} /Nums has an unpaired key"));
                    }
                    let mut previous = None;
                    for pair in values.chunks_exact(2) {
                        if let Some(key) = integer(&resolve(self.doc, &pair[0])?) {
                            if previous.is_some_and(|previous| key <= previous) {
                                self.problems.push(format!("7.9.7: parent-tree {where_} /Nums keys are not strictly increasing"));
                            }
                            previous = Some(key);
                            extend_limits(&mut nodes[index].actual, (key, key));
                            if self.parents.insert(key, pair[1].clone()).is_some() {
                                self.problems.push(format!("7.9.7: duplicate parent-tree key {key} in {where_}"));
                            }
                        } else {
                            self.problems.push(format!("7.9.7: parent-tree {where_} /Nums has a non-integer key"));
                        }
                    }
                }
                PdfObj::Null => {}
                _ => self.problems.push(format!("7.9.7: parent-tree {where_} /Nums is not an array")),
            }
            match resolved_entry(self.doc, &dict, "Kids")? {
                PdfObj::Array(kids) => {
                    for kid in kids.into_iter().rev() {
                        if kid.as_ref().is_none() {
                            self.problems.push(format!("7.9.7: parent-tree {where_} /Kids entry is not an indirect reference"));
                        }
                        pending.push(Task::Enter(kid, Some(index)));
                    }
                }
                PdfObj::Null => {}
                _ => self.problems.push(format!("7.9.7: parent-tree {where_} /Kids is not an array")),
            }
        }
        Ok(())
    }
}

fn extend_limits(limits: &mut Option<(i64, i64)>, value: (i64, i64)) {
    *limits = Some(match *limits {
        Some((low, high)) => (low.min(value.0), high.max(value.1)),
        None => value,
    });
}

fn resolve(doc: &mut PdfDocument<'_>, obj: &PdfObj) -> PdfResult<PdfObj> {
    let mut value = obj.clone();
    let mut seen = HashSet::new();
    while let PdfObj::Ref(reference) = value {
        if !seen.insert(reference) {
            return Err(PdfError::new(format!("cyclic indirect reference at object {}", reference.num)));
        }
        value = doc.resolve_ref(reference)?;
    }
    Ok(value)
}

fn resolve_dict(doc: &mut PdfDocument<'_>, obj: &PdfObj) -> PdfResult<PdfDict> {
    match resolve(doc, obj)? {
        PdfObj::Dict(dict) => Ok(dict),
        _ => Err(PdfError::new("structure entry is not a dictionary")),
    }
}

fn optional_dict(doc: &mut PdfDocument<'_>, obj: Option<&PdfObj>) -> PdfResult<PdfDict> {
    match obj {
        None | Some(PdfObj::Null) => Ok(PdfDict::new()),
        Some(obj) => resolve_dict(doc, obj),
    }
}

fn resolved_entry(doc: &mut PdfDocument<'_>, dict: &PdfDict, key: &str) -> PdfResult<PdfObj> {
    match dict.get(key) {
        Some(value) => resolve(doc, value),
        None => Ok(PdfObj::Null),
    }
}

fn integer(obj: &PdfObj) -> Option<i64> {
    match obj { PdfObj::Int(value) => Some(*value), _ => None }
}

fn text_entry(doc: &mut PdfDocument<'_>, dict: &PdfDict, key: &str) -> PdfResult<Option<String>> {
    Ok(resolved_entry(doc, dict, key)?.as_str_bytes().map(text_string))
}

fn text_string(bytes: &[u8]) -> String {
    if let Some(bytes) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units = bytes.chunks(2).map(|pair| {
            if pair.len() == 2 { u16::from_be_bytes([pair[0], pair[1]]) } else { 0xfffd }
        });
        char::decode_utf16(units).map(|value| value.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
    } else {
        // The crate has no PDFDocEncoding table; retain the Latin-1 fallback.
        bytes.iter().map(|&byte| char::from(byte)).collect()
    }
}

fn location(reference: Option<ObjRef>) -> String {
    reference.map_or_else(|| "direct object".into(), |reference| format!("object {}", reference.num))
}

fn standard_type(kind: &str) -> bool {
    matches!(kind, "Document" | "Part" | "Art" | "Sect" | "Div" | "BlockQuote" |
        "Caption" | "TOC" | "TOCI" | "Index" | "NonStruct" | "Private" | "P" |
        "H" | "H1" | "H2" | "H3" | "H4" | "H5" | "H6" | "L" | "LI" | "Lbl" |
        "LBody" | "Table" | "TR" | "TH" | "TD" | "THead" | "TBody" | "TFoot" |
        "Span" | "Quote" | "Note" | "Reference" | "BibEntry" | "Code" | "Link" |
        "Annot" | "Ruby" | "RB" | "RT" | "RP" | "Warichu" | "WT" | "WP" |
        "Figure" | "Formula" | "Form")
}

fn attribute_values(owner: &str, key: &str) -> Option<&'static [&'static str]> {
    match (owner, key) {
        ("Layout", "Placement") => Some(&["Block", "Inline", "Before", "Start", "End"]),
        ("Layout", "WritingMode") => Some(&["LrTb", "RlTb", "TbRl"]),
        ("List", "ListNumbering") => Some(&["None", "Disc", "Circle", "Square", "Decimal", "UpperRoman", "LowerRoman", "UpperAlpha", "LowerAlpha"]),
        ("Table", "Scope") => Some(&["Row", "Column", "Both"]),
        _ => None,
    }
}

fn check_child_types(element: &StructElement, notes: &mut Vec<String>) {
    let (section, allowed): (&str, &[&str]) = match element.standard.as_str() {
        "L" => ("14.8.4.3.3", &["LI", "L", "Caption"]),
        "LI" => ("14.8.4.3.3", &["Lbl", "LBody"]),
        "Table" => ("14.8.4.3.4", &["TR", "THead", "TBody", "TFoot", "Caption"]),
        "THead" | "TBody" | "TFoot" => ("14.8.4.3.4", &["TR"]),
        "TR" => ("14.8.4.3.4", &["TH", "TD"]),
        _ => return,
    };
    for kid in &element.kids {
        let description = match kid {
            StructKid::Element(kid) if allowed.contains(&kid.standard.as_str()) => continue,
            StructKid::Element(kid) => format!("/{} child {}", kid.standard, location(kid.object)),
            StructKid::Content { page, mcid, .. } => format!("direct content child on page {page}, MCID {mcid}"),
            StructKid::Object { object, .. } => format!("direct OBJR child object {}", object.num),
        };
        notes.push(format!("{section}: /{} {} has {description}", element.standard, location(element.object)));
    }
}

#[derive(Default)]
struct TextBuffer {
    text: String,
    y: Option<f64>,
}

impl TextBuffer {
    /// Appends text drawn at baseline `y` with an em of `size`: a baseline that moves by half
    /// an em or more starts a new line; a smaller move (a superscript, a mark) does not.
    fn push(&mut self, text: &str, y: f64, size: f64) {
        if text.is_empty() { return; }
        if self.y.is_some_and(|previous| (previous - y).abs() >= (size * 0.5).max(0.5)) {
            self.text.push('\n');
        }
        self.text.push_str(text);
        self.y = Some(y);
    }
}

#[derive(Default)]
struct PageContent {
    mcids: HashMap<i64, TextBuffer>,
    artifacts: Vec<TextBuffer>,
    untagged: Vec<TextBuffer>,
}

struct Sequence {
    tag: String,
    mcid: Option<i64>,
    // Effective context, cached at BMC/BDC so each text unit takes constant work.
    content: Option<i64>,
    artifact: Option<usize>,
    replacement: Option<usize>,
    reversed: bool,
    actual_text: Option<String>,
    emitted: bool,
}

#[derive(Clone, Default)]
struct TextState {
    font: String,
    size: f64,
    leading: f64,
}

struct PageReader<'a> {
    page: &'a PdfPage,
    index: usize,
    marked: bool,
    references: &'a HashMap<i64, Vec<usize>>,
    objects: &'a HashSet<ObjRef>,
    problems: &'a mut Vec<String>,
    content: PageContent,
    sequences: Vec<Sequence>,
    untagged_run: Option<usize>,
    state: TextState,
    line: [f64; 6],
}

impl<'a> PageReader<'a> {
    fn read(
        page: &'a PdfPage,
        index: usize,
        marked: bool,
        references: &'a HashMap<i64, Vec<usize>>,
        objects: &'a HashSet<ObjRef>,
        problems: &'a mut Vec<String>,
    ) -> PdfResult<PageContent> {
        let mut reader = Self {
            page, index, marked, references, objects, problems,
            content: PageContent::default(), sequences: Vec::new(), untagged_run: None,
            state: TextState::default(), line: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        };
        let mut saved = Vec::new();
        for op in parse_content_stream(&page.content_data)? {
            match op {
                PdfOp::SaveState => saved.push(reader.state.clone()),
                PdfOp::RestoreState => {
                    if let Some(state) = saved.pop() { reader.state = state; }
                }
                PdfOp::BeginText => reader.line = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                PdfOp::SetFont(font, size) => {
                    reader.state.font = font;
                    reader.state.size = size;
                }
                PdfOp::SetTextLeading(leading) => reader.state.leading = leading,
                PdfOp::SetTextMatrix(matrix) => reader.line = matrix,
                PdfOp::MoveText(tx, ty) => tm_pre_translate(&mut reader.line, tx, ty),
                PdfOp::MoveTextSetLeading(tx, ty) => {
                    reader.state.leading = -ty;
                    tm_pre_translate(&mut reader.line, tx, ty);
                }
                PdfOp::NextLine => tm_pre_translate(&mut reader.line, 0.0, -reader.state.leading),
                PdfOp::ShowText(bytes) => reader.show(&bytes, "Tj"),
                PdfOp::ShowTextArray(items) => {
                    if !items.iter().any(|item| matches!(item, TextArrayItem::Text(_))) {
                        reader.real("TJ", Some(""));
                    }
                    for item in items {
                        if let TextArrayItem::Text(bytes) = item { reader.show(&bytes, "TJ"); }
                    }
                }
                PdfOp::ShowTextNextLine(bytes) => {
                    tm_pre_translate(&mut reader.line, 0.0, -reader.state.leading);
                    reader.show(&bytes, "'");
                }
                PdfOp::ShowTextNextLineSpacing(_, _, bytes) => {
                    tm_pre_translate(&mut reader.line, 0.0, -reader.state.leading);
                    reader.show(&bytes, "\"");
                }
                PdfOp::BeginMarkedContent(tag) => reader.begin(tag, &PdfDict::new()),
                PdfOp::BeginMarkedContentProps(tag, properties) => {
                    let dict = match properties {
                        PdfObj::Dict(dict) => dict,
                        PdfObj::Name(name) => match reader.page.properties.get(&name) {
                            Some(dict) => dict.clone(),
                            None => {
                                reader.problems.push(format!("14.6: page {index}, /{tag} BDC property list /{name} is not in /Resources /Properties"));
                                PdfDict::new()
                            }
                        },
                        _ => {
                            reader.problems.push(format!("14.6: page {index}, /{tag} BDC property list is neither a dictionary nor a name"));
                            PdfDict::new()
                        }
                    };
                    reader.begin(tag, &dict);
                }
                PdfOp::EndMarkedContent => reader.end(),
                PdfOp::PaintXObject(name) => {
                    if !reader.page.xobjects.get(&name)
                        .is_some_and(|object| reader.objects.contains(&object.obj_ref))
                    {
                        reader.real("Do", None);
                    }
                }
                PdfOp::PaintShading(_) => reader.real("sh", None),
                PdfOp::InlineImage { .. } => reader.real("BI", None),
                PdfOp::Stroke => reader.real("S", None),
                PdfOp::CloseStroke => reader.real("s", None),
                PdfOp::Fill => reader.real("f/F", None),
                PdfOp::FillEvenOdd => reader.real("f*", None),
                PdfOp::FillStroke => reader.real("B", None),
                PdfOp::FillStrokeEvenOdd => reader.real("B*", None),
                PdfOp::CloseFillStroke => reader.real("b", None),
                PdfOp::CloseFillStrokeEvenOdd => reader.real("b*", None),
                _ => {}
            }
        }
        while let Some(sequence) = reader.sequences.last() {
            reader.problems.push(format!("14.6: page {index}, /{} MCID {:?} is still open at end of content", sequence.tag, sequence.mcid));
            reader.end();
        }
        Ok(reader.content)
    }

    fn begin(&mut self, tag: String, properties: &PdfDict) {
        let mcid = properties.get("MCID").and_then(integer);
        if properties.get("MCID").is_some_and(|value| !matches!(value, PdfObj::Null)) && mcid.is_none() {
            self.problems.push(format!("14.7.4.2: page {}, /{tag} has a non-integer /MCID", self.index));
        }
        let parent = self.sequences.last();
        let mut artifact = parent.and_then(|sequence| sequence.artifact);
        let content = mcid.or_else(|| parent.and_then(|sequence| sequence.content));
        let mut replacement = parent.and_then(|sequence| sequence.replacement);
        let reversed = tag == "ReversedChars" || parent.is_some_and(|sequence| sequence.reversed);
        if tag == "Artifact" && artifact.is_none() {
            artifact = Some(self.content.artifacts.len());
            self.content.artifacts.push(TextBuffer::default());
        }
        if let Some(mcid) = mcid {
            if self.content.mcids.contains_key(&mcid) {
                self.problems.push(format!("14.7.4.2: page {}, MCID {mcid} is carried by two marked-content sequences", self.index));
            } else {
                self.content.mcids.insert(mcid, TextBuffer::default());
            }
            if mcid < 0 {
                self.problems.push(format!("14.7.4.2: page {}, negative marked-content MCID {mcid}", self.index));
            }
            if artifact.is_some() {
                self.problems.push(format!("14.8.2.2.2: page {}, MCID {mcid} is inside an artifact sequence", self.index));
            }
        }
        let actual_text = properties.get_str("ActualText").map(text_string);
        if replacement.is_none() && actual_text.is_some() {
            replacement = Some(self.sequences.len());
        }
        self.sequences.push(Sequence {
            tag, mcid, content, artifact, replacement, reversed, actual_text, emitted: false,
        });
        self.untagged_run = None;
    }

    fn end(&mut self) {
        if let Some(index) = self.sequences.len().checked_sub(1) {
            let sequence = &self.sequences[index];
            // Empty replacements are emitted at their own EMC, not at a nested EMC.
            if sequence.replacement == Some(index) && !sequence.emitted {
                self.unit("");
            }
            self.sequences.pop();
            self.untagged_run = None;
        } else {
            self.problems.push(format!("14.6: page {}, EMC has no open marked-content sequence", self.index));
        }
    }

    fn show(&mut self, bytes: &[u8], operator: &str) {
        let mut units: Vec<String> = match self.page.fonts.get(&self.state.font) {
            Some(font) => decode_codes(font, bytes).into_iter().map(|(_, text)| text).collect(),
            None => {
                self.problems.push(format!("9.3.2: page {}, {operator} has no resolved current font /{}", self.index, self.state.font));
                bytes.iter().map(|&byte| char::from(byte).to_string()).collect()
            }
        };
        if self.sequences.last().is_some_and(|sequence| sequence.reversed) {
            units.reverse();
        }
        if self.outside_structure() && self.marked {
            self.real(operator, Some(&units.concat()));
        }
        for unit in units { self.unit(&unit); }
    }

    fn outside_structure(&self) -> bool {
        let context = self.sequences.last();
        context.and_then(|sequence| sequence.artifact).is_none()
            && !context.and_then(|sequence| sequence.content)
                .is_some_and(|mcid| self.references.contains_key(&mcid))
    }

    fn real(&mut self, operator: &str, text: Option<&str>) {
        if self.marked && self.outside_structure() {
            let text = text.map_or_else(String::new, |text| format!(", text {text:?}"));
            let mcid = self.sequences.last().and_then(|sequence| sequence.content);
            self.problems.push(format!("14.8.2.2: page {}, MCID {mcid:?}, operator {operator}{text} is outside artifacts and structure content items", self.index));
        }
    }

    fn unit(&mut self, text: &str) {
        let replacement = self.sequences.last().and_then(|sequence| sequence.replacement);
        let actual;
        let text = if let Some(index) = replacement {
            let sequence = &mut self.sequences[index];
            if sequence.emitted { return; }
            sequence.emitted = true;
            actual = sequence.actual_text.clone().unwrap_or_default();
            &actual
        } else { text };
        let context = self.sequences.last();
        let y = self.line[5];
        // The em in user space: the font size scaled by the text matrix's vertical axis.
        let size = self.state.size * self.line[2].hypot(self.line[3]);
        if let Some(artifact) = context.and_then(|sequence| sequence.artifact) {
            self.content.artifacts[artifact].push(text, y, size);
        } else if let Some(mcid) = context.and_then(|sequence| sequence.content) {
            if let Some(buffer) = self.content.mcids.get_mut(&mcid) {
                buffer.push(text, y, size);
            }
            if !self.references.contains_key(&mcid) { self.untagged(text, y, size); }
        } else {
            self.untagged(text, y, size);
        }
    }

    fn untagged(&mut self, text: &str, y: f64, size: f64) {
        if text.is_empty() { return; }
        let index = match self.untagged_run {
            Some(index) => index,
            None => {
                let index = self.content.untagged.len();
                self.content.untagged.push(TextBuffer::default());
                self.untagged_run = Some(index);
                index
            }
        };
        self.content.untagged[index].push(text, y, size);
    }
}
