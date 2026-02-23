use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::View,
    widget::*,
    window_voice_input::{VoiceInjectEvent, WindowVoiceInput},
};

const VOICE_TARGET_SAMPLE_RATE: usize = 16_000;
const VOICE_LOOP_SECONDS: usize = 1;
const WAVE_SAMPLES_PER_TEXEL: usize = 160; // 10ms @ 16k
const WAVE_TEX_WIDTH: usize =
    (VOICE_TARGET_SAMPLE_RATE * VOICE_LOOP_SECONDS) / WAVE_SAMPLES_PER_TEXEL;

const DISPLAY_TARGET_PEAK: f32 = 0.72;
const DISPLAY_MIN_REF: f32 = 0.003;
const DISPLAY_SILENCE_PEAK: f32 = 0.005;
const DISPLAY_MAX_GAIN: f32 = 10.0;
const DISPLAY_ATTACK: f32 = 0.20;
const DISPLAY_RELEASE: f32 = 0.06;
const DISPLAY_GAIN_UP: f32 = 0.08;
const DISPLAY_GAIN_DOWN: f32 = 0.22;
const DISPLAY_MU: f32 = 14.0;
const DISPLAY_SILENCE_CARRIER_BASE: f32 = 0.006;
const DISPLAY_SILENCE_CARRIER_SWING: f32 = 0.004;

#[derive(Clone, Debug, Default)]
pub enum VoiceWaveAction {
    #[default]
    None,
    Clicked(KeyModifiers),
    RecordVoice(bool),
    InjectText(String),
    InjectEnter,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    set_type_default() do #(DrawVoiceWave::script_shader(vm)){
        ..mod.draw.DrawQuad
        wave_tex: texture_2d(float)
        mic_on: 0.0
        voice_active: 0.0
        submit_flash: 0.0
        ring_head: 0.0

