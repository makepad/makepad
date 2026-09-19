//! Upload — a target that takes a dropped file, a list of what is going, and
//! a square that holds a picture.
//!
//! # The platform does surface file drops
//!
//! It arrives as its own event, not as a hit: `Event::Drag` while a drag is
//! over the window and `Event::Drop` when it is let go, tested against an
//! area with `event.drag_hits(cx, area)`. Each carries `DragItem`s in one of
//! two shapes, and which one you get is the platform, not a setting:
//!
//! * `DragItem::FilePath` — a path on disk. Desktop.
//! * `DragItem::VirtualFile` — a name, a media type and the bytes. In the
//!   browser, where there is no path to hand over. Its hover events carry
//!   empty placeholders and only the drop carries the names, so a target
//!   that judged a drag on the names alone would refuse everything until it
//!   was too late; the filter here lets an unnamed drag through and judges
//!   the drop.
//!
//! Both shapes are flattened into one [`OfferedFile`] so a host reads a drop
//! the same way on every platform.
//!
//! # What these deliberately do not do
//!
//! **No file dialog.** A press on a target reports [`DropzoneAction::Browse`]
//! and stops. Opening the picker is a platform call with its own permission
//! story, and a widget that opened one would be a widget you could not use
//! without it.
//!
//! **No sending.** `FileList` draws a transfer; it does not run one. It has
//! no socket, no queue and no retry timer — `Retry` is an action, and the
//! host that owns the connection decides what that means.
//!
//! **No content check.** The filter reads the name and the media type the
//! platform declared. It never opens the file. A `.png` that is really a zip
//! gets through, and that is the host's job to catch, not a target's.
//!
//! # Why the list draws its own rows
//!
//! `PortalList` virtualises: it asks a host for the rows that are on screen,
//! one at a time, in a draw loop the host runs. That is the right shape for
//! a million rows and the wrong one for the handful a person just dropped —
//! it would put the loop in every caller to save work there is none of. This
//! list draws every row it has, resolves its own `Fit` height from the row
//! count, and leaves scrolling to whatever it is put inside.
use crate::{
    badge::measure,
    image::ImageWidgetRefExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    CxWidgetExt,
};
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DropzoneBase = #(Dropzone::register_widget(vm))
    mod.widgets.FileListBase = #(FileList::register_widget(vm))
    mod.widgets.ImageWellBase = #(ImageWell::register_widget(vm))

    set_type_default() do #(DrawDropzone::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawFileListBg::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawFileRow::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawImageWell::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawClearMark::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawAddMark::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawWellOverlay::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** A target that takes a file dropped on it, and says beforehand
     * whether it will. */
    mod.widgets.Dropzone = set_type_default() do mod.widgets.DropzoneBase{
        width: Fill
        height: 120
        margin: theme.mspace_1

        /** the line across the middle */
        text: "Drop a file here"
        /** a quieter second line; empty lets `accept` write itself out */
        hint: ""
        /** what it will take: extensions, a media family, or empty for anything */
        accept: ""
        /** take more than one at a time 0..1 step 1 */
        multiple: true
        /** write `accept` out under the prompt when `hint` is empty 0..1 step 1 */
        show_filter_hint: true
        /** greyed, and deaf to a drag 0..1 step 1 */
        disabled: false
        /** space between the two lines in pixels 0..16 step 1 */
        line_gap: 5.0

        color_text: theme.color_text
        color_hint: theme.color_text_meta
        color_text_disabled: theme.color_text_disabled

        // Every value below is plain, and every one has a field on the draw
        // struct behind it. A prop on a draw type that also carries Rust
        // instance fields must line up with the struct or the slots shift,
        // and a uniform overridden by a caller regenerates the value table
        // out from under them.
        draw_bg +: {
            accepting: 0.0
            refusing: 0.0
            hover: 0.0
            focus: 0.0
            disabled: 0.0

            /** border thickness in pixels 0..6 step 0.25 */
            border_size: 1.0
            /** how much thicker the border goes while a drag is over it 0..6 step 0.25 */
            border_lift: 1.5
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: theme.corner_radius

            color: theme.color_inset
            color_hover: theme.color_inset_hover
            color_accepting: theme.color_success_container
            color_refusing: theme.color_error_container
            color_disabled: theme.color_inset_disabled

            border_color: theme.color_bevel_inset_1
            border_color_focus: theme.color_bevel_focus
            border_color_accepting: theme.color_success
            border_color_refusing: theme.color_error
            border_color_disabled: theme.color_bevel_inset_1_disabled

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // The border thickens as well as changes colour: on a box
                // this large a colour swap alone is easy to miss, and the
                // edge is where the eye already is while dragging.
                let edge = self.border_size + (self.accepting + self.refusing) * self.border_lift
                sdf.box(
                    edge
                    edge
                    self.rect_size.x - edge * 2.
                    self.rect_size.y - edge * 2.
                    self.border_radius
                )
                let fill = self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_accepting, self.accepting)
                    .mix(self.color_refusing, self.refusing)
                    .mix(self.color_disabled, self.disabled)
                let stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_accepting, self.accepting)
                    .mix(self.border_color_refusing, self.refusing)
                    .mix(self.border_color_disabled, self.disabled)
                sdf.fill_keep(fill)
                sdf.stroke(stroke, edge)
                return sdf.result
            }
        }

        draw_text +: {
            text_style: theme.font_bold{font_size: theme.font_size_p line_spacing: 1.0}
        }
        draw_hint +: {
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
    }

    /** One row per file: a name, a size, a bar and where it has got to. */
    mod.widgets.FileList = set_type_default() do mod.widgets.FileListBase{
        width: Fill
        height: Fit

        /** height of one row in pixels 28..96 step 1 */
        row_height: 46.0
        /** clear space between rows in pixels 0..12 step 1 */
        row_gap: 2.0
        /** padding inside a row, left and right, in pixels 0..32 step 1 */
        row_inset: 12.0
        /** padding above the first row and below the last in pixels 0..24 step 1 */
        list_inset: 6.0
        /** thickness of the progress bar in pixels 2..12 step 1 */
        bar_height: 4.0
        /** room at the trailing edge for the retry or the remove mark 24..120 step 2 */
        action_width: 54.0
        /** room before it for the size and the state 40..220 step 5 */
        meta_width: 130.0
        /** radius of the state dot in pixels 2..10 step 0.5 */
        dot_radius: 4.0
        /** shown in place of the rows while the list is empty */
        empty_text: "No files yet"
        /** the word on a failed row's action */
        retry_text: "Retry"
        /** greyed, and deaf to a press 0..1 step 1 */
        disabled: false

        color_name: theme.color_text
        color_name_disabled: theme.color_text_disabled
        color_meta: theme.color_text_meta
        color_waiting: theme.color_text_meta
        color_sending: theme.color_primary
        color_done: theme.color_success
        color_failed: theme.color_error

        draw_bg +: {
            disabled: 0.0
            /** border thickness in pixels 0..4 step 0.25 */
            border_size: 1.0
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: theme.corner_radius
            color: theme.color_inset
            color_disabled: theme.color_inset_disabled
            border_color: theme.color_bevel_inset_1

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )
                sdf.fill_keep(self.color.mix(self.color_disabled, self.disabled))
                sdf.stroke(self.border_color, self.border_size)
                return sdf.result
            }
        }

        draw_row +: {
            hover: 0.0
            disabled: 0.0
            progress: 0.0
            dot_x: 16.0
            dot_r: 4.0
            bar_x: 24.0
            bar_y: 30.0
            bar_w: 100.0
            bar_h: 4.0
            /** corner rounding radius of the row 0..24 step 0.5 */
            border_radius: theme.corner_radius
            color: #00000000
            color_hover: theme.color_surface_container_high
            color_disabled: #00000000
            track_color: theme.color_surface_container_highest
            mark_color: theme.color_primary

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0., 0., self.rect_size.x, self.rect_size.y, self.border_radius)
                sdf.fill(
                    self.color
                        .mix(self.color_hover, self.hover)
                        .mix(self.color_disabled, self.disabled)
                )
                // The state dot. A disc rather than a glyph: no face in the
                // default chain can be relied on for a mark this small.
                sdf.circle(self.dot_x, self.rect_size.y * 0.5, self.dot_r)
                sdf.fill(self.mark_color)
                // The track, then the part of it that is done. The geometry
                // arrives from the same place the hit test reads it, so what
                // is drawn is what is pressed.
                sdf.box(self.bar_x, self.bar_y, self.bar_w, self.bar_h, self.bar_h * 0.5)
                sdf.fill(self.track_color)
                if self.progress > 0.0 {
                    // Never thinner than it is tall: a one percent send is a
                    // dot, not a smear.
                    let w = min(max(self.bar_w * self.progress, self.bar_h), self.bar_w)
                    sdf.box(self.bar_x, self.bar_y, w, self.bar_h, self.bar_h * 0.5)
                    sdf.fill(self.mark_color)
                }
                return sdf.result
            }
        }

        draw_name +: {
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
        draw_meta +: {
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
    }

    /** A square that shows the picture once there is one. */
    mod.widgets.ImageWell = set_type_default() do mod.widgets.ImageWellBase{
        width: 120
        height: 120
        margin: theme.mspace_1

        /** shown across the middle while the well is empty */
        text: "Drop a picture"
        /** what it will take */
        accept: "image/*"
        /** it would take a picture, said without a drag being over it 0..1 step 1 */
        accepting: false
        /** it would not take one, and says so 0..1 step 1 */
        refusing: false
        /** an engraved plus over the prompt while the well is empty 0..1 step 1 */
        add_mark: false
        /** the plus's box in pixels 8..80 step 1 */
        add_mark_size: 28.0
        /** clear space between the plus and the prompt under it 0..24 step 1 */
        add_mark_gap: 6.0
        /** drawn as chosen; the host decides which well that is 0..1 step 1 */
        selected: false
        /** how far a load into this well has got, 0..1; below zero draws no bar -1..1 step 0.01 */
        progress: -1.0
        /** seconds a `reject` flash lasts before the well goes quiet again 0..3 step 0.05 */
        reject_secs: 0.7
        /** a mark in the corner that empties the well 0..1 step 1 */
        clearable: true
        /** the corner mark's box in pixels 12..40 step 1 */
        clear_size: 20.0
        /** greyed, and deaf to a drag 0..1 step 1 */
        disabled: false

        color_text: theme.color_text_meta
        color_text_disabled: theme.color_text_disabled

        // The picture is a slot, not a property per setting the image
        // already has: a caller that wants a different fit, or a `src`
        // declared in the DSL, writes it on the image itself. It carries no
        // width or height because the well hands it its own rect every draw
        // — the picture IS the well's face, not something laid beside it.
        // Nor a corner: the ground's is handed over with the rect, so a
        // caller rounding the well rounds what is in it.
        picture: mod.widgets.Image{
            fit: mod.widgets.ImageFit.CropToFill
        }

        draw_bg +: {
            accepting: 0.0
            refusing: 0.0
            selected: 0.0
            hover: 0.0
            focus: 0.0
            disabled: 0.0

            /** border thickness in pixels 0..6 step 0.25 */
            border_size: 1.0
            /** how much thicker the border goes for a loud state 0..6 step 0.25 */
            border_lift: 1.5
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: theme.corner_radius

            color: theme.color_inset
            color_hover: theme.color_inset_hover
            color_selected: theme.color_secondary_container
            color_accepting: theme.color_success_container
            color_refusing: theme.color_error_container
            color_disabled: theme.color_inset_disabled

            border_color: theme.color_bevel_inset_1
            border_color_selected: theme.color_primary
            border_color_focus: theme.color_bevel_focus
            border_color_accepting: theme.color_success
            border_color_refusing: theme.color_error
            border_color_disabled: theme.color_bevel_inset_1_disabled

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // Chosen thickens the ring too. Over a picture the ring is
                // the only thing left to say it with, and a colour swap on
                // a hairline is easy to miss.
                let loud = self.accepting + self.refusing + self.selected
                let edge = self.border_size + loud * self.border_lift
                sdf.box(
                    edge
                    edge
                    self.rect_size.x - edge * 2.
                    self.rect_size.y - edge * 2.
                    self.border_radius
                )
                let fill = self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_selected, self.selected)
                    .mix(self.color_accepting, self.accepting)
                    .mix(self.color_refusing, self.refusing)
                    .mix(self.color_disabled, self.disabled)
                // Focus sits after chosen: where the keyboard is must stay
                // visible on a well that is also part of a selection.
                let stroke = self.border_color
                    .mix(self.border_color_selected, self.selected)
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_accepting, self.accepting)
                    .mix(self.border_color_refusing, self.refusing)
                    .mix(self.border_color_disabled, self.disabled)
                sdf.fill_keep(fill)
                sdf.stroke(stroke, edge)
                return sdf.result
            }
        }

        // The plus. Its own layer rather than part of the ground, because
        // the ground quad is pushed before its rect is resolved and the
        // mark has to be placed against that rect.
        draw_add +: {
            hover: 0.0
            disabled: 0.0
            /** the plus's stroke in pixels 1..8 step 0.5 */
            stroke: 2.0
            /** how far the lit copy sits below the ink; 0 draws it flat 0..4 step 0.5 */
            engrave: 1.0

            color: theme.color_text_meta
            color_hover: theme.color_text
            color_disabled: theme.color_text_disabled
            color_engrave: theme.color_bevel_inset_2

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // Boxes, not paths, the way the library's other empty-state
                // figures are drawn: two mirrored path segments in one
                // shader here once painted only the second, and a mark that
                // sometimes fails to appear is worse than a plainer one
                // that always does. Consecutive boxes union into the same
                // field, so one fill paints both arms and neither can go
                // missing without the other.
                let e = self.engrave
                let m = min(self.rect_size.x, self.rect_size.y) - e
                let mx = self.rect_size.x * 0.5
                let my = self.rect_size.y * 0.5 - e * 0.5
                let w = max(1.0, self.stroke)
                let ink = self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_disabled, self.disabled)
                // Drawn twice: a lit copy a touch lower, then the ink over
                // it. That is the whole of the engraving, and it is what
                // keeps the plus reading as cut INTO the face rather than
                // laid on top of it like a button.
                sdf.box(mx - m * 0.5, my - w * 0.5 + e, m, w, w * 0.5)
                sdf.box(mx - w * 0.5, my - m * 0.5 + e, w, m, w * 0.5)
                sdf.fill(self.color_engrave)
                sdf.box(mx - m * 0.5, my - w * 0.5, m, w, w * 0.5)
                sdf.box(mx - w * 0.5, my - m * 0.5, w, m, w * 0.5)
                sdf.fill(ink)
                return sdf.result
            }
        }

        // The chosen-or-armed ring and the progress bar, OVER the picture.
        // The ground is drawn before the picture and a picture cropped to
        // fill covers it whole, border and all, so a well that is chosen,
        // armed or loading could not say so once it had something in it.
        draw_over +: {
            selected: 0.0
            accepting: 0.0
            refusing: 0.0
            disabled: 0.0
            progress: -1.0

            /** ring thickness in pixels 0..6 step 0.25 */
            border_size: 1.0
            /** how much thicker the ring goes for a loud state 0..6 step 0.25 */
            border_lift: 1.5
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: theme.corner_radius
            /** thickness of the progress bar in pixels 2..12 step 1 */
            bar_height: 4.0
            /** clear space around the progress bar in pixels 0..32 step 1 */
            bar_inset: 8.0

            ring_color_selected: theme.color_primary
            ring_color_accepting: theme.color_success
            ring_color_refusing: theme.color_error
            track_color: theme.color_surface_container_highest
            track_color_disabled: theme.color_inset_disabled
            mark_color: theme.color_primary
            mark_color_disabled: theme.color_text_disabled

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let loud = self.selected + self.accepting + self.refusing
                if loud > 0.001 {
                    // The same ring the ground draws, at the same width, so
                    // the two cannot disagree about where the edge is.
                    let edge = self.border_size + loud * self.border_lift
                    let ink = self.ring_color_selected
                        .mix(self.ring_color_accepting, self.accepting)
                        .mix(self.ring_color_refusing, self.refusing)
                    sdf.box(
                        edge
                        edge
                        self.rect_size.x - edge * 2.
                        self.rect_size.y - edge * 2.
                        self.border_radius
                    )
                    sdf.stroke(vec4(ink.xyz, ink.w * min(loud, 1.0)), edge)
                }
                if self.progress >= 0.0 {
                    let h = self.bar_height
                    let x = self.bar_inset
                    let w = self.rect_size.x - self.bar_inset * 2.
                    let y = self.rect_size.y - self.bar_inset - h
                    sdf.box(x, y, w, h, h * 0.5)
                    sdf.fill(self.track_color.mix(self.track_color_disabled, self.disabled))
                    // Never thinner than it is tall: a one percent load is a
                    // dot, not a smear.
                    let f = min(max(w * self.progress, h), w)
                    sdf.box(x, y, f, h, h * 0.5)
                    sdf.fill(self.mark_color.mix(self.mark_color_disabled, self.disabled))
                }
                return sdf.result
            }
        }

        // The corner mark sits over the picture, so it is a layer of its own
        // rather than part of the ground: the ground is drawn first and the
        // picture covers it.
        draw_clear +: {
            hot: 0.0
            /** radius of the corner mark's disc in pixels 6..20 step 0.5 */
            radius: 10.0
            color: theme.color_scrim
            color_hot: theme.color_error

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.radius)
                sdf.fill(self.color.mix(self.color_hot, self.hot))
                return sdf.result
            }
        }

        draw_text +: {
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
        draw_mark +: {
            color: theme.color_white
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
    }
}

