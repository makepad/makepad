//! The log list story: a pane that follows the newest line, the five marks
//! it can put in the gutter, and which colon-and-number shapes it will turn
//! into a link.
use crate::makepad_widgets::log::LogLevel;
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.LogListOverview = StoryPage{
        StoryNote{text: "Somewhere in every application there is a pane of arriving messages. The platform has kept them all along — every log and error line, with its level, in a ring any part of the app can read — and the ring's own notes say it was moved out of the automation surface so an app could show its own log. Nothing was built to show it, so two applications here grew their own: one drawing the tail into a single multi-line label, one a hand-written virtual list with its own icons. Neither is a widget. This is the third one, written once."}

        StoryHeading{text: "Following the newest"}
        StoryNote{text: "The pane holds the bottom while lines arrive and lets go the instant you scroll away from it — scroll up, press \"fifty at once\", and the lines land below you instead of dragging you down. Scroll back to the bottom and it takes hold again. It says which of the two it is doing and counts what you missed, so a host can offer the way back; \"back to the newest\" is that offer. Letting go holds LINES, not row numbers: once the pane is full every arriving line drops one off the front and renumbers every row behind it, so the view is given those rows back before the frame draws and the words under your eye stay where they were."}
        live_pane := LogList{
            width: Fill
            height: 210.
            lines: [
                "log | [I] src/main.rs:64:5 - starting up"
                "log | [I] src/settings.rs:112:9 - read 31 settings"
                "wait | [.] src/device.rs:88:13 - waiting for the output device"
                "warn | [W] src/device.rs:141:9 - the device answered after 2100ms"
                "log | [I] src/device.rs:150:9 - opened at 48000Hz, 512 frames"
                "log | [I] src/library.rs:207:5 - scanned 4812 files in 3.1s"
                "error | [E] src/library.rs:263:17 - two entries claim the same id"
                "log | [I] src/window.rs:44:5 - window 1 shown"
            ]
        }
        StoryRow{
            one_more := Button{text: "one more line"}
            fifty := Button{text: "fifty at once"}
            back := Button{text: "back to the newest"}
            errors_only := CheckBox{text: "errors only"}
            needle := TextInput{width: 160. empty_text: "filter"}
        }
        StoryRow{
            state := Label{text: "following the newest"}
        }

        StoryHeading{text: "The mark"}
        StoryNote{text: "Five levels, and each one gets a stripe down the leading edge and a word in the gutter — the word so that the level survives a screenshot, a colour-blind reader and a monochrome printer, which a coloured dot on its own does not. The two are coloured separately, because they are asking for different things: the word is ten points of text and has to be READ, so it takes a theme role that flips with the theme, while the stripe is three points of colour beside a word that already spells the level out, so it keeps the saturated status hue and stays a thing to find rather than a thing to read. Switch this page to the light theme and watch the amber stripe stay amber while the word goes dark. The stripe is painted by the row's own background rather than being a child of it: the row is sized to fit its text, and a full-height child inside a fitted parent resolves to nothing and never paints."}
        marks := LogList{
            width: Fill
            height: 150.
            lines: [
                "wait | something is in progress and has not finished"
                "log | the ordinary line, and the quietest mark"
                "warn | something is wrong but the work went on"
                "error | something failed and did not go on"
                "panic | the process is not going to recover from this"
            ]
        }

        StoryHeading{text: "References inside a line"}
        StoryNote{text: "Anything shaped like path:line or path:line:column becomes a link in place, in the middle of the wrapped text rather than pinned to the end of the row. Pressing one reports the path and the numbers and does nothing else — whether that file can be opened is the host's question. The scan is syntactic and deliberately narrow: a drive letter stays attached to its path, and clock times, plain word:number pairs and addresses with ports are left as text."}
        refs := LogList{
            width: Fill
            height: 190.
            lines: [
                "error | [E] widgets/src/log_list.rs:412:9 - a line and a column"
                "warn | [W] src/loader.rs:88 - a line on its own"
                "log | [I] C:\\work\\src\\loader.rs:88:4 - a drive letter is part of the path"
                "log | see (src/a.rs:9), and src/b.rs:10. - punctuation stays outside"
                "log | 12:30:45 finished the pass - a clock is not a file"
                "log | frames:31 dropped:0 - and neither is a count"
                "log | listening on http://127.0.0.1:8080 - nor is a port"
            ]
        }
        StoryRow{
            pressed := Label{text: "nothing pressed yet"}
        }

        StoryHeading{text: "What it costs"}
        StoryNote{text: "The list underneath virtualises, so about thirty rows exist as widgets whether the pane holds two hundred lines or ten thousand. A held line costs its own text and roughly fifty-six bytes, so ten thousand ordinary lines is on the order of a megabyte and a half; cap is how many are kept, 2000 by default to match the platform's ring, and the oldest go first. Pushing a line is constant work. Changing the filter or the floor is one pass over everything held — once per change, not once per frame. What it will not do at that size: it never measures a row, so with lines of differing heights the scroll bar is an estimate and the thumb drifts as you drag it."}
    }
}

/// A line number that moves about, so the demo's references are not all the
/// same one — the pane is being shown to somebody looking for a bug.
fn made_up_line(n: usize) -> usize {
    40 + (n * 7) % 240
}

/// A line in the shape this platform's log sink writes.
fn made_up_message(n: usize) -> (LogLevel, String) {
    let level = match n % 11 {
        0 => LogLevel::Error,
        4 => LogLevel::Warning,
        7 => LogLevel::Wait,
        _ => LogLevel::Log,
    };
    let prefix = match level {
        LogLevel::Error => "[E]",
        LogLevel::Warning => "[W]",
        LogLevel::Wait => "[.]",
        _ => "[I]",
    };
    let words = match level {
        LogLevel::Error => "the pass gave up",
        LogLevel::Warning => "the pass took longer than it should have",
        LogLevel::Wait => "waiting on the pass before this one",
        _ => "the pass finished",
    };
    (level, format!("{prefix} src/pump.rs:{}:9 - line {n}, {words}", made_up_line(n)))
}

