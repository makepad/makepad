//! Media and MediaFigure — a picture that copes with not having arrived
//! yet, or not existing at all, and one with a caption under it.
//!
//! An `Image` is honest about exactly one thing: the picture. It has no
//! size until the bytes are decoded, so the row it sits in reflows the
//! moment they land; it has nothing to show while they are on their way;
//! and a source that is absent, unreadable, or not a picture at all leaves
//! a hole with no way to say so. Every screen that shows pictures it did
//! not author — a feed, a gallery, a wall of covers — writes the same three
//! answers around it, and writes them differently each time.
//!
//! `Media` is those three answers and nothing else:
//!
//! * The box keeps a SHAPE of its own, from the first frame, whatever the
//!   picture turns out to be. A `ratio` fills in whichever axis the caller
//!   left to the content, so the page settles before the picture exists and
//!   does not jump when it arrives. `ratio: 0.0` gives that up and lets the
//!   picture decide — which is the jump, stated as a choice.
//! * A PLACEHOLDER stands in the box while a source is on its way, and a
//!   still mark stands there when there is nothing left to wait for. Both
//!   are slots, so an app that has a better answer (a blurred thumbnail, a
//!   spinner, its own artwork) puts it there.
//! * A second source is tried when the first one fails. Absent, errored,
//!   or answering with bytes that are not a picture — a redirect page, a
//!   truncated file — all count as failure, because from the reader's side
//!   they are the same thing.
//!
//! The fit rules are the familiar three. COVER keeps the picture's shape
//! and fills the box, cropping whatever hangs over; CONTAIN keeps its shape
//! and puts the whole of it inside, showing the ground where it does not
//! reach; FILL stretches it to the box, shape and all. Cover is done by the
//! image's own shader rather than by drawing something oversized and
//! clipping it: the arithmetic here works out the rect either way, and
//! hands the box's own size to a picture that would overflow it.
//!
//! The source goes on the `Media`, not on the image sitting in its slot.
//! The box only knows about the sources it fetched itself: an image asked
//! to load its own `src` is one the box cannot see waiting, and it will
//! show the mark for a missing picture until that load lands.
//!
//! **What it is not.** It is not a loader. The image widget and the image
//! cache below it do every byte of the fetching, decoding and caching, and
//! this widget only decides WHICH source they are pointed at and what is
//! drawn while they work. It cannot be told WHERE in the box a cropped
//! picture is taken from: a fitted picture is centred, or wherever the
//! box's own `align` puts it, and nothing finer than that. It
//! does not clip the picture to its own rounded corners — the ground behind
//! has a radius, the picture does not, so a covering picture in a rounded
//! box has square corners. It does not retry, back off, or wait for
//! scrolling: a source that failed is done until the source itself changes.
//! And an SVG source is not a picture as far as this widget is concerned,
//! for the same reason it is not one to the image widget's own `src`: that
//! path decodes rasters on a worker. Put the drawing in the slot instead.

use crate::{
    image::ImageWidgetRefExt,
    widget_tree::CxWidgetExt,
    image_cache::{image_size_by_data, ImageFit},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// How the picture is fitted into the box it is given.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum MediaFit {
    /// Keep the picture's shape and fill the box; crop the overhang.
    #[pick]
    Cover = 0,
    /// Keep the picture's shape and fit the whole of it inside the box.
    Contain = 1,
    /// Stretch the picture to the box, shape and all.
    Fill = 2,
}

/// What the box is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaStatus {
    /// A source is on its way, or its bytes are still being decoded.
    Loading,
    /// The picture is there.
    Ready,
    /// Every source failed. Nothing more is coming.
    Missing,
}

