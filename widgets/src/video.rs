use crate::{
    makepad_derive_widget::*, makepad_draw::*, makepad_platform::event::video_decoding::*,
    widget::*, VideoColorFormat,
};
use std::{
    ops::Range,
    sync::mpsc::channel,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const MAX_FRAMES_TO_DECODE: usize = 20;
const FRAME_BUFFER_LOW_WATER_MARK: usize = MAX_FRAMES_TO_DECODE / 3;

// Usage
// is_looping - determines if the video should be played in a loop. defaults to false.
// hold_to_pause - determines if the video should be paused when the user hold the pause button. defaults to false.
// autoplay - determines if the video should start playback when the widget is created. defaults to false.

live_design! {
    VideoBase = {{Video}} {}
}

// TODO:

// - Add audio playback
// - Add support for SemiPlanar nv21, currently we assume that SemiPlanar is nv12
// - Add function to restart playback manually when not looping.

// - Optimizations:
//      - determine frame chunk size based on memory usage: minimal amount of frames to keep in memory for smooth playback considering their size
//      - we're allocating new vec and copying data from java into rust when decoding, if we need to we could have a shared memory buffer between them, but that
//        introduces a lot of complexity.


#[derive(Live)]
pub struct Video {
    // Drawing
    #[live]
    draw_bg: DrawColor,
    #[walk]
    walk: Walk,
    #[live]
    layout: Layout,
    #[live]
    scale: f64,

    // Source and textures
    #[live]
    source: LiveDependency,
    #[rust]
    textures: [Option<Texture>; 3],

    // Playback
    #[live(false)]
    is_looping: bool,
    #[live(false)]
    hold_to_pause: bool,
    #[live(false)]
    autoplay: bool,
    #[rust]
    playback_state: PlaybackState,
    #[rust]
    pause_time: Option<Instant>,
    #[rust]
    total_pause_duration: Duration,

    // Original video metadata
    #[rust]
    video_width: usize,
    #[rust]
    video_height: usize,
    #[rust]
    total_duration: u128,
    #[rust]
    original_frame_rate: usize,
    #[rust]
    color_format: VideoColorFormat,

    // Buffering
    #[rust]
    frames_buffer: SharedFrameBuffer,
    #[rust]
    tmp_recycled_vec: Vec<u8>,

    // Frame
    #[rust]
    is_current_texture_preview: bool,
    #[rust]
    next_frame_ts: u128,
    #[rust]
    frame_ts_interval: f64,
    #[rust]
    start_time: Option<Instant>,
    #[rust]
    tick: Timer,

    // Decoding
    #[rust]
    decoding_receiver: ToUIReceiver<Vec<u8>>,
    #[rust]
    decoding_state: DecodingState,
    // #[rust]
    // vec_pool: SharedVecPool,
    #[rust]
    available_to_fetch: bool,

    #[rust]
    id: LiveId,
}

#[derive(Clone, Default, PartialEq, WidgetRef)]
pub struct VideoRef(WidgetRef);

impl VideoRef {
    pub fn begin_decoding(&mut self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.initialize_decoding(cx);
        }
    }

    // it will initialize decoding if not already initialized
    pub fn show_preview(&mut self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_preview(cx);
        }
    }

    // it will initialize decoding if not already initialized
    pub fn begin_playback(&mut self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.begin_playback(cx);
        }
    }

    pub fn pause_playback(&self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.pause_playback();
        }
    }

    pub fn resume_playback(&self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.resume_playback();
        }
    }

    // it will finish playback and cleanup decoding
    pub fn end_playback(&mut self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.end_playback(cx);
        }
    }
}

#[derive(Clone, Default, WidgetSet)]
pub struct VideoSet(WidgetSet);

impl VideoSet {}

#[derive(Default, PartialEq)]
enum DecodingState {
    #[default]
    NotStarted,
    Initializing,
    Initialized,
    Decoding,
    ChunkFinished,
}

#[derive(Default, PartialEq, Debug)]
enum PlaybackState {
    #[default]
    NotStarted,
    Previewing,
    Playing,
    Paused,
    Finished,
}

impl LiveHook for Video {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, Video);
    }

    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.id = LiveId::unique();
        if self.autoplay {
            self.begin_playback(cx);
        }
    }
}

#[derive(Clone, WidgetAction)]
pub enum VideoAction {
    None,
}

impl Widget for Video {
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_bg.draw_walk(cx, walk);
        WidgetDraw::done()
    }

    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem),
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut |cx, action| {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid));
        });
    }
}

