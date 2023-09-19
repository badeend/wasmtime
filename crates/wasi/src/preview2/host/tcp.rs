use crate::preview2::bindings::{
    io::streams::{InputStream, OutputStream},
    poll::poll::Pollable,
    sockets::network::{self, ErrorCode, IpAddressFamily, IpSocketAddress, Network},
    sockets::tcp::{self, ShutdownType},
};
use crate::preview2::network::TableNetworkExt;
use crate::preview2::poll::TablePollableExt;
use crate::preview2::stream::TableStreamExt;
use crate::preview2::tcp::{HostTcpSocket, HostTcpState, TableTcpSocketExt};
use crate::preview2::{HostPollable, PollableFuture, WasiView};
use cap_net_ext::{AddressFamily, Blocking, PoolExt, TcpListenerExt};
use cap_std::net::TcpListener;
use io_lifetimes::AsSocketlike;
use rustix::io::Errno;
use rustix::net::sockopt;
use std::{any::Any, net::SocketAddr};
use tokio::io::Interest;

use super::network::ErrorExt;

impl<T: WasiView> tcp::Host for T {
    fn start_bind(
        &mut self,
        this: tcp::TcpSocket,
        network: Network,
        local_address: IpSocketAddress,
    ) -> Result<(), network::Error> {
        let table = self.table_mut();
        let socket = table.get_tcp_socket(this)?;
        let network = table.get_network(network)?;
        let local_address: SocketAddr = local_address.into();

        match socket.tcp_state {
            HostTcpState::Default => {}
            HostTcpState::Bound
            | HostTcpState::Connected
            | HostTcpState::ConnectFailed
            | HostTcpState::Listening => return Err(ErrorCode::InvalidState.into()),
            HostTcpState::BindStarted
            | HostTcpState::Connecting
            | HostTcpState::ConnectReady
            | HostTcpState::ListenStarted => return Err(ErrorCode::ConcurrencyConflict.into()),
        }

        let local_address_family = AddressFamily::of_socket_addr(local_address);
        if local_address_family != socket.address_family {
            return Err(ErrorCode::InvalidArgument.into());
        }

        let binder = network.0.tcp_binder(local_address)?;

        // Perform the OS bind call.
        binder
            .bind_existing_tcp_listener(&*socket.tcp_socket().as_socketlike_view::<TcpListener>())
            .map_err(|error| match error.errno() {
                Some(Errno::AFNOSUPPORT) => ErrorCode::InvalidArgument.into(),
                #[cfg(windows)]
                Some(Errno::NOBUFS) => ErrorCode::AddressInUse.into(), // Windows returns WSAENOBUFS when the ephemeral ports have been exhausted.
                _ => Into::<network::Error>::into(error),
            })?;

        let socket = table.get_tcp_socket_mut(this)?;
        socket.tcp_state = HostTcpState::BindStarted;

        Ok(())
    }

    fn finish_bind(&mut self, this: tcp::TcpSocket) -> Result<(), network::Error> {
        let table = self.table_mut();
        let socket = table.get_tcp_socket_mut(this)?;

        match socket.tcp_state {
            HostTcpState::BindStarted => {}
            _ => return Err(ErrorCode::NotInProgress.into()),
        }

        socket.tcp_state = HostTcpState::Bound;

        Ok(())
    }

