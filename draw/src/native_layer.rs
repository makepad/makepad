use crate::{turtle::AlignEntry, Cx2d, NativeLayer, NativeLayerMode, NativeLayerStyle, Rect};

/// Places a native layer alongside the current draw list's GPU draws.
pub trait NativeLayerDraw {
    fn draw(&self, cx: &mut Cx2d, rect: Rect, style: NativeLayerStyle) -> NativeLayerMode;
}

impl NativeLayerDraw for NativeLayer {
    fn draw(&self, cx: &mut Cx2d, rect: Rect, style: NativeLayerStyle) -> NativeLayerMode {
        let Some(draw_list_id) = cx.get_current_draw_list_id() else {
            return NativeLayerMode::Unsupported;
        };
        let pass_id = cx.draw_lists[draw_list_id]
            .draw_pass_id
            .unwrap_or_else(|| cx.current_pass_id());
        let draw_item_id = cx.draw_lists[draw_list_id].draw_items.len();
        let mode = self.draw_in_list(cx, draw_list_id, pass_id, rect, style);
        if cx.draw_lists[draw_list_id].draw_items.len() > draw_item_id {
            // Final turtle alignment and clipping are resolved together with
            // the neighboring GPU instances when this pass-sized turtle ends.
            cx.align_list.push(AlignEntry::NativeLayer {
                draw_list_id,
                draw_item_id,
            });
        }
        mode
    }
}
