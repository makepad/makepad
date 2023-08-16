use std::time::Instant;
use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

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
    texture: Option<Texture>,

    // TODO:
    // Implement a ring buffer
    #[rust]
    frames: Vec<VideoFrame>,
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
}

#[derive(Clone)]
struct VideoFrame {
    pixel_data: Vec<u32>,
    timestamp: f64,
}

#[derive(Clone, Default, PartialEq, WidgetRef)]
pub struct VideoRef(WidgetRef);

#[derive(Default)]
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
        // TODO: using start_timeout because start_interval doesn't repeat on android
        self.tick = cx.start_timeout(DEFAULT_FPS_INTERVAL); 
        self.start_decoding(cx);
        self.decoding_state = DecodingState::Decoding;
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
            if self.frames.len() > 0 {
                self.draw(cx);
            }
        }

        if let Event::VideoStream(event) = event {
            // just limiting amount of frames for debugging
            if self.frames.len() <= 300 {
                if event.pixel_data.len() != 0 {
                    self.width = event.video_width as usize;
                    self.height = event.video_height as usize;
                    self.original_frame_rate = event.original_frame_rate;

                    let rgba_pixel_data =
                        convert_nv12_to_rgba(&event.pixel_data, self.width, self.height);

                    self.frames.push(VideoFrame {
                        pixel_data: rgba_pixel_data,
                        timestamp: event.timestamp as f64 / 1_000_000.0, // Convert to seconds
                    });
                } 
            }
            if event.is_eos {
                makepad_error_log::log!(
                    "DECODING FINISHED, total: {} frames",
                    self.frames.len()
                );
                self.decoding_state = DecodingState::Finished;
            }
        }
    }

    fn draw(&mut self, cx: &mut Cx) {
        if self.frames.len() > 0 {
            let now = Instant::now();
            let elapsed = now.duration_since(self.last_update.0).as_secs_f64();
            self.accumulated_time += elapsed;

            // Iterate as long as the accumulated time exceeds the timestamp of the current frame
            // This helps in catching up in case some frames were skipped due to longer `elapsed` times.
            while self.accumulated_time >= self.frames[self.current_frame].timestamp {
                let frame = &self.frames[self.current_frame];

                // makepad_error_log::log!(
                //     "Drawing frame: {} of {}",
                //     self.current_frame,
                //     self.frames.len()
                // );

                // Update the texture and redraw
                self.update_texture(cx, &mut frame.pixel_data.clone());
                self.draw_bg
                    .draw_vars
                    .set_texture(0, self.texture.as_ref().unwrap());

                self.redraw(cx);

                // Check if we're at the last frame
                if self.current_frame == self.frames.len() - 1 {
                    // Adjust accumulated time and reset current frame
                    self.accumulated_time -= self.frames[self.current_frame].timestamp;
                    self.current_frame = 0;
                } else {
                    self.current_frame += 1;
                }
            }

            self.last_update = MyInstant(now);
        }
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
                cx.decode_video(data);
                makepad_error_log::log!("DECODING BEGAN");
            }
            Err(_e) => {
                todo!()
            }
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

            // Convert RGB to RGBA and store as BGRA. TODO WHY DOES THIS WORK?
            rgba_data.push(0xFF << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32);
        }
    }

    rgba_data
}
