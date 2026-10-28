use {
    crate::{
        area::Area,
        cx::Cx,
        file_dialogs::VirtualFile,
        event::{
            event::{DragHit, Event},
            finger::{HitOptions, Inset},
            KeyCode, KeyModifiers,
        },
        makepad_live_id::*,
        makepad_math::*,
    },
    std::sync::Arc,
    std::sync::Mutex,
};

#[derive(Clone, Debug)]
pub struct DragEvent {
    pub modifiers: KeyModifiers,
    pub handled: Arc<Mutex<bool>>,
    pub abs: Vec2d,
    pub items: Arc<Vec<DragItem>>,
    pub response: Arc<Mutex<DragResponse>>,
}

#[derive(Clone, Debug)]
pub struct DropEvent {
    pub modifiers: KeyModifiers,
    pub handled: Arc<Mutex<bool>>,
    pub abs: Vec2d,
    pub items: Arc<Vec<DragItem>>,
}

#[derive(Clone, Debug)]
pub struct DragHitEvent {
    pub modifiers: KeyModifiers,
    pub abs: Vec2d,
    pub rect: Rect,
    pub state: DragState,
    pub items: Arc<Vec<DragItem>>,
    pub response: Arc<Mutex<DragResponse>>,
}

#[derive(Clone, Debug)]
pub struct DropHitEvent {
    pub modifiers: KeyModifiers,
    pub abs: Vec2d,
    pub rect: Rect,
    pub items: Arc<Vec<DragItem>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragState {
    In,
    Over,
    Out,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragResponse {
    None,
    Copy,
    Link,
    Move,
}

/// A drag that can leave the app ([`Cx::start_external_dragging`]): files another app takes,
/// under an optional picture.
pub struct DragOut {
    pub items: Vec<DragOutItem>,
    /// Shown under the pointer; `None` shows each file type's icon.
    pub image: Option<DragImage>,
}

pub enum DragOutItem {
    /// An existing file, by absolute path.
    File(String),
    /// A file made for this drag only, by [`FilePromise::produce`].
    Promise(FilePromise),
}

/// A file a drag carries before it exists: transcoded, exported, rendered. `produce` runs on its
/// own thread from the moment the drag starts and writes the file to the path it is given; the
/// receiving app gets the file when it is complete (macOS: a file promise that also answers a
/// plain file URL; elsewhere the file URL the window system hands over, once written). A failed
/// or cancelled drop leaves nothing behind but a temporary file the next drag cleans up.
pub struct FilePromise {
    /// The file's name as the receiver will see it, extension included.
    pub name: String,
    /// Its MIME type (`audio/mpeg`); the receiver's type when the system asks for one.
    pub mime: String,
    pub produce: Box<dyn FnOnce(&std::path::Path) -> Result<(), String> + Send + 'static>,
}

/// The picture of a drag: `width` x `height` RGBA8 rows at the window's pixel scale.
#[derive(Clone, Debug, PartialEq)]
pub struct DragImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A file of a started drag out, as the platform carries it.
#[derive(Clone, Debug, PartialEq)]
pub enum DragOutFile {
    /// An existing file.
    File(String),
    /// A promised file: its final path (being written by its producer) and MIME type.
    Promise { path: String, mime: String },
}

impl FilePromise {
    /// Starts the producer on its own thread and returns where the file will appear. The
    /// handshake with the window system is `makepad_hal::drag_promise`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn start(self) -> DragOutFile {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = makepad_hal::drag_promise::root();
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        if n == 0 {
            sweep_old_drags(&root);
        }
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let dir = root.join(format!("{}-{stamp}-{n}", std::process::id()));
        // a name is one path component, whatever the app passed
        let name: String = self.name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
        let name = if name.trim_matches('.').is_empty() { "file".to_string() } else { name };
        let path = dir.join(&name);
        let promised = DragOutFile::Promise { path: path.to_string_lossy().into_owned(), mime: self.mime };
        let produce = self.produce;
        let spawned = std::thread::Builder::new().name("drag-out".into()).spawn(move || {
            let part = makepad_hal::drag_promise::part_path(&path);
            let result = std::fs::create_dir_all(&dir)
                .map_err(|e| e.to_string())
                .and_then(|()| produce(&part))
                .and_then(|()| complete(&part, &path));
            if let Err(e) = result {
                crate::error!("drag out: making {}: {e}", path.display());
                let _ = std::fs::remove_file(&part);
                let _ = std::fs::write(makepad_hal::drag_promise::failed_path(&path), e);
            }
        });
        if spawned.is_err() {
            crate::error!("drag out: no thread for the producer");
        }
        promised
    }

    /// The web has no files to hand over: the producer never runs and the platform ends the
    /// drag (`Event::DragEnd`).
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn start(self) -> DragOutFile {
        DragOutFile::Promise { path: String::new(), mime: self.mime }
    }
}

/// Puts a finished producer's file in place: its bytes reach the disk first (a receiver that
/// opens the final name right away, from any process, reads the whole file), then the rename
/// makes the complete file appear under its final name in one step. An empty file is a failed
/// producer, not a file to hand over.
#[cfg(not(target_arch = "wasm32"))]
fn complete(part: &std::path::Path, path: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(part).map_err(|e| format!("the producer wrote no file: {e}"))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if len == 0 {
        return Err("the producer wrote an empty file".into());
    }
    file.sync_all().map_err(|e| format!("syncing {}: {e}", part.display()))?;
    drop(file);
    std::fs::rename(part, path).map_err(|e| e.to_string())
}

/// Removes the temporary folders of drags out more than a day old (a receiver may read a
/// dropped file a while after the drop, so recent ones stay).
#[cfg(not(target_arch = "wasm32"))]
fn sweep_old_drags(root: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let day = std::time::Duration::from_secs(24 * 3600);
    for entry in entries.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > day);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragItem {
    FilePath {
        path: String,
        internal_id: Option<LiveId>,
    },
    String {
        value: String,
        internal_id: Option<LiveId>,
    },
    /// A browser-selected file whose bytes live only in the application.
    /// Web hover events use empty values as count/type placeholders; the
    /// values on [`Event::Drop`] always contain the complete file bytes.
    VirtualFile(VirtualFile),
}

/*
pub enum HitTouch {
    Single,
    Multi
}*/

// Status

#[derive(Default)]
pub struct CxDragDrop {
    drag_area: Area,
    next_drag_area: Area,
    internal_drag_items: Option<Arc<Vec<DragItem>>>,
    /// Held while the pointer event that produced a drag event is itself being
    /// dispatched, so that dispatch neither produces the same drag event again
    /// nor trips `start_dragging`'s "start drag twice".
    suspended_drag_items: Option<Arc<Vec<DragItem>>>,
}

pub(crate) enum InternalDragEvent {
    Drag(DragEvent),
    Drop(DropEvent),
    /// The drag was called off (Escape): no drop, only the end.
    End,
}

impl CxDragDrop {
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn cancel_internal_drag(&mut self) -> bool {
        let active = self.internal_drag_items.take().is_some();
        let suspended = self.suspended_drag_items.take().is_some();
        active || suspended
    }

