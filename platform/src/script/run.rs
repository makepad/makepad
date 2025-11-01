use crate::*;
use makepad_script::*;
use crate::script::vm::*;

use std::{
    process::{Command, Child, Stdio},
    thread,
    io::prelude::*,
    io::BufReader,
    collections::hash_map::HashMap,
};

struct ChildProcess {
    #[allow(unused)]
    child: Child,
    #[allow(unused)]
    in_send: FromUISender<ChildIn>,
    out_recv: ToUIReceiver<ChildOut>,
}

enum ChildOut {
    StdOut(String),
    StdErr(String),
    Term,
}

enum ChildIn {
    #[allow(unused)]
    Send(String),
    Term,
}

impl ChildProcess {
        
    pub fn spawn(mut command:Command) -> Result<ChildProcess, std::io::Error> {
        
        let mut child = command.spawn()?;
        
        let mut stdin = child.stdin.take().expect("stdin cannot be taken!");
        let stdout = child.stdout.take().expect("stdout cannot be taken!");
        let stderr = child.stderr.take().expect("stderr cannot be taken!");
        
        let out_recv:ToUIReceiver<ChildOut> = Default::default();
        let out_send = out_recv.sender();
        
        let mut in_send:FromUISender<ChildIn> = Default::default();
        let in_recv = in_send.receiver();
        
        let _stdout_thread = {
            let out_send = out_send.clone();
            let in_send = in_send.sender();
            thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop{
                    let mut line = String::new();
                    if let Ok(len) = reader.read_line(&mut line){
                        if len == 0{
                            break
                        }
                        if out_send.send(ChildOut::StdOut(line)).is_err(){
                            break;
                        }
                    }
                    else{
                        let _ = out_send.send(ChildOut::Term);
                        let _ = in_send.send(ChildIn::Term);
                        break;
                    }
                }
            })
        };
                
        let _stderr_thread = {
            let out_send = out_send.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                loop{
                    let mut line = String::new();
                    if let Ok(len) = reader.read_line(&mut line){
                        if len == 0{
                            break
                        }
                        if out_send.send(ChildOut::StdErr(line)).is_err(){
                            break;
                        }
                    }
                    else{
                        break;
                    }
                }
            });
        };
        
        let _stdin_thread = {
            thread::spawn(move || {
                while let Ok(line) = in_recv.recv() {
                    match line {
                        ChildIn::Send(line) => {
                            if let Err(_) = stdin.write_all(line.as_bytes()){
                                //println!("Stdin send error {}",e);
                                                                
                            }
                            let _ = stdin.flush();
                        }
                        ChildIn::Term=>{
                            break;
                        }
                    }
                }
            });
        };
        
        
        Ok(ChildProcess {
            in_send,
            out_recv,
            child,
        })
    }
    
    #[allow(unused)]
    pub fn kill(mut self) {
        let _ = self.in_send.send(ChildIn::Term);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct CxScriptChildProcess{
    #[allow(unused)]
    id: LiveId,
    child: ChildProcess,
    events: ChildEvents,
}

#[derive(Script, ScriptHook)]
pub struct ChildEvents{
    #[live] pub on_stdout: Option<ScriptFnRef>,
    #[live] pub on_stderr: Option<ScriptFnRef>,
    #[live] pub on_term: Option<ScriptFnRef>,
}

#[derive(Script, ScriptHook)]
pub struct ChildCmd{
    #[live] pub cmd: String,
    #[live] pub args: Option<Vec<String>>,
    #[live] pub env: Option<HashMap<String,String>>,
    #[live] pub cwd: Option<String>
}

impl Cx{
    pub(crate) fn handle_script_child_processes(&mut self){
        let mut i = 0;
        while i<self.script_data.child_processes.len(){
            let mut term = false;

            while let Ok(value) = self.script_data.child_processes[i].child.out_recv.try_recv(){
                match value{
                    ChildOut::StdOut(s)=>{
                        if let Some(handler) = self.script_data.child_processes[i].events.on_stdout.as_obj(){
                            self.with_vm(|vm|{
                                let str = vm.heap.new_string_from_str(&s);
                                vm.call(handler.into(), &[str.into()]);
                            })
                        }
                    }
                    ChildOut::StdErr(s)=>{
                        if let Some(handler) = self.script_data.child_processes[i].events.on_stderr.as_obj(){
                            self.with_vm(|vm|{
                                let str = vm.heap.new_string_from_str(&s);
                                vm.call(handler.into(), &[str.into()]);
                            })
                        }
                    }
                    ChildOut::Term=>{
                        if let Some(handler) = self.script_data.child_processes[i].events.on_term.as_obj(){
                            self.with_vm(|vm|{
                                vm.call(handler.into(), &[]);
                            })
                        }
                        term = true;
                        break;
                    }
                }
            }
            if term{
                self.script_data.child_processes.remove(i);
            }
            else{
                i += 1;
            }
        }
    }
}

pub fn define_run_module(vm:&mut ScriptVm){
    let run = vm.new_module(id_lut!(run));
    
    script_proto!(vm, run, ChildEvents);
    script_proto!(vm, run, ChildCmd);
    
    vm.add_fn(run, id!(child), script_args_def!(cmd=NIL, events=NIL), move |vm, args|{
        
        let cmd = script_value!(vm, args.cmd);
        let events = script_value!(vm, args.events);
                
        if !script_has_proto!(vm, cmd, run.ChildCmd) || 
            !script_has_proto!(vm, events, run.ChildEvents){
            return vm.thread.trap.err_invalid_arg_type()
        }
        
        let cmd = ChildCmd::script_from_value(vm, cmd);
        let events = ChildEvents::script_from_value(vm, events);
        
        let mut cmd_build = Command::new(cmd.cmd);
        
        if let Some(env) = cmd.env{
            for (key, value) in env {
                cmd_build.env(key, value);
            }
        }
        if let Some(args) = cmd.args{
            cmd_build.args(args);
        }
        cmd_build
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
            
        if let Some(cwd) = cmd.cwd{
            cmd_build.current_dir(cwd);
        }
        
        let cx = vm.cx_mut();
        
        match ChildProcess::spawn(cmd_build){
            Ok(child)=>{
                
                let id = LiveId::unique();
                cx.script_data.child_processes.push(CxScriptChildProcess{
                    child,
                    id,
                    events,
                });
                id.escape()
            }
            Err(_e)=>{
                println!("ERROR SPAWNING");
                vm.thread.trap.err_child_process()
            }
        }
    });
    
}   