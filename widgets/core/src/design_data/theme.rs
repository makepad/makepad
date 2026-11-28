use crate::{makepad_draw::*, widget_tree::live_id_token};
use crate::desktop_style::{self, StyleSheet};
use crate::makepad_micro_serde::{DeJson, JsonValue, SerJson};
use crate::theme_tokens::{self, Scheme, TokenValue};
use super::props::fmt_f64;
use crate::widget_tree::CxWidgetExt;
use std::path::Path;

/// The on-disk theme definition a caller can edit, or the root widget's
/// source when the application has yet to add its own theme override.
pub fn theme_define_file(cx: &mut Cx) -> Option<String> {
    let library = Path::new(crate::widgets_dir());
    let library = library.canonicalize().unwrap_or_else(|_| library.to_path_buf());
    let editable = |file: &str| {
        let path = Path::new(file).canonicalize().ok()?;
        (path.is_file() && !path.starts_with(&library)).then(|| path.to_string_lossy().replace('\\', "/"))
    };
    let definition = cx.with_vm(|vm| {
        let theme = vm.module(id!(theme));
        vm.construction_chain(theme.into()).iter().find_map(|level| {
            let site = level.loc()?;
            let ip = level.made_at();
            if !ip.is_unknown() {
                let body = vm.bx.body_module_read(ip.body as usize);
                if let Some(module) = body.module() {
                    if let Some(file) = Cx::resolve_script_mod_path(module) {
                        return editable(&file);
                    }
                }
            }
            editable(site.file())
        })
    });
    definition.or_else(|| {
        let tree = cx.widget_tree();
        let root = tree.widget(tree.root_uid());
        editable(&super::locate::widget_source_file(cx, &root).ok()?)
    })
}

/// One global theme value: a colour (packed `0xrrggbbaa`) or a number.
/// Part of the public reflection API.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThemeVal {
    Color(u32),
    Num(f64),
}

impl ThemeVal {
    fn read(value: ScriptValue) -> Option<Self> {
        value.as_color().map(Self::Color).or_else(|| value.as_f64().map(Self::Num))
    }
}

impl From<ThemeVal> for TokenValue {
    fn from(value: ThemeVal) -> Self {
        match value {
            ThemeVal::Color(c) => Self::Color(c),
            ThemeVal::Num(n) => Self::Num(n),
        }
    }
}

/// The app's theme: every colour and number in `mod.theme`, with the
/// level (file:line) that defines it. Read from the script cascade. Part of
/// the public reflection API: a theme panel's rows and the probe behind
/// `/tweak/op?op=theme&name=`.
pub fn theme_values(cx: &mut Cx) -> Vec<(String, LiveId, ThemeVal, String)> {
    let mut out: Vec<(String, LiveId, ThemeVal, String)> = Vec::new();
    cx.with_vm(|vm| {
        let theme = vm.module(id!(theme));
        let chain = vm.construction_chain(theme.into());
        let mut keys: Vec<(LiveId, String)> = Vec::new();
        for lvl in &chain {
            let loc = lvl.loc()
                .map(|l| format!("{}:{}", l.file().rsplit('/').next().unwrap_or(l.file()), l.line()))
                .unwrap_or_default();
            for key in lvl.own_keys() {
                if !keys.iter().any(|(k, _)| k == key) {
                    keys.push((*key, loc.clone()));
                }
            }
        }
        for (key, loc) in keys {
            let name = live_id_token(key);
            if let Some(value) = ThemeVal::read(vm.value(theme, key)) {
                out.push((name, key, value, loc));
            }
        }
    });
    out
}

/// Overwrite one theme value in the script heap, wherever in the theme's
/// prototype chain it is defined (the module is immutable to scripts).
pub fn theme_heap_set(cx: &mut Cx, key: LiveId, value: ScriptValue) -> bool {
    let mut hit = false;
    cx.with_vm(|vm| {
        let theme = vm.module(id!(theme));
        let mut chain = Vec::new();
        let mut current = Some(theme);
        while let Some(obj) = current {
            chain.push(obj);
            current = vm.heap().proto(obj).as_object();
        }
        for obj in chain.into_iter().rev() {
            if vm.heap().map_entries(obj).iter().any(|e| e.key == key.into()) {
                // The own entry exists: update it in place, preserving its definition order.
                vm.heap_mut().poke_scope_value(obj, key, value);
                hit = true;
            }
        }
    });
    hit
}