    fn start_connect(
        &mut self,
        this: tcp::TcpSocket,
        network: Network,
        remote_address: IpSocketAddress,
    ) -> Result<(), network::Error> {
        let table = self.table_mut();
        let r = {
            let socket = table.get_tcp_socket(this)?;
            let network = table.get_network(network)?;
            let remote_address: SocketAddr = remote_address.into();

            match socket.tcp_state {
                HostTcpState::Default => {}
                HostTcpState::Bound => {
                    // Connecting on explicitly bound sockets should be allowed,
                    // but at the moment Networks can't be checked for equality, so we can't support this
                    // without introducing a security risk.
                    return Err(ErrorCode::InvalidState.into());
                }
                HostTcpState::Connected | HostTcpState::ConnectFailed | HostTcpState::Listening => {
                    return Err(ErrorCode::InvalidState.into())
                }
                HostTcpState::Connecting
                | HostTcpState::ConnectReady
                | HostTcpState::ListenStarted
                | HostTcpState::BindStarted => return Err(ErrorCode::ConcurrencyConflict.into()),
            }

            let remote_address_family = AddressFamily::of_socket_addr(remote_address);
            if remote_address_family != socket.address_family {
                return Err(ErrorCode::InvalidArgument.into());
            }

            if remote_address.ip().is_unspecified() {
                return Err(ErrorCode::InvalidArgument.into());
            }

            if remote_address.port() == 0 {
                return Err(ErrorCode::InvalidArgument.into());
            }

            let connecter = network.0.tcp_connecter(remote_address)?;

            // Do an OS `connect`. Our socket is non-blocking, so it'll either...
            {
                let view = &*socket.tcp_socket().as_socketlike_view::<TcpListener>();
                let r = connecter.connect_existing_tcp_listener(view);
                r
            }
        };

        match r {
            // succeed immediately,
            Ok(()) => {
                let socket = table.get_tcp_socket_mut(this)?;
                socket.tcp_state = HostTcpState::ConnectReady;
                return Ok(());
            }
            // continue in progress,
            Err(err) if err.errno() == Some(INPROGRESS) => {}
            // or fail immediately.
            Err(err) => {
                let socket = table.get_tcp_socket_mut(this)?;
                socket.tcp_state = HostTcpState::ConnectFailed;

                return Err(match err.errno() {
                    Some(Errno::AFNOSUPPORT) => ErrorCode::InvalidArgument.into(),
                    #[cfg(windows)]
                    Some(Errno::ADDRNOTAVAIL) => ErrorCode::InvalidArgument.into(),
                    #[cfg(not(windows))]
                    Some(Errno::ADDRNOTAVAIL) | Some(Errno::AGAIN) => {
                        // Ephemeral ports exhausted.
                        ErrorCode::AddressInUse.into()
                    }
                    _ => Into::<network::Error>::into(err),
                });
            }
        }

        let socket = table.get_tcp_socket_mut(this)?;
        socket.tcp_state = HostTcpState::Connecting;

        Ok(())
    }

    fn finish_connect(
        &mut self,
        this: tcp::TcpSocket,
    ) -> Result<(InputStream, OutputStream), network::Error> {
        let table = self.table_mut();
        let socket = table.get_tcp_socket_mut(this)?;

        match socket.tcp_state {
            HostTcpState::ConnectReady => {}
            HostTcpState::Connecting => {
                // Do a `poll` to test for completion, using a timeout of zero
                // to avoid blocking.
                match rustix::event::poll(
                    &mut [rustix::event::PollFd::new(
                        socket.tcp_socket(),
                        rustix::event::PollFlags::OUT,
                    )],
                    0,
                ) {
                    Ok(0) => return Err(ErrorCode::WouldBlock.into()),
                    Ok(_) => (),
                    Err(err) => Err(err).unwrap(),
                }

                // Check whether the connect succeeded.
                match sockopt::get_socket_error(socket.tcp_socket()) {
                    Ok(Ok(())) => {}
                    Err(err) | Ok(Err(err)) => {
                        socket.tcp_state = HostTcpState::ConnectFailed;
                        return Err(err.into());
                    }
                }
            }
            _ => return Err(ErrorCode::NotInProgress.into()),
        };

        socket.tcp_state = HostTcpState::Connected;
        let (input, output) = socket.as_split();
        let input_stream = self.table_mut().push_input_stream_child(input, this)?;
        let output_stream = self.table_mut().push_output_stream_child(output, this)?;

        Ok((input_stream, output_stream))
    }

