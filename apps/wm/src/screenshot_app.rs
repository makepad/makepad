//! Screenshots, the WM's half: the hotkeys, the pointer while a selection or a
//! thumbnail is up, the frame grab and the hand-over of the saved file's path to
//! the window it is dropped on. The state and the overlay are `screenshot.rs`.

use crate::screenshot::{
    bgra_to_frame, crop, pixel_rect, save_async, selection_rect, thumb_rect, Frame, Kind, Phase, Saved, Select, Thumb, ThumbDrag,
    DRAG_SLOP, SWIPE_EDGE, THUMB_EASE, THUMB_LIFE,
};
use crate::*;
use makepad_widgets::makepad_platform::screen_capture::{add_screen_capture, remove_screen_capture, ScreenCaptureOptions};
use std::sync::{Arc, Mutex};

/// How many frames a grab waits for the readback before giving up.
const GRAB_TRIES: u32 = 240;

fn to_rect(r: LRect) -> Rect {
    Rect { pos: dvec2(r.x, r.y), size: dvec2(r.w, r.h) }
}

impl App {
    /// The screen the pointer is on, in window points.
    fn shot_screen(&mut self) -> Rect {
        let state = self.state_mut();
        let rects = state.screens.rects();
        match rects.get(state.screens.active) {
            Some(r) => to_rect(*r),
            None => rects.first().map_or(Rect { pos: dvec2(0.0, 0.0), size: dvec2(1280.0, 800.0) }, |r| to_rect(*r)),
        }
    }

    /// Super+Shift+4 / Super+Shift+3, and the keys of a selection in progress.
    pub(super) fn shot_key(&mut self, cx: &mut Cx, e: &KeyEvent) -> bool {
        if self.state_mut().shot.selecting() {
            match e.key_code {
                KeyCode::Escape => self.shot_cancel(cx),
                KeyCode::Space => {
                    if let Phase::Selecting(s) = &mut self.state_mut().shot.phase {
                        s.pick_window = !s.pick_window;
                        s.anchor = None;
                    }
                    self.shot_update_window(cx);
                }
                _ => {}
            }
            self.redraw_all(cx);
            // The keyboard is the selection's while it is up.
            return true;
        }
        if !(e.modifiers.logo && e.modifiers.shift && !e.modifiers.control && !e.modifiers.alt) {
            return false;
        }
        let kind = match e.key_code {
            KeyCode::Key4 => Kind::Region,
            KeyCode::Key3 => Kind::Screen,
            _ => return false,
        };
        if !e.is_repeat {
            self.shot_begin(cx, kind);
        }
        true
    }

    /// Ask for the next presented frame, before any overlay is drawn over it.
    pub(super) fn shot_begin(&mut self, cx: &mut Cx, kind: Kind) {
        if self.state_mut().shot.busy() {
            return;
        }
        let Some(window) = self.ui.window(cx, ids!(main_window)).window_id() else { return };
        let screen = self.shot_screen();
        let slot: Arc<Mutex<Option<Frame>>> = Arc::new(Mutex::new(None));
        let sink_slot = slot.clone();
        let sink = add_screen_capture(ScreenCaptureOptions { window_id: Some(window.id()), max_fps: 0.0 }, move |f| {
            if let Ok(mut held) = sink_slot.try_lock() {
                if held.is_none() {
                    *held = Some(Frame { width: f.width, height: f.height, rgba: f.rgba.to_vec() });
                }
            }
        });
        self.state_mut().shot.phase = Phase::Grabbing { kind, sink, slot, tries: 0, screen };
        self.shot_next = cx.new_next_frame();
        self.redraw_all(cx);
    }

    pub(super) fn shot_cancel(&mut self, cx: &mut Cx) {
        let shot = &mut self.state_mut().shot;
        if let Phase::Grabbing { sink, .. } = &shot.phase {
            remove_screen_capture(*sink);
        }
        shot.phase = Phase::Idle;
        self.redraw_all(cx);
    }

