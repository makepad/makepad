//! `design_typed path= prop= value=`: a property edit that runs nothing in the app. (Not the
//! tweaker's `design_set`, which writes the edit into the app's source.)
//!
//! The value is read as data (`syntax::parse_value`) and built against the property's declared
//! type (`ScriptVm::typed_edit_text`): scalars, float vector literals, `theme.*` values, variants
//! of the declared enum and fields of the declared struct, never a call (a vector literal is
//! read, not called), an expression or a name from a scope. It is then applied the way
//! `design_apply` applies an evaluated chunk, and answered the same way.
//! This is the op for edits that come from outside the app, a collaborator's among them;
//! `design_apply` evaluates its text and stays the app owner's own tool.

use super::props::{drawn_values, reflect_flat, use_drawn_values};
use crate::makepad_script::{syntax::parse_value, trap::NoTrap};
use crate::{makepad_draw::*, widget::*};

fn identifier(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) fn remote(cx: &mut Cx, widget: &WidgetRef, args: &super::Args) -> Result<String, String> {
    let answer = typed_edit(cx, widget, args);
    if let Err(why) = &answer {
        log!("design_typed refused {}: {why}", super::arg(args, "prop").unwrap_or("?"));
    }
    answer
}

fn typed_edit(cx: &mut Cx, widget: &WidgetRef, args: &super::Args) -> Result<String, String> {
    super::apply::require_target(widget)?;
    let prop = super::need(args, "prop")?;
    let text = super::need(args, "value")?;
    let path: Vec<LiveId> = prop
        .split('.')
        .map(|segment| identifier(segment).then(|| LiveId::from_str(segment)))
        .collect::<Option<_>>()
        .ok_or_else(|| format!("`{prop}` is not a property name"))?;
    // A draw layer's Rust plumbing (`draw_bg.rect_pos`, `draw_clip`, ...) is the renderer's, not a
    // property: never listed, never set.
    if prop.split('.').skip(1).any(super::props::is_shader_plumbing) {
        return Err(format!("`{prop}` is the renderer's, not a property"));
    }
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return Err("the widget has no script source".into());
    }
    let mut before = reflect_flat(cx, widget);
    use_drawn_values(&mut before, &drawn_values(cx, widget));
    let mut target = widget.clone();
    // One `with_vm`: no eval runs between building the values and applying them.
    cx.with_vm(|vm| {
        // A field of a struct the source holds as a shorthand (`padding: 0`) has no object to
        // edit into: the struct is set whole, from its reflected field rows with this field's text.
        let whole = shorthand_parent(vm, source, &path).then(|| whole_struct(&before, prop, text)).flatten();
        let (path, text) = match &whole {
            Some(body) => (&path[..path.len() - 1], body.as_str()),
            None => (&path[..], text),
        };
        let edit = vm.typed_edit_text(source, path, text)?;
        // The property's value now, to put back if the edit fails. Serializing fn-ref fields
        // reports harmless type-check errors (see `reflect_flat`): dropped.
        vm.capture_errors();
        let current = widget.current_to_value(vm);
        let _ = vm.take_errors();
        let revert = captured(vm, current, &path).and_then(|value| vm.typed_edit_value(source, &path, value).ok());
        let _ = makepad_platform::shader_error::take();
        vm.capture_errors();
        target.script_apply(vm, &Apply::Eval, &mut Scope::default(), edit.into());
        let errors = vm.take_errors();
        // The draw shader compiles inside the apply: a widget that stopped drawing is put back.
        let shader = makepad_platform::shader_error::take();
        if errors.is_empty() && shader.is_none() {
            return Ok(());
        }
        if let Some(revert) = revert {
            vm.capture_errors();
            target.script_apply(vm, &Apply::Eval, &mut Scope::default(), revert.into());
            let _ = vm.take_errors();
            let _ = makepad_platform::shader_error::take();
        }
        Err(match shader {
            Some(error) => format!("shader compile error: {}", super::apply::terse_shader_error(&error)),
            None => format!("apply error: {}", errors.join("; ")),
        })
    })?;
    let mut after = reflect_flat(cx, widget);
    use_drawn_values(&mut after, &drawn_values(cx, widget));
    Ok(super::apply::answer(cx, widget, &before, &after, Some(prop)))
}

/// Whether a parent segment of `path` holds a plain value in the source (`padding: 0`), not fields.
fn shorthand_parent(vm: &ScriptVm, source: ScriptObject, path: &[LiveId]) -> bool {
    let mut at = source;
    for segment in &path[..path.len().saturating_sub(1)] {
        let value = vm.heap().value(at, (*segment).into(), NoTrap);
        match value.as_object() {
            Some(object) => at = object,
            None => return !value.is_nil() && !value.is_err(),
        }
    }
    false
}

/// `{a: .. b: ..}`: the struct at `prop`'s parent from its reflected field rows (the values the
/// app runs with), `prop`'s own field taking `text`. None when that field is not a row or `text`
/// is not one value on its own.
fn whole_struct(rows: &[(String, String, bool)], prop: &str, text: &str) -> Option<String> {
    let (parent, leaf) = prop.rsplit_once('.')?;
    parse_value(text).ok()?;
    let fields: Vec<(&str, &str)> = rows
        .iter()
        .filter_map(|(name, value, _)| {
            let field = name.strip_prefix(parent)?.strip_prefix('.')?;
            (!field.contains('.')).then_some((field, value.as_str()))
        })
        .collect();
    fields.iter().any(|(field, _)| *field == leaf).then(|| {
        let body: Vec<String> = fields.iter().map(|(field, value)| format!("{field}: {}", if *field == leaf { text } else { value })).collect();
        format!("{{{}}}", body.join(" "))
    })
}

/// The value at `path` in a widget's `current_to_value`.
fn captured(vm: &ScriptVm, current: ScriptValue, path: &[LiveId]) -> Option<ScriptValue> {
    let mut value = current;
    for segment in path {
        value = vm.heap().value(value.as_object()?, (*segment).into(), NoTrap);
    }
    (!value.is_err()).then_some(value)
}
