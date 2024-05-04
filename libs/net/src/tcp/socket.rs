use std::{
    cell::UnsafeCell,
    error::Error,
    io::{Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex}
};
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};

struct UnsafeMutator<T> {
    value: UnsafeCell<T>,
}

impl<T> UnsafeMutator<T> {
    fn new(value: T) -> UnsafeMutator<T> {
        return UnsafeMutator {
            value: UnsafeCell::new(value),
        };
    }

    fn mut_value(&self) -> &mut T {
        return unsafe { &mut *self.value.get() };
    }
}

unsafe impl<T> Sync for UnsafeMutator<T> {}

struct ReadWrapper<R>
    where
        R: Read,
{
    inner: Arc<UnsafeMutator<R>>,
}

impl<R: Read> Read for ReadWrapper<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        return self.inner.mut_value().read(buf);
    }
}

struct WriteWrapper<W>
    where
        W: Write,
{
    inner: Arc<UnsafeMutator<W>>,
}

impl<W: Write> Write for WriteWrapper<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        return self.inner.mut_value().write(buf);
    }

    fn flush(&mut self) -> Result<(), std::io::Error> {
        return self.inner.mut_value().flush();
    }
}

pub struct Socket {
    pub output_stream: Arc<Mutex<dyn Write + Send>>,
    pub input_stream: Arc<Mutex<dyn Read + Send>>,
}

impl Socket {
    pub fn bind(host: &str, port: u16, secure: bool) -> Result<Socket, Box<dyn Error>> {
        let tcp_stream = match TcpStream::connect((host, port)) {
            Ok(x) => x,
            Err(e) => return Err(Box::new(e)),
        };
        if secure {
            //        let tls_connector = TlsConnector::builder().build().unwrap();
            let mut tls_connector_builder = SslConnector::builder(SslMethod::tls()).unwrap();
            tls_connector_builder.set_verify(SslVerifyMode::NONE); // Disable certificate verification
            let tls_connector = tls_connector_builder.build();

            let tls_stream = match tls_connector.connect(host, tcp_stream) {
                Ok(x) => x,
                Err(e) => return Err(Box::new(e)),
            };
            let mutator = Arc::new(UnsafeMutator::new(tls_stream));
            let input_stream = Arc::new(Mutex::new(ReadWrapper {
                inner: mutator.clone(),
            }));
            let output_stream = Arc::new(Mutex::new(WriteWrapper { inner: mutator }));

            let socket = Socket {
                output_stream,
                input_stream,
            };
            return Ok(socket);
        } else {
            let mutator = Arc::new(UnsafeMutator::new(tcp_stream));
            let input_stream = Arc::new(Mutex::new(ReadWrapper {
                inner: mutator.clone(),
            }));
            let output_stream = Arc::new(Mutex::new(WriteWrapper {
                inner: mutator
            }));
            let socket = Socket {
                output_stream,
                input_stream,
            };
            return Ok(socket);
        }
    }
}
