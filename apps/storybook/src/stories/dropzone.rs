//! The upload story: a target that takes a dropped file, a list of what is
//! going, and a square that holds a picture.
use crate::makepad_widgets::dropzone::*;
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryUploadBase = #(StoryUpload::register_widget(vm))

    /** The transfer demo: a target, a list, and the buttons that move it. */
    mod.storybook.StoryUpload = set_type_default() do mod.storybook.StoryUploadBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2

        inbox := mod.widgets.Dropzone{
            width: Fill
            height: 80
            text: "Drop files to add them to the list"
            show_filter_hint: false
        }
        list := mod.widgets.FileList{width: Fill}
        View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            send := Button{text: "Send"}
            add := Button{text: "Add one"}
            reset := Button{text: "Start over"}
        }
        reported := Label{text: "nothing pressed yet"}
    }

    mod.stories.UploadOverview = StoryPage{
        StoryNote{text: "Three widgets for the same job. A target takes a file dropped on it and says beforehand whether it will; a list draws what is going and where each file has got to; a square holds a picture once there is one. None of them opens a file dialog and none of them sends anything: they report, and the host does the work."}

        StoryHeading{text: "A target for a dropped file"}
        StoryNote{text: "Drag a file from the desktop onto it. The border thickens and the face goes green while it would take what is over it, and red while it would not."}
        StoryRow{
            width: Fill
            subject := mod.widgets.Dropzone{
                width: Fill
                text: "Drop a file here"
                accept: ""
            }
        }
        StoryRow{
            dropped_note := Label{text: "nothing dropped yet"}
        }

        StoryHeading{text: "What it will take"}
        StoryNote{text: "`accept` is a list of extensions, family patterns like `image/*`, or empty for anything. It is a name test and nothing more — the file is never opened, so a `.png` that is really a zip gets in and catching that is yours. With `hint` empty the target writes the rule out under the prompt."}
        StoryRow{
            width: Fill
            mod.widgets.Dropzone{
                width: Fill
                height: 90
                text: "Pictures"
                accept: "image/*"
            }
            mod.widgets.Dropzone{
                width: Fill
                height: 90
                text: "One document"
                accept: "pdf, docx, txt, md"
                multiple: false
            }
        }

        StoryHeading{text: "A list of what is going"}
        StoryNote{text: "Press Send. The bars fill at their own rates, the big recording fails part way, and its row swaps its remove mark for a retry. The retry gets there — which is the point of having one."}
        demo := mod.storybook.StoryUpload{}

        StoryHeading{text: "A square that shows the picture"}
        StoryNote{text: "Empty, it draws a prompt and takes a drop like any other target. With a picture it shows it cropped to fill, and a mark in the corner empties it again."}
        StoryRow{
            well := mod.widgets.ImageWell{
                width: 140
                height: 140
                text: "Drop a picture"
            }
            mod.widgets.ImageWell{
                width: 140
                height: 140
                picture: mod.widgets.Image{
                    src: crate_resource("self:resources/photo_landscape.jpg")
                    fit: mod.widgets.ImageFit.CropToFill
                }
            }
            mod.widgets.ImageWell{
                width: 140
                height: 140
                text: "Off"
                disabled: true
            }
        }
        StoryRow{
            well_note := Label{text: "the well is empty"}
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            width: Fill
            mod.widgets.Dropzone{
                width: Fill
                height: 80
                text: "Off"
                disabled: true
            }
        }
    }
}

/// The rows the demo starts with. The big recording is the one that fails:
/// a list where everything succeeds never shows the half of it that matters.
const DEMO_FILES: &[(&str, u64)] = &[
    ("annual-report-final-v3.pdf", 2_480_000),
    ("logo.svg", 14_200),
    ("field-recording-2026-04-11.wav", 148_900_000),
    ("notes.md", 3_100),
];

/// Where the doomed row gives up.
const FAILS_AT: f64 = 0.62;