impl Video {
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, VideoAction),
    ) {
        if let Event::VideoDecodingInitialized(event) = event {
            if event.video_id == self.id {
                self.handle_decoding_initialized(cx, event);
            }
        }

        if let Event::VideoChunkDecoded(video_id) = event {
            if *video_id == self.id {
                self.decoding_state = DecodingState::ChunkFinished;
                self.available_to_fetch = true;
            }
        }

        if self.tick.is_event(event).is_some() {
            self.maybe_show_preview(cx);
            self.maybe_advance_playback(cx);

            if self.should_fetch() {
                self.available_to_fetch = false;
                cx.fetch_next_video_frames(self.id, MAX_FRAMES_TO_DECODE);
            } else if self.should_request_decoding() {
                let frames_to_decode = if self.playback_state == PlaybackState::Previewing {
                    1
                } else {
                    MAX_FRAMES_TO_DECODE
                };
                cx.decode_next_video_chunk(self.id, frames_to_decode);
                self.decoding_state = DecodingState::Decoding;
            }
        }

        self.handle_gestures(cx, event);
        self.handle_activity_events(event);
        self.handle_errors(event);
    }

    fn initialize_decoding(&mut self, cx: &mut Cx) {
        if self.decoding_state == DecodingState::NotStarted {
            match cx.get_dependency(self.source.as_str()) {
                Ok(data) => {
                    cx.initialize_video_decoding(self.id, data, 60);
                    self.decoding_state = DecodingState::Initializing;
                }
                Err(e) => {
                    error!(
                        "initialize_decoding: resource not found {} {}",
                        self.source.as_str(),
                        e
                    );
                }
            }
        }
    }

    fn handle_decoding_initialized(&mut self, cx: &mut Cx, event: &VideoDecodingInitializedEvent) {
        self.decoding_state = DecodingState::Initialized;
        self.video_width = event.video_width as usize;
        self.video_height = event.video_height as usize;
        self.original_frame_rate = event.frame_rate;
        self.total_duration = event.duration;
        self.color_format = event.color_format;
        self.frame_ts_interval = 1000000.0 / self.original_frame_rate as f64;

        let is_plannar = if self.color_format == VideoColorFormat::YUV420Planar {
            1.0
        } else {
            0.0
        };
        self.draw_bg.set_uniform(cx, id!(is_plannar), &[is_plannar]);
        self.draw_bg
            .set_uniform(cx, id!(video_height), &[self.video_height as f32]);
        self.draw_bg
            .set_uniform(cx, id!(video_width), &[self.video_width as f32]);

        // Debug
        // makepad_error_log::log!(
        //     "Video id {} - decoding initialized: \n {}x{}px | {} FPS | Color format: {:?} | Timestamp interval: {:?}",
        //     self.id.0,
        //     self.video_width,
        //     self.video_height,
        //     self.original_frame_rate,
        //     self.color_format,
        //     self.frame_ts_interval
        // );

        cx.decode_next_video_chunk(self.id, MAX_FRAMES_TO_DECODE + MAX_FRAMES_TO_DECODE / 2);
        self.decoding_state = DecodingState::Decoding;

        self.begin_buffering_thread(cx);
        self.tick = cx.start_interval(8.0);
    }

    fn begin_buffering_thread(&mut self, cx: &mut Cx) {
        let video_sender = self.decoding_receiver.sender();
        cx.video_decoding_input(self.id, move |data| {
            let _ = video_sender.send(data);
        });

        let frames_buffer = Arc::clone(&self.frames_buffer);

        let (_new_sender, new_receiver) = channel();
        let old_receiver = std::mem::replace(&mut self.decoding_receiver.receiver, new_receiver);

        thread::spawn(move || loop {
            let mut frame_group = old_receiver.recv().unwrap();
            let mut buffer = frames_buffer.lock().unwrap();
            buffer.append(&mut frame_group);
        });
    }

    fn maybe_show_preview(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Previewing && !self.is_current_texture_preview {
            let frame_metadata = self.parse_next_frame_metadata();
            self.update_textures(cx, &frame_metadata);
            self.is_current_texture_preview = true;

            self.draw_bg.set_uniform(cx, id!(is_last_frame), &[0.0]);
            self.draw_bg.set_uniform(cx, id!(texture_available), &[1.0]);
            self.redraw(cx);
        }
    }

    fn maybe_advance_playback(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Playing {
            let now = Instant::now();
            let video_time_us = match self.start_time {
                Some(start_time) => now.duration_since(start_time).as_micros(),
                None => 0,
            };

            if video_time_us >= self.next_frame_ts || self.start_time.is_none() {
                if self.frames_buffer.lock().unwrap().is_empty() {
                    return;
                }

                let frame_metadata = self.parse_next_frame_metadata();
                self.update_textures(cx, &frame_metadata);

                if self.start_time.is_none() {
                    self.start_time = Some(now);
                    self.draw_bg.set_uniform(cx, id!(is_last_frame), &[0.0]);
                    self.draw_bg.set_uniform(cx, id!(texture_available), &[1.0]);
                }
                self.redraw(cx);

                // if at the last frame, loop or stop
                if frame_metadata.is_eos {
                    self.next_frame_ts = 0;
                    self.start_time = None;
                    if !self.is_looping {
                        self.draw_bg.set_uniform(cx, id!(is_last_frame), &[1.0]);
                        self.playback_state = PlaybackState::Finished;
                    }
                } else {
                    self.next_frame_ts =
                        frame_metadata.timestamp + self.frame_ts_interval.ceil() as u128;
                }
            }
        }
    }

    fn parse_next_frame_metadata(&self) -> FrameMetadata {
        let mut frame_buffer = self.frames_buffer.lock().unwrap();
        // | Timestamp (8B)  | Y Stride (4B) | U Stride (4B) | V Stride (4B) | isEoS (1B) | Pixel data length (4b) | Pixel Data |

        if frame_buffer.len() < 25 {
            panic!("Insufficient data to parse frame metadata");
        }

        // might have to update for different endianness depending of the platform
        let timestamp = u64::from_be_bytes(frame_buffer[0..8].try_into().unwrap()) as u128;

        let y_stride = u32::from_be_bytes(frame_buffer[8..12].try_into().unwrap()) as usize;
        let u_stride = u32::from_be_bytes(frame_buffer[12..16].try_into().unwrap()) as usize;
        let v_stride = u32::from_be_bytes(frame_buffer[16..20].try_into().unwrap()) as usize;

        let is_eos = frame_buffer[20] != 0;

        let frame_length = u32::from_be_bytes(frame_buffer[21..25].try_into().unwrap()) as usize;
        let frame_range = 0..frame_length;

        // Drain the metadata from the buffer
        frame_buffer.drain(0..25);

        FrameMetadata {
            timestamp,
            y_stride,
            u_stride,
            v_stride,
            frame_range,
            is_eos,
        }
    }

    fn update_textures(&mut self, cx: &mut Cx, frame_metadata: &FrameMetadata) {
        let range = &frame_metadata.frame_range;
        let y_stride = frame_metadata.y_stride;
        let u_stride = frame_metadata.u_stride;
        let v_stride = frame_metadata.v_stride;

        self.draw_bg
            .set_uniform(cx, id!(y_stride), &[y_stride as f32]);
        self.draw_bg
            .set_uniform(cx, id!(u_stride), &[u_stride as f32]);
        self.draw_bg
            .set_uniform(cx, id!(v_stride), &[v_stride as f32]);

        match self.color_format {
            VideoColorFormat::YUV420Planar => {
                // y
                let y_plane_range = range.start..range.start + y_stride * self.video_height;
                self.drain_frame_buffer(y_plane_range);
                self.update_texture(cx, 0, TextureFormat::ImageR8, y_stride);

                // u
                let u_plane_start = range.start; // + y_stride * self.video_height;
                let u_plane_range =
                    u_plane_start..u_plane_start + u_stride * (self.video_height / 2);
                self.drain_frame_buffer(u_plane_range);
                self.update_texture(cx, 1, TextureFormat::ImageR8, u_stride);

                // v
                let v_plane_start = u_plane_start; //+ u_stride * (self.video_height / 2);
                let v_plane_range =
                    v_plane_start..v_plane_start + v_stride * (self.video_height / 2);
                self.drain_frame_buffer(v_plane_range);
                self.update_texture(cx, 2, TextureFormat::ImageR8, v_stride);
            }
            VideoColorFormat::YUV420SemiPlanar => {
                // y
                let y_plane_range = range.start..range.start + y_stride * self.video_height;
                self.drain_frame_buffer(y_plane_range);
                self.update_texture(cx, 0, TextureFormat::ImageR8, y_stride);

                // uv
                let uv_plane_size = y_stride * self.video_height / 2;
                let uv_plane_range = 0..uv_plane_size;
                self.drain_frame_buffer(uv_plane_range);
                self.update_texture(cx, 1, TextureFormat::ImageRG8, u_stride);
            }
            _ => todo!(),
        }
    }

    fn drain_frame_buffer(&mut self, range: Range<usize>) {
        let mut frame_buffer = self.frames_buffer.lock().unwrap();
        self.tmp_recycled_vec.clear();
        self.tmp_recycled_vec.extend(frame_buffer.drain(range));
    }

    fn update_texture(&mut self, cx: &mut Cx, slot: usize, format: TextureFormat, stride: usize) {
        if self.textures[slot].is_none() {
            let new_texture = Texture::new(cx);
            let (width, height) = match (self.color_format, slot) {
                (VideoColorFormat::YUV420Planar, 0) | (VideoColorFormat::YUV420SemiPlanar, 0) => {
                    (self.video_width, self.video_height)
                }
                (VideoColorFormat::YUV420Planar, _) => {
                    (self.video_width / 2, self.video_height / 2)
                }
                (VideoColorFormat::YUV420SemiPlanar, 1) => {
                    (self.video_width, self.video_height / 2)
                }
                _ => panic!("Unsupported color format or slot"),
            };
            new_texture.set_desc(
                cx,
                TextureDesc {
                    format,
                    width: Some(width),
                    height: Some(height),
                    mipmapping: false,
                    unpack_row_length: Some(stride),
                },
            );
            self.textures[slot] = Some(new_texture);
        }

        let texture = self.textures[slot].as_mut().unwrap();
        texture.swap_image_u8(cx, &mut self.tmp_recycled_vec);

        self.draw_bg.draw_vars.set_texture(slot, &texture);
    }

    fn handle_gestures(&mut self, cx: &mut Cx, event: &Event) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.hold_to_pause {
                    self.pause_playback();
                }
            }
            Hit::FingerUp(_fe) => {
                if self.hold_to_pause {
                    self.resume_playback();
                }
            }
            _ => (),
        }
    }

    fn handle_activity_events(&mut self, event: &Event) {
        match event {
            Event::Pause => self.pause_playback(),
            Event::Resume => self.resume_playback(),
            _ => (),
        }
    }

    fn handle_errors(&mut self, event: &Event) {
        if let Event::VideoDecodingError(event) = event {
            if event.video_id == self.id {
                error!(
                    "Error decoding video with id {} : {}",
                    self.id.0, event.error
                );
            }
        }
    }

    fn show_preview(&mut self, cx: &mut Cx) {
        if self.playback_state != PlaybackState::Previewing {
            if self.decoding_state == DecodingState::NotStarted {
                self.initialize_decoding(cx);
            }
            self.playback_state = PlaybackState::Previewing;
        }
    }

    fn begin_playback(&mut self, cx: &mut Cx) {
        if self.decoding_state == DecodingState::NotStarted {
            self.initialize_decoding(cx);
        }
        self.playback_state = PlaybackState::Playing;
    }

    fn pause_playback(&mut self) {
        if self.playback_state != PlaybackState::Paused {
            self.pause_time = Some(Instant::now());
            self.playback_state = PlaybackState::Paused;
        }
    }

    fn resume_playback(&mut self) {
        if let Some(pause_time) = self.pause_time.take() {
            let pause_duration = Instant::now().duration_since(pause_time);
            self.total_pause_duration += pause_duration;
            if let Some(start_time) = self.start_time.as_mut() {
                *start_time += pause_duration;
            }
        }
        self.playback_state = PlaybackState::Playing;
    }

    fn end_playback(&mut self, cx: &mut Cx) {
        self.playback_state = PlaybackState::Finished;
        self.start_time = None;
        self.next_frame_ts = 0;
        self.cleanup_decoding(cx);
    }

    fn should_fetch(&self) -> bool {
        self.available_to_fetch && self.is_buffer_running_low()
    }

    fn should_request_decoding(&self) -> bool {
        match self.decoding_state {
            DecodingState::ChunkFinished => self.is_buffer_running_low(),
            _ => false,
        }
    }

    fn is_buffer_running_low(&self) -> bool {
        self.frames_buffer.lock().unwrap().len() < FRAME_BUFFER_LOW_WATER_MARK
    }

    fn cleanup_decoding(&mut self, cx: &mut Cx) {
        if self.decoding_state != DecodingState::NotStarted {
            cx.cleanup_video_decoding(self.id);
            self.frames_buffer.lock().unwrap().clear();
            self.decoding_state = DecodingState::NotStarted;
        }
    }
}

#[derive(Debug)]
struct FrameMetadata {
    timestamp: u128,
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    frame_range: Range<usize>,
    is_eos: bool,
}

type SharedFrameBuffer = Arc<Mutex<Vec<u8>>>;
