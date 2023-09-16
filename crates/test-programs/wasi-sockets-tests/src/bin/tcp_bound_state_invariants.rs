use wasi::sockets::network::{self, ErrorCode, IpAddress, IpAddressFamily, IpSocketAddress};
use wasi::sockets::{instance_network, tcp};
use wasi_sockets_tests::*;

fn test_tcp_bound_state_invariants(net: tcp::Network, family: IpAddressFamily) {
    let bind_address = IpSocketAddress::new(IpAddress::new_loopback(family), 0);
    let sock = TcpSock::new(family).unwrap();
    sock.bind(net, bind_address).unwrap();



    assert!(matches!(
        tcp::start_bind(sock.fd, net, bind_address),
        Err(ErrorCode::InvalidState)
    ));
    assert!(matches!(
        tcp::finish_bind(sock.fd),
        Err(ErrorCode::NotInProgress)
    ));
    // Skipping: tcp::start_connect
    assert!(matches!(
        tcp::finish_connect(sock.fd),
        Err(ErrorCode::NotInProgress)
    ));
    // Skipping: tcp::start_listen
    assert!(matches!(
        tcp::finish_listen(sock.fd),
        Err(ErrorCode::NotInProgress)
    ));
    assert!(matches!(tcp::accept(sock.fd), Err(ErrorCode::InvalidState)));
    assert!(matches!(
        tcp::shutdown(sock.fd, tcp::ShutdownType::Both),
        Err(ErrorCode::InvalidState)
    ));

    assert!(matches!(tcp::local_address(sock.fd), Ok(_)));
    assert!(matches!(
        tcp::remote_address(sock.fd),
        Err(ErrorCode::InvalidState)
    ));
    assert_eq!(tcp::address_family(sock.fd), family);

    if family == IpAddressFamily::Ipv6 {
        assert!(matches!(tcp::ipv6_only(sock.fd), Ok(_)));
        assert!(matches!(
            tcp::set_ipv6_only(sock.fd, true),
            Err(ErrorCode::InvalidState)
        ));
    } else {
        assert!(matches!(
            tcp::ipv6_only(sock.fd),
            Err(ErrorCode::NotSupported)
        ));
        assert!(matches!(
            tcp::set_ipv6_only(sock.fd, true),
            Err(ErrorCode::NotSupported)
        ));
    }

    // assert!(matches!(tcp::set_listen_backlog_size(sock.fd, 32), Err(ErrorCode::AlreadyBound))); // FIXME
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
    let net = instance_network::instance_network();

    test_tcp_bound_state_invariants(net, IpAddressFamily::Ipv4);
    test_tcp_bound_state_invariants(net, IpAddressFamily::Ipv6);

    network::drop_network(net);
}
