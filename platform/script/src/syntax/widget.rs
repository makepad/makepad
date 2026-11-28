//! Structural operations on the ordered entries of a Splash object.
use super::edit::{self, Replace, Shape};
use super::write::{self, ColorStyle, Value};
use super::{parse_value_with, At, EditError, Field, Kind, Loc, Syntax};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place { Before, Inside, After }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetChild {
    pub field_index: usize,
    pub name: Option<String>,
    pub ty: String,
    pub value_loc: Loc,
    pub span: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct WidgetEdit {
    pub syntax: Syntax,
    pub edits: Vec<Replace>,
    pub focus_open_brace: Option<usize>,
    pub focus_name: Option<String>,
}

fn error(message: impl Into<String>) -> EditError { EditError { message: message.into() } }

pub fn find_object_by_open_brace(syntax: &Syntax, open_byte: usize) -> Option<Loc> {
    let mut result = None;
    syntax.walk(&mut |loc, node| {
        if matches!(node.kind, Kind::Object { open, .. } if open == open_byte) {
            result = Some(loc.clone());
        }
    });
    result
}

pub fn parent_field_of(syntax: &Syntax, widget_loc: &Loc) -> Option<(Loc, usize)> {
    let parent = widget_loc.parent()?;
    let index = *widget_loc.0.last()?;
    syntax.node(&parent)?.object()?.1.get(index)?;
    Some((parent, index))
}

fn fields<'a>(syntax: &'a Syntax, loc: &Loc) -> Result<&'a [Field], EditError> {
    syntax.require(loc)?.object().map(|(_, fields)| fields).ok_or_else(|| error("expected an object"))
}

pub(super) fn child_type(field: &Field) -> Option<&str> {
    if field.op == ":=" || (field.name.is_none() && field.op.is_empty()) {
        field.value.object()?.0
    } else {
        None
    }
}

/// Syntactic ordered entries (named `:=` and bare constructors), excluding map
/// fields. These paths can be local aliases, so no runtime registry is needed.
pub fn widget_children(syntax: &Syntax, object_loc: &Loc) -> Vec<WidgetChild> {
    let Ok(entries) = fields(syntax, object_loc) else { return Vec::new() };
    entries.iter().enumerate().filter_map(|(i, f)| Some(WidgetChild {
        field_index: i, ty: child_type(f)?.to_owned(), name: f.name.clone(),
        value_loc: object_loc.child(i), span: f.span.clone(),
    })).collect()
}

/// Whether this value occupies a named map slot in an enclosing object.
pub fn is_fixed_slot(syntax: &Syntax, widget_loc: &Loc) -> bool {
    parent_field_of(syntax, widget_loc).is_some_and(|(p, i)| {
        let f = &fields(syntax, &p).unwrap()[i];
        f.name.is_some() && matches!(f.op.as_str(), ":" | "+:")
    })
}

fn require_list_child<'a>(syntax: &'a Syntax, loc: &Loc) -> Result<(Loc, usize, &'a Field), EditError> {
    let (parent, index) = parent_field_of(syntax, loc).ok_or_else(|| error("expected an ordered child of an object"))?;
    let f = &fields(syntax, &parent)?[index];
    if child_type(f).is_none() {
        if let Some(name) = f.name.as_ref().filter(|_| matches!(f.op.as_str(), ":" | "+:")) {
            return Err(error(format!("cannot change the position or name of fixed slot `{name}`")));
        }
        return Err(error("expected a typed ordered child"));
    }
    Ok((parent, index, f))
}

pub fn fresh_child_name(syntax: &Syntax, container_loc: &Loc, stem: &str) -> String {
    let names: std::collections::HashSet<_> = fields(syntax, container_loc).unwrap_or(&[]).iter()
        .filter_map(|f| f.name.as_deref()).collect();
    if !names.contains(stem) { return stem.to_owned(); }
    (2..).map(|n| format!("{stem}_{n}")).find(|name| !names.contains(name.as_str())).unwrap()
}

fn resolved_name(syntax: &Syntax, container: &Loc, name: Option<&str>) -> Result<Option<String>, EditError> {
    name.map(|s| {
        if !write::is_ident(s) || write::is_reserved(s) { return Err(error(format!("invalid widget name `{s}`"))); }
        Ok(fresh_child_name(syntax, container, s))
    }).transpose()
}

