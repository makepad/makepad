use std::{
    io,
    io::{Read, Write},
    sync::mpsc,
    time::Duration,
};
use windows::{
    core::{HRESULT, HSTRING},
    WinRT::*,
};

fn io_other(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.into())
}

// These synchronous socket methods run on networking workers. The completion
// delegate wakes that worker; no executor, polling, or UI-thread wait is needed.
struct Completed(mpsc::Sender<()>);
impl ActionCompletedImpl for Completed {
    fn Invoke(&self, _: Option<&AsyncAction>, _: AsyncStatus) -> Result<(), HRESULT> {
        let _ = self.0.send(());
        Ok(())
    }
}
impl UInt32CompletedImpl for Completed {
    fn Invoke(&self, _: Option<&UInt32Operation>, _: AsyncStatus) -> Result<(), HRESULT> {
        let _ = self.0.send(());
        Ok(())
    }
}
impl BoolCompletedImpl for Completed {
    fn Invoke(&self, _: Option<&BoolOperation>, _: AsyncStatus) -> Result<(), HRESULT> {
        let _ = self.0.send(());
        Ok(())
    }
}
unsafe fn wait_action(action: &AsyncAction) -> Result<(), HRESULT> {
    let (tx, rx) = mpsc::channel();
    action.put_Completed(&ActionCompleted::implement(Box::new(Completed(tx))))?;
    rx.recv().map_err(|_| HRESULT(0x80004005u32 as i32))?;
    action.GetResults()
}
unsafe fn wait_count(operation: &UInt32Operation) -> Result<u32, HRESULT> {
    let (tx, rx) = mpsc::channel();
    operation.put_Completed(&UInt32Completed::implement(Box::new(Completed(tx))))?;
    rx.recv().map_err(|_| HRESULT(0x80004005u32 as i32))?;
    operation.GetResults()
}
unsafe fn wait_bool(operation: &BoolOperation) -> Result<bool, HRESULT> {
    let (tx, rx) = mpsc::channel();
    operation.put_Completed(&BoolCompleted::implement(Box::new(Completed(tx))))?;
    rx.recv().map_err(|_| HRESULT(0x80004005u32 as i32))?;
    operation.GetResults()
}

pub(crate) struct SocketStream {
    socket: Option<StreamSocket>,
    reader: Option<DataReader>,
    writer: Option<DataWriter>,
    /// Pinned connections run on Schannel directly.
    schannel: Option<crate::tls::platform::ServerStream>,
}

// StreamSocket, DataReader and DataWriter are agile WinRT runtime classes.
// Ownership moves to a networking worker; methods require exclusive I/O access.
unsafe impl Send for SocketStream {}

