//! Structural changes between Splash object trees. Child indices count only
//! ordered constructors. Typed map slots are patched as whole property values.
use super::literal_class::check_literal_class;
use super::widget::child_type;
use super::{Field, Kind, Loc, Node, Syntax};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Named(String),
    Bare { ty: String, ordinal: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AstPatchOp {
    SetProp { path: Vec<NodeKey>, prop: String, value_splash: String, literal_class: bool },
    RemoveProp { path: Vec<NodeKey>, prop: String },
    InsertChild { parent: Vec<NodeKey>, at_index: usize, name: Option<String>, splash: String, literal_class: bool },
    RemoveChild { parent: Vec<NodeKey>, key: NodeKey },
    ReorderChild { parent: Vec<NodeKey>, key: NodeKey, to_index: usize },
    RenameChild { parent: Vec<NodeKey>, old_key: NodeKey, new_name: String },
}

fn property_diff(base: &[Field], edited: &[Field], before: &str, after: &str, path: &[NodeKey], prefix: &str, ops: &mut Vec<AstPatchOp>) {
    let properties = |f: &&Field| f.name.is_some() && child_type(f).is_none();
    let old: HashMap<_, _> = base.iter().filter(properties).map(|f| (f.name.as_deref().unwrap(), f)).collect();
    let new: HashMap<_, _> = edited.iter().filter(properties).map(|f| (f.name.as_deref().unwrap(), f)).collect();
    for f in base.iter().filter(properties) {
        let name = f.name.as_deref().unwrap();
        if !new.contains_key(name) {
            ops.push(AstPatchOp::RemoveProp { path: path.to_vec(), prop: format!("{prefix}{name}") });
        }
    }
    for f in edited.iter().filter(properties) {
        let name = f.name.as_deref().unwrap();
        let prop = format!("{prefix}{name}");
        if let Some(previous) = old.get(name) {
            if previous.op == f.op {
                if previous.value.text(before) == f.value.text(after) { continue; }
                if let (Some((None, a)), Some((None, b))) = (previous.value.object(), f.value.object()) {
                    property_diff(a, b, before, after, path, &format!("{prop}."), ops);
                    continue;
                }
            }
        }
        ops.push(AstPatchOp::SetProp {
            path: path.to_vec(), prop, value_splash: f.value.text(after).to_owned(),
            literal_class: check_literal_class(&f.value, after).is_ok(),
        });
    }
}

fn key_for<'a>(name: Option<&str>, ty: &'a str, counts: &mut HashMap<&'a str, usize>) -> NodeKey {
    if let Some(name) = name { return NodeKey::Named(name.to_owned()); }
    let ordinal = counts.entry(ty).or_default();
    let key = NodeKey::Bare { ty: ty.to_owned(), ordinal: *ordinal };
    *ordinal += 1;
    key
}

struct Child<'a> {
    key: NodeKey,
    ty: &'a str,
    field: &'a Field,
}

fn children(fields: &[Field]) -> Vec<Child<'_>> {
    let mut counts = HashMap::new();
    fields.iter().filter_map(|field| {
        let ty = child_type(field)?;
        Some(Child { key: key_for(field.name.as_deref(), ty, &mut counts), ty, field })
    }).collect()
}

fn body_tokens<'a>(child: &Child, source: &'a str) -> Vec<&'a str> {
    let text = child.field.value.text(source);
    crate::tokenizer::lex(text).iter().map(|t| &text[t.span()]).collect()
}

