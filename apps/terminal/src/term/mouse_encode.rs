//! Mouse reporting -> PTY byte encoding.
//!
//! Port of ghostty `src/input/mouse_encode.zig`: X10/normal/button/any-event
//! tracking in the X10, UTF-8 (1005), SGR (1006), urxvt (1015) and
//! SGR-pixels (1016) formats, with motion dedup left to the caller.
//!
//! LANE CONTRACT: keep the public surface; port ghostty's encoding rules
//! and tests (button number mapping incl. wheel 64/65, extra buttons 128+,
//! modifier bits shift=4 meta=8 ctrl=16, motion flag 32, release encoding
//! 'm' vs 'M' in SGR and button=3 in legacy, coordinate clamping 223 for
//! X10, 2015 for UTF-8).
//!
//! Port notes: ghostty's `encode()` also owns surface-pixel -> cell
//! conversion, the "outside the viewport" rules and motion dedup against the
//! last reported cell. Those all need renderer state (cell size, padding,
//! screen size, button-held tracking), so in this port they stay with the
//! caller and `encode_mouse` takes the already-resolved cell and pixel
//! position. Everything from `shouldReport`/`buttonCode` down is ported
//! as-is.

use crate::term::key_encode::KeyMods;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    Button8,
    Button9,
    Button10,
    Button11,
    /// Motion with no button pressed.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    Press,
    Release,
    Motion,
}

/// Which tracking mode is active (from modes 9/1000/1002/1003).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseTracking {
    None,
    X10,
    Normal,
    Button,
    Any,
}

/// Which encoding format is active (from modes 1005/1006/1015/1016;
/// SGR-pixels wins over SGR wins over urxvt wins over UTF-8 wins over X10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseFormat {
    X10,
    Utf8,
    Sgr,
    Urxvt,
    SgrPixels,
}

#[derive(Clone, Copy, Debug)]
pub struct MouseReport {
    pub kind: MouseEventKind,
    pub button: MouseButton,
    pub mods: KeyMods,
    /// 0-based cell position.
    pub col: u32,
    pub row: u32,
    /// Pixel position within the grid, for SGR-pixels.
    pub x_px: u32,
    pub y_px: u32,
}

/// Encode one mouse report. Returns None when this event should not be
/// reported under `tracking` (e.g. motion under Normal, release under X10).
pub fn encode_mouse(
    report: &MouseReport,
    tracking: MouseTracking,
    format: MouseFormat,
) -> Option<Vec<u8>> {
    if !should_report(report, tracking) {
        return None;
    }

    let button_code = button_code(report, tracking, format)?;

    match format {
        MouseFormat::X10 => {
            // The single-byte form tops out at 32 + 223 = 255.
            if report.col > 222 || report.row > 222 {
                return None;
            }

            // +1 because our x/y are zero-indexed and the protocol is
            // one-indexed.
            let mut out = Vec::with_capacity(6);
            out.extend_from_slice(b"\x1b[M");
            out.push(32 + button_code);
            out.push(32 + report.col as u8 + 1);
            out.push(32 + report.row as u8 + 1);
            Some(out)
        }

        MouseFormat::Utf8 => {
            let mut out = Vec::with_capacity(9);
            out.extend_from_slice(b"\x1b[M");

            // The button code always fits in a single byte.
            out.push(32 + button_code);

            // Coordinates are UTF-8 encoded, which tops out at U+10FFFF.
            let x_cp = char::from_u32(report.col + 33)?;
            let y_cp = char::from_u32(report.row + 33)?;
            let mut buf = [0u8; 4];
            out.extend_from_slice(x_cp.encode_utf8(&mut buf).as_bytes());
            out.extend_from_slice(y_cp.encode_utf8(&mut buf).as_bytes());
            Some(out)
        }

        MouseFormat::Sgr => Some(
            format!(
                "\x1b[<{};{};{}{}",
                button_code,
                report.col + 1,
                report.row + 1,
                final_byte(report),
            )
            .into_bytes(),
        ),

        MouseFormat::Urxvt => Some(
            format!(
                "\x1b[{};{};{}M",
                32 + button_code as u32,
                report.col + 1,
                report.row + 1,
            )
            .into_bytes(),
        ),

        MouseFormat::SgrPixels => Some(
            format!(
                "\x1b[<{};{};{}{}",
                button_code,
                report.x_px,
                report.y_px,
                final_byte(report),
            )
            .into_bytes(),
        ),
    }
}

