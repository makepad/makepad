use crate::makepad_platform::shared_framebuf::aux_chan;
#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;
use std::{
    io::prelude::*,
    io::BufReader,
    path::PathBuf,
    process::{Child, Command, Stdio},
    str,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

pub struct ChildProcess {
    pub child: Child,
    pub stdin_sender: Sender<ChildStdIn>,
    pub line_sender: Sender<ChildStdIO>,
    pub line_receiver: Receiver<ChildStdIO>,
    pub aux_chan_listener: Option<aux_chan::ExternalEndpointListener>,
}

pub enum ChildStdIO {
    StdOut(String),
    StdErr(String),
    Term,
    Kill,
}

pub enum ChildStdIn {
    Send(String),
    Term,
}

impl ChildProcess {
    pub fn start(
        cmd: &str,
        args: &[String],
        current_dir: PathBuf,
        env: &[(String, String)],
        aux_chan: bool,
    ) -> Result<ChildProcess, std::io::Error> {
        let (mut child, aux_chan_listener) = if aux_chan {
            let studio_addr = env
                .iter()
                .find_map(|(key, value)| {
                    if key == "STUDIO" {
                        Some(value.clone())
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "missing STUDIO in child env",
                    )
                })?;
            let studio_build_id = env
                .iter()
                .find_map(|(key, value)| {
                    if key == "STUDIO_BUILD_ID" {
                        Some(value.clone())
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "missing STUDIO_BUILD_ID in child env",
                    )
                })?;
            let aux_chan_listener = aux_chan::ExternalEndpointListener::new_for_studio(
                studio_addr.as_str(),
                studio_build_id.as_str(),
            )?;

            let mut cmd_build = Command::new(cmd);
            cmd_build
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .current_dir(current_dir);

            for (key, value) in env {
                cmd_build.env(key, value);
            }

            prepare_child_process_stdio_isolation(&mut cmd_build);
            let child = cmd_build.spawn()?;
            (child, Some(aux_chan_listener))
        } else {
            let mut cmd_build = Command::new(cmd);
            cmd_build
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .current_dir(current_dir);

            for (key, value) in env {
                cmd_build.env(key, value);
            }
            prepare_child_process_stdio_isolation(&mut cmd_build);
            (cmd_build.spawn()?, None)
        };

        let (line_sender, line_receiver) = mpsc::channel();
        let (stdin_sender, stdin_receiver) = mpsc::channel();

        let mut stdin = child.stdin.take().expect("stdin cannot be taken!");
        let stdout = child.stdout.take().expect("stdout cannot be taken!");
        let stderr = child.stderr.take().expect("stderr cannot be taken!");

        let _stdout_thread = {
            let line_sender = line_sender.clone();
            let stdin_sender = stdin_sender.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let mut line = String::new();
                    if let Ok(len) = reader.read_line(&mut line) {
                        if len == 0 {
                            break;
                        }
                        if line_sender.send(ChildStdIO::StdOut(line)).is_err() {
                            break;
                        }
                    } else {
                        let _ = line_sender.send(ChildStdIO::Term);
                        let _ = stdin_sender.send(ChildStdIn::Term);
                        break;
                    }
                }
            })
        };

        let _stderr_thread = {
            let line_sender = line_sender.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                loop {
                    let mut line = String::new();
                    if let Ok(len) = reader.read_line(&mut line) {
                        if len == 0 {
                            break;
                        }
                        if line_sender.send(ChildStdIO::StdErr(line)).is_err() {
                            break;
                        };
                    } else {
                        break;
                    }
                }
            });
        };

        let _stdin_thread = {
            thread::spawn(move || {
                while let Ok(line) = stdin_receiver.recv() {
                    match line {
                        ChildStdIn::Send(line) => {
                            if let Err(_) = stdin.write_all(line.as_bytes()) {
                                //println!("Stdin send error {}",e);
                            }
                            let _ = stdin.flush();
                        }
                        ChildStdIn::Term => {
                            break;
                        }
                    }
                }
            });
        };
        Ok(ChildProcess {
            stdin_sender,
            line_sender,
            child,
            line_receiver,
            aux_chan_listener,
        })
    }

    pub fn wait(mut self) {
        let _ = self.child.wait();
    }

    pub fn kill(mut self) {
        let _ = self.stdin_sender.send(ChildStdIn::Term);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "macos")]
fn prepare_child_process_stdio_isolation(cmd: &mut Command) {
    // Make stdin-loop child processes deterministic by dropping all inherited
    // non-stdio file descriptors (including any leaked PTY-related fds).
    unsafe {
        cmd.pre_exec(|| {
            let max_fd = libc_ffi::getdtablesize();
            if max_fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            for fd in 3..max_fd {
                let _ = libc_ffi::close(fd);
            }
            Ok(())
        });
    }
}

#[cfg(not(target_os = "macos"))]
fn prepare_child_process_stdio_isolation(_cmd: &mut Command) {}

#[cfg(target_os = "macos")]
mod libc_ffi {
    extern "C" {
        pub fn getdtablesize() -> i32;
        pub fn close(fd: i32) -> i32;
    }
}
