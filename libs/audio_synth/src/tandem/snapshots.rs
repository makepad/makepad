//! What picking an oscillator type applies (the reference's menu "Init"
//! snapshots, interop facts from the spec): values and flags over the
//! type's slot defaults. Used for interactive type changes only; a patch
//! load never applies them. The effects' snapshots are the family's own
//! ([`crate::kit::fxs::init_values`]).

use crate::kit::core::SlotValue;
use crate::kit::oscs;

/// The slots an oscillator of type `ty` starts with when picked, and its
/// retrigger flag (None: keep).
pub fn osc(ty: u8) -> ([SlotValue; 7], Option<bool>) {
    let mut s = oscs::default_slots(ty);
    let set = |s: &mut [SlotValue; 7], k: usize, v: f32| s[k].v = v;
    let mut retrigger = None;
    match ty {
        0 => {
            for (k, v) in [(0, 0.0), (1, 0.5), (2, 0.5), (3, 0.0), (4, 0.0), (5, 0.1), (6, 1.0)] {
                set(&mut s, k, v);
            }
            s[5].extend = false;
            retrigger = Some(false);
        }
        8 => {
            for (k, v) in [(0, 1.0), (1, 0.0), (2, 0.0), (3, 0.5), (4, 0.0), (5, 0.1), (6, 1.0)] {
                set(&mut s, k, v);
            }
            s[1].deactivated = true;
            s[2].deactivated = true;
            s[5].extend = false;
            retrigger = Some(false);
        }
        2 | 7 => {
            s[0].extend = true;
            s[5].extend = false;
            retrigger = Some(false);
        }
        1 => {
            s[1].extend = true;
            s[5].extend = false;
            retrigger = Some(true);
        }
        6 => {
            s[6].extend = true;
            retrigger = Some(true);
        }
        5 => {
            s[1].extend = false;
            s[3].extend = false;
            s[5].extend = false;
            s[6].extend = true;
            retrigger = Some(true);
        }
        9 => {
            set(&mut s, 0, 0.0);
            s[4].extend = false;
            s[6].deform = 256;
        }
        10 | 11 => s[4].extend = false,
        3 => {
            s[4].deform = 1;
            s[5].extend = false;
        }
        4 => {
            for v in s.iter_mut() {
                v.v = 0.0;
            }
            retrigger = Some(false);
        }
        _ => {}
    }
    (s, retrigger)
}

