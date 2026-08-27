//! The page thumbnail strip (Cmd+Shift+T).
//!
//! There is no rasterization step to be expensive here: a thumbnail is the
//! same `PdfPageView` the main view uses, driven at a small zoom over the
//! *same* `Rc<CachedPage>` the document already parsed — no second parse, no
//! offscreen pass, no texture. Its `PortalList` only walks the handful of
//! thumbnails that are actually on screen, so the cost of the strip is
//! roughly "draw four more pages, very small" rather than "draw the
//! document".
//!
//! Two things keep that honest at strip scale: `PdfPageView` already skips
//! text below half a pixel, and the strip is narrow (a Letter page lands
//! near 0.2x), so most vector work collapses to a few pixels of geometry.

use crate::theme::Palette;
use makepad_widgets::*;
use std::rc::Rc;

/// How wide the paper is inside the strip, in pixels.
pub const THUMB_WIDTH: f64 = 118.0;
/// The strip's own width: paper, its frame, the number under it, and room
/// for the scroll bar.
pub const STRIP_WIDTH: f64 = 150.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MpPdfThumbsBase = #(MpPdfThumbs::register_widget(vm))

    mod.widgets.MpPdfThumbs = set_type_default() do mod.widgets.MpPdfThumbsBase{
        width: 150
        height: Fill
        flow: Down

        strip_bg := SolidView{
            width: Fill
            height: Fill
            flow: Down
            draw_bg +: { color: mod.mpp.bg_dark }

            strip := PortalList{
                width: Fill
                height: Fill
                flow: Down

                Thumb := View{
                    width: Fill
                    height: Fit
                    flow: Down
                    spacing: 2
                    align: Align{x: 0.5}
                    padding: Inset{top: 8 bottom: 4}
                    cursor: MouseCursor.Hand
                    new_batch: true

                    frame := SolidView{
                        width: Fit
                        height: Fit
                        padding: 2
                        draw_bg +: { color: mod.mpp.bg_dark }
                        page_view := mod.widgets.PdfPageView{
                            width: 118
                            height: 152
                        }
                    }

                    num := Label{
                        text: ""
                        draw_text +: {
                            color: mod.mpp.dim
                            text_style: theme.font_regular{font_size: 8}
                        }
                    }
                }
            }
        }
    }
}

/// What the strip tells the viewer.
#[derive(Clone, Debug, Default)]
pub enum MpPdfThumbsAction {
    /// A thumbnail was clicked: go to this page (0-based).
    Goto(usize),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct MpPdfThumbs {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    /// The very same parsed pages the document view holds — shared, not
    /// copied, so the strip costs no extra parsing and no extra memory
    /// beyond the pointers.
    #[rust]
    pages: Vec<Rc<CachedPage>>,
    #[rust]
    current: usize,
}

impl Widget for MpPdfThumbs {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        if let Event::Actions(actions) = event {
            let list = self.view.portal_list(cx, ids!(strip));
            for (index, item) in list.items_with_actions(actions) {
                if item.as_view().finger_down(actions).is_some() {
                    cx.widget_action(self.widget_uid(), MpPdfThumbsAction::Goto(index));
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_thumbs(cx, &mut list);
            }
        }
        DrawStep::done()
    }
}

impl MpPdfThumbs {
    /// Share the document's parsed pages with the strip.
    pub fn set_pages(&mut self, cx: &mut Cx, pages: Vec<Rc<CachedPage>>) {
        self.pages = pages;
        self.view.redraw(cx);
    }

    /// Mark `page` (0-based) as the current one and keep it in view.
    pub fn set_current(&mut self, cx: &mut Cx, page: usize) {
        if self.current == page {
            return;
        }
        self.current = page;
        let list = self.view.portal_list(cx, ids!(strip));
        let first = list.first_id();
        let visible = list.visible_items().max(1);
        if page < first || page >= first + visible {
            // Keep one thumbnail of context above the target where there is
            // one, so a jump does not pin the current page to the very top.
            list.set_first_id_and_scroll(page.saturating_sub(1), 0.0);
        }
        self.view.redraw(cx);
    }

    fn draw_thumbs(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        if self.pages.is_empty() {
            return;
        }
        let palette = Palette::shared();
        let accent = palette.accent_vec4();
        let plain = palette.bg_dark_vec4();

        list.set_item_range(cx, 0, self.pages.len());

        while let Some(index) = list.next_visible_item(cx) {
            let Some(page) = self.pages.get(index).cloned() else {
                continue;
            };
            let size = page.size();
            let zoom = THUMB_WIDTH / size.x;
            let height = size.y * zoom;
            let color = if index == self.current { accent } else { plain };
            let number = format!("{}", index + 1);

            // Each apply targets its own widget: `:=` children live in the
            // item's vec, so a nested `frame: {...}` key on the item would
            // write a property nothing reads.
            let mut item = list.item(cx, index, id!(Thumb));
            let mut frame = item.widget(&**cx, ids!(frame));
            script_apply_eval!(cx, frame, {
                draw_bg +: { color: #(color) }
            });
            let mut page_view = item.widget(&**cx, ids!(frame.page_view));
            script_apply_eval!(cx, page_view, {
                width: #(THUMB_WIDTH)
                height: #(height)
            });
            item.label(&**cx, ids!(num)).set_text(&mut **cx, &number);

            while let Some(step) = item.draw(cx, &mut Scope::empty()).step() {
                if let Some(mut page_view) = step.borrow_mut::<PdfPageView>() {
                    page_view.render_page(cx, &page, zoom);
                }
            }
        }
    }
}

impl MpPdfThumbsRef {
    pub fn set_pages(&self, cx: &mut Cx, pages: Vec<Rc<CachedPage>>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_pages(cx, pages);
        }
    }

    pub fn set_current(&self, cx: &mut Cx, page: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_current(cx, page);
        }
    }
}
