//! Display colors are sampled before launching a child, never imposed on peers.
use crate::term::color::{parse_color_spec, Rgb};
use std::{
    fmt::Write,
    io,
    time::{Duration, Instant},
};

pub const COLOR_COUNT: usize = 259;

#[derive(Clone)]
pub struct Theme {
    pub colors: [Option<Rgb>; COLOR_COUNT],
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            colors: [None; COLOR_COUNT],
        }
    }
}

impl Theme {
    pub fn encode(&self) -> String {
        self.colors
            .iter()
            .map(|color| match color {
                Some(rgb) => format!("{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b),
                None => "-".into(),
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn decode(value: &str) -> Result<Self, String> {
        let parts: Vec<_> = value.split(',').collect();
        if parts.len() != COLOR_COUNT {
            return Err("Invalid screen theme color count".into());
        }
        let mut theme = Self::default();
        for (index, value) in parts.into_iter().enumerate() {
            if value != "-" {
                if value.len() != 6 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("Invalid screen theme RGB".into());
                }
                theme.colors[index] = parse_color_spec(&format!("#{value}"));
            }
        }
        Ok(theme)
    }

    pub fn merge_missing(&mut self, captured: &Self) {
        for (color, captured) in self.colors.iter_mut().zip(captured.colors) {
            if color.is_none() {
                *color = captured;
            }
        }
    }
}

#[derive(Default)]
struct OscReader {
    pending: Vec<u8>,
    since: Option<Instant>,
}

impl OscReader {
    fn feed(&mut self, bytes: &[u8], mut osc: impl FnMut(&[u8]) -> bool) -> Vec<u8> {
        let mut other = Vec::new();
        for &byte in bytes {
            if self.pending.is_empty() {
                if byte == 0x1b {
                    self.pending.push(byte);
                    self.since = Some(Instant::now());
                } else {
                    other.push(byte);
                }
                continue;
            }
            if self.pending.len() == 1 && byte != b']' {
                other.append(&mut self.pending);
                if byte == 0x1b {
                    self.pending.push(byte);
                    self.since = Some(Instant::now());
                } else {
                    other.push(byte);
                    self.since = None;
                }
                continue;
            }
            self.pending.push(byte);
            let end = if byte == 7 {
                Some(self.pending.len() - 1)
            } else if self.pending.ends_with(b"\x1b\\") {
                Some(self.pending.len() - 2)
            } else {
                None
            };
            if let Some(end) = end {
                if !osc(&self.pending[2..end]) {
                    other.append(&mut self.pending);
                }
                self.pending.clear();
                self.since = None;
            } else if self.pending.len() >= 512 {
                other.append(&mut self.pending);
                self.since = None;
            }
        }
        other
    }

    fn expired(&mut self) -> Vec<u8> {
        if self
            .since
            .is_some_and(|since| since.elapsed() >= Duration::from_millis(25))
        {
            self.since = None;
            std::mem::take(&mut self.pending)
        } else {
            Vec::new()
        }
    }
}

fn color_command(bytes: &[u8]) -> Option<(usize, Option<Rgb>)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (code, rest) = text.split_once(';').unwrap_or((text, ""));
    let (index, spec) = match code {
        "4" => {
            let (index, spec) = rest.split_once(';')?;
            let index = index.parse::<usize>().ok().filter(|i| *i < 256)?;
            (index, spec)
        }
        "10" => (256, rest),
        "11" => (257, rest),
        "12" => (258, rest),
        "104" => (rest.parse::<usize>().ok().filter(|i| *i < 256)?, ""),
        "110" => (256, ""),
        "111" => (257, ""),
        "112" => (258, ""),
        _ => return None,
    };
    if spec.is_empty() {
        Some((index, None))
    } else {
        Some((index, Some(display_color(spec)?)))
    }
}

fn display_color(spec: &str) -> Option<Rgb> {
    if let Some((prefix, components)) = spec.split_once(':') {
        if prefix.eq_ignore_ascii_case("rgba") {
            let (rgb, alpha) = components.rsplit_once('/')?;
            if !(1..=4).contains(&alpha.len()) || !alpha.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            return parse_color_spec(&format!("rgb:{rgb}"));
        }
        if prefix.eq_ignore_ascii_case("rgb") {
            return parse_color_spec(&format!("rgb:{components}"));
        }
    }
    parse_color_spec(spec)
}

pub struct DisplayColors {
    pub theme: Theme,
    pub initial_input: Vec<u8>,
    input: OscReader,
    output: OscReader,
    changed: [bool; COLOR_COUNT],
}

impl Default for DisplayColors {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            initial_input: Vec::new(),
            input: OscReader::default(),
            output: OscReader::default(),
            changed: [false; COLOR_COUNT],
        }
    }
}

