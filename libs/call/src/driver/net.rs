//! The network worker: the call's mesh, every participant's link, the voice ring from the input
//! callback, encoded video from the encoder, and whole frames to the decoder.

use super::decode::Decoders;
use super::encode::Encoded;
use super::voice::{AudioLevels, MixControl, SourceLevel, VoiceDecoder, VoiceFrames, VoiceMarks};
use super::{CallConfig, CallEvent, NetCommand, PeerId, PeerStats, Shared, UiMsg};
use super::{EncodedSource, VideoCaps, View};
use crate::guard::{estimate_for, share_frame_bits, video_ask_for};
use crate::mesh::{MediaState, Mesh};
use crate::session::{sender_tag, Event, ReceiveOutcome};
use crate::wire::{stream, voice_flags, Header, Voice};
use makepad_teamtalk::jitter::JitterRing;
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
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

struct ReceivedAudio {
    ring: Arc<JitterRing>,
    decoder: VoiceDecoder,
    marks: Arc<VoiceMarks>,
    level: Arc<SourceLevel>,
}

struct SendAudio {
    stream: u8,
    frames: VoiceFrames,
    seq_base: u32,
    last_sent: Option<u64>,
}

#[derive(Default)]
struct Subscription {
    panel: View,
    announced: View,
    due: u64,
}

impl Subscription {
    fn set(&mut self, now: u64, view: View) {
        if self.panel == view { return; }
        // Call::watch_share queues openings and closings synchronously on the reliable link.
        if !self.panel.open() || !view.open() { self.announced = view; }
        self.panel = view;
        let grows = u64::from(view.width) * u64::from(view.height) > u64::from(self.announced.width) * u64::from(self.announced.height)
            || view.fps > self.announced.fps;
        self.due = now + if !view.open() { 0 } else if grows { 300_000 } else { 1_000_000 };
    }

    fn changed(&mut self, now: u64) -> Option<View> {
        if self.panel == self.announced || now < self.due { return None; }
        self.announced = self.panel;
        Some(self.panel)
    }
}

// One fps is the floor, even when a single picture takes longer than a second to send.
const SHARE_UP_STEPS: [u32; 5] = [30, 15, 10, 5, 2];
const SHARE_UP_WAIT_US: u64 = 2_000_000;

struct Share {
    source: Box<dyn EncodedSource>,
    caps: VideoCaps,
    want: View,
    // When each higher step first fit both the panel and the encoder budget.
    up_since: [Option<u64>; SHARE_UP_STEPS.len()],
    kbps: u32,
    wanted_bps: u64,
    keys: crate::video::KeyGate,
}

impl Share {
    fn new(caps: VideoCaps, mut source: Box<dyn EncodedSource>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        source.set_wake(wake);
        source.want(0, 0, 0);
        Self { source, caps, want: View::default(), up_since: [None; SHARE_UP_STEPS.len()], kbps: 0, wanted_bps: 0, keys: Default::default() }
    }

    fn frame_rate(&mut self, now: u64, max_fps: u64) -> u32 {
        for (step, since) in SHARE_UP_STEPS.into_iter().zip(&mut self.up_since) {
            if u64::from(step) > max_fps {
                *since = None;
            } else if !self.want.open() || step <= self.want.fps || now.saturating_sub(*since.get_or_insert(now)) >= SHARE_UP_WAIT_US {
                return step;
            }
        }
        1
    }

    fn next(&mut self, now: u64, panel: View, rate: u64, audio: usize, key: bool, ready: bool) -> Option<super::codec::Packet> {
        let mut want = View { width: panel.width.min(self.caps.width), height: panel.height.min(self.caps.height), fps: panel.fps.min(self.caps.fps) };
        if want.open() {
            let ask = video_ask_for(rate, audio);
            let ask_kbps = (ask / 1_000).clamp(1, u64::from(u32::MAX));
            // The encoder takes whole kbit/s. Only count bits available to both it and the link.
            let budget = ask.min(ask_kbps * 1_000);
            let frame_bits = share_frame_bits(want.width, want.height);
            self.wanted_bps = estimate_for(frame_bits.saturating_mul(u64::from(want.fps)), audio);
            want.fps = self.frame_rate(now, (budget / frame_bits).min(u64::from(want.fps)));
            if self.want.open() && want.fps != self.want.fps {
                eprintln!("share fps {} -> {} at {:.1} Mbit/s", self.want.fps, want.fps, rate as f64 / 1_000_000.0);
            }
            // A slow link lowers the frame rate, never the picture: every frame keeps a full frame's
            // bits, the one-fps floor included, and the pacer takes as long as the link needs.
            let kbps = ask_kbps.max((frame_bits * u64::from(want.fps)).div_ceil(1_000)).min(u64::from(u32::MAX)) as u32;
            // Publish the initial bitrate before demand can wake the source's encoder.
            if self.kbps != kbps { self.source.set_kbps(kbps); self.kbps = kbps; }
        }
        if self.want != want {
            self.source.want(want.width, want.height, want.fps);
            self.want = want;
            self.keys.request();
        }
        if !want.open() { self.kbps = 0; self.wanted_bps = 0; return None; }
        if key { self.keys.request(); }
        if self.keys.take_after(now, crate::session::SHARE_RETRY_US) { self.source.key_now(); }
        // While every viewer is backed up, the source can supersede unencoded pictures.
        // One slow link must not stall the other viewers.
        if !ready { return None; }
        let packet = self.source.pop()?;
        if packet.key { self.keys.made_key(now); }
        Some(packet)
    }
}

