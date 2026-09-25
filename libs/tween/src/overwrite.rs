//! Overwriting (design 5.13): the engine is GSAP's global timeline, so
//! overwrite rules see every tween in it.
//!
//! - `Overwrite::None` (GSAP `false`, the default): nothing is killed; the
//!   tween rendered later in the walk wins.
//! - `Overwrite::All` (GSAP `true`): at creation, every linked tween loses
//!   its tracks on the new tween's targets, whatever its state.
//! - `Overwrite::Auto` (GSAP `"auto"`): at the new tween's first render, the
//!   tracks it shares with tweens that are running at that moment (initted,
//!   unpaused, their global window containing now) are killed. The per-slot
//!   rival chain makes this O(tracks sharing the slot).
//!
//! A tween left without live tracks is killed (Interrupt below progress 1);
//! a group left without live children is killed too. Kills are permanent.

use crate::engine::*;
use crate::ids::PropKey;
use crate::spec::Targets;

impl TweenEngine {
    /// Overwrite `Auto` for node `n` at global time `g` (its init).
    pub(crate) fn overwrite_auto(&mut self, n: u32, g: f64) {
        let (s, e) = self.track_range(n);
        for k in s..e {
            if self.tr_meta[k as usize].flags & T_ALIVE == 0 {
                continue;
            }
            let slot = self.tr_slot[k as usize];
            // The chain holds the newest track first, so rivals die (and
            // report) newest first, as GSAP's backward getTweensOf loop does.
            let mut r = self.sl_first_track[slot as usize];
            while r != NIL {
                let next = self.tr_next_in_slot[r as usize];
                if r != k && self.tr_meta[r as usize].flags & T_ALIVE != 0 {
                    let m = self.tr_node[r as usize];
                    if m != n && self.is_running_at(m, g) {
                        self.kill_track(r);
                        self.after_track_kill(m);
                    }
                }
                r = next;
            }
        }
    }

    /// GSAP `getTweensOf(targets, globalTime)` for overwriting: the tween (or
    /// its stagger / keyframes group) is attached, initted, unpaused and its
    /// global window contains `g`.
    fn is_running_at(&mut self, m: u32, g: f64) -> bool {
        if self.hot[m as usize].flags & (F_KILLED | F_FREE) != 0 {
            return false;
        }
        let p = self.cold[m as usize].parent;
        let top = if p != NIL && self.kind(p) == K_GROUP {
            p
        } else {
            m
        };
        if !self.attached(top) || !self.window_contains(top, g) {
            return false;
        }
        top == m || self.window_contains(m, g)
    }

    fn window_contains(&mut self, x: u32, g: f64) -> bool {
        let h = self.hot[x as usize];
        if h.flags & F_INITTED == 0 || h.ts == 0.0 {
            return false;
        }
        let tdur = self.total_duration(x);
        self.global_time(x, 0.0) <= g && self.global_time(x, tdur) > g
    }

    /// After one of `m`'s tracks died: kill `m` when nothing of it is left,
    /// then its group when that has nothing left either.
    pub(crate) fn after_track_kill(&mut self, m: u32) {
        if self.cold[m as usize].live_tracks > 0 || self.has_live_child(m) {
            return;
        }
        self.kill_node(m, true);
        let p = self.cold[m as usize].parent;
        if p != NIL
            && self.kind(p) == K_GROUP
            && self.cold[p as usize].live_tracks == 0
            && !self.has_live_child(p)
        {
            self.kill_node(p, true);
        }
    }

    fn has_live_child(&self, n: u32) -> bool {
        let mut c = self.cold[n as usize].first;
        while c != NIL {
            if self.hot[c as usize].flags & (F_KILLED | F_FREE) == 0 {
                return true;
            }
            c = self.hot[c as usize].next;
        }
        false
    }

    /// Overwrite `All` for new node `n` on targets `t`, at creation.
    pub(crate) fn overwrite_all(&mut self, n: u32, t: Targets) {
        self.kill_tracks_matching(n, t, None);
    }

    /// Kills the live tracks (newest first) on `t` (and `props` when given)
    /// of every attached tween other than `except` and its descendants.
    pub(crate) fn kill_tracks_matching(
        &mut self,
        except: u32,
        t: Targets,
        props: Option<&[PropKey]>,
    ) {
        let mut k = self.tr_slot.len();
        while k > 0 {
            k -= 1;
            if self.tr_meta[k].flags & T_ALIVE == 0 {
                continue;
            }
            let s = self.tr_slot[k] as usize;
            let target = self.sl_target[s];
            if !(0..t.len()).any(|i| t.get(i) == target) {
                continue;
            }
            if let Some(ps) = props {
                if !ps.contains(&self.sl_key[s]) {
                    continue;
                }
            }
            let m = self.tr_node[k];
            if self.hot[m as usize].flags & (F_KILLED | F_FREE) != 0
                || self.within(m, except)
                || !self.attached(m)
            {
                continue;
            }
            self.kill_track(k as u32);
            self.after_track_kill(m);
        }
    }

    /// Whether `m` is `a` or one of its descendants.
    fn within(&self, m: u32, a: u32) -> bool {
        if a == NIL {
            return false;
        }
        let mut x = m;
        while x != NIL {
            if x == a {
                return true;
            }
            x = self.cold[x as usize].dp;
        }
        false
    }
}
