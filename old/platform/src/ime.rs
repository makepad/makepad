//! IME (Input Method Editor) Configuration Types
//!
//! This module provides types for configuring soft keyboards on mobile platforms.
//!
//! # Architecture Overview
//!
//! Mobile platforms (iOS/Android) use different approaches for IME integration:
//!
//! ## Android (Bidirectional Sync)
//! - **Source of truth**: Java's `mEditable` (SpannableStringBuilder) in MakepadInputConnection
//! - **Sync direction**: Bidirectional (Java <-> Rust)
//! - **Events**: `ImeTextStateChanged` sends full text + selection + composing region
//! - **Echo detection**: Both sides track last-sent state to avoid sync loops
//! - **Programmatic updates**: Rust->Java via `SyncImeState` (e.g., clear button)
//!
//! ## iOS (Unidirectional Sync)
//! - **Source of truth**: Rust's text buffer in TextInput widget
//! - **Sync direction**: Unidirectional (iOS -> Rust)
//! - **Events**: `TextInput` with `replace_last` flag for composition preview
//! - **Composition**: UITextInput protocol handles marked text inline
//!
//! ## Why They Differ
//! Android's InputConnection requires a Java-side shadow buffer that the IME can query
//! synchronously (getTextBeforeCursor, etc.). iOS's UITextInput is more event-driven.
//!
//! ## Input Filtering
//! Both platforms filter input at the widget layer (TextInput::filter_input).
//! Android additionally filters in Java's commitText() to catch IME bypass cases.
//!
//! For widget-level IME handling details, see `widgets/src/text_input.rs`.

use crate::{
    cx::Cx,
    live_traits::*,
    makepad_derive_live::*,
    makepad_live_compiler::{
        LiveId, LiveModuleId, LiveNode, LiveNodeSliceApi, LiveType, LiveTypeInfo, LiveValue,
    },
    makepad_live_tokenizer::{live_error_origin, LiveErrorOrigin},
};

/// Input mode hint for soft keyboards (matches web standard `inputmode` attribute).
///
/// Supported on iOS and Android. On desktop platforms, this has no effect.
///
/// Variants:
/// - `Text`: Default keyboard for the current locale
/// - `Ascii`: Optimized for ASCII characters (no emoji suggestions)
/// - `Url`: Optimized for URLs (includes /, .com shortcuts)
/// - `Numeric`: Number pad (0-9 only)
/// - `Tel`: Phone number pad (includes *, #)
/// - `Email`: Optimized for email addresses (includes @, shortcuts)
/// - `Decimal`: Decimal number pad (0-9 and decimal point)
/// - `Search`: Optimized for web search queries
#[derive(Clone, Copy, Debug, PartialEq, Live, LiveHook)]
#[live_ignore]
pub enum InputMode {
    #[pick]
    Text,
    Ascii,
    Url,
    Numeric,
    Tel,
    Email,
    Decimal,
    Search,
}

/// Autocapitalization behavior for soft keyboards.
///
/// Supported on iOS and Android. On desktop platforms, this has no effect.
///
/// Variants:
/// - `None`: No automatic capitalization
/// - `Words`: Capitalize the first letter of each word
/// - `Sentences`: Capitalize the first letter of each sentence (default)
/// - `AllCharacters`: Capitalize all characters (shift lock)
#[derive(Clone, Copy, Debug, PartialEq, Live, LiveHook)]
#[live_ignore]
pub enum AutoCapitalize {
    None,
    Words,
    #[pick]
    Sentences,
    AllCharacters,
}

/// Autocorrection behavior for soft keyboards.
///
/// Supported on iOS and Android. On desktop platforms, this has no effect.
///
/// Variants:
/// - `Default`: Use system default (typically enabled)
/// - `Enabled`: Force enable autocorrection
/// - `Disabled`: Force disable autocorrection (useful for code, usernames, etc.)
#[derive(Clone, Copy, Debug, PartialEq, Live, LiveHook)]
#[live_ignore]
pub enum AutoCorrect {
    #[pick]
    Default,
    Enabled,
    Disabled,
}

/// Return key type - controls the visual appearance and action of the return key.
///
/// Supported on iOS and Android. On desktop platforms, this has no effect.
/// The actual action button text depends on the platform and locale.
#[derive(Clone, Copy, Debug, PartialEq, Live, LiveHook)]
#[live_ignore]
pub enum ReturnKeyType {
    #[pick]
    Default,
    /// "Go" action, typically for URL fields, dismisses keyboard.
    Go,
    /// "Search" action, typically for search fields, dismisses keyboard.
    Search,
    /// "Send" action, typically for messaging, dismisses keyboard.
    Send,
    // TODO: Implement Next once we have a way to know which field is next
    // Next,
    /// "Done" action, typically for forms, dismisses keyboard.
    Done,
}

impl Default for InputMode {
    fn default() -> Self {
        InputMode::Text
    }
}

impl Default for AutoCapitalize {
    fn default() -> Self {
        AutoCapitalize::Sentences
    }
}

impl Default for AutoCorrect {
    fn default() -> Self {
        AutoCorrect::Default
    }
}

impl Default for ReturnKeyType {
    fn default() -> Self {
        ReturnKeyType::Default
    }
}

/// Soft keyboard configuration for mobile platforms (iOS/Android).
/// These settings have no effect on desktop platforms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SoftKeyboardConfig {
    pub input_mode: InputMode,
    pub autocapitalize: AutoCapitalize,
    pub autocorrect: AutoCorrect,
    pub return_key_type: ReturnKeyType,
}

/// Text input configuration combining cross-platform and mobile-specific settings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextInputConfig {
    /// Soft keyboard settings for mobile platforms. No effect on desktop.
    pub soft_keyboard: SoftKeyboardConfig,
    /// Whether the input field supports multiple lines.
    pub is_multiline: bool,
    /// Whether to mask input characters (password field).
    pub is_secure: bool,
}
