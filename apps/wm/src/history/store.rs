//! The layout history as plain data: snapshots, the ring of them, the text
//! file they are kept in, and matching a snapshot's windows to live clients.
//! No WM state in here, so all of it is unit-tested.
use makepad_app_host::ClientId;
use crate::layout::LRect;
use crate::snap::Zone;

/// Snapshots kept.
pub const RING: usize = 200;

/// First line of the file; another version reads as empty.
const HEADER: &str = "wm-layouts 2";

/// Two rects closer than this (logical px) in every field are the same.
const SAME_PX: f64 = 0.5;

/// Every zone, in file order: a zone is stored as its index here.
const ZONES: [Zone; 14] = [
    Zone::Maximize,
    Zone::Left,
    Zone::Right,
    Zone::Top,
    Zone::Bottom,
    Zone::TopLeft,
    Zone::TopRight,
    Zone::BottomLeft,
    Zone::BottomRight,
    Zone::LeftWide,
    Zone::RightNarrow,
    Zone::LeftThird,
    Zone::MiddleThird,
    Zone::RightThird,
];

/// One window of an arrangement.
#[derive(Clone, Debug, PartialEq)]
pub struct Win {
    /// App identity: the registry id and the title at the time.
    pub app: String,
    pub title: String,
    /// The live client this was taken from; None for a window read back
    /// from the file (it is matched by identity then).
    pub client: Option<ClientId>,
    /// Its desk (logical) rect: a parked window keeps the full size here.
    pub rect: LRect,
    pub minimized: bool,
    pub maximized: bool,
    /// Parked by spacewarp.
    pub parked: bool,
    /// The desktop space it is on (0-based among the display's desktops; a
    /// window in a fullscreen space counts for the desktop it came from).
    pub space: usize,
    pub snap: Option<Zone>,
    /// The focused window of its screen.
    pub focus: bool,
}

/// All floating windows at one moment, back to front (z-order).
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Wall clock, seconds since the epoch.
    pub t: u64,
    /// The desktop style it was taken under (`DesktopStyle as u8`): rects of
    /// another style mean nothing, so a scrub only visits its own.
    pub style: u8,
    /// How many desktop spaces there were (the most of any display), and
    /// which one the active display showed.
    pub desktops: usize,
    pub cur: usize,
    pub wins: Vec<Win>,
}

fn same_rect(a: LRect, b: LRect) -> bool {
    (a.x - b.x).abs() < SAME_PX
        && (a.y - b.y).abs() < SAME_PX
        && (a.w - b.w).abs() < SAME_PX
        && (a.h - b.h).abs() < SAME_PX
}

impl Win {
    /// The same window in the same state. Titles are not compared (they
    /// change on their own); clients only when both sides have one.
    fn same(&self, o: &Win) -> bool {
        self.app == o.app
            && (self.client.is_none() || o.client.is_none() || self.client == o.client)
            && same_rect(self.rect, o.rect)
            && self.minimized == o.minimized
            && self.maximized == o.maximized
            && self.parked == o.parked
            && self.space == o.space
            && self.snap == o.snap
            && self.focus == o.focus
    }
}

impl Snapshot {
    /// The same arrangement: same windows, state, stacking and focus.
    pub fn same(&self, o: &Snapshot) -> bool {
        self.style == o.style
            && self.desktops == o.desktops
            && self.wins.len() == o.wins.len()
            && self.wins.iter().zip(&o.wins).all(|(a, b)| a.same(b))
    }
}

impl Snapshot {
    /// The same arrangement as far as desktop `space` goes.
    pub fn same_in(&self, o: &Snapshot, space: usize) -> bool {
        let mine = |s: &Snapshot| s.wins.iter().filter(|w| w.space == space).cloned().collect::<Vec<Win>>();
        let (a, b) = (mine(self), mine(o));
        self.style == o.style && a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.same(y))
    }
}

/// The ring, oldest first.
#[derive(Default)]
pub struct Ring {
    pub snaps: Vec<Snapshot>,
}

impl Ring {
    /// Keep `s` unless it is empty or the same as the newest. True if stored.
    pub fn push(&mut self, s: Snapshot) -> bool {
        if s.wins.is_empty() || self.snaps.last().is_some_and(|l| l.same(&s)) {
            return false;
        }
        self.snaps.push(s);
        if self.snaps.len() > RING {
            self.snaps.remove(0);
        }
        true
    }

    /// Keep only the windows `keep` accepts; snapshots left empty, or the same as the
    /// one before, go.
    pub fn retain_wins(&mut self, keep: impl Fn(&Win) -> bool) {
        let snaps = std::mem::take(&mut self.snaps);
        for mut s in snaps {
            s.wins.retain(|w| keep(w));
            self.push(s);
        }
    }

