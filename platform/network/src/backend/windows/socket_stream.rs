use makepad_futures_legacy::executor;
use std::{
    io,
    io::{Read, Write},
    time::Duration,
};
use windows::{
    core::Interface,
    Foundation::IClosable,
    Networking::{
        HostName,
        Sockets::{SocketProtectionLevel, StreamSocket},
    },
    Security::Cryptography::Certificates::ChainValidationResult,
    Storage::Streams::{DataReader, DataWriter, InputStreamOptions},
};

fn io_other(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.into())
}

fn close_if_possible<T: Interface>(object: &T) {
    if let Ok(closable) = object.cast::<IClosable>() {
        let _ = closable.Close();
    }
}

pub(crate) struct SocketStream {
    socket: Option<StreamSocket>,
    reader: Option<DataReader>,
    writer: Option<DataWriter>,
}

impl SocketStream {
    pub fn connect(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Self> {
        let host_name = HostName::CreateHostName(&host.into())
            .map_err(|err| io_other(format!("HostName::CreateHostName failed: {err}")))?;
        let service_name = port.into();
        let socket = StreamSocket::new()
            .map_err(|err| io_other(format!("StreamSocket::new failed: {err}")))?;

        if let Ok(control) = socket.Control() {
            let _ = control.SetNoDelay(true);
            if use_tls && ignore_ssl_cert {
                if let Ok(errors) = control.IgnorableServerCertificateErrors() {
                    let _ = errors.Append(ChainValidationResult::Untrusted);
                    let _ = errors.Append(ChainValidationResult::InvalidName);
                    let _ = errors.Append(ChainValidationResult::Expired);
                    let _ = errors.Append(ChainValidationResult::IncompleteChain);
                    let _ = errors.Append(ChainValidationResult::Revoked);
                }
            }
        }

        async fn connect_socket(
            socket: &StreamSocket,
            host_name: &HostName,
            service_name: &windows::core::HSTRING,
            use_tls: bool,
        ) -> windows::core::Result<()> {
            if use_tls {
                socket
                    .ConnectWithProtectionLevelAsync(
                        host_name,
                        service_name,
                        SocketProtectionLevel::Tls12,
                    )?
                    .await?;
            } else {
                socket.ConnectAsync(host_name, service_name)?.await?;
            }
            Ok(())
        }

        executor::block_on(connect_socket(&socket, &host_name, &service_name, use_tls))
            .map_err(|err| io_other(format!("StreamSocket connect failed: {err}")))?;

        let input = socket
            .InputStream()
            .map_err(|err| io_other(format!("StreamSocket::InputStream failed: {err}")))?;
        let output = socket
            .OutputStream()
            .map_err(|err| io_other(format!("StreamSocket::OutputStream failed: {err}")))?;

        let reader = DataReader::CreateDataReader(&input)
            .map_err(|err| io_other(format!("DataReader::CreateDataReader failed: {err}")))?;
        let _ = reader.SetInputStreamOptions(InputStreamOptions::Partial);
        let writer = DataWriter::CreateDataWriter(&output)
            .map_err(|err| io_other(format!("DataWriter::CreateDataWriter failed: {err}")))?;

        Ok(Self {
            socket: Some(socket),
            reader: Some(reader),
            writer: Some(writer),
        })
    }

    pub fn into_tls(self, _host: &str, _ignore_ssl_cert: bool) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "socket stream TLS upgrade is not supported on windows yet",
        ))
    }

    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn shutdown(&mut self) {
        if let Some(writer) = self.writer.take() {
            close_if_possible(&writer);
        }
        if let Some(reader) = self.reader.take() {
            close_if_possible(&reader);
        }
        if let Some(socket) = self.socket.take() {
            close_if_possible(&socket);
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
        async fn load(reader: &DataReader, read_size: u32) -> windows::core::Result<u32> {
            reader.LoadAsync(read_size)?.await
        }
        let bytes_loaded = executor::block_on(load(reader, read_size))
            .map_err(|err| io_other(format!("DataReader::LoadAsync failed: {err}")))?
            as usize;
        if bytes_loaded == 0 {
            return Ok(0);
        }

        reader
            .ReadBytes(&mut buf[..bytes_loaded])
            .map_err(|err| io_other(format!("DataReader::ReadBytes failed: {err}")))?;
        Ok(bytes_loaded)
    }
}

impl Write for SocketStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let Some(writer) = self.writer.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "windows socket stream is closed",
            ));
        };

        writer
            .WriteBytes(buf)
            .map_err(|err| io_other(format!("DataWriter::WriteBytes failed: {err}")))?;
        async fn store(writer: &DataWriter) -> windows::core::Result<u32> {
            writer.StoreAsync()?.await
        }
        let written = executor::block_on(store(writer))
            .map_err(|err| io_other(format!("DataWriter::StoreAsync failed: {err}")))?
            as usize;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let Some(writer) = self.writer.as_ref() else {
            return Ok(());
        };
        async fn flush(writer: &DataWriter) -> windows::core::Result<bool> {
            writer.FlushAsync()?.await
        }
        executor::block_on(flush(writer))
            .map_err(|err| io_other(format!("DataWriter::FlushAsync failed: {err}")))?;
        Ok(())
    }
}