    pub(crate) fn start_internal_drag(&mut self, items: Vec<DragItem>) {
        assert!(self.internal_drag_items.is_none(), "start drag twice");
        self.internal_drag_items = Some(Arc::new(items));
    }

    pub(crate) fn internal_drag_event(&mut self, event: &Event) -> Option<InternalDragEvent> {
        match event {
            Event::MouseMove(event) => {
                let items = self.internal_drag_items.as_ref()?.clone();
                Some(InternalDragEvent::Drag(DragEvent {
                    modifiers: event.modifiers,
                    handled: Arc::new(Mutex::new(false)),
                    abs: event.abs,
                    items,
                    response: Arc::new(Mutex::new(DragResponse::None)),
                }))
            }
            Event::MouseUp(event) if event.button.is_primary() => {
                let items = self.internal_drag_items.take()?;
                Some(InternalDragEvent::Drop(DropEvent {
                    modifiers: event.modifiers,
                    handled: Arc::new(Mutex::new(false)),
                    abs: event.abs,
                    items,
                }))
            }
            // Escape calls the drag off: the items are dropped on the floor
            // and everyone who watched the drag hears that it ended.
            Event::KeyDown(event) if event.key_code == KeyCode::Escape => {
                self.internal_drag_items.take()?;
                Some(InternalDragEvent::End)
            }
            _ => None,
        }
    }

    /// Brackets the dispatch of the pointer event a drag event came from: the
    /// items are out of the way for it, and come back unless it started a new
    /// drag of its own. Losing them to an unwound dispatch ends the drag,
    /// which beats a flag that stays stuck for the life of the process.
    pub(crate) fn suspend_internal_drag(&mut self) {
        self.suspended_drag_items = self.internal_drag_items.take();
    }

