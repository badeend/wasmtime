use wasi::sockets::network::{self, IpAddressFamily, IpSocketAddress};
use wasi::sockets::{instance_network, tcp};
use wasi_sockets_tests::wasi::sockets::network::IpAddress;
use wasi_sockets_tests::*;

/// Bind a socket and start listening.
fn test_tcp_listen(net: tcp::Network, family: IpAddressFamily) {
    let bind_addr = IpSocketAddress::new(IpAddress::new_loopback(family), 0);

    let sock = TcpSock::new(family).unwrap();
    sock.bind(net, bind_addr).unwrap();
    // tcp::set_listen_backlog_size(sock.fd, 32).unwrap(); // FIXME
    sock.listen().unwrap();
}


fn main() {
    let net = instance_network::instance_network();

    test_tcp_listen(net, IpAddressFamily::Ipv4);
    test_tcp_listen(net, IpAddressFamily::Ipv6);

    network::drop_network(net);
}
