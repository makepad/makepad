use {
    crate::{makepad_platform::live_atomic::*, makepad_widgets::*},
    makepad_widgets::permission::{Permission, PermissionStatus},
    std::sync::Arc,
};

live_design! {
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;

    BoldLabel = <Label> {
        draw_text: {
            text_style: <THEME_FONT_BOLD> {}
        }
    }

    DeviceSelector = <View> {
        height: Fit
        align: {x: 0.0, y: 0.5}
        spacing: 5

        label = <BoldLabel> {}

        device_selector = <DropDown> {
            draw_text: {
                text_style: {font_size: 11}
            }
            popup_menu_position: BelowInput
            popup_menu: {
                width: 300, height: Fit,
                menu_item: <PopupMenuItem> {
                    width: Fill, height: Fit,
                    align: { y: 0.5 }
                    padding: {left: 15, right: 15, top: 10, bottom: 10}
                }
            }
            margin: 5
            labels: ["default"]
            values: [default]
        }
    }

    DevicesSelector = <View> {
        height: Fit, width: Fill
        flow: Down, spacing: 5
        <View> {
            height: Fit
            mic_selector = <DeviceSelector> {
                width: Fit
                label = { text: "Mic:"}
            }
            // mute_control = <MuteControl> {}
        }
        speaker_selector = <DeviceSelector> {
            label = { text: "Speaker:"}
        }
    }

    MicStats = <View> {
        height: Fit
        flow: Down, spacing: 10
        <BoldLabel> {
            text: "🎤  Microphone Capture"
            draw_text: {
                text_style: {font_size: 12}
            }
        }

        status_label = <BoldLabel> {
            text: "Status: Not started"
            draw_text: {
                color: #f88
            }
        }

        frames_label = <BoldLabel> {
            text: "Frames: 0"
            draw_text: {
                color: #aaa
            }
        }

        peak_label = <BoldLabel> {
            text: "Peak: 0.0000"
            draw_text: {
                color: #aaa
            }
        }

        samples_label = <BoldLabel> {
            text: "Samples: [waiting...]"
            draw_text: {
                color: #aaa
            }
        }

        level_bar = <View> {
            width: Fill
            height: 40
            show_bg: true
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return #222;
                }
            }

            level_fill = <View> {
                width: 0
                height: Fill
                show_bg: true
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        let level = self.pos.x;
                        if level < 0.5 {
                            return mix(#0f0, #ff0, level * 2.0);
                        } else {
                            return mix(#ff0, #f00, (level - 0.5) * 2.0);
                        }
                    }
                }
            }
        }

        callback_label = <BoldLabel> {
            text: "Callbacks: 0"
            draw_text: {
                color: #8f8
            }
        }
    }

    Playback = <View> {
        height: Fit
        flow: Down, spacing: 10
        <BoldLabel> {
            text: "🔊  Playback"
            draw_text: {
                text_style: {font_size: 12}
            }
        }

        passthrough_toggle = <Toggle> {
            draw_bg: {
                size: 20.0;
            }
            label_walk: {
                margin: <THEME_MSPACE_H_1> { left: (25.0 + THEME_SPACE_2) }
            }
            text: "Enable Mic Passthrough"
            draw_text: {
                text_style: {font_size: 12}
            }
        }

        <View> {
            height: Fit
            spacing: 10
            align: {x: 0.0, y: 0.5}

            <BoldLabel> {
                text: "Volume:"
                draw_text: {
                    text_style: {font_size: 11}
                }
            }

            volume_slider = <Slider> {
                width: 200
                height: 30
                min: 0.0
                max: 1.0
                step: 0.01
                default: 0.3
            }

            volume_label = <BoldLabel> {
                text: "30%"
                draw_text: {
                    text_style: {font_size: 11}
                    color: #aaa
                }
            }
        }

        passthrough_status = <BoldLabel> {
            text: "Status: Passthrough disabled"
            draw_text: {
                color: #f88
                text_style: {font_size: 11}
            }
        }
    }

    Header = <View> {
        align: {x: 0.5, y: 0.5}
        height: Fit
        <Label> {
            text: "Makead Audio Test"
            draw_text: {
                color: #f9
                text_style: <THEME_FONT_BOLD>{font_size: 15}
            }
        }
    }

    Separator = <View> {
        height: 1
        margin: 10
        show_bg: true
        draw_bg: {
            color: #8
        }
    }

    PermissionWarning = <View> {
        visible: false
        height: Fit
        spacing: 10
        flow: Down
        permission_warning_label = <BoldLabel> {
            text: "⚠️ Microphone access is required to use this app.
Click on the button below to request permission."
            draw_text: {
                color: #f88
            }
        }
        request_permission_button = <Button> {
            visible: false
            padding: 10
            text: "Trigger Permission Request"
        }
    }

    App = {{App}} {
        ui: <Window>{
            window: {inner_size: vec2(600, 800)},
            padding: 10
            show_bg: true
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(#3, mix(#7, #3, self.pos.y),self.pos.x);
                }
            }

            body = <View>{
                padding:20
                flow:Down
                spacing: 10

                <Header> {}
                permission_warning = <PermissionWarning> {}
                <DevicesSelector> {}
                <Separator> {}
                <MicStats> {}
                <Separator> {}
                <Playback> {}
            }
        }
    }
}