// Keep the original index of each match. Resolve identity before comparing
// order. Equal constructor bodies pair first, then unambiguous type-only pairs.
fn match_children(old: &[Child], new: &[Child], before: &str, after: &str) -> (Vec<Option<usize>>, Vec<bool>) {
    let mut keyed: HashMap<_, VecDeque<_>> = HashMap::new();
    for (i, child) in old.iter().enumerate() {
        keyed.entry((&child.key, child.ty)).or_default().push_back(i);
    }
    let mut matched: Vec<_> = new.iter().map(|c| keyed.get_mut(&(&c.key, c.ty)).and_then(VecDeque::pop_front)).collect();
    let mut used = vec![false; old.len()];
    for &i in matched.iter().flatten() { used[i] = true; }
    let mut bodies: HashMap<_, VecDeque<_>> = HashMap::new();
    for (i, child) in old.iter().enumerate().filter(|(i, _)| !used[*i]) {
        bodies.entry((child.ty, body_tokens(child, before))).or_default().push_back(i);
    }
    for (i, c) in new.iter().enumerate() {
        if matched[i].is_none() && c.field.name.is_some() {
            matched[i] = bodies.get_mut(&(c.ty, body_tokens(c, after))).and_then(VecDeque::pop_front);
            if let Some(j) = matched[i] { used[j] = true; }
        }
    }
    let mut left: HashMap<_, Vec<_>> = HashMap::new();
    let mut right: HashMap<_, Vec<_>> = HashMap::new();
    for (i, c) in old.iter().enumerate().filter(|(i, _)| !used[*i]) { left.entry(c.ty).or_default().push(i); }
    for (i, c) in new.iter().enumerate().filter(|(i, _)| matched[*i].is_none()) { right.entry(c.ty).or_default().push(i); }
    for (ty, candidates) in right {
        if let ([i], Some([j])) = (candidates.as_slice(), left.get(ty).map(Vec::as_slice)) {
            if new[*i].field.name.is_some() { matched[*i] = Some(*j); used[*j] = true; }
        }
    }
    (matched, used)
}

// Unique matched indices turn the sibling LCS into a longest increasing subsequence.
fn stationary(matched: &[Option<usize>]) -> Vec<bool> {
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![None; matched.len()];
    for (i, index) in matched.iter().enumerate() {
        let Some(index) = *index else { continue };
        let k = tails.partition_point(|&j| matched[j].unwrap() < index);
        if k > 0 { previous[i] = Some(tails[k - 1]); }
        if k == tails.len() { tails.push(i); } else { tails[k] = i; }
    }
    let mut keep = vec![false; matched.len()];
    let mut link = tails.last().copied();
    while let Some(i) = link { keep[i] = true; link = previous[i]; }
    keep
}

// Prefix counts of siblings still in each original slot, including children
// already moved just before that slot. This keeps live target indices O(log N).
struct Ranks(Vec<usize>);
impl Ranks {
    fn adjust(&mut self, slot: usize, insert: bool) {
        let mut i = slot + 1;
        while i < self.0.len() {
            if insert { self.0[i] += 1; } else { self.0[i] -= 1; }
            i += i & i.wrapping_neg();
        }
    }
    fn before(&self, mut slot: usize) -> usize {
        let mut count = 0;
        while slot > 0 { count += self.0[slot]; slot &= slot - 1; }
        count
    }
}

// For one constructor type, each surviving bare child has a slot and a gap
// before it. An anchor of another type falls into the corresponding gap.
// Together these trees have O(N) slots regardless of the number of types.
struct BareRanks {
    original: Vec<usize>,
    counts: Ranks,
}

impl BareRanks {
    fn new(original: Vec<usize>) -> Self {
        let mut counts = Ranks(vec![0; 2 * original.len() + 2]);
        for i in 0..original.len() { counts.adjust(2 * i + 1, true); }
        Self { original, counts }
    }

    fn gap_before(&self, slot: usize) -> usize {
        2 * self.original.partition_point(|&i| i < slot)
    }

    // Renames and moves both consume the child in its original slot.
    fn take(&mut self, slot: usize) -> usize {
        let index = self.gap_before(slot) + 1;
        let ordinal = self.counts.before(index);
        self.counts.adjust(index, false);
        ordinal
    }

    fn insert_before(&mut self, anchor: usize) {
        self.counts.adjust(self.gap_before(anchor), true);
    }
}

