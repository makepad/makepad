use crate::object::{string, Objects};
use crate::{destination, OutlineEntry, PageRecord};

#[derive(Default)]
struct Node {
    object: usize,
    parent: usize,
    first: Option<usize>,
    last: Option<usize>,
    previous: Option<usize>,
    next: Option<usize>,
    descendants: usize,
}

pub(crate) fn write(
    objects: &mut Objects,
    pages: &[PageRecord],
    entries: Vec<OutlineEntry>,
) -> String {
    if entries.is_empty() {
        return String::new();
    }
    // Node zero is the root. A stack finds each nearest shallower predecessor in O(n).
    let root = objects.reserve();
    let mut nodes = Vec::with_capacity(entries.len() + 1);
    nodes.push(Node {
        object: root,
        ..Node::default()
    });
    let mut ancestors: Vec<usize> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        while let Some(&ancestor) = ancestors.last() {
            if entries[ancestor - 1].level < entry.level {
                break;
            }
            ancestors.pop();
        }
        let parent = ancestors.last().copied().unwrap_or(0);
        let node = index + 1;
        let previous = nodes[parent].last;
        if let Some(previous) = previous {
            nodes[previous].next = Some(node);
        } else {
            nodes[parent].first = Some(node);
        }
        nodes[parent].last = Some(node);
        nodes.push(Node {
            object: objects.reserve(),
            parent,
            previous,
            ..Node::default()
        });
        ancestors.push(node);
    }
    // Every parent precedes its children, so descendant totals need one reverse pass.
    for index in (1..nodes.len()).rev() {
        let parent = nodes[index].parent;
        nodes[parent].descendants += nodes[index].descendants + 1;
    }
    for (index, node) in nodes.iter().enumerate() {
        let mut body = if index == 0 {
            String::from("<< /Type /Outlines")
        } else {
            let entry = &entries[index - 1];
            format!(
                "<< /Title {} /Parent {} 0 R /Dest {}",
                string(&entry.title),
                nodes[node.parent].object,
                destination(pages, entry.page, entry.y),
            )
        };
        for (key, target) in [
            ("First", node.first),
            ("Last", node.last),
            ("Prev", node.previous),
            ("Next", node.next),
        ] {
            if let Some(target) = target {
                body.push_str(&format!(" /{key} {} 0 R", nodes[target].object));
            }
        }
        body.push_str(&format!(" /Count {} >>", node.descendants));
        objects.write(node.object, &body);
    }
    format!("/Outlines {root} 0 R /PageMode /UseOutlines")
}