    pub(crate) fn resume_internal_drag(&mut self) {
        let suspended = self.suspended_drag_items.take();
        if self.internal_drag_items.is_none() {
            self.internal_drag_items = suspended;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn cycle_drag(&mut self) {
        self.drag_area = self.next_drag_area;
        self.next_drag_area = Area::Empty;
    }

    pub(crate) fn update_area(&mut self, old_area: Area, new_area: Area) {
        if self.drag_area == old_area {
            self.drag_area = new_area;
        }
    }
}

impl Event {
    pub fn drag_hits(&self, cx: &mut Cx, area: Area) -> DragHit {
        self.drag_hits_with_options(cx, area, HitOptions::default())
    }

    pub fn drag_hits_with_options(&self, cx: &mut Cx, area: Area, options: HitOptions) -> DragHit {
        match self {
            Event::Drag(event) => {
                let rect = area.clipped_rect(cx);
                if area == cx.drag_drop.drag_area {
                    if !*event.handled.lock().unwrap_or_else(|e| e.into_inner())
                        && Inset::rect_contains_with_inset(event.abs, &rect, &options.margin)
                    {
                        //log!("drag_hist_with_options: Drag, in drag area, event handled and rect ({:?}) contains ({},{}) with margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                        cx.drag_drop.next_drag_area = area;
                        *event.handled.lock().unwrap_or_else(|e| e.into_inner()) = true;
                        DragHit::Drag(DragHitEvent {
                            rect,
                            modifiers: event.modifiers,
                            abs: event.abs,
                            items: event.items.clone(),
                            state: DragState::Over,
                            response: event.response.clone(),
                        })
                    } else {
                        //log!("drag_hist_with_options: Drag, in drag area, event not handled or rect ({:?}) doesn't contain ({},{}) with margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                        DragHit::Drag(DragHitEvent {
                            rect,
                            modifiers: event.modifiers,
                            state: DragState::Out,
                            items: event.items.clone(),
                            abs: event.abs,
                            response: event.response.clone(),
                        })
                    }
                } else {
                    if !*event.handled.lock().unwrap_or_else(|e| e.into_inner())
                        && Inset::rect_contains_with_inset(event.abs, &rect, &options.margin)
                    {
                        //log!("drag_hits_with_options: Drag, not in drag_area, event not handled and rect ({:?}) contains ({},{}) with margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                        cx.drag_drop.next_drag_area = area;
                        *event.handled.lock().unwrap_or_else(|e| e.into_inner()) = true;
                        DragHit::Drag(DragHitEvent {
                            modifiers: event.modifiers,
                            rect,
                            state: DragState::In,
                            items: event.items.clone(),
                            abs: event.abs,
                            response: event.response.clone(),
                        })
                    } else {
                        //log!("drag_hits_with_options: Drag, not in drag_area, event handled or rect ({:?}) doesn't contain ({},{}) with margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                        DragHit::NoHit
                    }
                }
            }
            Event::Drop(event) => {
                let rect = area.clipped_rect(cx);
                if !*event.handled.lock().unwrap_or_else(|e| e.into_inner())
                    && Inset::rect_contains_with_inset(event.abs, &rect, &options.margin)
                {
                    //log!("drag_hits_with_options: Drop, event not handled and rect {:?} contains ({},{}) in margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                    cx.drag_drop.next_drag_area = Area::default();
                    *event.handled.lock().unwrap_or_else(|e| e.into_inner()) = true;
                    DragHit::Drop(DropHitEvent {
                        modifiers: event.modifiers,
                        rect,
                        abs: event.abs,
                        items: event.items.clone(),
                    })
                } else {
                    //log!("drag_hits_with_options: Drop, event handled or rect {:?} doesn't contain ({},{}) in margin {:?}",rect,event.abs.x,event.abs.y,options.margin);
                    DragHit::NoHit
                }
            }
            _ => DragHit::NoHit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::{MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent},
        window::WindowId,
    };
    use std::{cell::Cell, cell::RefCell, rc::Rc};

    #[test]
    fn internal_drag_appends_drag_drop_end_to_the_pointer_events() {
        let sequence = Rc::new(RefCell::new(Vec::new()));
        let seen = sequence.clone();
        let item = DragItem::String {
            value: "effect".to_string(),
            internal_id: Some(LiveId(7)),
        };
        let dragged_item = item.clone();
        let mut cx = Cx::new(Box::new(move |cx, event| {
            seen.borrow_mut().push(event.name());
            if matches!(event, Event::MouseDown(_)) {
                cx.start_dragging(vec![dragged_item.clone()]);
            }
        }));
        let window_id = WindowId(0, 0);

        cx.call_event_handler(&Event::MouseDown(MouseDownEvent {
            abs: dvec2(10.0, 20.0),
            button: MouseButton::PRIMARY,
            window_id,
            modifiers: Default::default(),
            handled: Cell::new(Area::Empty),
            time: 1.0,
        }));
        for (time, abs) in [(2.0, dvec2(20.0, 30.0)), (3.0, dvec2(30.0, 40.0))] {
            cx.call_event_handler(&Event::MouseMove(MouseMoveEvent {
                abs,
                lock_delta: Default::default(),
                window_id,
                modifiers: Default::default(),
                time,
                handled: Cell::new(Area::Empty),
            }));
        }
        cx.call_event_handler(&Event::MouseUp(MouseUpEvent {
            abs: dvec2(40.0, 50.0),
            button: MouseButton::PRIMARY,
            window_id,
            modifiers: Default::default(),
            time: 4.0,
        }));

        assert_eq!(
            sequence.borrow().as_slice(),
            [
                "MouseDown", "MouseMove", "Drag", "MouseMove", "Drag", "MouseUp", "Drop",
                "DragEnd"
            ]
        );
    }

    #[test]
    fn internal_drag_without_movement_still_ends_the_drag() {
        let sequence = Rc::new(RefCell::new(Vec::new()));
        let seen = sequence.clone();
        let mut cx = Cx::new(Box::new(move |cx, event| {
            seen.borrow_mut().push(event.name());
            if matches!(event, Event::MouseDown(_)) {
                cx.start_dragging(vec![DragItem::String {
                    value: "effect".to_string(),
                    internal_id: Some(LiveId(7)),
                }]);
            }
        }));
        let window_id = WindowId(0, 0);

        cx.call_event_handler(&Event::MouseDown(MouseDownEvent {
            abs: dvec2(10.0, 20.0),
            button: MouseButton::PRIMARY,
            window_id,
            modifiers: Default::default(),
            handled: Cell::new(Area::Empty),
            time: 1.0,
        }));
        cx.call_event_handler(&Event::MouseUp(MouseUpEvent {
            abs: dvec2(10.0, 20.0),
            button: MouseButton::PRIMARY,
            window_id,
            modifiers: Default::default(),
            time: 2.0,
        }));

        // A release that never moved still has to unwind whatever `start_dragging`
        // set up, so a drop target isn't left painting a ghost of the dragged item.
        assert_eq!(
            sequence.borrow().as_slice(),
            ["MouseDown", "MouseUp", "Drop", "DragEnd"]
        );
    }
}

#[cfg(test)]
mod drag_out_tests {
    use super::*;