app_main!(App);

use std::sync::Mutex;

#[derive(Live, LiveAtomic, LiveHook, LiveRead, LiveRegister)]
#[live_ignore]
pub struct Store {
    #[live(0i64)]
    frame_count: i64a,
    #[live(0i64)]
    callback_count: i64a,
    #[live(0.0f64)]
    peak_level: f64a,
    #[live(0.0f64)]
    avg_level: f64a,
    #[live(0.3f64)]
    passthrough_volume: f64a,
    #[live(0.0f64)]
    input_sample_rate: f64a,
    #[rust]
    passthrough_enabled: Mutex<bool>,
    #[rust]
    samples: Mutex<Vec<f32>>,
    #[rust]
    passthrough_audio: Arc<Mutex<Vec<f32>>>,
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[live]
    store: Arc<Store>,
    #[rust]
    audio_update_signal: SignalToUI,
    #[rust]
    audio_devices: Vec<AudioDeviceDesc>,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        makepad_audio_graph::live_design(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        self.handle_device_selection(cx, actions);

        // Handle passthrough toggle
        if let Some(enabled) = self.ui.check_box(id!(passthrough_toggle)).changed(actions) {
            if let Ok(mut passthrough_enabled) = self.store.passthrough_enabled.lock() {
                *passthrough_enabled = enabled;
            }

            // Clear the buffer when toggling to prevent audio artifacts
            if let Ok(mut buffer) = self.store.passthrough_audio.lock() {
                buffer.clear();
            }

            if enabled {
                self.setup_passthrough_output(cx);
            }

            self.update_passthrough_ui(cx);
        }

        // Handle volume slider
        if let Some(volume) = self.ui.slider(id!(volume_slider)).slided(actions) {
            self.store.passthrough_volume.set(volume);
            self.update_volume_label(cx, volume);
        }

        if self
            .ui
            .button(id!(request_permission_button))
            .clicked(actions)
        {
            cx.request_permission(Permission::AudioInput);
        }
    }

    fn handle_startup(&mut self, cx: &mut Cx) {
        cx.request_permission(Permission::AudioInput);
    }