impl MediaStatus {
    /// The name a snapshot reports.
    pub fn name(self) -> &'static str {
        match self {
            MediaStatus::Loading => "loading",
            MediaStatus::Ready => "ready",
            MediaStatus::Missing => "missing",
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Declared before the `use` below, because a block's `use` only sees
    // what exists when it runs. Not splatted: `Fill` is already the
    // turtle's word for a size, so the fit stays behind its type name and
    // is written `MediaFit.Cover`.
    let MediaFit = set_type_default() do #(MediaFit::script_api(vm))
    mod.widgets.MediaFit = MediaFit

    use mod.widgets.*

    mod.widgets.DrawMediaBase = #(DrawMedia::script_component(vm))
    set_type_default() do #(DrawMedia::script_shader(vm)){
        ..mod.draw.DrawQuad

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.border_radius)
            sdf.fill(self.color)
            return sdf.result
        }
    }

    mod.widgets.MediaBase = #(Media::register_widget(vm))

    /** A picture in a box that has its shape before the picture does.
     *
     * The slots are drawn one at a time in the same place, so the flow is
     * Overlay and the picture, the placeholder and the missing mark all
     * land in the middle of the box. */
    mod.widgets.Media = set_type_default() do mod.widgets.MediaBase{
        width: Fill
        height: Fit
        flow: Overlay
        align: Align{x: 0.5, y: 0.5}

        /** the box's width over its height; 0 leaves the shape to the picture 0..4 step 0.05 */
        ratio: 1.5
        /** MediaFit.Cover Contain Fill */
        fit: MediaFit.Cover
        /** drawn at all */
        visible: true

        // Slots, not properties: a caller hands over a whole widget, so
        // they are written `placeholder: X{...}` and never `placeholder :=
        // X{...}`, which would leave the slot empty and the box blank.
        /** the picture; the box hands it its own rect every draw */
        picture: mod.widgets.Image{}
        /** stands in the box while a source is on its way */
        placeholder: mod.widgets.ContentPlaceholder{width: Fill height: Fill}
        /** stands in the box when every source has failed; still, because
         * nothing is coming */
        missing: mod.widgets.ContentPlaceholder{
            width: Fill
            height: Fill
            animation: mod.widgets.PlaceholderAnimation.Static
        }

        // Both values are plain and both have a field on the draw struct
        // behind them. No uniform(): overriding a uniform on a draw type
        // that also carries instance fields moves the instance slots out
        // from under the caller who overrode one.
        draw_bg +: {
            /** the ground, seen wherever the picture does not reach */
            color: theme.color_placeholder
            /** corner rounding of the ground; the picture is not clipped to it 0..24 step 0.5 */
            border_radius: 0.0
        }
    }

    /** The words under a picture: quiet, small, and as wide as the figure
     * so they wrap inside its column instead of running past it.
     *
     * A slot filled with a bare `Label` takes the label's own dress, so
     * without this every caller writing a caption would re-write these
     * four lines to get the same one. */
    mod.widgets.MediaCaption = mod.widgets.Label{
        width: Fill
        height: Fit
        padding: 0.0
        draw_text +: {
            color: theme.color_text_meta
            text_style: theme.font_body_s
        }
    }

    mod.widgets.MediaFigureBase = #(MediaFigure::register_widget(vm))

    /** A picture with its caption under it.
     *
     * Give the figure a width. The caption fills it so the words wrap in
     * the picture's own column, and a Fill inside a Fit is laid out and
     * never painted. */
    mod.widgets.MediaFigure = set_type_default() do mod.widgets.MediaFigureBase{
        width: Fill
        height: Fit
        /** gap between the picture and the caption 0..24 step 1 */
        spacing: theme.space_1
        /** drawn at all */
        visible: true

        /** the picture */
        media: mod.widgets.Media{}
        /** the words under it */
        caption: mod.widgets.MediaCaption{}
    }
}

/// The ground the picture is drawn on: one colour and one corner radius.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMedia {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_radius: f32,
}

