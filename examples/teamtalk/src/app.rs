/*
TeamTalk is a LAN (wired only) p2p audiochat supporting as many clients as you have bandwidth.
For 6 clients it should pull about 25 megabits. You can use it to have a super low latency
helicopter-headset experience, silent disco, and so on.
This example shows using networking and audio IO
*/

use makepad_draw2::*;
use makepad_draw2::makepad_platform::{
    audio::AudioBuffer,
    audio_stream::AudioStreamSender,
    makepad_micro_serde::*,
};
use std::net::UdpSocket;
use std::time::Duration;

// Network standard sample rate - all audio is transmitted at this rate
const NETWORK_SAMPLE_RATE: f64 = 44100.0;
// Maximum samples in a wire packet (640 i16 values = 1280 bytes)
// For mono: 640 frames, for stereo: 320 frames
const MAX_WIRE_SAMPLES: usize = 640;

/// Simple linear interpolation resampler
fn resample(input: &AudioBuffer, from_rate: f64, to_rate: f64) -> AudioBuffer {
    if (from_rate - to_rate).abs() < 1.0 {
        return input.clone();
    }
    
    let ratio = to_rate / from_rate;
    let new_frame_count = ((input.frame_count() as f64 * ratio).round() as usize).max(1);
    let mut output = AudioBuffer::new_with_size(new_frame_count, input.channel_count());
    
    for chan in 0..input.channel_count() {
        let inp = input.channel(chan);
        let out = output.channel_mut(chan);
        for i in 0..new_frame_count {
            let src_pos = i as f64 / ratio;
            let src_idx = src_pos as usize;
            let frac = (src_pos - src_idx as f64) as f32;
            
            let sample0 = inp.get(src_idx).copied().unwrap_or(0.0);
            let sample1 = inp.get(src_idx + 1).copied().unwrap_or(sample0);
            out[i] = sample0 + (sample1 - sample0) * frac;
        }
    }
    
    output
}

/// Smooth limiter with fast attack, slow release
fn apply_limiter(buf: &mut [f32], limiter_gain: &mut f32) {
    const MAX_VOLUME: f32 = 0.6;
    const TARGET_GAIN: f32 = 0.1;
    const ATTACK_COEF: f32 = 0.3;
    const RELEASE_COEF: f32 = 0.001;
    
    for v in buf.iter_mut() {
        let desired = if v.abs() > MAX_VOLUME { TARGET_GAIN } else { 1.0 };
        if desired < *limiter_gain {
            *limiter_gain += (desired - *limiter_gain) * ATTACK_COEF;
        } else {
            *limiter_gain += (desired - *limiter_gain) * RELEASE_COEF;
        }
        *v *= *limiter_gain;
    }
}

/// Calculate average peak level
fn calculate_peak(buf: &[f32]) -> f32 {
    let sum: f32 = buf.iter().map(|v| v.abs()).sum();
    sum / buf.len() as f32
}

/// Logarithmic fade-in
fn apply_fade_in(buf: &mut [f32], fade_samples: usize) {
    let ramp_len = fade_samples.min(buf.len());
    let k = 3.0_f32;
    let norm = k.exp() - 1.0;
    for i in 0..ramp_len {
        let t = i as f32 / ramp_len as f32;
        let gain = ((k * t).exp() - 1.0) / norm;
        buf[i] *= gain;
    }
}

/// Logarithmic fade-out
fn apply_fade_out(buf: &mut [f32], fade_samples: usize) {
    let ramp_len = fade_samples.min(buf.len());
    let k = 3.0_f32;
    let norm = k.exp() - 1.0;
    for i in 0..ramp_len {
        let t = i as f32 / ramp_len as f32;
        let gain = 1.0 - ((k * t).exp() - 1.0) / norm;
        buf[i] *= gain;
    }
    // Zero remainder after fade
    for i in ramp_len..buf.len() {
        buf[i] = 0.0;
    }
}

app_main!(App);

script_run!{
    use mod.std.*;
    #(App::script_api(vm)){
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_draw2::script_run(vm);
        App::script_run(vm, script_run)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[script] window: WindowHandle,
    #[script] pass: Pass,
    #[script] main_draw_list: DrawList2d,
}