    /// While picking a window: the one under the pointer.
    fn shot_update_window(&mut self, cx: &mut Cx) {
        let cursor = match &self.state_mut().shot.phase {
            Phase::Selecting(s) if s.pick_window => s.cursor,
            _ => return,
        };
        let shown = self.desk(cx).borrow::<WmDesk>().and_then(|d| d.window_shown_at(cursor));
        if let Phase::Selecting(s) = &mut self.state_mut().shot.phase {
            s.window = shown;
        }
    }

    /// The frame clock of the grab, the writer and the thumbnail.
    fn shot_tick(&mut self, cx: &mut Cx, time: f64) {
        // 1. A frame asked for: taken when it arrives.
        let mut grabbed = None;
        let mut failed = false;
        {
            let shot = &mut self.state_mut().shot;
            if let Phase::Grabbing { kind, sink, slot, tries, screen } = &mut shot.phase {
                if let Some(frame) = slot.lock().unwrap().take() {
                    grabbed = Some((*kind, frame, *screen, *sink));
                } else {
                    *tries += 1;
                    failed = *tries > GRAB_TRIES;
                }
            }
        }
        if let Some((kind, frame, screen, sink)) = grabbed {
            remove_screen_capture(sink);
            self.shot_got_frame(cx, kind, frame, screen);
        } else if failed {
            self.shot_cancel(cx);
            self.show_osd(cx, shell::osd::OsdShow::message("Screenshot failed: no frame"));
        } else if matches!(self.state_mut().shot.phase, Phase::Grabbing { tries, .. } if tries % 6 == 5) {
            // The readback rides on a presented frame: ask for one.
            self.redraw_all(cx);
        }
        // 1b. A picked window's own texture, read back once a frame has gone by.
        let mut captured = None;
        let mut capture_failed = false;
        if let Phase::Capturing { texture, px, tries } = &mut self.state_mut().shot.phase {
            *tries += 1;
            captured = Some((texture.texture_id(), *px));
            capture_failed = *tries > GRAB_TRIES;
        }
        if let Some((id, px)) = captured {
            let got = cx.take_render_texture_captures().into_iter().find(|(t, ..)| *t == id);
            if let Some((_, w, h, bytes)) = got {
                self.state_mut().shot.phase = Phase::Idle;
                match bgra_to_frame(&bytes, w, h, px) {
                    Some(frame) => self.shot_save(frame),
                    None => self.show_osd(cx, shell::osd::OsdShow::message("Screenshot failed: odd window size")),
                }
            } else if capture_failed {
                self.state_mut().shot.phase = Phase::Idle;
                self.show_osd(cx, shell::osd::OsdShow::message("Screenshot failed: no window image"));
            }
        }
        // 2. The writer's answer.
        let saved = self.state_mut().shot.saving.as_ref().and_then(|slot| slot.lock().unwrap().take());
        if let Some(result) = saved {
            self.state_mut().shot.saving = None;
            match result {
                Ok(saved) => self.shot_show_thumb(cx, saved),
                Err(why) => {
                    log!("wm: screenshot: {why}");
                    self.show_osd(cx, shell::osd::OsdShow::message("Screenshot not saved"));
                }
            }
        }
        // 3. The thumbnail's clock.
        let dt = if self.shot_last > 0.0 { (time - self.shot_last).clamp(0.0, 0.05) } else { 0.0 };
        self.shot_last = time;
        let mut gone = false;
        if let Some(thumb) = &mut self.state_mut().shot.thumb {
            let step = dt / THUMB_EASE;
            let dragging = thumb.drag.as_ref().is_some_and(|d| d.moved);
            let held = thumb.hovered || dragging;
            if !thumb.leaving && !held {
                thumb.left -= dt;
                if thumb.left <= 0.0 {
                    thumb.leaving = true;
                }
            }
            if thumb.leaving {
                thumb.shown -= step;
                gone = thumb.shown <= 0.0;
            } else {
                thumb.shown = (thumb.shown + step).min(1.0);
            }
        }
        if gone {
            self.state_mut().shot.thumb = None;
        }
        let shot = &self.state_mut().shot;
        if shot.animating() {
            self.shot_next = cx.new_next_frame();
            self.redraw_all(cx);
        } else {
            self.shot_last = 0.0;
        }
    }

