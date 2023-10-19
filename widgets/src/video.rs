use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_platform::{event::video_decoding::*},
    widget::*,
    VideoColorFormat,
};
use std::{
    collections::VecDeque,
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

// - Add support for SemiPlanar nv21, currently we assume that SemiPlanar is nv12
// - Add function to restart playback manually when not looping.

// - Optimizations:
//      - we could offload work by moving the YUV interleaving to the GPU. however this either requires to either add support for textures as vec<u8> in makeapd
//        or convert to vec<u32> more directly and having the GPU do the interleaving. currently since we're already iterating over the data when converting to vec<u32>,
//        we're also packing it into YUV in way that's simple to grab in the shader.
//      - lower memory usage by avoiding copying on frame chunk deserialization
//      - determine frame chunk size based on memory usage: minimal amount of frames to keep in memory for smooth playback considering their size
//      - we're allocating new vec and copying data from java into rust when decoding, if we need to we could have a shared memory buffer between them, but that
//        introduces a lot of complexity.

// Future:
//  - Add audio playback

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

    #[live]
    source: LiveDependency,
    #[rust]
    texture: Option<Texture>,

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
    frames_buffer: SharedRingBuffer,

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
    #[rust]
    vec_pool: SharedVecPool,
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
        if self.texture.is_none() {
            return;
        }

        self.draw_bg
            .draw_vars
            .set_texture(0, self.texture.as_ref().unwrap());

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
                    cx.initialize_video_decoding(self.id, data, 100);
                    self.decoding_state = DecodingState::Initializing;
                }
                Err(e) => {
                    error!("initialize_decoding: resource not found {} {}", self.source.as_str(), e);
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
        let vec_pool = Arc::clone(&self.vec_pool);

        let video_width = self.video_width.clone();
        let video_height = self.video_height.clone();
        let color_format = self.color_format.clone();

        let (_new_sender, new_receiver) = channel();
        let old_receiver = std::mem::replace(&mut self.decoding_receiver.receiver, new_receiver);

        thread::spawn(move || loop {
            let frame_group = old_receiver.recv().unwrap();
            deserialize_chunk(
                Arc::clone(&frames_buffer),
                Arc::clone(&vec_pool),
                &frame_group,
                video_width,
                video_height,
                color_format,
            );
        });
    }

    fn maybe_show_preview(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Previewing {
            if !self.is_current_texture_preview {
                let current_frame = { self.frames_buffer.lock().unwrap().get() };
                match current_frame {
                    Some(current_frame) => {
                        self.draw_bg.set_uniform(cx, id!(is_last_frame), &[0.0]);
                        self.draw_bg.set_uniform(cx, id!(texture_available), &[1.0]);
                        self.update_texture(cx, current_frame.pixel_data);
                        self.is_current_texture_preview = true;
                        self.redraw(cx);
                    }
                    None => {}
                }
            }
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
                let maybe_current_frame = { self.frames_buffer.lock().unwrap().get() };

                match maybe_current_frame {
                    Some(current_frame) => {
                        if self.start_time.is_none() {
                            self.start_time = Some(now);
                            self.draw_bg.set_uniform(cx, id!(is_last_frame), &[0.0]);
                            self.draw_bg.set_uniform(cx, id!(texture_available), &[1.0]);
                        }

                        self.update_texture(cx, current_frame.pixel_data);
                        self.redraw(cx);

                        // if at the last frame, loop or stop
                        if current_frame.is_eos {
                            self.next_frame_ts = 0;
                            self.start_time = None;
                            if !self.is_looping {
                                self.draw_bg.set_uniform(cx, id!(is_last_frame), &[1.0]);
                                self.playback_state = PlaybackState::Finished;
                            }
                        } else {
                            self.next_frame_ts =
                                current_frame.timestamp_us + self.frame_ts_interval.ceil() as u128;
                        }
                    }
                    // empty buffer, decoder is falling behind
                    None => {}
                }
            }
        }
    }

    fn update_texture(&mut self, cx: &mut Cx, pixel_data: Arc<Mutex<Vec<u32>>>) {
        if self.texture.is_none() {
            let texture = Texture::new(cx);
            texture.set_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: self.video_width,
                    height: self.video_height,
                    data: vec![]
                },
            );
            self.texture = Some(texture);
        }

        let texture = self.texture.as_mut().unwrap();

        {
            let mut data_locked = pixel_data.lock().unwrap();
            texture.swap_vec_u32(cx, &mut *data_locked);
        }

        self.vec_pool
            .lock()
            .unwrap()
            .release(pixel_data.lock().unwrap().to_vec());
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
                error!("Error decoding video with id {} : {}", self.id.0, event.error);
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
        self.frames_buffer.lock().unwrap().data.len() < FRAME_BUFFER_LOW_WATER_MARK
    }

    fn cleanup_decoding(&mut self, cx: &mut Cx) {
        if self.decoding_state != DecodingState::NotStarted {
            cx.cleanup_video_decoding(self.id);
            self.frames_buffer.lock().unwrap().clear();
            self.vec_pool.lock().unwrap().clear();
            self.decoding_state = DecodingState::NotStarted;
        }
    }
}

type SharedRingBuffer = Arc<Mutex<RingBuffer>>;
#[derive(Clone)]
struct RingBuffer {
    data: VecDeque<VideoFrame>,
    last_added_index: Option<usize>,
}

