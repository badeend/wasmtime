use crate::preview2::bindings::io::poll::Pollable;
use crate::preview2::bindings::sockets::ip_name_lookup::{Host, HostResolveAddressStream};
use crate::preview2::bindings::sockets::network::{
    Error, ErrorCode, IpAddress, IpAddressFamily, Network,
};
use crate::preview2::network::TableNetworkExt;
use crate::preview2::poll::{Subscribe, TablePollableExt};
use crate::preview2::{AbortOnDropJoinHandle, WasiView};
use anyhow::Result;
use std::io;
use std::mem;
use std::net::{SocketAddr, ToSocketAddrs};
use std::vec;
use wasmtime::component::Resource;

pub enum ResolveAddressStream {
    Waiting(AbortOnDropJoinHandle<io::Result<Vec<IpAddress>>>),
    Done(io::Result<vec::IntoIter<IpAddress>>),
}

#[async_trait::async_trait]
impl<T: WasiView> Host for T {
    fn resolve_addresses(
        &mut self,
        network: Resource<Network>,
        name: String,
        family: Option<IpAddressFamily>,
        include_unavailable: bool,
    ) -> Result<Resource<ResolveAddressStream>, Error> {
        if !self.table().get_network(&network)?.allow_ip_name_lookup {
            return Err(ErrorCode::PermanentResolverFailure.into());
        }

        // ignored for now, should probably have a future PR to actually take
        // this into account. This would require invoking `getaddrinfo` directly
        // rather than using the standard library to do it for us.
        let _ = include_unavailable;

        // For now use the standard library to perform actual resolution through
        // the usage of the `ToSocketAddrs` trait. This blocks the current
        // thread, so use `spawn_blocking`. Finally note that this is only
        // resolving names, not ports, so force the port to be 0.
        let task = tokio::task::spawn_blocking(move || -> io::Result<Vec<_>> {
            let result = (name.as_str(), 0).to_socket_addrs()?;
            Ok(result
                .filter_map(|addr| {
                    // In lieu of preventing these addresses from being resolved
                    // in the first place, filter them out here.
                    match addr {
                        SocketAddr::V4(addr) => match family {
                            None | Some(IpAddressFamily::Ipv4) => {
                                let [a, b, c, d] = addr.ip().octets();
                                Some(IpAddress::Ipv4((a, b, c, d)))
                            }
                            Some(IpAddressFamily::Ipv6) => None,
                        },
                        SocketAddr::V6(addr) => match family {
                            None | Some(IpAddressFamily::Ipv6) => {
                                let [a, b, c, d, e, f, g, h] = addr.ip().segments();
                                Some(IpAddress::Ipv6((a, b, c, d, e, f, g, h)))
                            }
                            Some(IpAddressFamily::Ipv4) => None,
                        },
                    }
                })
                .collect())
        });
        let task = AbortOnDropJoinHandle(task);
        let resource = self
            .table_mut()
            .push_resource(ResolveAddressStream::Waiting(task))?;
        Ok(resource)
    }
}

#[async_trait::async_trait]
impl<T: WasiView> HostResolveAddressStream for T {
    fn resolve_next_address(
        &mut self,
        resource: Resource<ResolveAddressStream>,
    ) -> Result<Option<IpAddress>, Error> {
        let stream = self.table_mut().get_resource_mut(&resource)?;
        loop {
            match stream {
                ResolveAddressStream::Waiting(future) => match crate::preview2::poll_noop(future) {
                    Some(result) => {
                        *stream = ResolveAddressStream::Done(result.map(|v| v.into_iter()));
                    }
                    None => return Err(ErrorCode::WouldBlock.into()),
                },
                ResolveAddressStream::Done(slot @ Err(_)) => {
                    // TODO: this `?` is what converts `io::Error` into `Error`
                    // and the conversion is not great right now. The standard
                    // library doesn't expose a ton of information through the
                    // return value of `getaddrinfo` right now so supporting a
                    // richer conversion here will probably require calling
                    // `getaddrinfo` directly.
                    mem::replace(slot, Ok(Vec::new().into_iter()))?;
                    unreachable!();
                }
                ResolveAddressStream::Done(Ok(iter)) => return Ok(iter.next()),
            }
        }
    }

    fn subscribe(
        &mut self,
        resource: Resource<ResolveAddressStream>,
    ) -> Result<Resource<Pollable>> {
        Ok(self.table_mut().push_host_pollable_resource(&resource)?)
    }

    fn drop(&mut self, resource: Resource<ResolveAddressStream>) -> Result<()> {
        self.table_mut().delete_resource(resource)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Subscribe for ResolveAddressStream {
    async fn ready(&mut self) -> Result<()> {
        if let ResolveAddressStream::Waiting(future) = self {
            *self = ResolveAddressStream::Done(future.await.map(|v| v.into_iter()));
        }
        Ok(())
    }
}