/// The demo's own transfer: it owns the frame clock, so the story file can
/// stay a page of records and one action handler.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryUpload {
    #[deref]
    view: View,
    #[rust]
    seeded: bool,
    #[rust]
    next_frame: NextFrame,
    /// The row that fails on its first attempt, and only its first.
    #[rust]
    doomed: Option<LiveId>,
    #[rust]
    added: usize,
}

impl StoryUpload {
    fn list(&self, cx: &Cx) -> FileListRef {
        self.view.file_list(cx, ids!(list))
    }

    fn seed(&mut self, cx: &mut Cx) {
        let list = self.list(cx);
        let entries: Vec<FileEntry> = DEMO_FILES
            .iter()
            .map(|(name, size)| FileEntry::new(*name, *size))
            .collect();
        self.doomed = entries.get(2).map(|e| e.id);
        list.set_entries(cx, entries);
    }

    /// Start everything that has not been sent, and everything that failed.
    pub fn start(&mut self, cx: &mut Cx) {
        let list = self.list(cx);
        for entry in list.entries() {
            if matches!(entry.state, TransferState::Waiting | TransferState::Failed) {
                list.set_note(cx, entry.id, "");
                list.set_state(cx, entry.id, TransferState::Sending, 0.0);
            }
        }
        self.next_frame = cx.new_next_frame();
    }

    pub fn retry(&mut self, cx: &mut Cx, id: LiveId) {
        // A retry that failed again would be honest and useless on a page
        // whose whole point is that the row offers one.
        if self.doomed == Some(id) {
            self.doomed = None;
        }
        let list = self.list(cx);
        list.set_note(cx, id, "");
        list.set_state(cx, id, TransferState::Sending, 0.0);
        self.next_frame = cx.new_next_frame();
    }

    pub fn remove(&mut self, cx: &mut Cx, id: LiveId) {
        let list = self.list(cx);
        list.remove(cx, id);
    }

    pub fn add(&mut self, cx: &mut Cx) {
        self.added += 1;
        let name = format!("scan-{:04}.tiff", self.added);
        let size = 400_000 + self.added as u64 * 317_000;
        let list = self.list(cx);
        list.push(cx, FileEntry::new(name, size));
    }

    /// Rows for files a target just took.
    pub fn take_files(&mut self, cx: &mut Cx, files: Vec<OfferedFile>) {
        let list = self.list(cx);
        for file in &files {
            list.push(cx, FileEntry::from_offered(file));
        }
    }

    pub fn start_over(&mut self, cx: &mut Cx) {
        self.added = 0;
        self.seed(cx);
    }

    /// One frame of the pretend transfer.
    fn tick(&mut self, cx: &mut Cx) {
        let list = self.list(cx);
        let doomed = self.doomed;
        let mut still_going = false;
        for (i, entry) in list.entries().iter().enumerate() {
            if entry.state != TransferState::Sending {
                continue;
            }
            // Different rates, because a list where every bar moves in step
            // looks like one bar drawn four times.
            let step = 0.010 + 0.004 * (i % 3) as f64;
            let next = entry.progress + step;
            if doomed == Some(entry.id) && next >= FAILS_AT {
                list.set_state(cx, entry.id, TransferState::Failed, FAILS_AT);
                list.set_note(cx, entry.id, "the far end hung up");
            } else if next >= 1.0 {
                list.set_state(cx, entry.id, TransferState::Done, 1.0);
            } else {
                list.set_state(cx, entry.id, TransferState::Sending, next);
                still_going = true;
            }
        }
        if still_going {
            self.next_frame = cx.new_next_frame();
        }
    }

    fn say(&self, cx: &mut Cx, text: &str) {
        let label = self.view.label(cx, ids!(reported));
        label.set_text(cx, text);
    }
}