    fn handle_signal(&mut self, cx: &mut Cx) {
        if self.audio_update_signal.check_and_clear() {
            self.update_audio_stats(cx);
            self.update_samples_display(cx);

            // Update passthrough UI occasionally to reduce processing
            let callback_count = self.store.callback_count.get();
            let is_enabled = self
                .store
                .passthrough_enabled
                .try_lock()
                .map(|guard| *guard)
                .unwrap_or(false);
            if is_enabled && callback_count % 20 == 0 {
                self.update_passthrough_ui(cx);
            }

            self.ui.redraw(cx);
        }
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        let mut input_names = Vec::new();
        let mut output_names = Vec::new();
        let mut default_input_name = String::new();
        let mut default_output_name = String::new();

        devices
            .descs
            .iter()
            .for_each(|desc| match desc.device_type {
                AudioDeviceType::Input => {
                    input_names.push(desc.name.clone());
                    if desc.is_default {
                        default_input_name = desc.name.clone();
                    }
                }
                AudioDeviceType::Output => {
                    output_names.push(desc.name.clone());
                    if desc.is_default {
                        default_output_name = desc.name.clone();
                    }
                }
            });

        let mic_dropdown = self.ui.drop_down(id!(mic_selector.device_selector));
        mic_dropdown.set_labels(cx, input_names.clone());
        mic_dropdown.set_selected_by_label(&default_input_name, cx);

        let speaker_dropdown = self.ui.drop_down(id!(speaker_selector.device_selector));
        speaker_dropdown.set_labels(cx, output_names.clone());
        speaker_dropdown.set_selected_by_label(&default_output_name, cx);

        // Automatically switch to default devices
        // e.g. when a user connects headphones we assume they want to use them right away.
        // Note: we do not want to automatically switch to default devices if the user has already selected a non-default device, unless
        // the default device is new (wasn't present in the previous list)
        let default_input = devices.default_input();
        let default_output = devices.default_output();

        // The default device is new, assume we want to use it
        if !self
            .audio_devices
            .iter()
            .any(|d| d.device_type == AudioDeviceType::Input && d.device_id == default_input[0])
        {
            cx.use_audio_inputs(&default_input);
        }

        // The default device is new, assume we want to use it
        if !self
            .audio_devices
            .iter()
            .any(|d| d.device_type == AudioDeviceType::Output && d.device_id == default_output[0])
        {
            cx.use_audio_outputs(&default_output);
        }

        self.audio_devices = devices.descs.clone();
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        if let Event::PermissionResult(pr) = event {
            if pr.permission == Permission::AudioInput {
                log!("Permission result: {:?}", pr);
                match pr.status {
                    PermissionStatus::Granted => {
                        self.initialize(cx);
                        self.ui.view(id!(permission_warning)).set_visible(cx, false);
                    }
                    PermissionStatus::DeniedPermanent => {
                        self.ui.label(id!(permission_warning_label)).set_text(cx, "⚠️ Microphone permission denied.\nPlease enable microphone access in the system settings.");
                        self.ui.view(id!(permission_warning)).set_visible(cx, true);
                        self.ui.button(id!(request_permission_button)).set_visible(cx, false);
                    }
                    PermissionStatus::DeniedCanRetry => {
                        self.ui.view(id!(permission_warning)).set_visible(cx, true);
                        self.ui.button(id!(request_permission_button)).set_visible(cx, true);
                    }
                    _ => {}
                }
            }
        }
    }
}

impl App {
    fn initialize(&mut self, cx: &mut Cx) {
        self.start_audio_test(cx);

        // Initialize passthrough UI
        self.update_passthrough_ui(cx);
        self.update_volume_label(cx, self.store.passthrough_volume.get());

        self.ui.redraw(cx);
    }

    fn handle_device_selection(&mut self, cx: &mut Cx, actions: &Actions) {
        // Handle speaker selection
        let speaker_dropdown = self.ui.drop_down(id!(speaker_selector.device_selector));
        if let Some(_id) = speaker_dropdown.changed(actions) {
            if let Some(device) = self.find_device_by_name(&speaker_dropdown.selected_label()) {
                cx.use_audio_outputs(&[device.device_id]);
            }
        }

        // Handle microphone selection
        let microphone_dropdown = self.ui.drop_down(id!(mic_selector.device_selector));
        if let Some(_id) = microphone_dropdown.changed(actions) {
            if let Some(device) = self.find_device_by_name(&microphone_dropdown.selected_label()) {
                cx.use_audio_inputs(&[device.device_id]);
            }
        }
    }

    fn find_device_by_name(&self, name: &str) -> Option<&AudioDeviceDesc> {
        self.audio_devices.iter().find(|device| device.name == name)
    }