/// One theme value as the text the ledger, the sidebar and the remote op
/// all show: `#rrggbbaa` for a colour, `fmt_f64` for a number.
pub fn theme_val_text(value: ThemeVal) -> String {
    match value {
        ThemeVal::Color(c) => hex_of(c),
        ThemeVal::Num(f) => fmt_f64(f),
    }
}

/// The text a theme edit applies. `theme.other` as a value means "what
/// `other` is right now", so a colour can be pointed at another token
/// without copying its hex; anything else is taken as written.
pub fn theme_edit_text(
    values: &[(String, LiveId, ThemeVal, String)],
    text: &str,
) -> Result<String, String> {
    match text.strip_prefix("theme.") {
        Some(other) => values
            .iter()
            .find(|(n, _, _, _)| n == other)
            .map(|(_, _, v, _)| theme_val_text(*v))
            .ok_or_else(|| format!("no theme value {other:?}")),
        None => Ok(text.to_string()),
    }
}

pub fn hex_of(c: u32) -> String {
    format!("#{c:08x}")
}

/// A validated change to one live theme token.
pub struct ThemeEdit {
    pub old: ThemeVal,
    pub new: ThemeVal,
    pub text: String,
    pub literal: String,
    pub site: String,
}

/// Resolve aliases, validate the value and update the defining heap entries.
/// The overlay supplies its existing override flag when restoring a colour.
pub fn theme_set_value(cx: &mut Cx, name: &str, text: &str, overridden: bool) -> Result<Option<ThemeEdit>, String> {
    let values = theme_values(cx);
    let (_, key, old, site) = values.iter().find(|(n, _, _, _)| n == name)
        .ok_or_else(|| format!("no theme value {name:?}"))?;
    let text = theme_edit_text(&values, text)?;
    let (new, literal, value) = match old {
        ThemeVal::Color(_) => {
            let digits = text.trim().trim_start_matches('#').as_bytes();
            let color = matches!(digits.len(), 3 | 6 | 8)
                .then(|| crate::makepad_script::colorhex::hex_bytes_to_u32(digits).ok())
                .flatten().ok_or_else(|| format!("{text:?} is not a colour"))?;
            (ThemeVal::Color(color), format!("#x{color:08X}"), ScriptValue::from_color(color))
        }
        ThemeVal::Num(_) => {
            let n: f64 = text.parse().map_err(|_| format!("{text:?} is not a number"))?;
            if !n.is_finite() {
                return Err(format!("{text:?} is not a finite number"));
            }
            (ThemeVal::Num(n), format!("{n:?}"), ScriptValue::from_f64(n))
        }
    };
    if new == *old && !overridden {
        return Ok(None);
    }
    theme_heap_set(cx, *key, value);
    Ok(Some(ThemeEdit {old: *old, new, literal, text: theme_val_text(new), site: site.clone()}))
}

/// A base and its scalar customizations for a data-only installation.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeInstallSpec {
    pub base: crate::BaseTheme,
    pub sheet: Option<String>,
    pub globals: Vec<(String, ThemeVal)>,
    pub overrides: Vec<(String, ThemeVal)>,
}

impl ThemeInstallSpec {
    fn validate(&self) -> Result<(), String> {
        if self.sheet.is_some() && !self.globals.is_empty() {
            return Err("a style sheet cannot combine with re-derived globals".into());
        }
        for (key, value) in &self.globals {
            if !theme_tokens::ALLOWED_INSTALL_GLOBALS.contains(&key.as_str()) {
                return Err(format!("unknown install global {key:?}"));
            }
            let color = matches!(key.as_str(), "color_tint" | "color_bg_app" | "color_fg_app");
            if color != matches!(value, ThemeVal::Color(_)) {
                return Err(format!("wrong value kind for install global {key:?}"));
            }
        }
        for (key, value) in self.globals.iter().chain(&self.overrides) {
            if !theme_tokens::is_token_key(key) {
                return Err(format!("invalid theme token key {key:?}"));
            }
            if matches!(value, ThemeVal::Num(n) if !n.is_finite()) {
                return Err(format!("{key:?} needs a finite number"));
            }
        }
        Ok(())
    }
}

