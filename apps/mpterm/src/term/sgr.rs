//! SGR (Select Graphic Rendition, `CSI ... m`) attribute parsing.
//!
//! Port of ghostty `src/terminal/sgr.zig`. The parser iterates a CSI's
//! params + separator bitset and yields `Attribute` values, handling:
//! - empty params = Unset (reset)
//! - 4 / 21 / 24 and colon form `4:0..4:5` (underline styles)
//! - 38/48/58 extended colors in ALL accepted forms:
//!   `38;5;n`, `38:5:n`, `38;2;r;g;b`, `38:2:r:g:b`,
//!   `38:2::r:g:b` (colorspace-id form) — and the mixed-separator rejects
//!   ghostty applies (a colon-introduced sequence consumes its subparams;
//!   malformed extended colors consume what they can and yield Unknown).
//! - 22 resets bold+faint; 39/49/59 reset fg/bg/underline color
//! - 30-37/40-47/90-97/100-107 basic + bright colors
//! - 53/55 overline on/off
//!
//! LANE CONTRACT: keep the public types; port ghostty's `Parser.next`
//! semantics and its test suite (sgr.zig has extensive tests — port them).

use crate::term::color::Rgb;
use crate::term::parser::Csi;
use crate::term::style::Underline;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attribute {
    /// SGR 0: reset all.
    Unset,
    Bold,
    ResetBold, // 22 (also resets faint)
    Faint,
    Italic,
    ResetItalic,
    Underline(Underline),
    ResetUnderline,
    UnderlineColorRgb(Rgb),
    UnderlineColorPalette(u8),
    ResetUnderlineColor,
    Overline,
    ResetOverline,
    Blink,
    ResetBlink,
    Inverse,
    ResetInverse,
    Invisible,
    ResetInvisible,
    Strikethrough,
    ResetStrikethrough,
    /// 30-37 (value 0..=7)
    Fg8(u8),
    /// 90-97 (value 8..=15)
    FgBright(u8),
    ResetFg,
    /// 40-47 (value 0..=7)
    Bg8(u8),
    /// 100-107 (value 8..=15)
    BgBright(u8),
    ResetBg,
    Fg256(u8),
    Bg256(u8),
    FgRgb(Rgb),
    BgRgb(Rgb),
    /// Unrecognized parameter — ignored by the terminal.
    Unknown(u16),
}

/// Result of an extended-color introducer (38/48/58) — the payload is the
/// same for fg/bg/underline, only the destination differs.
#[derive(Clone, Copy)]
enum ExtColor {
    Rgb(Rgb),
    Palette(u8),
}

/// Iterate the attributes of an SGR CSI.
pub struct SgrIter<'a> {
    pub csi: &'a Csi,
    pub idx: usize,
}

pub fn attributes(csi: &Csi) -> SgrIter<'_> {
    SgrIter { csi, idx: 0 }
}

impl<'a> SgrIter<'a> {
    /// True when the separator that followed `params[idx]` was a colon.
    /// Always false past the end of the params (and past the bitset width).
    #[inline]
    fn is_colon_at(&self, idx: usize) -> bool {
        idx < 32 && self.csi.sep_is_colon(idx)
    }

    /// ghostty `countColon`: how many consecutive colon separators start at
    /// the current index. The final param can never have a separator after
    /// it, so the scan stops one short of the end.
    fn count_colon(&self) -> usize {
        let len = self.csi.params_len;
        let mut idx = self.idx;
        let mut count = 0;
        while idx + 1 < len && self.is_colon_at(idx) {
            idx += 1;
            count += 1;
        }
        count
    }

    /// ghostty `consumeUnknownColon`: skip the whole colon-joined group so
    /// the params that follow it are not misread as top-level attributes.
    fn consume_unknown_colon(&mut self) {
        self.idx += self.count_colon() + 1;
    }

