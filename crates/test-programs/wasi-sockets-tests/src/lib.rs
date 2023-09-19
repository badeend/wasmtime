wit_bindgen::generate!("test-command-with-sockets" in "../../wasi/wit");

use wasi::io::streams::{self, InputStream, OutputStream};
use wasi::poll::poll;
use wasi::sockets::network::{
    ErrorCode, IpAddress, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress,
};
use wasi::sockets::tcp;
use wasi::sockets::tcp_create_socket;

pub struct Subscription {
    pollable: poll::Pollable,
}

impl Subscription {
    pub fn new(pollable: poll::Pollable) -> Self {
        Subscription { pollable }
    }

    pub fn wait(&self) {
        loop {
            let wait = poll::poll_oneoff(&[self.pollable]);
            if wait[0] {
                break;
            }
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        poll::drop_pollable(self.pollable);
    }
}

pub fn write(output: streams::OutputStream, mut bytes: &[u8]) -> (usize, streams::StreamStatus) {
    let total = bytes.len();
    let mut written = 0;

    let s = Subscription::new(streams::subscribe_to_output_stream(output));

    while !bytes.is_empty() {
        s.wait();

        let permit = match streams::check_write(output) {
            Ok(n) => n,
            Err(_) => return (written, streams::StreamStatus::Ended),
        };

        let len = bytes.len().min(permit as usize);
        let (chunk, rest) = bytes.split_at(len);

        match streams::write(output, chunk) {
            Ok(()) => {}
            Err(_) => return (written, streams::StreamStatus::Ended),
        }

        match streams::blocking_flush(output) {
            Ok(()) => {}
            Err(_) => return (written, streams::StreamStatus::Ended),
        }

        bytes = rest;
        written += len;
    }

    (total, streams::StreamStatus::Open)
}

pub struct TcpSock {
    pub fd: tcp::TcpSocket,
}

impl TcpSock {
    pub fn new(address_family: IpAddressFamily) -> Result<TcpSock, ErrorCode> {
        Ok(TcpSock {
            fd: tcp_create_socket::create_tcp_socket(address_family)?,
        })
    }

    pub fn bind(&self, network: u32, local_address: IpSocketAddress) -> Result<(), ErrorCode> {
        let sub = Subscription::new(tcp::subscribe(self.fd));

        tcp::start_bind(self.fd, network, local_address)?;

        loop {
            match tcp::finish_bind(self.fd) {
                Err(ErrorCode::WouldBlock) => sub.wait(),
                result => return result,
            }
        }
    }

    pub fn listen(&self) -> Result<(), ErrorCode> {
        let sub = Subscription::new(tcp::subscribe(self.fd));

        tcp::start_listen(self.fd)?;

        loop {
            match tcp::finish_listen(self.fd) {
                Err(ErrorCode::WouldBlock) => sub.wait(),
                result => return result,
            }
        }
    }

    pub fn connect(
        &self,
        network: u32,
        remote_address: IpSocketAddress,
    ) -> Result<(InputStream, OutputStream), ErrorCode> {
        let sub = Subscription::new(tcp::subscribe(self.fd));

        tcp::start_connect(self.fd, network, remote_address)?;

        loop {
            match tcp::finish_connect(self.fd) {
                Err(ErrorCode::WouldBlock) => sub.wait(),
                result => return result,
            }
        }
    }

    pub fn accept(&self) -> Result<(TcpSock, InputStream, OutputStream), ErrorCode> {
        let sub = Subscription::new(tcp::subscribe(self.fd));

        loop {
            match tcp::accept(self.fd) {
                Err(ErrorCode::WouldBlock) => sub.wait(),
                Err(e) => return Err(e),
                Ok((s, i, o)) => return Ok((TcpSock { fd: s }, i, o)),
            }
        }
    }
}

impl Drop for TcpSock {
    fn drop(&mut self) {
        tcp::drop_tcp_socket(self.fd);
    }
}

impl Eq for IpAddress {}
impl PartialEq for IpAddress {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ipv4(l0), Self::Ipv4(r0)) => l0 == r0,
            (Self::Ipv6(l0), Self::Ipv6(r0)) => l0 == r0,
            _ => false,
        }
    }
}

impl IpAddress {
    pub const IPV4_LOOPBACK: IpAddress = IpAddress::Ipv4((127, 0, 0, 1));
    pub const IPV6_LOOPBACK: IpAddress = IpAddress::Ipv6((0, 0, 0, 0, 0, 0, 0, 1));

    pub const IPV4_UNSPECIFIED: IpAddress = IpAddress::Ipv4((0, 0, 0, 0));
    pub const IPV6_UNSPECIFIED: IpAddress = IpAddress::Ipv6((0, 0, 0, 0, 0, 0, 0, 0));

    pub const IPV4_MAPPED_LOOPBACK: IpAddress =
        IpAddress::Ipv6((0, 0, 0, 0, 0, 0xFFFF, 0x7F00, 0x0001));

    pub const fn new_loopback(family: IpAddressFamily) -> IpAddress {
        match family {
            IpAddressFamily::Ipv4 => Self::IPV4_LOOPBACK,
            IpAddressFamily::Ipv6 => Self::IPV6_LOOPBACK,
        }
    }

    pub const fn new_unspecified(family: IpAddressFamily) -> IpAddress {
        match family {
            IpAddressFamily::Ipv4 => Self::IPV4_UNSPECIFIED,
            IpAddressFamily::Ipv6 => Self::IPV6_UNSPECIFIED,
        }
    }

    pub const fn family(&self) -> IpAddressFamily {
        match self {
            IpAddress::Ipv4(_) => IpAddressFamily::Ipv4,
            IpAddress::Ipv6(_) => IpAddressFamily::Ipv6,
        }
    }
}

impl IpSocketAddress {
    pub const fn new(ip: IpAddress, port: u16) -> IpSocketAddress {
        match ip {
            IpAddress::Ipv4(addr) => IpSocketAddress::Ipv4(Ipv4SocketAddress {
                port: port,
                address: addr,
            }),
            IpAddress::Ipv6(addr) => IpSocketAddress::Ipv6(Ipv6SocketAddress {
                port: port,
                address: addr,
                flow_info: 0,
                scope_id: 0,
            }),
        }
    }

    pub const fn ip(&self) -> IpAddress {
        match self {
            IpSocketAddress::Ipv4(addr) => IpAddress::Ipv4(addr.address),
            IpSocketAddress::Ipv6(addr) => IpAddress::Ipv6(addr.address),
        }
    }

    pub const fn port(&self) -> u16 {
        match self {
            IpSocketAddress::Ipv4(addr) => addr.port,
            IpSocketAddress::Ipv6(addr) => addr.port,
        }
    }

    pub const fn family(&self) -> IpAddressFamily {
        match self {
            IpSocketAddress::Ipv4(_) => IpAddressFamily::Ipv4,
            IpSocketAddress::Ipv6(_) => IpAddressFamily::Ipv6,
        }
    }
}
