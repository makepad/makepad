//! The screen's pictures: JPEGs from the store decoded on a worker thread at
//! the size they are drawn (a tile's square, or fit to the screen), then
//! turned into textures on the UI thread as they come back.
//!
//! The UI thread only sends (never waits) and drains with `try_recv`. One
//! long-lived decode thread is plenty: a 512 px JPEG decodes in a few ms.
//! While a picture is decoded at a new size, the one it replaces stays to be
//! drawn, so a tile never goes blank when the wall's layout changes.

use makepad_app_party::image;
use makepad_app_party::store::Jpeg;
use makepad_app_party::wall_plan;
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};

/// Which picture: a take's frame (the screen shows only redrawn pictures).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Pic {
    Take(u64, usize),
}

/// The whole picture fitted within a screen's pixels.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Size {
    Fit(u32, u32),
}

/// A picture at the size it is drawn.
pub type Want = (Pic, Size);

struct Decoded {
    want: Want,
    width: usize,
    height: usize,
    bgra: Result<Vec<u32>, String>,
}

pub struct Frame {
    pub texture: Texture,
    pub width: usize,
    pub height: usize,
}

pub struct Frames {
    jobs: Sender<(Want, Jpeg)>,
    done: Receiver<Decoded>,
    asked: HashSet<Want>,
    wanted: HashSet<Want>,
    ready: HashMap<Pic, Vec<(Size, Frame)>>,
}

impl Frames {
    pub fn new() -> Frames {
        let (jobs, job_rx) = channel::<(Want, Jpeg)>();
        let (done_tx, done) = channel();
        // The screen's decoder: lives as long as the app, fed by the channel.
        #[allow(clippy::disallowed_methods)]
        let _ = std::thread::Builder::new().name("party-frames".into()).spawn(move || {
            while let Ok(((pic, size), jpeg)) = job_rx.recv() {
                let decoded = match image::decode_jpeg(&jpeg) {
                    Ok(rgb) => {
                        let rgb = match size {
                            Size::Fit(w, h) => {
                                let (w, h) = wall_plan::fit_within(rgb.width, rgb.height, w as usize, h as usize);
                                if (w, h) != (rgb.width, rgb.height) { image::resize(&rgb, w, h) } else { rgb }
                            }
                        };
                        Decoded { want: (pic, size), width: rgb.width, height: rgb.height, bgra: Ok(image::bgra(&rgb)) }
                    }
                    Err(e) => Decoded { want: (pic, size), width: 0, height: 0, bgra: Err(e) },
                };
                if done_tx.send(decoded).is_err() {
                    return;
                }
            }
        });
        Frames { jobs, done, asked: HashSet::new(), wanted: HashSet::new(), ready: HashMap::new() }
    }

    /// Makes `wanted` what is held: asks for what is missing (it arrives in
    /// a later [`Frames::drain`]) and lets go of the rest, keeping one other
    /// size of a picture until its wanted size is there.
    pub fn sync(&mut self, wanted: &HashMap<Want, Jpeg>) {
        for (want, jpeg) in wanted {
            let ready = self.ready.get(&want.0).is_some_and(|sizes| sizes.iter().any(|(s, _)| *s == want.1));
            if !ready && self.asked.insert(*want) {
                let _ = self.jobs.send((*want, jpeg.clone()));
            }
        }
        self.asked.retain(|want| wanted.contains_key(want));
        self.wanted = wanted.keys().copied().collect();
        self.ready.retain(|pic, sizes| {
            let has_wanted = sizes.iter().any(|(s, _)| wanted.contains_key(&(*pic, *s)));
            if has_wanted {
                sizes.retain(|(s, _)| wanted.contains_key(&(*pic, *s)));
            } else if wanted.keys().any(|(p, _)| p == pic) {
                // Still decoding at its new size: the newest other size stands in.
                sizes.truncate(1);
            } else {
                sizes.clear();
            }
            !sizes.is_empty()
        });
    }

    /// `pic` at `size`, or failing that at whatever size is held.
    pub fn get(&self, pic: Pic, size: Size) -> Option<&Frame> {
        let sizes = self.ready.get(&pic)?;
        sizes.iter().find(|(s, _)| *s == size).or(sizes.first()).map(|(_, f)| f)
    }

    /// Textures for everything decoded since the last call; true when some came.
    pub fn drain(&mut self, cx: &mut Cx) -> bool {
        let mut any = false;
        while let Ok(decoded) = self.done.try_recv() {
            if !self.asked.remove(&decoded.want) {
                // Not wanted any more (the layout moved on, the source went).
                continue;
            }
            match decoded.bgra {
                Ok(data) => {
                    let texture = Texture::new_with_format(
                        cx,
                        TextureFormat::VecBGRAu8_32 { width: decoded.width, height: decoded.height, data: Some(data), updated: TextureUpdated::Full },
                    );
                    let (pic, size) = decoded.want;
                    let wanted = &self.wanted;
                    let sizes = self.ready.entry(pic).or_default();
                    // The stand-in goes now that a wanted size is here.
                    sizes.retain(|(s, _)| *s != size && wanted.contains(&(pic, *s)));
                    sizes.insert(0, (size, Frame { texture, width: decoded.width, height: decoded.height }));
                    any = true;
                }
                Err(e) => log!("party: a picture did not decode: {e}"),
            }
        }
        any
    }

    /// Pictures asked for and not decoded yet.
    pub fn pending(&self) -> usize {
        self.asked.len()
    }

    /// Bytes of pictures held in textures, for the log.
    pub fn bytes(&self) -> usize {
        self.ready.values().flatten().map(|(_, f)| f.width * f.height * 4).sum()
    }
}