fn destination(syntax: &Syntax, target: &Loc, place: Place) -> Result<(Loc, usize), EditError> {
    match place {
        Place::Inside => Ok((target.clone(), fields(syntax, target)?.len())),
        Place::Before | Place::After => {
            let (parent, i, _) = require_list_child(syntax, target)?;
            Ok((parent, i + usize::from(place == Place::After)))
        }
    }
}

fn finish(syntax: &Syntax, edits: Vec<Replace>, expected: Shape, focus: &Loc, name: Option<String>) -> Result<WidgetEdit, EditError> {
    let syntax = syntax.apply_checked(&edits, expected)?;
    let focus_open_brace = match syntax.require(focus)?.kind {
        Kind::Object { open, .. } => Some(open),
        _ => None,
    };
    Ok(WidgetEdit { syntax, edits, focus_open_brace, focus_name: name })
}

fn prefix(name: Option<&str>) -> String {
    name.map_or_else(String::new, |s| format!("{s} := "))
}

fn entry_shape(syntax: &Syntax, text: &str) -> Result<(String, Shape), EditError> {
    let node = parse_value_with(text, &syntax.units).map_err(|e| error(e.to_string()))?;
    match &node.kind {
        Kind::Entry { name, op, value, .. } => Ok((format!("{name}{op}"), edit::shape(value, text))),
        _ => Ok((String::new(), edit::shape(&node, text))),
    }
}

fn insert_shape(container: &mut Shape, index: usize, entry: (String, Shape)) {
    container.names.insert(index, entry.0);
    container.kids.insert(index, entry.1);
}

fn remove_shape(container: &mut Shape, index: usize) -> (String, Shape) {
    (container.names.remove(index), container.kids.remove(index))
}

pub fn insert_widget(syntax: &Syntax, target_loc: &Loc, place: Place, ty: &str, name: Option<&str>, props: &[(String, Value)], colors: ColorStyle) -> Result<WidgetEdit, EditError> {
    let (parent, i) = destination(syntax, target_loc, place)?;
    let name = resolved_name(syntax, &parent, name)?;
    let value = Value::Object(Some(ty.into()), props.to_vec());
    let target = syntax.require(target_loc)?;
    let indent = if place == Place::Inside {
        let neighbour = fields(syntax, target_loc)?.last().map(|f| f.span.start);
        edit::child_indent(syntax.source(), target, neighbour)
    } else {
        edit::line_indent(syntax.source(), target.span.start).to_owned()
    };
    let text = format!("{}{}", prefix(name.as_deref()), write::write_indented_with(&value, &indent, colors).map_err(error)?);
    let entry = entry_shape(syntax, &text)?;
    if entry.1.label != format!("object:{ty}") { return Err(error("expected widget constructor")); }
    let edits = edit::insert_child_edits(syntax.source(), syntax.require(&parent)?, i, &text)?;
    let mut expected = syntax.old_shape();
    insert_shape(expected.at_mut(&parent.0), i, entry);
    finish(syntax, edits, expected, &parent.child(i), name)
}

pub fn delete_widget(syntax: &Syntax, target_loc: &Loc) -> Result<WidgetEdit, EditError> {
    let (parent, i, _) = require_list_child(syntax, target_loc)?;
    let edits = edit::remove_element_edits(syntax.source(), syntax.require(&parent)?, i)?;
    let mut expected = syntax.old_shape();
    remove_shape(expected.at_mut(&parent.0), i);
    finish(syntax, edits, expected, &parent, None)
}

