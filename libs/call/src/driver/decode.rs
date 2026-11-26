//! The decode worker: one decoder per participant, opened on a key frame, and its pictures to the
//! UI thread (which keeps the newest per tile).

use super::codec::{Codecs, Decoder};
use super::{CallEvent, PeerId, Shared, UiMsg};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

struct Stream {
    peer: PeerId,
    decoder: Option<Box<dyn Decoder>>,
    /// Capture times of the pictures pushed and not yet out, by frame number.
    pending: BTreeMap<i64, u64>,
    reported: bool,
}

pub(super) struct Decode {
    codecs: Arc<dyn Codecs>,
    shared: Arc<Shared>,
    frames: Receiver<(PeerId, crate::video::Frame)>,
    ui: Sender<UiMsg>,
    signal: Arc<dyn Fn() + Send + Sync>,
}

impl Decode {
    pub(super) fn new(
        codecs: Arc<dyn Codecs>,
        shared: Arc<Shared>,
        frames: Receiver<(PeerId, crate::video::Frame)>,
        ui: Sender<UiMsg>,
        signal: Arc<dyn Fn() + Send + Sync>,
    ) -> Decode {
        Decode { codecs, shared, frames, ui, signal }
    }

    fn tell(&self, msg: UiMsg) {
        let _ = self.ui.send(msg);
        (self.signal)();
    }

    pub(super) fn run(self) {
        let mut streams: Vec<Stream> = Vec::new();
        while !self.shared.stop.load(Ordering::Relaxed) {
            let Ok((peer, frame)) = self.frames.recv_timeout(Duration::from_millis(50)) else { continue };
            let i = match streams.iter().position(|s| s.peer == peer) {
                Some(i) => i,
                None => {
                    streams.push(Stream { peer, decoder: None, pending: BTreeMap::new(), reported: false });
                    streams.len() - 1
                }
            };
            let s = &mut streams[i];
            if frame.key {
                if s.decoder.as_mut().is_some_and(|d| !d.accepts(&frame.data)) {
                    // A new stream (a new size after a step): a new decoder, and the pictures the
                    // old one held never come.
                    s.decoder = None;
                    s.pending.clear();
                }
                if s.decoder.is_none() {
                    match self.codecs.decoder() {
                        Ok(mut d) => {
                            d.accepts(&frame.data);
                            let _ = self.ui.send(UiMsg::Decoder(d.name()));
                            s.decoder = Some(d);
                        }
                        Err(e) => {
                            if !std::mem::replace(&mut s.reported, true) {
                                let what = format!("this person's video couldn't be decoded: {e}");
                                let _ = self.ui.send(UiMsg::Event(CallEvent::Problem { peer: Some(peer), what }));
                            }
                        }
                    }
                }
            }
            let Some(d) = &mut s.decoder else { continue };
            s.pending.insert(i64::from(frame.frame), frame.capture_us);
            match d.push(&frame.data, i64::from(frame.frame)) {
                Ok(pictures) => {
                    let mut any = false;
                    for p in pictures {
                        while s.pending.first_key_value().is_some_and(|(&k, _)| k < p.pts_100ns) {
                            s.pending.pop_first();
                        }
                        let capture_us = s.pending.remove(&p.pts_100ns).unwrap_or(frame.capture_us);
                        let _ = self.ui.send(UiMsg::Picture { peer: Some(peer), picture: Arc::new(p), capture_us });
                        any = true;
                    }
                    if any {
                        (self.signal)();
                    }
                }
                Err(e) => {
                    if !std::mem::replace(&mut s.reported, true) {
                        self.tell(UiMsg::Event(CallEvent::Problem { peer: Some(peer), what: format!("a picture from this person couldn't be decoded: {e}") }));
                    }
                }
            }
        }
    }
}
