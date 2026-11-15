use crate::object::{number, string, Objects};
use crate::{
    ArtifactKind, ListNumbering, PageRecord, RunTag, StructAttrs, StructId, StructKind, Tagging,
    WritingMode,
};

struct Content {
    page: usize,
    mcid: usize,
}

struct Element {
    object: usize,
    parent: Option<StructId>,
    kind: StructKind,
    attrs: StructAttrs,
    children: Vec<StructId>,
    content: Vec<Content>,
}

pub(crate) struct Structure {
    root: usize,
    tagging: Tagging,
    elements: Vec<Element>,
    roots: Vec<StructId>,
    // Only pages with MCIDs need an entry; the owners are in MCID order.
    pages: Vec<(usize, Vec<usize>)>,
}

impl Structure {
    pub(crate) fn new(objects: &mut Objects, tagging: Tagging) -> Self {
        Self {
            root: objects.reserve(),
            tagging,
            elements: Vec::new(),
            roots: Vec::new(),
            pages: Vec::new(),
        }
    }

    pub(crate) fn add_element(
        &mut self,
        objects: &mut Objects,
        parent: Option<StructId>,
        kind: StructKind,
        attrs: StructAttrs,
    ) -> StructId {
        let id = StructId(self.elements.len());
        if let Some(parent) = parent {
            let parent = self.elements.get_mut(parent.0)
                .expect("structure parent must be known");
            assert!(
                parent.content.is_empty(),
                "structure parent already owns content"
            );
            parent.children.push(id);
        } else {
            self.roots.push(id);
        }
        self.elements.push(Element {
            object: objects.reserve(),
            parent,
            kind,
            attrs,
            children: Vec::new(),
            content: Vec::new(),
        });
        id
    }

    pub(crate) fn begin_group(
        &mut self,
        content: &mut String,
        tag: RunTag,
        page: usize,
        owners: &mut Vec<usize>,
    ) {
        match tag {
            RunTag::None => {}
            RunTag::Element(id) => {
                let element = self.elements.get_mut(id.0)
                    .expect("run structure element must be known");
                assert!(
                    element.children.is_empty(),
                    "run structure element already has children"
                );
                let mcid = owners.len();
                owners.push(element.object);
                element.content.push(Content { page, mcid });
                content.push_str(&format!("/{} <</MCID {mcid}>> BDC\n", element.kind.name()));
            }
            RunTag::Artifact(kind) => {
                let subtype = match kind {
                    ArtifactKind::Header => "Header",
                    ArtifactKind::Footer => "Footer",
                };
                content.push_str(&format!(
                    "/Artifact <</Type /Pagination /Subtype /{subtype}>> BDC\n"
                ));
            }
        }
    }

    pub(crate) fn add_page(&mut self, index: usize, owners: Vec<usize>) -> String {
        if owners.is_empty() {
            return String::new();
        }
        self.pages.push((index, owners));
        format!(" /StructParents {index} /Tabs /S")
    }