fn final_byte(report: &MouseReport) -> char {
    if report.kind == MouseEventKind::Release {
        'm'
    } else {
        'M'
    }
}

/// True if this event should be reported for the given tracking mode.
fn should_report(report: &MouseReport, tracking: MouseTracking) -> bool {
    match tracking {
        MouseTracking::None => false,

        // X10 only reports button presses of left, middle and right.
        MouseTracking::X10 => {
            report.kind == MouseEventKind::Press
                && matches!(
                    report.button,
                    MouseButton::Left | MouseButton::Middle | MouseButton::Right
                )
        }

        // Normal mode does not report motion.
        MouseTracking::Normal => report.kind != MouseEventKind::Motion,

        // Button mode requires an active button, including for motion.
        MouseTracking::Button => report.button != MouseButton::None,

        // Any mode reports everything.
        MouseTracking::Any => true,
    }
}

fn button_code(
    report: &MouseReport,
    tracking: MouseTracking,
    format: MouseFormat,
) -> Option<u8> {
    let mut acc: u8 = if report.button == MouseButton::None {
        // No button means motion with no pressed button.
        3
    } else if report.kind == MouseEventKind::Release
        && format != MouseFormat::Sgr
        && format != MouseFormat::SgrPixels
    {
        // Legacy releases are always encoded as button 3.
        3
    } else {
        match report.button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::WheelUp => 64,
            MouseButton::WheelDown => 65,
            MouseButton::WheelLeft => 66,
            MouseButton::WheelRight => 67,
            MouseButton::Button8 => 128,
            MouseButton::Button9 => 129,
            // Buttons 10 and 11 are ambiguous in every mouse protocol, so
            // ghostty drops them.
            MouseButton::Button10 | MouseButton::Button11 => return None,
            MouseButton::None => unreachable!(),
        }
    };

    // X10 does not include modifiers.
    if tracking != MouseTracking::X10 {
        if report.mods.shift {
            acc += 4;
        }
        if report.mods.alt {
            acc += 8;
        }
        if report.mods.ctrl {
            acc += 16;
        }
    }

    // Motion adds another bit.
    if report.kind == MouseEventKind::Motion {
        acc += 32;
    }

    Some(acc)
}