/// Whether an axis the turtle reported is a real measurement. A `Fit` axis
/// peeks as NaN and a collapsed one as zero, and neither is a box.
fn measured(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// The box the widget takes: what the walk pinned down, and `ratio` (or the
/// picture's own shape) for whichever axis the caller left open.
///
/// An axis the walk settled always wins — `ratio` answers for the open one,
/// never over a stated one. A box with neither a ratio nor a picture has no
/// shape to take and takes nothing, which is the reflow `ratio` exists to
/// prevent, arrived at by asking for it.
pub(crate) fn box_size(peeked: (f64, f64), ratio: f64, natural: Option<(f64, f64)>) -> (f64, f64) {
    let ratio = if ratio.is_finite() && ratio > 0.0 {
        Some(ratio)
    } else {
        match natural {
            Some((w, h)) if measured(w) && measured(h) => Some(w / h),
            _ => None,
        }
    };
    let (mut w, mut h) = peeked;
    match (measured(w), measured(h)) {
        (true, true) => {}
        (true, false) => h = ratio.map_or(0.0, |ratio| w / ratio),
        (false, true) => w = ratio.map_or(0.0, |ratio| h * ratio),
        // Nothing stated on either axis: the picture's own size is all
        // there is to go on, and before it arrives there is nothing.
        (false, false) => {
            let (nw, nh) = natural.unwrap_or((0.0, 0.0));
            w = nw;
            h = nh;
        }
    }
    // max() returns the other side of a NaN, which is what an unmeasured
    // axis reports.
    (w.max(0.0), h.max(0.0))
}

/// The size the picture is drawn at inside a box, under `fit`.
///
/// `Cover` deliberately answers with a size LARGER than the box on one
/// axis: that overhang is the crop. The draw path hands a picture that
/// would overflow the box's own size instead, and lets the image's shader
/// crop to exactly the rect this describes — nothing here clips, so an
/// oversized child would paint over its neighbours.
pub(crate) fn fit_size(fit: MediaFit, natural: Option<(f64, f64)>, bounds: (f64, f64)) -> (f64, f64) {
    let (bw, bh) = bounds;
    if !measured(bw) || !measured(bh) {
        return (0.0, 0.0);
    }
    let (sw, sh) = match natural {
        Some((w, h)) if measured(w) && measured(h) => (w, h),
        // Nothing known about the picture — a slot holding something that
        // is not an image widget, and has no pixel size to ask for. The
        // box is the only honest answer.
        _ => return (bw, bh),
    };
    match fit {
        MediaFit::Fill => (bw, bh),
        MediaFit::Contain => {
            let scale = (bw / sw).min(bh / sh);
            (sw * scale, sh * scale)
        }
        MediaFit::Cover => {
            let scale = (bw / sw).max(bh / sh);
            (sw * scale, sh * scale)
        }
    }
}

/// The image widget's own fit that draws ours. Cover is the crop the image
/// shader already does; the other two are exact sizes worked out here, so
/// the image is told to take the rect it is given and nothing else.
fn image_fit(fit: MediaFit) -> ImageFit {
    match fit {
        MediaFit::Cover => ImageFit::CropToFill,
        MediaFit::Contain | MediaFit::Fill => ImageFit::Stretch,
    }
}

/// Which source the widget is on. It only moves forward: a source that
/// failed is not tried again until `src` itself changes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Attempt {
    #[default]
    First,
    Second,
    Spent,
}

/// What one source has to say when it is asked for its bytes.
enum Fetch {
    /// On its way. Ask again next draw.
    Waiting,
    Arrived(PathBuf, Rc<Vec<u8>>),
    /// Absent, errored, or no such resource. Nothing will arrive.
    Failed,
}

/// Ask a source for its bytes, the way the image widget asks for its own,
/// with the difference that a failure is reported rather than swallowed:
/// this widget has somewhere else to go when one fails.
fn fetch(cx: &mut Cx, src: &Option<ScriptHandleRef>) -> Fetch {
    let Some(handle_ref) = src else {
        return Fetch::Failed;
    };
    let handle = handle_ref.as_handle();
    let heap_key = handle_ref.heap_key();
    if let Some(data) = cx.get_resource(heap_key, handle) {
        return arrived(cx, heap_key, handle, data);
    }
    cx.load_script_resource(heap_key, handle);
    if let Some(data) = cx.get_resource(heap_key, handle) {
        return arrived(cx, heap_key, handle, data);
    }
    // No bytes. Either the request is still in flight (an http resource on
    // the web), or the load already failed — a file that is not there is an
    // error, not a wait — or there is no such resource at all.
    let resources = cx.script_data.resources.resources.borrow();
    match resources.iter().find(|res| res.has_handle(heap_key, handle)) {
        Some(res) if !res.is_error() => Fetch::Waiting,
        _ => Fetch::Failed,
    }
}