    pub(crate) fn write(self, objects: &mut Objects, pages: &[PageRecord]) -> String {
        let parent_tree = objects.reserve();
        for element in &self.elements {
            let parent = element.parent.map_or(self.root, |id| self.elements[id.0].object);
            let mut body = format!(
                "<< /Type /StructElem /S /{} /P {parent} 0 R", element.kind.name()
            );
            if !element.children.is_empty() {
                body.push_str(" /K [");
                for child in &element.children {
                    body.push_str(&format!("{} 0 R ", self.elements[child.0].object));
                }
                body.push(']');
            } else if let Some(first) = element.content.first() {
                body.push_str(&format!(" /Pg {} 0 R /K [", pages[first.page].object));
                for item in &element.content {
                    if item.page == first.page {
                        body.push_str(&format!("{} ", item.mcid));
                    } else {
                        body.push_str(&format!(
                            "<< /Type /MCR /Pg {} 0 R /MCID {} >> ",
                            pages[item.page].object, item.mcid,
                        ));
                    }
                }
                body.push(']');
            } else if let Some((page, _)) = element.attrs.bbox {
                let page = bbox_page(pages, page);
                body.push_str(&format!(" /Pg {} 0 R", page.object));
            }
            write_attrs(&mut body, &element.attrs, pages);
            if let Some(alt) = &element.attrs.alt {
                body.push_str(&format!(" /Alt {}", string(alt)));
            }
            if let Some(lang) = &element.attrs.lang {
                body.push_str(&format!(" /Lang {}", string(lang)));
            }
            body.push_str(" >>");
            objects.write(element.object, &body);
        }

        let mut root = String::from("<< /Type /StructTreeRoot /K [");
        for id in &self.roots {
            root.push_str(&format!("{} 0 R ", self.elements[id.0].object));
        }
        root.push_str(&format!(
            "] /ParentTree {parent_tree} 0 R /ParentTreeNextKey {} >>",
            pages.len(),
        ));
        objects.write(self.root, &root);

        let mut nums = String::from("<< /Nums [");
        for (index, owners) in &self.pages {
            nums.push_str(&format!("{index} ["));
            for object in owners {
                nums.push_str(&format!("{object} 0 R "));
            }
            nums.push_str("] ");
        }
        nums.push_str("] >>");
        objects.write(parent_tree, &nums);

        let mut catalog = format!(
            " /MarkInfo << /Marked true >> /StructTreeRoot {} 0 R", self.root
        );
        if let Some(lang) = &self.tagging.lang {
            catalog.push_str(&format!(" /Lang {}", string(lang)));
        }
        let direction = match self.tagging.direction {
            WritingMode::LrTb => "L2R",
            WritingMode::RlTb => "R2L",
        };
        catalog.push_str(&format!(" /ViewerPreferences << /Direction /{direction} >>"));
        catalog
    }
}

pub(crate) fn end_group(content: &mut String, tag: RunTag) {
    if tag != RunTag::None {
        content.push_str("EMC\n");
    }
}

fn bbox_page(pages: &[PageRecord], index: usize) -> &PageRecord {
    pages.get(index).unwrap_or_else(|| {
        panic!("structure BBox page {index} must be added before finish")
    })
}

fn write_attrs(body: &mut String, attrs: &StructAttrs, pages: &[PageRecord]) {
    let mut layout = String::new();
    if let Some(mode) = attrs.writing_mode {
        let mode = match mode {
            WritingMode::LrTb => "LrTb",
            WritingMode::RlTb => "RlTb",
        };
        layout.push_str(&format!(" /WritingMode /{mode}"));
    }
    if attrs.block {
        layout.push_str(" /Placement /Block");
    }
    if let Some((index, rect)) = attrs.bbox {
        let page = bbox_page(pages, index);
        layout.push_str(&format!(
            " /BBox [{} {} {} {}]",
            number(f64::from(rect.x)),
            number(f64::from(page.height) - f64::from(rect.y) - f64::from(rect.h)),
            number(f64::from(rect.x) + f64::from(rect.w)),
            number(f64::from(page.height) - f64::from(rect.y)),
        ));
    }
    let mut attributes = Vec::new();
    if !layout.is_empty() {
        attributes.push(format!("<< /O /Layout{layout} >>"));
    }
    if let Some(numbering) = attrs.list_numbering {
        let numbering = match numbering {
            ListNumbering::None => "None",
            ListNumbering::Disc => "Disc",
            ListNumbering::Decimal => "Decimal",
        };
        attributes.push(format!("<< /O /List /ListNumbering /{numbering} >>"));
    }
    if attrs.scope_column {
        attributes.push(String::from("<< /O /Table /Scope /Column >>"));
    }
    match attributes.len() {
        0 => {}
        1 => body.push_str(&format!(" /A {}", attributes[0])),
        _ => body.push_str(&format!(" /A [{}]", attributes.join(" "))),
    }
}
