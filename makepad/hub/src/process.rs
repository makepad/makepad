use closefds::*;
use std::process::{Command, Child, Stdio};
use std::os::unix::process::{CommandExt};
use std::sync::{mpsc};
use std::io::{Read};
use std::str;

pub struct Process {
    pub child: Option<Child>,
    pub rx_line: Option<mpsc::Receiver<Option<String>>>,
    pub rx_err: Option<mpsc::Receiver<Option<String>>>,
}

impl Process {
    
    pub fn start(cmd: &str, args: &[&str], current_dir: &str) -> Result<Process, std::io::Error> {
        fn create_process(cmd: &str, args: &[&str], current_dir: &str) -> Result<Child, std::io::Error> {
            unsafe {
                Command::new(cmd) .args(args) .pre_exec( || {
                    let _ = close_fds_on_exec(vec![0, 1, 2]).unwrap()();
                    println!("\0");
                    Ok(())
                })
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .current_dir(current_dir)
                    .spawn()
            }
        }

        let mut child = create_process(cmd, args, current_dir) ?;

        let mut stdout = child.stdout.take().expect("stdout cannot be taken!");
        let mut stderr = child.stderr.take().expect("stderr cannot be taken!");
        let mut zero = [0u8; 2];

        let bytes = stdout.read(&mut zero) ?;
        
        if bytes == 0 || zero[0] != 0 {
            panic!("Process start incorrect startup state {} {}", bytes, zero[0]);
        }

        let (tx_line, rx_line) = mpsc::channel();
        let _stdout_thread = {
            std::thread::spawn(move || {
                let mut storage = Vec::new();
                loop {
                    let offset = storage.len();
                    storage.resize(offset + 1024, 0u8);
                    let new_len = storage.len();
                    let n_bytes_read = stdout.read(&mut storage[offset..new_len]).expect("cannot read");
                    if n_bytes_read == 0{
                        tx_line.send(None).expect("tx_line cannot send - unexpected");
                        return;
                    }
                    storage.resize(offset + n_bytes_read, 0u8);
                    let mut start = 0;
                    for (index, ch) in storage.iter().enumerate() {
                        if *ch == '\n' as u8 {
                            // emit a line
                            if let Ok(line) = str::from_utf8(&storage[start..(index+1)]) {
                                tx_line.send(Some(line.to_string())).expect("tx_line cannot send - unexpected");;
                            }
                            start = index + 1;
                        }
                    }
                    storage.drain(0..start);
                }
            })
        };

        let (tx_err, rx_err) = mpsc::channel();
        let _stderr_thread = {
            std::thread::spawn(move || {
                let mut storage = Vec::new();
                loop {
                    let offset = storage.len();
                    storage.resize(offset + 1024, 0u8);
                    let new_len = storage.len();
                    let n_bytes_read = stderr.read(&mut storage[offset..new_len]).expect("cannot read");
                    if n_bytes_read == 0{
                        tx_err.send(None).expect("tx_line cannot send - unexpected");
                        return;
                    }
                    storage.resize(offset + n_bytes_read, 0u8);
                    let mut start = 0;
                    for (index, ch) in storage.iter().enumerate() {
                        if *ch == '\n' as u8 {
                            // emit a line
                            if let Ok(line) = str::from_utf8(&storage[start..(index+1)]) {
                                tx_err.send(Some(line.to_string())).expect("tx_line cannot send - unexpected");;
                            }
                            start = index + 1;
                        }
                    }
                    storage.drain(0..start);
                }
            })
        };
        
        Ok(Process {
            child: Some(child),
            rx_line: Some(rx_line),
            rx_err: Some(rx_err)
        })
    }
    
    pub fn kill(&mut self) {
        if let Some(child) = &mut self.child{
            let _ = child.kill();
            self.child = None;
        }
    }
}