    fn shot_got_frame(&mut self, cx: &mut Cx, kind: Kind, frame: Frame, screen: Rect) {
        // Device pixels per point, from the frame itself.
        let width = self.ui.window(cx, ids!(main_window)).get_inner_size(cx).x;
        let scale = if width > 0.0 { frame.width as f64 / width } else { 1.0 };
        match kind {
            Kind::Screen => match pixel_rect(screen, scale, frame.width, frame.height) {
                Some(px) => {
                    self.shot_save(crop(&frame, px));
                    self.state_mut().shot.phase = Phase::Idle;
                    self.shot_thumb_screen = screen;
                }
                None => self.shot_cancel(cx),
            },
            Kind::Region => {
                self.shot_thumb_screen = screen;
                let cursor = self.shot_pointer;
                self.state_mut().shot.phase = Phase::Selecting(Select {
                    frame: Arc::new(frame),
                    scale,
                    screen,
                    pick_window: false,
                    anchor: None,
                    cursor,
                    window: None,
                });
                cx.set_cursor(MouseCursor::Crosshair);
            }
        }
        self.redraw_all(cx);
    }

    fn shot_save(&mut self, frame: Frame) {
        let slot = Arc::new(Mutex::new(None));
        save_async(frame, slot.clone());
        self.state_mut().shot.saving = Some(slot);
    }

