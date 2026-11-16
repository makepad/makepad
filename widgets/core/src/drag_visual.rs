//! A window overlay that carries a widget at the pointer during an in-app drag.
//! Each drag move redraws one overlay list in the window under the pointer; there
//! are no redraws outside a drag. Use it for strings and items with an `internal_id`.

use crate::{makepad_draw::*, widget::WidgetRef};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DragChip = mod.widgets.RoundedView{
        width: Fit
        height: Fit
        flow: Right
        align: Align{y: 0.5}
        padding: Inset{left: 8 right: 8 top: 5 bottom: 5}
        spacing: 6
        draw_bg +: {
            color: theme.color_fg_app
            border_color: theme.color_bevel
            border_size: 1.0
            border_radius: 4.0
        }
        swatch := RoundedView{
            width: 8
            height: 8
            visible: false
            draw_bg +: {
                color: theme.color_mark_active
                border_radius: 4.0
            }
        }
        label := Label{
            text: ""
            draw_text +: {
                color: theme.color_label_inner
                text_style: theme.font_regular{font_size: theme.font_size_p}
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct DragVisualState {
    pub(crate) visual: Option<WidgetRef>,
    pub(crate) offset: DVec2,
    pub(crate) pointer: Option<(WindowId, DVec2)>,
}

pub trait CxDragVisualExt {
    /// Draws the visual at the pointer plus `offset` in the window under the
    /// pointer, from the first drag move to the drop or the end.
    fn start_dragging_with(&mut self, items: Vec<DragItem>, visual: WidgetRef, offset: DVec2);

    /// Returns the running drag's visual for changes such as a target label.
    /// Widget setters such as a label's `set_text` redraw it.
    fn drag_visual(&mut self) -> Option<WidgetRef>;
}

impl CxDragVisualExt for Cx {
    fn start_dragging_with(&mut self, items: Vec<DragItem>, visual: WidgetRef, offset: DVec2) {
        let state = self.global::<DragVisualState>();
        state.visual = Some(visual);
        state.offset = offset;
        state.pointer = None;
        self.start_dragging(items);
    }

    fn drag_visual(&mut self) -> Option<WidgetRef> {
        self.global::<DragVisualState>().visual.clone()
    }
}
