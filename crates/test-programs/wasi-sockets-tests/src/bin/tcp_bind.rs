use wasi::sockets::network::{self, ErrorCode, IpAddressFamily, IpSocketAddress};
use wasi::sockets::{instance_network, tcp};
use wasi_sockets_tests::wasi::sockets::network::IpAddress;
use wasi_sockets_tests::*;

/// Bind a socket and let the system determine a port.
fn test_tcp_bind_ephemeral_port(net: tcp::Network, ip: IpAddress) {
    let bind_addr = IpSocketAddress::new(ip, 0);

    let sock = TcpSock::new(ip.family()).unwrap();
    sock.bind(net, bind_addr).unwrap();

    let bound_addr = tcp::local_address(sock.fd).unwrap();

    assert_eq!(bind_addr.ip(), bound_addr.ip());
    assert_ne!(bind_addr.port(), bound_addr.port());
}

/// Bind a socket on a specified port.
fn test_tcp_bind_specific_port(net: tcp::Network, ip: IpAddress) {
    const PORT: u16 = 54321;

    let bind_addr = IpSocketAddress::new(ip, PORT);

    let sock = TcpSock::new(ip.family()).unwrap();
    sock.bind(net, bind_addr).unwrap();

    let bound_addr = tcp::local_address(sock.fd).unwrap();

    assert_eq!(bind_addr.ip(), bound_addr.ip());
    assert_eq!(bind_addr.port(), bound_addr.port());
}

/// Two sockets may not be actively bound to the same address at the same time.
fn test_tcp_bind_addrinuse(net: tcp::Network, ip: IpAddress) {
    let bind_addr = IpSocketAddress::new(ip, 0);

    let sock1 = TcpSock::new(ip.family()).unwrap();
    sock1.bind(net, bind_addr).unwrap();
    sock1.listen().unwrap();

    let bound_addr = tcp::local_address(sock1.fd).unwrap();

    let sock2 = TcpSock::new(ip.family()).unwrap();
    assert_eq!(sock2.bind(net, bound_addr), Err(ErrorCode::AddressInUse));
}

// Try binding to an address that is not configured on the system.
fn test_tcp_bind_addrnotavail(net: tcp::Network, ip: IpAddress) {
    let bind_addr = IpSocketAddress::new(ip, 0);

    let sock = TcpSock::new(ip.family()).unwrap();

    assert_eq!(sock.bind(net, bind_addr), Err(ErrorCode::AddressNotBindable));
}

/// Bind should validate the input address.
fn test_tcp_bind_wrong_family(net: tcp::Network, family: IpAddressFamily) {
    let wrong_ip = match family {
        IpAddressFamily::Ipv4 => IpAddress::IPV6_LOOPBACK,
        IpAddressFamily::Ipv6 => IpAddress::IPV4_LOOPBACK,
    };

    let sock = TcpSock::new(family).unwrap();
    let result = sock.bind(net, IpSocketAddress::new(wrong_ip, 0));

    assert!(matches!(result, Err(ErrorCode::InvalidArgument)));
}

fn test_tcp_bind_dual_stack(net: tcp::Network) {
    let sock = TcpSock::new(IpAddressFamily::Ipv6).unwrap();
    let addr = IpSocketAddress::new(IpAddress::IPV4_MAPPED_LOOPBACK, 0);

    // Even on platforms that don't support dualstack sockets,
    // setting ipv6_only to true (disabling dualstack mode) should work.
    tcp::set_ipv6_only(sock.fd, true).unwrap();

    // Binding a IPv4-mapped-IPv6 address on a ipv6-only socket should fail:
    assert!(matches!(
        sock.bind(net, addr),
        Err(ErrorCode::InvalidArgument)
    ));

    match tcp::set_ipv6_only(sock.fd, false) {
        Err(ErrorCode::NotSupported) => {
            println!("Skipping dual stack test");
            return;
        }
        Err(e) => panic!("Unexpected set_ipv6_only error code: {:?}", e),
        Ok(_) => {
            sock.bind(net, addr).unwrap();

            let bound_addr = tcp::local_address(sock.fd).unwrap();

            assert_eq!(bound_addr.family(), IpAddressFamily::Ipv6);
        }
    }
}

fn main() {
    let net = instance_network::instance_network();

    test_tcp_bind_ephemeral_port(net, IpAddress::IPV4_LOOPBACK);
    test_tcp_bind_ephemeral_port(net, IpAddress::IPV6_LOOPBACK);
    test_tcp_bind_ephemeral_port(net, IpAddress::IPV4_UNSPECIFIED);
    test_tcp_bind_ephemeral_port(net, IpAddress::IPV6_UNSPECIFIED);

    test_tcp_bind_specific_port(net, IpAddress::IPV4_LOOPBACK);
    test_tcp_bind_specific_port(net, IpAddress::IPV6_LOOPBACK);
    test_tcp_bind_specific_port(net, IpAddress::IPV4_UNSPECIFIED);
    test_tcp_bind_specific_port(net, IpAddress::IPV6_UNSPECIFIED);

    test_tcp_bind_addrinuse(net, IpAddress::IPV4_LOOPBACK);
    test_tcp_bind_addrinuse(net, IpAddress::IPV6_LOOPBACK);
    test_tcp_bind_addrinuse(net, IpAddress::IPV4_UNSPECIFIED);
    test_tcp_bind_addrinuse(net, IpAddress::IPV6_UNSPECIFIED);

    test_tcp_bind_addrnotavail(net, IpAddress::Ipv4((192, 0, 2, 0))); // Reserved for documentation and examples.
    test_tcp_bind_addrnotavail(net, IpAddress::Ipv6((0x2001, 0x0db8, 0, 0, 0, 0, 0, 0))); // Reserved for documentation and examples.

    test_tcp_bind_wrong_family(net, IpAddressFamily::Ipv4);
    test_tcp_bind_wrong_family(net, IpAddressFamily::Ipv6);

    test_tcp_bind_dual_stack(net);

    network::drop_network(net);
}