    fn start_listen(&mut self, this: tcp::TcpSocket) -> Result<(), network::Error> {
        let table = self.table_mut();
        let socket = table.get_tcp_socket_mut(this)?;

        match socket.tcp_state {
            HostTcpState::Bound => {}
            HostTcpState::Default
            | HostTcpState::Connected
            | HostTcpState::ConnectFailed
            | HostTcpState::Listening => return Err(ErrorCode::InvalidState.into()),
            HostTcpState::ListenStarted
            | HostTcpState::Connecting
            | HostTcpState::ConnectReady
            | HostTcpState::BindStarted => return Err(ErrorCode::ConcurrencyConflict.into()),
        }

        socket
            .tcp_socket()
            .as_socketlike_view::<TcpListener>()
            .listen(None)
            .map_err(|error| match error.errno() {
                Some(Errno::DESTADDRREQ) => ErrorCode::InvalidState.into(), // Not bound (POSIX)
                Some(Errno::INVAL) => ErrorCode::InvalidState.into(), // Already connected (POSIX), Not bound (Windows)

                #[cfg(windows)]
                Some(Errno::MFILE) => ErrorCode::OutOfMemory.into(), // We're not trying to create a new socket. Rewrite it to less surprising error code.
                _ => Into::<network::Error>::into(error),
            })?;

        socket.tcp_state = HostTcpState::ListenStarted;

        Ok(())
    }

    fn finish_listen(&mut self, this: tcp::TcpSocket) -> Result<(), network::Error> {
        let table = self.table_mut();
        let socket = table.get_tcp_socket_mut(this)?;

        match socket.tcp_state {
            HostTcpState::ListenStarted => {}
            _ => return Err(ErrorCode::NotInProgress.into()),
        }

        socket.tcp_state = HostTcpState::Listening;

        Ok(())
    }

    fn accept(
        &mut self,
        this: tcp::TcpSocket,
    ) -> Result<(tcp::TcpSocket, InputStream, OutputStream), network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;

        match socket.tcp_state {
            HostTcpState::Listening => {}
            _ => return Err(ErrorCode::InvalidState.into()),
        }

        // Do the OS accept call.
        let tcp_socket = socket.tcp_socket();
        let (connection, _addr) = tcp_socket
            .try_io(Interest::READABLE, || {
                tcp_socket
                    .as_socketlike_view::<TcpListener>()
                    .accept_with(Blocking::No)
            })
            .map_err(|error| match error.errno() {
                Some(Errno::INVAL) => ErrorCode::InvalidState.into(), // Not listening
                #[cfg(windows)]
                Some(Errno::INPROGRESS) => ErrorCode::WouldBlock.into(), // "A blocking Windows Sockets 1.1 call is in progress, or the service provider is still processing a callback function.""

                // Skip over transient errors.
                // https://github.com/WebAssembly/wasi-sockets/pull/18/files#r1120716442
                // Due to Linux' non-standard behavior, this is quite a list.
                Some(
                    Errno::CONNABORTED
                    | Errno::CONNRESET
                    | Errno::NETRESET
                    | Errno::HOSTUNREACH
                    | Errno::HOSTDOWN
                    | Errno::NETDOWN
                    | Errno::NETUNREACH
                    | Errno::PROTO
                    | Errno::NOPROTOOPT
                    | Errno::NONET
                    | Errno::OPNOTSUPP,
                ) => ErrorCode::WouldBlock.into(),
                Some(Errno::NOMEM | Errno::NOBUFS | Errno::MFILE | Errno::NFILE) => {
                    // FIXME: technically these are transient.
                    // We should return WouldBlock, but the socket remains immediately readable,
                    // so to prevent applications ending up in a spin loop:
                    ErrorCode::OutOfMemory.into()
                }

                _ => Into::<network::Error>::into(error),
            })?;

        let tcp_socket =
            HostTcpSocket::from_accepted_tcp_stream(connection, socket.address_family)?;

        let (input, output) = tcp_socket.as_split();

        let tcp_socket = self.table_mut().push_tcp_socket(tcp_socket)?;
        let input_stream = self
            .table_mut()
            .push_input_stream_child(input, tcp_socket)?;
        let output_stream = self
            .table_mut()
            .push_output_stream_child(output, tcp_socket)?;

