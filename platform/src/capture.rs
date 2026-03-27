use crate::DrawPassId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaptureSource {
    Framebuffer,
    CachedView { draw_pass_id: DrawPassId },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureRequest {
    pub request_id: u64,
    pub source: CaptureSource,
}

#[derive(Debug, Clone)]
pub struct CaptureResult {
    pub request_id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