    pub fn start_audio_test(&mut self, cx: &mut Cx) {
        self.ui
            .label(id!(status_label))
            .set_text(cx, "Status: Setting up audio input...");
        self.ui.redraw(cx);

        let store = self.store.clone();
        let audio_signal = self.audio_update_signal.clone();
        let mut frame_counter = 0i64;
        let mut callback_counter = 0i64;
        let mut update_counter = 0u32;

        // Audio input capture
        cx.audio_input(0, move |info, input_buffer| {
            frame_counter += input_buffer.frame_count as i64;
            callback_counter += 1;

            // Store input sample rate for resampling
            store.input_sample_rate.set(info.sample_rate);

            // Force immediate UI update for first callback to test if we're getting called
            if callback_counter == 1 {
                audio_signal.set(); // Force immediate UI update to show we got our first callback
            }

            // Calculate audio statistics from first channel only
            let channel_0 = input_buffer.channel(0);
            let mut peak = 0.0f32;
            let mut avg = 0.0f32;

            for &sample in channel_0 {
                let abs_val = sample.abs();
                avg += abs_val;
                if abs_val > peak {
                    peak = abs_val;
                }
            }

            if !channel_0.is_empty() {
                avg /= channel_0.len() as f32;
            }

            // Update store values
            store.frame_count.set(frame_counter);
            store.callback_count.set(callback_counter);
            store.peak_level.set(peak as f64);
            store.avg_level.set(avg as f64);

            // Store first few samples for display
            if let Ok(mut samples) = store.samples.lock() {
                samples.clear();
                let channel_0 = input_buffer.channel(0);
                for i in 0..20.min(channel_0.len()) {
                    samples.push(channel_0[i]);
                }
            }

            // Store audio data for passthrough if enabled
            if let Ok(passthrough_enabled) = store.passthrough_enabled.try_lock() {
                if *passthrough_enabled {
                    if let Ok(mut passthrough_audio) = store.passthrough_audio.try_lock() {
                        let channel_0 = input_buffer.channel(0);

                        // Append new audio data to buffer
                        passthrough_audio.extend_from_slice(channel_0);

                        // Keep buffer size reasonable to prevent excessive latency
                        const MAX_BUFFER_SIZE: usize = 1200; // ~25ms at 48kHz
                        if passthrough_audio.len() > MAX_BUFFER_SIZE {
                            let excess = passthrough_audio.len() - MAX_BUFFER_SIZE;
                            passthrough_audio.drain(..excess);
                        }
                    }
                }
            }

            update_counter += 1;

            // Update UI every 5 callbacks (roughly 10Hz at typical rates)
            if update_counter >= 5 {
                update_counter = 0;
                audio_signal.set();
            }

            // Also update immediately if we detect audio for the first time
            if callback_counter == 1 || (callback_counter < 10 && peak > 0.01) {
                audio_signal.set();
            }
        });

        // Update status to show callback is registered
        self.ui
            .label(id!(status_label))
            .set_text(cx, "Status: Audio callback registered, waiting for data...");

        self.ui.redraw(cx);
    }

    fn setup_passthrough_output(&mut self, cx: &mut Cx) {
        let store = self.store.clone();
        let mut first_output = true;

        // Audio output callback for mic passthrough
        cx.audio_output(0, move |info, output_buffer| {
            if first_output {
                first_output = false;
            }
            // Always start with silence
            output_buffer.zero();

            // Only play if passthrough is enabled (use try_lock to avoid blocking)
            let is_enabled = if let Ok(passthrough_enabled) = store.passthrough_enabled.try_lock() {
                *passthrough_enabled
            } else {
                false
            };

            if !is_enabled {
                return;
            }

            // Get volume setting
            let volume = store.passthrough_volume.get() as f32;
            let input_rate = store.input_sample_rate.get();
            let output_rate = info.sample_rate;

            if let Ok(mut passthrough_audio) = store.passthrough_audio.try_lock() {
                if !passthrough_audio.is_empty() {
                    let frame_count = output_buffer.frame_count();
                    let channel_count = output_buffer.channel_count();

                    // Check if we need to resample
                    // When using bult-in speaker and microphone, in most systems the sample rate will be 48kHz for both
                    // and no resampling is needed, the mic audio can be directly passed to the speaker.
                    // However when using headphones, specially bluetooth ones, the mic sample rate can be quite different (e.g. 44.1kHz or 16kHz or others)
                    let needs_resampling = (input_rate - output_rate).abs() > 1.0;

                    if needs_resampling && input_rate > 0.0 {
                        // Resample using linear interpolation
                        let ratio = input_rate / output_rate;

                        for frame_idx in 0..frame_count {
                            let src_pos = frame_idx as f64 * ratio;
                            let src_idx = src_pos as usize;

                            let sample = if src_idx < passthrough_audio.len() {
                                if src_idx + 1 < passthrough_audio.len() {
                                    // Linear interpolation between two samples
                                    let frac = (src_pos - src_idx as f64) as f32;
                                    let s0 = passthrough_audio[src_idx];
                                    let s1 = passthrough_audio[src_idx + 1];
                                    s0 + (s1 - s0) * frac
                                } else {
                                    passthrough_audio[src_idx]
                                }
                            } else {
                                break; // Not enough input samples
                            };

                            let scaled_sample = (sample * volume).clamp(-1.0, 1.0);

                            // Write to all output channels
                            for channel_idx in 0..channel_count {
                                let channel = output_buffer.channel_mut(channel_idx);
                                channel[frame_idx] = scaled_sample;
                            }
                        }

                        // Calculate how many input samples were consumed
                        let consumed = ((frame_count as f64 * ratio) as usize).min(passthrough_audio.len());
                        if consumed > 0 {
                            passthrough_audio.drain(..consumed);
                        }
                    } else {
                        // No resampling needed - direct copy
                        let samples_available = passthrough_audio.len().min(frame_count);

                        for frame_idx in 0..samples_available {
                            let sample = passthrough_audio[frame_idx];
                            let scaled_sample = (sample * volume).clamp(-1.0, 1.0);

                            // Write to all output channels (mono to stereo/multi-channel)
                            for channel_idx in 0..channel_count {
                                let channel = output_buffer.channel_mut(channel_idx);
                                channel[frame_idx] = scaled_sample;
                            }
                        }

                        // Remove consumed samples from the front of the buffer
                        if samples_available > 0 {
                            passthrough_audio.drain(..samples_available);
                        }
                    }
                }
            }
        });
    }