// ---------------------------------------------------------------------------
// Tests (ported from ghostty `mouse_encode.zig`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn report(kind: MouseEventKind, button: MouseButton) -> MouseReport {
        MouseReport {
            kind,
            button,
            mods: KeyMods::default(),
            col: 0,
            row: 0,
            x_px: 0,
            y_px: 0,
        }
    }

    fn at(mut r: MouseReport, col: u32, row: u32) -> MouseReport {
        r.col = col;
        r.row = row;
        r
    }

    fn px(mut r: MouseReport, x: u32, y: u32) -> MouseReport {
        r.x_px = x;
        r.y_px = y;
        r
    }

    #[track_caller]
    fn expect(r: &MouseReport, tracking: MouseTracking, format: MouseFormat, want: &[u8]) {
        let got = encode_mouse(r, tracking, format).unwrap_or_default();
        assert_eq!(
            got.as_slice(),
            want,
            "got {:?}, want {:?}",
            String::from_utf8_lossy(&got),
            String::from_utf8_lossy(want)
        );
    }

    const ALL_KINDS: [MouseEventKind; 3] = [
        MouseEventKind::Press,
        MouseEventKind::Release,
        MouseEventKind::Motion,
    ];

    // -- should_report -------------------------------------------------------

    #[test]
    fn should_report_none_mode_never_reports() {
        for kind in ALL_KINDS {
            assert!(!should_report(
                &report(kind, MouseButton::Left),
                MouseTracking::None
            ));
        }
    }

    #[test]
    fn should_report_x10_reports_only_left_middle_right_press() {
        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            assert!(should_report(
                &report(MouseEventKind::Press, button),
                MouseTracking::X10
            ));
        }

        // Release is not reported.
        assert!(!should_report(
            &report(MouseEventKind::Release, MouseButton::Left),
            MouseTracking::X10
        ));

        // Motion is not reported.
        assert!(!should_report(
            &report(MouseEventKind::Motion, MouseButton::Left),
            MouseTracking::X10
        ));

        // Other buttons are not reported.
        assert!(!should_report(
            &report(MouseEventKind::Press, MouseButton::WheelUp),
            MouseTracking::X10
        ));

        // No button is not reported.
        assert!(!should_report(
            &report(MouseEventKind::Press, MouseButton::None),
            MouseTracking::X10
        ));
    }

    #[test]
    fn should_report_normal_reports_press_and_release_but_not_motion() {
        assert!(should_report(
            &report(MouseEventKind::Press, MouseButton::Left),
            MouseTracking::Normal
        ));
        assert!(should_report(
            &report(MouseEventKind::Release, MouseButton::Left),
            MouseTracking::Normal
        ));
        assert!(!should_report(
            &report(MouseEventKind::Motion, MouseButton::Left),
            MouseTracking::Normal
        ));
    }

    #[test]
    fn should_report_button_mode_requires_a_button() {
        for kind in ALL_KINDS {
            assert!(should_report(
                &report(kind, MouseButton::Left),
                MouseTracking::Button
            ));
            assert!(!should_report(
                &report(kind, MouseButton::None),
                MouseTracking::Button
            ));
        }
    }

    #[test]
    fn should_report_any_mode_reports_everything() {
        for kind in ALL_KINDS {
            assert!(should_report(
                &report(kind, MouseButton::Left),
                MouseTracking::Any
            ));
        }
        assert!(should_report(
            &report(MouseEventKind::Motion, MouseButton::None),
            MouseTracking::Any
        ));
    }

    // -- encoding ------------------------------------------------------------

    #[test]
    fn x10_press_left() {
        // X10 tracking never encodes modifiers, even when they are held.
        let mut r = report(MouseEventKind::Press, MouseButton::Left);
        r.mods = KeyMods {
            shift: true,
            alt: true,
            ctrl: true,
            ..Default::default()
        };
        expect(
            &r,
            MouseTracking::X10,
            MouseFormat::X10,
            &[0x1B, b'[', b'M', 32, 33, 33],
        );
    }

    #[test]
    fn x10_ignores_release() {
        assert_eq!(
            encode_mouse(
                &report(MouseEventKind::Release, MouseButton::Left),
                MouseTracking::X10,
                MouseFormat::X10
            ),
            None
        );
    }

    #[test]
    fn normal_ignores_motion() {
        assert_eq!(
            encode_mouse(
                &report(MouseEventKind::Motion, MouseButton::Left),
                MouseTracking::Normal,
                MouseFormat::Sgr
            ),
            None
        );
    }

    #[test]
    fn button_mode_requires_button() {
        assert_eq!(
            encode_mouse(
                &report(MouseEventKind::Motion, MouseButton::None),
                MouseTracking::Button,
                MouseFormat::Sgr
            ),
            None
        );
    }

    #[test]
    fn sgr_release_keeps_button_identity() {
        let r = at(report(MouseEventKind::Release, MouseButton::Right), 4, 5);
        expect(&r, MouseTracking::Any, MouseFormat::Sgr, b"\x1b[<2;5;6m");
    }

    #[test]
    fn sgr_motion_with_no_button() {
        let r = at(report(MouseEventKind::Motion, MouseButton::None), 1, 2);
        expect(&r, MouseTracking::Any, MouseFormat::Sgr, b"\x1b[<35;2;3M");
    }

    #[test]
    fn sgr_press_with_modifiers() {
        let mut r = at(report(MouseEventKind::Press, MouseButton::Left), 0, 0);
        r.mods.shift = true;
        expect(&r, MouseTracking::Any, MouseFormat::Sgr, b"\x1b[<4;1;1M");
        r.mods.alt = true;
        expect(&r, MouseTracking::Any, MouseFormat::Sgr, b"\x1b[<12;1;1M");
        r.mods.ctrl = true;
        expect(&r, MouseTracking::Any, MouseFormat::Sgr, b"\x1b[<28;1;1M");
    }

    #[test]
    fn sgr_drag_sets_the_motion_bit() {
        let r = at(report(MouseEventKind::Motion, MouseButton::Left), 9, 9);
        expect(
            &r,
            MouseTracking::Button,
            MouseFormat::Sgr,
            b"\x1b[<32;10;10M",
        );
    }

    #[test]
    fn urxvt_with_modifiers() {
        let mut r = at(report(MouseEventKind::Press, MouseButton::Left), 2, 3);
        r.mods = KeyMods {
            shift: true,
            alt: true,
            ctrl: true,
            ..Default::default()
        };
        expect(&r, MouseTracking::Any, MouseFormat::Urxvt, b"\x1b[60;3;4M");
    }

    #[test]
    fn urxvt_release_uses_legacy_button_3_encoding() {
        let r = at(report(MouseEventKind::Release, MouseButton::Right), 2, 3);
        expect(&r, MouseTracking::Any, MouseFormat::Urxvt, b"\x1b[35;3;4M");
    }

    #[test]
    fn x10_release_uses_legacy_button_3_encoding() {
        let r = at(report(MouseEventKind::Release, MouseButton::Right), 2, 3);
        expect(
            &r,
            MouseTracking::Normal,
            MouseFormat::X10,
            &[0x1B, b'[', b'M', 32 + 3, 32 + 3, 32 + 4],
        );
    }

    #[test]
    fn utf8_encodes_large_coordinates() {
        let r = at(report(MouseEventKind::Press, MouseButton::Left), 300, 400);
        let out = encode_mouse(&r, MouseTracking::Any, MouseFormat::Utf8).unwrap();
        assert_eq!(&out[0..4], &[0x1B, b'[', b'M', 32]);
        let tail = std::str::from_utf8(&out[4..]).unwrap();
        let cps: Vec<u32> = tail.chars().map(|c| c as u32).collect();
        assert_eq!(cps, vec![333, 433]);
    }

    #[test]
    fn x10_coordinate_limit() {
        let r = at(report(MouseEventKind::Press, MouseButton::Left), 223, 0);
        assert_eq!(
            encode_mouse(&r, MouseTracking::X10, MouseFormat::X10),
            None
        );
        // 222 is still fine (32 + 222 + 1 = 255).
        let r = at(report(MouseEventKind::Press, MouseButton::Left), 222, 222);
        expect(
            &r,
            MouseTracking::X10,
            MouseFormat::X10,
            &[0x1B, b'[', b'M', 32, 255, 255],
        );
    }

    #[test]
    fn sgr_wheel_button_mappings() {
        for (button, code) in [
            (MouseButton::WheelUp, 64),
            (MouseButton::WheelDown, 65),
            (MouseButton::WheelLeft, 66),
            (MouseButton::WheelRight, 67),
        ] {
            let r = report(MouseEventKind::Press, button);
            expect(
                &r,
                MouseTracking::Any,
                MouseFormat::Sgr,
                format!("\x1b[<{};1;1M", code).as_bytes(),
            );
        }
    }

    #[test]
    fn sgr_extra_button_mappings() {
        for (button, code) in [(MouseButton::Button8, 128), (MouseButton::Button9, 129)] {
            let r = report(MouseEventKind::Press, button);
            expect(
                &r,
                MouseTracking::Any,
                MouseFormat::Sgr,
                format!("\x1b[<{};1;1M", code).as_bytes(),
            );
        }
    }

    #[test]
    fn unsupported_button_is_ignored() {
        for button in [MouseButton::Button10, MouseButton::Button11] {
            let r = at(report(MouseEventKind::Press, button), 1, 1);
            assert_eq!(encode_mouse(&r, MouseTracking::Any, MouseFormat::Sgr), None);
        }
    }

    #[test]
    fn sgr_pixels_uses_pixel_coordinates() {
        let r = px(at(report(MouseEventKind::Press, MouseButton::Left), 1, 2), 10, 20);
        expect(
            &r,
            MouseTracking::Any,
            MouseFormat::SgrPixels,
            b"\x1b[<0;10;20M",
        );
    }

    #[test]
    fn sgr_pixels_release_keeps_button_identity() {
        let r = px(
            at(report(MouseEventKind::Release, MouseButton::Right), 1, 2),
            10,
            20,
        );
        expect(
            &r,
            MouseTracking::Any,
            MouseFormat::SgrPixels,
            b"\x1b[<2;10;20m",
        );
    }

    #[test]
    fn tracking_none_encodes_nothing() {
        for kind in ALL_KINDS {
            assert_eq!(
                encode_mouse(
                    &report(kind, MouseButton::Left),
                    MouseTracking::None,
                    MouseFormat::Sgr
                ),
                None
            );
        }
    }
}
