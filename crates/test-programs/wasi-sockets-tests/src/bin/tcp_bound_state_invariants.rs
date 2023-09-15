use wasi::sockets::network::{ErrorCode, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress};
use wasi::sockets::{instance_network, tcp, tcp_create_socket};
use wasi_sockets_tests::*;

fn test_tcp_bound_state_invariants(
    net: tcp::Network,
    sock: tcp::TcpSocket,
    family: IpAddressFamily,
    bind_address: IpSocketAddress,
) {
    assert!(matches!(
        tcp::start_bind(sock, net, bind_address),
        Err(ErrorCode::AlreadyBound)
    ));
    assert!(matches!(
        tcp::finish_bind(sock),
        Err(ErrorCode::NotInProgress)
    ));
    // Skipping: tcp::start_connect
    assert!(matches!(
        tcp::finish_connect(sock),
        Err(ErrorCode::NotInProgress)
    ));
    // Skipping: tcp::start_listen
    assert!(matches!(
        tcp::finish_listen(sock),
        Err(ErrorCode::NotInProgress)
    ));
    assert!(matches!(tcp::accept(sock), Err(ErrorCode::NotListening)));
    assert!(matches!(
        tcp::shutdown(sock, tcp::ShutdownType::Both),
        Err(ErrorCode::NotConnected)
    ));

    assert!(matches!(tcp::local_address(sock), Ok(_)));
    assert!(matches!(
        tcp::remote_address(sock),
        Err(ErrorCode::NotConnected)
    ));
    assert_eq!(tcp::address_family(sock), family);

    if family == IpAddressFamily::Ipv6 {
        assert!(matches!(tcp::ipv6_only(sock), Ok(_)));
        assert!(matches!(
            tcp::set_ipv6_only(sock, true),
            Err(ErrorCode::AlreadyBound)
        ));
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

    // assert!(matches!(tcp::set_listen_backlog_size(sock, 32), Err(ErrorCode::AlreadyBound))); // FIXME
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
    let net = instance_network::instance_network();
    
    {
        let addr = IpSocketAddress::Ipv4(Ipv4SocketAddress {
            port: 0,                 // use any free port
            address: (127, 0, 0, 1), // localhost
        });

        let sock = tcp_create_socket::create_tcp_socket(IpAddressFamily::Ipv4).unwrap();
        tcp_blocking_bind(sock, net, addr).unwrap();

        test_tcp_bound_state_invariants(
            net,
            sock,
            IpAddressFamily::Ipv4,
            addr,
        );
    }
    {
        let addr = IpSocketAddress::Ipv6(Ipv6SocketAddress {
            port: 0,                           // use any free port
            address: (0, 0, 0, 0, 0, 0, 0, 1), // localhost
            flow_info: 0,
            scope_id: 0,
        });

        let sock = tcp_create_socket::create_tcp_socket(IpAddressFamily::Ipv6).unwrap();
        tcp_blocking_bind(sock, net, addr).unwrap();

        test_tcp_bound_state_invariants(
            net,
            sock,
            IpAddressFamily::Ipv6,
            addr,
        );
    }
}
