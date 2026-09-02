use crate::{vm, ScriptStd, ScriptVmStdExt};
use makepad_network::{FromUISender, ToUIReceiver};
use makepad_script::*;
use std::{
    collections::hash_map::HashMap,
    io::prelude::*,
    io::BufReader,
    process::{Child, Command, Stdio},
    thread,
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

pub struct ScriptChildProcessState {
    #[allow(unused)]
    pub id: LiveId,
    child: ChildProcess,
    pub events: ScriptChildEvents,
}

#[derive(Script, ScriptHook)]
pub struct ScriptChildEvents {
    #[live]
    pub on_stdout: Option<ScriptFnRef>,
    #[live]
    pub on_stderr: Option<ScriptFnRef>,
    #[live]
    pub on_term: Option<ScriptFnRef>,
}

#[derive(Script, ScriptHook)]
pub struct ScriptChildCmd {
    #[live]
    pub cmd: String,
    #[live]
    pub args: Option<Vec<String>>,
    #[live]
    pub env: Option<HashMap<String, String>>,
    #[live]
    pub cwd: Option<String>,
}

pub fn handle_script_child_processes(host: &mut dyn ScriptHost) {
    let mut i = 0;
    while i
        < host
            .script_std()
            .downcast_ref::<ScriptStd>()
            .unwrap()
            .data
            .child_processes
            .len()
    {
        let mut term = false;

        loop {
            let value = host
                .script_std()
                .downcast_mut::<ScriptStd>()
                .unwrap()
                .data
                .child_processes[i]
                .child
                .out_recv
                .try_recv();
            let Ok(value) = value else {
                break;
            };
            match value {
                ChildOut::StdOut(s) => {
                    let handler = host
                        .script_std()
                        .downcast_ref::<ScriptStd>()
                        .unwrap()
                        .data
                        .child_processes[i]
                        .events
                        .on_stdout
                        .as_object();
                    if let Some(handler) = handler {
                        vm::with_vm_and_async(host, |vm| {
                            let str = vm.bx.heap.new_string_from_str(&s);
                            vm.call(handler.into(), &[str.into()]);
                        });
                    }
                }
                ChildOut::StdErr(s) => {
                    let handler = host
                        .script_std()
                        .downcast_ref::<ScriptStd>()
                        .unwrap()
                        .data
                        .child_processes[i]
                        .events
                        .on_stderr
                        .as_object();
                    if let Some(handler) = handler {
                        vm::with_vm_and_async(host, |vm| {
                            let str = vm.bx.heap.new_string_from_str(&s);
                            vm.call(handler.into(), &[str.into()]);
                        });
                    }
                }
                ChildOut::Term => {
                    let handler = host
                        .script_std()
                        .downcast_ref::<ScriptStd>()
                        .unwrap()
                        .data
                        .child_processes[i]
                        .events
                        .on_term
                        .as_object();
                    if let Some(handler) = handler {
                        vm::with_vm_and_async(host, |vm| {
                            vm.call(handler.into(), &[]);
                        });
                    }
                    term = true;
                    break;
                }
            }
        }
        if term {
            host.script_std()
                .downcast_mut::<ScriptStd>()
                .unwrap()
                .data
                .child_processes
                .remove(i);
        } else {
            i += 1;
        }
    }
}

pub fn script_mod(vm: &mut ScriptVm) {
    let run = vm.new_module(id_lut!(run));

    set_script_value_to_api!(vm, run.ScriptChildEvents);
    set_script_value_to_api!(vm, run.ScriptChildCmd);

    vm.add_method(
        run,
        id_lut!(child),
        script_args_def!(cmd = NIL, events = NIL),
        move |vm, args| {
            let cmd = script_value!(vm, args.cmd);
            let events = script_value!(vm, args.events);

            if !script_has_proto!(vm, cmd, run.ScriptChildCmd)
                || !script_has_proto!(vm, events, run.ScriptChildEvents)
            {
                return script_err_type_mismatch!(vm.trap(), "invalid run arg type");
            }

            let cmd = ScriptChildCmd::script_from_value(vm, cmd);
            let events = ScriptChildEvents::script_from_value(vm, events);

            let mut cmd_build = Command::new(cmd.cmd);

            if let Some(env) = cmd.env {
                for (key, value) in env {
                    cmd_build.env(key, value);
                }
            }
            if let Some(args) = cmd.args {
                cmd_build.args(args);
            }
            cmd_build
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            if let Some(cwd) = cmd.cwd {
                cmd_build.current_dir(cwd);
            }

            match ChildProcess::spawn(cmd_build) {
                Ok(child) => {
                    let id = LiveId::unique();
                    vm.std_mut::<ScriptStd>()
                        .data
                        .child_processes
                        .push(ScriptChildProcessState { child, id, events });
                    id.escape()
                }
                Err(_) => script_err_io!(vm.bx.threads.trap(), "child process error"),
            }
        },
    );
}

impl ChildProcess {
    pub fn spawn(mut command: Command) -> Result<ChildProcess, std::io::Error> {
        let mut child = command.spawn()?;

        let mut stdin = child.stdin.take().expect("stdin cannot be taken!");
        let stdout = child.stdout.take().expect("stdout cannot be taken!");
        let stderr = child.stderr.take().expect("stderr cannot be taken!");

        let out_recv: ToUIReceiver<ChildOut> = Default::default();
        let out_send = out_recv.sender();

        let mut in_send: FromUISender<ChildIn> = Default::default();
        let in_recv = in_send
            .receiver()
            .expect("child stdin receiver is taken exactly once");

        let _stdout_thread = {
            let out_send = out_send.clone();
            let in_send = in_send.sender();
            thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let mut line = String::new();
                    if let Ok(len) = reader.read_line(&mut line) {
                        if len == 0 {
                            break;
                        }
                        if out_send.send(ChildOut::StdOut(line)).is_err() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let _ = out_send.send(ChildOut::Term);
                let _ = in_send.send(ChildIn::Term);
            })
        };

        let _stderr_thread = {
            let out_send = out_send.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                loop {
                    let mut line = String::new();
                    if let Ok(len) = reader.read_line(&mut line) {
                        if len == 0 {
                            break;
                        }
                        if out_send.send(ChildOut::StdErr(line)).is_err() {
                            break;
                        }
                    } else {
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
                            let _ = stdin.write_all(line.as_bytes());
                            let _ = stdin.flush();
                        }
                        ChildIn::Term => break,
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
}