fn arrived(cx: &Cx, heap_key: usize, handle: ScriptHandle, data: Rc<Vec<u8>>) -> Fetch {
    let path = cx.get_resource_abs_path(heap_key, handle).unwrap_or_default();
    Fetch::Arrived(PathBuf::from(path), data)
}

/// A picture that copes with not having arrived yet, or not existing.
#[derive(Script, ScriptHook, Widget)]
pub struct Media {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The ground, which is also the box: it carries the area, so it is
    /// drawn on every pass whatever else is.
    #[redraw]
    #[live]
    pub draw_bg: DrawMedia,

    /// The picture. A slot rather than a property, so a caller can hand
    /// over an image already dressed the way the app wants it.
    #[find]
    #[live]
    pub picture: WidgetRef,
    /// What stands in the box while a source is on its way.
    #[find]
    #[live]
    pub placeholder: WidgetRef,
    /// What stands in the box when every source has failed.
    #[find]
    #[live]
    pub missing: WidgetRef,

    /// The first source to try.
    #[live]
    src: Option<ScriptHandleRef>,
    /// The one to try when the first fails.
    #[live]
    fallback: Option<ScriptHandleRef>,

    #[live]
    pub fit: MediaFit,
    /// Width over height. Zero leaves the shape to the picture.
    #[live(1.5)]
    pub ratio: f64,
    #[live(true)]
    #[visible]
    visible: bool,

    /// The handle the walk down the sources belongs to. A `src` replaced
    /// after the fact starts the walk again instead of keeping the answer
    /// worked out for the old one.
    #[rust]
    wired: Option<ScriptHandle>,
    #[rust]
    attempt: Attempt,
    /// The picture's own pixel size, read from the header the moment the
    /// bytes arrive — well before the decode finishes, which is what lets
    /// a `ratio: 0.0` box take the right shape on the same frame.
    #[rust]
    natural: Option<(f64, f64)>,
    /// The fit last pushed into the image widget.
    #[rust]
    applied_fit: Option<MediaFit>,
    /// What the last draw put in the box.
    #[rust]
    drawn: Option<MediaStatus>,
}

impl Media {
    /// Walk down the sources until one has bytes or there are none left.
    ///
    /// Called at the top of every draw, the way the image widget polls its
    /// own resource: a source still in flight is simply asked again on the
    /// next one.
    fn pump(&mut self, cx: &mut Cx) {
        let wanted = self.src.as_ref().map(|src| src.as_handle());
        if self.wired != wanted {
            self.wired = wanted;
            self.attempt = Attempt::First;
            self.natural = None;
        }
        loop {
            let source = match self.attempt {
                Attempt::First => &self.src,
                Attempt::Second => &self.fallback,
                Attempt::Spent => return,
            };
            match fetch(cx, source) {
                Fetch::Waiting => return,
                Fetch::Arrived(path, data) => {
                    if self.show(cx, &path, &data) {
                        self.attempt = Attempt::Spent;
                        return;
                    }
                    self.give_up();
                }
                Fetch::Failed => self.give_up(),
            }
        }
    }

    fn give_up(&mut self) {
        self.attempt = match self.attempt {
            Attempt::First => Attempt::Second,
            _ => Attempt::Spent,
        };
    }

    /// Hand the bytes to the picture. False means this source is not one.
    ///
    /// The header is read here rather than left to the decode, because a
    /// source that answers with something else — a redirect page, a
    /// truncated file — is a source that FAILED, and there is a second one
    /// to try. It also settles the picture's size a frame or more before
    /// the decode does.
    fn show(&mut self, cx: &mut Cx, path: &Path, data: &Rc<Vec<u8>>) -> bool {
        let Ok((w, h)) = image_size_by_data(&data[..], path) else {
            return false;
        };
        self.natural = Some((w as f64, h as f64));
        let image = self.picture.as_image();
        if image.borrow().is_none() {
            // The slot holds something that is not an image widget. There
            // is nothing to hand bytes to, and nothing failed either.
            return true;
        }
        image
            .load_image_from_data_async(cx, path, Arc::new((**data).clone()))
            .is_ok()
    }

