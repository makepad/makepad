//! CLI test for terminal_core: spawns a PTY, renders the terminal grid to stdout.
//! Run with: cargo run -p makepad-terminal-core --bin terminal-test

use makepad_terminal_core::{Pty, Terminal};
use std::io::{self, Read, Write};

fn main() {
    eprintln!("[test] Spawning PTY...");
    let pty = match Pty::spawn(80, 24, None, &[], None) {
        Ok(p) => {
            eprintln!("[test] PTY spawned OK, child pid={}", p.child_pid());
            p
        }
        Err(e) => {
            eprintln!("[test] Failed to spawn PTY: {}", e);
            std::process::exit(1);
        }
    };

    let mut terminal = Terminal::new(80, 24);

    // Set stdin to raw mode so we can forward keypresses
    let _raw_guard = RawMode::enter();

    eprintln!("[test] Entering main loop. Type to interact, Ctrl+Q to quit.\r");

    // Spawn a thread to read from real stdin and forward to PTY
    let writer = pty.writer_clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        let stdin = io::stdin();
        let mut handle = stdin.lock();
        loop {
            match handle.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = &buf[..n];
                    // Ctrl+Q to quit
                    if data.contains(&0x11) {
                        std::process::exit(0);
                    }
                    let _ = writer.send(data.to_vec());
                }
                Err(_) => break,
            }
        }
    });

    // Main loop: read PTY output, process through terminal, render
    loop {
        // Block for data
        match pty.try_read() {
            Some(data) => {
                terminal.process_bytes(&data);
                let outbound = terminal.take_outbound();
                if !outbound.is_empty() {
                    let _ = pty.write(&outbound);
                }
                render_screen(&terminal);
            }
            None => {
                // No data available, sleep briefly
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        }
    }
}

fn render_screen(terminal: &Terminal) {
    let screen = terminal.screen();
    let cols = screen.cols();
    let rows = screen.rows();

    // Move cursor home and clear
    print!("\x1b[H");

    for row in 0..rows {
        let mut line = String::with_capacity(cols);
        for col in 0..cols {
            let cell = screen.grid.cell(col, row);
            let ch = cell.codepoint;
            if ch == '\0' || ch == ' ' {
                line.push(' ');
            } else {
                line.push(ch);
            }
        }
        // Trim trailing spaces for cleaner output
        let trimmed = line.trim_end();
        print!("{}\x1b[K\r\n", trimmed);
    }

    // Show cursor position
    let cursor = &screen.cursor;
    print!("\x1b[{};{}H", cursor.y + 1, cursor.x + 1);

    io::stdout().flush().unwrap();
}

/// RAII guard for raw terminal mode
struct RawMode {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    orig: libc_termios::Termios,
}

impl RawMode {
    fn enter() -> Self {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        unsafe {
            let mut orig: libc_termios::Termios = std::mem::zeroed();
            libc_termios::tcgetattr(0, &mut orig);
            let mut raw = orig;
            // Disable echo, canonical mode, signals
            raw.c_lflag &= !(libc_termios::ECHO
                | libc_termios::ICANON
                | libc_termios::ISIG
                | libc_termios::IEXTEN);
            // Disable input processing
            raw.c_iflag &= !(libc_termios::IXON
                | libc_termios::ICRNL
                | libc_termios::BRKINT
                | libc_termios::INPCK
                | libc_termios::ISTRIP);
            // Disable output processing
            raw.c_oflag &= !(libc_termios::OPOST);
            // Read returns after 1 byte
            raw.c_cc[libc_termios::VMIN] = 1;
            raw.c_cc[libc_termios::VTIME] = 0;
            libc_termios::tcsetattr(0, libc_termios::TCSAFLUSH, &raw);
            Self { orig }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Self {}
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        unsafe {
            libc_termios::tcsetattr(0, libc_termios::TCSAFLUSH, &self.orig);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod libc_termios {
    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct Termios {
        pub c_iflag: u64,
        pub c_oflag: u64,
        pub c_cflag: u64,
        pub c_lflag: u64,
        pub c_cc: [u8; 20],
        pub c_ispeed: u64,
        pub c_ospeed: u64,
    }

    pub const ECHO: u64 = 0x00000008;
    pub const ICANON: u64 = 0x00000100;
    pub const ISIG: u64 = 0x00000080;
    pub const IEXTEN: u64 = 0x00000400;
    pub const IXON: u64 = 0x00000200;
    pub const ICRNL: u64 = 0x00000100;
    pub const BRKINT: u64 = 0x00000002;
    pub const INPCK: u64 = 0x00000010;
    pub const ISTRIP: u64 = 0x00000020;
    pub const OPOST: u64 = 0x00000001;
    pub const VMIN: usize = 16;
    pub const VTIME: usize = 17;
    pub const TCSAFLUSH: i32 = 2;

    extern "C" {
        pub fn tcgetattr(fd: i32, termios_p: *mut Termios) -> i32;
        pub fn tcsetattr(fd: i32, optional_actions: i32, termios_p: *const Termios) -> i32;
    }
}