fn install_entries(object: &JsonValue, field: &str) -> Result<Vec<(String, ThemeVal)>, String> {
    let Some(entries) = object.get(field) else { return Ok(Vec::new()) };
    let entries = entries.as_array().ok_or_else(|| format!("{field} must be an array"))?;
    let mut values = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some([key, value]) = entry.as_array().map(Vec::as_slice) else {
            return Err(format!("{field} needs [key, value] pairs"));
        };
        let key = key.as_str().ok_or_else(|| format!("{field} key must be a string"))?;
        let value = if let Some(n) = value.as_f64() {
            ThemeVal::Num(n)
        } else {
            let color = value.as_str().and_then(|s| s.strip_prefix('#'))
                .filter(|s| matches!(s.len(), 3 | 6 | 8) && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .and_then(|s| crate::makepad_script::colorhex::hex_bytes_to_u32(s.as_bytes()).ok())
                .ok_or_else(|| format!("{key:?} needs a hex colour or a finite number"))?;
            ThemeVal::Color(color)
        };
        values.push((key.to_owned(), value));
    }
    Ok(values)
}

pub fn parse_install_spec(json: &str) -> Result<ThemeInstallSpec, String> {
    let object = JsonValue::deserialize_json_strict(json).map_err(|err| format!("invalid theme spec: {err}"))?;
    let base = object.get("base").and_then(JsonValue::as_str)
        .and_then(crate::BaseTheme::parse).ok_or("expected dark, light or skeleton base")?;
    let sheet = match object.get("sheet") {
        None | Some(JsonValue::Null) => None,
        Some(JsonValue::String(id)) if desktop_style::find(id).is_some() => Some(id.clone()),
        Some(JsonValue::String(id)) => return Err(format!("unknown style sheet {id:?}")),
        Some(_) => return Err("sheet must be a style sheet id or null".into()),
    };
    let spec = ThemeInstallSpec {
        base, sheet, globals: install_entries(&object, "globals")?, overrides: install_entries(&object, "overrides")?,
    };
    spec.validate()?;
    Ok(spec)
}

pub fn apply_install_spec(cx: &mut Cx, spec: &ThemeInstallSpec) -> Result<(), String> {
    spec.validate()?;
    let sheet = spec.sheet.as_deref()
        .map(|id| StyleSheet::named(id).ok_or_else(|| format!("unknown style sheet {id:?}")))
        .transpose()?;
    crate::set_base_theme(cx, spec.base);
    if let Some(sheet) = sheet {
        cx.with_vm(|vm| desktop_style::install(vm, sheet));
        if !spec.overrides.is_empty() {
            let pins = spec.overrides.iter().map(|(k, v)| (k.clone(), TokenValue::from(*v).render())).collect();
            crate::set_theme_pins(cx, Some(pins));
        }
    } else {
        cx.with_vm(desktop_style::uninstall);
        if !spec.globals.is_empty() || !spec.overrides.is_empty() {
            let scheme = match spec.base {
                crate::BaseTheme::Dark => Scheme::Dark,
                crate::BaseTheme::Light => Scheme::Light,
                crate::BaseTheme::Skeleton => Scheme::Skeleton,
            };
            let tokens = |entries: &[(String, ThemeVal)]| -> Vec<_> {
                entries.iter().map(|(k, v)| (k.clone(), TokenValue::from(*v))).collect()
            };
            let script = theme_tokens::theme_derived_script(crate::THEME_MIX_NAME, "theme_install_source",
                scheme, &tokens(&spec.globals), &tokens(&spec.overrides));
            crate::set_theme_mix(cx, Some(script));
        }
    }
    cx.request_style_reload();
    cx.redraw_all();
    Ok(())
}

