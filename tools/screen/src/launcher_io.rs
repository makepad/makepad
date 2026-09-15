use std::{
    io,
    time::{Duration, Instant},
};

const ENTER: &[u8] = b"\x1b[?1049h\x1b[0m\x1b[?25l\x1b[=0;1u\x1b[>4;0m\x1b[?2004h";
const LEAVE: &[u8] = b"\x1b[0m\x1b[?2004l\x1b[?25h\x1b[?1049l";

#[cfg(unix)]
pub struct Console {
    raw: crate::unix::RawTerminal,
    input: crate::unix::NonblockingGuard,
    output: crate::unix::NonblockingGuard,
    signals: crate::unix::SignalGuard,
    active: bool,
}

#[cfg(unix)]
impl Console {
    pub fn new() -> Result<Self, String> {
        use crate::unix::*;
        if !is_tty(0) || !is_tty(1) {
            return Err("The screen launcher requires an interactive terminal".into());
        }
        let mut console = Self {
            raw: RawTerminal::new(0).map_err(|e| e.to_string())?,
            input: NonblockingGuard::new(0).map_err(|e| e.to_string())?,
            output: NonblockingGuard::new(1).map_err(|e| e.to_string())?,
            signals: SignalGuard::new().map_err(|e| e.to_string())?,
            active: true,
        };
        console.write(ENTER)?;
        Ok(console)
    }
    pub fn size(&self) -> Result<(usize, usize), String> {
        crate::unix::window_size(1)
            .map(|(x, y)| (usize::from(x).min(400), usize::from(y).min(200)))
            .map_err(|e| e.to_string())
    }
    pub fn interrupted(&self) -> bool {
        self.signals.interrupted().is_some()
    }
    pub fn read(&mut self) -> Result<Option<Vec<u8>>, String> {
        let mut bytes = [0; 4096];
        match crate::unix::read_fd(0, &mut bytes) {
            Ok(n) => Ok(Some(bytes[..n].to_vec())),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let until = Instant::now() + Duration::from_millis(500);
        let mut sent = 0;
        while sent < bytes.len() {
            if Instant::now() >= until {
                return Err("Launcher terminal output timed out".into());
            }
            match crate::unix::write_fd(1, &bytes[sent..]) {
                Ok(0) => return Err("Launcher terminal output closed".into()),
                Ok(n) => sent += n,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
    pub fn close(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let screen = self.write(LEAVE);
        let raw = self.raw.restore().map_err(|e| e.to_string());
        let input = self.input.restore().map_err(|e| e.to_string());
        let output = self.output.restore().map_err(|e| e.to_string());
        screen.and(raw).and(input).and(output)
    }
}

#[cfg(windows)]
pub struct Console {
    console: crate::client::windows_console::Console,
    io: crate::client::windows_console::ConsoleIo,
    active: bool,
}

#[cfg(windows)]
impl Console {
    pub fn new() -> Result<Self, String> {
        let console = crate::client::windows_console::Console::new().map_err(|e| e.to_string())?;
        if !console.is_terminal() {
            return Err("The screen launcher requires an interactive console".into());
        }
        let io = console.io().map_err(|e| e.to_string())?;
        let mut result = Self {
            console,
            io,
            active: true,
        };
        result.write(ENTER)?;
        Ok(result)
    }
    pub fn size(&self) -> Result<(usize, usize), String> {
        self.console
            .size()
            .map(|(x, y)| (x as usize, y as usize))
            .map_err(|e| e.to_string())
    }
    pub fn interrupted(&self) -> bool {
        self.console.interrupted().is_some()
    }
    pub fn read(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.io.read().map_err(|e| e.to_string())
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let until = Instant::now() + Duration::from_millis(500);
        let mut sent = 0;
        while sent < bytes.len() {
            if Instant::now() >= until {
                return Err("Launcher console output timed out".into());
            }
            match self.io.write(&bytes[sent..]) {
                Ok(0) => return Err("Launcher console output closed".into()),
                Ok(n) => sent += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
    pub fn close(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let screen = self.io.restore_vt(LEAVE).map_err(|e| e.to_string());
        let workers = self.io.shutdown().map_err(|e| e.to_string());
        let console = self.console.restore().map_err(|e| e.to_string());
        screen.and(workers).and(console)
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