/// `draw_abs` takes the top of the LINE box, not the top of the ink. The ink
/// starts about this share of the font size below it, so a line placed by the
/// band it should sit in has to be lifted by that much or it rides low.
const INK_DROP: f64 = 0.30;

/// The y `draw_abs` wants so one line of `font_size` sits centred in a band
/// of `height` whose top is `top`.
fn line_y(top: f64, height: f64, font_size: f64) -> f64 {
    top + (height - font_size) * 0.5 - font_size * INK_DROP
}

/// Characters kept from the end of an elided name.
const TAIL_CHARS: usize = 6;

/// The height of the band a file name is drawn in, inside a row.
const NAME_BAND: f64 = 18.0;
/// Clear space between that band and the bar under it.
const BAR_GAP: f64 = 5.0;

// ---------------------------------------------------------------------------
// What arrives, and what is let in
// ---------------------------------------------------------------------------

/// One file offered to a target, whatever shape the platform handed it over
/// in. `path` is empty in a browser and `bytes` is `None` on a desktop; a
/// host that wants to work on both reads whichever it got.
#[derive(Clone, Debug, PartialEq)]
pub struct OfferedFile {
    /// The name with no directory part.
    pub name: String,
    /// Where it is on disk, or empty.
    pub path: String,
    /// The media type the platform declared, or empty.
    pub mime: String,
    /// Bytes, in bytes. Zero when the platform said nothing and the file
    /// could not be looked at.
    pub size: u64,
    /// The contents, when they came with the drop rather than a path.
    pub bytes: Option<Arc<[u8]>>,
}

impl OfferedFile {
    /// A file the host is naming itself, rather than one that was dropped.
    pub fn named(name: impl Into<String>, size: u64) -> Self {
        Self {
            name: name.into(),
            path: String::new(),
            mime: String::new(),
            size,
            bytes: None,
        }
    }
}

/// The name at the end of a path, with either separator.
///
/// Not `Path::file_name`: that splits on the separator of the platform doing
/// the splitting, and a path can arrive from somewhere else.
fn base_name(path: &str) -> &str {
    match path.rfind(|c: char| c == '/' || c == '\\') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// The lowercased text after the last dot, or empty. A leading dot is a
/// hidden file, not an extension.
fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// The family an extension belongs to, for a `image/*` style pattern on a
/// platform that declared no media type.
fn family_of(ext: &str) -> Option<&'static str> {
    const IMAGE: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "avif", "heic", "tif", "tiff", "ico",
        "qoi",
    ];
    const AUDIO: &[&str] = &[
        "wav", "mp3", "flac", "ogg", "oga", "opus", "aac", "m4a", "aiff", "aif",
    ];
    const VIDEO: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "m4v"];
    const TEXT: &[&str] = &[
        "txt", "md", "csv", "tsv", "json", "xml", "yaml", "yml", "toml", "log", "rs",
    ];
    if IMAGE.contains(&ext) {
        Some("image")
    } else if AUDIO.contains(&ext) {
        Some("audio")
    } else if VIDEO.contains(&ext) {
        Some("video")
    } else if TEXT.contains(&ext) {
        Some("text")
    } else {
        None
    }
}

/// Which files a target will take.
///
/// A NAME test and nothing more. It reads the extension and whatever media
/// type the platform declared, and never opens the file: a `.png` that is
/// really a zip gets in, and catching that is the host's job.
///
/// A pattern is one of `png`, `.png`, `*.png` (all the same extension),
/// `image/*` (a family), `image/png` (a declared type, read as the extension
/// `png` when the platform declared none), or `*` for anything. An empty
/// specification takes anything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileFilter {
    patterns: Vec<String>,
}

impl FileFilter {
    /// Split a specification on commas and whitespace.
    pub fn parse(spec: &str) -> Self {
        Self {
            patterns: spec
                .split(|c: char| c == ',' || c.is_whitespace())
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_lowercase)
                .collect(),
        }
    }

    /// Nothing was named, so nothing is turned away.
    pub fn takes_anything(&self) -> bool {
        self.patterns.is_empty() || self.patterns.iter().any(|p| p == "*" || p == "*/*")
    }

    /// Whether a file with this name and declared type is let in.
    pub fn accepts(&self, name: &str, mime: &str) -> bool {
        if self.takes_anything() {
            return true;
        }
        // A drag the platform has not named yet is let through. The browser
        // reports a count and a type and no names until the drop lands, and
        // a target that refused on no evidence would read as broken to the
        // one person who could not tell why.
        if name.is_empty() && mime.is_empty() {
            return true;
        }
        let ext = extension(name);
        let mime = mime.to_lowercase();
        self.patterns
            .iter()
            .any(|pattern| matches_pattern(pattern, &ext, &mime))
    }

    /// Whether this file is let in.
    pub fn takes(&self, file: &OfferedFile) -> bool {
        self.accepts(&file.name, &file.mime)
    }

    /// The filter as a line a person can read, for a target that writes out
    /// what it will take.
    pub fn describe(&self) -> String {
        if self.takes_anything() {
            return "any file".to_string();
        }
        let mut parts: Vec<String> = Vec::new();
        for pattern in &self.patterns {
            let part = match pattern.split_once('/') {
                Some((family, "*")) => format!("any {family}"),
                Some((_, sub)) => sub.to_string(),
                None => pattern
                    .trim_start_matches('*')
                    .trim_start_matches('.')
                    .to_string(),
            };
            if !part.is_empty() && !parts.contains(&part) {
                parts.push(part);
            }
        }
        parts.join(", ")
    }
}

