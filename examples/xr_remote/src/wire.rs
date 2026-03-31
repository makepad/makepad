use crate::protocol::{XR_REMOTE_CONTROL_PORT, XR_REMOTE_MAX_MEDIA_PACKET_BYTES, XR_REMOTE_MEDIA_PORT};
use makepad_widgets::makepad_platform::makepad_micro_serde::{DeBin, SerBin};
use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream, UdpSocket},
    thread,
    time::Duration,
};

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn remote_host() -> String {
    std::env::var("MAKEPAD_XR_REMOTE_HOST").unwrap_or_else(|_| "127.0.0.1".to_string())
}

pub fn control_port() -> u16 {
    std::env::var("MAKEPAD_XR_REMOTE_CONTROL_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(XR_REMOTE_CONTROL_PORT)
}

pub fn media_port() -> u16 {
    std::env::var("MAKEPAD_XR_REMOTE_MEDIA_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(XR_REMOTE_MEDIA_PORT)
}

/// Upper bound for a single framed payload; avoids hostile/corrupt length prefixes.
pub const MAX_FRAMED_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

pub fn send_framed<T: SerBin>(stream: &mut TcpStream, packet: &T) -> io::Result<()> {
    let payload = packet.serialize_bin();
    let len = (payload.len() as u32).to_be_bytes();
    stream.write_all(&len)?;
    stream.write_all(&payload)?;
    Ok(())
}

pub fn recv_framed<T: DeBin>(stream: &mut TcpStream) -> io::Result<T> {
    let mut len_bytes = [0u8; 4];
    stream.read_exact(&mut len_bytes)?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len > MAX_FRAMED_PAYLOAD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "framed packet length exceeds cap",
        ));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;
    T::deserialize_bin(&payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid packet"))
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn recv_udp_packet<T: DeBin>(socket: &UdpSocket, buffer: &mut [u8]) -> io::Result<T> {
    let (len, _) = socket.recv_from(buffer)?;
    T::deserialize_bin(&buffer[..len])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid udp packet"))
}

pub fn max_media_packet_bytes() -> usize {
    XR_REMOTE_MAX_MEDIA_PACKET_BYTES
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn connect_with_retry(addr: &str) -> TcpStream {
    loop {
        match TcpStream::connect(addr) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                return stream;
            }
            Err(err) => {
                eprintln!("xr_remote connect retry addr={addr} err={err}");
                thread::sleep(Duration::from_millis(1000));
            }
        }
    }
}

pub fn bind_listener(port: u16) -> TcpListener {
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).unwrap_or_else(|err| {
        panic!("xr_remote failed to bind {addr}: {err}");
    });
    let _ = listener.set_nonblocking(false);
    listener
}

pub fn bind_udp_socket(port: u16) -> UdpSocket {
    let addr = format!("0.0.0.0:{port}");
    let socket = UdpSocket::bind(&addr).unwrap_or_else(|err| {
        panic!("xr_remote failed to bind udp {addr}: {err}");
    });
    let _ = socket.set_nonblocking(false);
    socket
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn bind_udp_socket_any() -> UdpSocket {
    bind_udp_socket(0)
}