    #[test]
    fn a_promise_produces_its_file_off_the_calling_thread() {
        let caller = std::thread::current().id();
        let promise = FilePromise {
            name: "Night/Drive.mp3".into(),
            mime: "audio/mpeg".into(),
            produce: Box::new(move |part| {
                assert_ne!(std::thread::current().id(), caller);
                std::fs::write(part, b"mp3 bytes").map_err(|e| e.to_string())
            }),
        };
        let DragOutFile::Promise { path, mime } = promise.start() else { panic!("a promise") };
        assert_eq!(mime, "audio/mpeg");
        assert!(path.ends_with("Night_Drive.mp3"), "{path}");
        makepad_hal::drag_promise::wait(std::path::Path::new(&path), std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"mp3 bytes");
        let failing = FilePromise { name: "x.mp3".into(), mime: String::new(), produce: Box::new(|_| Err("no decoder for Opus".into())) };
        let DragOutFile::Promise { path, .. } = failing.start() else { panic!("a promise") };
        let err = makepad_hal::drag_promise::wait(std::path::Path::new(&path), std::time::Duration::from_secs(10)).unwrap_err();
        assert_eq!(err, "no decoder for Opus");
        // a producer that "succeeds" with an empty file never hands a 0-byte file to a receiver
        let empty = FilePromise { name: "e.mp4".into(), mime: String::new(), produce: Box::new(|part| std::fs::write(part, b"").map_err(|e| e.to_string())) };
        let DragOutFile::Promise { path, .. } = empty.start() else { panic!("a promise") };
        let err = makepad_hal::drag_promise::wait(std::path::Path::new(&path), std::time::Duration::from_secs(10)).unwrap_err();
        assert_eq!(err, "the producer wrote an empty file");
        assert!(!std::path::Path::new(&path).exists());
    }
}
