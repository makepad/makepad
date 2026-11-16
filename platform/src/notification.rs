//! OS notifications: `Cx::post_notification` posts or replaces a notification by id,
//! `Cx::remove_notification` removes it, and a click arrives as `Event::NotificationClick`.
//!
//! There is no permission API: the OS asks the person at the first post. A refusal means
//! nothing shows, logged once by the HAL. macOS needs an .app bundle with a bundle identifier;
//! otherwise posts are dropped, logged once. Hosted apps (the makepad WM) and backends without
//! notifications drop them for now.

use crate::{cx::Cx, cx_api::CxOsOp, makepad_live_id::LiveId};

#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub id: LiveId,
    pub title: String,
    pub body: String,
}

/// The person clicked a notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotificationClickEvent {
    pub id: LiveId,
}

impl Cx {
    /// Posts a notification, replacing the one already shown with the same id.
    pub fn post_notification(&mut self, n: Notification) {
        self.platform_ops.push_back(CxOsOp::PostNotification(n));
    }

    pub fn remove_notification(&mut self, id: LiveId) {
        self.platform_ops.push_back(CxOsOp::RemoveNotification(id));
    }
}