fn matches_pattern(pattern: &str, ext: &str, mime: &str) -> bool {
    if let Some((family, sub)) = pattern.split_once('/') {
        if sub == "*" {
            if !mime.is_empty() {
                return mime.split('/').next() == Some(family);
            }
            return !ext.is_empty() && family_of(ext) == Some(family);
        }
        if !mime.is_empty() {
            return mime == pattern;
        }
        // Nothing was declared, so the subtype is read as an extension:
        // "image/png" and "png" then mean the same thing.
        return !ext.is_empty() && ext == sub;
    }
    let want = pattern.trim_start_matches('*').trim_start_matches('.');
    !want.is_empty() && want == ext
}

/// A byte count as a person reads it.
///
/// Decimal units, because that is what storage and transfer are quoted in and
/// a progress line that disagrees with the file manager beside it starts an
/// argument nobody wins. One decimal below ten, none above, so the number
/// stops shuffling width while it counts up.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["kB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.5 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if value < 9.95 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// Fit `s` into `room` by cutting the middle out rather than the end: the
/// tail of a file name is its extension, and that is the part that says what
/// the file is. `draw_abs` takes no width to clip against, so the cut has to
/// be made in the string before it is drawn.
fn elide_middle(s: &str, room: f64, width_of: &mut dyn FnMut(&str) -> f64) -> String {
    if room <= 0.0 {
        return String::new();
    }
    if width_of(s) <= room {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    // Shrink the tail first. Without this a narrow column returned a mark
    // and a tail that were together wider than the room they were cut for.
    let mut tail_len = TAIL_CHARS.min(chars.len().saturating_sub(1));
    while tail_len > 0 {
        let probe: String = std::iter::once('\u{2026}')
            .chain(chars[chars.len() - tail_len..].iter().copied())
            .collect();
        if width_of(&probe) <= room {
            break;
        }
        tail_len -= 1;
    }
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    let mut head = String::new();
    for c in chars[..chars.len() - tail_len].iter() {
        let probe = format!("{head}{c}\u{2026}{tail}");
        if width_of(&probe) > room {
            break;
        }
        head.push(*c);
    }
    format!("{head}\u{2026}{tail}")
}

/// Flatten the platform's drag items into files. A dragged string is not a
/// file and is dropped here rather than handed on as one with no bytes.
fn offered_files(items: &[DragItem]) -> Vec<OfferedFile> {
    let mut out = Vec::new();
    for item in items {
        match item {
            DragItem::FilePath { path, .. } => {
                // The one time any of these widgets touches the filesystem.
                // A list that says "0 B" beside every row is not a list.
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                out.push(OfferedFile {
                    name: base_name(path).to_string(),
                    path: path.clone(),
                    mime: String::new(),
                    size,
                    bytes: None,
                });
            }
            DragItem::VirtualFile(file) => out.push(OfferedFile {
                name: file.name.clone(),
                path: String::new(),
                mime: file.mime.clone(),
                size: file.size,
                bytes: Some(file.bytes.clone()),
            }),
            DragItem::String { .. } => {}
        }
    }
    out
}

/// What a target is showing while a drag is over it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum Look {
    #[default]
    Idle,
    Accepting,
    Refusing,
}

/// What a well shows: the drag while one is over it, and whatever the host
/// asked for the rest of the time.
///
/// Two looks rather than one because they answer to different people. A drag
/// is the platform talking and lasts exactly as long as the pointer is over
/// the target. A host-set look outlives it — a well armed because a browse
/// button was pressed, or refusing because the server turned the picture
/// away — and one field would lose it, since a drag anywhere else in the
/// window reports `NoHit` here and would wipe it back to idle.
fn resolve_look(drag: Look, host: Look) -> Look {
    if drag == Look::Idle {
        host
    } else {
        drag
    }
}

/// The look a host asked for, from its two flags. Refusing wins over
/// accepting: it is the louder answer and the safer one to be wrong with.
fn host_look(accepting: bool, refusing: bool) -> Look {
    if refusing {
        Look::Refusing
    } else if accepting {
        Look::Accepting
    } else {
        Look::Idle
    }
}

/// Where the plus and the prompt go in an empty well: the plus's box, and
/// the band the one line of prompt is centred in.
///
/// With no plus the band is the whole well, which is exactly where the
/// prompt has always been drawn — turning the mark on is the only thing that
/// moves anything. With one, the pair is treated as a single stack and
/// centred together, so the plus lifts the words rather than landing on top
/// of them.
fn empty_face(rect: Rect, mark: Option<f64>, line: f64, gap: f64) -> (Option<Rect>, Rect) {
    let Some(size) = mark else {
        return (None, rect);
    };
    let line = line.max(0.0);
    // No words, no gap, or the plus sits off centre with nothing under it.
    let gap = if line > 0.0 { gap.max(0.0) } else { 0.0 };
    let top = rect.pos.y + (rect.size.y - (size + gap + line)) * 0.5;
    (
        Some(Rect {
            pos: dvec2(rect.pos.x + (rect.size.x - size) * 0.5, top),
            size: dvec2(size, size),
        }),
        Rect {
            pos: dvec2(rect.pos.x, top + size + gap),
            size: dvec2(rect.size.x, line),
        },
    )
}

// ---------------------------------------------------------------------------
// Dropzone
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDropzone {
    #[deref]
    draw_super: DrawQuad,
    /// 1 while a drag the filter would take is over the target.
    #[live]
    accepting: f32,
    /// 1 while a drag it would not take is over it.
    #[live]
    refusing: f32,
    #[live]
    hover: f32,
    #[live]
    focus: f32,
    #[live]
    disabled: f32,
    #[live]
    border_size: f32,
    #[live]
    border_lift: f32,
    #[live]
    border_radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_accepting: Vec4f,
    #[live]
    color_refusing: Vec4f,
    #[live]
    color_disabled: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_color_focus: Vec4f,
    #[live]
    border_color_accepting: Vec4f,
    #[live]
    border_color_refusing: Vec4f,
    #[live]
    border_color_disabled: Vec4f,
}

#[derive(Clone, Debug, Default)]
pub enum DropzoneAction {
    /// Files the target took. Never empty.
    Dropped(Vec<OfferedFile>),
    /// Files it would not take: the filter turned them away, or `multiple`
    /// is off and these came after the first.
    Refused(Vec<OfferedFile>),
    /// The target was pressed, or entered from the keyboard. It opens no
    /// file dialog of its own — that call belongs to the host.
    Browse,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Dropzone {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    pub draw_bg: DrawDropzone,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_hint: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    /// The line across the middle.
    #[live]
    pub text: String,
    /// A quieter second line. Empty lets `accept` write itself out.
    #[live]
    pub hint: String,
    /// What it will take. See [`FileFilter`].
    #[live]
    pub accept: String,
    /// Take more than one at a time. With this off, a drop of several
    /// reports the first as taken and the rest as refused, rather than
    /// swallowing them where nobody would see it happen.
    #[live(true)]
    pub multiple: bool,
    #[live(true)]
    pub show_filter_hint: bool,
    #[live]
    pub disabled: bool,
    #[live(5.0)]
    line_gap: f64,

    #[live]
    color_text: Vec4f,
    #[live]
    color_hint: Vec4f,
    #[live]
    color_text_disabled: Vec4f,

    #[rust]
    look: Look,
    #[rust]
    hovered: bool,
}

impl Dropzone {
    /// The filter as it stands, built fresh: `accept` is a live property and
    /// the tweaker may have rewritten it since the last drag.
    pub fn filter(&self) -> FileFilter {
        FileFilter::parse(&self.accept)
    }

    fn hint_line(&self) -> String {
        if !self.hint.is_empty() {
            return self.hint.clone();
        }
        if !self.show_filter_hint {
            return String::new();
        }
        self.filter().describe()
    }

    fn set_look(&mut self, cx: &mut Cx, look: Look) {
        if self.look != look {
            self.look = look;
            self.draw_bg.redraw(cx);
        }
    }

    fn handle_drag(&mut self, cx: &mut Cx, event: &Event) {
        let uid = self.uid;
        match event.drag_hits(cx, self.draw_bg.area()) {
            DragHit::Drag(drag) => {
                let files = offered_files(&drag.items);
                let filter = self.filter();
                let look = if drag.state == DragState::Out || files.is_empty() {
                    Look::Idle
                } else if files.iter().any(|f| filter.takes(f)) {
                    Look::Accepting
                } else {
                    Look::Refusing
                };
                // Say no as well as yes: the pointer's own badge is the only
                // thing outside the window that reports either.
                if let Ok(mut response) = drag.response.lock() {
                    *response = if look == Look::Accepting {
                        DragResponse::Copy
                    } else {
                        DragResponse::None
                    };
                }
                self.set_look(cx, look);
            }
            DragHit::Drop(landed) => {
                self.set_look(cx, Look::Idle);
                let filter = self.filter();
                let (mut taken, mut refused): (Vec<_>, Vec<_>) = offered_files(&landed.items)
                    .into_iter()
                    .partition(|f| filter.takes(f));
                if !self.multiple && taken.len() > 1 {
                    refused.extend(taken.drain(1..));
                }
                if !refused.is_empty() {
                    cx.widget_action(uid, DropzoneAction::Refused(refused));
                }
                if !taken.is_empty() {
                    cx.widget_action(uid, DropzoneAction::Dropped(taken));
                }
            }
            DragHit::DragEnd | DragHit::NoHit => self.set_look(cx, Look::Idle),
        }
    }
}

impl Widget for Dropzone {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.look = Look::Idle;
            self.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.disabled {
            return;
        }
        // A drag is its own event, not a hit, and has to be tested first: it
        // reaches `hits` as nothing at all.
        if matches!(event, Event::Drag(_) | Event::Drop(_) | Event::DragEnd) {
            self.handle_drag(cx, event);
            return;
        }
        let uid = self.uid;
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.hovered = true;
                cx.set_cursor(MouseCursor::Hand);
                self.draw_bg.redraw(cx);
            }
            // The cursor is per-pass, not per-entry: setting it only on the
            // way in leaves it to whatever asked last while the pointer is
            // still sitting on the target.
            Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                if fe.is_over {
                    cx.widget_action(uid, DropzoneAction::Browse);
                }
                self.draw_bg.redraw(cx);
            }
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => {
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                if matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::Space) {
                    cx.widget_action(uid, DropzoneAction::Browse);
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let focused = cx.cx.cx.has_key_focus(self.draw_bg.area());
        self.draw_bg.accepting = if self.look == Look::Accepting { 1.0 } else { 0.0 };
        self.draw_bg.refusing = if self.look == Look::Refusing { 1.0 } else { 0.0 };
        self.draw_bg.hover = if self.hovered { 1.0 } else { 0.0 };
        self.draw_bg.focus = if focused { 1.0 } else { 0.0 };
        self.draw_bg.disabled = if self.disabled { 1.0 } else { 0.0 };

        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();

        let hint = self.hint_line();
        let title_size = self.draw_text.text_style.font_size as f64;
        let hint_size = self.draw_hint.text_style.font_size as f64;
        let block = if hint.is_empty() {
            title_size
        } else {
            title_size + self.line_gap + hint_size
        };
        let mut top = rect.pos.y + (rect.size.y - block) * 0.5;

        if !self.text.is_empty() {
            let text = self.text.clone();
            let width = measure(&self.draw_text, cx, &text);
            self.draw_text.color = if self.disabled {
                self.color_text_disabled
            } else {
                self.color_text
            };
            self.draw_text.draw_abs(
                cx,
                dvec2(
                    rect.pos.x + (rect.size.x - width) * 0.5,
                    line_y(top, title_size, title_size),
                ),
                &text,
            );
            top += title_size + self.line_gap;
        }
        if !hint.is_empty() {
            let width = measure(&self.draw_hint, cx, &hint);
            self.draw_hint.color = if self.disabled {
                self.color_text_disabled
            } else {
                self.color_hint
            };
            self.draw_hint.draw_abs(
                cx,
                dvec2(
                    rect.pos.x + (rect.size.x - width) * 0.5,
                    line_y(top, hint_size, hint_size),
                ),
                &hint,
            );
        }
        self.draw_bg.end(cx);

        if !self.disabled {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.clone()
    }
}