    /// The snapshots a scrub of desktop `space` visits: those under `style`
    /// in which that desktop's windows differ from the last one visited,
    /// oldest first. Each desktop has a history of its own.
    pub fn of_space(&self, style: u8, space: usize) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for i in self.of_style(style) {
            let mine = |s: &Snapshot| s.wins.iter().filter(|w| w.space == space).cloned().collect::<Vec<Win>>();
            let now = mine(&self.snaps[i]);
            if now.is_empty() && out.is_empty() {
                continue;
            }
            let same = out.last().is_some_and(|&l| {
                let before = mine(&self.snaps[l]);
                before.len() == now.len() && before.iter().zip(&now).all(|(a, b)| a.same(b))
            });
            if !same {
                out.push(i);
            }
        }
        out
    }

    /// Indices of the snapshots taken under `style`, oldest first.
    pub fn of_style(&self, style: u8) -> Vec<usize> {
        self.snaps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.style == style)
            .map(|(i, _)| i)
            .collect()
    }
}

fn esc(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// The file: a header, then per snapshot `S <secs> <style> <desktops> <shown>`
/// and one `W <flags> <desktop> <zone|-> <x> <y> <w> <h> <app>TAB<title>`
/// line per window.
pub fn serialise(ring: &Ring) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for s in &ring.snaps {
        out.push_str(&format!("S {} {} {} {}\n", s.t, s.style, s.desktops, s.cur));
        for w in &s.wins {
            let mut flags = String::new();
            for (on, c) in [(w.minimized, 'm'), (w.maximized, 'M'), (w.parked, 'p'), (w.focus, 'f')] {
                if on {
                    flags.push(c);
                }
            }
            if flags.is_empty() {
                flags.push('-');
            }
            let zone = w
                .snap
                .and_then(|z| ZONES.iter().position(|o| *o == z))
                .map_or("-".to_string(), |i| i.to_string());
            out.push_str(&format!("W {} {} {} {} {} {} {} ", flags, w.space, zone, w.rect.x, w.rect.y, w.rect.w, w.rect.h));
            esc(&w.app, &mut out);
            out.push('\t');
            esc(&w.title, &mut out);
            out.push('\n');
        }
    }
    out
}

fn parse_win(line: &str) -> Option<Win> {
    let mut f = line.splitn(8, ' ');
    let flags = f.next()?;
    let space: usize = f.next()?.parse().ok()?;
    let zone = f.next()?;
    let mut n = [0.0f64; 4];
    for v in n.iter_mut() {
        *v = f.next()?.parse().ok().filter(|v: &f64| v.is_finite())?;
    }
    let (app, title) = f.next()?.split_once('\t')?;
    Some(Win {
        app: unesc(app),
        title: unesc(title),
        client: None,
        rect: LRect::new(n[0], n[1], n[2], n[3]),
        minimized: flags.contains('m'),
        maximized: flags.contains('M'),
        parked: flags.contains('p'),
        space,
        snap: zone.parse::<usize>().ok().and_then(|i| ZONES.get(i).copied()),
        focus: flags.contains('f'),
    })
}

/// Read a file written by [`serialise`]. A damaged line is skipped, an
/// unknown version gives an empty ring; only the newest [`RING`] are kept.
pub fn parse(text: &str) -> Ring {
    let mut lines = text.lines();
    let mut ring = Ring::default();
    if lines.next() != Some(HEADER) {
        return ring;
    }
    for line in lines {
        if let Some(rest) = line.strip_prefix("S ") {
            let mut f = rest.split(' ');
            if let (Some(Ok(t)), Some(Ok(style)), Some(Ok(desktops)), Some(Ok(cur))) =
                (f.next().map(str::parse), f.next().map(str::parse), f.next().map(str::parse), f.next().map(str::parse))
            {
                ring.snaps.push(Snapshot { t, style, desktops, cur, wins: Vec::new() });
            }
        } else if let Some(rest) = line.strip_prefix("W ") {
            if let (Some(s), Some(w)) = (ring.snaps.last_mut(), parse_win(rest)) {
                s.wins.push(w);
            }
        }
    }
    ring.snaps.retain(|s| !s.wins.is_empty());
    let extra = ring.snaps.len().saturating_sub(RING);
    ring.snaps.drain(..extra);
    ring
}

/// A live client as far as matching cares.
#[derive(Clone, Debug, PartialEq)]
pub struct Live {
    pub client: ClientId,
    pub app: String,
    pub title: String,
}

