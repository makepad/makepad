use {
    std::{
        process::{Command, Child, Stdio},
        sync::mpsc::{self, Receiver},
        thread,
        io::prelude::*,
        io::{BufReader},
        str,
        path::{PathBuf},
    }
};

pub struct ChildProcess {
    pub child: Option<Child>,
    pub line_receiver: Receiver<ChildLine>,
}

pub enum ChildLine {
    StdOut(String),
    StdErr(String),
    Term
}

impl ChildProcess {
    
    pub fn start(cmd: &str, args: &[&str], current_dir: PathBuf, env: &[(&str, &str)]) -> Result<ChildProcess, std::io::Error> {
        
        let mut cmd_build = Command::new(cmd);
        
        cmd_build.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir);
        
        for (key, value) in env {
            cmd_build.env(key, value);
        }
        
        let mut child = cmd_build.spawn()?;
        
        let (line_sender, line_receiver) = mpsc::channel();

        let stdout = child.stdout.take().expect("stdout cannot be taken!");
        let stderr = child.stderr.take().expect("stderr cannot be taken!");
        let _stdout_thread = {
            let line_sender = line_sender.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop{
                    let mut line = String::new();
                    if let Ok(len) = reader.read_line(&mut line){
                        if len == 0{
                            break
                        }
                        if line_sender.send(ChildLine::StdOut(line)).is_err(){
                            break;
                        }
                    }
                    else{
                        let _ = line_sender.send(ChildLine::Term);
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
                        if line_sender.send(ChildLine::StdErr(line)).is_err(){
                            break
                        };
                    }
                    else{
                        break;
                    }
                }
            });
        };
        
        Ok(ChildProcess {
            child: Some(child),
            line_receiver,
        })
    }
    
    pub fn wait(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.wait();
            self.child = None;
        }
    }
    
    pub fn kill(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
            self.child = None;
        }
    }
}