impl Drop for Share {
    fn drop(&mut self) { self.source.want(0, 0, 0); }
}

struct NetPeer {
    id: PeerId,
    tag: u16,
    voice: bool,
    view: View,
    watch: Subscription,
    out: SyncSender<Vec<u8>>,
    /// What the link had no room for yet, in order, with the time each was queued. Never dropped.
    /// It can't quietly build latency: its oldest age counts as pacer delay, so the guard skips
    /// pictures while it lasts, and the session stamped each datagram before it, so the wait
    /// shows as path delay and the rate backs off.
    backlog: VecDeque<(u64, Vec<u8>)>,
    full: u64,
    gone: bool,
    cap: Option<Arc<AtomicU64>>,
    audio: BTreeMap<u8, ReceivedAudio>,
    marks: Arc<VoiceMarks>,
    levels: Arc<AudioLevels>,
    heard: bool,
    /// The longest any datagram had waited in the backlog since the last stats.
    backlog_peak_us: u64,
}

#[derive(Default)]
struct NetState {
    events: Vec<(PeerId, Event)>,
    out: Vec<(PeerId, Vec<u8>)>,
    payload: Vec<u8>,
    last_stats: u64,
    last_cap: u64,
}

pub(super) struct Net {
    shared: Arc<Shared>,
    inlet: Arc<Mutex<Receiver<(SocketAddr, Vec<u8>)>>>,
    commands: Receiver<NetCommand>,
    video: Receiver<Encoded>,
    decoders: Decoders,
    audio: Vec<SendAudio>,
    next_audio_seq: BTreeMap<u8, u32>,
    mix: Option<MixControl>,
    share: Option<Share>,
    wake: Arc<dyn Fn() + Send + Sync>,
    ui: Sender<UiMsg>,
    signal: Arc<dyn Fn() + Send + Sync>,
    call_id: [u8; 16],
    mesh: Mesh<PeerId>,
    peers: Vec<NetPeer>,
    scratch_events: Vec<Event>,
    /// Non-audio media no joined peer could open, in arrival order, with the arrival time.
    early: VecDeque<(u64, u16, Vec<u8>)>,
    /// Early media no join claimed within [`EARLY_US`].
    strays: u64,
}

