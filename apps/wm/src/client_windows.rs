//! A hosted app's windows after its first (Stage's output window, a second view): each is
//! a window of its own on the desk, with its own tile, frame, focus and close. Its slot has
//! no process of its own: messages go to its app's process, and the app's frames, input and
//! window ops for it carry its `window_id` (`ime::HostedWindow`).
//!
//! This is the bookkeeping: which desk client shows which window of which app.

use makepad_app_host::ClientId;
use std::collections::HashMap;

/// Where a later window's tile starts in the app's own coordinates: far apart, so the app
/// finds the window a pointer event is for by its position (`window_id_contains`).
pub const ORIGIN_STRIDE: f64 = 100_000.0;

#[derive(Default)]
pub struct ClientWindows {
    /// (app's client, window_id) -> the desk client showing that window.
    tiles: HashMap<(ClientId, usize), ClientId>,
    /// The desk client of a later window -> (app's client, window_id).
    owners: HashMap<ClientId, (ClientId, usize)>,
}

impl ClientWindows {
    /// `window_id` of the app `client` is shown by the desk client `tile`.
    pub fn add(&mut self, client: ClientId, window_id: usize, tile: ClientId) {
        self.tiles.insert((client, window_id), tile);
        self.owners.insert(tile, (client, window_id));
    }

    /// The desk client that shows `window_id` of `client`: the window's own, or the app's
    /// (its first window, and anything not known as a later window).
    pub fn route(&self, client: ClientId, window_id: usize) -> ClientId {
        self.tiles.get(&(client, window_id)).copied().unwrap_or(client)
    }

    /// The app and window a desk client shows, when it is a later window.
    pub fn owner(&self, tile: ClientId) -> Option<(ClientId, usize)> {
        self.owners.get(&tile).copied()
    }

    /// The app closed `window_id`: its desk client, which goes.
    pub fn closed(&mut self, client: ClientId, window_id: usize) -> Option<ClientId> {
        let tile = self.tiles.remove(&(client, window_id))?;
        self.owners.remove(&tile);
        Some(tile)
    }

    /// A desk client went (closed, or its app ended): forget it. When it was an app, its
    /// later windows' desk clients are returned: they go with it.
    pub fn forget(&mut self, gone: ClientId) -> Vec<ClientId> {
        if let Some(key) = self.owners.remove(&gone) {
            self.tiles.remove(&key);
            return Vec::new();
        }
        let windows: Vec<ClientId> = self.owners.iter().filter(|(_, (c, _))| *c == gone).map(|(t, _)| *t).collect();
        for tile in &windows {
            if let Some(key) = self.owners.remove(tile) {
                self.tiles.remove(&key);
            }
        }
        windows
    }

    /// The desk clients showing `client`'s windows: the app's own first, then its later ones.
    pub fn tiles_of(&self, client: ClientId) -> Vec<ClientId> {
        let mut out = vec![client];
        let mut later: Vec<(usize, ClientId)> = self.tiles.iter().filter(|((c, _), _)| *c == client).map(|((_, w), t)| (*w, *t)).collect();
        later.sort();
        out.extend(later.into_iter().map(|(_, t)| t));
        out
    }

    /// The origin of the n-th later window of `client` (1 for the first one).
    pub fn next_origin(&self, client: ClientId) -> f64 {
        (self.owners.values().filter(|(c, _)| *c == client).count() + 1) as f64 * ORIGIN_STRIDE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake client (app 7, main window 1) opens a second window (2), which gets a desk
    /// client of its own (40); frames route by window; closing the second window removes
    /// only its tile; the app ending removes every later window with it.
    #[test]
    fn a_client_opens_and_closes_a_second_window() {
        let mut w = ClientWindows::default();
        assert_eq!(w.next_origin(7), ORIGIN_STRIDE);
        w.add(7, 2, 40);
        assert_eq!(w.route(7, 1), 7, "the main window stays the app's tile");
        assert_eq!(w.route(7, 2), 40);
        assert_eq!(w.owner(40), Some((7, 2)));
        assert_eq!(w.owner(7), None);
        assert_eq!(w.next_origin(7), 2.0 * ORIGIN_STRIDE, "a third window starts further away");
        // the app closes its second window: that tile goes, the app stays
        assert_eq!(w.closed(7, 2), Some(40));
        assert_eq!(w.route(7, 2), 7);
        assert_eq!(w.closed(7, 2), None, "closed once");
        // opened again, then the app ends: its later windows go with it
        w.add(7, 2, 41);
        w.add(7, 3, 42);
        w.add(8, 2, 50);
        let mut gone = w.forget(7);
        gone.sort();
        assert_eq!(gone, vec![41, 42]);
        assert_eq!(w.route(8, 2), 50, "another app's window is untouched");
        // the person closes a later window's tile: only that window is forgotten
        assert!(w.forget(50).is_empty());
        assert_eq!(w.route(8, 2), 8);
    }
}