impl SocketStream {
    pub fn connect(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Self> {
        let (socket, reader, writer) = unsafe {
            // The WinRT socket is agile and used only by the networking workers.
            let host_name = HostNameFactory::factory("Windows.Networking.HostName")
                .and_then(|factory| factory.CreateHostName(&HSTRING::from_str(host)?))
                .map_err(|err| io_other(format!("HostName failed: {err}")))?;
            let service_name =
                HSTRING::from_str(port).map_err(|err| io_other(format!("service name: {err}")))?;
            let socket = StreamSocket::activate("Windows.Networking.Sockets.StreamSocket")
                .map_err(|err| io_other(format!("StreamSocket failed: {err}")))?;
            if let Ok(control) = socket.get_Control() {
                let _ = control.put_NoDelay(true);
                if use_tls && ignore_ssl_cert {
                    if let Ok(errors) = StreamSocketControl2::query(control.as_raw())
                        .and_then(|control| control.get_IgnorableServerCertificateErrors())
                    {
                        for error in [
                            CHAIN_VALIDATION_RESULT_UNTRUSTED,
                            CHAIN_VALIDATION_RESULT_INVALID_NAME,
                            CHAIN_VALIDATION_RESULT_EXPIRED,
                            CHAIN_VALIDATION_RESULT_INCOMPLETE_CHAIN,
                            CHAIN_VALIDATION_RESULT_REVOKED,
                            CHAIN_VALIDATION_RESULT_WRONG_USAGE,
                            CHAIN_VALIDATION_RESULT_BASIC_CONSTRAINTS_ERROR,
                        ] {
                            let _ = errors.Append(error);
                        }
                    }
                }
            }
            let connect = if use_tls {
                socket.ConnectWithProtectionLevelAsync(
                    &host_name,
                    &service_name,
                    SOCKET_PROTECTION_LEVEL_TLS12,
                )
            } else {
                socket.ConnectAsync(&host_name, &service_name)
            }
            .map_err(|err| io_other(format!("StreamSocket connect: {err}")))?;
            wait_action(&connect)
                .map_err(|err| io_other(format!("StreamSocket connect: {err}")))?;
            let input = socket
                .get_InputStream()
                .map_err(|err| io_other(format!("InputStream: {err}")))?;
            let output = socket
                .get_OutputStream()
                .map_err(|err| io_other(format!("OutputStream: {err}")))?;
            let reader = DataReaderFactory::factory("Windows.Storage.Streams.DataReader")
                .and_then(|factory| factory.CreateDataReader(&input))
                .map_err(|err| io_other(format!("DataReader: {err}")))?;
            let _ = reader.put_InputStreamOptions(INPUT_STREAM_OPTIONS_PARTIAL);
            let writer = DataWriterFactory::factory("Windows.Storage.Streams.DataWriter")
                .and_then(|factory| factory.CreateDataWriter(&output))
                .map_err(|err| io_other(format!("DataWriter: {err}")))?;
            (socket, reader, writer)
        };

        Ok(Self {
            socket: Some(socket),
            reader: Some(reader),
            writer: Some(writer),
            schannel: None,
        })
    }

    /// TLS on Schannel without chain validation (self-signed server); returns
    /// the server certificate's SHA-256. Synchronous Schannel rather than
    /// WinRT: a WinRT socket cannot safely issue a second blocking operation
    /// on the same thread.
    pub fn connect_capture(host: &str, port: &str) -> io::Result<(Self, [u8; 32])> {
        use std::net::ToSocketAddrs;
        let port: u16 = port.parse().map_err(|_| io_other("bad port"))?;
        let mut last = None;
        for addr in (host, port).to_socket_addrs()? {
            match std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
                Ok(tcp) => {
                    let _ = tcp.set_nodelay(true);
                    tcp.set_read_timeout(Some(Duration::from_secs(20)))?;
                    let (stream, fp) = crate::tls::platform::connect_capture(tcp, host)?;
                    stream.tcp().set_read_timeout(None)?;
                    return Ok((Self { socket: None, reader: None, writer: None, schannel: Some(stream) }, fp));
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| io_other("no address")))
    }

    pub fn into_tls(self, _host: &str, _ignore_ssl_cert: bool) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "socket stream TLS upgrade is not supported on windows yet",
        ))
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match &self.schannel {
            Some(s) => s.tcp().set_read_timeout(timeout),
            None => Ok(()),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match &self.schannel {
            Some(s) => s.tcp().set_write_timeout(timeout),
            None => Ok(()),
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(s) = self.schannel.as_mut() {
            s.shutdown();
        }
        if let Some(writer) = self.writer.take() {
            unsafe {
                if let Ok(closable) = Closable::query(writer.as_raw()) {
                    let _ = closable.Close();
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            unsafe {
                if let Ok(closable) = Closable::query(reader.as_raw()) {
                    let _ = closable.Close();
                }
            }
        }
        if let Some(socket) = self.socket.take() {
            unsafe {
                if let Ok(closable) = Closable::query(socket.as_raw()) {
                    let _ = closable.Close();
                }
            }
        }
    }
}

impl Drop for SocketStream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Read for SocketStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(s) = self.schannel.as_mut() {
            return s.read(buf);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let Some(reader) = self.reader.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "windows socket stream is closed",
            ));
        };

        let read_size = buf.len().min(u32::MAX as usize) as u32;
        let bytes_loaded = unsafe {
            reader
                .LoadAsync(read_size)
                .and_then(|operation| wait_count(&operation))
        }
        .map_err(|err| io_other(format!("DataReader::LoadAsync failed: {err}")))?
            as usize;
        if bytes_loaded == 0 {
            return Ok(0);
        }

        unsafe { reader.ReadBytes(&mut buf[..bytes_loaded]) }
            .map_err(|err| io_other(format!("DataReader::ReadBytes failed: {err}")))?;
        Ok(bytes_loaded)
    }
}

impl Write for SocketStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(s) = self.schannel.as_mut() {
            return s.write(buf);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let Some(writer) = self.writer.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "windows socket stream is closed",
            ));
        };

        unsafe { writer.WriteBytes(buf) }
            .map_err(|err| io_other(format!("DataWriter::WriteBytes failed: {err}")))?;
        let written = unsafe {
            writer
                .StoreAsync()
                .and_then(|operation| wait_count(&operation))
        }
        .map_err(|err| io_other(format!("DataWriter::StoreAsync failed: {err}")))?
            as usize;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(s) = self.schannel.as_mut() {
            return s.flush();
        }
        let Some(writer) = self.writer.as_ref() else {
            return Ok(());
        };
        unsafe {
            writer
                .FlushAsync()
                .and_then(|operation| wait_bool(&operation))
        }
        .map_err(|err| io_other(format!("DataWriter::FlushAsync failed: {err}")))?;
        Ok(())
    }
}
