// forkpty version of Process
//use render::*;
//use libc::{winsize,termios};
//use std::ptr;
//use std::mem;

pub struct Process {
}

impl Process {
    // starts a process with a read/write pipe
    // since rusts Process is broken.
    
    pub fn write(&mut self, _values: &str) {
    }

    // start a process with terminal read output
    // to Cx
    /*
    #[cfg(target_os = "linux")]
    pub fn get_platform_termios()->termios{
        termios {
            c_iflag: libc::ICRNL | libc::IXON | libc::IXANY | libc::IMAXBEL
                | libc::BRKINT | libc::IUTF8,
            c_oflag: libc::OPOST | libc::ONLCR,
            c_cflag: libc::CREAD | libc::CS8 | libc::HUPCL,
            c_lflag: libc::ICANON | libc::ISIG | libc::IEXTEN | libc::ECHO
                | libc::ECHOE | libc::ECHOK | libc::ECHOKE | libc::ECHOCTL,
            c_line:0,
            c_cc: {
                let mut c_cc: [libc::cc_t; libc::NCCS] = [0; libc::NCCS];
                c_cc[libc::VEOF] = 4;
                c_cc[libc::VEOL] = 255;
                c_cc[libc::VEOL2] = 255;
                c_cc[libc::VERASE] = 0x7f;
                c_cc[libc::VWERASE] = 23;
                c_cc[libc::VKILL] = 21;
                c_cc[libc::VREPRINT] = 18;
                c_cc[libc::VINTR] = 3;
                c_cc[libc::VQUIT] = 0x1c;
                c_cc[libc::VSUSP] = 26;
                c_cc[libc::VSTART] = 17;
                c_cc[libc::VSTOP] = 19;
                c_cc[libc::VLNEXT] = 22;
                c_cc[libc::VDISCARD] = 15;
                c_cc[libc::VMIN] = 1;
                c_cc[libc::VTIME] = 0;
                // apple only?
                //c_cc[libc::VDSUSP] = 25;
                //c_cc[libc::VSTATUS] = 20;
                c_cc
            },
            c_ispeed: libc::B230400,
            c_ospeed: libc::B230400,
        }
    }

    #[cfg(target_os = "macos")]
    pub fn get_platform_termios()->termios{
        termios {
            c_iflag: libc::ICRNL | libc::IXON | libc::IXANY | libc::IMAXBEL
                | libc::BRKINT | libc::IUTF8,
            c_oflag: libc::OPOST | libc::ONLCR,
            c_cflag: libc::CREAD | libc::CS8 | libc::HUPCL,
            c_lflag: libc::ICANON | libc::ISIG | libc::IEXTEN | libc::ECHO
                | libc::ECHOE | libc::ECHOK | libc::ECHOKE | libc::ECHOCTL,
            c_cc: {
                let mut c_cc: [libc::cc_t; libc::NCCS] = [0; libc::NCCS];
                c_cc[libc::VEOF] = 4;
                c_cc[libc::VEOL] = 255;
                c_cc[libc::VEOL2] = 255;
                c_cc[libc::VERASE] = 0x7f;
                c_cc[libc::VWERASE] = 23;
                c_cc[libc::VKILL] = 21;
                c_cc[libc::VREPRINT] = 18;
                c_cc[libc::VINTR] = 3;
                c_cc[libc::VQUIT] = 0x1c;
                c_cc[libc::VSUSP] = 26;
                c_cc[libc::VSTART] = 17;
                c_cc[libc::VSTOP] = 19;
                c_cc[libc::VLNEXT] = 22;
                c_cc[libc::VDISCARD] = 15;
                c_cc[libc::VMIN] = 1;
                c_cc[libc::VTIME] = 0;
                // apple only?
                c_cc[libc::VDSUSP] = 25;
                c_cc[libc::VSTATUS] = 20;
                c_cc
            },
            c_ispeed: libc::B230400,
            c_ospeed: libc::B230400,
        }
    }

    pub fn start() -> Process {
        // lets start a child process
        // including threads and all
        // posting signals when there is IO
        let mut winp = winsize {
            ws_col: 80,
            ws_row: 25,
            ws_xpixel: 0,
            ws_ypixel: 0
        };
        
        let mut termp = Self::get_platform_termios();
        
        unsafe{
            let mut master = mem::uninitialized();
            let pid = libc::forkpty(&mut master, ptr::null_mut(), &mut termp, &mut winp);
            if pid != 0{ // we are the master process
                // lets start a thread to read from / write to the master fd
                
            }
            else{ // child process. exec a shell or something
                
            }
            println!("WE GOT A PID {}", pid);
        }
        Process {}
    }*/
    pub fn start() -> Process {
        Process {}
    }
}