// this is the protocol enum with 'micro-serde' binary serialise/deserialise macro on it.
#[derive(SerBin, DeBin, Debug)]
enum TeamTalkWire {
    Silence { client_uid: u64, frame_count: u32 },
    Audio { client_uid: u64, channel_count: u32, data: Vec<i16> },
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.window.set_pass(cx, &self.pass);
        self.pass.set_window_clear_color(cx, vec4(0.2, 0.2, 0.3, 1.0));
        self.start_network_stack(cx);
    }

    fn handle_draw_2d(&mut self, cx: &mut Cx2d) {
        if !cx.will_redraw(&mut self.main_draw_list, Walk::default()) {
            return
        }

        cx.begin_pass(&self.pass, None);
        self.main_draw_list.begin_always(cx);

        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());

        // No UI - just audio streaming

        cx.end_pass_sized_turtle();
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass);
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        for desc in &devices.descs {
            println!("{}", desc)
        }
        cx.use_audio_inputs(&devices.default_input());
        cx.use_audio_outputs(&devices.default_output());
    }
    
    fn handle_signal(&mut self, _cx: &mut Cx) {
        // Placeholder for signal handling
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let _ = self.match_event_with_draw_2d(cx, event);
    }
}

impl App {
    pub fn start_network_stack(&mut self, cx: &mut Cx) {
        // not a very good uid, but it'll do.
        let my_client_uid = LiveId::from_str(&format!("{:?}", std::time::SystemTime::now())).0;
        
        // AudioStream is an mpsc channel that buffers at the recv side
        // and allows arbitrary chunksized reads. Little utility struct.
        // platform2's create_pair takes (min_buf, max_buf) at creation time
        let (mic_send, mut mic_recv) = AudioStreamSender::create_pair(1, 4);
        let (mix_send, mut mix_recv) = AudioStreamSender::create_pair(1, 4);

        // the UDP broadcast socket
        let write_audio = UdpSocket::bind("0.0.0.0:41531").unwrap();
        write_audio.set_read_timeout(Some(Duration::new(5, 0))).unwrap();
        write_audio.set_broadcast(true).unwrap();

        let read_audio = write_audio.try_clone().unwrap();
        
        // our microphone broadcast network thread
        // Buffer adapts to input channel count: mono=640 frames, stereo=320 frames
        std::thread::spawn(move || {
            let mut wire_data = Vec::new();
            let mut output_buffer = AudioBuffer::default();
            let mut was_silent = true;
            let fade_in_samples = 280; // ~6ms at 44100Hz
            let mut limiter_gain = 1.0_f32;
            
            loop {
                mic_recv.recv_stream();
                loop {
                    // Resize buffer based on channel count from input
                    // MAX_WIRE_SAMPLES total samples: mono=640 frames, stereo=320 frames
                    let channel_count = output_buffer.channel_count().max(1);
                    let frame_count = MAX_WIRE_SAMPLES / channel_count;
                    output_buffer.resize(frame_count, channel_count);
                    
                    if mic_recv.read_buffer(0, &mut output_buffer) == 0 {
                        break;
                    }
                    
                    let channel_count = output_buffer.channel_count();
                    let frame_count = output_buffer.frame_count();
                    
                    // Process all channels: limiter, silence detection, fades
                    let mut peak = 0.0_f32;
                    for ch in 0..channel_count {
                        let buf = output_buffer.channel_mut(ch);
                        apply_limiter(buf, &mut limiter_gain);
                        peak = peak.max(calculate_peak(buf));
                    }
                    
                    let is_active = peak > 0.001;
                    let wire_packet = match (is_active, was_silent) {
                        (true, true) => {
                            // Fade in from silence
                            for ch in 0..channel_count {
                                apply_fade_in(output_buffer.channel_mut(ch), fade_in_samples);
                            }
                            was_silent = false;
                            TeamTalkWire::Audio {
                                client_uid: my_client_uid,
                                channel_count: channel_count as u32,
                                data: output_buffer.to_i16()
                            }
                        }
                        (true, false) => {
                            // Active, no transition
                            was_silent = false;
                            TeamTalkWire::Audio {
                                client_uid: my_client_uid,
                                channel_count: channel_count as u32,
                                data: output_buffer.to_i16()
                            }
                        }
                        (false, false) => {
                            // Fade out to silence
                            for ch in 0..channel_count {
                                apply_fade_out(output_buffer.channel_mut(ch), fade_in_samples);
                            }
                            was_silent = true;
                            TeamTalkWire::Audio {
                                client_uid: my_client_uid,
                                channel_count: channel_count as u32,
                                data: output_buffer.to_i16()
                            }
                        }
                        (false, true) => {
                            // Still silent
                            TeamTalkWire::Silence {
                                client_uid: my_client_uid,
                                frame_count: frame_count as u32
                            }
                        }
                    };
                    
                    wire_data.clear();
                    wire_packet.ser_bin(&mut wire_data);
                    let _ = write_audio.send_to(&wire_data, "10.0.0.255:41531");
                }
            }
        });
        
        // the network audio receiving thread
        std::thread::spawn(move || {
            let mut read_buf = [0u8; 4096];

            while let Ok((len, _addr)) = read_audio.recv_from(&mut read_buf) {
                let read_buf = &read_buf[0..len];

                let packet = match TeamTalkWire::deserialize_bin(read_buf) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // create an audiobuffer from the data
                // Received data keeps its original channel count (mono or stereo)
                let (client_uid, buffer) = match packet {
                    TeamTalkWire::Audio { client_uid, channel_count, data } => {
                        let buffer = AudioBuffer::from_i16(&data, channel_count as usize);
                        (client_uid, buffer)
                    }
                    TeamTalkWire::Silence { client_uid, frame_count } => {
                        // Silence packets are mono (1 channel)
                        (client_uid, AudioBuffer::new_with_size(frame_count as usize, 1))
                    }
                };

                if client_uid != my_client_uid {
                    // platform2 uses send() instead of write_buffer()
                    let _ = mix_send.send(client_uid, buffer);
                }
            }
        });

        cx.audio_input(0, move |info, input_buffer| {
            let mut input_buffer = input_buffer.clone();
            // Inputs send mono to save network bandwidth
            input_buffer.make_single_channel();
            // Resample to network rate before sending
            let resampled = resample(&input_buffer, info.sample_rate, NETWORK_SAMPLE_RATE);
            let _ = mic_send.send(0, resampled);
        });

        let volume = 7.0f32; // output volume multiplier
        cx.audio_output(0, move |info, output_buffer| {
            output_buffer.zero();
            mix_recv.try_recv_stream();
            
            let out_channels = output_buffer.channel_count();
            let out_frames = output_buffer.frame_count();
            
            // Calculate how many frames we need at network rate to fill output buffer
            let ratio = NETWORK_SAMPLE_RATE / info.sample_rate;
            let network_frames = (out_frames as f64 * ratio).ceil() as usize;
            
            // We read into a mono buffer since network data may be mono
            // (saves bandwidth - inputs only send mono)
            let mut network_buf = AudioBuffer::new_with_size(network_frames, 1);
            
            for i in 0..mix_recv.num_routes() {
                if mix_recv.read_buffer(i, &mut network_buf) != 0 {
                    // Resample from network rate to device rate
                    let resampled = resample(&network_buf, NETWORK_SAMPLE_RATE, info.sample_rate);
                    let src_channels = resampled.channel_count();
                    let copy_frames = resampled.frame_count().min(out_frames);
                    
                    // Mix into output, upmixing mono to stereo if needed
                    for frame in 0..copy_frames {
                        for out_ch in 0..out_channels {
                            // If source is mono, use channel 0 for all output channels
                            let src_ch = if src_channels == 1 { 0 } else { out_ch.min(src_channels - 1) };
                            let src_sample = resampled.channel(src_ch)[frame];
                            output_buffer.channel_mut(out_ch)[frame] += src_sample * volume;
                        }
                    }
                }
            }
        });
    }
}
