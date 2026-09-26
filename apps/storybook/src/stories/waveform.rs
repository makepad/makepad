//! The waveform story: a lot of numbers across a lane, the three kinds of
//! thing people put on one, and what a finger does to each.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::sync::OnceLock;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryWaveformStageBase = #(StoryWaveformStage::register_widget(vm))

    /** The page's own column, which hands its lanes their numbers as it
     * draws. See the Rust struct for why a page of Rust-fed lanes cannot
     * be seeded from the story's actions handler. */
    mod.storybook.StoryWaveformStage = set_type_default() do mod.storybook.StoryWaveformStageBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
    }

    mod.stories.WaveformOverview = StoryPage{
        stage := mod.storybook.StoryWaveformStage{
        StoryNote{text: "Six lanes and one ninety-second recording. Four of them are handed the same four and a half thousand min/max pairs; one is handed the same recording reduced to one magnitude a column, to show what that reduction costs; and one is handed nothing at all. Two carry regions and marks set from Rust and one carries the same ones written as markup, so three lanes carry neither. Every lane shows the whole recording across its own width, because that is the only thing this widget draws \u{2014} there is no zoom and nothing to scroll."}

        StoryHeading{text: "Peaks, and nothing else"}
        StoryNote{text: "A lane with no regions, no marks and no playhead: just the shape, with a rule down the middle at the level the peaks are measured from. The lowest and the highest sample in each column are kept apart, and this series leans downward on its transients by about a third, so the two halves of the envelope are visibly not the same half twice. From seventy-four seconds to seventy-eight the whole signal sits above the centre line, the way a recording with an offset on it does."}
        StoryRow{
            pairs := Waveform{
                width: Fill
                duration: 90.
                show_playhead: false
                marker_strip: 0.
            }
        }

        StoryHeading{text: "One number a column"}
        StoryNote{text: "The same recording through set_peaks, which takes one magnitude per column and mirrors it. The magnitude here is half the distance between a column's two ends, which is what a host with pairs computes when something downstream wants one number. Hold this lane against the one above: every column is exactly symmetric, so the lean on the hits is gone, and the four offset seconds from seventy-four to seventy-eight come back as an ordinary quiet stretch \u{2014} nothing in them is taller than the fade either side, and nothing says they were ever off the centre line. That is the whole difference between the two setters, and it is why set_peak_pairs is the one to use when the numbers came off real audio."}
        StoryRow{
            mirrored := Waveform{
                width: Fill
                height: 60.
                duration: 90.
                show_playhead: false
                marker_strip: 0.
            }
        }

        StoryHeading{text: "Regions, marks and a playhead"}
        StoryNote{text: "The top band belongs to the marks and nothing else is drawn or grabbed there; everything under it is the peaks, the region washes and their brackets. Press the body to move the playhead. Take a region by its opening or closing edge \u{2014} the edge brightens when the pointer is near enough to take it. Take a mark by its chip. Every one of those reports, and none of them does anything else. These three regions and six marks were set from Rust."}
        StoryRow{
            marked := Waveform{
                width: Fill
                duration: 90.
                playhead: 38.
            }
        }

        StoryHeading{text: "The short one"}
        StoryNote{text: "WaveformStrip is the same widget with a thin mark band, no names and less rounding \u{2014} the shape for a transport bar under something else, where the lane is a position and not a workspace. The chips are still there and still grabbable; only the names are gone, because a name under a toolbar is a name nobody has room to read. The band moves with marker_strip alone: the widget writes that one number into all three of its shaders every draw."}
        StoryRow{
            strip := WaveformStrip{
                width: Fill
                duration: 90.
                playhead: 38.
            }
        }

        StoryHeading{text: "A lane with nothing in it"}
        StoryNote{text: "No peaks were ever handed to this one, and none ever will be. It draws its ground and its centre rule and waits. A lane that drew nothing at all would look like a lane that failed to lay out, and the one moment this must not look broken is while it is waiting for a file."}
        StoryRow{
            Waveform{
                width: Fill
                height: 60.
                duration: 90.
                show_playhead: false
                marker_strip: 0.
            }
        }

        StoryHeading{text: "Written in markup, and the one to drive"}
        StoryNote{text: "The same three regions and six marks as the Rust lane above, written here as string lists instead \u{2014} one line each, fields separated by bars. The fourth field is an intent word, which is how the library colours anything that means something: a region marked warning is the same amber as a badge that means warning, in every theme. The last mark takes a raw #4f9d69 instead, which is the escape hatch for a host with a palette of its own. Note the colour has no x in front of the hex: the x that a colour needs in DSL source is there to stop the Rust tokenizer reading the digits as a suffixed numeric literal \u{2014} 1e2 is the clean example \u{2014} and there is no tokenizer inside a string. The hash is not optional either: without it, ace, decade and beaded are all runs of hex digits, and a lane that read one of them as a colour would be answering a typo."}
        StoryNote{text: "This is also the lane the controls panel writes on, and the one the property table beside it reflects \u{2014} so the regions and markers rows in that table are read off this lane rather than off the Rust ones. The table clips a value at twenty-four characters, so what those two rows show is the first line and an ellipsis: they are worth reading for provenance, not for content. Nothing on this page calls set_regions on it: that setter drops the string list a lane was written with, which is exactly what makes markup and Rust two ways and not one. The panel does not follow the lane either: it writes values and never reads them back, so after dragging the playhead here the Playhead slider still stands where it was, and the next nudge of it will move the playhead to the slider's number. That is the panel's shape, not the widget's."}
        StoryRow{
            subject := Waveform{
                width: Fill
                duration: 90.
                playhead: 12.
                regions: [
                    "12 | 30 | Build | primary"
                    "30 | 66 | The loud part | success"
                    "70.5 | 73 | Turnaround | warning"
                ]
                markers: [
                    "0 | Top"
                    "12 | In | info"
                    "30 | Drop | error"
                    "32 | Fill | error"
                    "66 | Out | secondary"
                    "84.5 | Tail | #4f9d69"
                ]
            }
        }
        StoryRow{
            says := Label{text: "the lane above has said nothing yet"}
        }

        StoryHeading{text: "A name that does not fit is not drawn"}
        StoryNote{text: "Turnaround runs two and a half seconds and its name is far wider than that stretch of lane, so it is left out rather than clipped or shortened: half a word says less than no word and costs a second working out that it is half a word, and the host has the whole name anyway. A mark's name is measured against the NEXT MARK IN TIME, which is why crowding drops the earlier name of a pair and never the later one. Drop and Fill are two seconds apart \u{2014} a forty-fifth of the lane, twenty-odd points at the width this page lays out at, and under ten once the two chip halves and the inset come off \u{2014} so Drop is the one left out. Fill is drawn: its own room is the thirty-four seconds to Out. Aiming is a different number again. Two chips closer together than twice marker_grab, fourteen points, have overlapping reaches, so a press in the band between them goes to whichever mark is nearer and to the earlier one on a tie \u{2014} but a press on either chip still takes that chip, because nearest wins. What takes the target away is chip_width: below nine points the chips themselves overlap and there is nothing separate left to point at."}
        }
    }
}