    /// ghostty `parseDirectColor`. `slice[1]` is always 2 here.
    ///
    /// Semicolon form is exactly `38;2;r;g;b`. Colon form accepts both
    /// `38:2:r:g:b` (3 colons after the introducer) and `38:2::r:g:b`
    /// (4 colons — the optional colorspace id, which is ignored). Anything
    /// else consumes the group and yields Unknown.
    fn parse_direct_color(&mut self, slice: &[u16], colon: bool) -> Option<Rgb> {
        // Any direct color style must have at least 5 values.
        if slice.len() < 5 {
            return None;
        }
        debug_assert_eq!(slice[1], 2);

        // Note: values are truncated to u8 rather than clamped/rejected —
        // out-of-range direct colors are undefined behavior, ghostty
        // `@truncate`s and so do we.
        if !colon {
            self.idx += 4;
            return Some(Rgb::new(slice[2] as u8, slice[3] as u8, slice[4] as u8));
        }

        match self.count_colon() {
            3 => {
                self.idx += 4;
                Some(Rgb::new(slice[2] as u8, slice[3] as u8, slice[4] as u8))
            }
            // A colorspace id sits at slice[2]; the color starts one later.
            // The length check is unreachable (4 colons imply 6 params) but
            // keeps the indexing panic-free.
            4 if slice.len() >= 6 => {
                self.idx += 5;
                Some(Rgb::new(slice[3] as u8, slice[4] as u8, slice[5] as u8))
            }
            _ => {
                self.consume_unknown_colon();
                None
            }
        }
    }

    /// The shared body of 38 / 48 / 58. `None` means "yield Unknown".
    fn parse_extended(&mut self, slice: &[u16], colon: bool) -> Option<ExtColor> {
        if slice.len() < 2 {
            return None;
        }
        match slice[1] {
            // `2` indicates direct-color (r, g, b).
            2 => self.parse_direct_color(slice, colon).map(ExtColor::Rgb),

            // `5` indicates indexed color. Note ghostty does not enforce
            // separator consistency here: `38;5;n`, `38:5:n` and the mixed
            // forms are all accepted.
            5 => {
                if slice.len() >= 3 {
                    self.idx += 2;
                    Some(ExtColor::Palette(slice[2] as u8))
                } else {
                    None
                }
            }

            _ => None,
        }
    }
}

impl<'a> Iterator for SgrIter<'a> {
    type Item = Attribute;

