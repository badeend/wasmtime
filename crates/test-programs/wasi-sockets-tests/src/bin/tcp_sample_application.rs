use wasi::io::streams;
use wasi::sockets::network::{self, IpAddress, IpAddressFamily, IpSocketAddress};
use wasi::sockets::{instance_network, tcp};
use wasi_sockets_tests::*;

/// A simple end-to-end conversation between a TCP server and client.
fn test_sample_application(net: tcp::Network, family: IpAddressFamily) {
    let bind_address = IpSocketAddress::new(IpAddress::new_loopback(family), 0);

    let first_message = b"Hello, world!";
    let second_message = b"Greetings, planet!";

    let sock = TcpSock::new(family).unwrap();

    sock.bind(net, bind_address).unwrap();
    sock.listen().unwrap();

    let addr = tcp::local_address(sock.fd).unwrap();

    {
        let client = TcpSock::new(family).unwrap();

        let (client_input, client_output) = client.connect(net, addr).unwrap();

        let (n, status) = write(client_output, &[]);
        assert_eq!(n, 0);
        assert_eq!(status, streams::StreamStatus::Open);

        let (n, status) = write(client_output, first_message);
        assert_eq!(n, first_message.len());
        assert_eq!(status, streams::StreamStatus::Open);

        streams::drop_input_stream(client_input);
        streams::drop_output_stream(client_output);
    }

    {
        let (_accepted, input, output) = sock.accept().unwrap();

        let (empty_data, status) = streams::read(input, 0).unwrap();
        assert!(empty_data.is_empty());
        assert_eq!(status, streams::StreamStatus::Open);

        let (data, status) = streams::blocking_read(input, first_message.len() as u64).unwrap();
        assert_eq!(status, streams::StreamStatus::Open);

        streams::drop_input_stream(input);
        streams::drop_output_stream(output);

        // Check that we sent and recieved our message!
        assert_eq!(data, first_message); // Not guaranteed to work but should work in practice.
    }

    {
        // Another client
        let client = TcpSock::new(family).unwrap();

        let (client_input, client_output) = client.connect(net, addr).unwrap();

        let (n, status) = write(client_output, second_message);
        assert_eq!(n, second_message.len());
        assert_eq!(status, streams::StreamStatus::Open);

        streams::drop_input_stream(client_input);
        streams::drop_output_stream(client_output);
    }

    {
        let (_accepted, input, output) = sock.accept().unwrap();
        let (data, status) = streams::blocking_read(input, second_message.len() as u64).unwrap();
        assert_eq!(status, streams::StreamStatus::Open);

        streams::drop_input_stream(input);
        streams::drop_output_stream(output);

        // Check that we sent and recieved our message!
        assert_eq!(data, second_message); // Not guaranteed to work but should work in practice.
    }
}

fn main() {
    let net = instance_network::instance_network();

    test_sample_application(net, IpAddressFamily::Ipv4);
    test_sample_application(net, IpAddressFamily::Ipv6);

    network::drop_network(net);
}
