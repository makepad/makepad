//! AppKit accessibility notifications. Role strings come from the semantic role table.
use crate::ObjcId;

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    pub fn NSAccessibilityPostNotification(element: ObjcId, notification: ObjcId);
    pub fn NSAccessibilityPostNotificationWithUserInfo(
        element: ObjcId,
        notification: ObjcId,
        info: ObjcId,
    );
    pub fn NSAccessibilityRoleDescription(role: ObjcId, subrole: ObjcId) -> ObjcId;
}

macro_rules! constants {
    ($($name:ident),* $(,)?) => {
        #[link(name = "AppKit", kind = "framework")]
        extern "C" { $(pub static $name: ObjcId;)* }
    };
}

constants! {
    NSAccessibilityFocusedUIElementChangedNotification,
    NSAccessibilityValueChangedNotification,
    NSAccessibilityTitleChangedNotification,
    NSAccessibilityLayoutChangedNotification,
    NSAccessibilitySelectedTextChangedNotification,
    NSAccessibilityUIElementDestroyedNotification,
    NSAccessibilityRowExpandedNotification,
    NSAccessibilityRowCollapsedNotification,
    NSAccessibilityAnnouncementRequestedNotification,
    NSAccessibilityAnnouncementKey,
    NSAccessibilityPriorityKey,
}
