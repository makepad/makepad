//! The level meter: a bar for a value that falls, both ways round, with the
//! high-water mark, the latching lamp and the display taper.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.LevelMeterOverview = StoryPage{
        StoryNote{text: "The one readout in the library that is read by its FALL. The bars and rings under Progress ease forward only and snap on any value set below the one on screen, on purpose. The Gauge beside them does ease down, but it eases a needle toward a target at one speed: no instant attack, no high-water mark, no repaint gate — a dial, not a meter. Load, throughput, latency and headroom want all three, so they get this. The bar takes a rise the instant it lands and gives it up on a schedule, a mark holds the highest recent reading, and a lamp latches when the host says a ceiling was passed."}
        StoryHeading{text: "One meter, under the controls"}
        StoryNote{text: "A standing reading, not a live one: nothing is feeding this meter, so the controls are what move it. Level stands the bar and the mark where you put them, and Taper moves where that reading is drawn. Release is ballistics, and ballistics have nothing to act on until a meter is being fed — so that one control is pointed at the fed meter below. Drag it, then press Hit."}
        StoryRow{
            subject := LevelMeter{level: 0.55}
        }

        StoryHeading{text: "Fed"}
        StoryNote{text: "Hit sends one full-scale reading and lets go of it, which is the whole behaviour in one press: the bar is there at once, then falls, and the mark stands a second before following it down. Half sends half scale: press it once the bar has fallen below the middle and it jumps straight up there, because a rise is taken at once wherever the bar had got to. Over sends a reading and lights the lamp; the lamp stays lit until Clear, or a press on the meter itself. Empty takes the channel away: bar, mark and lamp together."}
        StoryRow{
            meter := LevelMeter{width: 260.}
            hit := Button{text: "Hit"}
            half := Button{text: "Half"}
            lamp_on := Button{text: "Over"}
            lamp_off := Button{text: "Clear"}
            empty := Button{text: "Empty"}
        }
        StoryRow{
            lamp_note := Label{text: "the lamp is out"}
        }

        StoryHeading{text: "Both ways round"}
        StoryNote{text: "One widget with an orientation, not two widgets. A column wants a height it can actually have: inside a page that scrolls, Fill collapses and the meter is laid out but never painted."}
        StoryRow{
            LevelMeterColumn{level: 0.9}
            LevelMeterColumn{level: 0.62}
            LevelMeterColumn{level: 0.34}
            LevelMeterColumn{level: 0.08}
            LevelMeterColumn{width: 16. level: 0.75 lamp: true over: true}
        }

        StoryHeading{text: "The lamp"}
        StoryNote{text: "The widget never decides what out of range means — the host hands it a bool through set_over. Handing it a false does nothing: an event that lasted a millisecond has to survive long enough to be seen, so only clearing puts the lamp out. The catalogue's own Lamp lit (property) switch is the exception that proves it — it is not a host calling set_over, it writes the over property straight onto the widget, which is the one path that puts the lamp out without clearing anything."}
        StoryRow{
            LevelMeter{level: 0.4}
            LevelMeter{level: 0.4 over: true}
            LevelMeter{level: 0.99 over: true}
            LevelMeter{level: 0.4 lamp: false}
        }

        StoryHeading{text: "Taper"}
        StoryNote{text: "The same reading on two scales. Linear spends almost the whole bar on the loud end; an exponent below one lifts the quiet end, where a meter watching something that lives near the floor does all its reading. It moves where a reading is DRAWN and never what it is."}
        StoryRow{
            Label{width: 90. text: "linear"}
            LevelMeter{width: 220. level: 0.2}
            Label{width: 90. text: "taper 0.5"}
            LevelMeter{width: 220. level: 0.2 taper: 0.5}
        }

        StoryHeading{text: "Trough, ladder and mark"}
        StoryNote{text: "The rungs are what make a level countable rather than merely long; a zero pitch draws the bar solid, and a wider pitch counts in coarser steps. The third is the same meter given room: twenty points tall, a rounder trough, a heavier mark and a longer lamp. Its longer lamp takes its room off the trough, and the reading is drawn against what is left, so all three stand at seven tenths of their own trough: a longer lamp shortens the fill, it never shortens the reading."}
        StoryRow{
            LevelMeter{width: 220. level: 0.7 draw_bg.segment: 0.0}
            LevelMeter{width: 220. level: 0.7 draw_bg.segment: 8.0}
            LevelMeter{width: 220. height: 20. level: 0.7 draw_bg.border_radius: 6. draw_bg.mark_size: 3. draw_bg.lamp_size: 12.}
        }

        StoryHeading{text: "Not live"}
        StoryNote{text: "Dimmed rather than emptied: a channel that has been taken out of service still has a last reading, and blanking it would claim the reading was zero."}
        StoryRow{
            LevelMeter{level: 0.6 disabled: true}
            LevelMeterColumn{level: 0.6 disabled: true}
        }
    }
}

