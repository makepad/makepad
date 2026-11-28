//! Structural editing accepts token-equivalent source, including changes to string contents.
use crate::{makepad_draw::Cx, widget::WidgetRef};
use makepad_widgets_core::design_data::locate::{self, LocateMode};
pub use makepad_widgets_core::design_data::locate::{widget_source_file, NodeSpan};

pub fn locate_widget(cx: &mut Cx, widget: &WidgetRef, text: Option<&str>) -> Result<NodeSpan, String> {
    locate::locate_widget(cx, widget, text, LocateMode::Lenient)
}
