use crate::{
    event::{
        Event, VideoBufferedRangesEvent, VideoDecodingErrorEvent, VideoPlaybackCompletedEvent,
        VideoPlaybackPreparedEvent, VideoSeekableRangesEvent, VideoTextureUpdatedEvent,
        VideoYuvTexturesReady,
    },
    makepad_live_id::LiveId,
    texture::{Texture, TextureFormat, TextureId},
    Cx,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct MediaTextureInfo {
    pub width: usize,
    pub height: usize,
}

pub trait MediaTextureBridge {
    fn alloc_yuv_texture(&mut self) -> Texture;
    fn alloc_external_texture(&mut self) -> Texture;
    fn texture_info(&self, texture_id: TextureId) -> Option<MediaTextureInfo>;
}

pub trait MediaEventBridge {
    fn emit_video_prepared(&mut self, event: VideoPlaybackPreparedEvent);
    fn emit_video_texture_updated(&mut self, event: VideoTextureUpdatedEvent);
    fn emit_video_completed(&mut self, event: VideoPlaybackCompletedEvent);
    fn emit_video_error(&mut self, event: VideoDecodingErrorEvent);
    fn emit_video_yuv_ready(&mut self, event: VideoYuvTexturesReady);
    fn emit_video_seekable_ranges(&mut self, event: VideoSeekableRangesEvent);
    fn emit_video_buffered_ranges(&mut self, event: VideoBufferedRangesEvent);
}

pub trait MediaControlBridge {
    fn media_tick(&mut self);
    fn media_handle_video_surface_update(&mut self, _video_id: LiveId) {}
}

impl MediaTextureBridge for Cx {
    fn alloc_yuv_texture(&mut self) -> Texture {
        self.textures.alloc(TextureFormat::VideoYuvPlane)
    }

    fn alloc_external_texture(&mut self) -> Texture {
        self.textures.alloc(TextureFormat::VideoExternal)
    }

    fn texture_info(&self, texture_id: TextureId) -> Option<MediaTextureInfo> {
        let pool_item = self.textures.0.pool.get(texture_id.0)?;
        let alloc = pool_item.item.alloc.as_ref()?;
        Some(MediaTextureInfo {
            width: alloc.width,
            height: alloc.height,
        })
    }
}

impl MediaEventBridge for Cx {
    fn emit_video_prepared(&mut self, event: VideoPlaybackPreparedEvent) {
        self.call_event_handler(&Event::VideoPlaybackPrepared(event));
    }

    fn emit_video_texture_updated(&mut self, event: VideoTextureUpdatedEvent) {
        self.call_event_handler(&Event::VideoTextureUpdated(event));
    }

    fn emit_video_completed(&mut self, event: VideoPlaybackCompletedEvent) {
        self.call_event_handler(&Event::VideoPlaybackCompleted(event));
    }

    fn emit_video_error(&mut self, event: VideoDecodingErrorEvent) {
        self.call_event_handler(&Event::VideoDecodingError(event));
    }

    fn emit_video_yuv_ready(&mut self, event: VideoYuvTexturesReady) {
        self.call_event_handler(&Event::VideoYuvTexturesReady(event));
    }

    fn emit_video_seekable_ranges(&mut self, event: VideoSeekableRangesEvent) {
        self.call_event_handler(&Event::VideoSeekableRanges(event));
    }

    fn emit_video_buffered_ranges(&mut self, event: VideoBufferedRangesEvent) {
        self.call_event_handler(&Event::VideoBufferedRanges(event));
    }
}

impl MediaControlBridge for Cx {
    fn media_tick(&mut self) {}
}