fn diff_pair(base: &Node, edited: &Node, before: &str, after: &str, path: &mut Vec<NodeKey>, ops: &mut Vec<AstPatchOp>) {
    let (Some((_, a)), Some((_, b))) = (base.object(), edited.object()) else { return };
    property_diff(a, b, before, after, path, "", ops);
    let old = children(a);
    let new = children(b);
    let (matched, used) = match_children(&old, &new, before, after);
    let mut bare_slots: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, c) in old.iter().enumerate() {
        if used[i] && matches!(c.key, NodeKey::Bare { .. }) {
            bare_slots.entry(c.ty).or_default().push(i);
        }
    }
    let mut bare: HashMap<_, _> = bare_slots.into_iter().map(|(ty, slots)| (ty, BareRanks::new(slots))).collect();
    for (i, c) in old.iter().enumerate().rev() {
        if !used[i] { ops.push(AstPatchOp::RemoveChild { parent: path.clone(), key: c.key.clone() }); }
    }
    // Rename bare keys in descending original order so their ordinals stay valid.
    let mut rename: Vec<_> = matched.iter().enumerate().filter_map(|(i, m)| {
        let j = (*m)?;
        (old[j].key != new[i].key).then_some((j, i))
    }).collect();
    rename.sort_unstable_by(|a, b| b.cmp(a));
    for (j, i) in rename {
        let mut old_key = old[j].key.clone();
        if let NodeKey::Bare { ty, ordinal } = &mut old_key {
            *ordinal = bare.get_mut(ty.as_str()).unwrap().take(j);
        }
        ops.push(AstPatchOp::RenameChild { parent: path.clone(), old_key, new_name: new[i].field.name.clone().unwrap() });
    }
    let keep = stationary(&matched);
    let mut ranks = Ranks(vec![0; old.len() + 2]);
    for &j in matched.iter().flatten() { ranks.adjust(j, true); }
    let mut anchor = old.len();
    for (i, m) in matched.iter().enumerate().rev() {
        let Some(j) = *m else { continue };
        if keep[i] {
            anchor = j;
        } else {
            let mut key = new[i].key.clone();
            if let NodeKey::Bare { ty, ordinal } = &mut key {
                let siblings = bare.get_mut(ty.as_str()).unwrap();
                *ordinal = siblings.take(j);
                siblings.insert_before(anchor);
            }
            ranks.adjust(j, false);
            ops.push(AstPatchOp::ReorderChild { parent: path.clone(), key, to_index: ranks.before(anchor) });
            ranks.adjust(anchor, true);
        }
    }
    for (i, c) in new.iter().enumerate() {
        if matched[i].is_none() {
            ops.push(AstPatchOp::InsertChild {
                parent: path.clone(), at_index: i, name: c.field.name.clone(), splash: c.field.value.text(after).into(),
                literal_class: check_literal_class(&c.field.value, after).is_ok(),
            });
        }
    }
    for (i, m) in matched.iter().enumerate() {
        if let Some(j) = *m {
            path.push(new[i].key.clone());
            diff_pair(&old[j].field.value, &new[i].field.value, before, after, path, ops);
            path.pop();
        }
    }
}

pub fn diff_objects(base: &Syntax, base_loc: &Loc, edited: &Syntax, edited_loc: &Loc) -> Vec<AstPatchOp> {
    let mut ops = Vec::new();
    if let (Some(a), Some(b)) = (base.node(base_loc), edited.node(edited_loc)) {
        diff_pair(a, b, base.source(), edited.source(), &mut Vec::new(), &mut ops);
    }
    ops
}

fn roots(tree: &Syntax) -> Vec<(NodeKey, &Node)> {
    let mut counts = HashMap::new();
    tree.statements().iter().filter_map(|stmt| {
        let (name, value) = match &stmt.kind {
            Kind::Assign { target, value, .. } => (Some(target.ident()?), value.as_ref()),
            Kind::Entry { name, value, .. } => (Some(name.as_str()), value.as_ref()),
            Kind::Object { .. } => (None, stmt),
            _ => return None,
        };
        let (ty, _) = value.object()?;
        Some((key_for(name, ty.unwrap_or(""), &mut counts), value))
    }).collect()
}

