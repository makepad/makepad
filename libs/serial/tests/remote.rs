#![cfg(unix)]
//! A remote line and its node end over a socket pair: each op reaches the board in order, timeouts
//! run on the node, a node's error keeps its kind, and a dropped link says so.

use makepad_serial::line::{Line, Step, CONNECT_WINDOW};
use makepad_serial::remote::{refuse, serve, Mode, Opener, RemoteLine, VERSION};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// The board's side as the node's port sees it: what it was told, in order.
#[derive(Default)]
struct Board {
    input: VecDeque<u8>,
    log: Vec<String>,
    fail_write: Option<io::ErrorKind>,
    fail_dtr: Option<io::ErrorKind>,
    refuse: Option<io::ErrorKind>,
    open: bool,
}

/// A port that echoes what is written to it.
struct Port(Arc<Mutex<Board>>);

impl Port {
    fn log(&self, what: String) {
        self.0.lock().unwrap().log.push(what);
    }
}

impl Line for Port {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut board = self.0.lock().unwrap();
        if let Some(kind) = board.fail_write {
            return Err(io::Error::new(kind, "the device is not reading"));
        }
        board.log.push(format!("write {}", bytes.len()));
        board.input.extend(bytes);
        Ok(())
    }

    fn read_byte(&mut self, timeout: Duration) -> io::Result<Option<u8>> {
        let mut board = self.0.lock().unwrap();
        let byte = board.input.pop_front();
        if byte.is_none() && !timeout.is_zero() {
            board.log.push(format!("quiet after {timeout:?}"));
        }
        Ok(byte)
    }

    fn set_dtr(&mut self, on: bool) -> io::Result<()> {
        if let Some(kind) = self.0.lock().unwrap().fail_dtr {
            return Err(io::Error::new(kind, "the port left"));
        }
        self.log(format!("dtr {on}"));
        Ok(())
    }

    fn set_rts(&mut self, on: bool) -> io::Result<()> {
        self.log(format!("rts {on}"));
        Ok(())
    }

    fn set_baud(&mut self, baud: u32) -> io::Result<()> {
        self.log(format!("baud {baud}"));
        Ok(())
    }

    fn drain_input(&mut self) -> io::Result<()> {
        let mut board = self.0.lock().unwrap();
        board.input.clear();
        board.log.push("drain".into());
        Ok(())
    }

    fn reopen(&mut self) -> io::Result<()> {
        unreachable!("the node reopens through its opener")
    }

    fn sleep(&mut self, d: Duration) {
        self.log(format!("wait {d:?}"));
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        let mut board = self.0.lock().unwrap();
        board.open = false;
        board.log.push("closed".into());
    }
}

struct Bench(Arc<Mutex<Board>>);

impl Bench {
    fn port(&mut self, what: String) -> io::Result<Box<dyn Line + Send>> {
        let mut board = self.0.lock().unwrap();
        if let Some(kind) = board.refuse {
            return Err(io::Error::new(kind, "off in serial.conf"));
        }
        assert!(!board.open, "the node opens one port at a time: {what}");
        board.open = true;
        board.log.push(what);
        Ok(Box::new(Port(self.0.clone())))
    }
}

impl Opener for Bench {
    fn open(&mut self, mode: Mode, baud: u32) -> io::Result<Box<dyn Line + Send>> {
        self.port(format!("open {mode:?} {baud}"))
    }

    fn reopen(&mut self, mode: Mode, baud: u32, window: Duration) -> io::Result<Box<dyn Line + Send>> {
        self.port(format!("reopen {mode:?} {baud} within {window:?}"))
    }
}

fn node(board: &Arc<Mutex<Board>>) -> (UnixStream, JoinHandle<io::Result<()>>) {
    let (client, node) = UnixStream::pair().expect("a socket pair");
    let mut bench = Bench(board.clone());
    let serving = std::thread::spawn(move || serve(node.try_clone()?, node, &mut bench));
    (client, serving)
}

fn log(board: &Arc<Mutex<Board>>) -> Vec<String> {
    std::mem::take(&mut board.lock().unwrap().log)
}

