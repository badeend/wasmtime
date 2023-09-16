use wasi::sockets::network::{self, ErrorCode, IpAddress, IpAddressFamily, IpSocketAddress};
use wasi::sockets::{instance_network, tcp};
use wasi::io::streams;
use wasi_sockets_tests::*;

fn test_tcp_connected_state_invariants(net: tcp::Network, family: IpAddressFamily) {
    let bind_address = IpSocketAddress::new(IpAddress::new_loopback(family), 0);
    let sock_listener = TcpSock::new(family).unwrap();
    sock_listener.bind(net, bind_address).unwrap();
    sock_listener.listen().unwrap();
    let addr_listener = tcp::local_address(sock_listener.fd).unwrap();
    let sock = TcpSock::new(family).unwrap();
    let (input, output) = sock.connect(net, addr_listener).unwrap();



    assert!(matches!(
        tcp::start_bind(sock.fd, net, bind_address),
        Err(ErrorCode::InvalidState)
    ));
    assert!(matches!(
        tcp::finish_bind(sock.fd),
        Err(ErrorCode::NotInProgress)
    ));
    assert!(matches!(
        tcp::start_connect(sock.fd, net, addr_listener),
        Err(ErrorCode::InvalidState)
    ));
    assert!(matches!(
        tcp::finish_connect(sock.fd),
        Err(ErrorCode::NotInProgress)
    ));
    assert!(matches!(
        tcp::start_listen(sock.fd),
        Err(ErrorCode::InvalidState)
    ));
    assert!(matches!(
        tcp::finish_listen(sock.fd),
        Err(ErrorCode::NotInProgress)
    ));
    assert!(matches!(tcp::accept(sock.fd), Err(ErrorCode::InvalidState)));
    // Skipping: tcp::shutdown

    assert!(matches!(tcp::local_address(sock.fd), Ok(_)));
    assert!(matches!(tcp::remote_address(sock.fd), Ok(_)));
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

    streams::drop_input_stream(input);
    streams::drop_output_stream(output);
}

fn main() {
    let net = instance_network::instance_network();

    test_tcp_connected_state_invariants(net, IpAddressFamily::Ipv4);
    test_tcp_connected_state_invariants(net, IpAddressFamily::Ipv6);

    network::drop_network(net);
}