impl RingBuffer {
    fn get(&mut self) -> Option<VideoFrame> {
        self.data.pop_front()
    }

    fn push(&mut self, frame: VideoFrame) {
        self.data.push_back(frame);

        match self.last_added_index {
            None => {
                self.last_added_index = Some(0);
            }
            Some(index) => {
                self.last_added_index = Some(index + 1);
            }
        }
    }

    fn clear(&mut self) {
        self.data.clear();
        self.last_added_index = None;
    }
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self {
            data: VecDeque::new(),
            last_added_index: None,
        }
    }
}

#[derive(Clone, Default)]
struct VideoFrame {
    pixel_data: Arc<Mutex<Vec<u32>>>,
    timestamp_us: u128,
    is_eos: bool,
}

type SharedVecPool = Arc<Mutex<VecPool>>;
#[derive(Default, Clone)]
pub struct VecPool {
    pool: Vec<Vec<u32>>,
}

impl VecPool {
    pub fn acquire(&mut self, capacity: usize) -> Vec<u32> {
        match self.pool.pop() {
            Some(mut vec) => {
                if vec.capacity() < capacity {
                    vec.resize(capacity, 0);
                }
                vec
            }
            None => vec![0u32; capacity],
        }
    }

    pub fn release(&mut self, vec: Vec<u32>) {
        self.pool.push(vec);
    }

    pub fn clear(&mut self) {
        self.pool.clear();
    }
}

fn deserialize_chunk(
    frames_buffer: SharedRingBuffer,
    vec_pool: SharedVecPool,
    frame_group: &[u8],
    video_width: usize,
    video_height: usize,
    color_format: VideoColorFormat,
) {
    let mut cursor = 0;

    // | Timestamp (8B)  | Y Stride (4B) | U Stride (4B) | V Stride (4B) | isEoS (1B) | Frame data length (4b) | Pixel Data |
    while cursor < frame_group.len() {
        // might have to update for different endinaess on other platforms
        let timestamp =
            u64::from_be_bytes(frame_group[cursor..cursor + 8].try_into().unwrap()) as u128;
        cursor += 8;
        let y_stride =
            u32::from_be_bytes(frame_group[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        let u_stride =
            u32::from_be_bytes(frame_group[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        let v_stride =
            u32::from_be_bytes(frame_group[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        let is_eos = u8::from_be_bytes(frame_group[cursor..cursor + 1].try_into().unwrap()) != 0;
        cursor += 1;
        let frame_length =
            u32::from_be_bytes(frame_group[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;

        let frame_data_end = cursor + frame_length;
        let pixel_data = &frame_group[cursor..frame_data_end];

        let mut pixel_data_u32 = vec_pool
            .lock()
            .unwrap()
            .acquire(video_width as usize * video_height as usize);

        match color_format {
            VideoColorFormat::YUV420Planar => planar_to_u32(
                pixel_data,
                video_width,
                video_height,
                y_stride,
                u_stride,
                v_stride,
                &mut pixel_data_u32,
            ),
            VideoColorFormat::YUV420SemiPlanar => semi_planar_to_u32(
                pixel_data,
                video_width,
                video_height,
                y_stride,
                u_stride,
                &mut pixel_data_u32,
            ),
            VideoColorFormat::YUV420Flexible => todo!(),
            VideoColorFormat::Unknown => todo!(),
        };

        frames_buffer.lock().unwrap().push(VideoFrame {
            pixel_data: Arc::new(Mutex::new(pixel_data_u32)),
            timestamp_us: timestamp,
            is_eos,
        });

        cursor = frame_data_end;
    }
}

fn planar_to_u32(
    data: &[u8],
    width: usize,
    height: usize,
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    packed_data: &mut [u32],
) {
    let mut y_idx = 0;

    let y_start = 0;
    let u_start = y_stride * height;
    let v_start = u_start + u_stride * (height / 2);

    for row in 0..height {
        let y_row_start = y_start + row * y_stride;
        let u_row_start = u_start + (row / 2) * u_stride;
        let v_row_start = v_start + (row / 2) * v_stride;

        for x in 0..width {
            let y = data[y_row_start + x];
            let u = data[u_row_start + x / 2];
            let v = data[v_row_start + x / 2];

            // Pack Y, U, and V into u32: Y in Blue channel, U in Green, V in Red
            packed_data[y_idx] = (v as u32) << 16 | (u as u32) << 8 | (y as u32);

            y_idx += 1;
        }
    }
}

fn semi_planar_to_u32(
    data: &[u8],
    width: usize,
    height: usize,
    y_stride: usize,
    uv_stride: usize,
    packed_data: &mut [u32],
) {
    let mut y_idx = 0;
    let uv_start = y_stride * height;

    for row in 0..height {
        let y_start = row * y_stride;
        let uv_row_start = uv_start + (row / 2) * uv_stride;

        for x in 0..width {
            let y = data[y_start + x];

            // calculate index for UV data
            let uv_idx = uv_row_start + (x / 2) * 2;
            let u = data[uv_idx];
            let v = data[uv_idx + 1];

            // pack Y, U, and V into u32: Y in Blue channel, U in Green, V in Red
            packed_data[y_idx] = (v as u32) << 16 | (u as u32) << 8 | (y as u32);

            y_idx += 1;
        }
    }
}
