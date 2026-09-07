struct PendingBytes {
    bytes: Arc<Vec<u8>>,
    offset: usize,
}
struct Peer {
    stream: UnixStream,
    input: Vec<u8>,
    output: VecDeque<PendingBytes>,
    queued: usize,
    started: Instant,
    attached: bool,
    read_only: bool,
    cols: u16,
    rows: u16,
    projection: Projection,
    generation: u64,
    dirty: bool,
    closing: bool,
}
impl Peer {
    fn queue(&mut self, kind: u8, bytes: &[u8]) -> Result<(), String> {
        let chunks = bytes.len().div_ceil(MAX_FRAME).max(1);
        if self
            .queued
            .saturating_add(bytes.len())
            .saturating_add(chunks * 5)
            > MAX_QUEUE
        {
            return Err("Screen client is too slow; reconnect for current state".into());
        }
        if bytes.is_empty() {
            let encoded = encode_frame(kind, bytes)?;
            self.queued += encoded.len();
            self.output.push_back(PendingBytes {
                bytes: Arc::new(encoded),
                offset: 0,
            });
        }
        for chunk in bytes.chunks(MAX_FRAME) {
            let encoded = encode_frame(kind, chunk)?;
            self.queued += encoded.len();
            self.output.push_back(PendingBytes {
                bytes: Arc::new(encoded),
                offset: 0,
            });
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), String> {
        let mut budget = 256 * 1024;
        while budget > 0 {
            let Some(front) = self.output.front_mut() else {
                break;
            };
            let end = (front.offset + budget).min(front.bytes.len());
            match self.stream.write(&front.bytes[front.offset..end]) {
                Ok(0) => return Err("Screen client closed".into()),
                Ok(n) => {
                    front.offset += n;
                    self.queued -= n;
                    budget -= n;
                    if front.offset == front.bytes.len() {
                        self.output.pop_front();
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(error(e)),
            }
        }
        Ok(())
    }
}
struct Host {
    location: SessionLocation,
    options: StartOptions,
    instance: String,
    terminal: HostedTerminal,
    pty: Pty,
    peers: Vec<Peer>,
    input: VecDeque<u8>,
    generation: u64,
    client_count: usize,
    stopping: Option<Instant>,
    killed: bool,
}
impl Host {
    fn status(&self) -> Value {
        json::obj(vec![
            ("running", Value::Bool(true)),
            ("version", Value::Int(VERSION as i64)),
            ("session_id", json::s(&self.location.session_id)),
            ("pid", Value::Int(std::process::id().into())),
            ("child_pid", Value::Int(self.pty.child_pid().into())),
            ("cols", Value::Int(self.terminal.cols() as i64)),
            ("rows", Value::Int(self.terminal.rows() as i64)),
            ("clients", Value::Int(self.client_count as i64)),
            ("cwd", json::s(self.options.cwd.to_string_lossy())),
            ("program", json::s(self.options.program.to_string_lossy())),
            ("instance", json::s(&self.instance)),
        ])
    }
    fn replies(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        if self.input.len().saturating_add(bytes.len()) > MAX_INPUT {
            return Err("Screen PTY input/reply backlog exceeded its bound".into());
        }
        self.input.extend(bytes);
        Ok(())
    }
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        self.pty.resize(cols, rows).map_err(error)?;
        self.generation = self.generation.wrapping_add(1);
        let update = self.terminal.resize(cols as usize, rows as usize);
        self.replies(update.replies)?;
        for peer in &mut self.peers {
            peer.dirty = true;
        }
        Ok(())
    }
    fn begin_stop(&mut self) -> Result<(), String> {
        if self.stopping.is_none() {
            self.pty.terminate_group(false).map_err(error)?;
            self.stopping = Some(Instant::now());
        }
        Ok(())
    }
    fn frame(&mut self, peer: &mut Peer, frame: Frame) -> Result<(), String> {
        match frame.kind {
            HELLO if !peer.attached => {
                let hello = parse_object(&frame.payload)?;
                if hello.get("version").and_then(Value::as_u64) != Some(VERSION)
                    || hello.get("session_id").and_then(Value::as_str)
                        != Some(&self.location.session_id)
                {
                    return Err("Screen hello version/session mismatch".into());
                }
                let (cols, rows) = dimensions(&hello)?;
                let read_only = hello
                    .get("read_only")
                    .and_then(Value::as_bool)
                    .ok_or("Screen hello requires read_only")?;
                peer.cols = cols;
                peer.rows = rows;
                peer.read_only = read_only;
                peer.attached = true;
                self.client_count += 1;
                if !read_only {
                    self.resize(cols, rows)?;
                }
                peer.projection.invalidate();
                let snapshot =
                    self.terminal
                        .render(&mut peer.projection, cols as usize, rows as usize);
                if let Some(error) = &peer.projection.error {
                    return Err(error.clone());
                }
                peer.queue(SNAPSHOT, &snapshot)?;
                peer.dirty = false;
                peer.generation = self.generation;
            }
            INPUT if peer.attached && !peer.read_only && self.stopping.is_none() => {
                if frame.payload.len() > 64 * 1024 {
                    return Err("Screen input exceeds 64 KiB".into());
                }
                if self.input.len().saturating_add(frame.payload.len()) > MAX_INPUT {
                    return Err("Screen input is busy; reconnect later".into());
                }
                self.input.extend(frame.payload);
            }
            RESIZE if peer.attached => {
                let (cols, rows) = dimensions(&parse_object(&frame.payload)?)?;
                peer.cols = cols;
                peer.rows = rows;
                peer.projection.invalidate();
                peer.dirty = true;
                self.resize(cols, rows)?;
            }
            STATUS => {
                parse_object(&frame.payload)?;
                peer.queue(STATUS_REPLY, self.status().to_json().as_bytes())?;
                if !peer.attached {
                    peer.closing = true;
                }
            }
            STOP if !peer.read_only => {
                let request = parse_object(&frame.payload)?;
                if request
                    .get("instance")
                    .is_some_and(|value| value.as_str() != Some(&self.instance))
                {
                    return Err("Session changed since selection; stop cancelled".into());
                }
                self.begin_stop()?;
                peer.queue(STATUS_REPLY, b"{\"stopping\":true}")?;
                if !peer.attached {
                    peer.closing = true;
                }
            }
            TEXT => {
                let value = parse_object(&frame.payload)?;
                let lines = value
                    .get("lines")
                    .and_then(Value::as_u64)
                    .filter(|n| *n > 0 && *n <= 5000)
                    .ok_or("Screen text lines must be 1..5000")?;
                let text = self.terminal.text_tail(lines as usize);
                if text.len() > MAX_FRAME {
                    return Err("Screen text is too large; request fewer lines".into());
                }
                peer.queue(TEXT_REPLY, text.as_bytes())?;
                if !peer.attached {
                    peer.closing = true;
                }
            }
            _ => return Err("Unsupported screen request or read-only input".into()),
        }
        Ok(())
    }
    fn tick_peers(&mut self) {
        let mut peers = std::mem::take(&mut self.peers);
        for mut peer in peers.drain(..) {
            if !peer.attached && peer.started.elapsed() > Duration::from_secs(5) {
                continue;
            }
            if peer.flush().is_err() || (peer.closing && peer.output.is_empty()) {
                continue;
            }
            let result = (|| {
                if !peer.closing {
                    let mut buffer = [0u8; 16384];
                    let mut eof = false;
                    for _ in 0..4 {
                        match peer.stream.read(&mut buffer) {
                            Ok(0) => {
                                eof = true;
                                break;
                            }
                            Ok(n) => peer.input.extend_from_slice(&buffer[..n]),
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(e) => return Err(error(e)),
                        }
                        if peer.input.len() > MAX_FRAME + 5 {
                            return Err("Screen client input exceeds its frame bound".into());
                        }
                    }
                    for _ in 0..8 {
                        let Some(frame) = decode_frame(&mut peer.input)? else {
                            break;
                        };
                        self.frame(&mut peer, frame)?;
                        if peer.closing {
                            break;
                        }
                    }
                    if eof {
                        peer.closing = true;
                    }
                }
                if peer.generation != self.generation {
                    peer.dirty = true;
                }
                if peer.attached && peer.dirty && peer.queued < 2 * 1024 * 1024 {
                    let output = self.terminal.render(
                        &mut peer.projection,
                        peer.cols as usize,
                        peer.rows as usize,
                    );
                    if let Some(error) = &peer.projection.error {
                        return Err(error.clone());
                    }
                    if !output.is_empty() {
                        peer.queue(OUTPUT, &output)?;
                    }
                    peer.dirty = false;
                    peer.generation = self.generation;
                }
                peer.flush()
            })();
            if let Err(message) = result {
                let _ = peer.queue(ERROR, message.as_bytes());
                let _ = peer.flush();
                continue;
            }
            self.peers.push(peer);
        }
        self.client_count = self
            .peers
            .iter()
            .filter(|peer| peer.attached && !peer.closing)
            .count();
    }
    fn run(&mut self, listener: UnixListener) -> Result<Option<i32>, String> {
        let signals = unix::SignalGuard::new().map_err(error)?;
        let mut exited = None;
        let mut exit_at = None;
        let mut last_bytes = Instant::now();
        loop {
            if signals.interrupted().is_some() {
                self.begin_stop()?;
            }
            for _ in 0..4 {
                let (stream, _) = match listener.accept() {
                    Ok(peer) => peer,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(error(e)),
                };
                if self.peers.len() >= MAX_CLIENTS
                    || unix::peer_uid(&stream).map_err(error)? != unix::current_uid()
                {
                    continue;
                }
                stream.set_nonblocking(true).map_err(error)?;
                self.peers.push(Peer {
                    stream,
                    input: Vec::new(),
                    output: VecDeque::new(),
                    queued: 0,
                    started: Instant::now(),
                    attached: false,
                    read_only: false,
                    cols: self.terminal.cols() as u16,
                    rows: self.terminal.rows() as u16,
                    projection: Projection::default(),
                    generation: self.generation,
                    dirty: true,
                    closing: false,
                });
            }
            let mut buffer = [0u8; 16384];
            for _ in 0..8 {
                match self.pty.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let update = self.terminal.process(&buffer[..n]);
                        self.replies(update.replies)?;
                        for peer in &mut self.peers {
                            peer.dirty = true;
                        }
                        last_bytes = Instant::now();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(error(e)),
                }
            }
            for _ in 0..4 {
                let data = self.input.make_contiguous();
                if data.is_empty() {
                    break;
                }
                match self.pty.write(&data[..data.len().min(16384)]) {
                    Ok(0) => break,
                    Ok(n) => {
                        self.input.drain(..n);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::BrokenPipe
                            || matches!(e.raw_os_error(), Some(5 | 32)) =>
                    {
                        self.input.clear();
                        break;
                    }
                    Err(e) => return Err(error(e)),
                }
            }
            self.tick_peers();
            if let Some(code) = self.pty.try_wait().map_err(error)? {
                if exit_at.is_none() {
                    exited = Some(code);
                    exit_at = Some(Instant::now());
                }
            }
            if self
                .stopping
                .is_some_and(|at| at.elapsed() > Duration::from_secs(3))
                && !self.killed
            {
                self.pty.terminate_group(true).map_err(error)?;
                self.killed = true;
            }
            if exit_at.is_some_and(|at| {
                (at.elapsed() > Duration::from_millis(100)
                    && last_bytes.elapsed() > Duration::from_millis(100))
                    || at.elapsed() > Duration::from_secs(1)
            }) {
                break;
            }
            if self
                .stopping
                .is_some_and(|at| at.elapsed() > Duration::from_secs(6))
            {
                return Err("Owned screen child did not exit after bounded termination".into());
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        let exit = json::obj(vec![
            (
                "code",
                exited
                    .map(|code| Value::Int(code.into()))
                    .unwrap_or(Value::Null),
            ),
            (
                "reason",
                json::s(if self.stopping.is_some() {
                    "stopped"
                } else {
                    "child_exited"
                }),
            ),
        ])
        .to_json();
        for peer in &mut self.peers {
            let _ = peer.queue(EXIT, exit.as_bytes());
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline && self.peers.iter().any(|peer| !peer.output.is_empty()) {
            self.peers.retain_mut(|peer| peer.flush().is_ok());
            std::thread::sleep(Duration::from_millis(8));
        }
        Ok(exited)
    }
}
