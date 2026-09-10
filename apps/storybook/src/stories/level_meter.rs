//! The level meter: a bar for a value that falls, both ways round, with the
//! high-water mark, the latching lamp and the display taper.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.LevelMeterOverview = StoryPage{
        StoryNote{text: "The one readout in the library that is allowed to go down. Everything under Progress eases forward only, on purpose; load, throughput, latency and headroom need the opposite, and the fall is the part being read. The bar takes a rise the instant it lands and gives it up on a schedule, a mark holds the highest recent reading, and a lamp latches when the host says a ceiling was passed."}
        StoryHeading{text: "One meter, under the controls"}
        StoryNote{text: "A standing reading, not a live one: nothing is feeding this meter, so the controls are what move it. Level stands the bar and the mark where you put them."}
        StoryRow{
            subject := LevelMeter{level: 0.55}
        }

        StoryHeading{text: "Fed"}
        StoryNote{text: "Hit sends one full-scale reading and lets go of it, which is the whole behaviour in one press: the bar is there at once, then falls, and the mark stands a second before following it down. Over sends a reading and lights the lamp; the lamp stays lit until Clear, or a press on the meter itself."}
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
        StoryNote{text: "The widget never decides what out of range means — the host hands it a bool. Handing it a false does nothing: an event that lasted a millisecond has to survive long enough to be seen, so only clearing puts the lamp out."}
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
        StoryNote{text: "The rungs are what make a level countable rather than merely long; a zero pitch draws the bar solid."}
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
        doc: "# LevelMeter\n\nA bar for a live value that FALLS. Every widget under Progress eases forward only — that file says outright that a bar sliding backwards looks like a bar that is lying — so load, throughput, latency, temperature and headroom had nothing to reach for. This is the widget for them, and the disagreement about what a falling number means is why it is a separate one rather than a flag on that family.\n\n- `feed(cx, reading)` is the live path: the highest value seen since the last one, as a share of full scale. The bar takes a rise the tick it arrives and gives it up over `release_secs` (nine tenths of the fall by default in 1.7 s). A mark holds the highest recent reading for `hold_secs` and then follows the bar down rather than dropping to meet it.\n- `level` is a STANDING reading for a meter nothing is feeding — a page being laid out, a catalogue row, a controls panel. Writing it again stands the bar and the mark there. It is not where a fed meter is, and it does not run anything.\n- `set_over(cx, true)` latches the lamp. Handing it a false does nothing on purpose: the event that lit it was over before the frame was. `clear` puts it out, and so does a press on the meter while `clear_on_press`, which raises `Cleared`.\n- `taper` is the exponent the reading is drawn on. One is linear; below one lifts the quiet end of the scale. It moves where a reading is drawn and never what `value` reports.\n- `vertical` turns the same widget into a column — `LevelMeterColumn` is that and no lamp. Give a column a real height: `Fill` inside a page that scrolls lays it out and never paints it.\n\n`MeterBallistics` beside the widget is the arithmetic on its own, for a host that draws its own meter: `tick(reading, dt)`, `level()`, `hold()` and `take_push()`, the deadband that decides whether anything has moved enough to be worth a repaint. It takes `dt` rather than reading a clock, so the fall is the same speed on a machine that is servicing it late — and so it can be held to its own arithmetic in a test.\n\nA reading past full scale is pinned, not remembered: one spike would otherwise hold the bar at the end through a second and a half of decay nobody can see. That a ceiling was passed is the lamp's news to carry.\n\nThe meter runs frames only while there is something left to animate, and asks for a redraw only when the drawn value has moved far enough to see. A settled meter costs nothing until the next reading arrives.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Level", target: "subject", kind: ControlKind::Number { prop: "level", min: 0., max: 1., step: 0.01, default: 0.55 } },
            Control { label: "Taper", target: "subject", kind: ControlKind::Number { prop: "taper", min: 0.2, max: 2., step: 0.05, default: 1. } },
            Control { label: "Release (s)", target: "subject", kind: ControlKind::Number { prop: "release_secs", min: 0.05, max: 3., step: 0.05, default: 0.74 } },
            Control { label: "Lamp", target: "subject", kind: ControlKind::Bool { prop: "lamp", default: true } },
            Control { label: "Lamp lit", target: "subject", kind: ControlKind::Bool { prop: "over", default: false } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(level_meter_actions),
    },
];
