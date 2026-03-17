use makepad_widgets::*;
use makepad_widgets::makepad_platform::MseDecodedFrame;
use makepad_widgets::makepad_platform::studio::StudioToApp;
use makepad_widgets::makepad_micro_serde::DeJson;
use std::rc::Rc;
use std::sync::mpsc;

app_main!(App);

const AV1_MP4: &[u8] = include_bytes!("../data/av1.mp4");
const H264_MP4: &[u8] = include_bytes!("../data/h264.mp4");
const AV1_FRAG_MP4: &[u8] = include_bytes!("../data/av1_frag.mp4");
const H264_FRAG_MP4: &[u8] = include_bytes!("../data/h264_frag.mp4");

fn can_play_report() -> String {
    let types = ["video/mp4"];
    let mut parts = Vec::new();
    for t in &types {
        let r = makepad_widgets::makepad_platform::can_play_type(t);
        let r = if r.is_empty() { "no" } else { r };
        parts.push(format!("{t}={r}"));
    }
    parts.join("  ")
}

/// Convert YUV plane data to BGRA u32 pixels for texture upload.
fn yuv_frame_to_bgra(frame: &MseDecodedFrame) -> Vec<u32> {
    let yuv = &frame.yuv;
    let w = yuv.width as usize;
    let h = yuv.height as usize;
    let mut pixels = vec![0u32; w * h];

    // BT.709 limited range matrix (matches the MSE player's default for AV1)
    // Y' = Y - 16, Cb = U - 128, Cr = V - 128
    // R = 1.164 * Y' + 1.793 * Cr
    // G = 1.164 * Y' - 0.534 * Cr - 0.213 * Cb
    // B = 1.164 * Y' + 2.115 * Cb
    // Fixed-point with 10-bit shift (multiply by 1024).
    let (yr, cr_r, cb_b, cr_g, cb_g) = match yuv.matrix {
        makepad_platform::video_decode::yuv::YuvColorMatrix::BT601 => {
            (1192, 1634, 2066, -832, -401)
        }
        makepad_platform::video_decode::yuv::YuvColorMatrix::BT2020 => {
            (1192, 1749, 2230, -624, -149)
        }
        _ => {
            // BT709
            (1192, 1836, 2164, -547, -218)
        }
    };

    let (cw, _ch) = yuv.layout.chroma_size(yuv.width, yuv.height);
    let cw = cw as usize;

    for row in 0..h {
        let uv_row = match yuv.layout {
            makepad_platform::video_decode::yuv::YuvLayout::I420 => row / 2,
            _ => row,
        };
        for col in 0..w {
            let uv_col = match yuv.layout {
                makepad_platform::video_decode::yuv::YuvLayout::I420
                | makepad_platform::video_decode::yuv::YuvLayout::I422 => col / 2,
                _ => col,
            };

            let y_val = yuv.y[row * w + col] as i32 - 16;
            let u_val = if cw > 0 {
                yuv.u[uv_row * cw + uv_col] as i32 - 128
            } else {
                0
            };
            let v_val = if cw > 0 {
                yuv.v[uv_row * cw + uv_col] as i32 - 128
            } else {
                0
            };

            let r = ((yr * y_val + cr_r * v_val + 512) >> 10).clamp(0, 255) as u32;
            let g = ((yr * y_val + cr_g * v_val + cb_g * u_val + 512) >> 10).clamp(0, 255) as u32;
            let b = ((yr * y_val + cb_b * u_val + 512) >> 10).clamp(0, 255) as u32;

            // BGRA packed as u32
            pixels[row * w + col] = b | (g << 8) | (r << 16) | (0xFF << 24);
        }
    }

    pixels
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(800, 500)
                body +: {
                    main_view := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        padding: 16

                        Label{
                            text: "Video Format Test — AV1/H.264 in MP4 + MSE"
                            draw_text.text_style.font_size: 18
                        }
                        can_play_label := Label{
                            text: ""
                            draw_text.text_style.font_size: 9
                            draw_text.color: #888
                        }
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 16

                            View{
                                width: 240
                                height: Fit
                                flow: Down
                                spacing: 4
                                Label{ text: "AV1 (MP4)" }
                                av1_mp4_video := Video{
                                    width: 240
                                    height: 180
                                    is_looping: true
                                    mute: true
                                }
                            }

                            View{
                                width: 240
                                height: Fit
                                flow: Down
                                spacing: 4
                                Label{ text: "H.264 (MP4)" }
                                h264_mp4_video := Video{
                                    width: 240
                                    height: 180
                                    is_looping: true
                                    mute: true
                                }
                            }
                        }

                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 16

                            View{
                                width: 240
                                height: Fit
                                flow: Down
                                spacing: 4
                                Label{ text: "AV1 via MSE" }
                                av1_mse_image := Image{
                                    width: 240
                                    height: 180
                                }
                                av1_mse_status := Label{
                                    text: "loading..."
                                    draw_text.text_style.font_size: 9
                                    draw_text.color: #888
                                }
                            }

                            View{
                                width: 240
                                height: Fit
                                flow: Down
                                spacing: 4
                                Label{ text: "H.264 via MSE" }
                                h264_mse_image := Image{
                                    width: 240
                                    height: 180
                                }
                                h264_mse_status := Label{
                                    text: "loading..."
                                    draw_text.text_style.font_size: 9
                                    draw_text.color: #888
                                }
                            }

                            View{
                                width: 240
                                height: Fit
                                flow: Down
                                spacing: 4
                                Label{ text: "AV1 via MSE (test)" }
                                av1_mse_mid_image := Image{
                                    width: 240
                                    height: 180
                                }
                                av1_mse_mid_status := Label{
                                    text: "loading..."
                                    draw_text.text_style.font_size: 9
                                    draw_text.color: #888
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    sources_set: bool,
    #[rust]
    av1_mse_state: MseState,
    #[rust]
    av1_mse_mid_state: MseState,
    #[rust]
    h264_mse_state: MseState,
}

/// State for the MSE streaming demo.
struct MseState {
    /// Decoded frames ready for display.
    frames: Vec<MseDecodedFrame>,
    /// Current frame index for playback.
    frame_index: usize,
    /// Texture for displaying decoded frames.
    texture: Option<Texture>,
    /// Next frame timer for playback animation.
    next_frame: NextFrame,
    /// Whether MSE init has been done.
    initialized: bool,
    /// Status message.
    status: String,
}

impl Default for MseState {
    fn default() -> Self {
        Self {
            frames: Vec::new(),
            frame_index: 0,
            texture: None,
            next_frame: NextFrame(0),
            initialized: false,
            status: String::new(),
        }
    }
}

impl App {
    fn decode_mse_clip(data: &[u8]) -> Result<(Vec<MseDecodedFrame>, u32, u32, u128, bool), String> {
        let plugin = makepad_platform::media_plugin().ok_or_else(|| "no media plugin".to_string())?;
        let mut player = plugin.create_mse_playback_engine("video/mp4")?;

        let result = player.append_data(data)?;
        let mut frames = result.video_frames;
        let eos = player.end_of_stream()?;
        frames.extend(eos.video_frames);
        let init = result.init;
        let width = init
            .as_ref()
            .and_then(|init| init.video_tracks.first())
            .map(|track| track.width)
            .unwrap_or(0);
        let height = init
            .as_ref()
            .and_then(|init| init.video_tracks.first())
            .map(|track| track.height)
            .unwrap_or(0);
        let duration_ms = init.as_ref().map(|init| init.duration_ms).unwrap_or(0);

        Ok((frames, width, height, duration_ms, init.is_some()))
    }

    fn init_av1_mse(&mut self, cx: &mut Cx) {
        if self.av1_mse_state.initialized {
            return;
        }
        self.av1_mse_state.initialized = true;

        let (frames, width, height, duration_ms, init_segment_parsed) =
            match Self::decode_mse_clip(AV1_FRAG_MP4) {
                Ok(v) => v,
                Err(e) => {
                    self.av1_mse_state.status = format!("MSE error: {e}");
                    return;
                }
            };

        if frames.is_empty() {
            self.av1_mse_state.status = format!(
                "MSE: init={}, 0 frames decoded ({}×{})",
                init_segment_parsed, width, height
            );
            return;
        }

        self.av1_mse_state.status = format!(
            "MSE: {} frames, {}×{}, {}ms",
            frames.len(), width, height, duration_ms
        );

        let texture = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 {
                width: width as usize,
                height: height as usize,
                data: Some(yuv_frame_to_bgra(&frames[0])),
                updated: TextureUpdated::Full,
            },
        );

        let image_ref = self.ui.image(cx, ids!(av1_mse_image));
        image_ref.set_texture(cx, Some(texture.clone()));

        self.av1_mse_state.texture = Some(texture);
        self.av1_mse_state.frames = frames;
        self.av1_mse_state.frame_index = 0;
        self.av1_mse_state.next_frame = cx.new_next_frame();
    }

    fn init_av1_mse_mid(&mut self, cx: &mut Cx) {
        if self.av1_mse_mid_state.initialized {
            return;
        }
        self.av1_mse_mid_state.initialized = true;

        let (frames, width, height, duration_ms, init_segment_parsed) =
            match Self::decode_mse_clip(AV1_FRAG_MP4) {
                Ok(v) => v,
                Err(e) => {
                    self.av1_mse_mid_state.status = format!("MSE error: {e}");
                    return;
                }
            };

        if frames.is_empty() {
            self.av1_mse_mid_state.status = format!(
                "MSE: init={}, 0 frames decoded ({}×{})",
                init_segment_parsed, width, height
            );
            return;
        }

        self.av1_mse_mid_state.status = format!(
            "MSE: {} frames, {}×{}, {}ms",
            frames.len(), width, height, duration_ms
        );

        let texture = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 {
                width: width as usize,
                height: height as usize,
                data: Some(yuv_frame_to_bgra(&frames[0])),
                updated: TextureUpdated::Full,
            },
        );

        let image_ref = self.ui.image(cx, ids!(av1_mse_mid_image));
        image_ref.set_texture(cx, Some(texture.clone()));

        self.av1_mse_mid_state.texture = Some(texture);
        self.av1_mse_mid_state.frames = frames;
        self.av1_mse_mid_state.frame_index = 0;
        self.av1_mse_mid_state.next_frame = cx.new_next_frame();
    }

    fn init_h264_mse(&mut self, cx: &mut Cx) {
        if self.h264_mse_state.initialized {
            return;
        }
        self.h264_mse_state.initialized = true;

        let (frames, width, height, duration_ms, init_segment_parsed) =
            match Self::decode_mse_clip(H264_FRAG_MP4) {
                Ok(v) => v,
                Err(e) => {
                    self.h264_mse_state.status = format!("MSE error: {e}");
                    return;
                }
            };

        if frames.is_empty() {
            self.h264_mse_state.status = format!(
                "MSE: init={}, 0 frames decoded ({}×{})",
                init_segment_parsed, width, height
            );
            eprintln!("H264 MSE decoded 0 frames");
            return;
        }

        self.h264_mse_state.status = format!(
            "MSE: {} frames, {}×{}, {}ms",
            frames.len(), width, height, duration_ms
        );
        if let Some(first) = frames.first() {
            let y_avg = if first.yuv.y.is_empty() {
                0u32
            } else {
                first.yuv.y.iter().map(|v| *v as u32).sum::<u32>() / first.yuv.y.len() as u32
            };
            eprintln!(
                "H264 MSE decoded {} frames; first frame y_avg={} layout={:?}",
                frames.len(),
                y_avg,
                first.yuv.layout
            );
        }

        let first_pixels = yuv_frame_to_bgra(&frames[0]);
        let avg_luma = if first_pixels.is_empty() {
            0u32
        } else {
            first_pixels
                .iter()
                .map(|p| {
                    let b = (p & 0xFF) as u32;
                    let g = ((p >> 8) & 0xFF) as u32;
                    let r = ((p >> 16) & 0xFF) as u32;
                    (r + g + b) / 3
                })
                .sum::<u32>()
                / first_pixels.len() as u32
        };
        eprintln!("H264 first converted frame avg_rgb_luma={avg_luma}");

        let texture = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 {
                width: width as usize,
                height: height as usize,
                data: Some(first_pixels),
                updated: TextureUpdated::Full,
            },
        );

        let image_ref = self.ui.image(cx, ids!(h264_mse_image));
        image_ref.set_texture(cx, Some(texture.clone()));

        self.h264_mse_state.texture = Some(texture);
        self.h264_mse_state.frames = frames;
        self.h264_mse_state.frame_index = 0;
        self.h264_mse_state.next_frame = cx.new_next_frame();
    }

    fn advance_av1_mse_frame(&mut self, cx: &mut Cx) {
        if self.av1_mse_state.frames.is_empty() {
            return;
        }

        self.av1_mse_state.frame_index =
            (self.av1_mse_state.frame_index + 1) % self.av1_mse_state.frames.len();

        let frame = &self.av1_mse_state.frames[self.av1_mse_state.frame_index];
        let pixels = yuv_frame_to_bgra(frame);
        let w = frame.yuv.width as usize;
        let h = frame.yuv.height as usize;

        if let Some(texture) = &self.av1_mse_state.texture {
            texture.set_data_u32(cx, w, h, pixels);
        }

        let image_ref = self.ui.image(cx, ids!(av1_mse_image));
        if let Some(mut inner) = image_ref.borrow_mut() {
            inner.redraw(cx);
        }

        self.ui.label(cx, ids!(av1_mse_status)).set_text(
            cx,
            &format!(
                "frame {}/{} pts={}ms",
                self.av1_mse_state.frame_index + 1,
                self.av1_mse_state.frames.len(),
                frame.pts_ms,
            ),
        );

        self.av1_mse_state.next_frame = cx.new_next_frame();
    }

    fn advance_av1_mse_mid_frame(&mut self, cx: &mut Cx) {
        if self.av1_mse_mid_state.frames.is_empty() {
            return;
        }

        self.av1_mse_mid_state.frame_index =
            (self.av1_mse_mid_state.frame_index + 1) % self.av1_mse_mid_state.frames.len();

        let frame = &self.av1_mse_mid_state.frames[self.av1_mse_mid_state.frame_index];
        let pixels = yuv_frame_to_bgra(frame);
        let w = frame.yuv.width as usize;
        let h = frame.yuv.height as usize;

        if let Some(texture) = &self.av1_mse_mid_state.texture {
            texture.set_data_u32(cx, w, h, pixels);
        }

        let image_ref = self.ui.image(cx, ids!(av1_mse_mid_image));
        if let Some(mut inner) = image_ref.borrow_mut() {
            inner.redraw(cx);
        }

        self.ui.label(cx, ids!(av1_mse_mid_status)).set_text(
            cx,
            &format!(
                "frame {}/{} pts={}ms",
                self.av1_mse_mid_state.frame_index + 1,
                self.av1_mse_mid_state.frames.len(),
                frame.pts_ms,
            ),
        );

        self.av1_mse_mid_state.next_frame = cx.new_next_frame();
    }

    fn advance_h264_mse_frame(&mut self, cx: &mut Cx) {
        if self.h264_mse_state.frames.is_empty() {
            return;
        }

        self.h264_mse_state.frame_index =
            (self.h264_mse_state.frame_index + 1) % self.h264_mse_state.frames.len();

        let frame = &self.h264_mse_state.frames[self.h264_mse_state.frame_index];
        let pixels = yuv_frame_to_bgra(frame);
        let w = frame.yuv.width as usize;
        let h = frame.yuv.height as usize;

        if let Some(texture) = &self.h264_mse_state.texture {
            texture.set_data_u32(cx, w, h, pixels);
        }

        let image_ref = self.ui.image(cx, ids!(h264_mse_image));
        if let Some(mut inner) = image_ref.borrow_mut() {
            inner.redraw(cx);
        }

        self.ui.label(cx, ids!(h264_mse_status)).set_text(
            cx,
            &format!(
                "frame {}/{} pts={}ms",
                self.h264_mse_state.frame_index + 1,
                self.h264_mse_state.frames.len(),
                frame.pts_ms,
            ),
        );

        self.h264_mse_state.next_frame = cx.new_next_frame();
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_media::install();
        // Enable stdin/stdout control channel for havi-makepad-cli screenshots.
        if std::env::var("MAKEPAD_EVENTS").is_ok() {
            Cx::set_studio_stdout_mode(true);
            let (tx, rx) = mpsc::channel();
            Cx::set_control_channel(rx);
            std::thread::spawn(move || {
                use std::io::BufRead;
                let reader = std::io::BufReader::new(std::io::stdin().lock());
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if line.is_empty() { continue; }
                    match StudioToApp::deserialize_json(&line) {
                        Ok(msg) => {
                            if tx.send(msg).is_err() { break; }
                            SignalToUI::set_ui_signal();
                        }
                        Err(e) => eprintln!("control parse error: {e:?}: {line}"),
                    }
                }
            });
            use std::io::Write;
            let _ = std::io::stdout().write_all(b"{\"ReadyToStart\":null}\n");
            let _ = std::io::stdout().flush();
        }
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        if self.av1_mse_state.next_frame.is_event(event).is_some() {
            self.advance_av1_mse_frame(cx);
        }
        if self.av1_mse_mid_state.next_frame.is_event(event).is_some() {
            self.advance_av1_mse_mid_frame(cx);
        }
        if self.h264_mse_state.next_frame.is_event(event).is_some() {
            self.advance_h264_mse_frame(cx);
        }

        if !self.sources_set {
            let av1 = self.ui.video(cx, &[live_id!(av1_mp4_video)]);
            let h264 = self.ui.video(cx, &[live_id!(h264_mp4_video)]);
            if av1.borrow().is_none() || h264.borrow().is_none() {
                return;
            }

            av1.set_source_in_memory(Rc::new(AV1_MP4.to_vec()));
            av1.begin_playback(cx);

            h264.set_source_in_memory(Rc::new(H264_MP4.to_vec()));
            h264.begin_playback(cx);

            self.ui
                .label(cx, ids!(can_play_label))
                .set_text(cx, &can_play_report());

            self.init_av1_mse(cx);
            self.ui
                .label(cx, ids!(av1_mse_status))
                .set_text(cx, &self.av1_mse_state.status);

            self.init_av1_mse_mid(cx);
            self.ui
                .label(cx, ids!(av1_mse_mid_status))
                .set_text(cx, &self.av1_mse_mid_state.status);

            self.init_h264_mse(cx);
            self.ui
                .label(cx, ids!(h264_mse_status))
                .set_text(cx, &self.h264_mse_state.status);

            self.sources_set = true;
        }
    }
}