/// How many columns of picture the sample recording carries per second, how
/// long it runs, and how many columns there are to a half-beat.
///
/// Fifty a second is one column per twenty milliseconds: fine enough that a
/// kick reads as a kick on a lane a thousand points wide, coarse enough that
/// the whole recording is four and a half thousand pairs and thirty-six
/// kilobytes. A real host would hand over one or two hundred a second.
///
/// A hit every twenty-five columns is a hit every half second, which is a
/// hundred and twenty to the minute.
const COLUMNS_PER_SECOND: usize = 50;
const RECORDING_SECONDS: f64 = 90.0;
const COLUMNS_PER_HALF_BEAT: usize = 25;

/// The recording the page draws, built rather than pasted, and built once.
///
/// A real file would be a megabyte of repository nobody could review, and a
/// hand-typed series would have no shape — and a waveform with no shape
/// demonstrates nothing, because the whole question a waveform answers is
/// what shape the thing has. So: four sections at four loudnesses, a hit on
/// every half beat and a lighter one between them, and a value noise on top
/// so no two columns are the same. The seed is fixed and the beat is counted
/// in whole columns rather than in fractions of a second, so it is the same
/// picture on every machine and on every run, which is what makes this page
/// worth screenshotting.
///
/// Two things are deliberate in the numbers.
///
/// The pairs are asymmetric: a hit pushes further DOWN than up, by about a
/// third, the way a drum recorded through one microphone does. A mirrored
/// envelope cannot show that, and the lane below the first one is on the
/// page so the two can be compared.
///
/// And nothing here reaches the rail. The deepest column in the whole
/// recording goes to -0.9532, in the loud section at 42 seconds, and the
/// highest to +0.9196, inside the offset passage at 76.5 — so neither half
/// of the envelope is ever pinned flat against the edge, and a rail is a
/// straight line, which is the smear this widget exists to avoid. The clamp
/// is in the arithmetic, not in a `min` at the end of it, and
/// `the_sample_recording_is_asymmetric_off_centre_and_never_clipped` pins
/// both figures so this paragraph cannot drift away from the series.
fn sample_pairs() -> Vec<(f32, f32)> {
    let count = (RECORDING_SECONDS * COLUMNS_PER_SECOND as f64) as usize;
    let mut out = Vec::with_capacity(count);
    let mut seed: u32 = 0x9e37_79b9;
    for i in 0..count {
        // The arrangement: a quiet opening, a build, the loud part, and a
        // fade. The regions on the page are written at these same four
        // times, so a region edge lands where the picture changes.
        let section = if i < 12 * COLUMNS_PER_SECOND {
            0.22
        } else if i < 30 * COLUMNS_PER_SECOND {
            0.38
        } else if i < 66 * COLUMNS_PER_SECOND {
            0.56
        } else {
            0.30
        };
        // A hit on the beat, a lighter one on the offbeat, and the room tone
        // between them. Without the offbeat the picture is a comb; with it
        // it reads as music.
        let phase = i % COLUMNS_PER_HALF_BEAT;
        let (hit, lean) = if phase < 2 {
            (1.0, 1.34)
        } else if phase == 12 || phase == 13 {
            (0.55, 1.18)
        } else {
            (0.22, 1.02)
        };
        // Four seconds recorded with an offset on the line: the whole
        // envelope sits above the centre, both ends of every column with it.
        // It is the one thing a mirrored magnitude cannot show, so the page
        // has to contain one or the claim beside it is unchecked.
        let offset = if (74 * COLUMNS_PER_SECOND..78 * COLUMNS_PER_SECOND).contains(&i) {
            0.55
        } else {
            0.0
        };
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let grain = 0.72 + (seed >> 8) as f64 / (1u32 << 24) as f64 * 0.56;
        let up = section * hit * grain;
        let down = up * lean;
        out.push(((offset - down) as f32, (offset + up) as f32));
    }
    out
}

