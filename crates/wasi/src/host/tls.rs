use crate::bindings::io::streams::{InputStream, OutputStream};
use crate::pipe::AsyncReadStream;
use crate::runtime::AbortOnDropJoinHandle;
use crate::stream::DuplexAsyncReadWrite;
use crate::{
    HostInputStream, HostOutputStream, Pollable, PollableFuture, StreamError, Subscribe, WasiImpl,
    WasiView,
};
use rustls::pki_types::ServerName;
use std::io;
use std::sync::{Arc, LazyLock};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use wasmtime::component::{Resource, ResourceTable};

type TlsStream = tokio_rustls::client::TlsStream<DuplexAsyncReadWrite>;

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

impl<T> crate::bindings::sockets::tls::Host for WasiImpl<T> where T: WasiView {}

pub struct ClientHandshake {
    tombstones: Tombstones,
    server_name: String,
    input: InputStream,
    output: OutputStream,
}

pub struct FutureClientStreams {
    tombstones: Option<Tombstones>,
    connect: PollableFuture<AbortOnDropJoinHandle<anyhow::Result<TlsStream>>>,
}

#[async_trait::async_trait]
impl Subscribe for FutureClientStreams {
    async fn ready(&mut self) {
        self.connect.ready().await
    }
}

pub struct ClientConnection {
    tombstones: Tombstones,
    writer: Arc<Mutex<TlsWriter>>,
    protocol_version: rustls::ProtocolVersion,
    cipher_suite: rustls::CipherSuite,
}