impl DropzoneRef {
    /// The files a drop landed, if this pass carried one.
    pub fn dropped(&self, actions: &Actions) -> Option<Vec<OfferedFile>> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            DropzoneAction::Dropped(files) => Some(files),
            _ => None,
        }
    }

    /// The files it turned away.
    pub fn refused(&self, actions: &Actions) -> Option<Vec<OfferedFile>> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            DropzoneAction::Refused(files) => Some(files),
            _ => None,
        }
    }

    /// The target was pressed. Open the platform's picker here, if the host
    /// has one; the target will not.
    pub fn browse(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), DropzoneAction::Browse))
            .unwrap_or(false)
    }

    /// What this target will take, for a host that wants to apply the same
    /// rule to files it got some other way.
    pub fn filter(&self) -> FileFilter {
        self.borrow().map(|inner| inner.filter()).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// FileList
// ---------------------------------------------------------------------------

/// Where one file has got to.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TransferState {
    /// Queued. Nothing has been sent.
    #[default]
    Waiting,
    /// On its way; `progress` is the share done.
    Sending,
    /// Sent, and the far end said so.
    Done,
    /// It did not arrive. The row offers a retry.
    Failed,
}

impl TransferState {
    /// The word printed beside the size.
    pub fn word(self) -> &'static str {
        match self {
            TransferState::Waiting => "waiting",
            TransferState::Sending => "sending",
            TransferState::Done => "done",
            TransferState::Failed => "failed",
        }
    }
}

/// One row.
#[derive(Clone, Debug)]
pub struct FileEntry {
    /// The host's handle. Actions carry it rather than a row number, so
    /// removing a row cannot renumber the others out from under a reply
    /// that is still in flight.
    pub id: LiveId,
    pub name: String,
    pub size: u64,
    /// 0..1.
    pub progress: f64,
    pub state: TransferState,
    /// Shown in place of the state word when it is not empty — the reason a
    /// row failed is worth more than the word "failed".
    pub note: String,
}

impl FileEntry {
    /// A waiting row with an id of its own.
    pub fn new(name: impl Into<String>, size: u64) -> Self {
        Self {
            id: LiveId::unique(),
            name: name.into(),
            size,
            progress: 0.0,
            state: TransferState::Waiting,
            note: String::new(),
        }
    }

    /// A waiting row for a file that was just dropped.
    pub fn from_offered(file: &OfferedFile) -> Self {
        Self::new(file.name.clone(), file.size)
    }

    /// How much of the bar is filled. A row that is done reads full whatever
    /// number the host last set, and one that never started reads empty:
    /// the bar says what the state says, or the two disagree on screen.
    pub fn shown_progress(&self) -> f64 {
        match self.state {
            TransferState::Waiting => 0.0,
            TransferState::Done => 1.0,
            _ => self.progress.clamp(0.0, 1.0),
        }
    }

    /// The line drawn on the right of the row.
    pub fn meta_line(&self) -> String {
        if self.note.is_empty() {
            format!("{} \u{00b7} {}", human_size(self.size), self.state.word())
        } else {
            format!("{} \u{00b7} {}", human_size(self.size), self.note)
        }
    }
}

/// Where the pieces of a row sit, and what a press lands on.
///
/// A type of its own for the same two reasons the range slider keeps one: it
/// can be tested without a script heap, and the hit test and the draw are
/// handed the same numbers from the same place.
#[derive(Copy, Clone, Debug, PartialEq)]
struct RowGeom {
    row_height: f64,
    row_gap: f64,
    row_inset: f64,
    list_inset: f64,
    bar_height: f64,
    action_width: f64,
    meta_width: f64,
    dot_radius: f64,
}

impl RowGeom {
    /// The height a list of `count` rows asks for. An empty list still asks
    /// for one row: it has a line to say it is empty, and a widget that
    /// resolved to nothing would leave a hole where the list was.
    fn height(&self, count: usize) -> f64 {
        let rows = count.max(1) as f64;
        self.list_inset * 2.0 + rows * self.row_height + (rows - 1.0) * self.row_gap
    }

    /// The top of row `i`, from the top of the list.
    fn row_top(&self, i: usize) -> f64 {
        self.list_inset + i as f64 * (self.row_height + self.row_gap)
    }

    /// Which row a y lands in. The gap between two rows belongs to neither.
    fn row_at(&self, y: f64, count: usize) -> Option<usize> {
        if count == 0 || y < self.list_inset {
            return None;
        }
        let pitch = self.row_height + self.row_gap;
        let i = ((y - self.list_inset) / pitch) as usize;
        if i >= count || y - self.row_top(i) > self.row_height {
            return None;
        }
        Some(i)
    }

    /// Whether an x in a row of `width` is on the trailing action.
    fn on_action(&self, x: f64, width: f64) -> bool {
        x >= width - self.row_inset - self.action_width && x <= width - self.row_inset
    }

    /// The centre of the trailing action's column.
    fn action_center(&self, width: f64) -> f64 {
        width - self.row_inset - self.action_width * 0.5
    }

    fn dot_x(&self) -> f64 {
        self.row_inset + self.dot_radius
    }

    fn name_x(&self) -> f64 {
        self.row_inset + self.dot_radius * 2.0 + 8.0
    }

    /// The right edge of the size-and-state column.
    fn meta_right(&self, width: f64) -> f64 {
        width - self.row_inset - self.action_width - 8.0
    }

    /// The left edge of that column, which is where the name has to stop.
    fn meta_left(&self, width: f64) -> f64 {
        (self.meta_right(width) - self.meta_width).max(self.name_x())
    }

    /// The bar: left edge and width, in the row's own frame.
    fn bar(&self, width: f64) -> (f64, f64) {
        let x = self.name_x();
        (x, (self.meta_right(width) - x).max(8.0))
    }

    /// The top of the name's band inside a row. The name, the gap and the
    /// bar are centred as one block, so a taller row grows evenly rather
    /// than leaving the bar stranded near the bottom.
    fn content_top(&self) -> f64 {
        ((self.row_height - (NAME_BAND + BAR_GAP + self.bar_height)) * 0.5).max(0.0)
    }

    fn bar_y(&self) -> f64 {
        self.content_top() + NAME_BAND + BAR_GAP
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFileListBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    disabled: f32,
    #[live]
    border_size: f32,
    #[live]
    border_radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_disabled: Vec4f,
    #[live]
    border_color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFileRow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    disabled: f32,
    /// The share of the bar that is filled, 0..1.
    #[live]
    progress: f32,
    /// The geometry the hit test uses, handed over every draw so the two
    /// cannot drift apart.
    #[live]
    dot_x: f32,
    #[live]
    dot_r: f32,
    #[live]
    bar_x: f32,
    #[live]
    bar_y: f32,
    #[live]
    bar_w: f32,
    #[live]
    bar_h: f32,
    #[live]
    border_radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_disabled: Vec4f,
    #[live]
    track_color: Vec4f,
    /// The dot and the filled part of the bar: this row's state, in colour.
    #[live]
    mark_color: Vec4f,
}

#[derive(Clone, Debug, Default)]
pub enum FileListAction {
    /// The retry word on a failed row was pressed.
    Retry(LiveId),
    /// The remove mark was pressed. The row is still there: see
    /// [`FileList::remove`].
    Removed(LiveId),
    /// A row was pressed somewhere other than its trailing action.
    Chose(LiveId),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FileList {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    pub draw_bg: DrawFileListBg,
    #[live]
    draw_row: DrawFileRow,
    #[live]
    draw_name: DrawText,
    #[live]
    draw_meta: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live(46.0)]
    row_height: f64,
    #[live(2.0)]
    row_gap: f64,
    #[live(12.0)]
    row_inset: f64,
    #[live(6.0)]
    list_inset: f64,
    #[live(4.0)]
    bar_height: f64,
    #[live(54.0)]
    action_width: f64,
    #[live(130.0)]
    meta_width: f64,
    #[live(4.0)]
    dot_radius: f64,

    /// Shown in place of the rows while there are none.
    #[live]
    pub empty_text: String,
    /// The word on a failed row's action.
    #[live]
    pub retry_text: String,
    #[live]
    pub disabled: bool,

    #[live]
    color_name: Vec4f,
    #[live]
    color_name_disabled: Vec4f,
    #[live]
    color_meta: Vec4f,
    #[live]
    color_waiting: Vec4f,
    #[live]
    color_sending: Vec4f,
    #[live]
    color_done: Vec4f,
    #[live]
    color_failed: Vec4f,

    #[rust]
    entries: Vec<FileEntry>,
    #[rust]
    hot_row: Option<usize>,
    #[rust]
    hot_action: bool,
}

impl FileList {
    /// The geometry as it stands, built fresh each draw: every number in it
    /// is a live property and the tweaker may have moved any of them.
    fn geom(&self) -> RowGeom {
        RowGeom {
            row_height: self.row_height,
            row_gap: self.row_gap,
            row_inset: self.row_inset,
            list_inset: self.list_inset,
            bar_height: self.bar_height,
            action_width: self.action_width,
            meta_width: self.meta_width,
            dot_radius: self.dot_radius,
        }
    }

    fn state_color(&self, state: TransferState) -> Vec4f {
        match state {
            TransferState::Waiting => self.color_waiting,
            TransferState::Sending => self.color_sending,
            TransferState::Done => self.color_done,
            TransferState::Failed => self.color_failed,
        }
    }

    pub fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<FileEntry>) {
        self.entries = entries;
        self.hot_row = None;
        self.draw_bg.redraw(cx);
    }

    /// Add a row and return its handle.
    pub fn push(&mut self, cx: &mut Cx, entry: FileEntry) -> LiveId {
        let id = entry.id;
        self.entries.push(entry);
        self.draw_bg.redraw(cx);
        id
    }