/// File-relative paths start with the assignment/entry name, or bare root's
/// constructor and occurrence. Each matched root then uses object-relative keys.
pub fn diff_syntax(base: &Syntax, edited: &Syntax) -> Vec<AstPatchOp> {
    let mut old: HashMap<_, VecDeque<_>> = HashMap::new();
    for (key, node) in roots(base) { old.entry(key).or_default().push_back(node); }
    let mut ops = Vec::new();
    for (key, node) in roots(edited) {
        if let Some(previous) = old.get_mut(&key).and_then(VecDeque::pop_front) {
            diff_pair(previous, node, base.source(), edited.source(), &mut vec![key], &mut ops);
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;
    use crate::syntax::widget::{delete_widget, move_widget, rename_widget, widget_children, Place};

    #[test]
    fn syntax_diff_replays_bare_ordinals_after_removal_and_reorder() {
        let root = Loc(vec![0]);
        for (before, after) in [
            ("View{Label{text:\"a\"} Label{text:\"b\"}}", "View{title := Label{text:\"b\"}}"),
            ("View{A{} B{} C{} Label{text:\"a\"} Label{text:\"b\"}}", "View{Label{text:\"a\"} Label{text:\"b\"} A{} B{} C{}}"),
        ] {
            let mut tree = parse(before).unwrap();
            let edited = parse(after).unwrap();
            let ops = diff_objects(&tree, &root, &edited, &root);
            assert_eq!(ops.len(), 2, "replay requires two structural changes for {after}");
            for op in ops {
                let key = match &op {
                    AstPatchOp::RemoveChild { key, .. } | AstPatchOp::ReorderChild { key, .. } => key,
                    AstPatchOp::RenameChild { old_key, .. } => old_key,
                    other => panic!("expected a change to an ordered child, got {other:?}"),
                };
                let NodeKey::Bare { ty, ordinal } = key else { panic!("expected a bare source for {op:?}") };
                let children = widget_children(&tree, &root);
                let (from, child) = children.iter().enumerate().filter(|(_, c)| c.name.is_none() && &c.ty == ty)
                    .nth(*ordinal).unwrap_or_else(|| panic!("{key:?} must resolve in {}", tree.source()));
                let source = &child.value_loc;
                tree = match op {
                    AstPatchOp::RemoveChild { .. } => delete_widget(&tree, source),
                    AstPatchOp::RenameChild { new_name, .. } => rename_widget(&tree, source, &new_name),
                    AstPatchOp::ReorderChild { to_index, .. } => {
                        let place = if from < to_index { Place::After } else { Place::Before };
                        move_widget(&tree, source, &children[to_index].value_loc, place)
                    }
                    _ => unreachable!(),
                }.expect("each patch must apply to the tree left by the previous patch").syntax;
            }
            assert_eq!(tree.print(), after, "replay must preserve the identity and order of both labels");
        }
    }

    #[test]
    fn syntax_diff_detects_prop_insert_remove_reorder_rename_and_tags_literal_class() {
        let base = parse("View{width: 2 height: 3 draw_bg +: {color: #abc} font_family: FontFamily{latin := FontMember{res: crate_resource(\"x\")}} a := A{} b := B{} title := Label{} gone := Box{}}").unwrap();
        let font = "FontFamily{latin := FontMember{res: crate_resource(\"y\")}}";
        let edited = parse(&format!("View{{width: 4 draw_bg +: {{color: #abc pixel: fn() {{ 1 }} }} font_family: {font} b := B{{}} a := A{{}} Label2{{}} caption := Label{{}}}}")).unwrap();
        let root = Loc(vec![0]);
        let key = |s: &str| NodeKey::Named(s.into());
        let ops = diff_objects(&base, &root, &edited, &root);
        assert_eq!(ops, vec![
            AstPatchOp::RemoveProp {path: vec![], prop: "height".into()},
            AstPatchOp::SetProp {path: vec![], prop: "width".into(), value_splash: "4".into(), literal_class: true},
            AstPatchOp::SetProp {path: vec![], prop: "draw_bg.pixel".into(), value_splash: "fn() { 1 }".into(), literal_class: false},
            AstPatchOp::SetProp {path: vec![], prop: "font_family".into(), value_splash: font.into(), literal_class: false},
            AstPatchOp::RemoveChild {parent: vec![], key: key("gone")},
            AstPatchOp::RenameChild {parent: vec![], old_key: key("title"), new_name: "caption".into()},
            AstPatchOp::ReorderChild {parent: vec![], key: key("b"), to_index: 0},
            AstPatchOp::InsertChild {parent: vec![], at_index: 2, name: None, splash: "Label2{}".into(), literal_class: true},
        ], "patches describe the edited order, retain renamed identity, and tag shader and font payloads");
        let a = parse("mod.widgets.A = View{w: 1}\nmod.widgets.B = View{w: 1}").unwrap();
        let b = parse("mod.widgets.A = View{w: 1}\nmod.widgets.B = View{w: 2}").unwrap();
        assert_eq!(diff_syntax(&a, &b), vec![AstPatchOp::SetProp {path: vec![key("mod.widgets.B")], prop: "w".into(), value_splash: "2".into(), literal_class: true}], "file-relative patch identifies the changed root");
    }
}
