use crate::dispatch::StudioEvent;
use makepad_terminal_core::Pty;
use std::collections::HashMap;
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
        event_tx: Sender<StudioEvent>,
    ) -> Result<(), String> {
        if let Some(existing) = self.terminals.get(&path) {
            let _ = existing.control_tx.send(TerminalControl::Resize { cols, rows });
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
        self.terminals.get(path).map(|terminal| terminal.mount.as_str())
    }

    pub fn remove_terminal(&mut self, path: &str) -> Option<String> {
        self.terminals.remove(path).map(|terminal| terminal.mount)
    }
}

fn run_terminal_loop(
    path: String,
    pty: Pty,
    control_rx: Receiver<TerminalControl>,
    event_tx: Sender<StudioEvent>,
) {
    let mut should_close = false;
    loop {
        loop {
            match control_rx.try_recv() {
                Ok(TerminalControl::Input(data)) => {
                    if pty.write(&data).is_err() {
                        should_close = true;
                        break;
                    }
                }
                Ok(TerminalControl::Resize { cols, rows }) => {
                    let _ = pty.resize(cols.max(1), rows.max(1));
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

        if let Some(data) = pty.try_read() {
            if !data.is_empty() {
                let _ = event_tx.send(StudioEvent::TerminalOutput { path: path.clone(), data });
            }
        }

        thread::sleep(Duration::from_millis(16));
    }

    let _ = event_tx.send(StudioEvent::TerminalExited { path, exit_code: 0 });
}
