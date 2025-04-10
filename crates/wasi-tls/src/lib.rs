//! # Wasmtime's [wasi-tls] (Transport Layer Security) Implementation
//!
//! This crate provides the Wasmtime host implementation for the [wasi-tls] API.
//! The [wasi-tls] world allows WebAssembly modules to perform SSL/TLS operations,
//! such as establishing secure connections to servers. TLS often relies on other wasi networking systems
//! to provide the stream so it will be common to enable the [wasi:cli] world as well with the networking features enabled.
//!
//! # An example of how to configure [wasi-tls] is the following:
//!
//! ```rust
//! use wasmtime_wasi::{IoView, WasiCtx, WasiCtxBuilder, WasiView};
//! use wasmtime::{
//!     component::{Linker, ResourceTable},
//!     Store, Engine, Result, Config
//! };
//! use wasmtime_wasi_tls::{LinkOptions, WasiTlsCtx};
//!
//! struct Ctx {
//!     table: ResourceTable,
//!     wasi_ctx: WasiCtx,
//! }
//!
//! impl IoView for Ctx {
//!     fn table(&mut self) -> &mut ResourceTable {
//!         &mut self.table
//!     }
//! }
//!
//! impl WasiView for Ctx {
//!     fn ctx(&mut self) -> &mut WasiCtx {
//!         &mut self.wasi_ctx
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let ctx = Ctx {
//!         table: ResourceTable::new(),
//!         wasi_ctx: WasiCtxBuilder::new()
//!             .inherit_stderr()
//!             .inherit_network()
//!             .allow_ip_name_lookup(true)
//!             .build(),
//!     };
//!
//!     let mut config = Config::new();
//!     config.async_support(true);
//!     let engine = Engine::new(&config)?;
//!
//!     // Set up wasi-cli
//!     let mut store = Store::new(&engine, ctx);
//!     let mut linker = Linker::new(&engine);
//!     wasmtime_wasi::add_to_linker_async(&mut linker)?;
//!
//!     // Add wasi-tls types and turn on the feature in linker
//!     let mut opts = LinkOptions::default();
//!     opts.tls(true);
//!     wasmtime_wasi_tls::add_to_linker(&mut linker, &mut opts, |h: &mut Ctx| {
//!         WasiTlsCtx::new(&mut h.table)
//!     })?;
//!
//!     // ... use `linker` to instantiate within `store` ...
//!     Ok(())
//! }
//!
//! ```
//! [wasi-tls]: https://github.com/WebAssembly/wasi-tls
//! [wasi:cli]: https://docs.rs/wasmtime-wasi/latest

#![deny(missing_docs)]
#![doc(test(attr(deny(warnings))))]
#![doc(test(attr(allow(dead_code, unused_variables, unused_mut))))]

use anyhow::Result;
use rustls::pki_types::ServerName;
use std::io;
use std::sync::Arc;
use std::{future::Future, mem, pin::Pin, sync::LazyLock};
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::pipe::{
    AsyncReadStream, InputStreamReader, OutputStreamReader, SharedAsyncWriteStream,
    SharedAsyncWriteStreamHandle,
};
use wasmtime_wasi::{
    async_trait, DynInputStream, DynOutputStream, DynPollable, IoError, Pollable, StreamError,
};

mod gen_ {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "wasi:tls/imports",
        with: {
            "wasi:io": wasmtime_wasi::bindings::io,
            "wasi:tls/types/client-connection": super::ClientConnection,
            "wasi:tls/types/client-handshake": super::ClientHandShake,
            "wasi:tls/types/future-client-streams": super::FutureClientStreams,
        },
        trappable_imports: true,
        async: {
            only_imports: [],
        }
    });
}
pub use gen_::wasi::tls::types::LinkOptions;
use gen_::wasi::tls::{self as generated};

fn default_client_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: LazyLock<Arc<rustls::ClientConfig>> = LazyLock::new(|| {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
        };
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(config)
    });
    Arc::clone(&CONFIG)
}

const MAX_READ: usize = 1024 * 1024;
const MAX_WRITE: usize = 1024 * 1024;

/// Wasi TLS context needed fro internal `wasi-tls`` state
pub struct WasiTlsCtx<'a> {
    table: &'a mut ResourceTable,
}

impl<'a> WasiTlsCtx<'a> {
    /// Create a new Wasi TLS context
    pub fn new(table: &'a mut ResourceTable) -> Self {
        Self { table }
    }
}