/// The same recording as one magnitude a column, for the mirrored lane.
///
/// Half the distance between a column's two ends, which is what a host with
/// pairs computes when something downstream wants one number. NOT the upward
/// half: the upward half of a signal with an offset on it carries the offset
/// as loudness, so the four offset seconds would arrive in the mirrored lane
/// as the tallest, most solid block on the page — louder than the loud part
/// — and the note beside that lane would be describing the opposite of what
/// a reader can see. Half the peak-to-peak cancels the offset exactly, which
/// is the honest demonstration: what mirroring loses is not the size of the
/// passage, it is the fact that the passage was off centre at all.
fn sample_magnitudes() -> Vec<f32> {
    peaks().iter().map(|(lo, hi)| (hi - lo) * 0.5).collect()
}

/// Built once and kept for as long as the app runs.
fn peaks() -> &'static Vec<(f32, f32)> {
    static PEAKS: OnceLock<Vec<(f32, f32)>> = OnceLock::new();
    PEAKS.get_or_init(sample_pairs)
}

fn magnitudes() -> &'static Vec<f32> {
    static MAGS: OnceLock<Vec<f32>> = OnceLock::new();
    MAGS.get_or_init(sample_magnitudes)
}

/// The one colour on this page that is not an intent, read through the same
/// parser the markup line uses, so the Rust lane and the markup lane really
/// do carry the same six marks.
fn own_green() -> Vec4f {
    let (c, _) = parse_hex_color("#4f9d69").expect("six hex digits");
    vec4(c[0], c[1], c[2], c[3])
}

/// Three regions, chosen so the lane has to answer three different
/// questions: one wide enough to hold its name, one that begins exactly
/// where the one before it ends, and one too narrow for a name at any width
/// this page is ever laid out at. Each carries an intent rather than a
/// colour, so all three stay right when the theme changes.
fn sample_regions() -> Vec<WaveformRegion> {
    vec![
        WaveformRegion::new(12.0, 30.0, "Build").with_intent(BadgeIntent::Primary),
        WaveformRegion::new(30.0, 66.0, "The loud part").with_intent(BadgeIntent::Success),
        WaveformRegion::new(70.5, 73.0, "Turnaround").with_intent(BadgeIntent::Warning),
    ]
}

/// Six marks. Drop and Fill are two seconds apart on purpose: far enough
/// that their nine-point chips do not overlap and each can be aimed at, and
/// close enough that there is no room for the EARLIER name between them, so
/// they are the pair that shows the name-skip rule. Fill's own name has the
/// thirty-four seconds to Out and is drawn — the rule looks forward, which
/// is the point the page makes with them. Tail is the escape hatch, in the
/// Rust form of the same colour the markup lane writes as a hex.
fn sample_markers() -> Vec<WaveformMarker> {
    vec![
        WaveformMarker::new(0.0, "Top"),
        WaveformMarker::new(12.0, "In").with_intent(BadgeIntent::Info),
        WaveformMarker::new(30.0, "Drop").with_intent(BadgeIntent::Error),
        WaveformMarker::new(32.0, "Fill").with_intent(BadgeIntent::Error),
        WaveformMarker::new(66.0, "Out").with_intent(BadgeIntent::Secondary),
        WaveformMarker::new(84.5, "Tail").with_color(own_green()),
    ]
}

/// The page's own column, which hands its lanes their numbers as it draws.
///
/// A story's only hook is `on_actions`, and `Event::Actions` is dispatched
/// only when there ARE actions (platform/src/os/cx_shared.rs:1110-1124). A
/// page whose whole content comes from Rust therefore cannot be seeded from
/// there: a theme change rebuilds every widget from its templates, no action
/// follows, and the catalogue shows six empty lanes until somebody clicks
/// something — hovering is not enough, because a hover raises no action. So
/// the seeding happens in a draw, which always runs. This is the arrangement
/// the data grid's page already uses for the same reason.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryWaveformStage {
    #[deref]
    view: View,
}