impl DisplayColors {
    /// A bounded startup probe. Other keyboard bytes remain queued for the
    /// attachment; late replies are consumed by the same reader afterwards.
    pub fn capture(
        &mut self,
        mut read: impl FnMut() -> io::Result<Option<Vec<u8>>>,
        mut write: impl FnMut(&[u8]) -> io::Result<usize>,
        mut interrupted: impl FnMut() -> bool,
    ) -> Result<(), String> {
        let mut request = String::from("\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]12;?\x1b\\");
        for index in 0..256 {
            let _ = write!(request, "\x1b]4;{index};?\x1b\\");
        }
        let mut sent = 0;
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(400) {
            if interrupted() {
                return Err("Screen color probe interrupted".into());
            }
            if sent < request.len() {
                match write(&request.as_bytes()[sent..]) {
                    Ok(0) => return Err("Terminal closed during color probe".into()),
                    Ok(count) => sent += count,
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    Err(e) => return Err(format!("Query terminal colors: {e}")),
                }
            }
            if let Some(bytes) = read().map_err(|e| format!("Read terminal colors: {e}"))? {
                if bytes.is_empty() {
                    return Err("Terminal input closed during color probe".into());
                }
                let bytes = self.filter_input(&bytes);
                self.initial_input.extend(bytes);
                if self.initial_input.len() > 64 * 1024 {
                    return Err("Too much input during terminal color probe".into());
                }
            }
            if sent == request.len() && self.theme.colors.iter().all(Option::is_some) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if sent != request.len() {
            return Err("Terminal color query output timed out".into());
        }
        Ok(())
    }

    pub fn filter_input(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.input.feed(bytes, |osc| {
            if let Some((index, Some(rgb))) = color_command(osc) {
                // Never adopt a later application-generated value as the
                // baseline to restore over the original terminal profile.
                if !self.changed[index] && self.theme.colors[index].is_none() {
                    self.theme.colors[index] = Some(rgb);
                }
                true
            } else {
                false
            }
        })
    }

    pub fn expired_input(&mut self) -> Vec<u8> {
        self.input.expired()
    }

    pub fn observe_output(&mut self, bytes: &[u8]) {
        self.output.feed(bytes, |osc| {
            if osc == b"104" {
                self.changed[..256].fill(true);
            } else if let Some((index, _)) = color_command(osc) {
                self.changed[index] = true;
            }
            true
        });
    }

    pub fn restore(&self) -> Vec<u8> {
        let mut output = String::new();
        for (index, changed) in self.changed.iter().enumerate() {
            if !changed {
                continue;
            }
            if let Some(rgb) = self.theme.colors[index] {
                let command = if index < 256 {
                    format!("4;{index}")
                } else {
                    (index - 246).to_string()
                };
                let _ = write!(
                    output,
                    "\x1b]{command};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                    rgb.r, rgb.g, rgb.b
                );
            } else {
                let command = if index < 256 {
                    format!("104;{index}")
                } else {
                    (index - 146).to_string()
                };
                let _ = write!(output, "\x1b]{command}\x1b\\");
            }
        }
        output.into_bytes()
    }
}