fn level_meter_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let meter = root.level_meter(cx, ids!(meter));
    let note = root.label(cx, ids!(lamp_note));
    if root.button(cx, ids!(hit)).clicked(actions) {
        meter.feed(cx, 1.0);
    }
    if root.button(cx, ids!(half)).clicked(actions) {
        meter.feed(cx, 0.5);
    }
    if root.button(cx, ids!(lamp_on)).clicked(actions) {
        meter.feed(cx, 1.0);
        meter.set_over(cx, true);
        note.set_text(cx, "the lamp is lit and will stay lit");
    }
    if root.button(cx, ids!(lamp_off)).clicked(actions) {
        meter.clear(cx);
        note.set_text(cx, "the lamp is out");
    }
    if root.button(cx, ids!(empty)).clicked(actions) {
        meter.reset(cx);
        note.set_text(cx, "the lamp is out");
    }
    // The press-to-clear path reports itself, so the page can say which of
    // the two ways the lamp went out.
    if meter.cleared(actions) {
        note.set_text(cx, "a press on the meter put the lamp out");
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "feedback/level-meter/overview",
        category: "Feedback",
        component: "LevelMeter",
        also: &["LevelMeterColumn"],
        name: "Overview",
        dsl: "LevelMeterOverview",
        added: "2026-09-10",
        tags: &["controls", "new"],
        doc: "# LevelMeter\n\nA bar for a live value that FALLS. The bars and rings under Progress ease forward only — that file says outright that a bar sliding backwards looks like a bar that is lying — and snap on any value set below the one on screen, so load, throughput, latency, temperature and headroom had nothing to reach for. `Gauge`, in the same file, is the one member of that family allowed to fall, and it eases a needle toward a target at one speed in both directions: no instant attack, no high-water mark, no repaint gate. A dial, not a meter. This is the widget for the rest, and the disagreement about what a falling number means is why it is a separate one rather than a flag on that family.\n\n- `feed(cx, reading)` is the live path: the highest value seen since the last one, as a share of full scale. The bar takes a rise the tick it arrives and gives it up over `release_secs` — the fall's TIME CONSTANT, not the time a fall takes: a tick covers `1 - e^(-dt/release_secs)` of what is left, so nine tenths of a fall is about 2.3 of them and the default 0.74 s is a nine-tenths fall in 1.7 s. A mark holds the highest recent reading for `hold_secs` and then follows the bar down rather than dropping to meet it.\n- `level` is a STANDING reading for a meter nothing is feeding — a page being laid out, a catalogue row, a controls panel. Writing it again stands the bar and the mark there. It is not where a fed meter is, and it does not run anything: `release_secs` and `hold_secs` have nothing to act on until something feeds the meter.\n- `set_over(cx, true)` latches the lamp. Handing it a false does nothing on purpose: the event that lit it was over before the frame was. `clear` puts it out, and so does a press on the meter while `clear_on_press`, which raises `Cleared`. Writing the `over` property is a different thing from calling `set_over` — it lands straight on the field, past the guard that drops a false, which is why this page's Lamp lit (property) switch can put a lit lamp out without clearing anything, and a host handing `set_over` a false cannot.\n- `taper` is the exponent the reading is drawn on. One is linear; below one lifts the quiet end of the scale. It moves where a reading is drawn and never what `value` reports.\n- `vertical` turns the same widget into a column — `LevelMeterColumn` is that and no lamp. Give a column a real height: `Fill` inside a page that scrolls lays it out and never paints it.\n\n`MeterBallistics` beside the widget is the arithmetic on its own, for a host that draws its own meter: `tick(reading, dt)`, `level()`, `hold()` and `take_push()`, the deadband that decides whether anything has moved enough to be worth a repaint. It takes `dt` rather than reading a clock, so the fall is the same speed on a machine that is servicing it late — and so it can be held to its own arithmetic in a test.\n\nA reading past full scale is pinned, not remembered: one spike would otherwise hold the bar at the end through a second and a half of decay nobody can see. That a ceiling was passed is the lamp's news to carry.\n\nThe meter runs frames only while there is something left to animate, and asks for a redraw only when the drawn value has moved far enough to see. A settled meter costs nothing until the next reading arrives.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Level", target: "subject", kind: ControlKind::Number { prop: "level", min: 0., max: 1., step: 0.01, default: 0.55 } },
            Control { label: "Taper", target: "subject", kind: ControlKind::Number { prop: "taper", min: 0.2, max: 2., step: 0.05, default: 1. } },
            // Release is the one parameter a standing meter cannot show: a
            // seeded bar never ticks, so nothing ever applies it. Pointed at
            // the fed meter instead, where a press on Hit shows the fall it
            // changes.
            Control { label: "Release (s)", target: "meter", kind: ControlKind::Number { prop: "release_secs", min: 0.05, max: 3., step: 0.05, default: 0.74 } },
            Control { label: "Lamp", target: "subject", kind: ControlKind::Bool { prop: "lamp", default: true } },
            // Named for what it does: this writes the property, which is not
            // the latch the page describes and is the one way to a dark lamp
            // that never calls clear.
            Control { label: "Lamp lit (property)", target: "subject", kind: ControlKind::Bool { prop: "over", default: false } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(level_meter_actions),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page is built from the DSL, which the Rust compiler never reads:
    /// a mistake in a `script_mod!` block shows up only in a running app's
    /// log, and a shader that fails to compile is not an error anywhere —
    /// the draw is skipped and the widget paints nothing at all. This page
    /// brings a shader of its own, a row the action handler reaches by id,
    /// and a control pointed at a widget that is not the subject, so
    /// building it and asking for every id anything here addresses is what
    /// turns any of those into a failed build.
    #[test]
    fn the_page_builds_and_every_id_it_is_driven_by_resolves() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            // Registering a template compiles nothing; making an instance out
            // of one does. Clearing here keeps another module's complaint out
            // of this test's answer.
            let _ = makepad_platform::shader_error::take();
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(
            makepad_platform::shader_error::take(),
            None,
            "the meter's draw shader failed to compile"
        );
        // The subject and every control target, or the panel moves a slider
        // that reaches nothing. `meter` is the one worth the trouble here:
        // Release is pointed at the fed meter rather than at the subject,
        // because a standing meter never ticks and never applies it.
        for target in std::iter::once(story.subject)
            .chain(story.controls.iter().map(|c| c.target))
            .filter(|t| !t.is_empty())
        {
            assert!(
                !page.widget(&cx, &[LiveId::from_str(target)]).is_empty(),
                "no widget at {target}"
            );
        }
        // And every id the action handler drives, as the type it expects: a
        // renamed button leaves `clicked` answering false for the rest of
        // the session, and nothing else here would notice.
        assert!(page.level_meter(&cx, ids!(meter)).borrow().is_some(), "no meter to feed");
        assert!(page.label(&cx, ids!(lamp_note)).borrow().is_some(), "no lamp_note to write");
        for (name, button) in [
            ("hit", ids!(hit)),
            ("half", ids!(half)),
            ("lamp_on", ids!(lamp_on)),
            ("lamp_off", ids!(lamp_off)),
            ("empty", ids!(empty)),
        ] {
            assert!(page.button(&cx, button).borrow().is_some(), "no button at {name}");
        }
    }
}
