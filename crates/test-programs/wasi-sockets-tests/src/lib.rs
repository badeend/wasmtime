wit_bindgen::generate!("test-command-with-sockets" in "../../wasi/wit");

use wasi::io::streams::{self, InputStream, OutputStream};
use wasi::poll::poll;
use wasi::sockets::{
    network::{ErrorCode, IpSocketAddress},
    tcp::{self, TcpSocket},
};

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

pub fn tcp_blocking_bind(
    socket: u32,
    network: u32,
    local_address: IpSocketAddress,
) -> Result<(), ErrorCode> {
    let s = Subscription::new(tcp::subscribe(socket));

    tcp::start_bind(socket, network, local_address)?;
    s.wait();
    tcp::finish_bind(socket)
}

pub fn tcp_blocking_listen(socket: u32) -> Result<(), ErrorCode> {
    let s = Subscription::new(tcp::subscribe(socket));

    tcp::start_listen(socket)?;
    s.wait();
    tcp::finish_listen(socket)
}

pub fn tcp_blocking_connect(
    socket: u32,
    network: u32,
    remote_address: IpSocketAddress,
) -> Result<(InputStream, OutputStream), ErrorCode> {
    let s = Subscription::new(tcp::subscribe(socket));

    tcp::start_connect(socket, network, remote_address)?;
    s.wait();
    tcp::finish_connect(socket)
}

pub fn tcp_blocking_accept(
    socket: u32,
) -> Result<(TcpSocket, InputStream, OutputStream), ErrorCode> {
    let s = Subscription::new(tcp::subscribe(socket));

    loop {
        match tcp::accept(socket) {
            Err(ErrorCode::WouldBlock) => s.wait(),
            Err(e) => return Err(e),
            Ok(r) => return Ok(r),
        }
    }
}