        Ok((tcp_socket, input_stream, output_stream))
    }

    fn local_address(&mut self, this: tcp::TcpSocket) -> Result<IpSocketAddress, network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;

        match socket.tcp_state {
            HostTcpState::Bound
            | HostTcpState::ListenStarted
            | HostTcpState::Listening
            | HostTcpState::Connected => {}
            _ => return Err(ErrorCode::InvalidState.into()),
        }

        let addr = socket
            .tcp_socket()
            .as_socketlike_view::<std::net::TcpStream>()
            .local_addr()?;
        Ok(addr.into())
    }

    fn remote_address(&mut self, this: tcp::TcpSocket) -> Result<IpSocketAddress, network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;

        match socket.tcp_state {
            HostTcpState::Connected => {}
            _ => return Err(ErrorCode::InvalidState.into()),
        }

        let addr = socket
            .tcp_socket()
            .as_socketlike_view::<std::net::TcpStream>()
            .peer_addr()?;
        Ok(addr.into())
    }

    fn address_family(&mut self, this: tcp::TcpSocket) -> Result<IpAddressFamily, anyhow::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;

        let family = match socket.address_family {
            AddressFamily::Ipv4 => IpAddressFamily::Ipv4,
            AddressFamily::Ipv6 => IpAddressFamily::Ipv6,
        };

        Ok(family)
    }

    fn ipv6_only(&mut self, this: tcp::TcpSocket) -> Result<bool, network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;

        match socket.address_family {
            AddressFamily::Ipv6 => {}
            AddressFamily::Ipv4 => return Err(ErrorCode::NotSupported.into()),
        }

        Ok(sockopt::get_ipv6_v6only(socket.tcp_socket())?)
    }

    fn set_ipv6_only(&mut self, this: tcp::TcpSocket, value: bool) -> Result<(), network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;

        match socket.address_family {
            AddressFamily::Ipv6 => {}
            AddressFamily::Ipv4 => return Err(ErrorCode::NotSupported.into()),
        }

        match socket.tcp_state {
            HostTcpState::Default => {}
            HostTcpState::BindStarted => return Err(ErrorCode::ConcurrencyConflict.into()),
            _ => return Err(ErrorCode::InvalidState.into()),
        }

        Ok(sockopt::set_ipv6_v6only(socket.tcp_socket(), value)?)
    }

    fn set_listen_backlog_size(
        &mut self,
        this: tcp::TcpSocket,
        value: u64,
    ) -> Result<(), network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;

        match socket.tcp_state {
            HostTcpState::Listening => {}
            _ => return Err(ErrorCode::NotInProgress.into()),
        }

        let value = value.try_into().map_err(|_| ErrorCode::OutOfMemory)?;
        Ok(rustix::net::listen(socket.tcp_socket(), value)?)
    }

    fn keep_alive(&mut self, this: tcp::TcpSocket) -> Result<bool, network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;
        Ok(sockopt::get_socket_keepalive(socket.tcp_socket())?)
    }

    fn set_keep_alive(&mut self, this: tcp::TcpSocket, value: bool) -> Result<(), network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;
        Ok(sockopt::set_socket_keepalive(socket.tcp_socket(), value)?)
    }

    fn no_delay(&mut self, this: tcp::TcpSocket) -> Result<bool, network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;
        Ok(sockopt::get_tcp_nodelay(socket.tcp_socket())?)
    }

    fn set_no_delay(&mut self, this: tcp::TcpSocket, value: bool) -> Result<(), network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;
        Ok(sockopt::set_tcp_nodelay(socket.tcp_socket(), value)?)
    }

    fn unicast_hop_limit(&mut self, this: tcp::TcpSocket) -> Result<u8, network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;

        let ttl = match socket.address_family {
            AddressFamily::Ipv4 => sockopt::get_ip_ttl(socket.tcp_socket())?
                .try_into()
                .unwrap(),
            AddressFamily::Ipv6 => sockopt::get_ipv6_unicast_hops(socket.tcp_socket())?,
        };

        Ok(ttl)
    }

    fn set_unicast_hop_limit(
        &mut self,
        this: tcp::TcpSocket,
        value: u8,
    ) -> Result<(), network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;

        match socket.address_family {
            AddressFamily::Ipv4 => sockopt::set_ip_ttl(socket.tcp_socket(), value.into())?,
            AddressFamily::Ipv6 => {
                sockopt::set_ipv6_unicast_hops(socket.tcp_socket(), Some(value))?
            }
        };

        Ok(())
    }

    fn receive_buffer_size(&mut self, this: tcp::TcpSocket) -> Result<u64, network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;
        Ok(sockopt::get_socket_recv_buffer_size(socket.tcp_socket())? as u64)
    }

    fn set_receive_buffer_size(
        &mut self,
        this: tcp::TcpSocket,
        value: u64,
    ) -> Result<(), network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;
        let value = value.try_into().map_err(|_| ErrorCode::OutOfMemory)?;
        Ok(sockopt::set_socket_recv_buffer_size(
            socket.tcp_socket(),
            value,
        )?)
    }

    fn send_buffer_size(&mut self, this: tcp::TcpSocket) -> Result<u64, network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;
        Ok(sockopt::get_socket_send_buffer_size(socket.tcp_socket())? as u64)
    }

    fn set_send_buffer_size(
        &mut self,
        this: tcp::TcpSocket,
        value: u64,
    ) -> Result<(), network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;
        let value = value.try_into().map_err(|_| ErrorCode::OutOfMemory)?;
        Ok(sockopt::set_socket_send_buffer_size(
            socket.tcp_socket(),
            value,
        )?)
    }

    fn subscribe(&mut self, this: tcp::TcpSocket) -> anyhow::Result<Pollable> {
        fn make_tcp_socket_future<'a>(stream: &'a mut dyn Any) -> PollableFuture<'a> {
            let socket = stream
                .downcast_mut::<HostTcpSocket>()
                .expect("downcast to HostTcpSocket failed");

            // Some states are ready immediately.
            match socket.tcp_state {
                HostTcpState::BindStarted
                | HostTcpState::ListenStarted
                | HostTcpState::ConnectReady => return Box::pin(async { Ok(()) }),
                _ => {}
            }

            // FIXME: Add `Interest::ERROR` when we update to tokio 1.32.
            let join = Box::pin(async move {
                socket
                    .inner
                    .ready(Interest::READABLE | Interest::WRITABLE)
                    .await
                    .unwrap();
                Ok(())
            });

            join
        }

        let pollable = HostPollable::TableEntry {
            index: this,
            make_future: make_tcp_socket_future,
        };

        Ok(self.table_mut().push_host_pollable(pollable)?)
    }

    fn shutdown(
        &mut self,
        this: tcp::TcpSocket,
        shutdown_type: ShutdownType,
    ) -> Result<(), network::Error> {
        let table = self.table();
        let socket = table.get_tcp_socket(this)?;

        match socket.tcp_state {
            HostTcpState::Connected => {}
            HostTcpState::Connecting | HostTcpState::ConnectReady => {
                return Err(ErrorCode::ConcurrencyConflict.into())
            }
            _ => return Err(ErrorCode::InvalidState.into()),
        }

        let how = match shutdown_type {
            ShutdownType::Receive => std::net::Shutdown::Read,
            ShutdownType::Send => std::net::Shutdown::Write,
            ShutdownType::Both => std::net::Shutdown::Both,
        };

        socket
            .tcp_socket()
            .as_socketlike_view::<std::net::TcpStream>()
            .shutdown(how)?;
        Ok(())
    }

    fn drop_tcp_socket(&mut self, this: tcp::TcpSocket) -> Result<(), anyhow::Error> {
        let table = self.table_mut();

        // As in the filesystem implementation, we assume closing a socket
        // doesn't block.
        let dropped = table.delete_tcp_socket(this)?;

        // If we might have an `event::poll` waiting on the socket, wake it up.
        #[cfg(not(unix))]
        {
            match dropped.tcp_state {
                HostTcpState::Default
                | HostTcpState::BindStarted
                | HostTcpState::Bound
                | HostTcpState::ListenStarted
                | HostTcpState::ConnectReady => {}

                HostTcpState::Listening | HostTcpState::Connecting | HostTcpState::Connected => {
                    match rustix::net::shutdown(&dropped.inner, rustix::net::Shutdown::ReadWrite) {
                        Ok(()) | Err(Errno::NOTCONN) => {}
                        Err(err) => Err(err).unwrap(),
                    }
                }
            }
        }

        drop(dropped);

        Ok(())
    }
}

// On POSIX, non-blocking TCP socket `connect` uses `EINPROGRESS`.
// <https://pubs.opengroup.org/onlinepubs/9699919799/functions/connect.html>
#[cfg(not(windows))]
const INPROGRESS: Errno = Errno::INPROGRESS;

// On Windows, non-blocking TCP socket `connect` uses `WSAEWOULDBLOCK`.
// <https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-connect>
#[cfg(windows)]
const INPROGRESS: Errno = Errno::WOULDBLOCK;
