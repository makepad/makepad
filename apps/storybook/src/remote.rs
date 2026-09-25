//! The catalogue's remote ops, reached as `/tweak/op?op=<name>&...` on the
//! app's `--remote` control surface.
//!
//! The platform hands `/tweak/op` to one callback, the design overlay's by
//! default. The catalogue installs a wrapper that answers its own ops and
//! delegates everything else to the overlay's, so a test can open a story,
//! pick a theme or reset the canvas without driving the pointer. The
//! callback runs on the UI thread but outside the app's event handling, so
//! it queues requests the app drains at the start of its next event.
use crate::makepad_widgets::tween;
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