impl<'a> generated::types::Host for WasiTlsCtx<'a> {}

/// Add the `wasi-tls` world's types to a [`wasmtime::component::Linker`].
pub fn add_to_linker<T: Send>(
    l: &mut wasmtime::component::Linker<T>,
    opts: &mut LinkOptions,
    f: impl Fn(&mut T) -> WasiTlsCtx + Send + Sync + Copy + 'static,
) -> Result<()> {
    generated::types::add_to_linker_get_host(l, &opts, f)?;
    Ok(())
}

enum TlsError {
    /// The component should trap. Under normal circumstances, this only occurs
    /// when the underlying transport stream returns [`StreamError::Trap`].
    Trap(anyhow::Error),

    /// A failure indicated by the underlying transport stream as
    /// [`StreamError::LastOperationFailed`].
    Io(wasmtime_wasi::IoError),

    /// A TLS protocol error occurred.
    Tls(rustls::Error),
}

impl TlsError {
    /// Create a [`TlsError::Tls`] error from a simple message.
    fn msg(msg: &str) -> Self {
        // (Ab)using rustls' error type to synthesize our own TLS errors:
        Self::Tls(rustls::Error::General(msg.to_string()))
    }
}

impl From<io::Error> for TlsError {
    fn from(error: io::Error) -> Self {
        // Report unexpected EOFs as an error to prevent truncation attacks.
        // See: https://docs.rs/rustls/latest/rustls/struct.Reader.html#method.read
        if let io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof = error.kind() {
            return Self::msg("underlying transport closed abruptly");
        }

        // Errors from underlying transport.
        // These have been wrapped inside `io::Error`s by our wasi-to-tokio stream transformer below.
        let error = match error.downcast::<StreamError>() {
            Ok(StreamError::LastOperationFailed(e)) => return Self::Io(e),
            Ok(StreamError::Trap(e)) => return Self::Trap(e),
            Ok(StreamError::Closed) => unreachable!("our wasi-to-tokio stream transformer should have translated this to a 0-sized read"),
            Err(e) => e,
        };

        // Errors from `rustls`.
        // These have been wrapped inside `io::Error`s by `tokio-rustls`.
        let error = match error.downcast::<rustls::Error>() {
            Ok(e) => return Self::Tls(e),
            Err(e) => e,
        };

        // All errors should have been handled by the clauses above.
        Self::Trap(anyhow::Error::new(error).context("unknown wasi-tls error"))
    }
}

type Transport =
    tokio::io::Join<InputStreamReader<DynInputStream>, OutputStreamReader<DynOutputStream>>;
type ClientTlsStream = tokio_rustls::client::TlsStream<Transport>;

///  Represents the ClientHandshake which will be used to configure the handshake
pub struct ClientHandShake {
    server_name: String,
    transport: Transport,
}

impl<'a> generated::types::HostClientHandshake for WasiTlsCtx<'a> {
    fn new(
        &mut self,
        server_name: String,
        input: Resource<DynInputStream>,
        output: Resource<DynOutputStream>,
    ) -> wasmtime::Result<Resource<ClientHandShake>> {
        let input = self.table.delete(input)?;
        let output = self.table.delete(output)?;
        Ok(self.table.push(ClientHandShake {
            server_name,
            transport: tokio::io::join(
                InputStreamReader::new(input),
                OutputStreamReader::new(output),
            ),
        })?)
    }

    fn finish(
        &mut self,
        this: wasmtime::component::Resource<ClientHandShake>,
    ) -> wasmtime::Result<Resource<FutureClientStreams>> {
        let handshake = self.table.delete(this)?;
        let server_name = handshake.server_name;
        let transport = handshake.transport;

        Ok(self
            .table
            .push(FutureStreams(FutureState::Pending(Box::pin(async move {
                let domain = ServerName::try_from(server_name)
                    .map_err(|_| TlsError::msg("invalid server name"))?;

                let stream = tokio_rustls::TlsConnector::from(default_client_config())
                    .connect(domain, transport)
                    .await?;
                Ok(stream)
            }))))?)
    }

    fn drop(
        &mut self,
        this: wasmtime::component::Resource<ClientHandShake>,
    ) -> wasmtime::Result<()> {
        self.table.delete(this)?;
        Ok(())
    }
}

enum FutureState<T> {
    Pending(Pin<Box<dyn Future<Output = T> + Send>>),
    Ready(T),
    Consumed,
}