/// The live client for each window of `wins`, or None when it is gone. A
/// window taken from a live client keeps that client while it runs (its title
/// may have changed); the rest (a restart) match an unclaimed client of the
/// same app and title. Nobody is matched twice.
pub fn resolve(wins: &[Win], live: &[Live]) -> Vec<Option<ClientId>> {
    let mut taken: Vec<ClientId> = Vec::new();
    let mut out: Vec<Option<ClientId>> = vec![None; wins.len()];
    for (i, w) in wins.iter().enumerate() {
        if let Some(c) = w.client {
            if live.iter().any(|l| l.client == c && l.app == w.app) && !taken.contains(&c) {
                taken.push(c);
                out[i] = Some(c);
            }
        }
    }
    for (i, w) in wins.iter().enumerate() {
        if out[i].is_some() || w.client.is_some() {
            continue;
        }
        if let Some(l) = live
            .iter()
            .find(|l| l.app == w.app && l.title == w.title && !taken.contains(&l.client))
        {
            taken.push(l.client);
            out[i] = Some(l.client);
        }
    }
    out
}

/// "just now", "40 s ago", "12 min ago", "3 h ago", "2 d ago".
pub fn ago(secs: u64) -> String {
    match secs {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{secs} s ago"),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(app: &str, title: &str, client: Option<ClientId>, x: f64) -> Win {
        Win {
            app: app.into(),
            title: title.into(),
            client,
            rect: LRect::new(x, 20.0, 300.0, 200.0),
            minimized: false,
            maximized: false,
            parked: false,
            space: 0,
            snap: None,
            focus: false,
        }
    }

    fn snap(t: u64, wins: Vec<Win>) -> Snapshot {
        Snapshot { t, style: 1, desktops: 1, cur: 0, wins }
    }

    #[test]
    fn identical_consecutive_snapshots_are_not_stored() {
        let mut r = Ring::default();
        assert!(r.push(snap(1, vec![win("a", "x", Some(1), 10.0)])));
        // Later, sub-pixel jitter, another title: the same arrangement.
        assert!(!r.push(snap(9, vec![win("a", "y", Some(1), 10.2)])));
        assert!(r.push(snap(10, vec![win("a", "x", Some(1), 50.0)])));
        // Back to the first one is a change from the newest.
        assert!(r.push(snap(11, vec![win("a", "x", Some(1), 10.0)])));
        assert_eq!(r.snaps.len(), 3);
    }

    #[test]
    fn state_stacking_and_focus_count_as_changes() {
        let mut r = Ring::default();
        let a = win("a", "", Some(1), 0.0);
        let b = win("b", "", Some(2), 0.0);
        assert!(r.push(snap(1, vec![a.clone(), b.clone()])));
        assert!(r.push(snap(2, vec![b.clone(), a.clone()])));
        let mut f = a.clone();
        f.focus = true;
        assert!(r.push(snap(3, vec![b.clone(), f])));
        let mut m = a.clone();
        m.minimized = true;
        assert!(r.push(snap(4, vec![b, m])));
    }

    /// Taking an app's windows out (example apps) drops the snapshots left empty and
    /// the ones that became the same as the one before.
    #[test]
    fn windows_taken_out_leave_no_empty_or_repeated_layouts() {
        let mut r = Ring::default();
        r.push(snap(1, vec![win("counter", "", Some(1), 0.0)]));
        r.push(snap(2, vec![win("notes", "", Some(2), 0.0), win("counter", "", Some(1), 0.0)]));
        r.push(snap(3, vec![win("notes", "", Some(2), 0.0), win("counter", "", Some(1), 40.0)]));
        r.push(snap(4, vec![win("notes", "", Some(2), 9.0)]));
        r.retain_wins(|w| w.app != "counter");
        assert_eq!(r.snaps.iter().map(|s| s.t).collect::<Vec<_>>(), vec![2, 4]);
    }

    #[test]
    fn an_empty_desk_is_not_a_layout() {
        let mut r = Ring::default();
        assert!(!r.push(snap(1, vec![])));
    }

    #[test]
    fn the_ring_keeps_the_newest_200() {
        let mut r = Ring::default();
        for i in 0..(RING as u64 + 30) {
            assert!(r.push(snap(i, vec![win("a", "", Some(1), i as f64 * 3.0)])));
        }
        assert_eq!(r.snaps.len(), RING);
        assert_eq!(r.snaps[0].t, 30);
        assert_eq!(r.snaps[RING - 1].t, RING as u64 + 29);
    }

    #[test]
    fn styles_are_visited_separately() {
        let mut r = Ring::default();
        r.push(Snapshot { t: 1, style: 0, desktops: 1, cur: 0, wins: vec![win("a", "", None, 0.0)] });
        r.push(Snapshot { t: 2, style: 1, desktops: 1, cur: 0, wins: vec![win("a", "", None, 5.0)] });
        r.push(Snapshot { t: 3, style: 1, desktops: 1, cur: 0, wins: vec![win("a", "", None, 9.0)] });
        assert_eq!(r.of_style(1), vec![1, 2]);
        assert_eq!(r.of_style(0), vec![0]);
    }

    fn rich_ring() -> Ring {
        let mut r = Ring::default();
        let mut a = win("terminal", "tab\tname\\ with\nodd chars", Some(7), 12.5);
        a.minimized = true;
        a.snap = Some(Zone::RightThird);
        let mut b = win("music", "Song", Some(8), -400.25);
        b.parked = true;
        b.focus = true;
        b.maximized = true;
        b.space = 2;
        let mut s = snap(100, vec![a, b]);
        s.desktops = 3;
        s.cur = 2;
        r.push(s);
        r.push(Snapshot { t: 200, style: 3, desktops: 1, cur: 0, wins: vec![win("files", "Home", Some(9), 0.0)] });
        r
    }

    #[test]
    fn each_desktop_has_a_history_of_its_own() {
        let mut r = Ring::default();
        let on = |space: usize, x: f64| Win { space, ..win("a", "", Some(1), x) };
        let other = |x: f64| Win { space: 1, ..win("b", "", Some(2), x) };
        r.push(snap(1, vec![on(0, 0.0), other(0.0)]));
        // Only desktop 1's window moves: desktop 0 has nothing new.
        r.push(snap(2, vec![on(0, 0.0), other(50.0)]));
        r.push(snap(3, vec![on(0, 90.0), other(50.0)]));
        assert_eq!(r.of_space(1, 0), vec![0, 2]);
        assert_eq!(r.of_space(1, 1), vec![0, 1]);
        assert_eq!(r.of_space(1, 5), Vec::<usize>::new());
    }

    #[test]
    fn serialise_parse_roundtrip() {
        let r = rich_ring();
        let back = parse(&serialise(&r));
        assert_eq!(back.snaps.len(), 2);
        for (a, b) in r.snaps.iter().zip(&back.snaps) {
            assert_eq!((a.t, a.style), (b.t, b.style));
            assert_eq!(a.wins.len(), b.wins.len());
            for (x, y) in a.wins.iter().zip(&b.wins) {
                // Clients are runtime only.
                assert_eq!(y.client, None);
                assert_eq!(Win { client: None, ..x.clone() }, *y);
            }
        }
        // And the file reads the same ring back to the same file.
        assert_eq!(serialise(&back), serialise(&r));
    }

    #[test]
    fn damaged_files_degrade() {
        assert!(parse("").snaps.is_empty());
        assert!(parse("wm-layouts 9\nS 1 1 1 0\nW - 0 - 0 0 1 1 a\tb\n").snaps.is_empty());
        let good = serialise(&rich_ring());
        let broken = good.replace("W - 0 - 12.5 20 300", "W - 0 - zz 20 300").replace("S 200 3", "S x 3");
        let r = parse(&broken);
        assert_eq!(r.snaps.len(), 1);
        assert!(parse("W - 0 - 0 0 1 1 a\tb\nwm-layouts 2\n").snaps.is_empty());
    }

    fn live(client: ClientId, app: &str, title: &str) -> Live {
        Live { client, app: app.into(), title: title.into() }
    }

    #[test]
    fn matching_after_a_restart_is_by_app_and_title() {
        let saved = parse(&serialise(&rich_ring()));
        let wins = &saved.snaps[0].wins;
        // New client ids; the music player came back, the terminal's title
        // changed, a second music window is new.
        let l = [live(31, "music", "Song"), live(32, "terminal", "other"), live(33, "music", "Song")];
        assert_eq!(resolve(wins, &l), vec![None, Some(31)]);
    }

    #[test]
    fn a_running_client_keeps_its_window_through_a_title_change() {
        let wins = [win("terminal", "old", Some(5), 0.0), win("terminal", "old", Some(6), 0.0)];
        let l = [live(6, "terminal", "new"), live(5, "terminal", "new2")];
        assert_eq!(resolve(&wins, &l), vec![Some(5), Some(6)]);
    }

    #[test]
    fn closed_clients_are_skipped_and_nobody_matches_twice() {
        let wins = [win("a", "t", Some(1), 0.0), win("a", "t", None, 0.0), win("b", "t", Some(3), 0.0)];
        let l = [live(1, "a", "t"), live(2, "a", "t")];
        assert_eq!(resolve(&wins, &l), vec![Some(1), Some(2), None]);
        // A recycled id of another app is not the same window.
        assert_eq!(resolve(&[win("a", "t", Some(1), 0.0)], &[live(1, "b", "t")]), vec![None]);
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(ago(0), "just now");
        assert_eq!(ago(40), "40 s ago");
        assert_eq!(ago(12 * 60 + 5), "12 min ago");
        assert_eq!(ago(3 * 3600), "3 h ago");
        assert_eq!(ago(2 * 86400 + 5), "2 d ago");
    }
}
