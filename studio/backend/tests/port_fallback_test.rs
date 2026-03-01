use makepad_studio_backend::{BackendConfig, StudioBackend};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};

fn find_occupied_port_below_max() -> (TcpListener, u16) {
    loop {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind temporary listener");
        let port = listener.local_addr().expect("listener local addr").port();
        if port < u16::MAX {
            return (listener, port);
        }
    }
}

#[test]
fn headless_backend_falls_back_to_higher_port_when_requested_port_is_busy() {
    let (_busy_listener, busy_port) = find_occupied_port_below_max();
    let config = BackendConfig {
        listen_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), busy_port),
        enable_in_process_gateway: false,
        ..Default::default()
    };

    let backend =
        StudioBackend::start_headless(config).expect("headless backend should bind with fallback");
    assert_eq!(backend.listen_address.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert!(
        backend.listen_address.port() > busy_port,
        "expected fallback port > {}, got {}",
        busy_port,
        backend.listen_address.port()
    );

    let mut stream =
        TcpStream::connect(backend.listen_address).expect("connect to fallback backend port");
    stream
        .write_all(b"GET /$studio_health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .expect("write health request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read health response");
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "unexpected health response: {response}"
    );
}