fn log_list_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let pane = root.log_list(cx, ids!(live_pane));
    if root.button(cx, ids!(one_more)).clicked(actions) {
        let n = crate::stories::bump(live_id!(log_list_line));
        let (level, text) = made_up_message(n);
        pane.push(cx, level, &text);
    }
    if root.button(cx, ids!(fifty)).clicked(actions) {
        // A burst, the way a log arrives: one redraw for the lot, which is
        // what `extend` is for.
        let burst: Vec<(LogLevel, String)> = (0..50)
            .map(|_| made_up_message(crate::stories::bump(live_id!(log_list_line))))
            .collect();
        pane.extend(cx, burst.iter().map(|(level, text)| (*level, text.as_str())));
    }
    if root.button(cx, ids!(back)).clicked(actions) {
        pane.follow(cx);
    }
    if let Some(on) = root.check_box(cx, ids!(errors_only)).changed(actions) {
        pane.set_floor(cx, if on { LogListFloor::Errors } else { LogListFloor::Everything });
    }
    if let Some(needle) = root.text_input(cx, ids!(needle)).changed(actions) {
        pane.set_filter(cx, &needle);
    }
    // Rewritten every pass rather than only when something changed: what it
    // reports includes whether the reader has scrolled, which no button of
    // this story's causes.
    let missed = match pane.unseen() {
        0 => String::new(),
        n => format!(" \u{b7} {n} arrived while you were reading"),
    };
    root.label(cx, ids!(state)).set_text(
        cx,
        &format!(
            "{} \u{b7} {} of {} lines showing{missed}",
            if pane.following() { "following the newest" } else { "let go, held where you left it" },
            pane.showing(),
            pane.held(),
        ),
    );
    for id in [live_id!(live_pane), live_id!(marks), live_id!(refs)] {
        let Some(reference) = root.log_list(cx, &[id]).reference_pressed(actions) else {
            continue;
        };
        let place = match reference.column {
            Some(column) => format!("line {}, column {column}", reference.line),
            None => format!("line {}", reference.line),
        };
        root.label(cx, ids!(pressed))
            .set_text(cx, &format!("{} \u{2014} {place}", reference.path));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data-display/loglist/overview",
    category: "Data display",
    component: "LogList",
    also: &["LogListRow", "LogListLink", "LogListEmpty", "LogListFloor"],
    name: "Overview",
    dsl: "LogListOverview",
    added: "2026-09-10",
    tags: &["new"],
    doc: "# LogList

A pane of arriving messages: a severity mark per line, the newest line held in view, and the file references inside the text made pressable.

**It does not read the platform's log ring.** The ring is there, and feeding this from it is three lines in a host's own tick — but a library widget that reached into a global would draw lines nobody handed it, could not be tested without a process that had logged something, and would be no use to a host whose lines come from a build, a device or a file. The host pushes; the level type is the platform's own, so that pump needs no conversion.

**Sticking is the point, and it is not this widget's code.** The virtualising list underneath already holds the bottom while content arrives and lets go when a reader scrolls away (`auto_tail`). What this adds is the report: it says when it lets go, counts what arrived while nobody was looking, and offers the way back, so a host can put a \"3 new\" pill on screen instead of guessing.

**And it holds lines, not row numbers.** The list numbers its rows from the oldest line showing, so once `cap` is reached — which for anything that logs at all is the steady state rather than an edge case — every arriving line drops one off the front and quietly makes every row number mean a newer line. The draw hands a reader who has scrolled away those rows back before it paints, so they stay on the lines they were reading. Without it a held view slides by one row per push while the pane goes on claiming to hold it.

**Two colours per level, not one.** The word in the gutter is ten points of text that has to be read in every theme, so it takes an adaptive role and measures between 9:1 and 13:1 against its row in all three; the stripe is three points of colour next to a word that already spells the level out, so it keeps the saturated status hue and stays something to find at a glance rather than something to read. Those hues are the same bytes in the light theme as in the dark one: the warning amber measures 1.25:1 against the light theme's row, which is a fine stripe and an unreadable word. The one exception is `log`, which stays on the theme's quiet meta role — the ordinary line is the absence of news, and its job in the gutter is to recede.

**What counts as a reference** is `path:line` or `path:line:column` — what this platform's log sink writes and what every compiler writes. The numbers are read from the RIGHT, which is what keeps `C:\\src\\a.rs:12:3` one reference rather than a column of 3 in a file called `C`. Clock times, `word:number` pairs and addresses with a port are left alone, and trailing punctuation stays outside the link. The pane marks what LOOKS like a reference and reports the press; whether the path can be opened is the host's question.

**`lines` is a fixture, not an input.** Written from markup as `\"level | text\"` it replaces whatever the pane holds, every time markup changes it — for a catalogue page, a screenshot or a test. A host that pushes must leave it empty. A head that is not one of the five level words is not a separator, so a message containing a bar keeps its front.

**Two things it will not do.** It never measures a row, so with wrapped lines of different heights the scroll bar is the list's estimate and the thumb drifts as you drag at ten thousand lines. And it has no notion of a line arriving twice: a message repeated a thousand times is a thousand rows, because collapsing them is a policy about a host's messages rather than about panes.",
    subject: "live_pane",
    feature: None,
    controls: &[],
    on_actions: Some(log_list_actions),
}];
