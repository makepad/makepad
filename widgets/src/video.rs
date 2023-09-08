use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_platform::{event::video_decoding::*, thread::*},
    widget::*,
    VideoColorFormat,
};
use std::{
    collections::VecDeque,
    sync::mpsc::channel,
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

const MAX_FRAMES_TO_DECODE: usize = 15;
const FRAME_BUTTER_LOW_WATER_MARK: usize = 10;

live_design! {
    VideoBase = {{Video}} {}
}

// TODO:
// - Add support for other color conversions. Currently only YUV420 Semi-Planar is supported, (and only nv12)
//      - adding color conversions is easy, we need to figure out how to get the codecs to give us the color format info.
//      - currently the flags I've tried like MediaFormat.KEY_COLOR_FORMAT, are null on most codecs.

// - Test frame rate to make sure it works right on different devices, current solution for ticks is somewhat a hack:
//     - sporadically some texture updates take longer than desired (5 - 10 ms) which causes subsequent calls create the next timeout a delay.
//       accumulated delays overtime make for slower playback.
//     - the current fix is to check for irregularities and compoensate on the next timeout.
//     - a better solution would be to have a loop on a separate thread in the platform's code, that sends timer events without having to schedule each one through events. 
//       tried this on Java but was missing some polisihng, it was actually too fast.
//     - a defentivie solution would be to fix the irregualarities, I believ they happen due to waiting for locks on the frame buffer and vec pools.
//       this can be improved by double buffering. (separate swappable read and write buffers)

// - Properly cleanup resources after playback is finished

// - Optimizations:
//      - lower memory usage by avoiding copying on frame chunk deserialization
//      - dynamically calculate chunk size this based on frame rate and size
//      - determine buffer size based on memory usage: minimal amount of frames to keep in memory for smooth playback considering their size
//      - testing multiple videos on a ListView will probably show other issues

// - Implement a pause/play

// - Post-conf:
//      - add audio playback

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
    y_texture: Option<Texture>,
    #[rust]
    uv_texture: Option<Texture>,

    // Playback options
    #[live]
    is_looping: bool,

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
    current_frame_ts: u128,
    #[rust]
    frame_ts_interval: f64,
    #[rust]
    last_update: Option<Instant>,
    #[rust]
    tick: Timer,
    #[rust]
    accumulated_time: u128,
    #[rust]
    playback_finished: bool,

    // Decoding
    #[rust]
    decoding_receiver: ToUIReceiver<Vec<u8>>,
    #[rust]
    decoding_state: DecodingState,
    #[rust]
    vec_pool_y: SharedVecPool,
    #[rust]
    vec_pool_uv: SharedVecPool,
    #[rust]
    last_timeout: Option<Instant>,

    #[rust]
    id: LiveId,
}

#[derive(Clone, Default, PartialEq, WidgetRef)]
pub struct VideoRef(WidgetRef);

#[derive(Clone, Default, WidgetSet)]
pub struct VideoSet(WidgetSet);

impl VideoSet {}

#[derive(Default, PartialEq)]
enum DecodingState {
    #[default]
    NotStarted,
    Decoding,
    Finished,
}

impl LiveHook for Video {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, Video);
    }

    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.id = LiveId::new(cx);
        self.initialize_decoding(cx);
    }
}

#[derive(Clone, WidgetAction)]
pub enum VideoAction {
    None,
}

impl Widget for Video {
    fn redraw(&mut self, cx: &mut Cx) {
        if self.uv_texture.is_none() || self.uv_texture.is_none() {
            return;
        }

        self.draw_bg
            .draw_vars
            .set_texture(0, self.y_texture.as_ref().unwrap());

        self.draw_bg
            .draw_vars
            .set_texture(1, self.uv_texture.as_ref().unwrap());
        self.draw_bg.redraw(cx);
    }

