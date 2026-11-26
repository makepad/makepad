//! The network worker: the call's mesh, every participant's link, the voice ring from the input
//! callback, encoded video from the encoder, and whole frames to the decoder.

use super::encode::Encoded;
use super::voice::{MixControl, VoiceDecoder, VoiceFrames, VoiceMarks};
use super::{CallConfig, CallEvent, NetCommand, PeerId, PeerStats, Shared, UiMsg};
use crate::mesh::Mesh;
use crate::session::{sender_tag, Event};
use crate::wire::{stream, voice_flags, Header, Voice};
use makepad_teamtalk::jitter::JitterRing;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

/// The longest the worker sleeps: voice frames and encoded video wait at most this for it.
const MAX_WAIT_US: u64 = 2_000;
const STATS_US: u64 = 250_000;
/// How often the link streams' caps follow the media rate (each walks the last second's sends).
const CAP_US: u64 = 50_000;
/// Media from someone not joined yet is kept this long and opened when they join: their first
/// pictures race the answer that makes us join them (a datagram is quicker than the link's
/// stream). Past it, nothing is lost for good: the receiver asks for a key frame, a round trip
/// late.
const EARLY_US: u64 = 1_000_000;
/// The least the link stream keeps while media flows, so its messages (the call's own signalling
/// among them) never stall.
const STREAM_FLOOR_BPS: u64 = 64_000;

struct NetPeer {
    id: PeerId,
    tag: u16,
    out: SyncSender<Vec<u8>>,
    /// What the link had no room for yet, in order, with the time each was queued. Never dropped.
    /// It can't quietly build latency: its oldest age counts as pacer delay, so the guard skips
    /// pictures while it lasts, and the session stamped each datagram before it, so the wait
    /// shows as path delay and the rate backs off.
    backlog: VecDeque<(u64, Vec<u8>)>,
    full: u64,
    gone: bool,
    cap: Option<Arc<AtomicU64>>,
    ring: Arc<JitterRing>,
    marks: Arc<VoiceMarks>,
    voice: VoiceDecoder,
    heard: bool,
    /// The longest any datagram had waited in the backlog since the last stats.
    backlog_peak_us: u64,
}

pub(super) struct Net {
    shared: Arc<Shared>,
    inlet: Receiver<(SocketAddr, Vec<u8>)>,
    commands: Receiver<NetCommand>,
    video: Receiver<Encoded>,
    frames: Sender<(PeerId, crate::video::Frame)>,
    voice: VoiceFrames,
    mix: MixControl,
    ui: Sender<UiMsg>,
    signal: Arc<dyn Fn() + Send + Sync>,
    call_id: [u8; 16],
    mesh: Mesh<PeerId>,
    peers: Vec<NetPeer>,
    /// Media whose sender tag no participant has yet, in arrival order, with the arrival time.
    early: VecDeque<(u64, u16, Vec<u8>)>,
    /// Early media no join claimed within [`EARLY_US`].
    strays: u64,
}

