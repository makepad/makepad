use crate::makepad_script::*;

script_mod! {
    mod.ime = {
        InputMode: mod.std.set_type_default() do #(InputMode::script_api(vm)),
        ..me.InputMode,
        AutoCapitalize: mod.std.set_type_default() do #(AutoCapitalize::script_api(vm)),
        ..me.AutoCapitalize,
        AutoCorrect: mod.std.set_type_default() do #(AutoCorrect::script_api(vm)),
        ..me.AutoCorrect,
        ReturnKeyType: mod.std.set_type_default() do #(ReturnKeyType::script_api(vm)),
        ..me.ReturnKeyType,
        TextInputContentType: mod.std.set_type_default() do #(TextInputContentType::script_api(vm)),
        ..me.TextInputContentType,
    }
}

/// Input mode hint for soft keyboards (matches web standard `inputmode` attribute).
///
/// Supported on iOS and Android. On desktop platforms, this has no effect.
#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq)]
pub enum InputMode {
    None,
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
#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq)]
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
#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq)]
pub enum AutoCorrect {
    #[pick]
    Default,
    Enabled,
    Disabled,
}

/// Return key type - controls the visual appearance and action of the return key.
///
/// Supported on iOS and Android. On desktop platforms, this has no effect.
#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq)]
pub enum ReturnKeyType {
    #[pick]
    Default,
    None,
    Go,
    Google,
    Join,
    Next,
    Route,
    Search,
    Send,
    Yahoo,
    Done,
    EmergencyCall,
    Continue,
    Previous,
}

/// AutoFill / content-type hint for a text field (maps to iOS UITextContentType).
/// Independent of whether the text is obscured. iOS/Android only; no-op on desktop.
#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq)]
pub enum TextInputContentType {
    #[pick]
    None,
    Username,
    Password,
    NewPassword,
    EmailAddress,
    Url,
    FullStreetAddress,
    TelephoneNumber,
    OneTimeCode,
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

impl Default for TextInputContentType {
    fn default() -> Self {
        TextInputContentType::None
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
    pub soft_keyboard: SoftKeyboardConfig,
    pub is_multiline: bool,
    pub is_secure: bool,
    pub submit_on_enter: bool,
    pub content_type: TextInputContentType,
    pub is_read_only: bool,
}

impl TextInputConfig {
    /// True for credential fields (username/password/email) iOS associates with login
    /// AutoFill, which "taint" a reused text view (so we recreate it when leaving one).
    pub fn taints_autofill(&self) -> bool {
        self.is_secure
            || matches!(
                self.content_type,
                TextInputContentType::Username
                    | TextInputContentType::Password
                    | TextInputContentType::NewPassword
                    | TextInputContentType::EmailAddress
            )
    }
}