    fn walk(&self) -> Walk {
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
                self.decoding_state = DecodingState::Finished;
                cx.fetch_next_video_frames(self.id, MAX_FRAMES_TO_DECODE);    
            }
        }

        if self.tick.is_event(event) {
            let now = Instant::now();
            let elapsed_time = now.duration_since(self.last_timeout.unwrap()).as_millis();

            let ideal_interval = ((1.0 / self.original_frame_rate as f64) * 1000.0) as u128;

            let adjusted_interval = if elapsed_time > ideal_interval {
                ideal_interval - (elapsed_time - ideal_interval)
            } else {
                ideal_interval                
            };

            self.tick = cx.start_timeout(adjusted_interval as f64);

            self.last_timeout = Some(now);

            if self.decoding_state == DecodingState::Finished
                || self.decoding_state == DecodingState::Decoding
                    && self.frames_buffer.lock().unwrap().data.len() > 5
            {
                self.process_tick(cx);
            }

            if self.should_request_decoding() {
                cx.decode_next_video_chunk(self.id, MAX_FRAMES_TO_DECODE);
                self.decoding_state = DecodingState::Decoding;
            }
        }
    }

    fn handle_decoding_initialized(&mut self, cx: &mut Cx, event: &VideoDecodingInitializedEvent) {
        self.video_width = event.video_width as usize;
        self.video_height = event.video_height as usize;
        self.original_frame_rate = event.frame_rate;
        self.total_duration = event.duration;
        self.color_format = event.color_format;
        self.frame_ts_interval = 1000000.0 / self.original_frame_rate as f64;

        makepad_error_log::log!(
            "<<<<<<<<<<<<<<< Decoding initialized: \n {}x{}px | {} FPS | Color format: {:?} | Timestamp interval: {:?}",
            self.video_width,
            self.video_height,
            self.original_frame_rate,
            self.color_format,
            self.frame_ts_interval
        );

        cx.decode_next_video_chunk(self.id, MAX_FRAMES_TO_DECODE + MAX_FRAMES_TO_DECODE / 2);
        self.decoding_state = DecodingState::Decoding;

        self.begin_buffering_thread(cx);

        self.tick = cx.start_timeout((1.0 / self.original_frame_rate as f64 / 2.0) * 1000.0);
        self.last_timeout = Some(Instant::now());
    }

    fn should_request_decoding(&self) -> bool {
        match self.decoding_state {
            DecodingState::Finished => {
                self.frames_buffer.lock().unwrap().data.len() < FRAME_BUTTER_LOW_WATER_MARK
            }
            _ => false,
        }
    }

    fn process_tick(&mut self, cx: &mut Cx) {
        let now = Instant::now();
        match self.last_update {
            Some(last_update_ts) => {
                let elapsed = now.duration_since(last_update_ts).as_micros();
                self.accumulated_time += elapsed;
            }
            None => {
                self.accumulated_time = 0;
            }
        };
        // block to limit the scope of the lock guard
        let maybe_current_frame = { self.frames_buffer.lock().unwrap().get() };

        match maybe_current_frame {
            Some(current_frame) => {
                if self.accumulated_time >= current_frame.timestamp_us {
                    self.update_textures(cx, current_frame.y_data, current_frame.uv_data);
                    self.redraw(cx);

                    // if at latest frame, restart
                    if self.current_frame_ts >= self.total_duration {
                        if self.is_looping {
                            self.current_frame_ts = 0;
                        } else {
                            self.playback_finished = true;
                            self.cleanup_decoding(cx);
                        }
                        self.accumulated_time -= current_frame.timestamp_us;
                    } else {
                        self.current_frame_ts =
                            (self.current_frame_ts as f64 + self.frame_ts_interval).ceil() as u128;
                    }
                }

                self.last_update = Some(now);
            }
            None => {
                makepad_error_log::log!("Empty Buffer");
            }
        }
    }

    fn update_textures(
        &mut self,
        cx: &mut Cx,
        y_data: Arc<Mutex<Vec<u32>>>,
        uv_data: Arc<Mutex<Vec<u32>>>,
    ) {
        if self.y_texture.is_none() {
            let texture = Texture::new(cx);
            texture.set_desc(
                cx,
                TextureDesc {
                    format: TextureFormat::ImageBGRA,
                    width: Some(self.video_width),
                    height: Some(self.video_height),
                },
            );
            self.y_texture = Some(texture);
        }

        if self.uv_texture.is_none() {
            let texture = Texture::new(cx);
            texture.set_desc(
                cx,
                TextureDesc {
                    format: TextureFormat::ImageBGRA,
                    width: Some(self.video_width / 2),
                    height: Some(self.video_height / 2),
                },
            );
            self.uv_texture = Some(texture);
        }

        let y_texture = self.y_texture.as_mut().unwrap();
        let uv_texture = self.uv_texture.as_mut().unwrap();

        {
            let mut y_data_locked = y_data.lock().unwrap();
            y_texture.swap_image_u32(cx, &mut *y_data_locked);
        }

        {
            let mut uv_data_locked = uv_data.lock().unwrap();
            uv_texture.swap_image_u32(cx, &mut *uv_data_locked);
        }

        self.vec_pool_y
            .lock()
            .unwrap()
            .release(y_data.lock().unwrap().to_vec());
        self.vec_pool_uv
            .lock()
            .unwrap()
            .release(uv_data.lock().unwrap().to_vec());
    }

    fn initialize_decoding(&self, cx: &mut Cx) {
        match cx.get_dependency(self.source.as_str()) {
            Ok(data) => {
                cx.initialize_video_decoding(self.id, data, 100);
            }
            Err(_e) => {
                todo!()
            }
        }
    }

    fn begin_buffering_thread(&mut self, cx: &mut Cx) {
        let video_sender = self.decoding_receiver.sender();
        cx.video_decoding_input(self.id, move |data| {
            let _ = video_sender.send(data);
        });

        let frames_buffer = Arc::clone(&self.frames_buffer);
        let vec_pool_y = Arc::clone(&self.vec_pool_y);
        let vec_pool_uv = Arc::clone(&self.vec_pool_uv);

        let video_width = self.video_width.clone();
        let video_height = self.video_height.clone();

        let (_new_sender, new_receiver) = channel();
        let old_receiver = std::mem::replace(&mut self.decoding_receiver.receiver, new_receiver);

        thread::spawn(move || loop {
            let frame_group = old_receiver.recv().unwrap();
            deserialize_chunk(
                Arc::clone(&frames_buffer),
                Arc::clone(&vec_pool_y),
                Arc::clone(&vec_pool_uv),
                &frame_group,
                video_width,
                video_height,
            );
        });
    }

    fn cleanup_decoding(&mut self, _cx: &mut Cx) {
        //cx.cleanup_video_decoding(self.id);
        //cx.cancel_timeout
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
    y_data: Arc<Mutex<Vec<u32>>>,
    uv_data: Arc<Mutex<Vec<u32>>>,
    timestamp_us: u128,
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
}

