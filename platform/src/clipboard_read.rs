//! Explicit asynchronous clipboard reads. `Cx::read_clipboard` returns a request id;
//! `Event::Clipboard` delivers all available representations to the requesting widget.
//! Ordinary text-field paste still arrives as `TextInput { was_paste: true }`.
//! A copy writes its text with richer flavours beside it (`ClipboardWrite`).

use crate::cx::Cx;
use crate::window_controls::HostedClipboard;
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

/// What a copy puts on the clipboard: its plain text, and the same content in the richer
/// flavours a paste target may prefer. A flavour left `None` is not offered; the text always is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipboardWrite {
    pub text: String,
    /// An HTML fragment (`text/html`).
    pub html: Option<String>,
    /// Markdown source (`text/markdown`).
    pub markdown: Option<String>,
}

impl ClipboardWrite {
    /// Plain text alone.
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Default::default() }
    }

    /// The flavours as the hosted message that goes ahead of the text (`request` 0 for a copy).
    pub fn hosted(&self, request: u64) -> HostedClipboard {
        HostedClipboard {
            request,
            text: self.text.clone(),
            html: self.html.clone().unwrap_or_default(),
            markdown: self.markdown.clone().unwrap_or_default(),
        }
    }

    /// `text` with the flavours a hosted message brought for it (none when they went with
    /// another text).
    pub fn with_hosted(text: String, flavours: Option<HostedClipboard>) -> Self {
        let flavour = |s: String| (!s.is_empty()).then_some(s);
        match flavours.filter(|f| f.text == text) {
            Some(f) => Self { text, html: flavour(f.html), markdown: flavour(f.markdown) },
            None => Self::text(text),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ClipboardContent {
    pub supported: bool,
    pub text: Option<String>,
    /// The text's HTML flavour (`text/html`), where the clipboard offers one.
    pub html: Option<String>,
    /// The text's Markdown flavour (`text/markdown`), where the clipboard offers one.
    pub markdown: Option<String>,
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
    /// A hosted child's: the flavours its host sent ahead of a read's answer.
    pub(crate) hosted_flavours: Option<HostedClipboard>,
    #[cfg(target_arch = "wasm32")]
    pending: Vec<u64>,
    /// On the web: what the last paste event's clipboard held, for the reads that paste asked for
    /// (a page reads the clipboard only in a paste event).
    #[cfg(target_arch = "wasm32")]
    pub(crate) web_paste: Option<ClipboardContent>,
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

    /// A hosted child's copy goes to its host's clipboard: the flavours first, then the text the
    /// studio protocol carries (a host that does not know the flavours takes the text alone).
    pub(crate) fn send_hosted_clipboard(write: ClipboardWrite) {
        let flavours = write.hosted(0);
        if flavours.any() {
            Cx::send_studio_message(crate::studio::AppToStudio::Custom(flavours.to_json()));
        }
        Cx::send_studio_message(crate::studio::AppToStudio::SetClipboard(write.text));
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn dispatch_clipboard_reads(&mut self) {
        let pending = std::mem::take(&mut self.clipboard.pending);
        if pending.is_empty() {
            return;
        }
        let content = Arc::new(self.clipboard.web_paste.take().unwrap_or_default());
        for request in pending {
            self.call_event_handler(&Event::Clipboard(ClipboardEvent { request, content: content.clone() }));
        }
    }
}
