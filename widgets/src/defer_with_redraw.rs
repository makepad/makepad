use makepad_draw::ui_runner::{UiRunner, DeferCallback};
use crate::Widget;


/// Extension only aviailable when the `UiRunner` is used with a `Widget`.
pub trait DeferWithRedraw<T: 'static> {
    /// Same as `defer` but calls `redraw` on the widget after the closure is run.
    fn defer_with_redraw(self, f: impl DeferCallback<T>);
}

impl<W: Widget + 'static> DeferWithRedraw<W> for UiRunner<W> {
    fn defer_with_redraw(self, f: impl DeferCallback<W>) {
        self.defer(|widget, cx, scope| {
            f(widget, cx, scope);
            widget.redraw(cx);
        });
    }
}