    fn next(&mut self) -> Option<Attribute> {
        // Copy the reference out so the params borrow is independent of
        // `self` and we can keep mutating `self.idx` below.
        let csi: &'a Csi = self.csi;
        let params = csi.params();

        if self.idx >= params.len() {
            // Add one to ensure we don't loop on unset. An empty param list
            // implicitly means unset; anything else means we're done.
            let first = self.idx == 0;
            self.idx += 1;
            return if first { Some(Attribute::Unset) } else { None };
        }

        let slice = &params[self.idx..];
        let colon = self.is_colon_at(self.idx);
        self.idx += 1;

        // If we have a colon separator then we need to ensure we're parsing
        // a value that allows it.
        if colon {
            match slice[0] {
                4 | 38 | 48 | 58 => {}
                _ => {
                    // Consume all the colon separated values and return
                    // them as unknown.
                    while self.is_colon_at(self.idx) {
                        self.idx += 1;
                    }
                    self.idx += 1;
                    return Some(Attribute::Unknown(slice[0]));
                }
            }
        }

        // `None` from any arm falls through to Unknown.
        let attr = match slice[0] {
            0 => Some(Attribute::Unset),

            1 => Some(Attribute::Bold),

            2 => Some(Attribute::Faint),

            3 => Some(Attribute::Italic),

            4 => {
                if !colon {
                    Some(Attribute::Underline(Underline::Single))
                } else if slice.len() < 2 {
                    // A trailing colon with no following sub-param (e.g.
                    // "ESC[58:4:m") leaves the colon separator bit set on
                    // the last param without adding another entry, so we can
                    // see param 4 with a colon but nothing after it.
                    None
                } else if self.is_colon_at(self.idx) {
                    // More subparams than an underline style can have.
                    self.consume_unknown_colon();
                    None
                } else {
                    self.idx += 1;
                    // For unknown underline styles, just render a single
                    // underline. `4:0` is the same as 24.
                    let style = Underline::from_param(slice[1]).unwrap_or(Underline::Single);
                    Some(match style {
                        Underline::None => Attribute::ResetUnderline,
                        style => Attribute::Underline(style),
                    })
                }
            }

            5 | 6 => Some(Attribute::Blink),

            7 => Some(Attribute::Inverse),

            8 => Some(Attribute::Invisible),

            9 => Some(Attribute::Strikethrough),

            21 => Some(Attribute::Underline(Underline::Double)),

            22 => Some(Attribute::ResetBold),

            23 => Some(Attribute::ResetItalic),

            24 => Some(Attribute::ResetUnderline),

            25 => Some(Attribute::ResetBlink),

            27 => Some(Attribute::ResetInverse),

            28 => Some(Attribute::ResetInvisible),

            29 => Some(Attribute::ResetStrikethrough),

            30..=37 => Some(Attribute::Fg8((slice[0] - 30) as u8)),

            38 => self.parse_extended(slice, colon).map(|c| match c {
                ExtColor::Rgb(rgb) => Attribute::FgRgb(rgb),
                ExtColor::Palette(i) => Attribute::Fg256(i),
            }),

            39 => Some(Attribute::ResetFg),

            40..=47 => Some(Attribute::Bg8((slice[0] - 40) as u8)),

            48 => self.parse_extended(slice, colon).map(|c| match c {
                ExtColor::Rgb(rgb) => Attribute::BgRgb(rgb),
                ExtColor::Palette(i) => Attribute::Bg256(i),
            }),

            49 => Some(Attribute::ResetBg),

            53 => Some(Attribute::Overline),

            55 => Some(Attribute::ResetOverline),

            58 => self.parse_extended(slice, colon).map(|c| match c {
                ExtColor::Rgb(rgb) => Attribute::UnderlineColorRgb(rgb),
                ExtColor::Palette(i) => Attribute::UnderlineColorPalette(i),
            }),

            59 => Some(Attribute::ResetUnderlineColor),

            // 82 instead of 90 to offset to "bright" colors
            90..=97 => Some(Attribute::FgBright((slice[0] - 82) as u8)),

            100..=107 => Some(Attribute::BgBright((slice[0] - 92) as u8)),

            _ => None,
        };

        Some(attr.unwrap_or(Attribute::Unknown(slice[0])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an SGR CSI with no colon separators.
    fn csi(params: &[u16]) -> Csi {
        csi_sep(params, &[])
    }

    /// Build an SGR CSI, setting a colon separator after each listed index.
    fn csi_sep(params: &[u16], colons: &[usize]) -> Csi {
        let mut c = Csi {
            final_byte: b'm',
            params_len: params.len(),
            ..Csi::default()
        };
        c.params[..params.len()].copy_from_slice(params);
        for &i in colons {
            c.params_sep |= 1 << i;
        }
        c
    }

    /// ghostty `testParseColon`: mark every param except the last as having
    /// a colon after it.
    fn csi_colon(params: &[u16]) -> Csi {
        let colons: Vec<usize> = (0..params.len().saturating_sub(1)).collect();
        csi_sep(params, &colons)
    }

    fn parse_one(params: &[u16]) -> Attribute {
        let c = csi(params);
        attributes(&c).next().unwrap()
    }

    fn parse_one_colon(params: &[u16]) -> Attribute {
        let c = csi_colon(params);
        attributes(&c).next().unwrap()
    }

    fn parse_all(c: &Csi) -> Vec<Attribute> {
        attributes(c).collect()
    }

    #[test]
    fn parser() {
        assert_eq!(parse_one(&[]), Attribute::Unset);
        assert_eq!(parse_one(&[0]), Attribute::Unset);

        assert_eq!(
            parse_one(&[38, 2, 40, 44, 52]),
            Attribute::FgRgb(Rgb::new(40, 44, 52))
        );
        assert_eq!(parse_one(&[38, 2, 44, 52]), Attribute::Unknown(38));

        assert_eq!(
            parse_one(&[48, 2, 40, 44, 52]),
            Attribute::BgRgb(Rgb::new(40, 44, 52))
        );
        assert_eq!(parse_one(&[48, 2, 44, 52]), Attribute::Unknown(48));
    }

    #[test]
    fn parser_multiple() {
        let c = csi(&[0, 38, 2, 40, 44, 52]);
        let mut it = attributes(&c);
        assert_eq!(it.next(), Some(Attribute::Unset));
        assert_eq!(it.next(), Some(Attribute::FgRgb(Rgb::new(40, 44, 52))));
        assert_eq!(it.next(), None);
        assert_eq!(it.next(), None);
    }

    #[test]
    fn unsupported_with_colon() {
        let c = csi_sep(&[0, 4, 1], &[0]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::Unknown(0), Attribute::Bold]
        );
    }

    #[test]
    fn unsupported_with_multiple_colon() {
        let c = csi_sep(&[0, 4, 2, 1], &[0, 1]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::Unknown(0), Attribute::Bold]
        );
    }

    #[test]
    fn bold() {
        assert_eq!(parse_one(&[1]), Attribute::Bold);
        assert_eq!(parse_one(&[22]), Attribute::ResetBold);
    }

    #[test]
    fn faint() {
        assert_eq!(parse_one(&[2]), Attribute::Faint);
        // 22 resets both bold and faint.
        assert_eq!(parse_one(&[22]), Attribute::ResetBold);
    }

    #[test]
    fn italic() {
        assert_eq!(parse_one(&[3]), Attribute::Italic);
        assert_eq!(parse_one(&[23]), Attribute::ResetItalic);
    }

    #[test]
    fn underline() {
        assert_eq!(parse_one(&[4]), Attribute::Underline(Underline::Single));
        assert_eq!(parse_one(&[24]), Attribute::ResetUnderline);
        assert_eq!(parse_one(&[21]), Attribute::Underline(Underline::Double));
    }

    #[test]
    fn underline_styles() {
        assert_eq!(parse_one_colon(&[4, 2]), Attribute::Underline(Underline::Double));
        // 4:0 aliases 24.
        assert_eq!(parse_one_colon(&[4, 0]), Attribute::ResetUnderline);
        assert_eq!(parse_one_colon(&[4, 1]), Attribute::Underline(Underline::Single));
        assert_eq!(parse_one_colon(&[4, 3]), Attribute::Underline(Underline::Curly));
        assert_eq!(parse_one_colon(&[4, 4]), Attribute::Underline(Underline::Dotted));
        assert_eq!(parse_one_colon(&[4, 5]), Attribute::Underline(Underline::Dashed));
        // Unknown styles render as a single underline.
        assert_eq!(parse_one_colon(&[4, 9]), Attribute::Underline(Underline::Single));
    }

    #[test]
    fn underline_style_with_more() {
        let c = csi_sep(&[4, 2, 1], &[0]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::Underline(Underline::Double), Attribute::Bold]
        );
    }