impl Widget for StoryUpload {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        // Seeded after the first draw rather than before it: the list is
        // looked up by id, and there is nothing to look up until the view
        // has been walked once. The seed asks for a redraw, so the rows
        // appear on the frame after this one.
        if !self.seeded {
            let list = self.list(cx.cx.cx);
            let ready = list.borrow().is_some();
            if ready {
                self.seeded = true;
                self.seed(cx.cx.cx);
            }
        }
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if self.next_frame.is_event(event).is_some() {
            self.tick(cx);
        }
    }
}

/// One line naming what was dropped.
fn name_list(files: &[OfferedFile]) -> String {
    files
        .iter()
        .map(|f| format!("{} ({})", f.name, human_size(f.size)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn upload_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // Every lookup first. A `borrow_mut` on the demo is held while it works,
    // and walking the tree under it while that borrow stands would ask for
    // the same cell twice.
    let zone = root.dropzone(cx, ids!(subject));
    let dropped_note = root.label(cx, ids!(dropped_note));
    let well = root.image_well(cx, ids!(well));
    let well_note = root.label(cx, ids!(well_note));
    let demo = root.story_upload(cx, ids!(demo));
    let inbox = root.dropzone(cx, ids!(demo.inbox));
    let list = root.file_list(cx, ids!(demo.list));
    let send = root.button(cx, ids!(demo.send));
    let add = root.button(cx, ids!(demo.add));
    let reset = root.button(cx, ids!(demo.reset));

    if let Some(files) = zone.dropped(actions) {
        dropped_note.set_text(cx, &format!("took {}", name_list(&files)));
    }
    if let Some(files) = zone.refused(actions) {
        dropped_note.set_text(cx, &format!("turned away {}", name_list(&files)));
    }
    if zone.browse(actions) {
        dropped_note.set_text(cx, "pressed — a host would open its file picker here");
    }

    if let Some(file) = well.picked(actions) {
        well_note.set_text(cx, &format!("showing {}", file.name));
    }
    if let Some(file) = well.refused(actions) {
        well_note.set_text(cx, &format!("would not take {}", file.name));
    }
    if well.cleared(actions) {
        well_note.set_text(cx, "the well is empty");
    }
    if well.browse(actions) {
        well_note.set_text(cx, "pressed — a host would open its file picker here");
    }

    if send.clicked(actions) {
        if let Some(mut demo) = demo.borrow_mut() {
            demo.start(cx);
            demo.say(cx, "sending");
        }
    }
    if add.clicked(actions) {
        if let Some(mut demo) = demo.borrow_mut() {
            demo.add(cx);
            demo.say(cx, "one more row");
        }
    }
    if reset.clicked(actions) {
        if let Some(mut demo) = demo.borrow_mut() {
            demo.start_over(cx);
            demo.say(cx, "back to the start");
        }
    }
    if let Some(id) = list.retried(actions) {
        if let Some(mut demo) = demo.borrow_mut() {
            demo.retry(cx, id);
            demo.say(cx, "retrying");
        }
    }
    // The mark asks; the host removes. This is the host.
    if let Some(id) = list.removed(actions) {
        if let Some(mut demo) = demo.borrow_mut() {
            demo.remove(cx, id);
            demo.say(cx, "row removed");
        }
    }
    if let Some(id) = list.chose(actions) {
        if let Some(demo) = demo.borrow() {
            let name = demo
                .list(cx)
                .entries()
                .into_iter()
                .find(|e| e.id == id)
                .map(|e| e.name)
                .unwrap_or_default();
            demo.say(cx, &format!("chose {name}"));
        }
    }
    if let Some(files) = inbox.dropped(actions) {
        if let Some(mut demo) = demo.borrow_mut() {
            let count = files.len();
            demo.take_files(cx, files);
            demo.say(cx, &format!("added {count} from the drop"));
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/dropzone/overview",
    category: "Inputs",
    component: "Dropzone",
    also: &["FileList", "ImageWell"],
    name: "Overview",
    dsl: "UploadOverview",
    added: "2026-09-10",
    tags: &["new", "upload", "drop", "files", "transfer", "progress", "picture"],
    doc: "# Dropzone, FileList, ImageWell\n\nThree widgets for getting files out of a person's hands and into an application, and for saying what happened to them afterwards.\n\n## The platform does surface file drops\n\nIt arrives as its own event rather than a hit — `Event::Drag` while a drag is over the window, `Event::Drop` when it is let go — tested against an area with `event.drag_hits(cx, area)`. Each carries items in one of two shapes, and which one you get is the platform, not a setting: a **path** on the desktop, and a **name, media type and the bytes** in a browser, where there is no path to hand over. Both are flattened into one `OfferedFile`, so a host reads a drop the same way everywhere and looks at `path` or `bytes` depending on what it got.\n\nOne wrinkle is worth knowing: browser hover events carry empty placeholders and only the drop carries the names. A target that judged a drag on the names alone would refuse everything on the web and only find out at the drop, so an unnamed drag is let through and the drop itself is judged.\n\n## What they deliberately do not do\n\n**No file dialog.** A press reports `Browse` and stops. Opening the picker is a platform call with its own permission story, and a widget that opened one would be a widget you could not use without it.\n\n**No sending.** `FileList` draws a transfer; it does not run one. There is no socket, no queue and no retry timer — `Retry` is an action, and the host that owns the connection decides what it means.\n\n**No content check.** The filter reads the name and whatever media type the platform declared. It never opens the file.\n\n## Dropzone\n\n`accept` is a list of patterns split on commas and spaces: `png`, `.png` and `*.png` all mean the same extension; `image/*` is a family, matched against the declared type when there is one and against a built-in extension table when there is not; `*` or empty takes anything. With `hint` empty the target writes the rule out under its prompt.\n\nThe three looks are idle, accepting and refusing, and the border thickens as well as changing colour — on a box this large a colour swap alone is easy to miss, and the edge is where the eye already is while dragging. The pointer's own badge is set to match, since that is the only report visible outside the window.\n\nWith `multiple: false` a drop of several reports the first as `Dropped` and the rest as `Refused`, rather than swallowing them where nobody would see it happen.\n\n## FileList\n\nOne row per file: a state dot, the name, the size and state, a bar, and a trailing action. Rows are keyed by a `LiveId` the list hands out, and every action carries that handle rather than a row number — so removing a row cannot renumber the others out from under a reply still in flight.\n\n`set_state(cx, id, state, progress)` moves a row on. The bar says what the state says: a `Done` row reads full whatever number was last set, a `Waiting` row reads empty. `set_note` replaces the state word with the reason a row failed, which is worth more than the word \"failed\".\n\n**The remove mark does not remove the row.** It reports `Removed(id)` and the host calls `remove` once it has stopped the transfer, because a row that vanished before the send was cancelled would be a lie about what the machine is doing.\n\nIt draws every row it has and resolves its own `Fit` height from the count, rather than virtualising the way `PortalList` does: that shape is right for a million rows and wrong for the handful a person just dropped, and it would put a draw loop in every caller to save work there is none of. Put it in something that scrolls.\n\n## ImageWell\n\nA square that shows the picture once there is one, cropped to fill. The picture is a slot rather than a copy of every setting an image already has, so a caller can declare a `src` on it or change its fit. Whether there is a picture is asked of the image each pass rather than tracked — a `src` decodes asynchronously, and a flag would go stale.\n\n## Reading a name that is too long\n\nA name wider than its column is cut in the **middle**, not at the end: the tail of a file name is its extension, and that is the part that says what the file is.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Prompt", target: "subject", kind: ControlKind::Text { prop: "text", default: "Drop a file here" } },
        Control { label: "Accepts", target: "subject", kind: ControlKind::Text { prop: "accept", default: "" } },
        Control { label: "Second line", target: "subject", kind: ControlKind::Text { prop: "hint", default: "" } },
        Control { label: "More than one", target: "subject", kind: ControlKind::Bool { prop: "multiple", default: true } },
        Control { label: "Write the rule out", target: "subject", kind: ControlKind::Bool { prop: "show_filter_hint", default: true } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(upload_actions),
}];
