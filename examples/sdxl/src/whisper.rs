use std::{
    io::prelude::*,
    io::BufReader,
    process::{Command, Stdio, Child}
};
use crate::makepad_platform::*;

#[derive(Debug)]
pub struct WhisperTextInput{
    pub clear: bool,
    pub text: String
}

pub struct WhisperProcess{
    child: Child
}

impl WhisperProcess{
    pub fn stop(mut self){
        let _ = self.child.kill();
    }
}

impl WhisperProcess{
    pub fn new()->Result<WhisperProcess,String>{
        #[cfg(target_os = "macos")]
        let model="/Users/admin/whisper.cpp/models/ggml-large-v3-turbo.bin";
        #[cfg(target_os = "macos")]
        let bin = "/Users/admin/whisper.cpp/stream";
        
        #[cfg(target_os = "windows")]
        let model="C:/Users/admin/whisper.cpp/models/ggml-base.en.bin";
        #[cfg(target_os = "windows")]
        let bin = "C:/Users/admin/whisper.cpp/stream.exe";
        
        let args = ["-m",model,"-t","8","--step","0","--length","5000"];
        let mut cmd_build = Command::new(bin);
        cmd_build.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        //.current_dir("/Users/admin/whisper.cpp");
                
        let mut child = cmd_build.spawn().map_err( | e | format!("Error starting whisper {e}"))?;
                
        let stdout = child.stdout.take().expect("stdout cannot be taken!");
        let _stdout_thread = {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop{
                    let mut line = String::new();
                    if let Ok(_) = reader.read_line(&mut line){
                        if line.len() == 0{
                            break
                        }
                        if line.len() > 34 && line.chars().nth(0).unwrap() == '[' && line.chars().nth(30).unwrap()==']'{
                            let text = line[34..].trim();
                            fn split_or<'a>(inp:&'a str, what:&str, found:&mut bool)->&'a str{
                                let iter = inp.split(what);
                                let count = iter.count();
                                if count>1{*found = true};
                                let iter = inp.split(what);
                                iter.last().unwrap()
                            }
                            // lets remove all occurances of the word Clear and clear and send a clear message
                            let mut f = false;
                            let text = split_or(split_or(split_or(split_or(text, "Clear.", &mut f), "clear.", &mut f), "Clear", &mut f), "clear", &mut f);
                            Cx::post_action(WhisperTextInput{
                                clear: f,
                                text: text.to_string(),
                            });
                        }
                    }
                    else{
                        break
                    }
                }
            })
        };
            
        /*let r = child.wait().map_err( | e | format!("Process {} in dir {:?} returned error {:?} ", cmd, cwd, e)) ?;
        if !r.success() {
            return Err(format!("Process {} in dir {:?} returned error exit code {} ", cmd, cwd, r));
        }*/
        Ok(WhisperProcess{
            child
        })
    }
}