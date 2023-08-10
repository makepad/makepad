use {
    std::{
        process::{Command, Child, Stdio},
        sync::mpsc::{self, Sender, Receiver},
        thread,
        io::prelude::*,
        io::{BufReader},
        str,
        path::{PathBuf},
    }
};

pub struct ChildProcess {
    pub child: Child,
    pub stdin_sender: Sender<ChildStdIn>,
    pub line_sender: Sender<ChildStdIO>,
    pub line_receiver: Receiver<ChildStdIO>,
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
    
    pub fn start(cmd: &str, args: &[&str], current_dir: PathBuf, env: &[(&str, &str)]) -> Result<ChildProcess, std::io::Error> {
        
        let mut cmd_build = Command::new(cmd);
        
        cmd_build.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir);
        
        for (key, value) in env {
            cmd_build.env(key, value);
        }
        
        let mut child = cmd_build.spawn()?;
        
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
                                //println!("Stdin send error {}", e);
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
