use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::time::Instant;

const DEFAULT_FPS_INTERVAL: f64 = 33.0;

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    Video = {{Video}} {
        walk:{
            width: 500
            height: 500
        }
        draw_bg: {
            texture image: texture2d
            instance image_scale: vec2(1.0, 1.0)
            instance image_pan: vec2(0.0, 0.0)
            uniform image_alpha: 1.0
            fn get_color(self) -> vec4 {
                return sample2d(self.image, self.pos * self.image_scale + self.image_pan).xyzw;
            }

            fn pixel(self) -> vec4 {
                let color = self.get_color();
                return Pal::premul(vec4(color.xyz, color.w * self.image_alpha))
            }

            shape: Solid,
            fill: Image
        }
    }
}

#[derive(Live)]
pub struct Video {
    #[live]
    walk: Walk,
    #[live]
    layout: Layout,
    #[live]
    draw_bg: DrawColor,
    #[live]
    scale: f64,

    #[live]
    source: LiveDependency,

    #[rust]
    width: usize,
    #[rust]
    height: usize,
    #[rust]
    total_duration: u64,

    #[rust]
    texture: Option<Texture>,

    #[rust]
    frames_buffer: RingBuffer,
    #[live]
    current_frame: usize,
    #[rust]
    decoding_state: DecodingState,

    #[rust]
    last_update: MyInstant,

    #[rust]
    tick: Timer,
    #[rust]
    accumulated_time: f64,
    #[rust]
    original_frame_rate: usize,

    #[rust]
    last_decode_request_ts: (u64, u64),

    #[rust]
    current_start_ts: u64,
    #[rust]
    current_end_ts: u64,

    #[rust]
    id: LiveId,
}

#[derive(Clone)]
struct VideoFrame {
    pixel_data: Vec<u32>,
    timestamp: f64,
}

#[derive(Clone, Default, PartialEq, WidgetRef)]
pub struct VideoRef(WidgetRef);

#[derive(Default, PartialEq)]
enum DecodingState {
    #[default]
    NotStarted,
    Idle,
    Decoding,
    Finished,
}

struct MyInstant(Instant);

impl Default for MyInstant {
    fn default() -> Self {
        MyInstant(Instant::now())
    }
}

impl LiveHook for Video {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, Video)
    }

    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.id = LiveId::new(cx);
        // TODO: using start_timeout because start_interval doesn't repeat on android
        // self.tick = cx.start_timeout(DEFAULT_FPS_INTERVAL);
        self.start_decoding(cx);
        // self.decoding_state = DecodingState::Decoding;
    }
}

#[derive(Clone, WidgetAction)]
pub enum VideoAction {
    None,
}

// TODO:
// - implement buffering
//  - play on a loop, use total duration and frame timestamp to determine next decodes and loop
//  - determine buffer size based on memory usage: minimal amount of frames to keep in memory for smooth playback considering their size
// - implement a pause/play

impl Widget for Video {
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    fn get_walk(&self) -> Walk {
        self.walk
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk)
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
        if self.tick.is_event(event) {
            self.tick = cx.start_timeout((1.0 / self.original_frame_rate as f64 / 2.0) * 1000.0);
            if self.frames_buffer.data.len() > 20 {
                self.draw(cx);
            }
        }

        if let Event::VideoDecodingInitialized(event) = event {
            self.width = event.video_width as usize;
            self.height = event.video_height as usize;
            self.original_frame_rate = event.frame_rate;
            self.total_duration = event.duration;

            makepad_error_log::log!(
                "Decoding initialized: {}x{}px | {} FPS",
                self.width,
                self.height,
                self.original_frame_rate
            );

            self.resize_frames_buffer();

            self.current_start_ts = 0;
            self.current_end_ts = CHUNK_DURATION_US;
            cx.decode_video_chunk(self.id, self.current_start_ts, self.current_end_ts);
            self.decoding_state = DecodingState::Decoding;

            self.tick = cx.start_timeout((1.0 / self.original_frame_rate as f64 / 2.0) * 1000.0);
        }

