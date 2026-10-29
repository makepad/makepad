//! Public UIKit accessibility functions and constants (UIAccessibility.h / Constants.h).
use crate::{NSRect, ObjcId, BOOL};

#[link(name = "UIKit", kind = "framework")]
extern "C" {
    pub fn UIAccessibilityIsVoiceOverRunning() -> BOOL;
    pub fn UIAccessibilityIsSwitchControlRunning() -> BOOL;
    pub fn UIAccessibilityConvertFrameToScreenCoordinates(rect: NSRect, view: ObjcId) -> NSRect;
    pub fn UIAccessibilityPostNotification(notification: u32, argument: ObjcId);

    pub static UIAccessibilityVoiceOverStatusDidChangeNotification: ObjcId;
    pub static UIAccessibilitySwitchControlStatusDidChangeNotification: ObjcId;
    pub static UIAccessibilityScreenChangedNotification: u32;
    pub static UIAccessibilityLayoutChangedNotification: u32;
    pub static UIAccessibilityAnnouncementNotification: u32;

    pub static UIAccessibilityTraitButton: u64;
    pub static UIAccessibilityTraitLink: u64;
    pub static UIAccessibilityTraitHeader: u64;
    pub static UIAccessibilityTraitSearchField: u64;
    pub static UIAccessibilityTraitImage: u64;
    pub static UIAccessibilityTraitSelected: u64;
    pub static UIAccessibilityTraitKeyboardKey: u64;
    pub static UIAccessibilityTraitStaticText: u64;
    pub static UIAccessibilityTraitNotEnabled: u64;
    pub static UIAccessibilityTraitUpdatesFrequently: u64;
    pub static UIAccessibilityTraitAdjustable: u64;
    pub static UIAccessibilityTraitTabBar: u64;
}
