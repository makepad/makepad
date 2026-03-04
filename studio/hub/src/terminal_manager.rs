use crate::dispatch::HubEvent;
use makepad_terminal_core::Pty;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

enum TerminalControl {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Close,
}

struct RunningTerminal {
    mount: String,
    control_tx: Sender<TerminalControl>,
}

struct PendingInput {
    data: Vec<u8>,
    offset: usize,
}

#[derive(Default)]
pub struct TerminalManager {
    terminals: HashMap<String, RunningTerminal>,
}

impl TerminalManager {
    pub fn open_terminal(
        &mut self,
        path: String,
        mount: String,
        cwd: &Path,
        cols: u16,
        rows: u16,
        env: HashMap<String, String>,
        event_tx: Sender<HubEvent>,
    ) -> Result<(), String> {
        if let Some(existing) = self.terminals.get(&path) {
            let _ = existing
                .control_tx
                .send(TerminalControl::Resize { cols, rows });
            return Ok(());
        }

        let env_pairs_owned: Vec<(String, String)> = env.into_iter().collect();
        let env_pairs: Vec<(&str, &str)> = env_pairs_owned
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();

        let pty = Pty::spawn(cols.max(1), rows.max(1), None, &env_pairs, Some(cwd))
            .map_err(|err| format!("failed to spawn terminal for {}: {}", cwd.display(), err))?;

        let (control_tx, control_rx) = mpsc::channel::<TerminalControl>();
        let path_clone = path.clone();
        thread::spawn(move || run_terminal_loop(path_clone, pty, control_rx, event_tx));

        self.terminals
            .insert(path, RunningTerminal { mount, control_tx });
        Ok(())
    }

    pub fn send_input(&self, path: &str, data: Vec<u8>) -> Result<(), String> {
        let Some(terminal) = self.terminals.get(path) else {
            return Err(format!("unknown terminal: {}", path));
        };
        terminal
            .control_tx
            .send(TerminalControl::Input(data))
            .map_err(|_| format!("failed to send input to terminal: {}", path))
    }

    pub fn resize(&self, path: &str, cols: u16, rows: u16) -> Result<(), String> {
        let Some(terminal) = self.terminals.get(path) else {
            return Err(format!("unknown terminal: {}", path));
        };
        terminal
            .control_tx
            .send(TerminalControl::Resize { cols, rows })
            .map_err(|_| format!("failed to resize terminal: {}", path))
    }

    pub fn close_terminal(&mut self, path: &str) {
        let Some(terminal) = self.terminals.remove(path) else {
            return;
        };
        let _ = terminal.control_tx.send(TerminalControl::Close);
    }

    pub fn mount_for_path(&self, path: &str) -> Option<&str> {
        self.terminals
            .get(path)
            .map(|terminal| terminal.mount.as_str())
    }

    pub fn remove_terminal(&mut self, path: &str) -> Option<String> {
        self.terminals.remove(path).map(|terminal| terminal.mount)
    }
}

fn run_terminal_loop(
    path: String,
    pty: Pty,
    control_rx: Receiver<TerminalControl>,
    event_tx: Sender<HubEvent>,
) {
    const MAX_READ_BYTES_PER_TICK: usize = 1 << 20;

    let mut should_close = false;
    let mut pending_input = VecDeque::<PendingInput>::new();
    let mut pending_resize: Option<(u16, u16)> = None;
    let mut last_resize_time = std::time::Instant::now() - Duration::from_secs(1);
    let resize_throttle = Duration::from_millis(50); // Throttle to 20fps to prevent TUI garbling

    loop {
        loop {
            match control_rx.try_recv() {
                Ok(TerminalControl::Input(data)) => {
                    if !data.is_empty() {
                        pending_input.push_back(PendingInput { data, offset: 0 });
                    }
                }
                Ok(TerminalControl::Resize { cols, rows }) => {
                    let cols = cols.max(1);
                    let rows = rows.max(1);
                    // Coalesce resize bursts; apply latest size once per loop.
                    pending_resize = Some((cols, rows));
                }
                Ok(TerminalControl::Close) => {
                    should_close = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    should_close = true;
                    break;
                }
            }
        }

        if should_close {
            break;
        }

        while let Some(front) = pending_input.front_mut() {
            let remaining = &front.data[front.offset..];
            match pty.try_write(remaining) {
                Ok(0) => {
                    should_close = true;
                    break;
                }
                Ok(n) => {
                    front.offset += n;
                    if front.offset >= front.data.len() {
                        pending_input.pop_front();
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    should_close = true;
                    break;
                }
            }
        }

        if should_close {
            break;
        }

        let mut output = Vec::new();
        loop {
            let Some(data) = pty.try_read() else {
                break;
            };
            if data.is_empty() {
                continue;
            }
            output.extend_from_slice(&data);
            if output.len() >= MAX_READ_BYTES_PER_TICK {
                break;
            }
        }
        let had_output = !output.is_empty();
        if had_output {
            let _ = event_tx.send(HubEvent::TerminalOutput {
                path: path.clone(),
                data: output,
            });
        }

        // Apply one coalesced resize after I/O so buffered output is consumed
        // before switching the terminal model geometry.
        // Throttle rapid resizes to prevent TUI apps from receiving a storm of SIGWINCH
        // signals, which causes them to draw intermediate UI states that are left behind.
        if let Some((cols, rows)) = pending_resize {
            let now = std::time::Instant::now();
            if now.duration_since(last_resize_time) >= resize_throttle {
                pending_resize = None;
                last_resize_time = now;
                if pty.resize(cols, rows).is_ok() {
                    let _ = event_tx.send(HubEvent::TerminalResized {
                        path: path.clone(),
                        cols,
                        rows,
                    });
                }
            }
        }

        if pending_input.is_empty() && pending_resize.is_none() && !had_output {
            thread::sleep(Duration::from_millis(16));
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    }

    let _ = event_tx.send(HubEvent::TerminalExited { path, exit_code: 0 });
}