    #[test]
    fn underline_style_with_too_many_colons() {
        let c = csi_sep(&[4, 2, 3, 1], &[0, 1]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::Unknown(4), Attribute::Bold]
        );
    }

    #[test]
    fn semicolon_underline_is_not_a_style() {
        // `4;3` must not become curly.
        let c = csi(&[4, 3]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::Underline(Underline::Single), Attribute::Italic]
        );
    }

    #[test]
    fn blink() {
        assert_eq!(parse_one(&[5]), Attribute::Blink);
        assert_eq!(parse_one(&[6]), Attribute::Blink);
        assert_eq!(parse_one(&[25]), Attribute::ResetBlink);
    }

    #[test]
    fn inverse() {
        assert_eq!(parse_one(&[7]), Attribute::Inverse);
        assert_eq!(parse_one(&[27]), Attribute::ResetInverse);
    }

    #[test]
    fn strikethrough() {
        assert_eq!(parse_one(&[9]), Attribute::Strikethrough);
        assert_eq!(parse_one(&[29]), Attribute::ResetStrikethrough);
    }

    #[test]
    fn overline() {
        assert_eq!(parse_one(&[53]), Attribute::Overline);
        assert_eq!(parse_one(&[55]), Attribute::ResetOverline);
    }

    #[test]
    fn color_8() {
        let c = csi(&[31, 43, 90, 103]);
        assert_eq!(
            parse_all(&c),
            vec![
                Attribute::Fg8(1),          // red
                Attribute::Bg8(3),          // yellow
                Attribute::FgBright(8),     // bright black
                Attribute::BgBright(11),    // bright yellow
            ]
        );
    }

    #[test]
    fn color_8_edges() {
        let c = csi(&[30, 37, 40, 47, 97, 100, 107, 39, 49]);
        assert_eq!(
            parse_all(&c),
            vec![
                Attribute::Fg8(0),
                Attribute::Fg8(7),
                Attribute::Bg8(0),
                Attribute::Bg8(7),
                Attribute::FgBright(15),
                Attribute::BgBright(8),
                Attribute::BgBright(15),
                Attribute::ResetFg,
                Attribute::ResetBg,
            ]
        );
    }

    #[test]
    fn color_256() {
        let c = csi(&[38, 5, 161, 48, 5, 236]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::Fg256(161), Attribute::Bg256(236)]
        );
    }

    #[test]
    fn color_256_colon() {
        let c = csi_colon(&[38, 5, 161]);
        assert_eq!(parse_all(&c), vec![Attribute::Fg256(161)]);
    }

    #[test]
    fn color_256_underline() {
        let c = csi(&[58, 5, 9]);
        assert_eq!(parse_all(&c), vec![Attribute::UnderlineColorPalette(9)]);
    }

    #[test]
    fn bg_color_24bit() {
        assert_eq!(
            parse_one_colon(&[48, 2, 1, 2, 3]),
            Attribute::BgRgb(Rgb::new(1, 2, 3))
        );
    }

    #[test]
    fn underline_color() {
        assert_eq!(
            parse_one_colon(&[58, 2, 1, 2, 3]),
            Attribute::UnderlineColorRgb(Rgb::new(1, 2, 3))
        );
        // With the optional colorspace id.
        assert_eq!(
            parse_one_colon(&[58, 2, 0, 1, 2, 3]),
            Attribute::UnderlineColorRgb(Rgb::new(1, 2, 3))
        );
    }

    #[test]
    fn reset_underline_color() {
        assert_eq!(parse_one(&[59]), Attribute::ResetUnderlineColor);
    }

    #[test]
    fn invisible() {
        let c = csi(&[8, 28]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::Invisible, Attribute::ResetInvisible]
        );
    }

    #[test]
    fn underline_bg_and_fg() {
        let c = csi(&[4, 38, 2, 255, 247, 219, 48, 2, 242, 93, 147, 4]);
        assert_eq!(
            parse_all(&c),
            vec![
                Attribute::Underline(Underline::Single),
                Attribute::FgRgb(Rgb::new(255, 247, 219)),
                Attribute::BgRgb(Rgb::new(242, 93, 147)),
                Attribute::Underline(Underline::Single),
            ]
        );
    }

    #[test]
    fn direct_color_fg_missing_color() {
        // This used to crash (and must terminate).
        let c = csi(&[38, 5]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::Unknown(38), Attribute::Blink]
        );
    }

    #[test]
    fn direct_color_bg_missing_color() {
        // This used to crash (and must terminate).
        let c = csi(&[48, 5]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::Unknown(48), Attribute::Blink]
        );
    }

    #[test]
    fn direct_ignore_optional_color_space() {
        // These behaviors have been verified against xterm.

        // Colon version should skip the optional color space identifier.
        // 3 8 : 2 : Pi : Pr : Pg : Pb
        assert_eq!(
            parse_one_colon(&[38, 2, 0, 1, 2, 3]),
            Attribute::FgRgb(Rgb::new(1, 2, 3))
        );
        // 4 8 : 2 : Pi : Pr : Pg : Pb
        assert_eq!(
            parse_one_colon(&[48, 2, 0, 1, 2, 3]),
            Attribute::BgRgb(Rgb::new(1, 2, 3))
        );
        // 5 8 : 2 : Pi : Pr : Pg : Pb
        assert_eq!(
            parse_one_colon(&[58, 2, 0, 1, 2, 3]),
            Attribute::UnderlineColorRgb(Rgb::new(1, 2, 3))
        );

        // Semicolon version should not parse the optional color space id.
        // 3 8 ; 2 ; Pr ; Pg ; Pb
        assert_eq!(
            parse_one(&[38, 2, 0, 1, 2, 3]),
            Attribute::FgRgb(Rgb::new(0, 1, 2))
        );
        // 4 8 ; 2 ; Pr ; Pg ; Pb
        assert_eq!(
            parse_one(&[48, 2, 0, 1, 2, 3]),
            Attribute::BgRgb(Rgb::new(0, 1, 2))
        );
        // 5 8 ; 2 ; Pr ; Pg ; Pb
        assert_eq!(
            parse_one(&[58, 2, 0, 1, 2, 3]),
            Attribute::UnderlineColorRgb(Rgb::new(0, 1, 2))
        );
    }

    #[test]
    fn direct_fg_colon_with_too_many_colons() {
        let c = csi_sep(&[38, 2, 0, 1, 2, 3, 4, 1], &[0, 1, 2, 3, 4, 5]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::Unknown(38), Attribute::Bold]
        );
    }

    #[test]
    fn direct_fg_colon_with_colorspace_and_extra_param() {
        let c = csi_sep(&[38, 2, 0, 1, 2, 3, 1], &[0, 1, 2, 3, 4]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::FgRgb(Rgb::new(1, 2, 3)), Attribute::Bold]
        );
    }

    #[test]
    fn direct_fg_colon_no_colorspace_and_extra_param() {
        let c = csi_sep(&[38, 2, 1, 2, 3, 1], &[0, 1, 2, 3]);
        assert_eq!(
            parse_all(&c),
            vec![Attribute::FgRgb(Rgb::new(1, 2, 3)), Attribute::Bold]
        );
    }

    // Kakoune sent this complex SGR sequence that caused invalid behavior.
    #[test]
    fn kakoune_input() {
        // This used to crash.
        let c = csi_sep(
            &[0, 4, 3, 38, 2, 175, 175, 215, 58, 2, 0, 190, 80, 70],
            &[1, 8, 9, 10, 11, 12],
        );
        assert_eq!(
            parse_all(&c),
            vec![
                Attribute::Unset,
                Attribute::Underline(Underline::Curly),
                Attribute::FgRgb(Rgb::new(175, 175, 215)),
                Attribute::UnderlineColorRgb(Rgb::new(190, 80, 70)),
            ]
        );
    }

    // Discussion #5930, another input sent by kakoune:
    // echo -e "\033[4:3;38;2;51;51;51;48;2;170;170;170;58;2;255;97;136m"
    #[test]
    fn kakoune_input_underline_fg_and_bg() {
        // This used to crash.
        let c = csi_sep(
            &[4, 3, 38, 2, 51, 51, 51, 48, 2, 170, 170, 170, 58, 2, 255, 97, 136],
            &[0],
        );
        assert_eq!(
            parse_all(&c),
            vec![
                Attribute::Underline(Underline::Curly),
                Attribute::FgRgb(Rgb::new(51, 51, 51)),
                Attribute::BgRgb(Rgb::new(170, 170, 170)),
                Attribute::UnderlineColorRgb(Rgb::new(255, 97, 136)),
            ]
        );
    }

    // Fuzz crash: afl-out/stream/default/crashes/id:000021
    // Input "ESC [ 5 8 : 4 : m" produces params [58, 4] with colon
    // separator bits set at indices 0 and 1. The trailing colon causes the
    // second iteration to see param 4 (underline) with a colon and a slice
    // of length 1.
    #[test]
    fn underline_colon_with_trailing_separator_and_short_slice() {
        let c = csi_sep(&[58, 4], &[0, 1]);
        assert_eq!(
            parse_all(&c),
            vec![
                // 58:4 is not a valid underline color (sub-param 4 is not
                // 2 or 5), so it falls through as unknown.
                Attribute::Unknown(58),
                // Param 4 with a trailing colon but no sub-param is
                // malformed, so it also falls through as unknown.
                Attribute::Unknown(4),
            ]
        );
    }

    #[test]
    fn unknown_params() {
        let c = csi(&[1, 200, 4]);
        assert_eq!(
            parse_all(&c),
            vec![
                Attribute::Bold,
                Attribute::Unknown(200),
                Attribute::Underline(Underline::Single),
            ]
        );
    }

    #[test]
    fn direct_color_truncates_out_of_range() {
        // ghostty @truncate's rather than clamping or rejecting.
        assert_eq!(
            parse_one(&[38, 2, 256, 511, 300]),
            Attribute::FgRgb(Rgb::new(0, 255, 44))
        );
        assert_eq!(parse_one(&[38, 5, 300]), Attribute::Fg256(44));
    }
}
