//! The catalogue's remote ops, reached as `/tweak/op?op=<name>&...` on the
//! app's `--remote` control surface.
//!
//! The platform hands `/tweak/op` to one callback, the design overlay's by
//! default. The catalogue installs a wrapper that answers its own ops and
//! delegates everything else to the overlay's, so a test can open a story,
//! pick a theme or reset the canvas without driving the pointer. The
//! callback runs on the UI thread but outside the app's event handling, so
//! it queues requests the app drains at the start of its next event.
use crate::makepad_widgets::*;
use crate::registry;
use crate::settings;
use crate::theme;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Open(String),
    Theme(usize),
    Reset,
}

static PENDING: Mutex<Vec<Request>> = Mutex::new(Vec::new());
static CURRENT: Mutex<Option<String>> = Mutex::new(None);

pub fn install(cx: &mut Cx) {
    cx.tweak_callback = Some(callback);
    cx.design_preview_callback = Some(design_preview);
}

/// The designer's preview of a story file, without the app-wide live edit:
/// the file's override is installed, the evaluated record dropped, and the
/// canvas rebuilt, which re-runs that one file. Files outside the stories
/// (a widget's own source) are left to the platform's live edit.
fn design_preview(cx: &mut Cx, file: &str, text: &str) -> Option<Result<(), String>> {
    let normalized = file.replace('\\', "/");
    if !normalized.contains("/apps/storybook/src/stories/") {
        return None;
    }
    // The canvas by type, not by name: a name search takes the shallowest
    // `canvas` in the tree, which need not be the story canvas.
    let canvas = cx
        .widget_tree()
        .flat_tree(cx)
        .into_iter()
        .find(|row| row.ty == "StoryCanvas")
        .map(|row| cx.widget_tree().widget(WidgetUid(row.uid)))
        .filter(|w| !w.is_empty());
    let Some(canvas) = canvas else {
        log!("storybook: design preview found no StoryCanvas; the live edit takes it");
        return None;
    };
    let changed = match cx.install_live_edit_text(file, text) {
        Ok(changed) => changed,
        Err(err) => return Some(Err(err)),
    };
    crate::stories::forget_evaluated(cx);
    // The rebuild runs the story file again under reload, as the live edit
    // would, so widgets that guard their state on `is_reload` behave alike.
    cx.live_edit_capture_begin();
    cx.with_vm(|vm| vm.bx.is_reload = true);
    let rebuilt = match canvas.borrow_mut::<crate::canvas::StoryCanvas>() {
        Some(mut canvas) => {
            canvas.rebuild(cx);
            true
        }
        None => false,
    };
    cx.with_vm(|vm| vm.bx.is_reload = false);
    cx.live_edit_capture_end();
    log!(
        "storybook: design preview of {} (override {}, canvas {})",
        normalized.rsplit('/').next().unwrap_or(&normalized),
        if changed { "installed" } else { "unchanged" },
        if rebuilt { "rebuilt" } else { "NOT rebuilt" }
    );
    if !rebuilt {
        return None;
    }
    Some(Ok(()))
}

/// The app records the story it shows so `story_state` can report it.
pub fn set_current(key: &str) {
    *CURRENT.lock().unwrap() = Some(key.to_string());
}

pub fn take_requests() -> Vec<Request> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}

fn arg<'a>(args: &'a [(String, String)], key: &str) -> Option<&'a str> {
    args.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn queue(cx: &mut Cx, request: Request) -> Result<String, String> {
    PENDING.lock().unwrap().push(request);
    cx.redraw_all();
    Ok("{\"ok\":1}".to_string())
}

fn callback(cx: &mut Cx, op: &str, args: &[(String, String)]) -> Result<String, String> {
    match op {
        "stories" => {
            let baseline = settings::baseline();
            let mut out = String::from("{\"stories\":[");
            for (i, story) in registry::all().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    "{{\"key\":{},\"category\":{},\"component\":{},\"name\":{},\"dsl\":{},\"added\":{},\"new\":{}}}",
                    json_str(story.key),
                    json_str(story.category),
                    json_str(story.component),
                    json_str(story.name),
                    json_str(story.dsl),
                    json_str(story.added),
                    if registry::is_new(story, &baseline) { 1 } else { 0 }
                ));
            }
            out.push_str(&format!("],\"baseline\":{}}}", json_str(&baseline)));
            Ok(out)
        }
        "story" => {
            let Some(key) = arg(args, "name").or_else(|| arg(args, "key")) else {
                return Err("need name=<story key>".to_string());
            };
            if registry::find(key).is_none() {
                return Err(format!("no story {key}"));
            }
            queue(cx, Request::Open(key.to_string()))
        }
        "story_theme" => {
            let Some(name) = arg(args, "name") else {
                return Err(format!("need name={}", theme::names().join("|")));
            };
            let Some(index) = theme::index_of(name) else {
                return Err(format!("no theme {name}"));
            };
            queue(cx, Request::Theme(index))
        }
        "story_reset" => queue(cx, Request::Reset),
        "story_state" => {
            let current = CURRENT.lock().unwrap().clone().unwrap_or_default();
            Ok(format!(
                "{{\"story\":{},\"theme\":{},\"baseline\":{},\"stories\":{}}}",
                json_str(&current),
                json_str(&theme::names()[theme::choice()]),
                json_str(&settings::baseline()),
                registry::all().count()
            ))
        }
        _ => crate::makepad_widgets::tweaker::tweak_callback(cx, op, args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_escape() {
        assert_eq!(json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    }
}