        if let Event::VideoStream(event) = event {
            // if self.frames_buffer.data.len() < self.frames_buffer.size {
            if event.pixel_data.len() != 0 {
                let rgba_pixel_data =
                    convert_nv12_to_rgba(&event.pixel_data, self.width, self.height);
                self.frames_buffer.push(VideoFrame {
                    pixel_data: rgba_pixel_data,
                    timestamp: event.timestamp as f64 / 1_000_000.0,
                });
            }

            // TODO
            // IF IS END OF CHUNK, GO FOR NEXT CHUNK
            if event.is_eos {
                makepad_error_log::log!("Chunk decoding finished");
                self.current_start_ts = self.current_end_ts;
                self.current_end_ts += CHUNK_DURATION_US;
                cx.decode_video_chunk(self.id, self.current_start_ts, self.current_end_ts);
            }
            // IF IS END OF VIDEO, GO FOR FIST CHUNK

        }
    }

    fn wont_repeat_decode_request(&self) -> bool {
        (self.current_start_ts, self.current_end_ts) == self.last_decode_request_ts
    }

    fn draw(&mut self, cx: &mut Cx) {
        // makepad_error_log::log!(
        //     "Tick frame for {} / {}",
        //     self.current_frame,
        //     self.frames_buffer.data.len()
        // );

        let now = Instant::now();
        let elapsed = now.duration_since(self.last_update.0).as_secs_f64();
        self.accumulated_time += elapsed;

        let frame_timestamp = self
            .frames_buffer
            .get(self.current_frame)
            .map(|f| f.timestamp)
            .unwrap_or(0.0);

        let mut cloned_pixel_data;
        match self
            .frames_buffer
            .get(self.current_frame)
            .map(|f| f.pixel_data.clone())
        {
            Some(pixel_data) => {
                cloned_pixel_data = pixel_data;
            }
            None => {
                makepad_error_log::log!("No pixel data for frame {}", self.current_frame);
                return;
            }
        }

        // Iterate as long as the accumulated time exceeds the timestamp of the current frame
        // This helps in catching up in case some frames were skipped due to longer `elapsed` times.
        // this used to be a while instead of if, we'll see if needed
        if self.accumulated_time >= frame_timestamp {
            self.update_texture(cx, &mut cloned_pixel_data);
            self.draw_bg
                .draw_vars
                .set_texture(0, self.texture.as_ref().unwrap());
            self.redraw(cx);

            // Check if we're at the last frame
            if self.current_frame == self.frames_buffer.data.len() - 1 {
                self.accumulated_time -= frame_timestamp;
                self.current_frame = 0;
            } else {
                self.current_frame += 1;
            }
        }

        self.last_update = MyInstant(now);
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_bg.draw_walk(cx, walk);
        WidgetDraw::done()
    }

    fn update_texture(&mut self, cx: &mut Cx, rgba_pixel_data: &mut Vec<u32>) {
        if let None = self.texture {
            self.texture = Some(Texture::new(cx));
        }
        let texture = self.texture.as_mut().unwrap();

        texture.set_desc(
            cx,
            TextureDesc {
                format: TextureFormat::ImageBGRA,
                width: Some(self.width),
                height: Some(self.height),
            },
        );

        texture.swap_image_u32(cx, rgba_pixel_data);
    }

    fn start_decoding(&self, cx: &mut Cx) {
        match cx.get_dependency(self.source.as_str()) {
            Ok(data) => {
                cx.initialize_video_decoding(self.id, data, 100);
                makepad_error_log::log!("Decoding initialization requested");
            }
            Err(_e) => {
                todo!()
            }
        }
    }

    fn resize_frames_buffer(&mut self) {
        let chunk_duration_seconds = CHUNK_DURATION_US as f64 / 1_000_000.0;
        let estimated_frames_per_chunk =
            (self.original_frame_rate as f64 * chunk_duration_seconds).ceil() as usize;

        // safety margin of 20%
        let buffer_size_with_margin = (estimated_frames_per_chunk as f64 * 1.2).ceil() as usize;

        makepad_error_log::log!(
            "Estimated frames per chunk: {}, Buffer size: {}",
            estimated_frames_per_chunk,
            buffer_size_with_margin
        );
        self.frames_buffer.size = buffer_size_with_margin;
        // self.frames_buffer.resize_buffer(buffer_size_with_margin);
    }
}

// const RING_BUFFER_SIZE: usize = 100;
const CHUNK_DURATION_US: u64 = 1_000_000;

struct RingBuffer {
    data: Vec<Option<VideoFrame>>,
    size: usize,
    start: usize,
    end: usize,
}

impl RingBuffer {
    // fn resize_buffer(&mut self, new_size: usize) {
    //     if new_size > self.data.capacity() {
    //         self.data.reserve(new_size - self.data.capacity());
    //     }
    //     self.data.resize_with(new_size, Default::default);
    // }

    fn get(&self, index: usize) -> Option<&VideoFrame> {
        self.data.get(index).and_then(|item| item.as_ref())
    }

    fn push(&mut self, frame: VideoFrame) {
        if self.data.len() < self.size {
            self.data.push(Some(frame));
            self.end += 1;
        } else {
            self.data[self.end] = Some(frame);
            self.end = (self.end + 1) % self.size;

            // If end has caught up to start, move start to the next oldest item
            if self.end == self.start {
                self.start = (self.start + 1) % self.size;
            }
        }
    }
}

impl Default for RingBuffer {
    fn default() -> Self {
        let mut data = Vec::with_capacity(60);
        // data.resize(RING_BUFFER_SIZE, None);
        Self {
            data,
            size: 0,
            start: 0,
            end: 0,
        }
    }
}

fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let y = y as f32;
    let u = u as f32 - 128.0;
    let v = v as f32 - 128.0;

    let r = (y + 1.402 * v).max(0.0).min(255.0) as u8;
    let g = (y - 0.344136 * u - 0.714136 * v).max(0.0).min(255.0) as u8;
    let b = (y + 1.772 * u).max(0.0).min(255.0) as u8;

    (r, g, b)
}

// Representation of color space
// MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420SemiPlanar. This format is also known as NV12. 21
// In NV12 format (YUV420SemiPlanar), the Y plane is fully sampled (like in YUV420Planar), but the U and V samples are interleaved. The layout is:
// First width * height bytes: Y samples
// Next width * height / 2 bytes: interleaved U and V samples (UVUVUV...)

// TODO:
// - support YUV420Planar
// - maybe move this logic to the DSL to run on the GPU

fn convert_nv12_to_rgba(data: &[u8], width: usize, height: usize) -> Vec<u32> {
    if data.len() < width * height * 3 / 2 {
        panic!("Input data is not of expected size for NV12 format");
    }

    let mut rgba_data = Vec::with_capacity(width * height);

    // Indices for the Y and UV data.
    let y_start = 0;
    let uv_start = width * height;

    for y in 0..height {
        for x in 0..width {
            // Get the Y value.
            let y_index = y_start + y * width + x;
            let y_value = data[y_index];

            // Get the U and V values. (For NV12 format, UV values are interleaved.)
            let uv_index = uv_start + (y / 2) * width + 2 * (x / 2);
            let u_value = data[uv_index];
            let v_value = data[uv_index + 1];

            let (r, g, b) = yuv_to_rgb(y_value, u_value, v_value);

            rgba_data.push(0xFF << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32);
        }
    }

    rgba_data
}
