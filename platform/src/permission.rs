#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Permission {
    /// Permission to access the microphone for audio input.
    /// 
    /// Required on: iOS, Android, macOS, Web
    /// Auto-granted on: Windows, Linux
    AudioInput,
    // Future permissions to be added here (Camera, Location, etc.)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PermissionStatus {
    /// Permission has been granted by the user.
    /// The app can freely use the requested functionality.
    Granted,
    
    /// Permission status has not been determined yet (first time asking).
    /// This typically occurs before the user has been prompted for the permission.
    /// The app should request the permission to show the system dialog.
    NotDetermined,
    
    /// Permission was denied but can be requested again.
    /// 
    /// **Android behavior:**
    /// - User denied the permission once but didn't trigger the "implicit don't ask again"
    /// - The app can show a rationale explanation and request again, 
    ///   e.g. "We need microphone access so you can record voice notes. Please allow access."
    /// - Modern Android (11+) automatically sets "don't ask again" after 2 denials
    /// 
    /// **iOS/macOS and Web behavior:**
    /// - This status is not used on Apple platforms
    /// - These platforms go directly from NotDetermined to DeniedPermanent
    DeniedCanRetry,
    
    /// Permission was permanently denied and cannot be requested again.
    /// The user must manually grant the permission in system settings.
    /// 
    /// **Android behavior:**
    /// - User selected "Don't ask again" (older Android) or triggered implicit denial (Android 11+)
    /// - After 2 denials on modern Android, the system stops showing permission dialogs
    /// - App should guide user to Settings > Apps > [App Name] > Permissions
    /// 
    /// **iOS/macOS behavior:**
    /// - User denied the permission once (Apple platforms don't re-prompt)
    /// - App should guide user to Settings > Privacy & Security > [Permission Type]
    /// 
    /// **Web behavior:**
    /// - User denied the permission (browsers typically don't re-prompt)
    /// - User must grant permission through browser settings (usually in URL bar)
    /// 
    /// **Desktop (Windows/Linux) behavior:**
    /// - Not applicable - desktop apps typically have all permissions granted by default
    DeniedPermanent,
}

#[derive(Debug, Clone)]
pub struct PermissionResult {
    pub permission: Permission,
    pub request_id: i32,
    pub status: PermissionStatus,
}