    /// Whether the picture slot has something to show. Asked of the image
    /// every pass rather than tracked: the decode lands in the image's own
    /// handler, and a flag kept here would go stale.
    fn has_picture(&self) -> bool {
        // Bound rather than chained: `as_image()` makes a temporary, and
        // a borrow taken straight off it dies at the end of the
        // statement while the match arms still want it.
        let image = self.picture.as_image();
        let borrowed = image.borrow();
        match borrowed {
            Some(image) => image.has_content(),
            // Not an image widget: whatever the caller put there is theirs
            // to show, and hiding it behind a placeholder for ever would be
            // the wrong reading of a slot that is already full.
            None => !self.picture.is_empty(),
        }
    }

    /// What the box is showing.
    pub fn status(&self) -> MediaStatus {
        if self.has_picture() {
            return MediaStatus::Ready;
        }
        // Spent with nothing read means every source failed. Spent with a
        // size read means the bytes are in the decoder.
        if self.attempt == Attempt::Spent && self.natural.is_none() {
            return MediaStatus::Missing;
        }
        MediaStatus::Loading
    }

    /// The picture's own size in pixels, once a source has been read.
    pub fn natural_size(&self) -> Option<(f64, f64)> {
        self.natural
    }
}

impl Widget for Media {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.pump(cx.cx.cx);

        let mut walk = cx.resolve_walk(walk, ResolveAt::BeforeBegin);
        let peeked = cx.peek_walk_turtle(walk);
        let (bw, bh) = box_size((peeked.size.x, peeked.size.y), self.ratio, self.natural);
        // An axis the walk left open is pinned down here rather than left
        // to the content: the placeholder and a covering picture both ask
        // to Fill, and a Fill inside a Fit is laid out and never painted.
        if !measured(peeked.size.x) {
            walk.width = Size::Fixed(bw);
        }
        if !measured(peeked.size.y) {
            walk.height = Size::Fixed(bh);
        }

        let inner = (
            (bw - self.layout.padding.width()).max(0.0),
            (bh - self.layout.padding.height()).max(0.0),
        );
        let (pw, ph) = fit_size(self.fit, self.natural, inner);
        let picture_walk = Walk {
            // Cover overflows on purpose and is cropped by the image's own
            // shader, so what is laid out is never bigger than the box.
            width: Size::Fixed(pw.min(inner.0)),
            height: Size::Fixed(ph.min(inner.1)),
            ..Walk::default()
        };
        if self.applied_fit != Some(self.fit) {
            self.applied_fit = Some(self.fit);
            // Pushed only when it changes: this redraws, and a redraw on
            // every draw is a loop.
            self.picture
                .as_image()
                .set_walk_and_fit(cx.cx.cx, picture_walk, image_fit(self.fit));
        }

        // The shape is the widget: the picture and the things that stand in
        // for it occupy the same place, not one after the other. Only the
        // flow is forced — `align` still decides where a contained picture
        // sits in the box.
        let layout = Layout {
            flow: Flow::Overlay,
            ..self.layout
        };
        self.draw_bg.begin(cx, walk, layout);
        let status = self.status();
        // Exactly one of the three is drawn. The picture is left out until
        // it has content, because an image with no texture bound is not
        // reliably nothing, and the slot standing in its place may well be
        // smaller than the box.
        let (name, slot, fitted) = match status {
            MediaStatus::Ready => (live_id!(picture), self.picture.clone(), true),
            MediaStatus::Loading => (live_id!(placeholder), self.placeholder.clone(), false),
            MediaStatus::Missing => (live_id!(missing), self.missing.clone(), false),
        };
        if !slot.is_empty() {
            // Inserted by name so a host can reach `ids!(media.picture)`;
            // a `#[find]` field hands out the slot's children instead of
            // the slot itself.
            cx.widget_tree_insert_child(self.uid, name, slot.clone());
            let slot_walk = if fitted {
                picture_walk
            } else {
                slot.walk(cx.cx.cx)
            };
            let _ = slot.draw_walk(cx, scope, slot_walk);
        }
        self.draw_bg.end(cx);
        self.drawn = Some(status);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        for slot in [&self.picture, &self.placeholder, &self.missing] {
            if !slot.is_empty() {
                slot.handle_event(cx, event, scope);
            }
        }
        // A finished decode lands in the image's own handler, which repaints
        // the image and knows nothing about the placeholder drawn over the
        // top of it — and while the picture is not being drawn at all, its
        // area is not one anything can repaint through. The box has to
        // notice the change itself, or the placeholder outlives the picture
        // it was standing in for.
        if let Some(drawn) = self.drawn {
            if drawn != self.status() {
                self.redraw(cx);
            }
        }
    }

    /// "loading", "ready" or "missing": what a test waits on.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.status().name().to_string())
    }
}

