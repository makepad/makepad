//! The session's sole VT parser. Peers consume projections, never PTY bytes.
use crate::snapshot::Projection;
use crate::term::{color::Rgb, stream::Stream, terminal::TermEvent, Terminal};

pub const MAX_COLS: usize = 400;
pub const MAX_ROWS: usize = 200;
pub const MAX_HISTORY: usize = 5_000;

pub struct Update {
    /// Write these bytes to the owned PTY exactly once, never to a client.
    pub replies: Vec<u8>,
}

pub struct HostedTerminal {
    terminal: Terminal,
    stream: Stream,
    history_limit: usize,
    history_epoch: u64,
    bells: u64,
}

impl HostedTerminal {
    pub fn new(cols: usize, rows: usize, history_limit: usize) -> Self {
        Self::with_theme(cols, rows, history_limit, &crate::theme::Theme::default())
    }

    pub fn with_theme(
        cols: usize,
        rows: usize,
        history_limit: usize,
        theme: &crate::theme::Theme,
    ) -> Self {
        let mut terminal = Terminal::new(cols.clamp(2, MAX_COLS), rows.clamp(2, MAX_ROWS));
        // Internal fallbacks are not the attached display's actual theme.
        // Reporting them to a child makes it blend message backgrounds
        // against an invented color (black can eliminate the shading).
        let base16 = terminal.base_palette[..16].try_into().unwrap();
        terminal.set_theme(&base16, Rgb::new(0xe5, 0xe5, 0xe5), Rgb::new(0, 0, 0));
        terminal.known_theme_colors = theme.colors.map(|color| color.is_some());
        for (index, color) in theme.colors[..256].iter().enumerate() {
            if let Some(rgb) = color {
                terminal.palette[index] = *rgb;
                terminal.base_palette[index] = *rgb;
            }
        }
        if let Some(rgb) = theme.colors[256] {
            terminal.default_fg = rgb;
            terminal.base_fg = rgb;
        }
        if let Some(rgb) = theme.colors[257] {
            terminal.default_bg = rgb;
            terminal.base_bg = rgb;
        }
        terminal.base_cursor = theme.colors[258];
        let history_limit = history_limit.min(MAX_HISTORY);
        terminal.primary.max_scrollback = history_limit;
        Self {
            terminal,
            stream: Stream::new(),
            history_limit,
            history_epoch: 0,
            bells: 0,
        }
    }

    pub fn process(&mut self, bytes: &[u8]) -> Update {
        for byte in bytes {
            let history = (
                self.terminal.primary.evicted,
                self.terminal.primary.scrollback.len(),
            );
            self.stream.next(*byte, &mut self.terminal);
            // A reset/clear followed by new output can restore the old row
            // count within this very batch. Record it before that happens.
            if self.terminal.primary.evicted < history.0
                || self.terminal.primary.scrollback.len() < history.1
            {
                self.history_epoch = self.history_epoch.wrapping_add(1);
            }
            // RIS reconstructs the primary screen with the core's default
            // limit. Restore our limit before another byte can scroll it.
            self.terminal.primary.max_scrollback = self.history_limit;
        }
        self.finish_update()
    }

    pub fn resize(&mut self, cols: usize, rows: usize) -> Update {
        self.terminal
            .resize(cols.clamp(2, MAX_COLS), rows.clamp(2, MAX_ROWS));
        self.finish_update()
    }

    fn finish_update(&mut self) -> Update {
        let screen = &mut self.terminal.primary;
        while screen.scrollback.len() > self.history_limit {
            screen.scrollback.pop_front();
            screen.evicted = screen.evicted.saturating_add(1);
        }
        for event in self.terminal.take_events() {
            if matches!(event, TermEvent::Bell) {
                self.bells = self.bells.wrapping_add(1);
            }
            // Title/pwd already live on Terminal. In particular, do not
            // replay clipboard operations or notifications on attachment.
        }
        Update {
            replies: self.terminal.take_outbound(),
        }
    }

    pub fn render(&self, projection: &mut Projection, cols: usize, rows: usize) -> Vec<u8> {
        projection.render(&self.terminal, self.bells, self.history_epoch, cols, rows)
    }

    pub fn snapshot(&self, cols: usize, rows: usize) -> Vec<u8> {
        self.render(&mut Projection::default(), cols, rows)
    }

    pub fn text_tail(&self, lines: usize) -> String {
        let screen = self.terminal.screen();
        let from = screen
            .total_rows()
            .saturating_sub(lines.min(MAX_HISTORY + MAX_ROWS));
        (from..screen.total_rows())
            .filter_map(|index| screen.row_virtual(index))
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn cols(&self) -> usize {
        self.terminal.cols()
    }
    pub fn rows(&self) -> usize {
        self.terminal.rows()
    }
    pub fn title(&self) -> &str {
        &self.terminal.title
    }
    pub fn pwd(&self) -> &str {
        &self.terminal.pwd
    }
}
