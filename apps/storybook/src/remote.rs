//! Inspect commands for the catalogue. Theme selection runs before the platform's
//! live-edit pass. Canvas commands run at the start of the app's next event.
use crate::makepad_widgets::tween;
use crate::makepad_widgets::*;
use crate::registry;
use crate::settings;
use crate::theme;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Open(String),
    Focus(bool),
    Reset,
}

#[derive(Default)]
struct SyncState {
    story: Option<String>,
    applied_theme: String,
    epoch: u64,
    drawn_epoch: u64,
    needs_live_edit: bool,
    drawn: bool,
    focus: bool,
    pending: Vec<Request>,
}

impl SyncState {
    fn queued(&mut self, request: Request) -> String {
        self.pending.push(request);
        self.advance()
    }

    fn focused(&mut self, on: bool) {
        self.focus = on;
        self.drawn = false;
    }

    fn advance(&mut self) -> String {
        self.epoch += 1;
        self.drawn = false;
        format!(r#"{{"ok":1,"epoch":{}}}"#, self.epoch)
    }

    fn theme_selected(&mut self, reloading: bool) -> String {
        // Two commands can arrive before the same LiveEdit. A no-op cannot
        // acknowledge an earlier reload that has not reached the app yet.
        self.needs_live_edit |= reloading;
        self.advance()
    }

    fn live_edit_applied(&mut self, theme: &str) {
        self.applied_theme = theme.to_owned();
        self.needs_live_edit = false;
        self.drawn = false;
    }

    fn mark_drawn(&mut self) {
        if self.pending.is_empty() && !self.needs_live_edit {
            self.drawn_epoch = self.epoch;
            self.drawn = true;
        }
    }

    fn settled(&self) -> bool {
        self.drawn && self.drawn_epoch == self.epoch && !self.needs_live_edit && self.pending.is_empty()
    }
}

static STATE: std::sync::LazyLock<Mutex<SyncState>> = std::sync::LazyLock::new(Default::default);
type RemoteCallback = fn(&mut Cx, &str, &[(String, String)]) -> Result<String, String>;
static PREVIOUS: Mutex<Option<RemoteCallback>> = Mutex::new(None);

pub fn install(cx: &mut Cx) {
    use makepad_platform::remote::{register_app_command, register_app_query};
    register_app_query("stories", "", "catalogue stories", |_| "", inspect);
    register_app_query("story_state", "", "current story, applied theme and draw epoch", |_| "", inspect);
    register_app_command("story", "name=<story key>", "open a story", |_| r#""name":{"type":"string"},"key":{"type":"string"}"#, inspect);
    register_app_command("story_theme", "name=<theme>", "select a theme", |_| r#""name":{"type":"string"}"#, inspect);
    register_app_command("story_reset", "", "reset the canvas", |_| "", inspect);
    register_app_command("story_focus", "on=0|1", "show the canvas alone", |_| r#""on":{"type":"boolean"}"#, inspect);
    register_app_command("tween", "[scale=] [paused=] [reduced=]", "inspect or adjust the tween ticker",
        |_| r#""scale":{"type":"number"},"paused":{"type":"boolean"},"reduced":{"type":"boolean"}"#, inspect);
    cx.design_preview_callback = Some(design_preview);
    if cx.tweak_callback.is_some_and(|f| std::ptr::fn_addr_eq(f, callback as RemoteCallback)) {
        return;
    }
    *PREVIOUS.lock().unwrap() = cx.tweak_callback;
    cx.tweak_callback = Some(callback);
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
    cx.with_vm(|vm| vm.set_reload(true));
    let rebuilt = match canvas.borrow_mut::<crate::canvas::StoryCanvas>() {
        Some(mut canvas) => {
            canvas.rebuild(cx);
            true
        }
        None => false,
    };
    cx.with_vm(|vm| vm.set_reload(false));
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

fn inspect(cx: &mut Cx, words: &[String], args: &[(String, String)]) -> Vec<String> {
    vec![callback(cx, &words[0], args).unwrap_or_else(|err| format!(r#"{{"err":{}}}"#, json_str(&err)))]
}

pub fn mark_story_opened(key: &str) {
    let mut state = STATE.lock().unwrap();
    state.story = Some(key.to_owned());
    state.drawn = false;
}

pub fn mark_startup(key: &str, theme: &str) {
    mark_story_opened(key);
    mark_live_edit_applied(theme);
}

pub fn mark_live_edit_applied(theme: &str) {
    STATE.lock().unwrap().live_edit_applied(theme);
}

pub fn mark_drawn() {
    STATE.lock().unwrap().mark_drawn();
}

pub fn mark_focus(on: bool) {
    STATE.lock().unwrap().focused(on);
}

pub fn take_requests() -> Vec<Request> {
    std::mem::take(&mut STATE.lock().unwrap().pending)
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

fn focus_arg(args: &[(String, String)]) -> Result<bool, String> {
    match arg(args, "on") {
        Some("1" | "true") => Ok(true),
        Some("0" | "false") => Ok(false),
        _ => Err("need on=0|1".into()),
    }
}

fn queue(cx: &mut Cx, request: Request) -> Result<String, String> {
    let reply = STATE.lock().unwrap().queued(request);
    cx.redraw_all();
    Ok(reply)
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
            let reloading = theme::select(cx, index);
            let reply = STATE.lock().unwrap().theme_selected(reloading);
            if !reloading { cx.redraw_all(); }
            Ok(reply)
        }
        "story_focus" => queue(cx, Request::Focus(focus_arg(args)?)),
        "story_reset" => queue(cx, Request::Reset),
        "story_state" => {
            let state = STATE.lock().unwrap();
            Ok(format!(
                r#"{{"story":{},"theme":{},"applied_theme":{},"epoch":{},"drawn":{},"settled":{},"focus":{},"baseline":{},"stories":{}}}"#,
                json_str(state.story.as_deref().unwrap_or_default()),
                json_str(&theme::names()[theme::choice()]),
                json_str(&state.applied_theme),
                state.epoch, state.drawn, state.settled(), state.focus,
                json_str(&settings::baseline()),
                registry::all().count()
            ))
        }
        "tween" => {
            // The app-wide tween ticker (GSAP globalTimeline.timeScale() /
            // pause() plus reduced motion): `scale=0.25&paused=1&reduced=0`,
            // each optional; answers the ticker as it now stands. `scale`
            // takes 0.1 to 4, the range of the playground's Global speed
            // slider, so the page always shows the value in effect; to
            // stop time use `paused=1` (a scale of 0 would keep every
            // playing host re-arming frames that move nothing).
            let scale = match arg(args, "scale") {
                Some(v) => match v.parse::<f64>() {
                    Ok(s) if (0.1..=4.0).contains(&s) => Some(s),
                    _ => return Err(format!("bad scale={v}")),
                },
                None => None,
            };
            let flag = |key: &str| -> Result<Option<bool>, String> {
                match arg(args, key) {
                    Some("1" | "true" | "yes" | "on") => Ok(Some(true)),
                    Some("0" | "false" | "no" | "off") => Ok(Some(false)),
                    Some(v) => Err(format!("bad {key}={v}")),
                    None => Ok(None),
                }
            };
            let paused = flag("paused")?;
            let reduced = flag("reduced")?;
            if scale.is_some() || paused.is_some() || reduced.is_some() {
                tween::set_tween_ticker(cx, |t| {
                    if let Some(s) = scale {
                        t.time_scale = s;
                    }
                    if let Some(p) = paused {
                        t.paused = p;
                    }
                    if let Some(r) = reduced {
                        t.reduced_motion = r;
                    }
                });
            }
            let t = tween::tween_ticker(cx);
            Ok(format!(
                "{{\"scale\":{},\"paused\":{},\"reduced\":{},\"epoch\":{}}}",
                t.time_scale, t.paused as u8, t.reduced_motion as u8, t.epoch
            ))
        }
        _ => {
            let previous = PREVIOUS.lock().unwrap().ok_or("storybook: no remote callback below this one")?;
            previous(cx, op, args)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_install_preserves_previous_remote_callback() {
        fn fake(_: &mut Cx, _: &str, _: &[(String, String)]) -> Result<String, String> {
            Ok(r#"{"fake":1}"#.into())
        }
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let original = *PREVIOUS.lock().unwrap();
        cx.tweak_callback = Some(fake);
        install(&mut cx);
        install(&mut cx);
        let installed = cx.tweak_callback.expect("catalogue installed its remote wrapper");
        assert!(PREVIOUS.lock().unwrap().is_some_and(|f| std::ptr::fn_addr_eq(f, fake as RemoteCallback)),
            "reinstall must preserve the original delegate");
        assert_eq!(installed(&mut cx, "design_tree", &[]).unwrap(), r#"{"fake":1}"#, "design commands reach the delegate");
        assert!(installed(&mut cx, "story_state", &[]).unwrap().contains("\"stories\":"), "catalogue commands reach their own handler");
        *PREVIOUS.lock().unwrap() = original;
    }

    #[test]
    fn a_reload_needs_its_live_edit_and_draw_but_a_no_op_needs_only_a_draw() {
        let mut state = SyncState::default();
        state.theme_selected(true);
        state.mark_drawn();
        assert!(!state.settled(), "a draw before LiveEdit still has the old theme");
        state.live_edit_applied("light");
        assert!(!state.settled(), "the new theme is applied but has not drawn");
        state.mark_drawn();
        assert!(state.settled());
        state.theme_selected(false);
        assert!(!state.settled(), "even a no-op acknowledges its own draw epoch");
        state.mark_drawn();
        assert!(state.settled());
        assert_eq!(state.drawn_epoch, 2);
    }

    #[test]
    fn story_focus_is_acknowledged_after_the_canvas_draws() {
        // A state of its own: the app tests drain the shared queue from other threads.
        let on = |v: &str| focus_arg(&[("on".into(), v.into())]);
        assert_eq!((on("1"), on("true"), on("0"), on("false")), (Ok(true), Ok(true), Ok(false), Ok(false)), "story_focus reads on=");
        assert!(focus_arg(&[]).is_err() && on("2").is_err(), "story_focus refuses a missing or unknown on=");
        let mut state = SyncState::default();
        state.queued(Request::Focus(true));
        state.mark_drawn();
        assert!(!state.settled(), "a draw while focus is queued does not settle it");
        assert_eq!(std::mem::take(&mut state.pending), [Request::Focus(true)], "focus is queued for the app");
        state.focused(true);
        assert!(!state.settled(), "focus applied but not drawn");
        state.mark_drawn();
        assert!(state.settled() && state.focus, "focus settles after the canvas draws");
    }

    #[test]
    fn json_strings_escape() {
        assert_eq!(json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    }
}