/// Future streams provides the tls streams after the handshake is completed
pub struct FutureStreams<T>(FutureState<Result<T, TlsError>>);

/// Library specific version of TLS connection after the handshake is completed.
/// This alias allows it to use with wit-bindgen component generator which won't take generic types
pub type FutureClientStreams = FutureStreams<ClientTlsStream>;

#[async_trait]
impl<T: Send + 'static> Pollable for FutureStreams<T> {
    async fn ready(&mut self) {
        match &mut self.0 {
            FutureState::Ready(_) | FutureState::Consumed => return,
            FutureState::Pending(task) => self.0 = FutureState::Ready(task.as_mut().await),
        }
    }
}

impl<'a> generated::types::HostFutureClientStreams for WasiTlsCtx<'a> {
    fn subscribe(
        &mut self,
        this: wasmtime::component::Resource<FutureClientStreams>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        wasmtime_wasi::subscribe(self.table, this)
    }

    fn get(
        &mut self,
        this: wasmtime::component::Resource<FutureClientStreams>,
    ) -> wasmtime::Result<
        Option<
            Result<
                Result<
                    (
                        Resource<ClientConnection>,
                        Resource<DynInputStream>,
                        Resource<DynOutputStream>,
                    ),
                    Resource<IoError>,
                >,
                (),
            >,
        >,
    > {
        let this = &mut self.table.get_mut(&this)?.0;
        match this {
            FutureState::Pending(_) => return Ok(None),
            FutureState::Consumed => return Ok(Some(Err(()))),
            FutureState::Ready(_) => (),
        }

        let FutureState::Ready(result) = mem::replace(this, FutureState::Consumed) else {
            unreachable!()
        };

        let tls_stream = match result {
            Ok(s) => s,
            Err(TlsError::Trap(e)) => return Err(e),
            Err(TlsError::Io(e)) => {
                let error = self.table.push(e)?;
                return Ok(Some(Ok(Err(error))));
            }
            Err(TlsError::Tls(e)) => {
                let error = self.table.push(wasmtime_wasi::IoError::new(e))?;
                return Ok(Some(Ok(Err(error))));
            }
        };

        let (rx, tx) = tokio::io::split(tls_stream);

        let input = AsyncReadStream::new(MAX_READ, rx);
        let output = SharedAsyncWriteStream::new(MAX_WRITE, tx);
        let client = ClientConnection {
            writer: output.handle(),
        };
        let input: DynInputStream = Box::new(input);
        let output: DynOutputStream = Box::new(output);

        let client = self.table.push(client)?;
        let input = self.table.push_child(input, &client)?;
        let output = self.table.push_child(output, &client)?;

        Ok(Some(Ok(Ok((client, input, output)))))
    }

    fn drop(
        &mut self,
        this: wasmtime::component::Resource<FutureClientStreams>,
    ) -> wasmtime::Result<()> {
        self.table.delete(this)?;
        Ok(())
    }
}

/// Represents the client connection and used to shut down the tls stream
pub struct ClientConnection {
    writer: SharedAsyncWriteStreamHandle<tokio::io::WriteHalf<ClientTlsStream>>,
}

impl<'a> generated::types::HostClientConnection for WasiTlsCtx<'a> {
    fn close_output(&mut self, this: Resource<ClientConnection>) -> wasmtime::Result<()> {
        _ = self.table.get_mut(&this)?.writer.shutdown();
        Ok(())
    }

    fn drop(&mut self, this: Resource<ClientConnection>) -> wasmtime::Result<()> {
        self.table.delete(this)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn test_future_client_streams_ready_can_be_canceled() {
        let (tx1, rx1) = oneshot::channel::<()>();

        let mut future_streams = FutureStreams(FutureState::Pending(Box::pin(async move {
            rx1.await
                .map_err(|_| TlsError::Trap(anyhow::anyhow!("oneshot canceled")))
        })));

        let mut fut = future_streams.ready();

        let mut cx = std::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(fut.as_mut().poll(&mut cx).is_pending());

        //cancel the readiness check
        drop(fut);

        match future_streams.0 {
            FutureState::Consumed => panic!("First future should be in Pending/ready state"),
            _ => (),
        }

        // make it ready and wait for it to progress
        tx1.send(()).unwrap();
        future_streams.ready().await;

        match future_streams.0 {
            FutureState::Ready(Ok(())) => (),
            _ => panic!("First future should be in Ready(Err) state"),
        }
    }
}
