//! What picking an oscillator or effect type applies (the reference's menu
//! "Init" snapshots, interop facts from the spec): values and flags over
//! the type's slot defaults. Used for interactive type changes only; a
//! patch load never applies them.

use crate::kit::core::SlotValue;
use crate::kit::{fxs, oscs};

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

/// The 12 slots an effect of type `ty` starts with when picked (`send`:
/// a send slot, whose final mix is 1).
pub fn fx(ty: u8, send: bool) -> [SlotValue; 12] {
    let mut p = fxs::default_values(ty);
    let vals: &[f32] = match ty {
        6 => &[0.0, -27.251785, 2.0, 0.0, 6.0, 2.0, 0.0, 54.964317, 2.223215],
        19 => &[0.5, 0.5, 0.5, 0.5, 0.5],
        16 => &[0.0; 11],
        17 => &[-18.6305, 0.75, 0.793701, 8.03822, 0.75, 0.793701, 35.9013, 0.75, 0.793701, 1.0, 0.0, 1.0],
        28 => &[0.0, -6.0, 0.25, 0.0, 0.0, 0.5, 0.5, 0.0, 0.25, 0.0, 1.0],
        5 => &[0.0, 3.81786, 1.944644, 38.421448, 0.0, 0.0, 0.0, 3.0, 2.03125, 30.535736],
        15 => &[0.0, 0.5, 0.5, 1.0, 0.0, 70.0, 0.5, 0.0, -2.0, 0.0, 0.0, 0.0],
        23 => &[0.85, 0.5, 0.5, 0.0, 30.0, 1.0, 0.1, 0.1, 0.0, 0.0, 0.0, 1.0],
        25 => &[-60.0, 70.0, 0.0, 0.0, 0.0, -60.0, 70.0, 0.0, 1.0],
        21 => &[0.0, -9.0, 0.25, -0.25, 0.75, 0.0, 0.793701, 0.793701, 0.793701, 0.25, -0.25, 1.0],
        7 => &[0.0, 1.0, -8.0, 0.0, 1.0],
        13 => &[0.0, 60.0, 0.2, 1.0, 0.3, 0.7, -60.0, 70.0, 1.0],
        24 => &[-24.0, 0.5, -60.0, 70.0, 0.0, 0.5, 1.0, 1.0],
        10 => &[0.0, -96.0, 0.0, 0.0, 0.0, 20.0, -15.4741, 49.0958, 0.0, 0.0, 0.0, 1.0],
        9 => &[-7.885717, -2.85, 0.669643, 0.0, -23.625008, 65.6625, 0.5],
        20 => &[45.2326448, -2.47393, 1.0, 2.46467, 0.6, 2.0, 40.0, 0.0, 0.0, 1.0, 1.0, 45.2326448],
        12 => &[1.0, 0.0, -2.0, 0.75, 3.0, 60.0, 12.0, 0.25, 0.2, 0.0, 1.0],
        3 => &[0.0, 0.5, 0.0, -2.0, 1.0, 0.5, 0.5, 0.0, 8.0, 1.0],
        4 => &[0.0, 0.0, 0.0, 0.7, 0.0, 0.0, 0.0, 1.0],
        31 => &[0.0, 1.0, 0.0, 0.0, 3.0, 0.0, -60.0, 70.0, 0.5],
        1 => &[-1.0, -1.0, 0.0, 0.0, -14.105346, 48.289276, -1.179463, 0.0, 0.0, 0.0, 0.25],
        30 => &[-1.73699, 1.0, 0.5, 20.0, 0.5, 0.0, 0.0, 0.0, 0.0, -60.0, 0.5],
        2 => &[-4.0, 2.0, 0.5, 1.0, 0.5, 3.0, 3.0, 0.0, 3.0, 0.5],
        11 => &[-3.0, 0.0, 1.0, 0.5, 0.5, 0.5, 0.5, 0.5, 0.0, 0.5],
        27 => &[0.5, 0.5, 1.0, 0.5, 0.5, 0.0, 0.0, 0.5],
        14 => &[46.0, 1.0, 0.0, 1.0],
        29 => &[0.0, 0.0, -96.0, 0.0, 0.0, 0.0, 0.0, 0.0, -96.0, 1.0, 1.0],
        8 => &[0.0, 0.0, 1.0, 0.0, -6.0, 0.0, 0.0, 0.0, -60.0],
        26 => &[0.0, -60.0, 0.0, -6.63049, 70.0, -60.0, 0.0, 50.2131, 70.0, 0.0, 0.0, 0.0],
        22 => &[0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5],
        18 => &[-24.0, 10.0, 0.0, 1.0],
        _ => &[],
    };
    for (k, v) in vals.iter().enumerate() {
        p[k].v = *v;
    }
    // The final mix slot of the types with a send variant.
    let mix = match ty {
        9 => Some(6),
        3 => Some(6),
        31 => Some(8),
        1 => Some(10),
        30 => Some(10),
        2 => Some(9),
        11 => Some(9),
        27 => Some(7),
        _ => None,
    };
    if let (Some(m), true) = (mix, send) {
        p[m].v = 1.0;
    }
    let deact: &[usize] = match ty {
        25 => &[0, 1],
        31 => &[3, 4, 6, 7],
        8 => &[8],
        26 => &[1, 2, 4, 5, 6, 8],
        _ => &[],
    };
    for k in deact {
        p[*k].deactivated = true;
    }
    if ty == 1 {
        p[0].temposync = true;
        p[1].temposync = true;
    }
    if ty == 20 {
        p[11].deactivated = false;
    }
    p
}