impl MediaRef {
    /// See [`Media::status`].
    pub fn status(&self) -> MediaStatus {
        self.borrow()
            .map(|inner| inner.status())
            .unwrap_or(MediaStatus::Loading)
    }

    /// See [`Media::natural_size`].
    pub fn natural_size(&self) -> Option<(f64, f64)> {
        self.borrow().and_then(|inner| inner.natural)
    }

    /// The picture itself, for a caller that needs the image widget.
    pub fn picture(&self) -> WidgetRef {
        self.borrow()
            .map(|inner| inner.picture.clone())
            .unwrap_or_default()
    }
}

/// A picture with its caption under it.
#[derive(Script, ScriptHook, Widget)]
pub struct MediaFigure {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// Picture and caption together: what a snapshot and the pick see.
    #[redraw]
    #[area]
    area: Area,

    /// The picture. Any widget, though a `Media` is the point.
    #[find]
    #[live]
    pub media: WidgetRef,
    /// The words under it.
    #[find]
    #[live]
    pub caption: WidgetRef,
    #[live(true)]
    #[visible]
    visible: bool,
}

impl Widget for MediaFigure {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        // Down, always. A caption BESIDE a picture is a row of two widgets,
        // and a row is not what this one is for.
        let layout = Layout {
            flow: Flow::Down,
            ..self.layout
        };
        cx.begin_turtle(walk, layout);
        for (name, slot) in [
            (live_id!(media), self.media.clone()),
            (live_id!(caption), self.caption.clone()),
        ] {
            if slot.is_empty() {
                continue;
            }
            cx.widget_tree_insert_child(self.uid, name, slot.clone());
            let slot_walk = slot.walk(cx.cx.cx);
            let _ = slot.draw_walk(cx, scope, slot_walk);
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        for slot in [&self.media, &self.caption] {
            if !slot.is_empty() {
                slot.handle_event(cx, event, scope);
            }
        }
    }

    /// The caption, so a host reads and writes the figure's words without
    /// knowing which widget is carrying them.
    fn text(&self) -> String {
        self.caption.text()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.caption.set_text(cx, v);
    }
}