    /// Take a row out. The remove mark does NOT do this: it reports
    /// [`FileListAction::Removed`] and the host calls this once it has
    /// stopped the transfer, because a row that vanished before the send was
    /// cancelled would be a lie about what the machine is doing.
    pub fn remove(&mut self, cx: &mut Cx, id: LiveId) {
        self.entries.retain(|e| e.id != id);
        self.hot_row = None;
        self.draw_bg.redraw(cx);
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.entries.clear();
        self.hot_row = None;
        self.draw_bg.redraw(cx);
    }

    /// Move a row on. `progress` is read only while the state is `Sending`.
    pub fn set_state(&mut self, cx: &mut Cx, id: LiveId, state: TransferState, progress: f64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.state = state;
            entry.progress = progress.clamp(0.0, 1.0);
            self.draw_bg.redraw(cx);
        }
    }

    /// Say why a row failed, in place of the word "failed".
    pub fn set_note(&mut self, cx: &mut Cx, id: LiveId, note: impl Into<String>) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.note = note.into();
            self.draw_bg.redraw(cx);
        }
    }

    /// The handle of the row at `index`, for a caller walking the list.
    pub fn id_at(&self, index: usize) -> Option<LiveId> {
        self.entries.get(index).map(|e| e.id)
    }
}

impl Widget for FileList {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.hot_row = None;
            self.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.disabled {
            return;
        }
        let uid = self.uid;
        let geom = self.geom();
        let count = self.entries.len();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverOut(_) => {
                if self.hot_row.is_some() {
                    self.hot_row = None;
                    self.hot_action = false;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let x = fe.abs.x - fe.rect.pos.x;
                let y = fe.abs.y - fe.rect.pos.y;
                let row = geom.row_at(y, count);
                let action = row.is_some() && geom.on_action(x, fe.rect.size.x);
                if (self.hot_row, self.hot_action) != (row, action) {
                    self.hot_row = row;
                    self.hot_action = action;
                    self.draw_bg.redraw(cx);
                }
                cx.set_cursor(if row.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
            }
            Hit::FingerUp(fe) if fe.is_over => {
                let x = fe.abs.x - fe.rect.pos.x;
                let y = fe.abs.y - fe.rect.pos.y;
                let Some(row) = geom.row_at(y, count) else {
                    return;
                };
                let Some(entry) = self.entries.get(row) else {
                    return;
                };
                let id = entry.id;
                let failed = entry.state == TransferState::Failed;
                if geom.on_action(x, fe.rect.size.x) {
                    if failed {
                        cx.widget_action(uid, FileListAction::Retry(id));
                    } else {
                        cx.widget_action(uid, FileListAction::Removed(id));
                    }
                } else {
                    cx.widget_action(uid, FileListAction::Chose(id));
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let geom = self.geom();
        let count = self.entries.len();
        // A Fit list asks for exactly the rows it has. `Fill` inside a `Fit`
        // parent resolves to nothing at all, so the natural height is worked
        // out here rather than left to the turtle to guess at.
        let walk = Walk {
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(geom.height(count)),
                other => other,
            },
            ..walk
        };
        let dim = if self.disabled { 1.0 } else { 0.0 };
        self.draw_bg.disabled = dim;
        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();

        let name_size = self.draw_name.text_style.font_size as f64;
        let meta_size = self.draw_meta.text_style.font_size as f64;

        if count == 0 {
            let text = self.empty_text.clone();
            if !text.is_empty() {
                self.draw_meta.color = self.color_meta;
                self.draw_meta.draw_abs(
                    cx,
                    dvec2(
                        rect.pos.x + geom.name_x(),
                        line_y(
                            rect.pos.y + geom.row_top(0),
                            geom.row_height,
                            meta_size,
                        ),
                    ),
                    &text,
                );
            }
            self.draw_bg.end(cx);
            return DrawStep::done();
        }

        // Taken out and put back so the loop can reach the draw layers on
        // `self` while it reads the rows.
        let entries = std::mem::take(&mut self.entries);
        let (bar_x, bar_w) = geom.bar(rect.size.x);
        for (i, entry) in entries.iter().enumerate() {
            let top = rect.pos.y + geom.row_top(i);
            let hot = self.hot_row == Some(i);
            let mark = self.state_color(entry.state);

            self.draw_row.hover = if hot { 1.0 } else { 0.0 };
            self.draw_row.disabled = dim;
            self.draw_row.progress = entry.shown_progress() as f32;
            self.draw_row.dot_x = geom.dot_x() as f32;
            self.draw_row.dot_r = geom.dot_radius as f32;
            self.draw_row.bar_x = bar_x as f32;
            self.draw_row.bar_w = bar_w as f32;
            self.draw_row.bar_y = geom.bar_y() as f32;
            self.draw_row.bar_h = geom.bar_height as f32;
            self.draw_row.mark_color = mark;
            self.draw_row.draw_abs(
                cx,
                Rect {
                    pos: dvec2(rect.pos.x, top),
                    size: dvec2(rect.size.x, geom.row_height),
                },
            );

            let band_top = top + geom.content_top();

            // The size and the state, right-aligned against the action.
            let meta = entry.meta_line();
            let meta_w = measure(&self.draw_meta, cx, &meta);
            self.draw_meta.color = if self.disabled { self.color_name_disabled } else { mark };
            self.draw_meta.draw_abs(
                cx,
                dvec2(
                    rect.pos.x + geom.meta_right(rect.size.x) - meta_w,
                    line_y(band_top, NAME_BAND, meta_size),
                ),
                &meta,
            );

            // The name, cut to whatever the size column left it.
            let name_x = rect.pos.x + geom.name_x();
            let room = (rect.pos.x + geom.meta_left(rect.size.x) - 8.0) - name_x;
            let draw_name = &self.draw_name;
            let name = elide_middle(&entry.name, room, &mut |s: &str| measure(draw_name, cx, s));
            self.draw_name.color = if self.disabled {
                self.color_name_disabled
            } else {
                self.color_name
            };
            self.draw_name
                .draw_abs(cx, dvec2(name_x, line_y(band_top, NAME_BAND, name_size)), &name);

            // The trailing action: a retry on a failed row, a remove mark on
            // every other. The multiplication sign, because the text font
            // has it and the heavy cross it does not.
            let (action, action_color) = if entry.state == TransferState::Failed {
                (self.retry_text.clone(), self.color_failed)
            } else {
                ("\u{00d7}".to_string(), self.color_meta)
            };
            if !action.is_empty() {
                let action_w = measure(&self.draw_meta, cx, &action);
                self.draw_meta.color = if self.disabled {
                    self.color_name_disabled
                } else if hot && self.hot_action {
                    self.color_name
                } else {
                    action_color
                };
                self.draw_meta.draw_abs(
                    cx,
                    dvec2(
                        rect.pos.x + geom.action_center(rect.size.x) - action_w * 0.5,
                        line_y(band_top, NAME_BAND, meta_size),
                    ),
                    &action,
                );
            }
        }
        self.entries = entries;
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn text(&self) -> String {
        match self.entries.len() {
            0 => self.empty_text.clone(),
            1 => "1 file".to_string(),
            n => format!("{n} files"),
        }
    }
}

impl FileListRef {
    pub fn set_entries(&self, cx: &mut Cx, entries: Vec<FileEntry>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_entries(cx, entries);
        }
    }

    pub fn push(&self, cx: &mut Cx, entry: FileEntry) -> LiveId {
        match self.borrow_mut() {
            Some(mut inner) => inner.push(cx, entry),
            None => LiveId(0),
        }
    }

    pub fn remove(&self, cx: &mut Cx, id: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.remove(cx, id);
        }
    }

    pub fn clear(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear(cx);
        }
    }

    pub fn set_state(&self, cx: &mut Cx, id: LiveId, state: TransferState, progress: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_state(cx, id, state, progress);
        }
    }

    pub fn set_note(&self, cx: &mut Cx, id: LiveId, note: impl Into<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_note(cx, id, note);
        }
    }

    /// The rows as they stand, copied out.
    pub fn entries(&self) -> Vec<FileEntry> {
        self.borrow()
            .map(|inner| inner.entries().to_vec())
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.borrow().map(|inner| inner.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The row whose retry was pressed.
    pub fn retried(&self, actions: &Actions) -> Option<LiveId> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            FileListAction::Retry(id) => Some(id),
            _ => None,
        }
    }

    /// The row whose remove mark was pressed. Stop the transfer, then call
    /// [`FileListRef::remove`].
    pub fn removed(&self, actions: &Actions) -> Option<LiveId> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            FileListAction::Removed(id) => Some(id),
            _ => None,
        }
    }

    /// The row that was pressed.
    pub fn chose(&self, actions: &Actions) -> Option<LiveId> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            FileListAction::Chose(id) => Some(id),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// ImageWell
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawImageWell {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    accepting: f32,
    #[live]
    refusing: f32,
    #[live]
    selected: f32,
    #[live]
    hover: f32,
    #[live]
    focus: f32,
    #[live]
    disabled: f32,
    #[live]
    border_size: f32,
    #[live]
    border_lift: f32,
    #[live]
    border_radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_selected: Vec4f,
    #[live]
    color_accepting: Vec4f,
    #[live]
    color_refusing: Vec4f,
    #[live]
    color_disabled: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_color_selected: Vec4f,
    #[live]
    border_color_focus: Vec4f,
    #[live]
    border_color_accepting: Vec4f,
    #[live]
    border_color_refusing: Vec4f,
    #[live]
    border_color_disabled: Vec4f,
}

/// The engraved plus over an empty well's prompt. Its own layer because the
/// ground quad is pushed before its rect is resolved, and the mark has to be
/// placed against that rect.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAddMark {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    disabled: f32,
    #[live]
    stroke: f32,
    #[live]
    engrave: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_disabled: Vec4f,
    #[live]
    color_engrave: Vec4f,
}

/// The chosen-or-armed ring and the progress bar, drawn over the picture.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawWellOverlay {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    selected: f32,
    #[live]
    accepting: f32,
    #[live]
    refusing: f32,
    #[live]
    disabled: f32,
    /// Below zero draws no bar at all.
    #[live]
    progress: f32,
    #[live]
    border_size: f32,
    #[live]
    border_lift: f32,
    #[live]
    border_radius: f32,
    #[live]
    bar_height: f32,
    #[live]
    bar_inset: f32,
    #[live]
    ring_color_selected: Vec4f,
    #[live]
    ring_color_accepting: Vec4f,
    #[live]
    ring_color_refusing: Vec4f,
    #[live]
    track_color: Vec4f,
    #[live]
    track_color_disabled: Vec4f,
    #[live]
    mark_color: Vec4f,
    #[live]
    mark_color_disabled: Vec4f,
}

/// The disc behind the corner mark. Its own layer because it is drawn over
/// the picture, and the well's ground is drawn under it.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawClearMark {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hot: f32,
    #[live]
    radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_hot: Vec4f,
}

