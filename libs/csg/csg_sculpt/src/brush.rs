//! Shared brush input, live strokes and haptics for desktop and XR.

use crate::layer::{Layer, Live};
use crate::math::*;
use crate::record::{quantise, Recorder};
use crate::stroke::{Mode, Stroke};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushSettings {
    pub mode: Mode,
    pub radius: f32,
    pub blend: f32,
    pub strength: f32,
    pub material: u8,
    pub chamfer: bool,
    pub bone: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushInput {
    /// Layer-frame surface hit or controller tip.
    pub tip: V3,
    /// Pressure 0..1; 1 for a mouse.
    pub pressure: f32,
    pub active: bool,
    pub invert: bool,
    pub smooth: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BrushEvent {
    Began,
    Updated { bricks: usize },
    Committed(Stroke),
    Haptic { strength: f32 },
}

pub struct Brush {
    pub settings: BrushSettings,
    recorder: Option<Recorder>,
    live: Option<Live>,
    mode: Mode,
    previous_inside: Option<bool>,
    updates: u64,
}

impl Brush {
    pub fn new(settings: BrushSettings) -> Brush {
        Brush { settings, recorder: None, live: None, mode: settings.mode, previous_inside: None, updates: 0 }
    }

    /// Modifiers choose the mode at the start of a stroke.
    pub fn effective_mode(&self, input: &BrushInput) -> Mode {
        if input.smooth {
            return Mode::Smooth;
        }
        match (self.settings.mode, input.invert) {
            (Mode::Add, true) => Mode::Carve,
            (Mode::Carve, true) => Mode::Add,
            (mode, _) => mode,
        }
    }

    pub fn update(&mut self, layer: &mut Layer, input: &BrushInput) -> Vec<BrushEvent> {
        let mut events = Vec::new();
        let s = layer.sample(input.tip);
        let inside = s < 0.0;
        if self.previous_inside.map_or(false, |previous| previous != inside) {
            events.push(BrushEvent::Haptic { strength: 0.5 });
        }
        self.previous_inside = Some(inside);
        self.updates = self.updates.wrapping_add(1);
        if self.is_drawing() && self.mode == Mode::Carve && inside && self.updates % 4 == 0 {
            events.push(BrushEvent::Haptic { strength: 0.1 });
        }

        if input.active {
            if let Some(live) = &mut self.live {
                let p = point(layer, input.tip, s, self.mode, live.stroke.radius);
                let recorder = self.recorder.as_mut().unwrap();
                recorder.push(p, input.pressure);
                layer.live_update(live, recorder.path());
                events.push(BrushEvent::Updated { bricks: live.last_touched });
            } else {
                self.mode = self.effective_mode(input);
                let settings = self.settings;
                let mut stroke = Stroke::new(self.mode, settings.radius, Vec::new())
                    .with_blend(settings.blend).with_strength(settings.strength).with_material(settings.material);
                stroke.chamfer = settings.chamfer;
                stroke.bone = settings.bone;
                if self.mode == Mode::Flatten {
                    // off the band the field is flat: fall back to up
                    let n = match normal(layer, input.tip) {
                        [0.0, 0.0, 0.0] => [0.0, 1.0, 0.0],
                        n => n,
                    };
                    stroke.plane = [quantise(n[0], 3), quantise(n[1], 3), quantise(n[2], 3), quantise(dot(n, input.tip), 3)];
                }
                let p = point(layer, input.tip, s, self.mode, settings.radius);
                let mut recorder = Recorder::new(settings.radius);
                recorder.push(p, input.pressure);
                stroke.path = recorder.path();
                let live = layer.live_begin(stroke);
                events.push(BrushEvent::Began);
                events.push(BrushEvent::Updated { bricks: live.last_touched });
                self.recorder = Some(recorder);
                self.live = Some(live);
            }
        } else if let Some(live) = self.live.take() {
            events.push(BrushEvent::Committed(layer.live_end(live)));
            self.recorder = None;
        }
        events
    }

    /// Restores the layer to before the running stroke.
    pub fn cancel(&mut self, layer: &mut Layer) {
        if let Some(live) = self.live.take() {
            layer.live_cancel(live);
        }
        self.recorder = None;
    }

    pub fn is_drawing(&self) -> bool {
        self.live.is_some()
    }
}

fn normal(layer: &Layer, tip: V3) -> V3 {
    let mut gradient = [0.0; 3];
    for a in 0..3 {
        let (mut lo, mut hi) = (tip, tip);
        lo[a] -= layer.cell;
        hi[a] += layer.cell;
        gradient[a] = layer.sample(hi) - layer.sample(lo);
    }
    normalize(gradient)
}

fn point(layer: &Layer, tip: V3, s: f32, mode: Mode, radius: f32) -> V3 {
    if mode == Mode::Carve && s.abs() < radius {
        sub(tip, scale(normal(layer, tip), 0.3 * radius))
    } else {
        tip
    }
}
