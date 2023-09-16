use wasi::sockets::network::{self, IpAddressFamily, IpSocketAddress};
use wasi::sockets::{instance_network, tcp};
use wasi_sockets_tests::wasi::io::streams::{InputStream, OutputStream, self};
use wasi_sockets_tests::wasi::sockets::network::IpAddress;
use wasi_sockets_tests::*;

/// Accept a single connection.
fn test_tcp_accept(net: tcp::Network, family: IpAddressFamily) {
    let (listener, bound_addr) = create_listener(net, family);
    let (_connected_client, input1, output1) = create_connection(net, bound_addr);

    if let Ok((_accepted_client, input2, output2)) = listener.accept() {
        streams::drop_input_stream(input2);
        streams::drop_output_stream(output2);
    } else {
        panic!("accept failed");
    }

    streams::drop_input_stream(input1);
    streams::drop_output_stream(output1);
}

fn main() {
    let net = instance_network::instance_network();

    test_tcp_accept(net, IpAddressFamily::Ipv4);
    test_tcp_accept(net, IpAddressFamily::Ipv6);

    network::drop_network(net);
}


fn create_listener(net: tcp::Network, family: IpAddressFamily) -> (TcpSock, IpSocketAddress) {
    let bind_addr = IpSocketAddress::new(IpAddress::new_loopback(family), 0);

    let sock = TcpSock::new(family).unwrap();
    sock.bind(net, bind_addr).unwrap();
    sock.listen().unwrap();

    let bound_addr = tcp::local_address(sock.fd).unwrap();

    return (sock, bound_addr);
}

fn create_connection(net: tcp::Network, addr: IpSocketAddress) -> (TcpSock, InputStream, OutputStream) {

    let sock = TcpSock::new(addr.family()).unwrap();
    
    let (input, output) = sock.connect(net, addr).unwrap();

    (sock, input, output)
}