#[derive(Clone, Debug, Default)]
pub enum ImageWellAction {
    /// A picture arrived and the well is showing it.
    Picked(OfferedFile),
    /// Something was dropped that the well would not take, or that it could
    /// not decode.
    Refused(OfferedFile),
    /// The corner mark emptied the well.
    Cleared,
    /// The well was pressed. It opens no picker of its own.
    Browse,
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct ImageWell {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    pub draw_bg: DrawImageWell,
    #[live]
    draw_add: DrawAddMark,
    #[live]
    draw_over: DrawWellOverlay,
    #[live]
    draw_clear: DrawClearMark,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_mark: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    /// The picture. A slot rather than a property per setting the image
    /// already has, so a caller can give it a different fit or declare a
    /// `src` on it without this widget growing a copy of each.
    #[find]
    #[live]
    pub picture: WidgetRef,

    /// Shown across the middle while the well is empty.
    #[live]
    pub text: String,
    /// What it will take. See [`FileFilter`].
    #[live]
    pub accept: String,
    /// It would take a picture, said without a drag being over it.
    ///
    /// Wanted wherever the picture is coming from somewhere else — a browse
    /// button, a paste, a pick in a list beside the well — which the
    /// drag-only version of this state cannot serve at all, because it can
    /// only be reached from inside a drag.
    #[live]
    pub accepting: bool,
    /// It would not take one, and says so. For a refusal with an end to it,
    /// see [`ImageWell::reject`].
    #[live]
    pub refusing: bool,
    /// An engraved plus over the prompt while the well is empty.
    ///
    /// Wanted wherever the well is one of several, or is small enough that
    /// the prompt has to be short: a plus in a square is the most
    /// recognisable "put something here" mark there is, and a face carrying
    /// nothing but a line of grey text does not read as a target at a
    /// glance. Off by default, because a well that has always been words
    /// alone must keep looking like one.
    #[live]
    pub add_mark: bool,
    #[live(28.0)]
    add_mark_size: f64,
    #[live(6.0)]
    add_mark_gap: f64,
    /// Drawn as chosen: a ring, and a tinted face while it is empty.
    ///
    /// Wanted where several wells stand together and one of them is the one
    /// being worked on — a gallery, a set of slots, a form with a picture
    /// per row. The well never sets this itself, exactly as a list row does
    /// not choose itself: which one is chosen is the host's to know.
    #[live]
    pub selected: bool,
    /// How far a load into this well has got, 0..1. Below zero draws no bar,
    /// which is where a well with nothing going on sits.
    ///
    /// The bar belongs on the well and not on a row beside it: the well is
    /// where the picture went, and a bar somewhere else leaves the reader
    /// matching the two up by eye.
    #[live(-1.0)]
    pub progress: f64,
    /// Seconds a [`ImageWell::reject`] flash lasts before the well goes
    /// quiet again.
    #[live(0.7)]
    pub reject_secs: f64,
    /// A mark in the corner that empties the well.
    #[live(true)]
    pub clearable: bool,
    #[live(20.0)]
    clear_size: f64,
    #[live]
    pub disabled: bool,

    #[live]
    color_text: Vec4f,
    #[live]
    color_text_disabled: Vec4f,

    /// What a drag over the well is saying, if there is one. The host's
    /// half lives in `accepting`/`refusing`; see [`resolve_look`] for why
    /// the two are kept apart.
    #[rust]
    drag_look: Look,
    #[rust]
    reject_timer: Timer,
    #[rust]
    hovered: bool,
    #[rust]
    clear_hot: bool,
}

impl ScriptHook for ImageWell {
    /// A `disabled: true` written in the DSL sets the field directly and
    /// never reaches `set_disabled`, so without this the well would dim
    /// while the picture inside it stayed bright.
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        if self.disabled {
            vm.with_cx_mut(|cx| self.picture.set_disabled(cx, true));
        }
    }
}

impl ImageWell {
    pub fn filter(&self) -> FileFilter {
        FileFilter::parse(&self.accept)
    }

    /// Whether there is a picture. Asked of the image every pass rather than
    /// tracked: a `src` declared in the DSL decodes asynchronously, so the
    /// well cannot know beforehand and a flag would go stale.
    pub fn has_picture(&self) -> bool {
        self.picture.as_image().has_texture()
    }

    /// Show a picture from a file on disk.
    pub fn show_file(&mut self, cx: &mut Cx, path: &str) -> bool {
        let ok = self
            .picture
            .as_image()
            .load_image_file_by_path(cx, std::path::Path::new(path))
            .is_ok();
        self.redraw(cx);
        ok
    }

    /// Show a picture from bytes already in hand, which is the shape a
    /// browser drop arrives in.
    pub fn show_data(&mut self, cx: &mut Cx, data: &[u8]) -> bool {
        let ok = self.picture.as_image().load_image_from_data(cx, data).is_ok();
        self.redraw(cx);
        ok
    }

    /// Show whichever of the two this file carries.
    pub fn show(&mut self, cx: &mut Cx, file: &OfferedFile) -> bool {
        if let Some(bytes) = &file.bytes {
            let bytes = bytes.clone();
            return self.show_data(cx, &bytes);
        }
        if !file.path.is_empty() {
            let path = file.path.clone();
            return self.show_file(cx, &path);
        }
        false
    }

    /// Empty the well.
    pub fn clear(&mut self, cx: &mut Cx) {
        self.picture.as_image().set_texture(cx, None);
        self.redraw(cx);
    }

    /// The corner mark's box, in the well's own frame.
    fn clear_rect(&self, rect: Rect) -> Rect {
        let size = self.clear_size;
        Rect {
            pos: dvec2(rect.pos.x + rect.size.x - size - 4.0, rect.pos.y + 4.0),
            size: dvec2(size, size),
        }
    }

    /// What the well is wearing: the drag while one is over it, else the
    /// look the host asked for.
    fn look(&self) -> Look {
        resolve_look(self.drag_look, host_look(self.accepting, self.refusing))
    }

    /// Say it would take a picture; see [`ImageWell::accepting`].
    pub fn set_accepting(&mut self, cx: &mut Cx, on: bool) {
        if self.accepting != on {
            self.accepting = on;
            self.draw_bg.redraw(cx);
        }
    }

    /// Hold the refusing look until something says otherwise, cancelling any
    /// flash in flight.
    pub fn set_refusing(&mut self, cx: &mut Cx, on: bool) {
        self.stop_reject(cx);
        if self.refusing != on {
            self.refusing = on;
            self.draw_bg.redraw(cx);
        }
    }

    /// Flash the refusing look for `reject_secs`, then go quiet by itself.
    ///
    /// This is the answer to a picture the host turned away for a reason the
    /// well cannot know — too large, the wrong shape, the server said no —
    /// and it has to come from outside, because the well's own filter only
    /// ever reads a name.
    pub fn reject(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.reject_timer);
        self.refusing = true;
        self.reject_timer = if self.reject_secs > 0.0 {
            cx.start_timeout(self.reject_secs)
        } else {
            Timer::empty()
        };
        self.draw_bg.redraw(cx);
    }

    pub fn selected(&self) -> bool {
        self.selected
    }

    /// Draw the well as chosen.
    pub fn set_selected(&mut self, cx: &mut Cx, selected: bool) {
        if self.selected != selected {
            self.selected = selected;
            self.redraw(cx);
        }
    }

    /// How far a load into this well has got, if one is running.
    pub fn progress(&self) -> Option<f64> {
        (self.progress >= 0.0).then(|| self.progress.clamp(0.0, 1.0))
    }

    /// Show a bar across the foot of the well, or `None` to take it away.
    pub fn set_progress(&mut self, cx: &mut Cx, progress: Option<f64>) {
        let v = progress.map(|p| p.clamp(0.0, 1.0)).unwrap_or(-1.0);
        if self.progress != v {
            self.progress = v;
            self.redraw(cx);
        }
    }

    /// Draw the engraved plus over the prompt while the well is empty.
    pub fn set_add_mark(&mut self, cx: &mut Cx, on: bool) {
        if self.add_mark != on {
            self.add_mark = on;
            self.redraw(cx);
        }
    }

    fn stop_reject(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.reject_timer);
        self.reject_timer = Timer::empty();
    }

    fn set_look(&mut self, cx: &mut Cx, look: Look) {
        if self.drag_look != look {
            self.drag_look = look;
            self.draw_bg.redraw(cx);
        }
    }

    fn handle_drag(&mut self, cx: &mut Cx, event: &Event) {
        let uid = self.uid;
        match event.drag_hits(cx, self.draw_bg.area()) {
            DragHit::Drag(drag) => {
                let files = offered_files(&drag.items);
                let filter = self.filter();
                let look = if drag.state == DragState::Out || files.is_empty() {
                    Look::Idle
                } else if files.iter().any(|f| filter.takes(f)) {
                    Look::Accepting
                } else {
                    Look::Refusing
                };
                if let Ok(mut response) = drag.response.lock() {
                    *response = if look == Look::Accepting {
                        DragResponse::Copy
                    } else {
                        DragResponse::None
                    };
                }
                self.set_look(cx, look);
            }
            DragHit::Drop(landed) => {
                self.set_look(cx, Look::Idle);
                let filter = self.filter();
                let files = offered_files(&landed.items);
                // One well holds one picture, so the rest of a multiple drop
                // is turned away out loud rather than dropped on the floor.
                let mut taken = false;
                for file in files {
                    if taken || !filter.takes(&file) {
                        cx.widget_action(uid, ImageWellAction::Refused(file));
                        continue;
                    }
                    if self.show(cx, &file) {
                        taken = true;
                        cx.widget_action(uid, ImageWellAction::Picked(file));
                    } else {
                        cx.widget_action(uid, ImageWellAction::Refused(file));
                    }
                }
            }
            DragHit::DragEnd | DragHit::NoHit => self.set_look(cx, Look::Idle),
        }
    }
}