pub(super) fn remote(cx: &mut Cx, args: &super::Args) -> Result<String, String> {
    use super::{arg, need, json_str};
    match (arg(args, "get"), arg(args, "set"), arg(args, "install")) {
        (Some(_), None, None) => {
            let name = arg(args, "name");
            let entries = theme_values(cx).into_iter().filter(|(n, _, _, _)| name.is_none_or(|s| s == n))
                .map(|(n, _, value, site)| format!(r#"{{"name":{},"value":{},"site":{}}}"#,
                    json_str(&n), json_str(&theme_val_text(value)), json_str(&site)))
                .collect::<Vec<_>>();
            if entries.is_empty() && name.is_some() { return Err("unknown theme token".into()); }
            let sheets = desktop_style::catalogue().into_iter().filter_map(|entry| {
                let base = theme_tokens::sheet_base(entry)?;
                let appearance = if base == Scheme::Dark { "dark" } else { "light" };
                Some(format!(r#"{{"id":{},"label":{},"base":"{}","appearance":"{appearance}"}}"#,
                    json_str(entry.id), json_str(entry.label), base.theme_name()))
            }).collect::<Vec<_>>();
            Ok(format!(r#"{{"base":"{}","sheet":{},"has_mix":{},"has_pins":{},"edits":{},"sheets":[{}],"values":[{}],"define_file":{}}}"#,
                crate::base_theme(cx).id(), cx.with_vm(desktop_style::current_name).serialize_json(),
                crate::theme_mix(cx).is_some(), crate::theme_pins(cx).is_some_and(|p| !p.is_empty()),
                crate::theme_edits(cx).serialize_json(), sheets.join(","), entries.join(","), theme_define_file(cx).serialize_json()))
        }
        (None, Some(_), None) => {
            let name = need(args, "name")?;
            let (old, new, changed) = if let Some(edit) = theme_set_value(cx, name, need(args, "value")?, false)? {
                crate::set_theme_edit(cx, name, Some(edit.literal));
                cx.request_style_reload();
                (theme_val_text(edit.old), theme_val_text(edit.new), true)
            } else {
                let current = cx.with_vm(|vm| {
                    let theme = vm.module(id!(theme));
                    ThemeVal::read(vm.value(theme, LiveId::from_str(name)))
                }).ok_or("unknown theme token")?;
                let text = theme_val_text(current);
                (text.clone(), text, false)
            };
            cx.redraw_all();
            Ok(format!(r#"{{"ok":true,"name":{},"old":{},"new":{},"changed":{changed},"global":{}}}"#,
                json_str(name), json_str(&old), json_str(&new), theme_tokens::is_global_key(name)))
        }
        (None, None, Some(_)) => {
            let spec = parse_install_spec(need(args, "spec")?)?;
            apply_install_spec(cx, &spec)?;
            Ok(format!(r#"{{"ok":true,"base":"{}","sheet":{}}}"#, spec.base.id(), spec.sheet.serialize_json()))
        }
        _ => Err("design theme needs get, set or install".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_value_reads_the_way_the_ledger_writes_it() {
        assert_eq!(theme_val_text(ThemeVal::Color(0xff5c39ff)), "#ff5c39ff");
        assert_eq!(theme_val_text(ThemeVal::Color(0x00000080)), "#00000080");
        assert_eq!(theme_val_text(ThemeVal::Num(2.0)), "2");
        assert_eq!(theme_val_text(ThemeVal::Num(0.125)), "0.125");
    }

    #[test]
    fn a_theme_alias_resolves_to_the_other_tokens_current_text() {
        let values = vec![
            (
                "color_a".to_string(),
                LiveId::from_str("color_a"),
                ThemeVal::Color(0xff5c39ff),
                "t.rs:1".to_string(),
            ),
            (
                "space_factor".to_string(),
                LiveId::from_str("space_factor"),
                ThemeVal::Num(10.0),
                "t.rs:2".to_string(),
            ),
        ];
        assert_eq!(theme_edit_text(&values, "theme.color_a").unwrap(), "#ff5c39ff");
        assert_eq!(theme_edit_text(&values, "theme.space_factor").unwrap(), "10");
        // Anything that is not an alias is taken as written; parsing is the
        // apply path's job, not this one's.
        assert_eq!(theme_edit_text(&values, "#000").unwrap(), "#000");
        assert_eq!(theme_edit_text(&values, "12").unwrap(), "12");
        assert!(theme_edit_text(&values, "theme.nope").unwrap_err().contains("nope"));
    }
}
