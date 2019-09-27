use closefds::*;
use std::process::{Command, Child, Stdio};
use std::os::unix::process::{CommandExt};
use std::sync::{mpsc};
use std::io::{Read, Write};

pub struct Process {
    pub child: Option<Child>,
    pub rx_line: Option<mpsc::Receiver<String>>,
}

impl Process {
    // unix 
    fn create_process(cmd: &str, args: &[&str], current_dir: &str) -> Result<Child, std::io::Error> {
        unsafe {
            Command::new(cmd) .args(args) .pre_exec(||{
                close_fds_on_exec(vec![0, 1, 2]).unwrap();
                // write 0 to signal close fds has completed
                let _ = std::io::stdout().write_all(b"0");
                Ok(())
            }) .stdout(Stdio::piped()) .stderr(Stdio::piped()) .current_dir(current_dir) .spawn()
        }
    }
    
    pub fn start(cmd: &str, args: &[&str], current_dir: &str) -> Result<Process, std::io::Error> {
        let (tx_line, rx_line) = mpsc::channel();
        let mut child = Self::create_process(cmd, args, current_dir)?;

        let mut stdout = child.stdout.take().expect("stdout cannot be taken!");
        let mut zero = [0u8;1];
        let bytes = stdout.read(&mut zero)?;

        if bytes != 0 || zero[0] != 0{
            panic!("Process start incorrect startup state");
        }
        
        let read_thread = {
            std::thread::spawn(move || {
                loop {
                    let mut data = [0u8; 4096];
                    let n_bytes_read = stdout.read(&mut data).expect("cannot read");
                    // lets add it to our 
                    //data.truncate(n_bytes_read);
                    //let _ = tx.send(data);
                    //Cx::post_signal(signal, SIGNAL_RUST_CHECKER);
                    if n_bytes_read == 0 {// terminating?
                        return
                    }
                }
            })
        };
        
        Ok(Process{
            child: Some(child),
            rx_line: Some(rx_line)
        })
    }
    
    fn kill(&mut self){
        
    }
}
