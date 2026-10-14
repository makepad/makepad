//! Explicit asynchronous clipboard reads. `Cx::read_clipboard` returns a request id;
//! `Event::Clipboard` delivers all available representations to the requesting widget.
//! Ordinary text-field paste still arrives as `TextInput { was_paste: true }`.

use crate::cx::Cx;
#[cfg(target_arch = "wasm32")]
use crate::event::Event;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum ClipboardImage {
    Png(Arc<Vec<u8>>),
    /// Tightly packed top-down RGBA8 from a Windows DIB.
    Rgba {
        width: u32,
        height: u32,
        rgba: Arc<Vec<u8>>,
    },
}

#[derive(Clone, Debug)]
pub struct ClipboardFile {
    pub url: String,
    /// Decoded native path, as in `DragItem::FilePath`.
    pub path: String,
}

#[derive(Clone, Debug, Default)]
pub struct ClipboardContent {
    pub supported: bool,
    pub text: Option<String>,
    pub image: Option<ClipboardImage>,
    pub files: Vec<ClipboardFile>,
}

#[derive(Clone, Debug)]
pub struct ClipboardEvent {
    pub request: u64,
    pub content: Arc<ClipboardContent>,
}

#[derive(Default)]
pub(crate) struct ClipboardRequests {
    next: u64,
    #[cfg(target_arch = "wasm32")]
    pending: Vec<u64>,
}

impl Cx {
    /// Enqueue a read, returning immediately. Match the id in `Event::Clipboard`;
    /// discard a late reply if its widget/document is no longer the paste destination.
    pub fn read_clipboard(&mut self) -> u64 {
        self.clipboard.next += 1;
        let request = self.clipboard.next;
        // a hosted child reads its host's clipboard (the one every app of the host shares)
        #[cfg(not(target_arch = "wasm32"))]
        if self.hal.hosted.is_some() {
            Cx::send_studio_message(crate::studio::AppToStudio::Relay(crate::studio::ChildRelay::ReadClipboard { request_id: request }));
        } else {
            self.hal.frames.w.clip_read(request, false);
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.clipboard.pending.push(request);
            self.new_next_frame();
        }
        request
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn dispatch_clipboard_reads(&mut self) {
        for request in std::mem::take(&mut self.clipboard.pending) {
            let content = ClipboardContent::default();
            self.call_event_handler(&Event::Clipboard(ClipboardEvent {
                request,
                content: Arc::new(content),
            }));
        }
    }
}