impl StoryWaveformStage {
    /// Hand each lane its numbers, once.
    ///
    /// This runs on every draw, so it has to cost nothing once the page is
    /// up. What it costs then is seven lookups by name down the view's
    /// children and a length comparison off each lane — no copy, and no
    /// borrow held past the comparison. That is the whole per-frame bill,
    /// and the lookups are the larger half of it.
    ///
    /// What it must not cost is the arrays. The setters do compare before
    /// they store, but they take their arrays BY VALUE and this page keeps
    /// its own copy in a `OnceLock`, so handing the same series over every
    /// frame would mean cloning thirty-six kilobytes per lane per frame to
    /// have it compared and dropped. Hence the count first, the clone only
    /// behind it — and the `is_empty` check in front of both, because a
    /// lane that cannot be found reports a count of zero forever and would
    /// otherwise be handed a fresh clone on every frame, silently, in the
    /// one place this page is trying not to copy.
    fn seed(&mut self, cx: &mut Cx) {
        for path in [ids!(pairs), ids!(marked), ids!(strip), ids!(subject)] {
            let lane = self.view.widget(cx, path).as_waveform();
            if lane.is_empty() {
                continue;
            }
            if lane.peak_count() != peaks().len() {
                lane.set_peak_pairs(cx, peaks().clone());
            }
        }
        let mirrored = self.view.widget(cx, ids!(mirrored)).as_waveform();
        if !mirrored.is_empty() && mirrored.peak_count() != magnitudes().len() {
            mirrored.set_peaks(cx, magnitudes().clone());
        }
        // The markup lane is not in this list. `set_regions` drops the
        // string list a lane was written with, and on that lane the string
        // list is the whole point — it is what the property table beside
        // the page reflects.
        for path in [ids!(marked), ids!(strip)] {
            let lane = self.view.widget(cx, path).as_waveform();
            if lane.is_empty() {
                continue;
            }
            if lane.region_count() == 0 {
                lane.set_regions(cx, sample_regions());
                lane.set_markers(cx, sample_markers());
            }
        }
    }
}

impl Widget for StoryWaveformStage {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Before the inner draw, not after it. The lanes are in
        // `View::children` from the moment the template is applied, not from
        // the moment it is drawn — `the_page_builds_and_every_named_lane_is_on_it`
        // finds all five on a page that has never drawn — so seeding first
        // finds them just as well and they draw WITH their numbers on the
        // very first frame.
        //
        // Seeding afterwards looked equivalent and was not: the setters ask
        // for a redraw through `Area::redraw`, and `Cx::redraw_list`
        // (platform/src/cx_api.rs:1585-1590) returns without recording
        // anything while `in_draw_event` is true, which it is for the whole
        // draw pass. The request was dropped, so the page was correct only
        // if something unrelated happened to schedule a second frame — and
        // the case that matters most, a theme change rebuilding every widget
        // from its template with no action to follow, is exactly the case
        // where nothing does.
        self.seed(cx);
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

/// Say what the subject last reported. The lanes are not seeded here — see
/// [`StoryWaveformStage`].
fn waveform_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // One line per gesture. A press on the body puts two actions in one
    // pass — Grabbed first, then Seeking — and every reader scans the pass,
    // so both are visible and the later assignment wins: pressing the body
    // shows the seek. A press on a region edge or a chip puts only Grabbed
    // in its pass, so there it is "took hold of" that shows. A live report
    // and its settled one never share a pass, so nothing has to be ordered
    // against anything.
    let subject = root.waveform(cx, ids!(subject));
    let mut said = None;
    if let Some(part) = subject.grabbed(actions) {
        said = Some(match part {
            WaveformPart::Playhead => "took hold of the playhead".to_string(),
            WaveformPart::RegionStart(i) => format!("took hold of region {i} by its start"),
            WaveformPart::RegionEnd(i) => format!("took hold of region {i} by its end"),
            WaveformPart::Marker(i) => format!("took hold of mark {i}"),
        });
    }
    if let Some(at) = subject.seeking(actions) {
        said = Some(format!("seeking to {at:.2}"));
    }
    if let Some((i, start, end)) = subject.region_moving(actions) {
        said = Some(format!("region {i} is {start:.2} to {end:.2}"));
    }
    if let Some((i, at)) = subject.marker_moving(actions) {
        said = Some(format!("mark {i} is at {at:.2}"));
    }
    if let Some(at) = subject.seeked(actions) {
        said = Some(format!("let go of the playhead at {at:.2}"));
    }
    if let Some((i, start, end)) = subject.region_moved(actions) {
        said = Some(format!("region {i} settled at {start:.2} to {end:.2}"));
    }
    if let Some((i, at)) = subject.marker_moved(actions) {
        said = Some(format!("mark {i} settled at {at:.2}"));
    }
    if let Some(i) = subject.marker_picked(actions) {
        said = Some(format!("mark {i} was picked, not moved"));
    }