impl Widget for ImageWell {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            // Every look goes, the host's as well as the drag's: a well
            // that is deaf must not go on saying it would take something.
            self.drag_look = Look::Idle;
            self.accepting = false;
            self.refusing = false;
            self.picture.set_disabled(cx, disabled);
            self.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.picture.handle_event(cx, event, scope);
        // Before the disabled gate: a flash that was running when the well
        // was switched off still has to end, or the well comes back refusing
        // for no reason anyone can see.
        if self.reject_timer.is_event(event).is_some() {
            self.reject_timer = Timer::empty();
            self.refusing = false;
            self.draw_bg.redraw(cx);
        }
        if self.disabled {
            return;
        }
        if matches!(event, Event::Drag(_) | Event::Drop(_) | Event::DragEnd) {
            self.handle_drag(cx, event);
            return;
        }
        let uid = self.uid;
        let showing = self.has_picture();
        let rect = self.draw_bg.area().rect(cx);
        let clear = self.clear_rect(rect);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.hovered = true;
                cx.set_cursor(MouseCursor::Hand);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                self.clear_hot = false;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOver(fe) => {
                let hot = showing && self.clearable && clear.contains(fe.abs);
                if self.clear_hot != hot {
                    self.clear_hot = hot;
                    self.draw_bg.redraw(cx);
                }
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                if fe.is_over {
                    if showing && self.clearable && clear.contains(fe.abs) {
                        self.clear(cx);
                        cx.widget_action(uid, ImageWellAction::Cleared);
                    } else {
                        cx.widget_action(uid, ImageWellAction::Browse);
                    }
                }
                self.draw_bg.redraw(cx);
            }
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => {
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                if matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::Space) {
                    cx.widget_action(uid, ImageWellAction::Browse);
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let showing = self.has_picture();
        let focused = cx.cx.cx.has_key_focus(self.draw_bg.area());
        let look = self.look();
        self.draw_bg.accepting = if look == Look::Accepting { 1.0 } else { 0.0 };
        self.draw_bg.refusing = if look == Look::Refusing { 1.0 } else { 0.0 };
        self.draw_bg.selected = if self.selected { 1.0 } else { 0.0 };
        self.draw_bg.hover = if self.hovered { 1.0 } else { 0.0 };
        self.draw_bg.focus = if focused { 1.0 } else { 0.0 };
        self.draw_bg.disabled = if self.disabled { 1.0 } else { 0.0 };

        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();

        // The well's face is cut to the well: the ground's corner goes to
        // the picture on every draw, since a caller may have written a
        // different one, and a picture cropped to fill covers the ground
        // whole — corners and all — the moment it has anything in it.
        // Written onto the draw struct rather than through a setter: a
        // setter redraws, and a redraw on every draw is a loop.
        let image = self.picture.as_image();
        if let Some(mut image) = image.borrow_mut() {
            image.draw_bg.border_radius = self.draw_bg.border_radius;
        }

        // The picture is drawn every pass, even while there is nothing to
        // see. A `src` declared in the DSL is fetched by the image's OWN
        // draw, so a slot only drawn once it already had a picture would
        // never get one. Empty, it is given no room at all.
        cx.widget_tree_insert_child(self.uid, live_id!(picture), self.picture.clone());
        let picture_size = if showing { rect.size } else { dvec2(0.0, 0.0) };
        let _ = self.picture.draw_walk(
            cx,
            scope,
            {
                // Absolute, at the well's own rect: the picture is the
                // well's face, not something laid out beside it.
                let mut w = Walk::new(
                    Size::Fixed(picture_size.x),
                    Size::Fixed(picture_size.y),
                );
                w.abs_pos = Some(rect.pos);
                w
            },
        );

        if !showing {
            // The plus and the prompt share the empty face, so where each
            // one goes is worked out once, for both.
            let size = self.draw_text.text_style.font_size as f64;
            let (mark_rect, band) = empty_face(
                rect,
                self.add_mark.then_some(self.add_mark_size),
                if self.text.is_empty() { 0.0 } else { size },
                self.add_mark_gap,
            );
            if let Some(mark_rect) = mark_rect {
                self.draw_add.hover = if self.hovered { 1.0 } else { 0.0 };
                self.draw_add.disabled = if self.disabled { 1.0 } else { 0.0 };
                self.draw_add.draw_abs(cx, mark_rect);
            }
            if !self.text.is_empty() {
                let text = self.text.clone();
                let width = measure(&self.draw_text, cx, &text);
                self.draw_text.color = if self.disabled {
                    self.color_text_disabled
                } else {
                    self.color_text
                };
                self.draw_text.draw_abs(
                    cx,
                    dvec2(
                        rect.pos.x + (rect.size.x - width) * 0.5,
                        line_y(band.pos.y, band.size.y, size),
                    ),
                    &text,
                );
            }
        }

        // Over the picture: the ring only when there is a picture hiding the
        // ground's own, and the bar whenever there is one to draw.
        let ring = showing && (self.selected || look != Look::Idle);
        let bar = self.progress >= 0.0;
        if ring || bar {
            self.draw_over.selected = if ring && self.selected { 1.0 } else { 0.0 };
            self.draw_over.accepting = if ring && look == Look::Accepting { 1.0 } else { 0.0 };
            self.draw_over.refusing = if ring && look == Look::Refusing { 1.0 } else { 0.0 };
            self.draw_over.disabled = if self.disabled { 1.0 } else { 0.0 };
            self.draw_over.progress = if bar { self.progress as f32 } else { -1.0 };
            self.draw_over.draw_abs(cx, rect);
        }

        if showing && self.clearable && !self.disabled {
            let mark_rect = self.clear_rect(rect);
            self.draw_clear.hot = if self.clear_hot { 1.0 } else { 0.0 };
            self.draw_clear.radius = (self.clear_size * 0.5) as f32;
            self.draw_clear.draw_abs(cx, mark_rect);
            let mark = "\u{00d7}";
            let size = self.draw_mark.text_style.font_size as f64;
            let width = measure(&self.draw_mark, cx, mark);
            self.draw_mark.draw_abs(
                cx,
                dvec2(
                    mark_rect.pos.x + (mark_rect.size.x - width) * 0.5,
                    line_y(mark_rect.pos.y, mark_rect.size.y, size),
                ),
                mark,
            );
        }

        self.draw_bg.end(cx);
        if !self.disabled {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.clone()
    }
}

impl ImageWellRef {
    pub fn has_picture(&self) -> bool {
        self.borrow().map(|inner| inner.has_picture()).unwrap_or(false)
    }

    /// Say it would take a picture; see [`ImageWell::set_accepting`].
    pub fn set_accepting(&self, cx: &mut Cx, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_accepting(cx, on);
        }
    }

