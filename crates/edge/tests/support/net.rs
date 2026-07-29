//! Bind-availability helpers for edge integration tests.

#![allow(dead_code)]

pub(crate) fn local_listener_bind_available() -> bool {
    let tcp_available = match std::net::TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(_) => true,
    };

    let udp_available = match std::net::UdpSocket::bind(("127.0.0.1", 0)) {
        Ok(socket) => {
            drop(socket);
            true
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(_) => true,
    };

    tcp_available && udp_available
}

pub(crate) fn local_tcp_bind_available() -> bool {
    match std::net::TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(_) => true,
    }
}
