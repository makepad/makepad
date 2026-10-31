//! Floating presentation over the persistent tiling tree. Switching styles never
//! destroys groups, split ratios, workspace membership, or an app instance.
use crate::layout::{ClientId, LRect};

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopWindow {
    pub client: ClientId,
    pub rect: LRect,
    pub minimized: bool,
    pub maximized: bool,
    pub snap: Option<crate::snap::Zone>,
    /// Parked by spacewarp: its desk rect keeps the full size and may lie
    /// outside the screen (only the shown, scaled rect is on it).
    pub warped: bool,
}
#[derive(Clone, Debug, Default)]
pub struct DesktopWindows {
    pub enabled: bool,
    pub windows: Vec<DesktopWindow>,
    /// Bottom of the area a maximised window leaves to a floating shelf (the
    /// macOS dock hovers over the desk, so the area does not reserve it).
    pub shelf_reserve: f64,
}
impl DesktopWindows {
    pub fn ensure(&mut self, client: ClientId, area: LRect) {
        if self.get(client).is_some() {
            return;
        }
        let offset = (self.windows.len() % 7) as f64 * 30.0;
        let mut rect = area.centered((area.w * 0.72).min(1000.0), (area.h * 0.76).min(720.0));
        rect.x = (area.x + 42.0 + offset).min(area.x + area.w - rect.w);
        rect.y = (area.y + 32.0 + offset).min(area.y + area.h - rect.h);
        self.windows.push(DesktopWindow {
            client,
            rect,
            minimized: false,
            maximized: false,
            snap: None,
            warped: false,
        });
    }
    /// The size the app asked for (`w` x `h`, its whole window): clamped to `area` (the
    /// screen minus bar and dock) and centred there. A maximised, snapped or parked
    /// window keeps its place. True when the window changed.
    pub fn request_size(&mut self, client: ClientId, w: f64, h: f64, area: LRect) -> bool {
        let Some(win) = self.get_mut(client) else { return false };
        if win.maximized || win.snap.is_some() || win.warped || !(w > 0.0 && h > 0.0) {
            return false;
        }
        let rect = area.centered(w.max(1.0), h.max(1.0));
        let changed = win.rect != rect;
        win.rect = rect;
        changed
    }
    pub fn get(&self, client: ClientId) -> Option<&DesktopWindow> {
        self.windows.iter().find(|w| w.client == client)
    }
    pub fn get_mut(&mut self, client: ClientId) -> Option<&mut DesktopWindow> {
        self.windows.iter_mut().find(|w| w.client == client)
    }
    pub fn minimized(&self, client: ClientId) -> bool {
        self.enabled && self.get(client).is_some_and(|w| w.minimized)
    }
    pub fn raise(&mut self, client: ClientId) {
        if let Some(i) = self.windows.iter().position(|w| w.client == client) {
            let w = self.windows.remove(i);
            self.windows.push(w);
        }
    }
    /// The drawn rects. `band` is where maximize and the snap zones apply
    /// (the display's centre band); every other window stays inside `area`.
    pub fn rects(&self, clients: &[ClientId], area: LRect, band: LRect) -> Vec<(ClientId, LRect)> {
        self.windows
            .iter()
            .filter(|w| clients.contains(&w.client) && !w.minimized)
            .map(|w| {
                let r = if w.maximized { LRect::new(band.x, band.y, band.w, (band.h - self.shelf_reserve).max(1.0)) } else if let Some(zone)=w.snap {zone.bounds(band)} else if w.warped { w.rect } else { fit(w.rect, area) };
                (w.client, r)
            })
            .collect()
    }
}
/// Keep every title bar reachable after an output/pane resize.
pub fn fit(mut r: LRect, area: LRect) -> LRect {
    r.w = r.w.min(area.w).max(1.0);
    r.h = r.h.min(area.h).max(1.0);
    // Allow the body offscreen/behind a dock, keeping a title-bar grab reachable.
    let reachable_w = r.w.min(80.0);
    let reachable_h = r.h.min(28.0);
    r.x =
        r.x.clamp(area.x - r.w + reachable_w, area.x + area.w - reachable_w);
    r.y = r.y.clamp(area.y, area.y + area.h - reachable_h);
    r
}
#[cfg(test)]
mod tests {
    use super::*;
    /// 149: Stage asks for 1680x1040 and opened at the default 715x606 on a 1969x1231
    /// desk. The asked size is honoured, centred; an ask bigger than the desk is clamped
    /// to it; a maximised window keeps its place.
    #[test]
    fn an_apps_requested_size_is_honoured_clamped_and_centred() {
        let area = LRect::new(0.0, 26.0, 1969.0, 1231.0 - 26.0 - 74.0);
        let mut d = DesktopWindows::default();
        d.enabled = true;
        d.ensure(1, area);
        assert!(d.request_size(1, 1680.0, 1040.0 + 32.0, area));
        let r = d.get(1).unwrap().rect;
        assert_eq!((r.w, r.h), (1680.0, 1072.0));
        assert!((r.x + r.w * 0.5 - (area.x + area.w * 0.5)).abs() < 1e-9 && (r.y + r.h * 0.5 - (area.y + area.h * 0.5)).abs() < 1e-9, "{r:?}");
        // bigger than the desk: the desk's usable area, exactly
        assert!(d.request_size(1, 4000.0, 3000.0, area));
        assert_eq!(d.get(1).unwrap().rect, area);
        // a maximised window is left alone
        d.get_mut(1).unwrap().maximized = true;
        assert!(!d.request_size(1, 800.0, 600.0, area));
        assert!(!d.request_size(2, 800.0, 600.0, area), "unknown window");
    }
    #[test]
    fn maximize_and_minimize_keep_restore_rect_and_stack() {
        let area = LRect::new(0.0, 30.0, 1200.0, 800.0);
        let mut d = DesktopWindows::default();
        d.enabled = true;
        d.ensure(1, area);
        d.ensure(2, area);
        let original = d.get(1).unwrap().rect;
        d.get_mut(1).unwrap().maximized = true;
        assert_eq!(d.rects(&[1, 2], area, area)[0].1, area);
        d.get_mut(1).unwrap().minimized = true;
        assert_eq!(d.rects(&[1, 2], area, area).len(), 1);
        d.get_mut(1).unwrap().minimized = false;
        d.get_mut(1).unwrap().maximized = false;
        d.raise(1);
        assert_eq!(d.rects(&[1, 2], area, area).last(), Some(&(1, original)));
    }
    #[test]
    fn shrinking_the_workarea_keeps_windows_reachable() {
        assert_eq!(
            fit(
                LRect::new(900.0, 700.0, 800.0, 500.0),
                LRect::new(10.0, 40.0, 300.0, 200.0)
            ),
            LRect::new(230.0, 212.0, 300.0, 200.0)
        );
    }
    #[test]
    fn snapping_keeps_restore_size_and_tracks_a_resized_workarea() {
        let area=LRect::new(10.,42.,1380.,794.);
        let mut d=DesktopWindows::default();
        d.ensure(1,area);
        let restore=d.get(1).unwrap().rect;
        d.get_mut(1).unwrap().snap=Some(crate::snap::Zone::BottomRight);
        assert_eq!(d.rects(&[1],area,area)[0].1,LRect::new(700.,439.,690.,397.));
        let small=LRect::new(10.,42.,800.,600.);
        assert_eq!(d.rects(&[1],small,small)[0].1,LRect::new(410.,342.,400.,300.));
        assert_eq!(d.get(1).unwrap().rect,restore);
        d.get_mut(1).unwrap().snap=None;
        assert_eq!(d.rects(&[1],area,area)[0].1,restore);
    }
}