    /// Hold the refusing look; see [`ImageWell::set_refusing`].
    pub fn set_refusing(&self, cx: &mut Cx, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_refusing(cx, on);
        }
    }

    /// Flash a refusal and go quiet again; see [`ImageWell::reject`].
    pub fn reject(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reject(cx);
        }
    }

    pub fn selected(&self) -> bool {
        self.borrow().map(|inner| inner.selected()).unwrap_or(false)
    }

    pub fn set_selected(&self, cx: &mut Cx, selected: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, selected);
        }
    }

    pub fn progress(&self) -> Option<f64> {
        self.borrow().and_then(|inner| inner.progress())
    }

    /// A bar across the foot of the well, or `None` for none.
    pub fn set_progress(&self, cx: &mut Cx, progress: Option<f64>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_progress(cx, progress);
        }
    }

    pub fn set_add_mark(&self, cx: &mut Cx, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_add_mark(cx, on);
        }
    }

    pub fn show_file(&self, cx: &mut Cx, path: &str) -> bool {
        self.borrow_mut()
            .map(|mut inner| inner.show_file(cx, path))
            .unwrap_or(false)
    }

    pub fn show_data(&self, cx: &mut Cx, data: &[u8]) -> bool {
        self.borrow_mut()
            .map(|mut inner| inner.show_data(cx, data))
            .unwrap_or(false)
    }

    pub fn show(&self, cx: &mut Cx, file: &OfferedFile) -> bool {
        self.borrow_mut()
            .map(|mut inner| inner.show(cx, file))
            .unwrap_or(false)
    }

    pub fn clear(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear(cx);
        }
    }

    /// The picture the well just took.
    pub fn picked(&self, actions: &Actions) -> Option<OfferedFile> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            ImageWellAction::Picked(file) => Some(file),
            _ => None,
        }
    }

    /// Something it would not take.
    pub fn refused(&self, actions: &Actions) -> Option<OfferedFile> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            ImageWellAction::Refused(file) => Some(file),
            _ => None,
        }
    }

    pub fn cleared(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), ImageWellAction::Cleared))
            .unwrap_or(false)
    }

    pub fn browse(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), ImageWellAction::Browse))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `script_mod!` block is invisible to the Rust compiler and a
    /// shader that fails to compile is not an error anywhere — the draw is
    /// simply skipped and the widget paints nothing. Building the well out
    /// of its type default and reading the shader-error slot back turns
    /// either into a failed build; the plus and the overlay ring are two
    /// new shaders, so this is what covers them.
    ///
    /// It also pins the promise that matters most about the four new
    /// states: every one of them defaults to exactly what the well did
    /// before it had them.
    #[test]
    fn the_well_comes_out_of_the_dsl_with_every_new_state_off() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let well = cx.with_vm(|vm| {
            crate::script_mod(vm);
            // Registering type defaults compiles nothing; making an instance
            // out of one does. Clearing here keeps any other module's
            // complaint out of this test's answer.
            let _ = crate::makepad_draw::makepad_platform::shader_error::take();
            ImageWell::script_new_with_default(vm)
        });
        assert_eq!(
            crate::makepad_draw::makepad_platform::shader_error::take(),
            None,
            "a draw shader failed to compile"
        );
        // Values only the DSL sets, so the block was really evaluated.
        assert_eq!(well.accept, "image/*");
        assert!(well.clearable);
        // And the four new states, every one of them off.
        assert!(!well.add_mark, "the empty face is words alone until asked");
        assert!(!well.selected);
        assert!(!well.accepting);
        assert!(!well.refusing);
        assert!(well.progress < 0.0, "no bar until a host asks for one");
        assert!(well.progress().is_none());
        // The numbers the new states draw with came through too.
        assert_eq!(well.add_mark_size, 28.0);
        assert_eq!(well.add_mark_gap, 6.0);
        assert_eq!(well.reject_secs, 0.7);
    }

    /// The well is rounded and its face is the picture, so the picture
    /// has to be rounded with it: cropped to fill, it covers the ground
    /// whole and a square picture would square the well's own corners off.
    /// One radius, written on the ground, reaching both.
    #[test]
    fn the_picture_is_cut_to_the_well_it_fills() {
        use crate::makepad_draw::cx_draw::CxDraw;

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut well = cx.with_vm(|vm| {
            vm.bx.captured_errors = Some(Vec::new());
            crate::script_mod(vm);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "{errors:#?}");
            let value = crate::script_eval!(vm, {
                use mod.widgets.*
                ImageWell{
                    width: 120.
                    height: 120.
                    draw_bg +: {border_radius: 7.0}
                }
            });
            ImageWell::script_from_value(vm, value)
        });

        let size = dvec2(200.0, 200.0);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, size);
        let mut draw_list = DrawList2d::new(&mut cx);
        {
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_overlay());
            let walk = well.walk;
            well.draw_walk_all(&mut cx2d, &mut Scope::empty(), walk);
            cx2d.end_pass_sized_turtle();
            draw_list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }

        assert_eq!(well.draw_bg.border_radius, 7.0, "the DSL reached the ground");
        let image = well.picture.as_image();
        let picture = image.borrow().expect("the slot holds a picture");
        assert_eq!(
            picture.draw_bg.border_radius, 7.0,
            "the face was left square in a rounded well"
        );
    }

    /// Turning the plus on is the only thing that may move the prompt, so
    /// with no plus the band it is centred in is still the whole well.
    #[test]
    fn a_well_with_no_plus_leaves_the_prompt_where_it_was() {
        let rect = Rect { pos: dvec2(10.0, 20.0), size: dvec2(120.0, 140.0) };
        let (mark, band) = empty_face(rect, None, 12.0, 6.0);
        assert!(mark.is_none());
        assert_eq!(band, rect);
    }

    #[test]
    fn the_plus_and_the_prompt_are_centred_together() {
        let rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(120.0, 120.0) };
        let (mark, band) = empty_face(rect, Some(28.0), 12.0, 6.0);
        let mark = mark.expect("a plus was asked for");
        assert_eq!(mark.size, dvec2(28.0, 28.0));
        // 28 + 6 + 12 = 46 tall, so the stack starts at (120 - 46) / 2 = 37.
        assert_eq!(mark.pos.y, 37.0);
        assert_eq!(mark.pos.x, 46.0, "and centred across");
        assert_eq!(band.pos.y, 71.0);
        assert_eq!(band.size.y, 12.0);
        // As much room above the plus as below the words.
        let above = mark.pos.y - rect.pos.y;
        let below = rect.size.y - (band.pos.y + band.size.y);
        assert!((above - below).abs() < 1e-9, "{above} vs {below}");
    }

    /// No words means no gap either, or the plus sits low with nothing
    /// under it to balance against.
    #[test]
    fn a_plus_with_no_prompt_takes_the_middle() {
        let rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(100.0, 100.0) };
        let (mark, _) = empty_face(rect, Some(40.0), 0.0, 6.0);
        assert_eq!(mark.unwrap().pos.y, 30.0);
    }

    /// Refusing is the louder answer, so it wins the moment both are set —
    /// a well that is both armed and refusing must read as refusing.
    #[test]
    fn refusing_beats_accepting() {
        assert_eq!(host_look(false, false), Look::Idle);
        assert_eq!(host_look(true, false), Look::Accepting);
        assert_eq!(host_look(false, true), Look::Refusing);
        assert_eq!(host_look(true, true), Look::Refusing);
    }

    #[test]
    fn a_drag_beats_the_look_the_host_asked_for() {
        assert_eq!(resolve_look(Look::Idle, Look::Idle), Look::Idle);
        // Nothing is being dragged: what the host armed is what shows.
        assert_eq!(resolve_look(Look::Idle, Look::Accepting), Look::Accepting);
        assert_eq!(resolve_look(Look::Idle, Look::Refusing), Look::Refusing);
        // A drag is over it, so the platform's answer is the live one — and
        // when it ends, the host's look is still underneath it.
        assert_eq!(resolve_look(Look::Refusing, Look::Accepting), Look::Refusing);
        assert_eq!(resolve_look(Look::Accepting, Look::Refusing), Look::Accepting);
    }

    /// One unit per character, so the elision tests read as counts.
    fn per_char(s: &str) -> f64 {
        s.chars().count() as f64
    }

    #[test]
    fn an_empty_filter_takes_anything() {
        let f = FileFilter::parse("");
        assert!(f.takes_anything());
        assert!(f.accepts("anything.xyz", ""));
        assert_eq!(f.describe(), "any file");
    }

    #[test]
    fn a_star_takes_anything_too() {
        assert!(FileFilter::parse("*").accepts("report.pdf", ""));
        assert!(FileFilter::parse("*/*").accepts("report.pdf", ""));
    }

    #[test]
    fn the_three_spellings_of_an_extension_are_one_pattern() {
        for spec in ["png", ".png", "*.png"] {
            let f = FileFilter::parse(spec);
            assert!(f.accepts("photo.png", ""), "{spec} did not take a png");
            assert!(!f.accepts("photo.jpg", ""), "{spec} took a jpg");
        }
    }

    #[test]
    fn a_pattern_list_splits_on_commas_and_spaces() {
        for spec in ["png,jpg", "png, jpg", "png jpg", " png ,  jpg "] {
            let f = FileFilter::parse(spec);
            assert!(f.accepts("a.png", ""), "{spec}");
            assert!(f.accepts("a.jpg", ""), "{spec}");
            assert!(!f.accepts("a.gif", ""), "{spec}");
        }
    }

    #[test]
    fn matching_ignores_case_on_both_sides() {
        let f = FileFilter::parse("PNG");
        assert!(f.accepts("Photo.PNG", ""));
        assert!(f.accepts("photo.png", ""));
    }

    #[test]
    fn a_family_reads_the_declared_type_when_there_is_one() {
        let f = FileFilter::parse("image/*");
        assert!(f.accepts("blob", "image/png"));
        assert!(!f.accepts("blob", "audio/mpeg"));
    }

    #[test]
    fn a_family_falls_back_to_the_extension_when_nothing_was_declared() {
        // A desktop drop carries a path and no media type at all, so a
        // family pattern that only read the type would refuse every file on
        // the platform most of them are dropped from.
        let f = FileFilter::parse("image/*");
        assert!(f.accepts("photo.jpeg", ""));
        assert!(!f.accepts("song.flac", ""));
        assert!(FileFilter::parse("audio/*").accepts("song.flac", ""));
    }

    #[test]
    fn an_exact_type_reads_as_an_extension_when_nothing_was_declared() {
        let f = FileFilter::parse("image/png");
        assert!(f.accepts("blob", "image/png"));
        assert!(!f.accepts("blob", "image/jpeg"));
        assert!(f.accepts("photo.png", ""));
        assert!(!f.accepts("photo.jpg", ""));
    }

    #[test]
    fn a_drag_the_platform_has_not_named_yet_is_let_through() {
        // The browser reports a count and nothing else until the drop, and a
        // target that refused on that would refuse everything on the web.
        let f = FileFilter::parse("png");
        assert!(f.accepts("", ""));
        assert!(!f.accepts("photo.gif", ""), "the drop itself is still judged");
    }

    #[test]
    fn a_leading_dot_is_a_hidden_file_and_not_an_extension() {
        assert_eq!(extension(".gitignore"), "");
        assert!(!FileFilter::parse("gitignore").accepts(".gitignore", ""));
    }

    #[test]
    fn a_filter_writes_itself_out_without_repeating_itself() {
        assert_eq!(FileFilter::parse("png, .png, *.png").describe(), "png");
        assert_eq!(FileFilter::parse("image/* pdf").describe(), "any image, pdf");
    }

    #[test]
    fn a_name_comes_off_either_separator() {
        assert_eq!(base_name("/home/a/report.pdf"), "report.pdf");
        assert_eq!(base_name("C:\\Users\\a\\report.pdf"), "report.pdf");
        assert_eq!(base_name("report.pdf"), "report.pdf");
        assert_eq!(base_name(""), "");
    }

    #[test]
    fn sizes_read_the_way_a_file_manager_says_them() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1), "1 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1000), "1.0 kB");
        assert_eq!(human_size(1536), "1.5 kB");
        assert_eq!(human_size(12_345_678), "12 MB");
        assert_eq!(human_size(1_000_000_000), "1.0 GB");
    }

    #[test]
    fn a_size_steps_up_a_unit_rather_than_printing_a_thousand_of_the_last() {
        // 999.5 kB rounds to "1000 kB" on any rule that formats first and
        // steps after, which is the wrong unit and one character wider than
        // the column was measured for.
        assert_eq!(human_size(999_499), "999 kB");
        assert_eq!(human_size(999_500), "1.0 MB");
    }

    #[test]
    fn a_size_loses_its_decimal_at_ten_so_the_column_stops_shuffling() {
        assert_eq!(human_size(9_949), "9.9 kB");
        assert_eq!(human_size(9_950), "10 kB");
    }

    #[test]
    fn a_name_that_fits_is_left_alone() {
        assert_eq!(elide_middle("report.pdf", 20.0, &mut |s: &str| per_char(s)), "report.pdf");
    }

    #[test]
    fn a_long_name_is_cut_in_the_middle_so_the_extension_survives() {
        let cut = elide_middle("a-very-long-report-name.pdf", 12.0, &mut |s: &str| per_char(s));
        assert!(cut.ends_with("me.pdf"), "the tail was lost: {cut}");
        assert!(cut.starts_with("a-ver"), "the head was lost: {cut}");
        assert_eq!(cut.chars().count(), 12);
    }

    #[test]
    fn a_cut_name_never_comes_back_wider_than_the_room_it_was_cut_for() {
        // The first cut kept a fixed tail, so a narrow column returned a
        // mark and six characters that together overflowed it.
        for room in 1..14 {
            let cut = elide_middle("a-very-long-report-name.pdf", room as f64, &mut |s: &str| per_char(s));
            assert!(
                cut.chars().count() <= room.max(1),
                "{room} points held {cut}"
            );
        }
    }

    #[test]
    fn no_room_draws_nothing() {
        assert_eq!(elide_middle("report.pdf", 0.0, &mut |s: &str| per_char(s)), "");
    }

    /// The geometry a DSL preset gives a row.
    fn geom() -> RowGeom {
        RowGeom {
            row_height: 46.0,
            row_gap: 2.0,
            row_inset: 12.0,
            list_inset: 6.0,
            bar_height: 4.0,
            action_width: 54.0,
            meta_width: 130.0,
            dot_radius: 4.0,
        }
    }

    #[test]
    fn the_hit_test_agrees_with_where_a_row_is_drawn() {
        let g = geom();
        for i in 0..4 {
            let top = g.row_top(i);
            assert_eq!(g.row_at(top + 1.0, 4), Some(i));
            assert_eq!(g.row_at(top + g.row_height - 1.0, 4), Some(i));
        }
    }

    #[test]
    fn the_gap_between_two_rows_belongs_to_neither() {
        let g = geom();
        let between = g.row_top(0) + g.row_height + g.row_gap * 0.5;
        assert_eq!(g.row_at(between, 3), None);
        assert_eq!(g.row_at(0.0, 3), None, "the padding above the first row");
        assert_eq!(g.row_at(g.row_top(3), 3), None, "past the last row");
    }

    #[test]
    fn an_empty_list_still_asks_for_a_row_of_height() {
        let g = geom();
        assert_eq!(g.height(0), g.height(1));
        assert!(g.height(3) > g.height(1));
    }

    #[test]
    fn only_the_trailing_column_is_the_action() {
        let g = geom();
        let width = 400.0;
        assert!(g.on_action(g.action_center(width), width));
        assert!(!g.on_action(g.name_x(), width));
        assert!(!g.on_action(g.meta_right(width) - 1.0, width));
    }

    #[test]
    fn the_bar_stops_before_the_size_column() {
        let g = geom();
        let width = 400.0;
        let (x, w) = g.bar(width);
        assert_eq!(x, g.name_x());
        assert!(x + w <= g.meta_right(width) + f64::EPSILON);
    }

    #[test]
    fn a_narrow_row_leaves_the_name_no_room_rather_than_a_negative_one() {
        let g = geom();
        // Everything but the name is a fixed column, so a list squeezed
        // narrower than those columns used to hand the name a negative
        // width and the elision a room to cut to that could never be met.
        let width = 120.0;
        assert!(g.meta_left(width) >= g.name_x());
        let (_, w) = g.bar(width);
        assert!(w > 0.0);
    }

    #[test]
    fn the_row_block_is_centred_however_tall_the_row_is() {
        let mut g = geom();
        g.row_height = 80.0;
        let above = g.content_top();
        let below = g.row_height - (g.bar_y() + g.bar_height);
        assert!((above - below).abs() < 0.001, "{above} above, {below} below");
    }

    #[test]
    fn the_bar_says_what_the_state_says() {
        let mut e = FileEntry::new("a.png", 10);
        assert_eq!(e.shown_progress(), 0.0, "waiting is empty");
        e.state = TransferState::Sending;
        e.progress = 0.4;
        assert_eq!(e.shown_progress(), 0.4);
        // A host that marks a row done without pushing the number to 1 is
        // reporting the truth; a bar that then read 0.4 would not be.
        e.state = TransferState::Done;
        assert_eq!(e.shown_progress(), 1.0);
    }

    #[test]
    fn a_failed_row_says_why_when_it_can() {
        let mut e = FileEntry::new("a.png", 2048);
        e.state = TransferState::Failed;
        assert_eq!(e.meta_line(), "2.0 kB \u{00b7} failed");
        e.note = "the far end hung up".to_string();
        assert_eq!(e.meta_line(), "2.0 kB \u{00b7} the far end hung up");
    }

    #[test]
    fn a_dragged_string_is_not_a_file() {
        let items = vec![
            DragItem::String {
                value: "some text".to_string(),
                internal_id: None,
            },
            DragItem::FilePath {
                path: "/tmp/report.pdf".to_string(),
                internal_id: None,
            },
        ];
        let files = offered_files(&items);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "report.pdf");
    }
}
