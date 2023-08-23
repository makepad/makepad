use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*, VideoColorFormat};
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
            texture y_image: texture2d
            texture uv_image: texture2d
            instance image_scale: vec2(1.0, 1.0)
            instance image_pan: vec2(0.0, 0.0)
            uniform image_alpha: 1.0

            fn yuv_to_rgb(y: float, u: float, v: float) -> vec4 {
                let c = y - 16.0;
                let d = u - 128.0;
                let e = v - 128.0;

                let r = clamp((298.0 * c + 409.0 * e + 128.0) / 65536.0, 0.0, 1.0);
                let g = clamp((298.0 * c - 100.0 * d - 208.0 * e + 128.0) / 65536.0, 0.0, 1.0);
                let b = clamp((298.0 * c + 516.0 * d + 128.0) / 65536.0, 0.0, 1.0);

                return vec4(r, g, b, 1.0);
            }

            fn get_color(self) -> vec4 {
                let y_sample = sample2d(self.y_image, self.pos * self.image_scale + self.image_pan).z;
                let uv_coords = (self.pos * self.image_scale + self.image_pan);
                let uv_sample = sample2d(self.uv_image, uv_coords);

                let u = uv_sample.x;
                let v = uv_sample.y;

                return yuv_to_rgb(y_sample * 255., u * 255., v * 255.);
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
    // Drawing
    #[live]
    draw_bg: DrawColor,
    #[live]
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

    // Original video metadata
    #[rust]
    width: usize,
    #[rust]
    height: usize,
    #[rust]
    total_duration: u64,
    #[rust]
    original_frame_rate: usize,
    #[rust]
    color_format: VideoColorFormat,

    // Buffering
    #[rust]
    frames_buffer: RingBuffer,
    #[rust]
    current_start_ts: u64,
    #[rust]
    current_end_ts: u64,

    // Frame
    #[live]
    current_frame: usize,
    #[rust]
    last_update: MyInstant,
    #[rust]
    tick: Timer,
    #[rust]
    accumulated_time: f64,

    // Decoding
    #[rust]
    last_decode_request_ts: (u64, u64),
    #[rust]
    decoding_state: DecodingState,

    #[rust]
    id: LiveId,
}

#[derive(Clone)]
struct VideoFrame {
    y_data: Vec<u8>,
    uv_data: Vec<u8>,
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
            self.color_format = event.color_format;

            makepad_error_log::log!(
                "Decoding initialized: {}x{}px | {} FPS | Color format: {:?}",
                self.width,
                self.height,
                self.original_frame_rate,
                self.color_format
            );

            self.resize_frames_buffer();

            self.current_start_ts = 0;
            self.current_end_ts = CHUNK_DURATION_US;
            cx.decode_video_chunk(self.id, self.current_start_ts, self.current_end_ts);
            self.decoding_state = DecodingState::Decoding;

            self.tick = cx.start_timeout((1.0 / self.original_frame_rate as f64 / 2.0) * 1000.0);
        }

        if let Event::VideoStream(event) = event {
            if event.pixel_data.len() != 0 {
                // TODO: check color format before splitting, might want to support NV12 and other formats
                let (y_data, uv_data) = split_nv12_data(
                    &event.pixel_data,
                    self.width,
                    self.height,
                    event.yuv_strides.0,
                    event.yuv_strides.1,
                );
                self.frames_buffer.push(VideoFrame {
                    y_data,
                    uv_data,
                    timestamp: event.timestamp as f64 / 1_000_000.0,
                });
            }

            // TODO:
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

    fn draw(&mut self, cx: &mut Cx) {
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
            .map(|f| (f.y_data.clone(), f.uv_data.clone()))
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
            self.update_textures(cx, &mut cloned_pixel_data.0, &mut cloned_pixel_data.1);

            self.draw_bg
                .draw_vars
                .set_texture(0, self.y_texture.as_ref().unwrap());

            self.draw_bg
                .draw_vars
                .set_texture(1, self.uv_texture.as_ref().unwrap());
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

    fn update_textures(&mut self, cx: &mut Cx, y_data: &mut Vec<u8>, uv_data: &mut Vec<u8>) {
        if let None = self.y_texture {
            self.y_texture = Some(Texture::new(cx));
        }
        if let None = self.uv_texture {
            self.uv_texture = Some(Texture::new(cx));
        }

        let y_texture = self.y_texture.as_mut().unwrap();
        let uv_texture = self.uv_texture.as_mut().unwrap();

        y_texture.set_desc(
            cx,
            TextureDesc {
                format: TextureFormat::ImageBGRA,
                width: Some(self.width),
                height: Some(self.height),
            },
        );

        uv_texture.set_desc(
            cx,
            TextureDesc {
                format: TextureFormat::ImageBGRA,
                width: Some(self.width / 2),
                height: Some(self.height / 2),
            },
        );

        // TODO:
        // - optimize by adding support for 8-bit textures
        // - optimize by already buffering the u32 data instead of converting at this stage.

        // Convert 8-bit Y data to 32-bit
        let mut y_data_u32: Vec<u32> = y_data.iter().map(|&y| 0xFFFFFF00u32 | (y as u32)).collect();
        // Convert 8-bit UV data to 32-bit (only 2 channels are used)
        let mut uv_data_u32: Vec<u32> = uv_data
            .chunks(2)
            .map(|chunk| {
                let u = chunk[0];
                let v = chunk[1];
                (u as u32) << 16 | (v as u32) << 8 | 0xFF000000u32
            })
            .collect();

        y_texture.swap_image_u32(cx, &mut y_data_u32);
        uv_texture.swap_image_u32(cx, &mut uv_data_u32);
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
    }
}

// TODO: dynamically calculate this based on frame rate and size
const CHUNK_DURATION_US: u64 = 1_000_000;

struct RingBuffer {
    data: Vec<Option<VideoFrame>>,
    size: usize,
    start: usize,
    end: usize,
}

impl RingBuffer {
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
        let data = Vec::with_capacity(60);
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

fn split_nv12_data(
    data: &[u8],
    width: usize,
    height: usize,
    y_stride: usize,
    uv_stride: usize,
) -> (Vec<u8>, Vec<u8>) {
    let mut y_data = Vec::with_capacity(width * height);
    let mut uv_data = Vec::with_capacity(width * height / 2);

    // Extract Y data
    for row in 0..height {
        let start = row * y_stride;
        let end = start + width;
        y_data.extend_from_slice(&data[start..end]);
    }

    // Extract UV data
    let uv_start = y_stride * height;
    for row in 0..(height / 2) {
        let start = uv_start + row * uv_stride;
        let end = start + width; // since U and V are interleaved, and are half the width of Y
        uv_data.extend_from_slice(&data[start..end]);
    }

    (y_data, uv_data)
}
