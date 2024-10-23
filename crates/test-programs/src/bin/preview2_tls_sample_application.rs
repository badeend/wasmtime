use core::str;

use test_programs::wasi::sockets::network::{IpSocketAddress, Network};
use test_programs::wasi::sockets::tcp::{ShutdownType, TcpSocket};
use test_programs::wasi::sockets::tls;

fn test_tls_sample_application() {
    const PORT: u16 = 443;
    const DOMAIN: &'static str = "example.com";

    let request = format!("GET / HTTP/1.1\r\nHost: {DOMAIN}\r\n\r\n");

    let net = Network::default();

    let Some(ip) = net
        .permissive_blocking_resolve_addresses(DOMAIN)
        .unwrap()
        .first()
        .map(|a| a.to_owned())
    else {
        eprintln!("DNS lookup failed.");
        return;
    };

    let socket = TcpSocket::new(ip.family()).unwrap();
    let (tcp_input, tcp_output) = socket
        .blocking_connect(&net, IpSocketAddress::new(ip, PORT))
        .unwrap();

    let (tls_client, tls_input, tls_output) =
        tls::ClientHandshake::new(DOMAIN, tcp_input, tcp_output)
            .blocking_finish()
            .unwrap();

    assert!(tls_client.protocol_version() >= 0x0301);
    assert!(tls_client.cipher_suite() != 0);

    tls_output.blocking_write_util(request.as_bytes()).unwrap();
    tls_client.blocking_close_notify(&tls_output).unwrap();
    socket.shutdown(ShutdownType::Send).unwrap();
    let response = tls_input.blocking_read_to_end().unwrap();
    let response = String::from_utf8(response).unwrap();

    assert!(response.contains("HTTP/1.1 200 OK"));
}

fn main() {
    test_tls_sample_application();
}