impl Net {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        config: &CallConfig,
        shared: Arc<Shared>,
        inlet: Receiver<(SocketAddr, Vec<u8>)>,
        commands: Receiver<NetCommand>,
        video: Receiver<Encoded>,
        frames: Sender<(PeerId, crate::video::Frame)>,
        voice: VoiceFrames,
        mix: MixControl,
        ui: Sender<UiMsg>,
        signal: Arc<dyn Fn() + Send + Sync>,
    ) -> Net {
        Net {
            shared,
            inlet,
            commands,
            video,
            frames,
            voice,
            mix,
            ui,
            signal,
            call_id: config.call_id,
            mesh: Mesh::new(&config.call_id, &config.my_pub),
            peers: Vec::new(),
            early: VecDeque::new(),
            strays: 0,
        }
    }

    fn tell(&self, event: CallEvent) {
        let _ = self.ui.send(UiMsg::Event(event));
        (self.signal)();
    }

    fn receive(&mut self, now: u64, datagram: &[u8], events: &mut Vec<(PeerId, Event)>) {
        // An empty datagram is a doorbell: something else is waiting for this worker.
        let Some((header, _)) = Header::parse(datagram) else { return };
        // Two participants may share a tag (a 1 in 65536 chance a pair): each tries, and only the
        // one whose secret opens it gets the datagram.
        let mut known = false;
        for i in 0..self.peers.len() {
            if self.peers[i].tag == header.src {
                known = true;
                let id = self.peers[i].id;
                self.mesh.receive(now, &id, datagram, events);
            }
        }
        // Voice that waited for a join would only play late; pictures wait for their key frame.
        if !known && header.stream != stream::VOICE {
            self.early.push_back((now, header.src, datagram.to_vec()));
        }
    }

    /// Opens the early media of someone who just joined, at the times it arrived, so their first
    /// key frame and the path's delays are as if they had been joined all along.
    fn claim_early(&mut self, peer: PeerId, tag: u16, events: &mut Vec<(PeerId, Event)>) {
        let mut rest = VecDeque::with_capacity(self.early.len());
        for (at, src, d) in std::mem::take(&mut self.early) {
            if src == tag {
                self.mesh.receive(at, &peer, &d, events);
            } else {
                rest.push_back((at, src, d));
            }
        }
        self.early = rest;
    }

    fn expire_early(&mut self, now: u64) {
        while self.early.front().is_some_and(|(at, _, _)| now.saturating_sub(*at) > EARLY_US) {
            self.early.pop_front();
            self.strays += 1;
        }
    }

    fn command(&mut self, command: NetCommand, events: &mut Vec<(PeerId, Event)>) {
        match command {
            NetCommand::Join(join, marks) => {
                self.leave(join.peer);
                let peer = self.mesh.join(join.peer, &join.their_pub);
                let installed = peer.install_send_secret(join.key_id, &join.send_secret).and_then(|()| peer.install_recv_secret(join.key_id, &join.recv_secret));
                if let Err(e) = installed {
                    self.tell(CallEvent::Problem { peer: Some(join.peer), what: format!("the call's keys for this person were refused ({e})") });
                }
                self.peers.push(NetPeer {
                    id: join.peer,
                    tag: sender_tag(&self.call_id, &join.their_pub),
                    out: join.out,
                    backlog: VecDeque::new(),
                    full: 0,
                    gone: false,
                    cap: join.stream_cap,
                    ring: Arc::new(JitterRing::new()),
                    marks,
                    voice: VoiceDecoder::default(),
                    heard: false,
                    backlog_peak_us: 0,
                });
                self.update_mix();
                self.claim_early(join.peer, sender_tag(&self.call_id, &join.their_pub), events);
            }
            NetCommand::Rekey { peer, key_id, send, recv } => {
                if let Some(p) = self.mesh.peer(&peer) {
                    if let Err(e) = p.install_send_secret(key_id, &send).and_then(|()| p.install_recv_secret(key_id, &recv)) {
                        self.tell(CallEvent::Problem { peer: Some(peer), what: format!("the call's new keys for this person were refused ({e})") });
                    }
                }
            }
            NetCommand::Leave(peer) => {
                self.leave(peer);
                self.update_mix();
            }
        }
    }

    fn leave(&mut self, peer: PeerId) {
        self.mesh.leave(&peer);
        if let Some(i) = self.peers.iter().position(|p| p.id == peer) {
            let p = self.peers.remove(i);
            if let Some(cap) = p.cap {
                cap.store(0, Ordering::Relaxed);
            }
        }
    }

    fn update_mix(&mut self) {
        self.mix.set(self.peers.iter().map(|p| (p.id, p.ring.clone(), p.marks.clone())));
    }

    fn send_out(&mut self, now: u64, out: &mut Vec<(PeerId, Vec<u8>)>) {
        for (id, d) in out.drain(..) {
            if let Some(p) = self.peers.iter_mut().find(|p| p.id == id) {
                if !p.gone {
                    p.backlog.push_back((now, d));
                }
            }
        }
        for p in &mut self.peers {
            while let Some((at, d)) = p.backlog.pop_front() {
                match p.out.try_send(d) {
                    Ok(()) => {}
                    Err(TrySendError::Full(d)) => {
                        p.backlog.push_front((at, d));
                        p.full += 1;
                        break;
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        // The link is gone; the controller ends or resumes the call.
                        p.gone = true;
                        p.backlog.clear();
                    }
                }
            }
        }
    }

    pub(super) fn run(mut self) {
        let mut events = Vec::new();
        let mut out = Vec::new();
        let mut payload = Vec::new();
        let mut last_stats = 0;
        let mut last_cap = 0;
        while !self.shared.stop.load(Ordering::Relaxed) {
            let now = self.shared.now_us();
            let next = self.mesh.peers().filter_map(|(_, p)| p.next_send_us()).min();
            let wait = next.map_or(MAX_WAIT_US, |at| at.saturating_sub(now).clamp(100, MAX_WAIT_US));
            let mut next = self.inlet.recv_timeout(Duration::from_micros(wait)).ok();
            // Commands first: a datagram already waiting from someone who just joined (or sealed
            // under a key just installed) then opens instead of being passed over.
            while let Ok(c) = self.commands.try_recv() {
                self.command(c, &mut events);
            }
            while let Some((_, d)) = next {
                self.receive(self.shared.now_us(), &d, &mut events);
                next = self.inlet.try_recv().ok();
            }
            let now = self.shared.now_us();
            self.expire_early(now);
            for (id, e) in events.drain(..) {
                let Some(p) = self.peers.iter_mut().find(|p| p.id == id) else { continue };
                if !std::mem::replace(&mut p.heard, true) {
                    let _ = self.ui.send(UiMsg::Event(CallEvent::Live(id)));
                    (self.signal)();
                }
                match e {
                    Event::Voice { seq, flags, payload, .. } => p.voice.insert(&p.ring, seq, flags, &payload),
                    Event::Video { frame, .. } => {
                        let _ = self.frames.send((id, frame));
                    }
                }
            }
            while let Some(f) = self.voice.pop(&mut payload) {
                let voice = Voice { flags: f.flags, seq: f.seq, capture_us: f.capture_us, payload: &payload };
                self.mesh.send_voice(now, &voice, f.flags & voice_flags::SILENT == 0, &mut out);
            }
            while let Ok(v) = self.video.try_recv() {
                let _ = self.mesh.send_video(now, stream::CAMERA_HIGH, &v.au, v.key, v.capture_us, &mut out);
            }
            if self.mesh.key_frame_due(stream::CAMERA_HIGH, now) {
                self.shared.key_wanted.store(true, Ordering::Relaxed);
            }
            self.mesh.poll(now, &mut out);
            self.send_out(now, &mut out);
            self.mix.tick();
            if !self.peers.is_empty() {
                let estimate = self.mesh.peers().map(|(_, p)| p.estimate_bps()).min().unwrap_or(crate::session::START_BPS);
                let paced = self.mesh.peers().map(|(_, p)| p.pacer_delay_us(now)).max().unwrap_or(0);
                // One encoding goes to every peer: video waits for every link's level, then the lossiest link decides.
                let level = self.mesh.peers().try_fold(0.0, |highest, (_, p)| p.video_loss_level().map(|level| level.max(highest)));
                let mut backlog = 0;
                for p in &mut self.peers {
                    let age = p.backlog.front().map_or(0, |(at, _)| now.saturating_sub(*at));
                    p.backlog_peak_us = p.backlog_peak_us.max(age);
                    backlog = backlog.max(age);
                }
                let delay = paced.max(backlog);
                self.shared.estimate_bps.store(estimate, Ordering::Relaxed);
                self.shared.pacer_delay_us.store(delay, Ordering::Relaxed);
                self.shared.loss_level_bits.store(level.unwrap_or(f64::NAN).to_bits(), Ordering::Relaxed);
            }
            let caps_due = now.saturating_sub(last_cap) >= CAP_US;
            if caps_due {
                last_cap = now;
            }
            for (id, peer) in self.mesh.peers().filter(|_| caps_due) {
                let Some(cap) = self.peers.iter().find(|p| p.id == *id).and_then(|p| p.cap.as_ref()) else { continue };
                let media_bps = peer.sent_since(now, 1_000_000).1 as u64 * 8;
                cap.store(peer.estimate_bps().saturating_sub(media_bps).max(STREAM_FLOOR_BPS), Ordering::Relaxed);
            }
            if now.saturating_sub(last_stats) >= STATS_US {
                last_stats = now;
                let peers = self
                    .mesh
                    .peers()
                    .map(|(id, peer)| {
                        let p = self.peers.iter().find(|p| p.id == *id);
                        PeerStats {
                            peer: *id,
                            rtt_us: peer.rtt_us(),
                            estimate_bps: peer.estimate_bps(),
                            random_loss: peer.random_loss(),
                            session: peer.stats,
                            video: peer.video_stats().next().map(|(_, v)| v),
                            backlog: p.map_or(0, |p| p.backlog.len()),
                            backlog_us: p.map_or(0, |p| p.backlog_peak_us),
                            link_full: p.map_or(0, |p| p.full),
                        }
                    })
                    .collect();
                for p in &mut self.peers {
                    p.backlog_peak_us = 0;
                }
                let _ = self.ui.send(UiMsg::Stats { peers, voice_dropped: self.voice.dropped(), strays: self.strays });
                (self.signal)();
            }
        }
        for p in &self.peers {
            if let Some(cap) = &p.cap {
                cap.store(0, Ordering::Relaxed);
            }
        }
    }
}