    fn shot_show_thumb(&mut self, cx: &mut Cx, saved: Saved) {
        let Saved { path, thumb, thumb_size, .. } = saved;
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 { width: thumb_size.0, height: thumb_size.1, data: Some(thumb), updated: TextureUpdated::Full },
        );
        log!("wm: screenshot saved to {}", path.display());
        let screen = self.shot_thumb_screen;
        self.state_mut().shot.thumb = Some(Thumb {
            path,
            texture,
            thumb_size,
            left: THUMB_LIFE,
            hovered: false,
            shown: 0.0,
            leaving: false,
            drag: None,
            screen,
        });
    }

    /// The pointer, while a selection or a thumbnail wants it. True: taken.
    pub(super) fn shot_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if let Some(ne) = self.shot_next.is_event(event) {
            self.shot_tick(cx, ne.time);
        }
        let p = match event {
            Event::MouseDown(e) => e.abs,
            Event::MouseMove(e) => e.abs,
            Event::MouseUp(e) => e.abs,
            Event::Scroll(e) => e.abs,
            _ => return false,
        };
        self.shot_pointer = p;
        if self.state_mut().shot.selecting() {
            return self.shot_select_pointer(cx, event, p);
        }
        self.shot_thumb_pointer(cx, event, p)
    }

    fn shot_select_pointer(&mut self, cx: &mut Cx, event: &Event, p: Vec2d) -> bool {
        match event {
            Event::MouseMove(_) => {
                if let Phase::Selecting(s) = &mut self.state_mut().shot.phase {
                    s.cursor = p;
                }
                self.shot_update_window(cx);
                cx.set_cursor(MouseCursor::Crosshair);
                self.redraw_all(cx);
            }
            Event::MouseDown(e) if e.button.contains(MouseButton::PRIMARY) => {
                if let Phase::Selecting(s) = &mut self.state_mut().shot.phase {
                    s.cursor = p;
                    if !s.pick_window {
                        s.anchor = Some(p);
                    }
                }
                self.shot_update_window(cx);
                self.redraw_all(cx);
            }
            Event::MouseUp(e) if e.button.contains(MouseButton::PRIMARY) => {
                self.shot_finish_selection(cx, p);
            }
            Event::MouseDown(_) => self.shot_cancel(cx),
            _ => {}
        }
        // A selection owns the pointer: nothing under it sees it.
        true
    }

    fn shot_finish_selection(&mut self, cx: &mut Cx, p: Vec2d) {
        let Phase::Selecting(s) = std::mem::replace(&mut self.state_mut().shot.phase, Phase::Idle) else { return };
        if s.pick_window {
            // The window's own image: the texture its app presented.
            let image = self.desk(cx).borrow::<WmDesk>().and_then(|d| d.window_image_at(p));
            match image {
                Some((texture, px)) if cx.request_render_texture_capture(&texture) => {
                    self.state_mut().shot.phase = Phase::Capturing { texture, px: (px[0], px[1]), tries: 0 };
                }
                _ => self.show_osd(cx, shell::osd::OsdShow::message("That window cannot be captured")),
            }
        } else {
            let chosen = s
                .anchor
                .map(|a| selection_rect(a, p, s.screen))
                .and_then(|r| pixel_rect(r, s.scale, s.frame.width, s.frame.height))
                .filter(|(_, _, w, h)| *w >= 4 && *h >= 4);
            // A click without a drag takes nothing.
            if let Some(px) = chosen {
                self.shot_save(crop(&s.frame, px));
            }
        }
        self.shot_next = cx.new_next_frame();
        self.redraw_all(cx);
    }

    fn shot_thumb_pointer(&mut self, cx: &mut Cx, event: &Event, p: Vec2d) -> bool {
        let Some(thumb) = self.state_mut().shot.thumb.as_mut() else { return false };
        if thumb.leaving {
            return false;
        }
        let home = thumb_rect(thumb.screen, thumb.thumb_size, thumb.shown);
        let over = home.contains(p);
        let mut taken = over;
        match event {
            Event::MouseMove(_) => {
                thumb.hovered = over || thumb.drag.is_some();
                if let Some(drag) = &mut thumb.drag {
                    drag.cursor = p;
                    if !drag.moved && ((p.x - drag.from.x).powi(2) + (p.y - drag.from.y).powi(2)).sqrt() >= DRAG_SLOP {
                        drag.moved = true;
                    }
                    taken = true;
                }
                if over || thumb.drag.is_some() {
                    cx.set_cursor(if thumb.drag.as_ref().is_some_and(|d| d.moved) { MouseCursor::Grabbing } else { MouseCursor::Hand });
                }
            }
            Event::MouseDown(e) if over && e.button.contains(MouseButton::PRIMARY) => {
                thumb.drag = Some(ThumbDrag { from: p, cursor: p, moved: false });
                thumb.hovered = true;
            }
            Event::MouseUp(e) if e.button.contains(MouseButton::PRIMARY) && thumb.drag.is_some() => {
                let drag = thumb.drag.take().unwrap();
                taken = true;
                if !drag.moved {
                    // A click opens the picture.
                    let path = thumb.path.clone();
                    thumb.leaving = true;
                    self.shot_open(cx, path);
                } else {
                    let (path, screen) = (thumb.path.clone(), thumb.screen);
                    self.shot_release(cx, path, screen, p);
                }
            }
            _ => {}
        }
        self.shot_next = cx.new_next_frame();
        self.redraw_all(cx);
        taken
    }

    /// The thumbnail was let go at `p`: onto a window it hands over the path,
    /// at the screen's right edge it is sent away, elsewhere it goes home.
    fn shot_release(&mut self, cx: &mut Cx, path: std::path::PathBuf, screen: Rect, p: Vec2d) {
        let target = self.desk(cx).borrow_mut::<WmDesk>().and_then(|mut d| d.drop_files_at(cx, p, vec![path.to_string_lossy().to_string()]));
        let thumb = self.state_mut().shot.thumb.as_mut();
        if let Some(thumb) = thumb {
            if let Some(client) = target {
                log!("wm: screenshot {} dropped on client {client}", path.display());
                thumb.leaving = true;
            } else if p.x >= screen.pos.x + screen.size.x - SWIPE_EDGE {
                thumb.leaving = true;
            } else {
                thumb.hovered = false;
                thumb.left = THUMB_LIFE;
            }
        }
        if let Some(client) = target {
            self.focus_client(cx, client);
        }
    }

    fn shot_open(&mut self, cx: &mut Cx, path: std::path::PathBuf) {
        let req = WmRequest::Open { app: None, path: path.to_string_lossy().to_string() };
        if let Some(open) = preview::OpenRequest::from_request(&req) {
            self.open_request(cx, open);
        }
    }
}
