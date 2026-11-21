//! Checks one spoken-turn conversation in wire order, counting messages in both directions.

use std::collections::HashMap;
use std::str::FromStr;

#[derive(Default)]
enum Reply {
    #[default]
    AwaitingTranscript,
    Heard,
    Said,
}

#[derive(Default)]
struct Turn {
    id: u16,
    recorded: u32,
    submitted: bool,
    reply: Reply,
    sent: u32,
    /// Unpaced: where the announced sentence's speech ends.
    speech_end: Option<u32>,
    context: Context,
    /// The attention gate's verdict came.
    filtered: bool,
}

/// A turn's context frame: none, sent, or sent and answered.
#[derive(Default, PartialEq)]
enum Context {
    #[default]
    None,
    Sent,
    Answered,
}

/// A cancelled turn can still receive its acknowledgement after the next take starts.
enum Terminal {
    AwaitingDone(u32),
    Finished,
}

/// Feed both directions to the same checker. Errors identify the direction and
/// one-based message number and leave the turn at the last valid message.
#[derive(Default)]
pub struct Checker {
    message: usize,
    hello: bool,
    unpaced: bool,
    turn: Option<Turn>,
    terminal: HashMap<u16, Terminal>,
}

type Checked<T> = Result<T, &'static str>;

fn words(bytes: &[u8]) -> Checked<(&str, &str)> {
    let line = std::str::from_utf8(bytes).map_err(|_| "text must be UTF-8")?;
    if line.contains(['\n', '\r']) { return Err("text must be one line"); }
    line.split_once(' ').ok_or("expected a verb and its arguments")
}

fn decimal<T: FromStr>(word: &str) -> Checked<T> {
    if word.is_empty() || !word.bytes().all(|b| b.is_ascii_digit()) {
        return Err("expected an unsigned decimal argument");
    }
    word.parse().map_err(|_| "decimal argument out of range")
}

fn pair(args: &str) -> Checked<(&str, &str)> {
    args.split_once(' ').ok_or("expected two arguments")
}

/// A failed turn's reason as the device may be told it: `<code>: <phrase>`, a lowercase code and
/// plain words, never a URL or an address (`://`, `@`, or a dotted or coloned number like an IP).
fn reason(detail: &str) -> Checked<()> {
    let (code, phrase) = detail.split_once(": ").ok_or("error gives `<code>: <phrase>`")?;
    if code.is_empty() || !code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err("an error's code is lowercase letters, digits and -");
    }
    let address = |word: &str| word.contains("://") || word.contains('@')
        || word.split(['.', ':']).filter(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_hexdigit())).count() >= 3;
    if phrase.trim().is_empty() || phrase.split(' ').any(address) { return Err("an error's phrase is plain words, never an address"); }
    Ok(())
}

struct Block {
    turn: u16,
    offset: u32,
    samples: u32,
}

impl Block {
    fn read(bytes: &[u8], magic: u8) -> Checked<Self> {
        let (header, pcm) = bytes.split_at_checked(8).ok_or("expected an eight-byte PCM header")?;
        if header[0] != magic { return Err("wrong PCM magic for this direction"); }
        if header[1] != 0 { return Err("PCM header version must be zero"); }
        if pcm.len() % 2 != 0 { return Err("PCM payload must have an even length"); }
        Ok(Self {
            turn: u16::from_le_bytes([header[2], header[3]]),
            offset: u32::from_le_bytes(header[4..8].try_into().unwrap()),
            samples: (pcm.len() / 2).try_into().map_err(|_| "PCM sample count out of range")?,
        })
    }

    fn advance(&self, count: &mut u32) -> Checked<()> {
        if self.offset != *count { return Err("PCM offset is not contiguous"); }
        *count = count.checked_add(self.samples).ok_or("PCM sample count out of range")?;
        Ok(())
    }
}

impl Checker {
    /// Start a new connection. Its first message is numbered one.
    pub fn ready(&mut self) {
        *self = Self::default();
    }

    /// Check a message from the caller to the hub.
    pub fn up(&mut self, binary: bool, bytes: &[u8]) -> Result<(), String> {
        self.message += 1;
        self.check_up(binary, bytes).map_err(|rule| format!("up message {}: {rule}", self.message))
    }

