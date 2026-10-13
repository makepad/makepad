//! The motion recorder: always on, cheap, the last ~10 s of desk frames.
//!
//! Per desk frame: when it was drawn (and the gap since the last one), per
//! display the shown space, the slide position and the shelves' hide
//! amounts, the space captures in use and made, and per drawn window (live,
//! or drawn into a space capture) its drawn and shown rects, what it was
//! drawn from, its TileAnim, the client's configured and presented sizes.
//!
//! Records are plain structs in a ring whose slots are reused (no
//! allocation once warm, no formatting): JSON is written only by a dump —
//! the `wm` test service's `motion_trace` tool (makepad-remote `call
//! wm.motion_trace`), read by apps/wm/tools/frametrace/analyze.py.
//! `MAKEPAD_WM_FRAMETRACE=/abs/file.jsonl` also streams every frame to a
//! file (the headless rig).
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::Write as _;

/// How much history the ring keeps.
pub const KEEP_SECS: f64 = 10.0;
/// And at most this many frames (10 s at 240 Hz, plus slack for capture draws).
pub const KEEP_FRAMES: usize = 3000;

pub type R4 = [f32; 4];

pub fn r4(x: f64, y: f64, w: f64, h: f64) -> R4 {
    [x as f32, y as f32, w as f32, h as f32]
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Src {
    #[default]
    Live,
    LiveCap,
    Reuse,
    Frozen,
    DockWarp,
    MinGone,
}

impl Src {
    fn name(self) -> &'static str {
        match self {
            Src::Live => "live",
            Src::LiveCap => "live-cap",
            Src::Reuse => "reuse",
            Src::Frozen => "frozen",
            Src::DockWarp => "dockwarp",
            Src::MinGone => "min-gone",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Anim {
    pub cur: R4,
    pub tgt: R4,
    pub moving: bool,
    pub open: f32,
    pub close: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Host {
    /// The size the child was last told (points).
    pub cfg: [f32; 2],
    /// The settled size asked for, if any.
    pub tsz: Option<[f32; 2]>,
    /// The presented frame's pixels.
    pub px: Option<[u32; 2]>,
    pub serial: u64,
    pub has: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Win {
    pub client: u64,
    /// Drawn into this space capture (display, slot), else live.
    pub cap: Option<(u16, u16)>,
    /// The app id, interned (`Ring::apps`).
    pub app: u16,
    pub src: Src,
    pub drawn: R4,
    pub shown: R4,
    pub req: Option<[f32; 2]>,
    pub anim: Option<Anim>,
    pub host: Option<Host>,
    pub side: i8,
    pub scale: f32,
    pub compact: bool,
    pub pose_shown: R4,
    pub half: Option<R4>,
    pub swollen: Option<R4>,
    pub frozen: bool,
    pub hidden: bool,
    pub dragged: bool,
    pub minimized: bool,
    /// The layout's word: 0 normal, 1 maximised, 2 fullscreen.
    pub mode: u8,
    /// The layout has it parked on a shelf.
    pub parked: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Slide {
    pub pos: f32,
    pub v: f32,
    pub from: u16,
    pub to: u16,
}

#[derive(Clone, Debug, Default)]
pub struct Disp {
    pub rect: R4,
    pub area: Option<R4>,
    pub cur: u16,
    pub active: u16,
    pub slots: Vec<u16>,
    pub slide: Option<Slide>,
    pub hide: [f32; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Shot {
    pub key: (u16, u16),
    pub off: f32,
    pub dst: R4,
    pub drawn: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Leave {
    pub client: u64,
    pub side: i8,
    pub s0: f32,
    pub from: f32,
    pub cursor: [f32; 2],
    pub size: [f32; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Ease {
    pub client: u64,
    pub t: f32,
    pub folding: bool,
}

/// One desk frame.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub n: u64,
    /// `host::now()` at the end of the draw.
    pub t: f64,
    /// Wall-clock seconds the draw took (CPU side of the desk draw).
    pub draw_ms: f32,
    pub desk: R4,
    pub disp: Vec<Disp>,
    pub active_screen: u16,
    pub dragging: Vec<u64>,
    pub leave: Option<Leave>,
    pub enter: Option<Ease>,
    pub unfold: Option<Ease>,
    pub held: Vec<(u16, u16)>,
    pub made: Vec<((u16, u16), Vec<u64>)>,
    pub shots: Vec<Shot>,
    pub underlay: u16,
    pub strip: f32,
    pub dock_out: f32,
    pub animating: bool,
    pub focus: Option<u64>,
    pub zorder: Vec<u64>,
    /// The chrome hit rects drawn: (client, rect, what): "title", "close",
    /// "min", "max", or "rXY" a resize edge (x, y in -1..1).
    pub hits: Vec<(u64, R4, &'static str)>,
    /// The dock's app icons (interned app id, rect) while the dock is shown.
    pub dock: Vec<(u16, R4)>,
    pub wins: Vec<Win>,
}

impl Frame {
    fn clear(&mut self) {
        self.disp.clear();
        self.dragging.clear();
        self.held.clear();
        self.made.clear();
        self.shots.clear();
        self.zorder.clear();
        self.hits.clear();
        self.dock.clear();
        self.wins.clear();
        self.leave = None;
        self.enter = None;
        self.unfold = None;
    }
}

#[derive(Default)]
struct Ring {
    frames: VecDeque<Frame>,
    /// The frame being drawn.
    cur: Frame,
    /// The space capture being drawn into, if any.
    cap: Option<(u16, u16)>,
    next_n: u64,
    apps: Vec<String>,
    began: Option<std::time::Instant>,
    stream: Option<std::fs::File>,
    stream_tried: bool,
}

thread_local! {
    static RING: RefCell<Ring> = RefCell::new(Ring::default());
}

/// Every frame is also streamed to `MAKEPAD_WM_FRAMETRACE` (the test rig).
pub fn streaming() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_WM_FRAMETRACE").is_some())
}

/// The desk draw starts.
pub fn begin_frame() {
    RING.with_borrow_mut(|r| {
        r.began = Some(std::time::Instant::now());
        r.cur.clear();
        r.cap = None;
    });
}

pub fn set_cap(key: Option<(usize, usize)>) {
    RING.with_borrow_mut(|r| r.cap = key.map(|(d, s)| (d as u16, s as u16)));
}

pub fn app_id(name: &str) -> u16 {
    RING.with_borrow_mut(|r| match r.apps.iter().position(|a| a == name) {
        Some(i) => i as u16,
        None => {
            r.apps.push(name.to_string());
            (r.apps.len() - 1) as u16
        }
    })
}

pub fn win(mut w: Win) {
    RING.with_borrow_mut(|r| {
        w.cap = r.cap;
        r.cur.wins.push(w);
    });
}

pub fn shot(s: Shot) {
    RING.with_borrow_mut(|r| r.cur.shots.push(s));
}

pub fn made(key: (usize, usize), members: impl Iterator<Item = u64>) {
    RING.with_borrow_mut(|r| r.cur.made.push(((key.0 as u16, key.1 as u16), members.collect())));
}

/// The frame is over: `fill` writes the frame's own fields; the frame goes
/// into the ring (and the stream).
pub fn end_frame(fill: impl FnOnce(&mut Frame)) {
    RING.with_borrow_mut(|r| {
        fill(&mut r.cur);
        r.next_n += 1;
        r.cur.n = r.next_n;
        r.cur.draw_ms = r.began.take().map_or(0.0, |b| b.elapsed().as_secs_f32() * 1000.0);
        if !r.stream_tried {
            r.stream_tried = true;
            if let Some(path) = std::env::var_os("MAKEPAD_WM_FRAMETRACE") {
                r.stream = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
            }
        }
        if r.stream.is_some() {
            let mut line = String::new();
            json_frame(&mut line, &r.cur, &r.apps);
            line.push('\n');
            if let Some(f) = r.stream.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
        // The oldest frames past KEEP_SECS / KEEP_FRAMES go; the last one
        // dropped is the next frame's slot (its buffers are kept).
        let t = r.cur.t;
        let done = std::mem::take(&mut r.cur);
        r.frames.push_back(done);
        let mut spare = None;
        while r.frames.len() > 1 && (r.frames.len() > KEEP_FRAMES || r.frames.front().is_some_and(|f| t - f.t > KEEP_SECS)) {
            spare = r.frames.pop_front();
        }
        if let Some(spare) = spare {
            r.cur = spare;
        }
        r.cur.clear();
    });
}

/// Write the ring as JSONL to `path`; the number of frames written.
pub fn dump(path: &std::path::Path) -> std::io::Result<usize> {
    RING.with_borrow(|r| {
        let mut out = String::with_capacity(r.frames.len() * 1024);
        for f in &r.frames {
            json_frame(&mut out, f, &r.apps);
            out.push('\n');
        }
        let mut file = std::fs::File::create(path)?;
        file.write_all(out.as_bytes())?;
        Ok(r.frames.len())
    })
}

fn jr(o: &mut String, r: &R4) {
    let _ = write!(o, "[{:.2},{:.2},{:.2},{:.2}]", r[0], r[1], r[2], r[3]);
}

fn jopt_r(o: &mut String, r: &Option<R4>) {
    match r {
        Some(r) => jr(o, r),
        None => o.push_str("null"),
    }
}

fn json_frame(o: &mut String, f: &Frame, apps: &[String]) {
    let _ = write!(o, "{{\"n\":{},\"t\":{:.5},\"draw_ms\":{:.3},\"desk\":", f.n, f.t, f.draw_ms);
    jr(o, &f.desk);
    o.push_str(",\"disp\":[");
    for (i, d) in f.disp.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(o, "{{\"i\":{},\"r\":", i);
        jr(o, &d.rect);
        let _ = write!(o, ",\"cur\":{},\"slots\":{:?},\"active\":{},\"slide\":", d.cur, d.slots, d.active);
        match d.slide {
            Some(s) => {
                let _ = write!(o, "{{\"pos\":{:.5},\"v\":{:.4},\"from\":{},\"to\":{}}}", s.pos, s.v, s.from, s.to);
            }
            None => o.push_str("null"),
        }
        let _ = write!(o, ",\"hide\":[{:.4},{:.4}],\"area\":", d.hide[0], d.hide[1]);
        jopt_r(o, &d.area);
        o.push('}');
    }
    let _ = write!(o, "],\"active\":{},\"dragging\":{:?},\"leave\":", f.active_screen, f.dragging);
    match f.leave {
        Some(l) => {
            let _ = write!(o, "{{\"c\":{},\"side\":{},\"s0\":{:.4},\"from\":{:.2},\"cursor\":[{:.1},{:.1}],\"size\":[{:.1},{:.1}]}}",
                l.client, l.side, l.s0, l.from, l.cursor[0], l.cursor[1], l.size[0], l.size[1]);
        }
        None => o.push_str("null"),
    }
    for (name, e, key) in [("enter", f.enter, "e"), ("unfold", f.unfold, "t")] {
        let _ = write!(o, ",\"{}\":", name);
        match e {
            Some(e) => {
                let _ = write!(o, "{{\"c\":{},\"{}\":{:.4},\"fold\":{}}}", e.client, key, e.t, e.folding);
            }
            None => o.push_str("null"),
        }
    }
    o.push_str(",\"held\":[");
    for (i, k) in f.held.iter().enumerate() {
        let _ = write!(o, "{}[{},{}]", if i > 0 { "," } else { "" }, k.0, k.1);
    }
    let _ = write!(o, "],\"underlay\":{},\"strip\":{:.3},\"dock_out\":{:.4},\"anim\":{},\"focus\":{},\"zorder\":{:?},\"made\":[",
        f.underlay, f.strip, f.dock_out, f.animating, f.focus.map_or("null".to_string(), |c| c.to_string()), f.zorder);
    for (i, (k, m)) in f.made.iter().enumerate() {
        let _ = write!(o, "{}{{\"key\":[{},{}],\"m\":{:?}}}", if i > 0 { "," } else { "" }, k.0, k.1, m);
    }
    o.push_str("],\"shots\":[");
    for (i, s) in f.shots.iter().enumerate() {
        let _ = write!(o, "{}{{\"key\":[{},{}],\"off\":{:.5},\"dst\":", if i > 0 { "," } else { "" }, s.key.0, s.key.1, s.off);
        jr(o, &s.dst);
        let _ = write!(o, ",\"drawn\":{}}}", s.drawn);
    }
    o.push_str("],\"hits\":[");
    for (i, (c, r, k)) in f.hits.iter().enumerate() {
        let _ = write!(o, "{}{{\"c\":{},\"k\":\"{}\",\"r\":", if i > 0 { "," } else { "" }, c, k);
        jr(o, r);
        o.push('}');
    }
    o.push_str("],\"dock\":[");
    for (i, (a, r)) in f.dock.iter().enumerate() {
        let _ = write!(o, "{}{{\"app\":\"{}\",\"r\":", if i > 0 { "," } else { "" }, apps.get(*a as usize).map_or("", |s| s.as_str()));
        jr(o, r);
        o.push('}');
    }
    o.push_str("],\"ev\":[],\"w\":[");
    for (i, w) in f.wins.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(o, "{{\"c\":{},\"cap\":", w.client);
        match w.cap {
            Some((d, s)) => {
                let _ = write!(o, "[{},{}]", d, s);
            }
            None => o.push_str("null"),
        }
        let app = apps.get(w.app as usize).map_or("", |s| s.as_str());
        let _ = write!(o, ",\"app\":\"{}\",\"src\":\"{}\",\"d\":", app, w.src.name());
        jr(o, &w.drawn);
        o.push_str(",\"s\":");
        jr(o, &w.shown);
        o.push_str(",\"req\":");
        match w.req {
            Some(c) => {
                let _ = write!(o, "[{:.2},{:.2}]", c[0], c[1]);
            }
            None => o.push_str("null"),
        }
        o.push_str(",\"anim\":");
        match w.anim {
            Some(a) => {
                o.push_str("{\"cur\":");
                jr(o, &a.cur);
                o.push_str(",\"tgt\":");
                jr(o, &a.tgt);
                let _ = write!(o, ",\"mv\":{},\"open\":{:.3},\"close\":", a.moving, a.open);
                match a.close {
                    Some(c) => {
                        let _ = write!(o, "{:.3}", c);
                    }
                    None => o.push_str("null"),
                }
                o.push('}');
            }
            None => o.push_str("null"),
        }
        o.push_str(",\"host\":");
        match w.host {
            Some(h) => {
                let _ = write!(o, "{{\"cfg\":[{:.2},{:.2}],\"tsz\":", h.cfg[0], h.cfg[1]);
                match h.tsz {
                    Some(t) => {
                        let _ = write!(o, "[{:.2},{:.2}]", t[0], t[1]);
                    }
                    None => o.push_str("null"),
                }
                o.push_str(",\"px\":");
                match h.px {
                    Some(p) => {
                        let _ = write!(o, "[{},{}]", p[0], p[1]);
                    }
                    None => o.push_str("null"),
                }
                let _ = write!(o, ",\"ser\":{},\"has\":{}}}", h.serial, h.has);
            }
            None => o.push_str("null"),
        }
        let _ = write!(o, ",\"pose\":{{\"side\":{},\"k\":{:.4},\"cmp\":{},\"shown\":", w.side, w.scale, w.compact);
        jr(o, &w.pose_shown);
        o.push_str("},\"half\":");
        jopt_r(o, &w.half);
        o.push_str(",\"swl\":");
        jopt_r(o, &w.swollen);
        let _ = write!(o, ",\"frz\":{},\"hid\":{},\"drag\":{},\"min\":{},\"mode\":{},\"parked\":{}}}", w.frozen, w.hidden, w.dragged, w.minimized, w.mode, w.parked);
    }
    o.push_str("]}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_last_seconds_and_reuses_its_slots() {
        for i in 0..50 {
            begin_frame();
            win(Win { client: 7, drawn: r4(1.0, 2.0, 3.0, 4.0), ..Default::default() });
            end_frame(|f| f.t = i as f64 * 0.5);
        }
        let n = RING.with_borrow(|r| (r.frames.len(), r.frames.front().unwrap().t, r.frames.back().unwrap().wins.len()));
        // 10 s at 0.5 s a frame: 21 frames (both ends kept).
        assert!(n.0 <= 21 && n.0 >= 20, "{:?}", n);
        assert_eq!(n.2, 1);
        let mut s = String::new();
        RING.with_borrow(|r| json_frame(&mut s, r.frames.back().unwrap(), &r.apps));
        assert!(s.starts_with("{\"n\":50,") && s.contains("\"c\":7,\"cap\":null"), "{s}");
    }
}
