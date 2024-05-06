use std::{
    process::{Command, Child, Stdio},
    sync::mpsc::{self, Sender, Receiver},
    thread,
    io::prelude::*,
    io::BufReader,
    str,
    path::PathBuf,
};
use crate::makepad_platform::cx_stdin::aux_chan;

pub struct ChildProcess {
    pub child: Child,
    pub stdin_sender: Sender<ChildStdIn>,
    pub line_sender: Sender<ChildStdIO>,
    pub line_receiver: Receiver<ChildStdIO>,
    pub aux_chan_host_endpoint: Option<aux_chan::HostEndpoint>,
}

pub enum ChildStdIO {
    StdOut(String),
    StdErr(String),
    Term,
    Kill
}

pub enum ChildStdIn {
    Send(String),
    Term,
}

impl ChildProcess {
    
    pub fn start(cmd: &str, args: &[String], current_dir: PathBuf, env: &[(&str, &str)], aux_chan:bool) -> Result<ChildProcess, std::io::Error> {
        let (mut child, aux_chan_host_endpoint) = if aux_chan{
            let (aux_chan_host_endpoint, aux_chan_client_endpoint) =
                aux_chan::make_host_and_client_endpoint_pair()?;
            
            let aux_chan_client_endpoint_inheritable =
                aux_chan_client_endpoint.into_child_process_inheritable()?;
            let mut cmd_build = Command::new(cmd);
                cmd_build.args(args)
                .args(aux_chan_client_endpoint_inheritable.extra_args_for_client_spawning())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .current_dir(current_dir);
                        
            for (key, value) in env {
                cmd_build.env(key, value);
            }
                        
            let child = cmd_build.spawn()?;
            drop(aux_chan_client_endpoint_inheritable);
            (child, Some(aux_chan_host_endpoint))        
        }
        else{
            let mut cmd_build = Command::new(cmd);
             cmd_build.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir);
                                    
            for (key, value) in env {
                cmd_build.env(key, value);
            }
            (cmd_build.spawn()?, None)
        };

        // In the parent process, an inherited fd doesn't need to exist past
        // the spawning of the child process (which clones non-`CLOEXEC` fds).
        
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
                loop{
                    let mut line = String::new();
                    if let Ok(len) = reader.read_line(&mut line){
                        if len == 0{
                            break
                        }
                        if line_sender.send(ChildStdIO::StdOut(line)).is_err(){
                            break;
                        }
                    }
                    else{
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
                loop{
                    let mut line = String::new();
                    if let Ok(len) = reader.read_line(&mut line){
                        if len == 0{
                            break
                        }
                        if line_sender.send(ChildStdIO::StdErr(line)).is_err(){
                            break
                        };
                    }
                    else{
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
                            if let Err(_) = stdin.write_all(line.as_bytes()){
                                //println!("Stdin send error {}",e);
                                
                            }
                            let _ = stdin.flush();
                        }
                        ChildStdIn::Term=>{
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
            aux_chan_host_endpoint,
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