    /// Check a message from the hub to the caller.
    pub fn down(&mut self, binary: bool, bytes: &[u8]) -> Result<(), String> {
        self.message += 1;
        self.check_down(binary, bytes).map_err(|rule| format!("down message {}: {rule}", self.message))
    }

    fn current(&mut self, id: u16) -> Checked<&mut Turn> {
        match self.terminal.get(&id) {
            Some(Terminal::Finished) => return Err("message after done or error"),
            Some(Terminal::AwaitingDone(_)) => return Err("only done may follow cancel"),
            None => {}
        }
        self.turn.as_mut().filter(|t| t.id == id).ok_or("expected the current turn")
    }

    fn replying(&mut self) -> Checked<&mut Turn> {
        self.turn.as_mut().filter(|t| t.submitted).ok_or("a reply requires a live turn after end")
    }

    fn check_up(&mut self, binary: bool, bytes: &[u8]) -> Checked<()> {
        if !self.hello {
            if binary { return Err("hello must come first after ready"); }
            let (verb, args) = words(bytes)?;
            if verb != "hello" { return Err("hello must come first after ready"); }
            let (version, rest) = pair(args)?;
            let (rate, option) = rest.split_once(' ').map_or((rest, None), |(rate, option)| (rate, Some(option)));
            if version != "1" || decimal::<u32>(rate)? == 0 { return Err("hello requires version 1 and a positive rate"); }
            self.unpaced = match option {
                None => false,
                Some("unpaced") => true,
                Some(_) => return Err("hello's only option is unpaced"),
            };
            self.hello = true;
            return Ok(());
        }
        if binary {
            let block = Block::read(bytes, b'M')?;
            let turn = self.current(block.turn)?;
            if turn.submitted { return Err("microphone block after end"); }
            return block.advance(&mut turn.recorded);
        }
        let (verb, args) = words(bytes)?;
        match verb {
            "hello" => Err("only one hello per ready"),
            "take" => {
                let id = decimal(args)?;
                if self.turn.as_ref().is_some_and(|t| !t.submitted) { return Err("new take before end or cancel"); }
                if self.turn.as_ref().is_some_and(|t| t.id == id) || matches!(self.terminal.get(&id), Some(Terminal::AwaitingDone(_))) {
                    return Err("turn number is still in use");
                }
                self.terminal.remove(&id);
                self.turn = Some(Turn { id, ..Turn::default() });
                Ok(())
            }
            "context" => {
                let (id, frame) = pair(args)?;
                let turn = self.current(decimal(id)?)?;
                if turn.submitted { return Err("context after end"); }
                if turn.context != Context::None { return Err("one context per turn"); }
                if !frame.starts_with('{') { return Err("a context is a JSON object"); }
                turn.context = Context::Sent;
                Ok(())
            }
            "end" => {
                let (id, samples) = pair(args)?;
                let turn = self.current(decimal(id)?)?;
                if turn.submitted { return Err("duplicate end"); }
                if turn.recorded != decimal::<u32>(samples)? { return Err("end count differs from microphone samples"); }
                turn.submitted = true;
                Ok(())
            }
            "cancel" => {
                let id = decimal(args)?;
                if self.terminal.contains_key(&id) { return Ok(()); }
                let sent = self.current(id)?.sent;
                self.terminal.insert(id, Terminal::AwaitingDone(sent));
                self.turn = None;
                Ok(())
            }
            _ => Err("unknown up verb"),
        }
    }

