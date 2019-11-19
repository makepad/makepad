//use closefds::*;
use std::process::{Command, Child, Stdio};
//use std::os::unix::process::{CommandExt};
use std::sync::{mpsc};
use std::io::{Read};
use std::str;

pub struct Process {
    pub child: Option<Child>,
    pub rx_line: Option<mpsc::Receiver<Option<(bool,String)>>>,
}

impl Process {
    
    pub fn start(cmd: &str, args: &[&str], current_dir: &str, env:&[(&str,&str)]) -> Result<Process, std::io::Error> {
        fn create_process(cmd: &str, args: &[&str], current_dir: &str, env:&[(&str,&str)]) -> Result<Child, std::io::Error> {
            let mut cbuild = Command::new(cmd);
            cbuild.args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .current_dir(current_dir);
            for (key,value) in env{
                cbuild.env(key,value); 
            }
            cbuild.spawn()
        }

        let mut child = create_process(cmd, args, current_dir, env) ?;
 
        let (tx_line, rx_line) = mpsc::channel();
        let tx_err = tx_line.clone();
        let mut stdout = child.stdout.take().expect("stdout cannot be taken!");
        let mut stderr = child.stderr.take().expect("stderr cannot be taken!");

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
                                tx_line.send(Some((false,line.to_string()))).expect("tx_line cannot send - unexpected");;
                            }
                            start = index + 1;
                        }
                    }
                    storage.drain(0..start);
                }
            })
        };

        let _stderr_thread = { 
            std::thread::spawn(move || { 
                let mut storage = Vec::new();
                loop {
                    let offset = storage.len();
                    storage.resize(offset + 1024, 0u8);
                    let new_len = storage.len();
                    let n_bytes_read = stderr.read(&mut storage[offset..new_len]).expect("cannot read");
                    if n_bytes_read == 0{
                        return; 
                    }
                    storage.resize(offset + n_bytes_read, 0u8);
                    let mut start = 0;
                    for (index, ch) in storage.iter().enumerate() {
                        if *ch == '\n' as u8 {
                            // emit a line
                            if let Ok(line) = str::from_utf8(&storage[start..(index+1)]) {
                                tx_err.send(Some((true,line.to_string()))).expect("tx_err cannot send - unexpected");;
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
        })
    }

    pub fn wait(&mut self) {
        if let Some(child) = &mut self.child{
            let _ = child.wait();
            self.child = None;
        }
    }
    
    pub fn kill(&mut self) {
        if let Some(child) = &mut self.child{
            let _ = child.kill();
            let _ = child.wait();
            self.child = None;
        }
    }
}