pub fn duplicate_widget(syntax: &Syntax, target_loc: &Loc, name: Option<&str>) -> Result<WidgetEdit, EditError> {
    let (parent, i, f) = require_list_child(syntax, target_loc)?;
    let original = f.name.as_deref();
    let stem = original.map(|s| s.rsplit_once('_')
        .filter(|(stem, n)| !stem.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .map_or(s, |(stem, _)| stem));
    let name = resolved_name(syntax, &parent, name.or(stem))?;
    let edits = edit::duplicate_element_edits(syntax.source(), syntax.require(&parent)?, i, &|text| {
        let mut copy = text.to_owned();
        if let Some(name) = &name {
            if original.is_some() { copy.replace_range(..f.name_span.len(), name); }
            else { copy.insert_str(0, &prefix(Some(name))); }
        }
        Ok(copy)
    })?;
    let mut expected = syntax.old_shape();
    let container = expected.at_mut(&parent.0);
    let entry = (name.as_ref().map_or_else(String::new, |s| format!("{s}:=")), container.kids[i].clone());
    insert_shape(container, i + 1, entry);
    finish(syntax, edits, expected, &parent.child(i + 1), name)
}

pub fn wrap_widget(syntax: &Syntax, target_loc: &Loc, wrapper_ty: &str, wrapper_name: Option<&str>) -> Result<WidgetEdit, EditError> {
    let (parent, i, _) = require_list_child(syntax, target_loc)?;
    if !write::is_path(wrapper_ty) { return Err(error("invalid wrapper type")); }
    let name = resolved_name(syntax, &parent, wrapper_name)?;
    let edits = edit::wrap_element_edits(syntax.source(), syntax.require(&parent)?, i, &prefix(name.as_deref()), wrapper_ty)?;
    let mut expected = syntax.old_shape();
    let container = expected.at_mut(&parent.0);
    let (child_name, child) = remove_shape(container, i);
    let wrapper = Shape { label: format!("object:{wrapper_ty}"), names: vec![child_name], kids: vec![child] };
    insert_shape(container, i, (name.as_ref().map_or_else(String::new, |s| format!("{s}:=")), wrapper));
    finish(syntax, edits, expected, target_loc, name)
}

pub fn move_widget(syntax: &Syntax, source_loc: &Loc, target_loc: &Loc, place: Place) -> Result<WidgetEdit, EditError> {
    let (src, from, field) = require_list_child(syntax, source_loc)?;
    if target_loc.0.starts_with(&source_loc.0) { return Err(error("cannot move a widget into itself")); }
    let (dst, at) = destination(syntax, target_loc, place)?;
    let name = field.name.clone();
    let mut expected = syntax.old_shape();
    let source = syntax.require(&src)?;
    if src == dst {
        let to = at - usize::from(from < at);
        let edits = edit::move_element_edits(syntax.source(), source, from, to)?;
        let container = expected.at_mut(&src.0);
        let entry = remove_shape(container, from);
        insert_shape(container, to, entry);
        return finish(syntax, edits, expected, &dst.child(to), name);
    }
    if name.is_some() && fields(syntax, &dst)?.iter().any(|f| f.name == name) {
        return Err(error("an entry with that name already exists at the destination"));
    }
    let edits = edit::reparent_element_edits(syntax.source(), source, from, syntax.require(&dst)?, At::Before(at))?;
    let mut focus = dst.child(at);
    if dst.0.starts_with(&src.0) && dst.0[src.0.len()] > from { focus.0[src.0.len()] -= 1; }
    // Inserting below the source parent can shift its address when the source is removed.
    if dst.0.len() > src.0.len() {
        let container = expected.at_mut(&src.0);
        let entry = (container.names[from].clone(), container.kids[from].clone());
        insert_shape(expected.at_mut(&dst.0), at, entry);
        remove_shape(expected.at_mut(&src.0), from);
    } else {
        let entry = remove_shape(expected.at_mut(&src.0), from);
        insert_shape(expected.at_mut(&dst.0), at, entry);
    }
    finish(syntax, edits, expected, &focus, name)
}

fn step(syntax: &Syntax, target: &Loc, up: bool) -> Result<WidgetEdit, EditError> {
    let (parent, index, _) = require_list_child(syntax, target)?;
    let children = widget_children(syntax, &parent);
    let neighbour = if up { children.iter().rfind(|c| c.field_index < index) }
        else { children.iter().find(|c| c.field_index > index) }
        .ok_or_else(|| error(if up { "already first child" } else { "already last child" }))?;
    move_widget(syntax, target, &neighbour.value_loc, if up { Place::Before } else { Place::After })
}

pub fn step_widget_up(syntax: &Syntax, target_loc: &Loc) -> Result<WidgetEdit, EditError> { step(syntax, target_loc, true) }

pub fn step_widget_down(syntax: &Syntax, target_loc: &Loc) -> Result<WidgetEdit, EditError> { step(syntax, target_loc, false) }

pub fn hoist_widget_out(syntax: &Syntax, target_loc: &Loc) -> Result<WidgetEdit, EditError> {
    let (parent, _, _) = require_list_child(syntax, target_loc)?;
    move_widget(syntax, target_loc, &parent, Place::After)
}

pub fn rename_widget(syntax: &Syntax, target_loc: &Loc, new_name: &str) -> Result<WidgetEdit, EditError> {
    let (parent, index, _) = require_list_child(syntax, target_loc)?;
    let edits = edit::rename_field_edits(syntax.source(), syntax.require(&parent)?, index, new_name)?;
    let mut expected = syntax.old_shape();
    expected.at_mut(&parent.0).names[index] = format!("{new_name}:=");
    finish(syntax, edits, expected, target_loc, Some(new_name.into()))
}

pub fn set_widget_prop(syntax: &Syntax, widget_loc: &Loc, prop_path: &str, value_splash: &str, colors: ColorStyle) -> Result<WidgetEdit, EditError> {
    if !write::is_path(prop_path) { return Err(error("invalid property path")); }
    let parse = |text: &str| parse_value_with(text, &syntax.units).map_err(|e| error(e.to_string()));
    let value = colors.rewrite(value_splash, &parse(value_splash)?);
    let mut loc = widget_loc.clone();
    let mut path = prop_path;
    let mut expected = syntax.old_shape();
    let edits = loop {
        let object = syntax.require(&loc)?;
        let fields = fields(syntax, &loc)?;
        if let Some((i, f)) = fields.iter().enumerate().find(|(_, f)| f.name.as_deref() == Some(path)) {
            if child_type(f).is_some() { return Err(error("property path selects an ordered child")); }
            *expected.at_mut(&loc.child(i).0) = edit::shape(&parse(&value)?, &value);
            break edit::replace_field_op_and_value_edits(syntax.source(), object, i, &f.op, &value)?;
        }
        let mut entry = value.clone();
        let parts = path.split('.');
        let head = parts.into_iter().next().unwrap();
        if let Some((_, rest)) = path.split_once('.') {
            if let Some((i, f)) = fields.iter().enumerate().find(|(_, f)| f.name.as_deref() == Some(head)) {
                if child_type(f).is_some() { return Err(error("property path crosses an ordered child")); }
                loc = loc.child(i);
                path = rest;
                continue;
            }
        }
        for (i, segment) in path.rsplit('.').enumerate() {
            let key = write::key(segment).map_err(error)?;
            entry = if i == 0 { format!("{key}: {entry}") } else { format!("{key} +: {{ {entry} }}") };
        }
        let index = fields.iter().position(|f| child_type(f).is_some()).unwrap_or(fields.len());
        insert_shape(expected.at_mut(&loc.0), index, entry_shape(syntax, &entry)?);
        break edit::insert_element_edits(syntax.source(), object, At::Before(index), &entry)?;
    };
    finish(syntax, edits, expected, widget_loc, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn object(syntax: &Syntax, marker: &str) -> Loc {
        let end = syntax.source().find(marker).expect("fixture marker exists") + marker.len() - 1;
        find_object_by_open_brace(syntax, end).expect("fixture marker ends at an object brace")
    }

    #[test]
    fn syntax_widget_insert_before_inside_after_preserves_comments() {
        let src = "View {\n    // save action\n    save := Button{} // shortcut\n}\n";
        let tree = parse(src).unwrap();
        let save = object(&tree, "Button{");
        for (place, expected) in [
            (Place::Before, "View {\n    Button{}\n    // save action\n    save := Button{} // shortcut\n}\n"),
            (Place::After, "View {\n    // save action\n    save := Button{} // shortcut\n    Button{}\n}\n"),
            (Place::Inside, "View {\n    // save action\n    save := Button{\n        Button{}\n    } // shortcut\n}\n"),
        ] {
            let out = insert_widget(&tree, &save, place, "Button", None, &[], ColorStyle::Hash).unwrap();
            assert_eq!(out.syntax.print(), expected, "{place:?} preserves surrounding trivia and layout");
            assert_eq!(out.syntax.source().as_bytes()[out.focus_open_brace.unwrap()], b'{', "{place:?} focuses the inserted constructor");
        }
        let long = "x".repeat(write::WIDTH + 1);
        let props = [("text".into(), Value::str(&long))];
        for (src, indent) in [("  View{}", "      "), ("  View{\n    width: Fill\n  }", "    ")] {
            let tree = parse(src).unwrap();
            let out = insert_widget(&tree, &Loc(vec![0]), Place::Inside, "Label", None, &props, ColorStyle::Hash).unwrap();
            assert!(out.syntax.source().contains(&format!("\n{indent}Label{{\n{indent}    text: \"{long}\"\n{indent}}}")),
                "a multiline child in {src} aligns its closing brace with its constructor");
        }
    }

    #[test]
    fn syntax_widget_up_down_skips_property_fields() {
        let src = "FieldWell {\n    padding: 8\n    a := Label{}\n    // fixed input\n    input: View { width: Fill }\n    b := Button{}\n}";
        let tree = parse(src).unwrap();
        let parent = Loc(vec![0]);
        let slot = object(&tree, "input: View {");
        let a = object(&tree, "Label{");
        let b = object(&tree, "Button{");
        let names = |s: &Syntax| widget_children(s, &parent).into_iter().map(|c| c.name.unwrap()).collect::<Vec<_>>();
        assert_eq!(names(&tree), ["a", "b"], "map properties and slots stay out of the ordered list");
        assert!(is_fixed_slot(&tree, &slot), "named input is a fixed slot");
        assert_eq!(step_widget_up(&tree, &a).unwrap_err().message, "already first child", "padding does not count as a preceding child");
        for out in [step_widget_up(&tree, &b), step_widget_down(&tree, &a)] {
            let moved = out.unwrap().syntax;
            assert_eq!(names(&moved), ["b", "a"], "stepping passes the intervening property");
            assert!(moved.source().contains("// fixed input\n    input: View"), "property comment stays attached");
        }
        for (op, result) in [
            ("up", step_widget_up(&tree, &slot)), ("down", step_widget_down(&tree, &slot)),
            ("delete", delete_widget(&tree, &slot)), ("duplicate", duplicate_widget(&tree, &slot, None)),
            ("wrap", wrap_widget(&tree, &slot, "View", None)), ("rename", rename_widget(&tree, &slot, "other")),
            ("move source", move_widget(&tree, &slot, &a, Place::Inside)),
        ] {
            assert!(result.unwrap_err().message.contains("fixed slot"), "{op} protects the fixed slot");
        }
        for place in [Place::Before, Place::After] {
            assert!(insert_widget(&tree, &slot, place, "View", None, &[], ColorStyle::Hash).is_err(), "fixed slots cannot anchor {place:?} insertion");
            assert!(move_widget(&tree, &a, &slot, place).is_err(), "fixed slots cannot anchor {place:?} moves");
        }
        let out = set_widget_prop(&tree, &slot, "width", "Fit", ColorStyle::Hash).unwrap();
        assert!(out.syntax.source().contains("input: View { width: Fit }"), "slot property updates in place");
        let out = insert_widget(&tree, &slot, Place::Inside, "Button", None, &[], ColorStyle::Hash).unwrap();
        assert_eq!(widget_children(&out.syntax, &slot).len(), 1, "slot accepts an ordered child inside");
    }

    #[test]
    fn syntax_widget_reparent_and_hoist_out_reindents_and_rejects_cycle() {
        let tree = parse("View{\n    // title\n    a := Label{\n        text: \"A\"\n    }\n    panel := Panel{}\n    input: View{}\n}").unwrap();
        let a = object(&tree, "Label{");
        let panel = object(&tree, "Panel{");
        let slot = object(&tree, "input: View{");
        let out = move_widget(&tree, &a, &panel, Place::Inside).unwrap().syntax;
        assert!(out.source().contains("    panel := Panel{\n        // title\n        a := Label{\n            text: \"A\"\n        }\n    }"), "move preserves and reindents the subtree with its comment");
        let panel = object(&out, "Panel{");
        let nested = object(&out, "Label{");
        assert!(move_widget(&out, &panel, &nested, Place::Inside).unwrap_err().message.contains("cannot move a widget into itself"), "moving an ancestor under its descendant refuses the cycle");
        let hoisted = hoist_widget_out(&out, &nested).unwrap().syntax;
        assert!(hoisted.source().contains("    }\n    // title\n    a := Label{\n        text:"), "hoist inserts the subtree immediately after its former parent");
        assert!(hoist_widget_out(&tree, &Loc(vec![0])).is_err(), "cannot hoist the root");
        let out = move_widget(&tree, &a, &slot, Place::Inside).unwrap().syntax;
        let nested = object(&out, "Label{");
        assert!(is_fixed_slot(&out, &nested.parent().unwrap()), "move inside targets the fixed slot");
        assert!(hoist_widget_out(&out, &nested).unwrap_err().message.contains("fixed slot"), "hoist cannot cross a fixed slot");
    }

    #[test]
    fn syntax_widget_wrap_preserves_child_name_and_comments() {
        let tree = parse("View{\n    // Save action\n    save_btn := Button{text: \"Save\"}\n}").unwrap();
        let out = wrap_widget(&tree, &object(&tree, "Button{"), "RoundedView", Some("rounded_view")).unwrap();
        assert_eq!(out.syntax.print(), "View{\n    rounded_view := RoundedView {\n        // Save action\n        save_btn := Button{text: \"Save\"}\n    }\n}", "wrapper encloses the original named entry and its comment");
        assert_eq!(out.focus_name.as_deref(), Some("rounded_view"), "wrapper receives focus");
    }

    #[test]
    fn syntax_widget_duplicate_picks_fresh_sibling_name() {
        let tree = parse("View{card := Card{} card_2 := Card{}}").unwrap();
        let out = duplicate_widget(&tree, &Loc(vec![0, 0]), None).unwrap();
        assert_eq!(out.syntax.print(), "View{card := Card{} card_3 := Card{} card_2 := Card{}}", "duplicate uses the first free suffix directly after the source");
        let out = duplicate_widget(&tree, &Loc(vec![0, 1]), None).unwrap();
        assert_eq!(out.focus_name.as_deref(), Some("card_3"), "numbered names reuse their base stem");
    }

    #[test]
    fn syntax_widget_rename_bare_and_named_checks_collisions() {
        let tree = parse("View{Label{} other := Button{}}").unwrap();
        let label = object(&tree, "Label{");
        let named = rename_widget(&tree, &label, "title").unwrap().syntax;
        assert_eq!(named.print(), "View{title := Label{} other := Button{}}", "bare constructor gains a named entry");
        assert_eq!(rename_widget(&named, &label, "heading").unwrap().syntax.print(), "View{heading := Label{} other := Button{}}", "renaming an existing entry replaces only its name");
        for name in ["other", "fn", "let"] {
            assert!(rename_widget(&named, &label, name).is_err(), "reject collision or reserved name {name}");
        }
    }

    #[test]
    fn syntax_widget_set_prop_preserves_merge_op_and_macro_hex() {
        let root = Loc(vec![0]);
        for (src, expected) in [
            ("View{draw_bg +: {color: #fff pixel: fn() { 1 }}}", "View{draw_bg +: {color: #x1a2b3cff pixel: fn() { 1 }}}"),
            ("View{Button{}}", "View{draw_bg +: { color: #x1a2b3cff } Button{}}"),
            ("View{draw_bg.color: #fff}", "View{draw_bg.color: #x1a2b3cff}"),
        ] {
            let tree = parse(src).unwrap();
            let out = set_widget_prop(&tree, &root, "draw_bg.color", "#1a2b3cff", ColorStyle::MacroHashX).unwrap();
            assert_eq!(out.syntax.print(), expected, "macro color write preserves surrounding fields for {src}");
        }
        let value = Value::Array(vec![Value::str("x".repeat(write::WIDTH)), Value::Color("aBc".into())]);
        let pretty = write::write_indented_with(&value, "", ColorStyle::MacroHashX).unwrap();
        assert!(pretty.contains("\n    #xaBc,"), "multiline writer propagates macro color style and preserves case");
    }
}
