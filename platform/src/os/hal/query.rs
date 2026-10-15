//! The app's answers to the automation runner's `Query` events (makepad-hal `automation`). The
//! runner knows no app: it asks for `widgets` and gets the widget tree the remote `/snap` route
//! reports, one line per widget (`Row`), whatever the app is.

use crate::{cx::Cx, event::hal_intake::hal_window_id};
use makepad_hal::automation::Row;

impl Cx {
    /// Answers one request with a `QueryReply` command (an unknown request gets `error: ...`).
    pub(crate) fn hal_answer_query(&mut self, req: u32, request: &[u8]) {
        let request = String::from_utf8_lossy(request);
        let reply = match request.trim() {
            "widgets" => self.hal_widget_rows(),
            other => format!("error: unknown request `{other}` (the app answers: widgets)"),
        };
        self.hal.frames.w.query_reply(req, reply.as_bytes());
    }

    /// The widget rows, window-local; a widget the widgets layer gave no path shows its id.
    fn hal_widget_rows(&self) -> String {
        let rows: Vec<(makepad_studio_protocol::WidgetSnapshot, Option<String>)> = if let Some(callback) = self.widget_paths_callback {
            callback(self).into_iter().map(|(w, p)| (w, Some(p))).collect()
        } else if let Some(callback) = self.widget_snapshot_callback {
            callback(self).into_iter().map(|w| (w, None)).collect()
        } else {
            Vec::new()
        };
        let mut out = String::new();
        for (w, path) in rows {
            let window = self.windows.id_iter().find(|id| id.id() == w.window_index && self.hal.window_ready(*id));
            let origin = window.map(|id| self.windows[id].window_geom.position).unwrap_or_default();
            // Rows are in layout points; the automation injects pointer input in the window's
            // native points. Under a dpi override (a WM display scale) the two differ by a
            // fixed factor, so rows are reported in the injection space.
            let to_native = window.map(|id| self.windows[id].effective_dpi_factor() / self.windows[id].native_dpi_factor()).unwrap_or(1.0);
            let row = Row {
                path: path.filter(|p| !p.is_empty()).unwrap_or_else(|| w.id.clone()),
                kind: w.widget_type.to_string(),
                visible: w.visible,
                x: (w.x as f64 - origin.x) * to_native,
                y: (w.y as f64 - origin.y) * to_native,
                w: w.width as f64 * to_native,
                h: w.height as f64 * to_native,
                window: window.map(hal_window_id),
                text: w.text.as_deref().or(w.value.as_deref()).unwrap_or("").to_string(),
            };
            out.push_str(&row.encode());
            out.push('\n');
        }
        out
    }
}
