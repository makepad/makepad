use widget::*;
use std::io::Read;
use std::process::{Command, Child, Stdio};
use std::sync::mpsc;

//#[derive(Clone)]
pub struct RustCompiler{
    pub view:View<ScrollBar>,
    pub bg:Quad,
    pub text:Text,
    pub _timer_id:u64,
    pub _child:Option<Child>,
    pub _rx:Option<mpsc::Receiver<std::vec::Vec<u8>>>,
    pub _thread:Option<std::thread::JoinHandle<()>>,
    pub _data:Vec<String>
}

#[derive(Clone, PartialEq)]
pub enum RustCompilerEvent{
    None,
}

impl Style for RustCompiler{
    fn style(cx:&mut Cx)->Self{
        Self{
            _timer_id:0,
            bg:Quad{
                ..Style::style(cx)
            },
            text:Text{
                ..Style::style(cx)
            },
            view:View{
                scroll_h:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                scroll_v:Some(ScrollBar{
                    smoothing:Some(0.15),
                    ..Style::style(cx)
                }),
                ..Style::style(cx)
            },
            _child:None,
            _thread:None,
            _rx:None,
            _data:Vec::new()
        }
    }
}

impl RustCompiler{
    pub fn start_poll(&mut self, cx:&mut Cx){
        self._timer_id = cx.start_timer(0.1,true);
        self.restart_rust_compiler();
    }

    pub fn handle_rust_compiler(&mut self, cx:&mut Cx, event:&mut Event)->RustCompilerEvent{
        // do shit here
        if self.view.handle_scroll_bars(cx, event){
            // do zshit.
        }
        if let Event::Timer(te) = event{
            if te.id == self._timer_id{
                if let Some(rx) = &self._rx{
                    if let Ok(data) = rx.try_recv(){
                        for ch in data{
                            if ch == '\n' as u8{
                                self._data.push(String::new());
                            }
                            else{
                                self._data.last_mut().unwrap().push(ch as char);
                            }
                        }
                        self.view.redraw_view_area(cx);
                    }
                }
            }
        }

        RustCompilerEvent::None
    }

    pub fn draw_rust_compiler(&mut self, cx:&mut Cx){
        if let Err(_) = self.view.begin_view(cx, &Layout{..Default::default()}){
            return
        }

        for line in &self._data{
            self.text.draw_text(cx, line);
            cx.turtle_new_line();
        }

        self.view.end_view(cx);
    }

    pub fn restart_rust_compiler(&mut self){
        self._data.truncate(0);
        self._data.push(String::new());

        if let Some(child) = &mut self._child{
            let _= child.kill();
        }

        let mut child = Command::new("cargo")
            .args(&["build"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir("./edit_repo")
            .spawn()
            .unwrap();

        let mut stderr =  child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();

        let thread = std::thread::spawn(move ||{
            loop{
                let mut data = vec![0; 1024];
                let n_bytes_read = stderr.read(&mut data).expect("cannot read");
                if n_bytes_read == 0{
                    return
                }
                data.truncate(n_bytes_read);
                let _ = tx.send(data);
            }
        });
        self._rx = Some(rx);
        self._thread = Some(thread);
        self._child = Some(child);
    }
}