impl Net {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        config: &CallConfig,
        shared: Arc<Shared>,
        inlet: Arc<Mutex<Receiver<(SocketAddr, Vec<u8>)>>>,
        commands: Receiver<NetCommand>,
        video: Receiver<Encoded>,
        decoders: Decoders,
        doorbell: SyncSender<(SocketAddr, Vec<u8>)>,
        ui: Sender<UiMsg>,
        signal: Arc<dyn Fn() + Send + Sync>,
    ) -> Net {
        Net {
            shared,
            inlet,
            commands,
            video,
            decoders,
            audio: Vec::new(),
            next_audio_seq: BTreeMap::new(),
            mix: None,
            share: None,
            wake: Arc::new(move || super::ring(&doorbell)),
            ui,
            signal,
            call_id: config.call_id,
            mesh: Mesh::new(&config.call_id, &config.my_pub),
            peers: Vec::new(),
            scratch_events: Vec::with_capacity(1),
            early: VecDeque::new(),
            strays: 0,
        }
    }

    fn tell(&self, event: CallEvent) {
        let _ = self.ui.send(UiMsg::Event(event));
        (self.signal)();
    }

    fn try_peer(&mut self, now: u64, id: PeerId, datagram: &[u8], events: &mut Vec<(PeerId, Event)>) -> ReceiveOutcome {
        self.scratch_events.clear();
        let outcome = self.mesh.try_receive(now, &id, datagram, &mut self.scratch_events);
        if outcome == ReceiveOutcome::Opened {
            events.extend(self.scratch_events.drain(..).map(|event| (id, event)));
        }
        outcome
    }

    fn receive(&mut self, now: u64, datagram: &[u8], events: &mut Vec<(PeerId, Event)>) {
        // An empty datagram is a doorbell: something else is waiting for this worker.
        let Some((header, _)) = Header::parse(datagram) else { return };
        // Only authentication (including an authenticated replay) identifies the sender.
        let mut first_bad_tag = None;
        let mut first_refused = None;
        for i in 0..self.peers.len() {
            if self.peers[i].tag != header.src { continue; }
            let id = self.peers[i].id;
            match self.try_peer(now, id, datagram, events) {
                ReceiveOutcome::Opened | ReceiveOutcome::Replayed => return,
                ReceiveOutcome::BadTag => { first_bad_tag.get_or_insert(id); }
                ReceiveOutcome::Refused => { first_refused.get_or_insert(id); }
            }
        }
        // Voice that waited for a join would only play late; pictures wait for their key frame.
        if !stream::is_audio(header.stream) {
            self.early.push_back((now, header.src, datagram.to_vec()));
        } else if let Some(id) = first_bad_tag {
            self.mesh.note_bad_tag(&id);
        } else if let Some(id) = first_refused {
            self.mesh.note_refused(&id);
        }
    }

    /// Opens the early media of someone who just joined, at the times it arrived, so their first
    /// key frame and the path's delays are as if they had been joined all along.
    fn claim_early(&mut self, peer: PeerId, tag: u16, events: &mut Vec<(PeerId, Event)>) {
        if self.early.is_empty() { return; }
        let mut rest = std::mem::take(&mut self.early);
        rest.retain(|(at, src, d)| *src != tag || matches!(self.try_peer(*at, peer, d, events),
            ReceiveOutcome::BadTag | ReceiveOutcome::Refused));
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
            NetCommand::Join { join, call_id, voice, watching, marks, levels } => {
                self.leave(join.peer);
                let cid = call_id.unwrap_or(self.call_id);
                let tag = sender_tag(&cid, &join.their_pub);
                let peer = self.mesh.join_with_call_id(join.peer, &cid, &join.their_pub, voice);
                peer.receive_share(watching);
                let installed = peer.install_send_secret(join.key_id, &join.send_secret).and_then(|()| peer.install_recv_secret(join.key_id, &join.recv_secret));
                if let Err(e) = installed {
                    self.tell(CallEvent::Problem { peer: Some(join.peer), what: format!("the call's keys for this person were refused ({e})") });
                }
                self.peers.push(NetPeer {
                    id: join.peer,
                    view: View::default(),
                    watch: Subscription::default(),
                    tag,
                    voice,
                    out: join.out,
                    backlog: VecDeque::new(),
                    full: 0,
                    gone: false,
                    cap: join.stream_cap,
                    audio: BTreeMap::new(),
                    marks,
                    levels,
                    heard: false,
                    backlog_peak_us: 0,
                });
                self.update_mix();
                self.claim_early(join.peer, tag, events);
            }
            NetCommand::PeerVoice { peer, on } => {
                if let Some(p) = self.peers.iter_mut().find(|p| p.id == peer && p.voice != on) {
                    p.voice = on;
                    self.mesh.set_voice(&peer, on);
                    if !on {
                        p.audio.remove(&stream::VOICE);
                        p.backlog.retain(|(_, d)| Header::parse(d).is_none_or(|(h, _)| h.stream != stream::VOICE));
                        self.update_mix();
                    }
                }
            }
            NetCommand::Rekey { peer, key_id, send, recv } => {
                if let Some(p) = self.mesh.peer(&peer) {
                    if let Err(e) = p.install_send_secret(key_id, &send).and_then(|()| p.install_recv_secret(key_id, &recv)) {
                        self.tell(CallEvent::Problem { peer: Some(peer), what: format!("the call's new keys for this person were refused ({e})") });
                    }
                }
            }
            NetCommand::Voice(mix) => {
                if let Some(old) = &mut self.mix { old.set(std::iter::empty()); }
                self.mix = mix;
                self.audio.retain(|s| s.stream != stream::VOICE);
                for p in &mut self.peers { p.audio.remove(&stream::VOICE); }
                self.update_mix();
            }
            NetCommand::Share(source) => {
                drop(self.share.take());
                for p in &mut self.peers {
                    p.view = View::default();
                    self.mesh.peer(&p.id).unwrap().stop_share();
                    p.backlog.retain(|(_, d)| Header::parse(d).is_none_or(|(h, _)| h.stream != stream::SHARE));
                }
                self.share = source.map(|(caps, source)| Share::new(caps, source, self.wake.clone()));
            }
            NetCommand::BeginWatch(id) => {
                if let Some(peer) = self.mesh.peer(&id) { peer.receive_share(true); }
            }
            NetCommand::Watch(id, view) => {
                if let Some(p) = self.peers.iter_mut().find(|p| p.id == id) {
                    p.watch.set(self.shared.now_us(), view);
                    self.mesh.peer(&id).unwrap().receive_share(view.open());
                    if !view.open() { self.decoders.close(id, stream::SHARE); }
                }
            }
            NetCommand::PeerView(id, view) => {
                if let Some(p) = self.peers.iter_mut().find(|p| p.id == id) {
                    if !p.view.open() && view.open() {
                        if let Some(share) = &mut self.share {
                            // A new viewer has no reference picture, even if another just got a key.
                            share.keys = Default::default();
                            share.keys.request();
                        }
                    }
                    if !view.open() {
                        self.mesh.peer(&id).unwrap().stop_share();
                        p.backlog.retain(|(_, d)| Header::parse(d).is_none_or(|(h, _)| h.stream != stream::SHARE));
                    }
                    p.view = view;
                }
            }
            NetCommand::AudioSource(stream, frames) => {
                let seq_base = *self.next_audio_seq.entry(stream).or_default();
                self.audio.push(SendAudio { stream, frames, seq_base, last_sent: None });
            }
            NetCommand::Leave(peer) => {
                self.leave(peer);
                self.update_mix();
            }
        }
    }

    fn leave(&mut self, peer: PeerId) {
        self.mesh.leave(&peer);
        self.decoders.leave(peer);
        if let Some(i) = self.peers.iter().position(|p| p.id == peer) {
            let p = self.peers.remove(i);
            if let Some(cap) = p.cap {
                cap.store(0, Ordering::Relaxed);
            }
        }
    }

    fn update_mix(&mut self) {
        let Some(mix) = &mut self.mix else { return };
        mix.set(self.peers.iter().flat_map(|p| p.audio.iter().map(move |(&stream, a)| {
            (p.id, stream, a.ring.clone(), a.marks.clone(), a.level.clone())
        })));
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
            let tried_media = !p.backlog.is_empty();
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
            if self.share.is_none() || !p.view.open() { continue; }
            let peer = self.mesh.peer(&p.id).unwrap();
            if tried_media { peer.set_padding_blocked(!p.backlog.is_empty()); }
            if p.backlog.is_empty() && !p.gone {
                peer.send_padding(now, |d| match p.out.try_send(d) {
                    Ok(()) => true,
                    Err(TrySendError::Full(_)) => false,
                    Err(TrySendError::Disconnected(_)) => { p.gone = true; false }
                });
            }
            if p.gone { peer.set_wanted(0); }
        }
    }

    fn next_wake_us(&self) -> Option<u64> {
        self.mesh.peers().filter_map(|(_, p)| p.next_send_us()).min()
    }

    fn step(
        &mut self,
        inlet: &Receiver<(SocketAddr, Vec<u8>)>,
        mut next: Option<(SocketAddr, Vec<u8>)>,
        state: &mut NetState,
    ) {
        // Commands first: a datagram already waiting from someone who just joined (or sealed
        // under a key just installed) then opens instead of being passed over.
        while let Ok(c) = self.commands.try_recv() {
            self.command(c, &mut state.events);
        }
        while let Ok((id, stream, data)) = self.decoders.returned_frames.try_recv() {
            if let Some(peer) = self.mesh.peer(&id) {
                peer.recycle_video(stream, data);
            }
        }
        while let Some((_, d)) = next {
            self.receive(self.shared.now_us(), &d, &mut state.events);
            next = inlet.try_recv().ok();
        }
        let now = self.shared.now_us();
        self.expire_early(now);
        let mut mix_changed = false;
        for (id, e) in state.events.drain(..) {
            let Some(p) = self.peers.iter_mut().find(|p| p.id == id) else { continue };
            if !std::mem::replace(&mut p.heard, true) {
                let _ = self.ui.send(UiMsg::Event(CallEvent::Live(id)));
                (self.signal)();
            }
            match e {
                Event::Voice { stream, seq, flags, payload, .. } => {
                    if self.mix.is_none() || (stream == stream::VOICE && !p.voice) { continue; }
                    let audio = p.audio.entry(stream).or_insert_with(|| {
                        mix_changed = true;
                        ReceivedAudio {
                            ring: Arc::new(JitterRing::new()),
                            decoder: VoiceDecoder::default(),
                            marks: if stream == stream::VOICE { p.marks.clone() } else { Arc::default() },
                            level: p.levels.slot(stream).unwrap().get_or_init(Arc::default).clone(),
                        }
                    });
                    audio.decoder.insert(&audio.ring, seq, flags, &payload);
                }
                Event::Video { stream, frame } => self.decoders.frame(id, stream, frame),
            }
        }
        if mix_changed {
            self.update_mix();
        }
        let mut active = 0;
        for source in &mut self.audio {
            while let Some(f) = source.frames.pop(&mut state.payload) {
                // A new microphone starts its framer at zero. The media session's stream
                // continues across attachments, including any holes the callback reported.
                let seq = source.seq_base.wrapping_add(f.seq);
                *self.next_audio_seq.get_mut(&source.stream).unwrap() = seq.wrapping_add(1);
                let voice = Voice { flags: f.flags, seq, capture_us: f.capture_us, payload: &state.payload };
                let before = state.out.len();
                let _ = self.mesh.send_voice(now, source.stream, &voice, f.flags & voice_flags::SILENT == 0, &mut state.out);
                if state.out.len() > before {
                    source.last_sent = Some(now);
                }
            }
            active += usize::from(source.last_sent.is_some_and(|at| now - at < 1_000_000));
        }
        self.shared.active_audio.store(active, Ordering::Relaxed);
        for p in &mut self.peers {
            if let Some(view) = p.watch.changed(now) {
                let _ = self.ui.send(UiMsg::View(p.id, view));
                (self.signal)();
            }
        }
        if let Some(share) = &mut self.share {
            let mut panel = View::default();
            let mut rate = u64::MAX;
            let mut key = false;
            let mut ready = false;
            for p in self.peers.iter().filter(|p| p.view.open() && !p.gone) {
                let peer = self.mesh.peer(&p.id).unwrap();
                panel.width = panel.width.max(p.view.width);
                panel.height = panel.height.max(p.view.height);
                panel.fps = panel.fps.max(p.view.fps);
                rate = rate.min(peer.estimate_bps());
                ready |= p.backlog.is_empty() && !peer.share_queued();
                key |= peer.key_frame_due(stream::SHARE, now);
            }
            let packet = share.next(now, panel, rate, active, key, ready);
            let wanted = share.wanted_bps;
            let mut packets = Vec::new();
            for p in self.peers.iter().filter(|p| p.view.open() && !p.gone) {
                let peer = self.mesh.peer(&p.id).unwrap();
                peer.set_wanted(wanted);
                if let Some(packet) = &packet {
                    if let Err(e) = peer.send_video(now, stream::SHARE, &packet.data, packet.key,
                        packet.pts_100ns.max(0) as u64 / 10, &mut packets) {
                        self.tell(CallEvent::Problem { peer: Some(p.id), what: format!("sending shared picture: {e}") });
                    }
                    state.out.extend(packets.drain(..).map(|d| (p.id, d)));
                }
            }
        }
        while let Ok(v) = self.video.try_recv() {
            let _ = self.mesh.send_video(now, stream::CAMERA_HIGH, &v.au, v.key, v.capture_us, &mut state.out);
        }
        if self.mesh.key_frame_due(stream::CAMERA_HIGH, now) {
            self.shared.key_wanted.store(true, Ordering::Relaxed);
        }
        for p in self.peers.iter().filter(|p| p.watch.panel.open()) {
            let (decoded, frame, errors) = self.decoders.share_progress(p.id);
            self.mesh.peer(&p.id).unwrap().decoded_share(now, decoded, frame, errors);
        }
        self.mesh.poll(now, &mut state.out);
        self.send_out(now, &mut state.out);
        if let Some(mix) = &mut self.mix { mix.tick(); }
        if !self.peers.is_empty() {
            let (estimate, paced, level) = self.mesh.media(now);
            let mut backlog = 0;
            for p in &mut self.peers {
                let Some((at, _)) = p.backlog.front() else { continue };
                let age = now.saturating_sub(*at);
                p.backlog_peak_us = p.backlog_peak_us.max(age);
                if self.mesh.media_state(&p.id) == Some(MediaState::Taking) {
                    backlog = backlog.max(age);
                }
            }
            let delay = paced.max(backlog);
            self.shared.estimate_bps.store(estimate, Ordering::Relaxed);
            self.shared.pacer_delay_us.store(delay, Ordering::Relaxed);
            self.shared.loss_level_bits.store(level.unwrap_or(f64::NAN).to_bits(), Ordering::Relaxed);
        }
        let caps_due = now.saturating_sub(state.last_cap) >= CAP_US;
        if caps_due {
            state.last_cap = now;
        }
        for (id, peer) in self.mesh.peers().filter(|_| caps_due) {
            let Some(cap) = self.peers.iter().find(|p| p.id == *id).and_then(|p| p.cap.as_ref()) else { continue };
            let media_bps = peer.sent_since(now, 1_000_000).1 as u64 * 8;
            cap.store(peer.estimate_bps().saturating_sub(media_bps).max(STREAM_FLOOR_BPS), Ordering::Relaxed);
        }
        if now.saturating_sub(state.last_stats) >= STATS_US {
            state.last_stats = now;
            let peers = self
                .mesh
                .peers()
                .map(|(id, peer)| {
                    let p = self.peers.iter().find(|p| p.id == *id);
                    let (share_decoded, _, share_decode_errors) = self.decoders.share_progress(*id);
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
                        share_received: peer.video_stats().find(|(s, _)| *s == stream::SHARE).map(|(_, v)| v).unwrap_or_default(),
                        share_decoded, share_decode_errors,
                        paced_bytes: peer.paced_bytes(),
                        share_source_counts: self.share.as_ref().and_then(|s| s.source.frame_counts()),
                    }
                })
                .collect();
            for p in &mut self.peers {
                p.backlog_peak_us = 0;
            }
            let _ = self.ui.send(UiMsg::Stats { peers, voice_dropped: self.audio.iter().map(|s| s.frames.dropped()).sum(), strays: self.strays, share_kbps: self.share.as_ref().map(|s| s.kbps) });
            (self.signal)();
        }
    }

    pub(super) fn run(mut self) {
        // Only the network worker waits for the previous call to release this link.
        let inlet = self.inlet.clone();
        let inlet = inlet.lock().unwrap_or_else(|err| err.into_inner());
        let mut state = NetState::default();
        while !self.shared.stop.load(Ordering::Relaxed) {
            let wait_for_inlet = || {
                let now = self.shared.now_us();
                let next = self.next_wake_us();
                let wait = next.map_or(MAX_WAIT_US, |at| at.saturating_sub(now).clamp(100, MAX_WAIT_US));
                inlet.recv_timeout(Duration::from_micros(wait)).ok()
            };
            #[cfg(test)]
            let next = match super::test_clock::tick() {
                Some(true) => inlet.try_recv().ok(),
                Some(false) => break,
                None => wait_for_inlet(),
            };
            #[cfg(not(test))]
            let next = wait_for_inlet();
            self.step(&inlet, next, &mut state);
        }
        for p in &self.peers {
            if let Some(cap) = &p.cap {
                cap.store(0, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwatching_peer_clears_wanted_and_stops_padding() {
        use crate::driver::{codec::{Packet, TestCodecs}, test_clock, Inlet, Join, VoiceSource};
        use crate::session::Peer;
        use crate::sim::{Path, PathConfig};
        use std::sync::mpsc::{channel, sync_channel};

        struct StillSource(Option<Packet>);
        impl EncodedSource for StillSource {
            fn pop(&mut self) -> Option<Packet> { self.0.take() }
            fn want(&mut self, _: u32, _: u32, _: u32) {}
            fn set_kbps(&mut self, _: u32) {}
            fn key_now(&mut self) {}
            fn set_wake(&mut self, _: Arc<dyn Fn() + Send + Sync>) {}
        }

        let cfg = CallConfig {
            inlet: None, call_id: [7; 16], my_pub: vec![1; 65], video: None,
            voice: VoiceSource::Off, echo_cancel: Default::default(), codecs: Arc::new(TestCodecs),
            spawn: Arc::new(|name, _| panic!("unexpected worker {name}")),
            signal: Arc::new(|| {}), clock: std::time::Instant::now(),
        };
        let Inlet { tx, rx } = Inlet::new();
        let (commands, commands_rx) = channel();
        let (ui, _messages) = channel();
        let shared = Arc::new(Shared::new(cfg.clock, false));
        let decoders = Decoders::new(cfg.spawn.clone(), cfg.codecs.clone(), shared.clone(), ui.clone(), cfg.signal.clone());
        let mut net = Net::new(&cfg, shared, rx, commands_rx, channel().1, decoders, tx.clone(), ui, cfg.signal.clone());
        let inlet = net.inlet.clone();
        let inlet = inlet.lock().unwrap();
        let mut state = NetState::default();
        let (out, out_rx) = sync_channel(64);
        let join = Join { peer: 2, their_pub: vec![2; 65], key_id: 0, send_secret: [1; 32], recv_secret: [2; 32], out, stream_cap: None };
        let mut remote = Peer::new(&cfg.call_id, &join.their_pub, &cfg.my_pub);
        remote.install_send_secret(join.key_id, &join.recv_secret).unwrap();
        remote.install_recv_secret(join.key_id, &join.send_secret).unwrap();
        commands.send(NetCommand::Join { join, call_id: None, voice: true, watching: false,
            marks: Arc::default(), levels: Arc::default() }).unwrap();
        let caps = VideoCaps { width: 1280, height: 720, fps: 30, content: crate::guard::Content::Share };
        let source = StillSource(Some(Packet { data: vec![1], key: true, pts_100ns: 0 }));
        commands.send(NetCommand::Share(Some((caps, Box::new(source))))).unwrap();
        commands.send(NetCommand::PeerView(2, View { width: caps.width, height: caps.height, fps: caps.fps })).unwrap();

        let path = PathConfig { latency_us: 3_000, ..Default::default() };
        let (mut ab, mut ba) = (Path::new(path, 11), Path::new(path, 23));
        let (mut packets, mut events) = (Vec::new(), Vec::new());
        let address = SocketAddr::from(([192, 0, 2, 1], 47100));
        let mut step = |net: &mut Net, now| {
            ab.poll(now, &mut packets);
            for d in packets.drain(..) { remote.receive(now, &d, &mut events); }
            events.clear();
            remote.poll(now, &mut packets);
            for d in packets.drain(..) { ba.send(now, d); }
            ba.poll(now, &mut packets);
            for d in packets.drain(..) { tx.try_send((address, d)).unwrap(); }
            test_clock::with_now(now, || net.step(&inlet, inlet.try_recv().ok(), &mut state));
            let mut pads = 0;
            for d in out_rx.try_iter() {
                if d.len() == crate::wire::MAX_DATAGRAM && Header::parse(&d).is_some_and(|(h, _)| h.stream == stream::FEEDBACK) {
                    pads += 1;
                }
                ab.send(now, d);
            }
            pads
        };

        let mut now = 0;
        loop {
            let pads = step(&mut net, now);
            let peer = net.mesh.peer(&2).unwrap();
            if pads > 0 && peer.wanted_bps() > 0 && peer.stats.padding_sent > 0 && net.next_wake_us().is_some() { break; }
            assert!(now < 2_000_000, "watching must arm padding and its wake within two seconds");
            now += 1_000;
        }
        commands.send(NetCommand::PeerView(2, View::default())).unwrap();
        now += 1_000;
        assert_eq!(step(&mut net, now), 0, "sent padding while closing the view");
        let before = net.mesh.peer(&2).unwrap().stats;
        for tick in 0..=2_000 {
            if tick > 0 {
                now += 1_000;
                assert_eq!(step(&mut net, now), 0, "padding on the wire at {now} µs");
            }
            let peer = net.mesh.peer(&2).unwrap();
            assert_eq!(peer.wanted_bps(), 0, "unwatched peer still wanted padding");
            assert_eq!(peer.stats.padding_sent, before.padding_sent, "padding count rose after unwatch");
            assert_eq!(peer.next_send_us(), None, "peer has a send deadline at {now} µs");
            assert_eq!(net.next_wake_us(), None, "net has a padding wake at {now} µs");
        }
        assert!(net.mesh.peer(&2).unwrap().stats.received > before.received, "feedback must continue after unwatch");
    }

    #[test]
    fn share_keeps_panel_resolution_and_waits_only_for_fps_increases() {
        #[derive(Default)]
        struct Requests {
            wants: Vec<View>,
            kbps: Vec<u32>,
        }
        struct Source(Arc<Mutex<Requests>>);
        impl EncodedSource for Source {
            fn want(&mut self, width: u32, height: u32, fps: u32) {
                self.0.lock().unwrap().wants.push(View { width, height, fps });
            }
            fn set_kbps(&mut self, kbps: u32) { self.0.lock().unwrap().kbps.push(kbps); }
            fn set_wake(&mut self, _: Arc<dyn Fn() + Send + Sync>) {}
            fn key_now(&mut self) {}
            fn pop(&mut self) -> Option<super::super::codec::Packet> { None }
        }

        let requests = Arc::new(Mutex::new(Requests::default()));
        let caps = VideoCaps { width: 1920, height: 1080, fps: 60, content: crate::guard::Content::Share };
        let mut share = Share::new(caps, Box::new(Source(requests.clone())), Arc::new(|| {}));
        let panel = View { width: 1280, height: 720, fps: 30 };
        let frame_bits = share_frame_bits(panel.width, panel.height);
        assert_eq!(frame_bits, 75_000, "a 720p share frame gets the top rung's saturation budget");
        for (now, rate, fps) in [
            (0, 20_000_000, 30),
            (1, 2_000_000, 15),
            (2, 1_200_000, 10),
            (3, 800_000, 5),
            (4, 400_000, 2),
            (5, 100_000, 1),
            // The dip still supports 2 fps but interrupts the wait for every higher step.
            (1_000_000, 20_000_000, 1),
            (2_000_000, 400_000, 1),
            (2_999_999, 20_000_000, 1),
            (3_000_000, 20_000_000, 2),
            (4_999_998, 20_000_000, 2),
            (4_999_999, 20_000_000, 30),
        ] {
            share.next(now, panel, rate, 2, false, true);
            assert_eq!(share.wanted_bps, estimate_for(frame_bits * u64::from(panel.fps), 2),
                "a still source needs the panel rate even while the encoder runs at {fps} fps");
            let requests = requests.lock().unwrap();
            let want = requests.wants.last().expect("the open panel must request capture");
            assert_eq!((want.width, want.height), (panel.width, panel.height), "the share keeps the panel's resolution at {rate} bit/s");
            assert_eq!(want.fps, fps, "fps must drop immediately and rise after two continuous seconds at {now} µs, {rate} bit/s");
            let kbps = u64::from(*requests.kbps.last().expect("the open panel must set the encoder bitrate"));
            let ask_kbps = (video_ask_for(rate, 2) / 1_000).max(1);
            assert_eq!(kbps, ask_kbps.max((frame_bits * u64::from(fps)).div_ceil(1_000)),
                "the encoder gets the full video ask with two audio streams, or a full frame's bits per frame, at {rate} bit/s");
            // At 100 kbit/s the ask is 1 kbit/s: a walk's Linux pair showed that as big macroblocks at the 1 fps floor.
            assert!(kbps * 1_000 / u64::from(fps) >= frame_bits, "the encoder must have {frame_bits} bits per frame at {fps} fps, {kbps} kbit/s");
        }
        let requests = requests.lock().unwrap();
        let wants: Vec<_> = requests.wants.iter().map(|view| view.fps).collect();
        assert_eq!(wants, [0, 30, 15, 10, 5, 2, 1, 2, 30], "only a sustained recovery reopens capture at a higher fps");
        drop(requests);
        share.next(5_000_000, View::default(), 100_000, 2, false, false);
        assert_eq!(share.wanted_bps, 0, "closing the last panel releases its wanted estimate");
    }

    #[test]
    fn a_watch_debounces_panel_resizes_and_closes_immediately() {
        let mut sub = Subscription::default();
        let large = View { width: 1280, height: 720, fps: 30 };
        let small = View { width: 640, height: 360, fps: 30 };
        sub.set(0, small);
        assert_eq!(sub.announced, small, "the initial VIEW is already queued by Watch");
        sub.set(100_000, large);
        sub.set(200_000, large);
        assert_eq!(sub.changed(399_999), None, "still resizing up");
        assert_eq!(sub.changed(400_000), Some(large), "stable larger panel is sent after 300 ms");
        sub.set(500_000, small);
        assert_eq!(sub.changed(1_499_999), None, "still resizing down");
        assert_eq!(sub.changed(1_500_000), Some(small), "stable smaller panel is sent after 1 s");
        sub.set(1_500_001, View::default());
        assert_eq!(sub.announced, View::default(), "closing was signalled immediately by Watch");
        assert_eq!(sub.changed(2_500_001), None, "unchanged panel must not resend a reliable VIEW");
    }
}
