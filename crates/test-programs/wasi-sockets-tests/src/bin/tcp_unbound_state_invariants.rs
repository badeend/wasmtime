use wasi::sockets::network::{ErrorCode, IpAddressFamily};
use wasi::sockets::{tcp, tcp_create_socket};
use wasi_sockets_tests::*;

fn test_tcp_unbound_state_invariants(sock: tcp::TcpSocket, family: IpAddressFamily) {
    // Skipping: tcp::start_bind
    assert!(matches!(
        tcp::finish_bind(sock),
        Err(ErrorCode::NotInProgress)
    ));
    // Skipping: tcp::start_connect
    assert!(matches!(
        tcp::finish_connect(sock),
        Err(ErrorCode::NotInProgress)
    ));
    assert!(matches!(tcp::start_listen(sock), Err(ErrorCode::NotBound)));
    assert!(matches!(
        tcp::finish_listen(sock),
        Err(ErrorCode::NotInProgress)
    ));
    assert!(matches!(tcp::accept(sock), Err(ErrorCode::NotListening)));
    assert!(matches!(
        tcp::shutdown(sock, tcp::ShutdownType::Both),
        Err(ErrorCode::NotConnected)
    ));

    assert!(matches!(tcp::local_address(sock), Err(ErrorCode::NotBound)));
    assert!(matches!(
        tcp::remote_address(sock),
        Err(ErrorCode::NotConnected)
    ));
    assert_eq!(tcp::address_family(sock), family);

    if family == IpAddressFamily::Ipv6 {
        assert!(matches!(tcp::ipv6_only(sock), Ok(_)));
        assert!(matches!(tcp::set_ipv6_only(sock, true), Ok(_)));
    } else {
        assert!(matches!(
            tcp::ipv6_only(sock),
            Err(ErrorCode::Ipv6OnlyOperation)
        ));
        assert!(matches!(
            tcp::set_ipv6_only(sock, true),
            Err(ErrorCode::Ipv6OnlyOperation)
        ));
    }

    // assert!(matches!(tcp::set_listen_backlog_size(sock, 32), Ok(_))); // FIXME
    assert!(matches!(tcp::keep_alive(sock), Ok(_)));
    assert!(matches!(tcp::set_keep_alive(sock, false), Ok(_)));
    assert!(matches!(tcp::no_delay(sock), Ok(_)));
    assert!(matches!(tcp::set_no_delay(sock, false), Ok(_)));
    assert!(matches!(tcp::unicast_hop_limit(sock), Ok(_)));
    assert!(matches!(tcp::set_unicast_hop_limit(sock, 255), Ok(_)));
    assert!(matches!(tcp::receive_buffer_size(sock), Ok(_)));
    assert!(matches!(tcp::set_receive_buffer_size(sock, 16000), Ok(_)));
    assert!(matches!(tcp::send_buffer_size(sock), Ok(_)));
    assert!(matches!(tcp::set_send_buffer_size(sock, 16000), Ok(_)));
}

fn main() {
    test_tcp_unbound_state_invariants(
        tcp_create_socket::create_tcp_socket(IpAddressFamily::Ipv4).unwrap(),
        IpAddressFamily::Ipv4,
    );
    test_tcp_unbound_state_invariants(
        tcp_create_socket::create_tcp_socket(IpAddressFamily::Ipv6).unwrap(),
        IpAddressFamily::Ipv6,
    );
}