    fn update_passthrough_ui(&mut self, cx: &mut Cx) {
        let is_enabled = if let Ok(passthrough_enabled) = self.store.passthrough_enabled.try_lock()
        {
            *passthrough_enabled
        } else {
            false
        };

        let (status_text, color) = if is_enabled {
            (
                "Status: ✅ Passthrough active".to_string(),
                vec4(0.5, 1.0, 0.5, 1.0),
            )
        } else {
            (
                "Status: Passthrough disabled".to_string(),
                vec4(1.0, 0.5, 0.5, 1.0),
            )
        };

        self.ui
            .label(id!(passthrough_status))
            .set_text(cx, &status_text);
        self.ui.label(id!(passthrough_status)).apply_over(
            cx,
            live! {
                draw_text: { color: (color) }
            },
        );

        self.ui.redraw(cx);
    }

    fn update_volume_label(&mut self, cx: &mut Cx, volume: f64) {
        let percentage = (volume * 100.0) as i32;
        self.ui
            .label(id!(volume_label))
            .set_text(cx, &format!("{}%", percentage));
        self.ui.redraw(cx);
    }

    fn update_audio_stats(&mut self, cx: &mut Cx) {
        let frame_count = self.store.frame_count.get();
        let callback_count = self.store.callback_count.get();
        let peak = self.store.peak_level.get();
        let avg = self.store.avg_level.get();

        // Update status label
        let status = if callback_count == 0 {
            "Status: ❌ No callbacks received!"
        } else if peak > 0.001 {
            "Status: ✅ AUDIO ACTIVE!"
        } else {
            "Status: ⚠️ Callbacks OK, but silent"
        };
        self.ui.label(id!(status_label)).set_text(cx, status);

        // Update frames and peak labels
        self.ui
            .label(id!(frames_label))
            .set_text(cx, &format!("Frames: {}", frame_count));
        self.ui
            .label(id!(peak_label))
            .set_text(cx, &format!("Peak: {:.4} | Avg: {:.4}", peak, avg));

        // Update callback counter with frequency
        let frequency = if frame_count > 0 {
            callback_count * 48000 / frame_count
        } else {
            0
        };
        self.ui.label(id!(callback_label)).set_text(
            cx,
            &format!("Callbacks: {} ({}Hz)", callback_count, frequency),
        );

        // Update level bar
        let width = (peak.min(1.0) * 360.0) as f64;
        self.ui
            .view(id!(level_fill))
            .apply_over(cx, live! {width: (width)});
    }

    fn update_samples_display(&mut self, cx: &mut Cx) {
        if let Ok(samples) = self.store.samples.lock() {
            if !samples.is_empty() {
                let samples_str: Vec<String> = samples
                    .iter()
                    .take(10)
                    .map(|s| format!("{:.3}", s))
                    .collect();
                self.ui
                    .label(id!(samples_label))
                    .set_text(cx, &format!("Samples: [{}]", samples_str.join(", ")));
            } else {
                self.ui
                    .label(id!(samples_label))
                    .set_text(cx, "Samples: [no data yet]");
            }
        }
    }
}