        pixel: fn() {
            let p = self.pos - vec2(0.5, 0.5)
            let r = length(p)

            let tau = 6.2831855
            let angle = atan2(p.y, p.x)
            let u = fract(angle / tau + 0.5 + self.ring_head + 0.25)
            let sample = self.wave_tex.sample(vec2(u, 0.5))

            // Decode two packed u16 values from one BGRA texel.
            // high16 -> (a,r), low16 -> (g,b)
            let hi_u = sample.w + sample.x / 256.0
            let lo_u = sample.y + sample.z / 256.0
            let v0 = clamp((hi_u - 0.5) * 2.0, -1.0, 1.0)
            let v1 = clamp((lo_u - 0.5) * 2.0, -1.0, 1.0)
            let amp = clamp(max(abs(v0), abs(v1)), 0.0, 1.0)
            let amp_vis = clamp(amp * 2.0, 0.0, 1.0)

            let ring_center = 0.41
            let ring_half = max((0.015 + amp_vis * 0.074) * self.mic_on, 0.0001)
            let ring_dist = abs(r - ring_center)
            let ring_mask = clamp(1.0 - ring_dist / ring_half, 0.0, 1.0)

            let halo_dist = abs(r - (ring_center + ring_half * 0.75))
            let halo_mask = clamp(1.0 - halo_dist / (0.018 + amp_vis * 0.024), 0.0, 1.0)
                * 0.36
                * self.mic_on

            let button_radius = 0.23
            let center_radius = 0.15
            let button_mask = clamp(1.0 - (r - button_radius) * 115.0, 0.0, 1.0)
            let center_mask = clamp(1.0 - (r - center_radius) * 150.0, 0.0, 1.0)

            let active_mix = clamp(self.voice_active + amp_vis * 0.7 * self.mic_on, 0.0, 1.0)
            let ring_off = vec3(0.56, 0.59, 0.58)
            let ring_on = vec3(1.0, 0.30, 0.34)
            let ring_hot = vec3(1.0, 0.50, 0.46)
            let ring_submit = vec3(1.0, 0.78, 0.33)
            let button_off = vec3(0.28, 0.30, 0.30)
            let button_on = vec3(0.90, 0.16, 0.20)
            let button_submit = vec3(1.0, 0.56, 0.22)

            let mut ring_color = ring_off.mix(ring_on, self.mic_on)
            ring_color = ring_color.mix(ring_hot, active_mix * 0.7)
            ring_color = ring_color.mix(ring_submit, self.submit_flash)

            let mut button_color = button_off.mix(button_on, self.mic_on)
            button_color = button_color.mix(button_submit, self.submit_flash)
            let center_hot = vec3(1.0, 0.36, 0.38).mix(vec3(1.0, 0.66, 0.36), self.submit_flash)
            let center_color = button_color.mix(center_hot, 0.22 + 0.18 * self.voice_active)

            let ring_alpha = clamp(ring_mask + halo_mask, 0.0, 1.0)
            let ring_fade = clamp(1.0 - (r - 0.50) * 18.0, 0.0, 1.0)
            let ring_alpha = ring_alpha * ring_fade

            let button_alpha = clamp(button_mask, 0.0, 1.0)
            let center_alpha = center_mask * button_alpha

            let mut color = vec3(0.0, 0.0, 0.0)
            color = color.mix(ring_color, ring_alpha)
            color = color.mix(button_color, button_alpha)
            color = color.mix(center_color, center_alpha)

            let alpha = clamp(ring_alpha + button_alpha, 0.0, 1.0)
            return vec4(color, alpha)
        }
    }

    mod.widgets.VoiceWaveBase = #(VoiceWave::register_widget(vm))
    mod.widgets.VoiceWave = set_type_default() do mod.widgets.VoiceWaveBase {
        width: 24
        height: 24
        margin: Inset{top: 1 right: 8}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVoiceWave {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    mic_on: f32,
    #[live]
    voice_active: f32,
    #[live]
    submit_flash: f32,
    #[live]
    ring_head: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct VoiceWave {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawVoiceWave,
    #[visible]
    #[live(true)]
    visible: bool,
    #[new]
    wave_texture: Texture,
    #[rust]
    texture_ready: bool,
    #[rust]
    append_min: f32,
    #[rust]
    append_max: f32,
    #[rust]
    display_env: f32,
    #[rust]
    display_gain: f32,
    #[rust]
    silence_phase: f32,
    #[rust]
    append_count: usize,
    #[rust]
    write_index: usize,
    #[rust]
    silence_bins_run: usize,
    #[rust]
    silence_frozen: bool,
    #[rust]
    animating: bool,
    #[rust]
    reset_on_next_append: bool,
    #[rust]
    voice_input: WindowVoiceInput,
    #[rust]
    voice_initialized: bool,
    #[rust]
    ptt_f1_down: bool,
    #[rust]
    ptt_owns_capture: bool,
}

impl VoiceWave {
    fn ensure_texture(&mut self, cx: &mut Cx) {
        if !self.texture_ready {
            self.wave_texture = Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    data: Some(vec![Self::min_max_to_texel(0.0, 0.0); WAVE_TEX_WIDTH]),
                    width: WAVE_TEX_WIDTH,
                    height: 1,
                    updated: TextureUpdated::Full,
                },
            );
            self.texture_ready = true;
        }
    }

    fn update_ring_head(&mut self) {
        self.draw_bg.ring_head = self.write_index as f32 / WAVE_TEX_WIDTH as f32;
    }

    fn signed_to_u16(v: f32) -> u32 {
        (((v.clamp(-1.0, 1.0) * 0.5) + 0.5) * 65535.0).round() as u32
    }

    fn compand_mu(v: f32) -> f32 {
        let x = v.clamp(-1.0, 1.0);
        let mag = x.abs();
        let y = ((1.0 + DISPLAY_MU * mag).ln() / (1.0 + DISPLAY_MU).ln()).clamp(0.0, 1.0);
        if x < 0.0 {
            -y
        } else {
            y
        }
    }

    fn gain_from_ref(reference_level: f32) -> f32 {
        if reference_level <= DISPLAY_MIN_REF {
            1.0
        } else {
            (DISPLAY_TARGET_PEAK / reference_level).clamp(1.0, DISPLAY_MAX_GAIN)
        }
    }

    fn normalize_min_max(min_v: f32, max_v: f32, gain: f32) -> (f32, f32) {
        (
            Self::compand_mu(min_v * gain),
            Self::compand_mu(max_v * gain),
        )
    }

    fn min_max_to_texel(min_v: f32, max_v: f32) -> u32 {
        let lo = Self::signed_to_u16(min_v.min(max_v));
        let hi = Self::signed_to_u16(max_v.max(min_v));
        (hi << 16) | lo
    }

    fn push_bin(&mut self, wave_buf: &mut [u32], min_v: f32, max_v: f32) -> bool {
        let peak = min_v.abs().max(max_v.abs());
        let is_silent_bin = peak < DISPLAY_SILENCE_PEAK;
        if is_silent_bin && self.silence_frozen {
            return false;
        }

        if peak < DISPLAY_SILENCE_PEAK {
            self.display_env += (DISPLAY_MIN_REF - self.display_env) * 0.08;
            self.silence_phase = (self.silence_phase + 0.35) % 6.2831855;
        } else {
            let coeff = if peak > self.display_env {
                DISPLAY_ATTACK
            } else {
                DISPLAY_RELEASE
            };
            self.display_env += (peak - self.display_env) * coeff;
        }
        let target_gain = Self::gain_from_ref(self.display_env);
        let gain_coeff = if target_gain < self.display_gain {
            DISPLAY_GAIN_DOWN
        } else {
            DISPLAY_GAIN_UP
        };
        self.display_gain += (target_gain - self.display_gain) * gain_coeff;

        let (mut min_raw, mut max_raw) = Self::normalize_min_max(min_v, max_v, self.display_gain);
        if peak < DISPLAY_SILENCE_PEAK {
            let carrier = DISPLAY_SILENCE_CARRIER_BASE
                + DISPLAY_SILENCE_CARRIER_SWING * self.silence_phase.sin().abs();
            let center = ((min_raw + max_raw) * 0.5).clamp(-0.03, 0.03);
            min_raw = (center - carrier).clamp(-0.08, 0.08);
            max_raw = (center + carrier).clamp(-0.08, 0.08);
        }

        wave_buf[self.write_index] = Self::min_max_to_texel(min_raw, max_raw);
        self.write_index = (self.write_index + 1) % WAVE_TEX_WIDTH;
        self.update_ring_head();
        if is_silent_bin {
            self.silence_bins_run = (self.silence_bins_run + 1).min(WAVE_TEX_WIDTH);
            if self.silence_bins_run >= WAVE_TEX_WIDTH {
                self.silence_frozen = true;
                self.animating = false;
            } else {
                self.animating = true;
            }
        } else {
            self.silence_bins_run = 0;
            self.silence_frozen = false;
            self.animating = true;
        }
        true
    }

    pub fn clear_wave(&mut self, cx: &mut Cx) {
        self.ensure_texture(cx);
        let mut wave_buf = self.wave_texture.take_vec_u32(cx);
        wave_buf.fill(Self::min_max_to_texel(0.0, 0.0));
        self.wave_texture.put_back_vec_u32(cx, wave_buf, None);
        self.append_min = 0.0;
        self.append_max = 0.0;
        self.display_env = DISPLAY_MIN_REF;
        self.display_gain = 1.0;
        self.silence_phase = 0.0;
        self.append_count = 0;
        self.write_index = 0;
        self.silence_bins_run = 0;
        self.silence_frozen = false;
        self.animating = false;
        self.update_ring_head();
        self.reset_on_next_append = false;
        self.draw_bg.redraw(cx);
    }

    pub fn set_wave_chunk(&mut self, cx: &mut Cx, samples: &[f32]) {
        self.ensure_texture(cx);
        let mut wave_buf = self.wave_texture.take_vec_u32(cx);
        wave_buf.fill(Self::min_max_to_texel(0.0, 0.0));

        self.append_count = 0;
        self.display_env = DISPLAY_MIN_REF;
        self.display_gain = 1.0;
        self.silence_phase = 0.0;
        self.write_index = 0;
        self.silence_bins_run = 0;
        self.silence_frozen = false;
        self.animating = true;

        if !samples.is_empty() {
            let keep = (WAVE_TEX_WIDTH * WAVE_SAMPLES_PER_TEXEL).min(samples.len());
            let start = samples.len() - keep;
            let recent = &samples[start..];

            for window in recent.chunks(WAVE_SAMPLES_PER_TEXEL) {
                let mut min_v = 1.0f32;
                let mut max_v = -1.0f32;
                for s in window {
                    min_v = min_v.min(*s);
                    max_v = max_v.max(*s);
                }
                if min_v > max_v {
                    min_v = 0.0;
                    max_v = 0.0;
                }
                let _ = self.push_bin(&mut wave_buf, min_v, max_v);
            }
        }

        self.wave_texture.put_back_vec_u32(cx, wave_buf, None);
        self.append_min = 0.0;
        self.append_max = 0.0;
        self.append_count = 0;
        self.reset_on_next_append = false;
        self.draw_bg.redraw(cx);
    }

    pub fn append_samples(&mut self, cx: &mut Cx, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        self.ensure_texture(cx);
        if self.reset_on_next_append {
            self.clear_wave(cx);
        }

        let mut wave_buf = self.wave_texture.take_vec_u32(cx);
        let mut wrote_any = false;

        for s in samples {
            if self.append_count == 0 {
                self.append_min = *s;
                self.append_max = *s;
            } else {
                self.append_min = self.append_min.min(*s);
                self.append_max = self.append_max.max(*s);
            }
            self.append_count += 1;

            if self.append_count >= WAVE_SAMPLES_PER_TEXEL {
                wrote_any |= self.push_bin(&mut wave_buf, self.append_min, self.append_max);
                self.append_count = 0;
            }
        }

        self.wave_texture.put_back_vec_u32(cx, wave_buf, None);
        if wrote_any {
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_mic_on(&mut self, cx: &mut Cx, on: bool) {
        self.draw_bg.mic_on = if on { 1.0 } else { 0.0 };
        if !on {
            self.clear_wave(cx);
        } else {
            self.silence_bins_run = 0;
            self.silence_frozen = false;
            self.animating = true;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_voice_active(&mut self, cx: &mut Cx, active: bool) {
        self.draw_bg.voice_active = if active { 1.0 } else { 0.0 };
        self.draw_bg.redraw(cx);
    }

    pub fn set_submit_flash(&mut self, cx: &mut Cx, flash: bool) {
        self.draw_bg.submit_flash = if flash { 1.0 } else { 0.0 };
        if flash {
            self.reset_on_next_append = true;
        }
        self.draw_bg.redraw(cx);
    }

    pub fn is_animating(&self) -> bool {
        self.animating
    }

    fn ensure_voice_initialized(&mut self, cx: &mut Cx) {
        if self.voice_initialized {
            return;
        }
        self.voice_initialized = true;
        self.voice_input.ensure_audio_callback(cx, 0);
    }

    fn sync_mic_state(&mut self, cx: &mut Cx) {
        let enabled = self.voice_input.is_enabled();
        self.set_mic_on(cx, enabled);
        if !enabled {
            self.voice_input.clear_pending();
            self.set_voice_active(cx, false);
            self.set_submit_flash(cx, false);
        }
        self.drain_and_refresh_voice(cx);
    }

    fn emit_inject_events(&self, cx: &mut Cx, events: Vec<VoiceInjectEvent>) {
        let uid = self.widget_uid();
        for event in events {
            match event {
                VoiceInjectEvent::Text(text) => {
                    cx.widget_action(uid, VoiceWaveAction::InjectText(text));
                }
                VoiceInjectEvent::Enter => {
                    cx.widget_action(uid, VoiceWaveAction::InjectEnter);
                }
            }
        }
    }

    /// Drain pending wave samples from voice_input into our own waveform display,
    /// then refresh visual state (voice_active, submit_flash, next_frame scheduling).
    fn drain_and_refresh_voice(&mut self, cx: &mut Cx) {
        // Drain pending wave samples
        while let Some(chunk) = self.voice_input.take_wave_chunk() {
            self.append_samples(cx, &chunk);
        }
        // Refresh visual state
        let state = self.voice_input.visual_state();
        self.set_voice_active(cx, state.voice_active);
        self.set_submit_flash(cx, state.submit_flash);
        if self.animating
            || state.voice_active
            || state.submit_flash
            || state.pending_wave
            || state.pending_inject
        {
            self.voice_input.request_next_frame(cx);
        }
    }

    fn set_enabled(&mut self, cx: &mut Cx, enabled: bool) {
        self.ensure_voice_initialized(cx);
        self.voice_input.set_enabled(cx, enabled);
        self.sync_mic_state(cx);
    }

    fn handle_voice_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ensure_voice_initialized(cx);
        let uid = self.widget_uid();

        if let Event::AudioDevices(devices) = event {
            self.voice_input.handle_audio_devices(cx, devices);
            self.sync_mic_state(cx);
        }
        if let Event::PermissionResult(result) = event {
            if self.voice_input.handle_permission_result(cx, result) {
                self.sync_mic_state(cx);
                cx.widget_action(
                    uid,
                    VoiceWaveAction::RecordVoice(self.voice_input.is_enabled()),
                );
            }
        }
        if let Event::Signal = event {
            let events = self.voice_input.process_signal_no_wave(cx);
            self.drain_and_refresh_voice(cx);
            self.emit_inject_events(cx, events);
        }
        if self.voice_input.is_timer_event(event) {
            self.drain_and_refresh_voice(cx);
            let events = self.voice_input.drain_ready_inject_events(cx);
            self.emit_inject_events(cx, events);
        }
        if let Event::Shutdown = event {
            self.voice_input.shutdown(cx);
        }

        // F1 push-to-talk and Cmd/Ctrl+1 toggle
        if let Event::KeyDown(key_event) = event {
            if key_event.key_code == KeyCode::F1 && !key_event.is_repeat {
                if !self.ptt_f1_down {
                    self.ptt_f1_down = true;
                    self.ptt_owns_capture = !self.voice_input.is_enabled();
                    if self.ptt_owns_capture {
                        self.set_enabled(cx, true);
                        cx.widget_action(uid, VoiceWaveAction::RecordVoice(true));
                    }
                }
            }
            let is_hotkey = !key_event.is_repeat
                && (key_event.modifiers.logo || key_event.modifiers.control)
                && key_event.key_code == KeyCode::Key1;
            if is_hotkey {
                let enabled = !self.voice_input.is_enabled();
                self.set_enabled(cx, enabled);
                cx.widget_action(uid, VoiceWaveAction::RecordVoice(enabled));
            }
        }
        if let Event::KeyUp(key_event) = event {
            if key_event.key_code == KeyCode::F1 && self.ptt_f1_down {
                self.ptt_f1_down = false;
                if self.ptt_owns_capture {
                    self.ptt_owns_capture = false;
                    self.set_enabled(cx, false);
                    self.voice_input.arm_enter_after_next_transcript();
                    cx.widget_action(uid, VoiceWaveAction::RecordVoice(false));
                }
            }
        }
    }
}

impl Widget for VoiceWave {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }

        self.handle_voice_event(cx, event);

        let uid = self.widget_uid();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerUp(fe) => {
                if fe.is_over {
                    let enabled = !self.voice_input.is_enabled();
                    self.set_enabled(cx, enabled);
                    cx.widget_action(uid, VoiceWaveAction::RecordVoice(enabled));
                    cx.widget_action(uid, VoiceWaveAction::Clicked(fe.modifiers));
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.ensure_texture(cx);
        self.draw_bg.draw_vars.set_texture(0, &self.wave_texture);
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }
}

impl VoiceWaveRef {
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            matches!(item.cast(), VoiceWaveAction::Clicked(_))
        } else {
            false
        }
    }

    /// Handle voice actions from the actions queue, injecting text/enter events
    /// into the given view. Returns `Some(enabled)` when voice recording state changed.
    pub fn handle_actions(
        &self,
        cx: &mut Cx,
        actions: &Actions,
        view: &mut View,
        scope: &mut Scope,
    ) -> Option<bool> {
        let mut result = None;
        for action in actions.filter_widget_actions_cast::<VoiceWaveAction>(self.widget_uid()) {
            match action {
                VoiceWaveAction::RecordVoice(enabled) => {
                    result = Some(enabled);
                }
                VoiceWaveAction::InjectText(text) => {
                    let text_event = Event::TextInput(TextInputEvent {
                        input: text,
                        replace_last: false,
                        was_paste: false,
                        ..Default::default()
                    });
                    view.handle_event(cx, &text_event, scope);
                }
                VoiceWaveAction::InjectEnter => {
                    let key = KeyEvent {
                        key_code: KeyCode::ReturnKey,
                        is_repeat: false,
                        modifiers: KeyModifiers::default(),
                        time: 0.0,
                    };
                    view.handle_event(cx, &Event::KeyDown(key), scope);
                    view.handle_event(cx, &Event::KeyUp(key), scope);
                }
                _ => {}
            }
        }
        result
    }

    pub fn set_enabled(&self, cx: &mut Cx, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_enabled(cx, enabled);
        }
    }

    pub fn is_enabled(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.voice_input.is_enabled()
        } else {
            false
        }
    }

    pub fn clear_wave(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_wave(cx);
        }
    }

    pub fn set_wave_chunk(&self, cx: &mut Cx, samples: &[f32]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_wave_chunk(cx, samples);
        }
    }

    pub fn append_samples(&self, cx: &mut Cx, samples: &[f32]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.append_samples(cx, samples);
        }
    }

    pub fn set_mic_on(&self, cx: &mut Cx, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_mic_on(cx, on);
        }
    }

    pub fn set_voice_active(&self, cx: &mut Cx, active: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_voice_active(cx, active);
        }
    }

    pub fn set_submit_flash(&self, cx: &mut Cx, flash: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_submit_flash(cx, flash);
        }
    }

    pub fn is_animating(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.is_animating()
        } else {
            false
        }
    }
}
