//! Permissions: the microphone and camera authorization of AVFoundation (location's is in
//! `geo`), checked and asked for. Moved from os/apple/macos/macos.rs (`handle_permission_*`).
//! iOS asks through the same `AVCaptureDevice` calls (the microphone's status and prompt are
//! the record permission of the audio session).

use super::Signal;
use crate::media::{permission_kind as kind, permission_status as status};
use makepad_apple_sys::*;
use std::sync::{Arc, Mutex};

/// Answers waiting for the session's step: (request id, kind, status).
pub type Answers = Arc<Mutex<Vec<(u32, u32, u32)>>>;

fn av_status(media_type: ObjcId) -> u32 {
    unsafe {
        let s: i32 = msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: media_type];
        match s {
            3 => status::GRANTED,                // AVAuthorizationStatusAuthorized
            2 | 1 => status::DENIED_PERMANENT,   // Denied / Restricted: macOS doesn't re-prompt
            _ => status::NOT_DETERMINED,         // NotDetermined (0) or unknown
        }
    }
}

/// The current status of a permission.
pub fn check(permission: u32) -> u32 {
    match permission {
        kind::AUDIO_INPUT => av_status(unsafe { AVMediaTypeAudio }),
        kind::CAMERA => av_status(unsafe { AVMediaTypeVideo }),
        #[cfg(not(target_os = "tvos"))]
        kind::LOCATION => super::geo::location_permission_status(),
        // headset cameras and scene data are Quest features
        _ => status::DENIED_PERMANENT,
    }
}

/// Asks the user for the microphone or the camera; the answer lands in `answers`.
pub fn request_av(permission: u32, req: u32, answers: Answers, signal: Signal) {
    let media_type = unsafe {
        if permission == kind::AUDIO_INPUT {
            AVMediaTypeAudio
        } else {
            AVMediaTypeVideo
        }
    };
    unsafe {
        // the completion handler runs on an arbitrary thread
        let completion_handler = objc_block!(move |granted: BOOL| {
            let result = if granted == YES { status::GRANTED } else { status::DENIED_PERMANENT };
            answers.lock().unwrap_or_else(|e| e.into_inner()).push((req, permission, result));
            signal.set();
        });
        let () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: media_type completionHandler: &completion_handler];
    }
}