fn deserialize_chunk(
    frames_buffer: SharedRingBuffer,
    vec_pool_y: SharedVecPool,
    vec_pool_uv: SharedVecPool,
    frame_group: &[u8],
    video_width: usize,
    video_height: usize,
) {
    let mut cursor = 0;

    // | Timestamp (8B)  | Y Stride (4B) | UV Stride (4B) | Frame data length (4b) | Pixel Data |
    let metadata_size = 20;

    while cursor < frame_group.len() {
        // might have to update for different endinaess on other platforms
        let timestamp =
            u64::from_be_bytes(frame_group[cursor..cursor + 8].try_into().unwrap()) as u128;
        let y_stride = u32::from_be_bytes(frame_group[cursor + 8..cursor + 12].try_into().unwrap());
        let uv_stride =
            u32::from_be_bytes(frame_group[cursor + 12..cursor + 16].try_into().unwrap());
        let frame_length =
            u32::from_be_bytes(frame_group[cursor + 16..cursor + 20].try_into().unwrap()) as usize;

        let frame_data_start = cursor + metadata_size;
        let frame_data_end = frame_data_start + frame_length;

        let pixel_data = &frame_group[frame_data_start..frame_data_end];

        let mut y_data = vec_pool_y
            .lock()
            .unwrap()
            .acquire(video_width * video_height);
        let mut uv_data = vec_pool_uv
            .lock()
            .unwrap()
            .acquire((video_width / 2) * (video_height / 2));

        split_nv12_data(
            pixel_data,
            video_width,
            video_height,
            y_stride as usize,
            uv_stride as usize,
            y_data.as_mut_slice(),
            uv_data.as_mut_slice(),
        );

        frames_buffer.lock().unwrap().push(VideoFrame {
            y_data: Arc::new(Mutex::new(y_data)),
            uv_data: Arc::new(Mutex::new(uv_data)),
            timestamp_us: timestamp,
        });

        cursor = frame_data_end;
    }
}

fn split_nv12_data(
    data: &[u8],
    width: usize,
    height: usize,
    y_stride: usize,
    uv_stride: usize,
    y_data: &mut [u32],
    uv_data: &mut [u32],
) {
    let mut y_idx = 0;
    let mut uv_idx = 0;

    if y_data.len() < width * height || uv_data.len() < (width / 2) * (height / 2) {
        makepad_error_log::log!(
            "y_data len: {}, uv_data len: {}, width: {}, height: {}",
            y_data.len(),
            uv_data.len(),
            width,
            height
        );
        return;
    }

    // Extract and convert Y data
    for row in 0..height {
        let start = row * y_stride;
        let end = start + width;
        for &y in &data[start..end] {
            y_data[y_idx] = 0xFFFFFF00u32 | (y as u32);
            y_idx += 1;
        }
    }

    // Extract and convert UV data
    let uv_start = y_stride * height;
    for row in 0..(height / 2) {
        let start = uv_start + row * uv_stride;
        let end = start + width;
        for chunk in data[start..end].chunks(2) {
            let u = chunk[0];
            let v = chunk[1];
            uv_data[uv_idx] = (u as u32) << 16 | (v as u32) << 8 | 0xFF000000u32;
            uv_idx += 1;
        }
    }
}