#[test]
fn a_remote_line_runs_each_op_on_the_node_in_order() {
    let board = Arc::new(Mutex::new(Board::default()));
    let (client, serving) = node(&board);
    let mut line = RemoteLine::open(client, "the bench PC", Mode::Serial, 115_200).expect("the session opens");
    line.ping().expect("a ping answers");
    assert_eq!(log(&board), ["open Serial 115200"], "the open names its mode and baud; a ping leaves the port alone");

    line.write_all(b"hello").expect("a write is sent");
    let echoed: Vec<u8> = (0..5).map(|_| line.read_byte(Duration::from_millis(50)).unwrap().expect("echoed")).collect();
    assert_eq!(echoed, b"hello", "a want brings every byte waiting, read back one at a time");
    assert_eq!(line.read_byte(Duration::from_millis(250)).unwrap(), None, "nothing waiting is quiet");
    assert_eq!(log(&board), ["write 5", "quiet after 250ms"], "the timeout runs on the node, as the client asked");

    line.sequence(&[Step::Dtr(false), Step::Rts(true), Step::Wait(Duration::from_millis(100)), Step::Rts(false), Step::Drain])
        .expect("a reset sequence runs");
    line.sleep(Duration::from_millis(30));
    line.set_baud(921_600).expect("the baud changes");
    assert_eq!(log(&board), ["dtr false", "rts true", "wait 100ms", "rts false", "drain", "wait 30ms", "baud 921600"],
        "a sequence reaches the port whole and in order, and a sleep is a wait on the node's clock");

    line.write_all(b"xy").expect("a write is sent");
    line.drain_input().expect("a drain answers");
    assert_eq!(line.read_byte(Duration::ZERO).unwrap(), None, "a drain drops what the port held");
    line.reopen().expect("the device opens again");
    assert_eq!(log(&board), ["write 2", "drain", "closed", format!("reopen Serial 921600 within {CONNECT_WINDOW:?}").as_str()],
        "a reopen closes the port first, then finds the device again at the session's baud within the window");

    board.lock().unwrap().fail_dtr = Some(io::ErrorKind::NotFound);
    let (index, err) = line.sequence(&[Step::Rts(true), Step::Dtr(true), Step::Rts(false)]).expect_err("a step fails");
    assert_eq!((index, err.kind()), (1, io::ErrorKind::NotFound), "a failed step is named by index and keeps its kind: {err}");
    assert_eq!(log(&board), ["rts true"], "the steps after a failed one do not run");
    board.lock().unwrap().fail_dtr = None;

    board.lock().unwrap().fail_write = Some(io::ErrorKind::TimedOut);
    line.write_all(b"z").expect("a write has no answer");
    let err = line.read_byte(Duration::from_millis(10)).expect_err("the write's failure answers the next op that waits");
    assert_eq!(err.kind(), io::ErrorKind::TimedOut, "a node's error keeps its kind, never a dropped link: {err}");
    board.lock().unwrap().fail_write = None;

    drop(line);
    serving.join().expect("the node thread ends").expect("a closed session ends cleanly");
    assert_eq!(log(&board).last().map(String::as_str), Some("closed"), "a client that goes closes the port");
}

#[test]
fn a_session_refused_or_dropped_says_which() {
    let board = Arc::new(Mutex::new(Board { refuse: Some(io::ErrorKind::PermissionDenied), ..Board::default() }));
    let (client, serving) = node(&board);
    let err = RemoteLine::open(client, "the bench PC", Mode::Serial, 115_200).err().expect("an off device is refused");
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "the opener's refusal reaches the client as itself: {err}");
    serving.join().expect("the node thread ends").expect("a refused session ends cleanly");

    let board = Arc::new(Mutex::new(Board::default()));
    let (client, node_end) = UnixStream::pair().expect("a socket pair");
    let mut bench = Bench(board.clone());
    let serving = std::thread::spawn(move || {
        // The bridge drops after the open: the node's end closes under the client.
        let mut answered = node_end.try_clone()?;
        let result = serve(node_end.try_clone()?.take(10 + 14), &mut answered, &mut bench);
        drop(node_end);
        result
    });
    let mut line = RemoteLine::open(client, "the bench PC", Mode::Serial, 115_200).expect("the session opens");
    serving.join().expect("the node thread ends").expect("the node ends with the stream");
    let err = line.read_byte(Duration::from_millis(10)).expect_err("the link dropped");
    assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted, "a dropped link is no timeout: {err}");
    assert!(err.to_string().contains("the link to the bench PC dropped"), "it names the PC: {err}");
    assert_eq!(line.ping().expect_err("still dropped").kind(), io::ErrorKind::ConnectionAborted, "every op after it says so");

    let (client, bridge) = UnixStream::pair().expect("a socket pair");
    let refusing = std::thread::spawn(move || refuse(bridge.try_clone()?, bridge, io::ErrorKind::ResourceBusy,
        "USB JTAG/serial debug unit is claimed by an app on the bench PC (4 s ago)"));
    let err = RemoteLine::open(client, "the bench PC", Mode::Serial, 115_200).err().expect("a busy device is refused");
    assert_eq!(err.kind(), io::ErrorKind::ResourceBusy, "a refused session answers on the wire, with its kind: {err}");
    assert!(err.to_string().ends_with("is claimed by an app on the bench PC (4 s ago)"), "and its words: {err}");
    refusing.join().expect("the bridge thread ends").expect("a refusal ends cleanly");

    let (mut client, serving) = node(&board);
    client.write_all(b"MPSERIAL").and_then(|()| client.write_all(&(VERSION + 1).to_le_bytes())).expect("a hello is sent");
    let mut answer = Vec::new();
    client.read_to_end(&mut answer).expect("the node answers and closes");
    assert_eq!(&answer[..8], b"MPSERIAL", "the node says hello with its own version");
    let text = String::from_utf8_lossy(&answer[10 + 9 + 1..]);
    assert_eq!(text, format!("the client speaks serial wire v{}, this node v{VERSION}", VERSION + 1), "an err names both versions");
    serving.join().expect("the node thread ends").expect("a version mismatch ends the session cleanly");
}