impl MediaFigureRef {
    /// The picture, for a caller that needs the media widget.
    pub fn media(&self) -> MediaRef {
        self.borrow()
            .map(|inner| inner.media.as_media())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture wider than the box it is put in, and one taller.
    const WIDE: Option<(f64, f64)> = Some((400.0, 100.0));
    const TALL: Option<(f64, f64)> = Some((100.0, 400.0));
    const EVERY_FIT: [MediaFit; 3] = [MediaFit::Cover, MediaFit::Contain, MediaFit::Fill];

    #[test]
    fn contain_puts_the_whole_picture_in_and_cover_fills_the_box() {
        // A wide picture in a tall box.
        assert_eq!(fit_size(MediaFit::Contain, WIDE, (100.0, 200.0)), (100.0, 25.0));
        assert_eq!(fit_size(MediaFit::Cover, WIDE, (100.0, 200.0)), (800.0, 200.0));
        // A tall picture in a wide box.
        assert_eq!(fit_size(MediaFit::Contain, TALL, (200.0, 100.0)), (25.0, 100.0));
        assert_eq!(fit_size(MediaFit::Cover, TALL, (200.0, 100.0)), (200.0, 800.0));
        // Either way round: contain never crosses the edge, cover never
        // leaves a gap, one axis touches exactly in both, and neither
        // changes the picture's own shape.
        for (natural, (bw, bh)) in [(WIDE, (100.0, 200.0)), (TALL, (200.0, 100.0))] {
            let (sw, sh) = natural.unwrap();
            for fit in [MediaFit::Contain, MediaFit::Cover] {
                let (w, h) = fit_size(fit, natural, (bw, bh));
                if fit == MediaFit::Contain {
                    assert!(w <= bw && h <= bh);
                } else {
                    assert!(w >= bw && h >= bh);
                }
                assert!(w == bw || h == bh);
                assert!((w / h - sw / sh).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn fill_takes_the_box_and_the_pictures_shape_with_it() {
        assert_eq!(fit_size(MediaFit::Fill, WIDE, (100.0, 200.0)), (100.0, 200.0));
        assert_eq!(fit_size(MediaFit::Fill, TALL, (200.0, 100.0)), (200.0, 100.0));
    }

    #[test]
    fn a_box_with_no_room_draws_nothing() {
        for fit in EVERY_FIT {
            assert_eq!(fit_size(fit, WIDE, (0.0, 0.0)), (0.0, 0.0));
            // One axis is enough to have nothing to draw in.
            assert_eq!(fit_size(fit, WIDE, (100.0, 0.0)), (0.0, 0.0));
            assert_eq!(fit_size(fit, WIDE, (0.0, 100.0)), (0.0, 0.0));
            // An axis the turtle could not measure reads as NaN.
            assert_eq!(fit_size(fit, WIDE, (f64::NAN, 100.0)), (0.0, 0.0));
        }
    }

    #[test]
    fn a_picture_nothing_is_known_about_takes_the_box() {
        for fit in EVERY_FIT {
            assert_eq!(fit_size(fit, None, (100.0, 200.0)), (100.0, 200.0));
            // A source with a zero side is no more use than no source.
            assert_eq!(fit_size(fit, Some((0.0, 10.0)), (100.0, 200.0)), (100.0, 200.0));
        }
    }

    #[test]
    fn the_ratio_answers_for_the_axis_the_walk_left_open() {
        // Width stated, height open.
        assert_eq!(box_size((300.0, f64::NAN), 1.5, None), (300.0, 200.0));
        // Height stated, width open.
        assert_eq!(box_size((f64::NAN, 200.0), 1.5, None), (300.0, 200.0));
        // Both stated: the walk wins and the ratio is not consulted.
        assert_eq!(box_size((300.0, 300.0), 1.5, None), (300.0, 300.0));
    }

    #[test]
    fn without_a_ratio_the_box_waits_for_the_picture_and_then_takes_its_shape() {
        // Nothing to go on: no shape, which is the reflow `ratio` prevents.
        assert_eq!(box_size((300.0, f64::NAN), 0.0, None), (300.0, 0.0));
        assert_eq!(box_size((f64::NAN, f64::NAN), 0.0, None), (0.0, 0.0));
        // Once the picture is read, its own shape fills the open axis.
        assert_eq!(box_size((300.0, f64::NAN), 0.0, WIDE), (300.0, 75.0));
        // With neither axis stated, the picture is the box.
        assert_eq!(box_size((f64::NAN, f64::NAN), 0.0, WIDE), (400.0, 100.0));
    }

    #[test]
    fn registration_is_wired() {
        let lib = include_str!("lib.rs");
        let media = include_str!("media.rs");
        assert!(lib.contains("pub mod media;"));
        assert!(lib.contains("media::*"));
        assert!(lib.contains("crate::media::script_mod(vm);"));
        assert!(media.contains("mod.widgets.MediaBase = #(Media::register_widget(vm))"));
        assert!(media.contains("mod.widgets.MediaFigureBase = #(MediaFigure::register_widget(vm))"));
        // Every name this module's DSL reaches for has to already exist
        // when its block runs: a block's `use` only sees what is there,
        // and a qualified name is no better off than a bare one.
        let at = lib.find("crate::media::script_mod(vm);").unwrap();
        for earlier in [
            "crate::view::script_mod(vm);",
            "crate::label::script_mod(vm);",
            "crate::image::script_mod(vm);",
            "crate::placeholder::script_mod(vm);",
        ] {
            assert!(lib.find(earlier).unwrap() < at, "media registers before {earlier}");
        }
    }
}