#[async_trait::async_trait]
impl<T> crate::bindings::sockets::tls::HostClientConnection for WasiImpl<T>
where
    T: WasiView,
{
    fn close_notify(
        &mut self,
        this: Resource<ClientConnection>,
    ) -> wasmtime::Result<Option<Result<(), Resource<crate::stream::Error>>>> {
        let result = self
            .table()
            .get_mut(&this)?
            .writer
            .try_lock()
            .map_err(|_| StreamError::trap("concurrent access to resource not supported"))?
            .close_notify();

        Ok(match result {
            None => None,
            Some(Ok(())) => Some(Ok(())),
            Some(Err(e)) => Some(Err(self.table().push(e)?)),
        })
    }

    fn protocol_version(&mut self, this: Resource<ClientConnection>) -> wasmtime::Result<u16> {
        let connection = self.table().get(&this)?;

        Ok(connection.protocol_version.get_u16())
    }

    fn cipher_suite(&mut self, this: Resource<ClientConnection>) -> wasmtime::Result<u16> {
        let connection = self.table().get(&this)?;

        Ok(connection.cipher_suite.get_u16())
    }

    fn drop(&mut self, this: Resource<ClientConnection>) -> wasmtime::Result<()> {
        self.table().delete(this)?.tombstones.delete(self.table())?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl<T> crate::bindings::sockets::tls::HostClientHandshake for WasiImpl<T>
where
    T: WasiView,
{
    fn new(
        &mut self,
        server_name: String,
        input: Resource<InputStream>,
        output: Resource<OutputStream>,
    ) -> wasmtime::Result<Resource<ClientHandshake>> {
        // Take the provided streams out of the table, but keep their indexes
        // alive to prevent the child resources outliving their parents:
        let input_stream = std::mem::replace(self.table().get_mut(&input)?, Box::new(Tombstone));
        let output_stream = std::mem::replace(self.table().get_mut(&output)?, Box::new(Tombstone));

        Ok(self.table().push(ClientHandshake {
            server_name,
            input: input_stream,
            output: output_stream,
            tombstones: Tombstones {
                input: input,
                output: output,
            },
        })?)
    }

    fn finish(
        &mut self,
        this: Resource<ClientHandshake>,
    ) -> wasmtime::Result<Resource<FutureClientStreams>> {
        let handshake = self.table().delete(this)?;

        let server_name = handshake.server_name;
        let inner_stream = DuplexAsyncReadWrite::new(handshake.input, handshake.output);

        Ok(self.table().push(FutureClientStreams {
            tombstones: Some(handshake.tombstones),
            connect: PollableFuture::Pending(crate::runtime::spawn(async move {
                let connector = tokio_rustls::TlsConnector::from(default_client_config());
                let domain = ServerName::try_from(server_name)?; // TODO: convert error
                let stream = connector.connect(domain, inner_stream).await?; // TODO: convert error
                Ok(stream)
            })),
        })?)
    }

    fn drop(&mut self, this: Resource<ClientHandshake>) -> wasmtime::Result<()> {
        self.table().delete(this)?.tombstones.delete(self.table())?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl<T> crate::bindings::sockets::tls::HostFutureClientStreams for WasiImpl<T>
where
    T: WasiView,
{
    fn subscribe(
        &mut self,
        this: Resource<FutureClientStreams>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        crate::poll::subscribe(self.table(), this)
    }

    fn get(
        &mut self,
        this: Resource<FutureClientStreams>,
    ) -> wasmtime::Result<
        Option<
            Result<
                Result<
                    (
                        Resource<ClientConnection>,
                        Resource<InputStream>,
                        Resource<OutputStream>,
                    ),
                    (),
                >,
                (),
            >,
        >,
    > {
        let this = self.table().get_mut(&this)?;

        let tls_stream = match this.connect {
            PollableFuture::Ready(_) => match this.connect.take().unwrap_ready() {
                Ok(tls_stream) => tls_stream,
                Err(e) => return Ok(Some(Ok(Err(())))), // TODO: don't throw away error
            },
            PollableFuture::Pending(_) => return Ok(None),
            PollableFuture::Consumed => return Ok(Some(Err(()))),
        };
        let tls_connection = tls_stream.get_ref().1;

        let protocol_version = tls_connection
            .protocol_version()
            .expect("protocol_version to be available after handshake");
        let cipher_suite = tls_connection
            .negotiated_cipher_suite()
            .expect("negotiated_cipher_suite to be available after handshake")
            .suite();

        let (rx, tx) = tokio::io::split(tls_stream);
        let writer = Arc::new(Mutex::new(TlsWriter::new(tx)));

        let client = ClientConnection {
            tombstones: this.tombstones.take().unwrap(),
            writer: writer.clone(),
            protocol_version,
            cipher_suite,
        };

        let input: InputStream = Box::new(AsyncReadStream::new(rx));
        let output: OutputStream = Box::new(TlsWriteStream(writer));

        let client = self.table().push(client)?;
        let input = self.table().push_child(input, &client)?;
        let output = self.table().push_child(output, &client)?;

        Ok(Some(Ok(Ok((client, input, output)))))
    }

    fn drop(&mut self, this: Resource<FutureClientStreams>) -> wasmtime::Result<()> {
        let mut future = self.table().delete(this)?;

        match future.tombstones.take() {
            Some(c) => c.delete(self.table())?,
            None => {}
        }
        Ok(())
    }
}

const READY_SIZE: usize = 1024 * 1024 * 1024;

type WriteHalf = tokio::io::WriteHalf<tokio_rustls::client::TlsStream<DuplexAsyncReadWrite>>;

struct TlsWriter {
    state: WriteState,
}

enum WriteState {
    Ready(WriteHalf),
    Writing(AbortOnDropJoinHandle<io::Result<WriteHalf>>),
    Closing(AbortOnDropJoinHandle<io::Result<()>>),
    Closed,
    Error(io::Error),
}

impl TlsWriter {
    fn new(stream: WriteHalf) -> Self {
        Self {
            state: WriteState::Ready(stream),
        }
    }

    fn write(&mut self, mut bytes: bytes::Bytes) -> Result<(), StreamError> {
        let WriteState::Ready(_) = self.state else {
            return Err(StreamError::Trap(anyhow::anyhow!(
                "unpermitted: must call check_write first"
            )));
        };

        if bytes.is_empty() {
            return Ok(());
        }

        let WriteState::Ready(mut stream) = std::mem::replace(&mut self.state, WriteState::Closed)
        else {
            unreachable!()
        };

        self.state = WriteState::Writing(crate::runtime::spawn(async move {
            while !bytes.is_empty() {
                match stream.write(&bytes).await {
                    Ok(n) => {
                        let _ = bytes.split_to(n);
                    }
                    Err(e) => return Err(e.into()),
                }
            }

            Ok(stream)
        }));

        Ok(())
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        // `flush` is a no-op here, as we're not managing any internal buffer. Additionally,
        // `write_ready` will join the background write task if it's active, so following `flush`
        // with `write_ready` will have the desired effect.
        match self.state {
            WriteState::Ready(_)
            | WriteState::Writing(_)
            | WriteState::Closing(_)
            | WriteState::Error(_) => Ok(()),
            WriteState::Closed => Err(StreamError::Closed),
        }
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        match &mut self.state {
            WriteState::Ready(_) => Ok(READY_SIZE),
            WriteState::Writing(_) => Ok(0),
            WriteState::Closing(_) => Ok(0),
            WriteState::Closed => Err(StreamError::Closed),
            WriteState::Error(_) => {
                let WriteState::Error(e) = std::mem::replace(&mut self.state, WriteState::Closed)
                else {
                    unreachable!()
                };

                Err(StreamError::LastOperationFailed(e.into()))
            }
        }
    }

    fn close_notify(&mut self) -> Option<Result<(), crate::stream::Error>> {
        match std::mem::replace(&mut self.state, WriteState::Closed) {
            // No write in progress, immediately shut down:
            WriteState::Ready(mut stream) => {
                self.state =
                    WriteState::Closing(crate::runtime::spawn(
                        async move { stream.shutdown().await },
                    ));
                None
            }

            // Schedule the shutdown after the current write has finished:
            WriteState::Writing(write) => {
                self.state = WriteState::Closing(crate::runtime::spawn(async move {
                    let mut stream = write.await?;
                    stream.shutdown().await
                }));
                None
            }

            WriteState::Closing(t) => {
                self.state = WriteState::Closing(t);
                None
            }
            WriteState::Closed => Some(Ok(())),
            WriteState::Error(e) => Some(Err(e.into())),
        }
    }

    async fn cancel(&mut self) {
        match std::mem::replace(&mut self.state, WriteState::Closed) {
            WriteState::Writing(task) => _ = task.cancel().await,
            WriteState::Closing(task) => _ = task.cancel().await,
            _ => {}
        }
    }

    async fn ready(&mut self) {
        match &mut self.state {
            WriteState::Writing(task) => {
                self.state = match task.await {
                    Ok(s) => WriteState::Ready(s),
                    Err(e) => WriteState::Error(e),
                }
            }
            WriteState::Closing(task) => {
                self.state = match task.await {
                    Ok(()) => WriteState::Closed,
                    Err(e) => WriteState::Error(e),
                }
            }
            _ => {}
        }
    }
}

struct TlsWriteStream(Arc<Mutex<TlsWriter>>);

#[async_trait::async_trait]
impl HostOutputStream for TlsWriteStream {
    fn write(&mut self, bytes: bytes::Bytes) -> Result<(), StreamError> {
        try_lock_for_stream(&self.0)?.write(bytes)
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        try_lock_for_stream(&self.0)?.flush()
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        try_lock_for_stream(&self.0)?.check_write()
    }

    async fn cancel(&mut self) {
        self.0.lock().await.cancel().await
    }
}

#[async_trait::async_trait]
impl Subscribe for TlsWriteStream {
    async fn ready(&mut self) {
        self.0.lock().await.ready().await
    }
}

fn try_lock_for_stream<T>(mutex: &Mutex<T>) -> Result<tokio::sync::MutexGuard<'_, T>, StreamError> {
    mutex
        .try_lock()
        .map_err(|_| StreamError::trap("concurrent access to resource not supported"))
}

/// Placeholder stream type that is substituted in place to keep the
/// parent<->child resource lifetime restrictions in tact.
struct Tombstone;

#[async_trait::async_trait]
impl HostInputStream for Tombstone {
    fn read(&mut self, _size: usize) -> Result<bytes::Bytes, StreamError> {
        Err(StreamError::trap("stream has been consumed"))
    }
}

#[async_trait::async_trait]
impl HostOutputStream for Tombstone {
    fn write(&mut self, _bytes: bytes::Bytes) -> Result<(), StreamError> {
        Err(StreamError::trap("stream has been consumed"))
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        Err(StreamError::trap("stream has been consumed"))
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        Err(StreamError::trap("stream has been consumed"))
    }
}

#[async_trait::async_trait]
impl Subscribe for Tombstone {
    async fn ready(&mut self) {}
}

struct Tombstones {
    input: Resource<InputStream>,
    output: Resource<OutputStream>,
}
impl Tombstones {
    fn delete(self, table: &mut ResourceTable) -> wasmtime::Result<()> {
        table.delete(self.input)?;
        table.delete(self.output)?;
        Ok(())
    }
}