    if let Some(text) = said {
        let label = root.label(cx, ids!(says));
        if label.text() != text {
            label.set_text(cx, &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_widgets::makepad_script::trap::NoTrap;

    /// Build the page the way the canvas does: register the widgets, the
    /// shell the page is made of and this file's own templates, then look
    /// the template up under `mod.stories` and make a widget out of it.
    fn built_page(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            super::script_mod(vm);
            let stories = vm.module(id!(stories));
            let value = vm
                .bx
                .heap
                .value(stories, LiveId::from_str("WaveformOverview").into(), NoTrap);
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// A page whose DSL is wrong builds nothing, or builds a lane that
    /// quietly ignored half of what was written on it, and says so nowhere
    /// a compiler can hear it. This is where it is heard: every lane is
    /// looked up by the name the handler uses, and every property the page
    /// writes on one is read back off it.
    #[test]
    fn the_page_builds_and_every_named_lane_is_on_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = built_page(&mut cx);
        assert!(!page.is_empty(), "the template built a widget");
        for lane in [ids!(pairs), ids!(mirrored), ids!(marked), ids!(strip), ids!(subject)] {
            let lane = page.waveform(&cx, lane);
            assert!(!lane.is_empty(), "a named lane is missing");
            assert_eq!(lane.duration(), RECORDING_SECONDS, "the axis is the recording");
        }
        assert!(!page.label(&cx, ids!(says)).is_empty(), "the readout is missing");

        // The two lanes with no band and no head, and the two with both.
        for bare in [ids!(pairs), ids!(mirrored)] {
            let lane = page.waveform(&cx, bare);
            let inner = lane.borrow().expect("a bare lane");
            assert!(!inner.show_playhead);
            assert_eq!(inner.marker_strip, 0.0);
        }
        let marked = page.waveform(&cx, ids!(marked));
        assert_eq!(marked.playhead(), 38.0);
        assert_eq!(marked.borrow().expect("marked").marker_strip, 16.0);
        // And the short one is the variant, not the plain preset.
        let short = page.waveform(&cx, ids!(strip));
        let short = short.borrow().expect("the short lane");
        assert_eq!(short.marker_strip, 8.0, "the transport preset's thin band");
        assert!(!short.show_names);
        assert_eq!(short.chip_width, 7.0);
    }

    /// The opening note counts the page. It is the one sentence a reader
    /// takes on trust, so the count is pinned to the page rather than to
    /// whoever last edited one of them.
    ///
    /// The needles are joined at runtime because this file is searching its
    /// own source: written as one literal, each would match itself and the
    /// counts would come out one too high.
    #[test]
    fn the_opening_note_counts_the_lanes_the_page_declares() {
        let src = include_str!("waveform.rs");
        let plain = format!("{}{}", "Wave", "form{");
        let short = format!("{}{}", "Wave", "formStrip{");
        assert_eq!(src.matches(plain.as_str()).count(), 5, "five plain lanes");
        assert_eq!(src.matches(short.as_str()).count(), 1, "and one short one");
        let counted = format!("{} lanes", "Six");
        assert!(src.contains(&counted), "the note says how many there are");
    }

    /// What the markup lane is written with, read the way the widget reads
    /// it: the fields in the order the line puts them, the intent word in
    /// the last one, and the raw colour where the page says there is one.
    #[test]
    fn the_markup_lane_carries_the_lines_the_note_describes() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = built_page(&mut cx);
        let subject = page.waveform(&cx, ids!(subject));
        let inner = subject.borrow().expect("the subject lane");
        assert_eq!(inner.regions.len(), 3, "three region lines");
        assert_eq!(inner.markers.len(), 6, "six mark lines");

        let regions = parse_regions(&inner.regions);
        assert_eq!(regions[0].name, "Build");
        assert_eq!(regions[0].intent, BadgeIntent::Primary);
        assert_eq!((regions[0].start, regions[0].end), (12.0, 30.0));
        assert_eq!(regions[2].name, "Turnaround");
        assert_eq!(regions[2].intent, BadgeIntent::Warning);

        let marks = parse_markers(&inner.markers);
        assert_eq!(marks[0].name, "Top");
        assert_eq!(marks[2].name, "Drop");
        assert_eq!(marks[2].intent, BadgeIntent::Error);
        // The escape hatch, written without the `x` a colour needs in DSL
        // source, and the same green the Rust list gives its own last mark.
        assert_eq!(marks[5].name, "Tail");
        assert_eq!(marks[5].color, Some(own_green()));
        assert_eq!(sample_markers()[5].color, Some(own_green()));
    }

    /// The two lists the page hands the Rust lanes, against the lines the
    /// markup lane is written with: the note says they are the same three
    /// regions and six marks, so they have to be.
    #[test]
    fn the_rust_lists_and_the_markup_lines_are_the_same_marks() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = built_page(&mut cx);
        let subject = page.waveform(&cx, ids!(subject));
        let inner = subject.borrow().expect("the subject lane");
        assert_eq!(parse_regions(&inner.regions), sample_regions());
        assert_eq!(parse_markers(&inner.markers), sample_markers());
    }

    /// The series the page draws: asymmetric, off the rail at both ends,
    /// and carrying one passage whose whole envelope sits above the centre
    /// line — the thing a mirrored magnitude cannot show, which the page
    /// claims twice.
    #[test]
    fn the_sample_recording_is_asymmetric_off_centre_and_never_clipped() {
        let peaks = peaks();
        assert_eq!(peaks.len(), 4500);
        for (lo, hi) in peaks.iter() {
            assert!(*lo >= -1.0 && *hi <= 1.0, "a column reached the rail: {lo}..{hi}");
            assert!(lo <= hi);
        }
        // Nothing is even close to the rail, so no stretch of either half
        // is a flat line. These are the two figures the doc comment above
        // `sample_pairs` states, held to a hundredth either side: that
        // paragraph's whole point is that the series stops short, so its
        // numbers are the ones that must not drift away from the series.
        let deepest = peaks.iter().fold(0.0f32, |acc, (lo, _)| acc.min(*lo));
        let highest = peaks.iter().fold(0.0f32, |acc, (_, hi)| acc.max(*hi));
        assert!((-0.96..-0.95).contains(&deepest), "the deepest column is {deepest}");
        assert!((0.91..0.925).contains(&highest), "the highest column is {highest}");

        // A hit leans downward by about a third: the picture a mirrored
        // envelope flattens.
        let hits: Vec<&(f32, f32)> = peaks
            .iter()
            .enumerate()
            .filter(|(i, _)| i % COLUMNS_PER_HALF_BEAT == 0)
            .map(|(_, pair)| pair)
            .collect();
        let leaning = hits.iter().filter(|(lo, hi)| -*lo > *hi * 1.2).count();
        assert!(leaning > hits.len() * 3 / 4, "{leaning} of {} hits lean down", hits.len());

        // And four seconds of it sit entirely above the centre line.
        let from = 74 * COLUMNS_PER_SECOND;
        let to = 78 * COLUMNS_PER_SECOND;
        for (lo, hi) in peaks[from..to].iter() {
            assert!(*lo > 0.0, "the offset passage dips to {lo}");
            assert!(*hi < 1.0);
        }
        // Either side of it the signal straddles the centre again.
        assert!(peaks[from - 1].0 < 0.0);
        assert!(peaks[to].0 < 0.0);
    }

    /// What the mirrored lane actually shows — the page's second-largest
    /// claim, and the one series nothing here used to read.
    ///
    /// The note beside that lane says the offset passage "comes back as an
    /// ordinary quiet stretch". That is true of half the peak-to-peak and
    /// false of the upward half: take `hi` instead and those four seconds
    /// arrive as the loudest, most solid block on the page, mean 0.64
    /// against 0.09 for the fade either side of them. So the sentence is
    /// pinned to the numbers here — the passage has to sit INSIDE the range
    /// of its surroundings at both ends, and average the same as them.
    #[test]
    fn the_mirrored_series_drops_the_offset_instead_of_showing_it_as_loudness() {
        let mags = magnitudes();
        assert_eq!(mags.len(), peaks().len(), "one magnitude a column");

        let from = 74 * COLUMNS_PER_SECOND;
        let to = 78 * COLUMNS_PER_SECOND;
        let passage = &mags[from..to];
        // The rest of the closing section: the same loudness rung, either
        // side of the passage, with nothing else different about it.
        let rest: Vec<f32> = mags[66 * COLUMNS_PER_SECOND..from]
            .iter()
            .chain(mags[to..].iter())
            .copied()
            .collect();

        let least = |v: &[f32]| v.iter().copied().fold(f32::INFINITY, f32::min);
        let most = |v: &[f32]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!(
            least(passage) >= least(&rest),
            "the passage dips to {} and its surroundings to {}",
            least(passage),
            least(&rest)
        );
        assert!(
            most(passage) <= most(&rest),
            "the passage reaches {} and its surroundings {}",
            most(passage),
            most(&rest)
        );
        let (a, b) = (mean(passage), mean(&rest));
        assert!(
            (a - b).abs() < b * 0.05,
            "the passage averages {a} and its surroundings {b}"
        );
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/waveform/overview",
    category: "Media",
    component: "Waveform",
    also: &["WaveformStrip"],
    name: "Overview",
    dsl: "WaveformOverview",
    added: "2026-09-10",
    tags: &["new", "audio", "peaks", "playhead", "regions", "markers", "scrub"],
    doc: "# Waveform

The peaks of a recording across a lane, with the regions and marks somebody put on it.

Every application that shows audio ends up drawing the same lane, and ends up drawing it badly in the same way: a quad per column, which is a hundred thousand quads for a long recording; or one column sampled per pixel, which is a different picture every time the window is resized and a picture of nothing much whichever pixel it lands on. Then the regions get built out of two sliders that can cross, the marks get built out of absolutely-positioned labels that overlap, and the playhead gets built out of a one-point `View` that is a frame behind everything else.

`Waveform` is that lane, once.

**It is handed numbers and nothing else.** Peaks, regions, marks, how long the recording is, and how much of that the peaks cover. It does not decode audio, does not read files, does not analyse anything, and has no idea where the numbers came from — a file, a stream, a synthesiser, a made-up series on this page. `set_peak_pairs` takes the lowest and highest sample in each column, which is what real audio gives you and what shows a signal sitting off the centre line; `set_peaks` takes one magnitude a column and mirrors it, for a host that only has an envelope. Everything it takes and everything it reports is in whatever unit `duration` is counted in. It never converts one and never formats one.

**The peaks are the fixed picture, and everything else slides against them.** They are spread across the first `peaks_span` of the axis — all of it unless the host says otherwise — so changing `duration` moves the regions, the marks and the playhead, and does not stretch the peaks. The invariant behind that is worth stating plainly, because nothing enforces it and nothing can: **the peak array covers exactly `0..peaks_span`.** A host that sends peaks for the first thirty seconds of a five-minute file and leaves `peaks_span` alone gets thirty seconds of audio drawn across five minutes of lane, with every mark against the wrong part of it and no error anywhere. `peaks_span: 30.` is the fix, and it is what a file that is still decoding should be setting as it goes.

**The lane has two rows, and that is the whole reason the gestures work.** The top `marker_strip` points belong to the marks: their chips and their names, and nothing else is drawn or grabbed there — not a region wash, not a region bracket. Everything under it is the peaks, the region washes and their brackets, and the region names along the bottom. So a press in the strip is a mark or it is nothing, and a press in the body is a region edge or it is a seek. No press has to be guessed at, and a finger that slips off a chip does not move the playhead as a parting gift. Set `marker_strip` to 0 and there are no marks at all: not drawn, not hit-tested. The three shaders and the hit test all derive that split from one expression, `clamp(marker_strip, 0, height)`, so a band of any size — nothing, the default, or taller than the lane itself — means the same thing to all four.

## The gestures

| Gesture | Result |
|---|---|
| press or drag the body | moves the playhead, live |
| drag a region's edge | resizes that end; the other stays put |
| drag a mark's chip | moves it |
| press a chip and let go | picks it — nothing moves |
| a press in the strip on nothing | nothing |
| arrow keys, Home, End | the playhead, and only the playhead |
| the wheel | nothing — it belongs to whatever the lane is standing in |

A dragged region edge stops short of the other end rather than shoving it along: the finger is on one end, and a control that moved the end nobody is touching would destroy a number that was set on purpose. `min_span` is how much room the two must leave each other, and zero lets them meet. That rule and `min_span` are the range slider's — settled there, and copied here rather than re-argued. The arithmetic is a second, private `Lane`, because the range slider's `Range` is private to its own module; it is the same rule, not the same code.

A pick and a move are told apart by how far the finger travelled, not by whether the number changed. On a five-minute lane at the width a panel like this one lays out at, one point is about a third of a second, so every click would otherwise be a small silent edit.

The keyboard gets the playhead and nothing else. It has one selection and this lane has three kinds of thing on it; giving it the regions and the marks would need a selection model, and a selection model is a bigger widget than this.

A lane holding key focus draws a ring in `color_primary` inside its own edge. That is a ring and not a tint of the ground on purpose: `color_inset_hover`, `color_inset_focus` and `color_inset_drag` are aliases of `color_inset` in both shipped desktop themes, so a ground tint would leave a focused lane pixel-identical to an unfocused one — on a tab stop whose arrow keys edit a value. The peaks carry hover, focus and drag instead, through the `color_val` ladder, which does step per state in all three themes.

## Reading it

Each of the three continuous gestures reports twice: `seeking` / `region_moving` / `marker_moving` on every frame of it, and `seeked` / `region_moved` / `marker_moved` where it stopped. Use the first for anything that must keep up with the finger and the second for anything expensive — a drag across the lane passes through several hundred values on the way. `grabbed` fires on the press and says which part was taken, which is the only way to know what a drag is about to be before it has moved anything. `marker_picked` is the click. An arrow key reports `seeked` and nothing else, because a key press is settled the moment it happens.

**A pass can carry two of these, and when it does, `Grabbed` is first.** Pressing the body emits `Grabbed(Playhead)` and then `Seeking(at)` in one pass. That is why every accessor here scans the pass rather than taking the first action out of it: a reader built on `find_widget_action` sees whichever came first and reports nothing for the other. A live report and its settled one never share a pass, so there is no ordering to get right beyond that.

The widget moves its own copy of a region or a mark while the drag is on, so the picture keeps up with the finger, and reports every step. A host that writes the reported value back changes nothing; a host that refuses is left looking at the widget's copy until it calls `set_regions` to say otherwise. That is the same bargain the slider family makes, and the alternative — waiting for the host to answer before drawing — is a lane that lags a frame behind the pointer.

## Colour

A region or a mark says what it MEANS, in the same eight words every small mark in this library speaks: `neutral`, `primary`, `secondary`, `tertiary`, `error`, `warning`, `success`, `info`. They resolve through the same shared palette a badge and a chip use, so a region that means \"error\" is the same red as the badge that does, and stays right when the theme flips.

A raw colour is available as a clearly-named escape hatch — `with_color` in Rust, `#rrggbb` in a markup line — for a host whose own palette already carries meaning, a mixer whose channels are colour-coded or a score whose parts are. It is the second choice on purpose: a hex baked into a line is a colour nobody re-checks against the light theme.

## What it costs

One quad for the lane, one per region, one per mark, one for the playhead, and one text draw per name that fits. Nothing in a frame scales with the number of peaks.

The peaks are paid for once per change of peaks, duration, covered span or lane width: they are reduced to one texel per device pixel and uploaded as a single row of texture, sixteen bits per value, then read once per pixel by the lane's shader. The reduction takes the extremes and never an average — an average of a hundred columns of music is the same grey smear whatever the music was, and the transients are the part somebody opened this to look at.

Both setters take their array **by value** and compare it against what is held before storing it, so a change is a move and never a copy. That does not make an unchanged call free for every caller: a host that keeps its own array has to clone it to call at all, and then pays the clone whether or not anything differed. A host that hands its array over for good pays nothing when the numbers are the same, and a host that keeps one should ask `peak_count` before it clones — which is what this page does, on every frame it draws.

At two hundred peaks per second of audio, an hour is seven hundred thousand pairs, about six megabytes held, and about a millisecond to reduce. That millisecond is paid again on every frame of a live window resize, so a recording with several million peaks in it will make a window edge visibly lag the pointer while it is being dragged. A hundred to two hundred peaks per second is the sensible range; more than that is detail no lane narrower than the recording is long can draw.

## The traps

* **A lane has a fixed height and never asks for one.** There is no content to size to, so `height: Fit` on the waveform itself resolves to nothing and the lane is laid out with no body and never painted. The preset gives it 96 points; override the number, not the kind. A parent that sizes to fit is fine — it sizes to that number.
* **The peaks must cover exactly `0..peaks_span`, and nothing checks it.** A short array against the full duration draws the wrong audio under every mark, silently. Set `peaks_span` while a file is still decoding.
* **The times in a line are plain numbers**, in the unit `duration` is counted in. `\"1:30\"` reads as zero. There is no clock-time parser here for the same reason there is none in `Timeline`: a minute and a half to one caller is a bar and three beats to another.
* **A colour in a line is written `#rrggbb`, without the `x`.** The `x` in `#x4f9d69` is there to stop the Rust tokenizer reading the digits as a suffixed numeric literal — `1e2` is the clean example — and there is no tokenizer inside a string. Written with it the field parses as neither an intent word nor a colour, and the item stays neutral. The `#` on the other hand is required: without it `ace`, `decade` and `beaded` are runs of hex digits, and a field that read one of them as a colour would be answering a typo. Most lines should carry an intent word anyway.
* **A name may not contain a `|`.**
* **`set_regions` and `set_markers` drop the markup list.** They have to: leaving it would let the next draw take it back. A lane written in markup is driven in markup, and a lane driven from Rust should not be written in markup as well.
* **Setting the playhead reports nothing.** A host that moved it already knows where it put it, and a widget that answered back would put every transport into a loop.
* **The default `duration` is 1**, which makes every number a fraction of the whole. That is a real axis and a working widget, but it is not seconds, and a host that forgot to set the duration will get fractions back and read them as seconds.
* **Two marks closer than twice `marker_grab` have overlapping reaches.** A press in the band between them goes to whichever is nearer, and to the earlier one on a tie; a press on either chip still takes that chip, because nearest wins. What removes the target altogether is `chip_width`: below that the chips overlap and there is nothing separate to aim at. The host owns how close it puts them.
* **A name is measured against the next mark in TIME.** Crowding therefore drops the EARLIER name of a pair and leaves the later one drawn, however wide it is — its own room runs to the mark after it.

## What it deliberately does not do

It does not zoom and it does not scroll. The whole recording is across the lane, always. A view that zooms needs a viewport, a scroll position, a level-of-detail pyramid and a second set of rules about what a gesture means at each zoom, and every one of those is a decision this widget would have to make on the host's behalf.

It does not move a region by its middle, and it does not create or delete anything. A press inside a region seeks, because a region is a span with a name and moving one whole is an edit with a meaning the widget cannot know. Making and destroying regions and marks is the host's list and the host's undo.

It draws no grid and no ruler. A grid means a unit and a unit means formatting, and this widget has no opinion about either.

It does not sort the lists it is given, does not care whether regions overlap, and draws them in the order they arrive with the last one on top. Overlapping regions are a real thing — a verse inside a section — and a widget that refused them would be wrong more often than it was right. Where two edges land on the same pixel, the one drawn on top is the one a press takes.

And it never decodes, resamples, analyses or normalises. The peaks it draws are the peaks it was handed, which is what makes two lanes fed from the same source comparable, and what makes a lane fed from something that is not audio at all still worth looking at.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Playhead", target: "subject", kind: ControlKind::Number { prop: "playhead", min: 0., max: 90., step: 0.5, default: 12. } },
        Control { label: "Show playhead", target: "subject", kind: ControlKind::Bool { prop: "show_playhead", default: true } },
        Control { label: "Show names", target: "subject", kind: ControlKind::Bool { prop: "show_names", default: true } },
        Control { label: "Mark band", target: "subject", kind: ControlKind::Number { prop: "marker_strip", min: 0., max: 40., step: 1., default: 16. } },
        Control { label: "Peaks cover", target: "subject", kind: ControlKind::Number { prop: "peaks_span", min: 0., max: 90., step: 1., default: 0. } },
        Control { label: "Edge reach", target: "subject", kind: ControlKind::Number { prop: "edge_grab", min: 2., max: 24., step: 1., default: 6. } },
        Control { label: "Least region", target: "subject", kind: ControlKind::Number { prop: "min_span", min: 0., max: 20., step: 0.5, default: 0. } },
        Control { label: "Drag step", target: "subject", kind: ControlKind::Number { prop: "step", min: 0., max: 5., step: 0.25, default: 0. } },
        Control { label: "Peak height", target: "subject", kind: ControlKind::Number { prop: "draw_lane.envelope", min: 0.2, max: 1., step: 0.02, default: 0.88 } },
        Control { label: "Region wash", target: "subject", kind: ControlKind::Number { prop: "draw_region.wash", min: 0., max: 0.6, step: 0.02, default: 0.16 } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(waveform_actions),
}];
