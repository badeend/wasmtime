use wasi::sockets::network::{ErrorCode, IpAddressFamily};
use wasi::sockets::tcp;
use wasi_sockets_tests::*;

fn test_tcp_unbound_state_invariants(family: IpAddressFamily) {
    let sock = TcpSock::new(family).unwrap();

    // Skipping: tcp::start_bind
    assert!(matches!(
        tcp::finish_bind(sock.fd),
        Err(ErrorCode::NotInProgress)
    ));
    // Skipping: tcp::start_connect
    assert!(matches!(
        tcp::finish_connect(sock.fd),
        Err(ErrorCode::NotInProgress)
    ));
    assert!(matches!(
        tcp::start_listen(sock.fd),
        Err(ErrorCode::NotBound)
    ));
    assert!(matches!(
        tcp::finish_listen(sock.fd),
        Err(ErrorCode::NotInProgress)
    ));
    assert!(matches!(tcp::accept(sock.fd), Err(ErrorCode::NotListening)));
    assert!(matches!(
        tcp::shutdown(sock.fd, tcp::ShutdownType::Both),
        Err(ErrorCode::NotConnected)
    ));

    assert!(matches!(
        tcp::local_address(sock.fd),
        Err(ErrorCode::NotBound)
    ));
    assert!(matches!(
        tcp::remote_address(sock.fd),
        Err(ErrorCode::NotConnected)
    ));
    assert_eq!(tcp::address_family(sock.fd), family);

    if family == IpAddressFamily::Ipv6 {
        assert!(matches!(tcp::ipv6_only(sock.fd), Ok(_)));
        assert!(matches!(tcp::set_ipv6_only(sock.fd, true), Ok(_)));
    } else {
        assert!(matches!(
            tcp::ipv6_only(sock.fd),
            Err(ErrorCode::Ipv6OnlyOperation)
        ));
        assert!(matches!(
            tcp::set_ipv6_only(sock.fd, true),
            Err(ErrorCode::Ipv6OnlyOperation)
        ));
    }

    // assert!(matches!(tcp::set_listen_backlog_size(sock.fd, 32), Ok(_))); // FIXME
    assert!(matches!(tcp::keep_alive(sock.fd), Ok(_)));
    assert!(matches!(tcp::set_keep_alive(sock.fd, false), Ok(_)));
    assert!(matches!(tcp::no_delay(sock.fd), Ok(_)));
    assert!(matches!(tcp::set_no_delay(sock.fd, false), Ok(_)));
    assert!(matches!(tcp::unicast_hop_limit(sock.fd), Ok(_)));
    assert!(matches!(tcp::set_unicast_hop_limit(sock.fd, 255), Ok(_)));
    assert!(matches!(tcp::receive_buffer_size(sock.fd), Ok(_)));
    assert!(matches!(
        tcp::set_receive_buffer_size(sock.fd, 16000),
        Ok(_)
    ));
    assert!(matches!(tcp::send_buffer_size(sock.fd), Ok(_)));
    assert!(matches!(tcp::set_send_buffer_size(sock.fd, 16000), Ok(_)));
}

fn main() {
    test_tcp_unbound_state_invariants(IpAddressFamily::Ipv4);
    test_tcp_unbound_state_invariants(IpAddressFamily::Ipv6);
}