    fn check_down(&mut self, binary: bool, bytes: &[u8]) -> Checked<()> {
        if binary {
            let block = Block::read(bytes, b'A')?;
            let unpaced = self.unpaced;
            let turn = self.current(block.turn)?;
            if !matches!(turn.reply, Reply::Said) { return Err("speech requires a preceding say"); }
            if unpaced {
                let end = turn.speech_end.ok_or("unpaced speech requires its speech line")?;
                if block.offset == turn.sent && turn.sent.saturating_add(block.samples) > end {
                    return Err("speech runs past its announced length");
                }
            }
            block.advance(&mut turn.sent)?;
            if turn.speech_end == Some(turn.sent) { turn.speech_end = None; }
            return Ok(());
        }
        // Unpaced, a sentence's speech is whole before the next line of its turn.
        let open = |turn: &Turn| if turn.speech_end.is_some() { Err("a sentence's speech is incomplete") } else { Ok(()) };
        let (verb, args) = words(bytes)?;
        match verb {
            // A status may come anywhere: `ready` at the route's open or as the answer to hello.
            "status" => match args {
                "ready" | "waiting" | "hearing" | "filtering" | "thinking" | "speaking" | "idle" => Ok(()),
                _ => Err("unknown status word"),
            },
            "heard" => {
                let turn = self.replying()?;
                if !matches!(turn.reply, Reply::AwaitingTranscript) { return Err("heard comes at most once, before any say or speech"); }
                turn.reply = Reply::Heard;
                Ok(())
            }
            "filter" => {
                let (id, verdict) = pair(args)?;
                let turn = self.current(decimal(id)?)?;
                if !matches!(turn.reply, Reply::Heard) || turn.filtered { return Err("a gate's verdict follows heard, once, before any say"); }
                turn.filtered = true;
                match verdict.split_once(' ').map_or(verdict, |(word, _)| word) {
                    "send" | "skip" | "open" => Ok(()),
                    _ => Err("a gate's verdict is send, skip or open"),
                }
            }
            "context" => {
                let (id, counts) = pair(args)?;
                let turn = self.current(decimal(id)?)?;
                if turn.context != Context::Sent { return Err("a context answer follows its frame, once"); }
                let (messages, rest) = pair(counts)?;
                decimal::<u32>(messages)?;
                // A frame with a picture adds whether the model took it: 1 or 0.
                match rest.split_once(' ') {
                    Some((tools, pictures)) => {
                        decimal::<u32>(tools)?;
                        if !matches!(pictures, "0" | "1") { return Err("a context answer's pictures are 0 or 1"); }
                    }
                    None => { decimal::<u32>(rest)?; }
                }
                turn.context = Context::Answered;
                Ok(())
            }
            "call" => {
                let (id, call) = pair(args)?;
                let turn = self.current(decimal(id)?)?;
                if !turn.submitted { return Err("call before end"); }
                open(turn)?;
                if !call.starts_with('{') { return Err("a call is a JSON object"); }
                Ok(())
            }
            "say" => {
                let turn = self.replying()?;
                open(turn)?;
                turn.reply = Reply::Said;
                Ok(())
            }
            "speech" if self.unpaced => {
                let (id, samples) = pair(args)?;
                let turn = self.current(decimal(id)?)?;
                if !turn.submitted { return Err("speech before end"); }
                if !matches!(turn.reply, Reply::Said) { return Err("speech requires a preceding say"); }
                open(turn)?;
                let samples = decimal::<u32>(samples)?;
                if samples == 0 { return Err("a speech line announces at least one sample"); }
                turn.speech_end = Some(turn.sent.checked_add(samples).ok_or("PCM sample count out of range")?);
                Ok(())
            }
            "done" | "error" => {
                let (id, detail) = pair(args)?;
                let id = decimal(id)?;
                let expected = match self.terminal.get(&id) {
                    Some(Terminal::Finished) => return Err("message after done or error"),
                    Some(Terminal::AwaitingDone(count)) if verb == "done" => *count,
                    Some(Terminal::AwaitingDone(_)) => return Err("only done may follow cancel"),
                    None => {
                        let turn = self.current(id)?;
                        if verb == "done" && !turn.submitted { return Err("done before end or cancel"); }
                        if verb == "done" { open(turn)?; }
                        turn.sent
                    }
                };
                if verb == "done" && expected != decimal::<u32>(detail)? { return Err("done count differs from speech samples"); }
                if verb == "error" { reason(detail)?; }
                self.terminal.insert(id, Terminal::Finished);
                if self.turn.as_ref().is_some_and(|t| t.id == id) { self.turn = None; }
                Ok(())
            }
            _ => Err("unknown down verb"),
        }
    }
}
