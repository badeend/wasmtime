//! Utility types for converting Tokio `AsyncRead`/`AsyncWrite` resources into
//! WASI streams and vice versa.

use crate::{
    async_trait, runtime::AbortOnDropJoinHandle, InputStream, OutputStream, Pollable, StreamError,
    StreamResult,
};
use bytes::{Buf, Bytes, BytesMut};
use futures::FutureExt;
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::Poll,
};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

enum AsyncReadState<IO> {
    Ready(IO, Bytes),
    Reading(AbortOnDropJoinHandle<io::Result<(IO, Bytes)>>),
    Canceling(AbortOnDropJoinHandle<io::Result<(IO, Bytes)>>, StreamError),
    Closed(StreamError),
}

pub struct AsyncReadStream<IO> {
    state: AsyncReadState<IO>,
    max_read: usize,
}

impl<IO> AsyncReadStream<IO>
where
    IO: AsyncRead + Unpin + Send + 'static,
{
    pub fn new(max_read: usize, resource: IO) -> Self {
        Self {
            state: AsyncReadState::Ready(resource, Bytes::new()),
            max_read,
        }
    }

    pub fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        // Attempt to join finished background tasks first:
        _ = self.poll_ready(&mut noop_context());

        let (inner, bytes) = match &mut self.state {
            AsyncReadState::Ready(inner, bytes) => (inner, bytes),
            AsyncReadState::Reading(_) | AsyncReadState::Canceling(..) => return Ok(Bytes::new()),
            AsyncReadState::Closed(e) => return Err(std::mem::replace(e, StreamError::Closed)),
        };

        if size == 0 {
            return Ok(Bytes::new());
        }

        // Attempt to read from the in-memory buffer:
        let chunk = bytes.split_to(bytes.len().min(size));
        if !chunk.is_empty() {
            return Ok(chunk);
        }

        // Attempt to perform the read immediately without sending it to a background task.
        let mut buf = BytesMut::zeroed(self.max_read.min(size));
        match Self::try_read(inner, &mut buf) {
            Ok(0) => {
                self.state = AsyncReadState::Closed(StreamError::Closed);
                return Err(StreamError::Closed);
            }
            Ok(bytes_read) => {
                buf.truncate(bytes_read);
                return Ok(buf.freeze());
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => {
                self.state = AsyncReadState::Closed(StreamError::Closed);
                return Err(StreamError::LastOperationFailed(e.into()));
            }
        }

        // Perform read in the background.
        let mut inner = self.take_ready().unwrap();
        self.state = AsyncReadState::Reading(crate::runtime::spawn(async move {
            let bytes_read = inner.read(&mut buf).await?;
            buf.truncate(bytes_read);
            return Ok((inner, buf.freeze()));
        }));

        Ok(Bytes::new())
    }

    /// Initiate cancellation of the associated WASI input stream. This stops
    /// the input stream from accepting new reads. Any active read operation
    /// will be aborted and pending buffers will be dropped. The first following
    /// read from the WASI guest will return the provided `error` value. If the
    /// stream is already closed, this function does nothing.
    ///
    /// Cancellation is not guaranteed to finish instantly. Use [Self::ready]
    /// to wait for its completion.
    pub fn cancel(&mut self, error: StreamError) {
        match &mut self.state {
            AsyncReadState::Reading(_) => {
                let mut task = self.take_reading().unwrap();
                task.abort(); // Notify the task to wrap up ASAP. Cancellation is not guaranteed to be immediate.
                self.state = AsyncReadState::Canceling(task, error);
            }
            AsyncReadState::Ready(..) => {
                self.state = AsyncReadState::Closed(error);
            }
            AsyncReadState::Canceling(..) | AsyncReadState::Closed(_) => {}
        }
    }

    /// Poll the associated WASI stream to be ready for reading or for the
    /// stream to be closed.
    pub fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<()> {
        match &mut self.state {
            AsyncReadState::Reading(task) => {
                self.state = match std::task::ready!(task.poll_unpin(cx)) {
                    Ok((resource, bytes)) => {
                        if bytes.is_empty() {
                            AsyncReadState::Closed(StreamError::Closed)
                        } else {
                            AsyncReadState::Ready(resource, bytes)
                        }
                    }
                    Err(e) => AsyncReadState::Closed(StreamError::LastOperationFailed(e.into())),
                }
            }
            AsyncReadState::Canceling(task, _) => {
                _ = std::task::ready!(task.poll_unpin(cx));
                self.state = AsyncReadState::Closed(self.take_canceling().unwrap());
            }
            AsyncReadState::Ready(..) | AsyncReadState::Closed(_) => {}
        }

        Poll::Ready(())
    }

    fn take_ready(&mut self) -> Option<IO> {
        match std::mem::replace(&mut self.state, AsyncReadState::Closed(StreamError::Closed)) {
            AsyncReadState::Ready(inner, _) => Some(inner),
            _ => None,
        }
    }

    fn take_reading(&mut self) -> Option<AbortOnDropJoinHandle<io::Result<(IO, Bytes)>>> {
        match std::mem::replace(&mut self.state, AsyncReadState::Closed(StreamError::Closed)) {
            AsyncReadState::Reading(task) => Some(task),
            _ => None,
        }
    }

    fn take_canceling(&mut self) -> Option<StreamError> {
        match std::mem::replace(&mut self.state, AsyncReadState::Closed(StreamError::Closed)) {
            AsyncReadState::Canceling(_, error) => Some(error),
            _ => None,
        }
    }

    fn try_read(io: &mut IO, buf: &mut [u8]) -> io::Result<usize> {
        try_io(|cx| {
            let mut buf = tokio::io::ReadBuf::new(buf);
            std::pin::pin!(io)
                .poll_read(cx, &mut buf)
                .map_ok(|_| buf.filled().len())
        })
    }
}

#[async_trait]
impl<IO> InputStream for AsyncReadStream<IO>
where
    IO: AsyncRead + Unpin + Send + 'static,
{
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        self.read(size)
    }

    async fn cancel(&mut self) {
        self.cancel(StreamError::Closed);
        self.ready().await;
    }
}

#[async_trait]
impl<IO> Pollable for AsyncReadStream<IO>
where
    IO: AsyncRead + Unpin + Send + 'static,
{
    async fn ready(&mut self) {
        std::future::poll_fn(|cx| self.poll_ready(cx)).await
    }
}

pub struct SharedAsyncReadStream<IO>(Arc<Mutex<AsyncReadStream<IO>>>);
pub struct SharedAsyncReadStreamHandle<IO>(Arc<Mutex<AsyncReadStream<IO>>>);

impl<IO> SharedAsyncReadStream<IO>
where
    IO: AsyncRead + Unpin + Send + 'static,
{
    pub fn new(max_read: usize, resource: IO) -> Self {
        Self(Arc::new(Mutex::new(AsyncReadStream::new(
            max_read, resource,
        ))))
    }

    pub fn handle(&self) -> SharedAsyncReadStreamHandle<IO> {
        SharedAsyncReadStreamHandle(self.0.clone())
    }

    fn lock(&mut self) -> std::sync::MutexGuard<'_, AsyncReadStream<IO>> {
        self.0.lock().expect("other thread panicked")
    }
}
impl<IO> SharedAsyncReadStreamHandle<IO>
where
    IO: AsyncRead + Unpin + Send + 'static,
{
    /// Initiate cancellation of the associated WASI input stream. This stops
    /// the input stream from accepting new reads. Any active read operation
    /// will be aborted and pending buffers will be dropped. The first following
    /// read from the WASI guest will return the provided `error` value. If the
    /// stream is already closed, this function does nothing.
    ///
    /// Cancellation is not guaranteed to finish instantly. Use [Self::ready]
    /// to wait for its completion.
    pub fn cancel(&self, error: StreamError) {
        self.lock().cancel(error)
    }

    /// Wait for the associated WASI stream to be ready for reading or for the
    /// stream to be closed.
    pub async fn ready(&self) {
        std::future::poll_fn(|cx| self.lock().poll_ready(cx)).await
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AsyncReadStream<IO>> {
        self.0.lock().expect("other thread panicked")
    }
}

#[async_trait]
impl<IO> InputStream for SharedAsyncReadStream<IO>
where
    IO: AsyncRead + Unpin + Send + 'static,
{
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        self.lock().read(size)
    }

    async fn cancel(&mut self) {
        self.lock().cancel(StreamError::Closed);
        self.ready().await;
    }
}

#[async_trait]
impl<IO> Pollable for SharedAsyncReadStream<IO>
where
    IO: AsyncRead + Unpin + Send + 'static,
{
    async fn ready(&mut self) {
        std::future::poll_fn(|cx| self.lock().poll_ready(cx)).await
    }
}

enum AsyncWriteState<IO> {
    Ready(IO, usize),
    Writing(AbortOnDropJoinHandle<io::Result<IO>>),
    Flushing(AbortOnDropJoinHandle<io::Result<IO>>),
    Closing(AbortOnDropJoinHandle<io::Result<IO>>),
    Canceling(AbortOnDropJoinHandle<io::Result<IO>>, StreamError),
    Closed(StreamError),
}

pub struct AsyncWriteStream<IO> {
    state: AsyncWriteState<IO>,
    max_write: usize,
}

impl<IO> AsyncWriteStream<IO>
where
    IO: AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(max_write: usize, resource: IO) -> Self {
        Self {
            state: AsyncWriteState::Ready(resource, 0),
            max_write,
        }
    }

    pub fn check_write(&mut self) -> StreamResult<usize> {
        // Attempt to join finished background task first:
        _ = self.poll_ready(&mut noop_context());

        match &mut self.state {
            AsyncWriteState::Ready(_, permit) => {
                *permit = self.max_write;
                Ok(*permit)
            }
            AsyncWriteState::Writing(_)
            | AsyncWriteState::Flushing(_)
            | AsyncWriteState::Closing(_)
            | AsyncWriteState::Canceling(..) => Ok(0),
            AsyncWriteState::Closed(_) => Err(self.take_error().unwrap()),
        }
    }

    pub fn write(&mut self, mut bytes: Bytes) -> StreamResult<()> {
        let inner = match &mut self.state {
            AsyncWriteState::Ready(inner, permit) if bytes.len() <= *permit => inner,
            _ => return Err(StreamError::Trap(anyhow::anyhow!("write not permitted"))),
        };

        if bytes.is_empty() {
            return Ok(());
        }

        // Attempt to perform the write immediately without sending it to a background task.
        match Self::try_write(inner, &bytes) {
            Ok(0) => {
                self.state = AsyncWriteState::Closed(StreamError::Closed);
                return Err(StreamError::Closed);
            }
            Ok(n) => {
                bytes.advance(n);

                if bytes.is_empty() {
                    return Ok(());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                self.state = AsyncWriteState::Closed(StreamError::Closed);
                return Err(StreamError::Closed);
            }
            Err(e) => {
                self.state = AsyncWriteState::Closed(StreamError::Closed);
                return Err(StreamError::LastOperationFailed(e.into()));
            }
        }

        // Perform remainder of the write in the background.
        let mut inner = self.take_ready().unwrap();
        self.state = AsyncWriteState::Writing(crate::runtime::spawn(async move {
            inner.write_all(&bytes).await?;
            Ok(inner)
        }));

        Ok(())
    }

    pub fn flush(&mut self) -> StreamResult<()> {
        match &mut self.state {
            AsyncWriteState::Ready(resource, permit) => {
                *permit = 0;

                // Attempt to flush immediately without sending it to a background task.
                match Self::try_flush(resource) {
                    Ok(()) => return Ok(()),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        self.state = AsyncWriteState::Closed(StreamError::Closed);
                        return Err(StreamError::LastOperationFailed(e.into()));
                    }
                }

                // Perform flush in the background.
                let mut inner = self.take_ready().unwrap();
                self.state = AsyncWriteState::Flushing(crate::runtime::spawn(async move {
                    inner.flush().await?;
                    Ok(inner)
                }));

                Ok(())
            }
            AsyncWriteState::Writing(_) => {
                let write = self.take_task().unwrap();

                // Schedule flush after the current write has finished:
                self.state = AsyncWriteState::Flushing(crate::runtime::spawn(async move {
                    let mut inner = write.await?;
                    inner.flush().await?;
                    Ok(inner)
                }));

                Ok(())
            }
            AsyncWriteState::Flushing(_)
            | AsyncWriteState::Closing(_)
            | AsyncWriteState::Canceling(..) => Ok(()),
            AsyncWriteState::Closed(_) => Err(self.take_error().unwrap()),
        }
    }

    /// Initiate graceful shutdown on the underlying [AsyncWrite] resource. This
    /// stops the WASI output stream from accepting new writes and the output
    /// stream won't report readiness until the shutdown sequence has completed.
    ///
    /// If the stream is already closed, this function does nothing and returns
    /// `Err(StreamError::Closed)`.
    pub fn shutdown(&mut self) -> StreamResult<()> {
        match &mut self.state {
            // No write in progress, immediately shut down:
            AsyncWriteState::Ready(inner, _) => {
                // Attempt to perform the shutdown immediately without sending it to a background task.
                match Self::try_shutdown(inner) {
                    Ok(()) => {
                        self.state = AsyncWriteState::Closed(StreamError::Closed);
                        return Ok(());
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        self.state =
                            AsyncWriteState::Closed(StreamError::LastOperationFailed(e.into()));
                        return Ok(());
                    }
                }

                // Execute shutdown sequence in the background.
                let mut inner = self.take_ready().unwrap();
                self.state = AsyncWriteState::Closing(crate::runtime::spawn(async move {
                    inner.shutdown().await?;
                    Ok(inner)
                }));

                Ok(())
            }

            // Schedule the shutdown after the current write has finished:
            AsyncWriteState::Writing(_) | AsyncWriteState::Flushing(_) => {
                let write = self.take_task().unwrap();
                self.state = AsyncWriteState::Closing(crate::runtime::spawn(async move {
                    let mut io = write.await?;
                    io.shutdown().await?;
                    Ok(io)
                }));

                Ok(())
            }

            AsyncWriteState::Closing(_) | AsyncWriteState::Canceling(..) => Ok(()),
            AsyncWriteState::Closed(_) => Err(StreamError::Closed),
        }
    }

    /// Initiate cancellation of the associated WASI output stream. This stops
    /// the output stream from accepting new writes. Any active write operation
    /// will be aborted and pending buffers will be dropped. The first following
    /// write from the WASI guest will return the provided `error` value.
    ///
    /// Cancellation is not guaranteed to finish instantly. Use [Self::ready]
    /// to wait for its completion.
    ///
    /// If the stream is already closed, this function does nothing.
    pub fn cancel(&mut self, error: StreamError) {
        match &mut self.state {
            AsyncWriteState::Writing(_)
            | AsyncWriteState::Flushing(_)
            | AsyncWriteState::Closing(_) => {
                let mut task = self.take_task().unwrap();
                task.abort(); // Notify the task to wrap up ASAP. Cancellation is not guaranteed to be immediate.
                self.state = AsyncWriteState::Canceling(task, error);
            }
            AsyncWriteState::Ready(..) => {
                self.state = AsyncWriteState::Closed(error);
            }
            AsyncWriteState::Canceling(..) | AsyncWriteState::Closed(_) => {}
        }
    }

    /// Poll the associated WASI stream to be ready for reading or for the
    /// stream to be closed.
    pub fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<()> {
        match &mut self.state {
            AsyncWriteState::Writing(task) | AsyncWriteState::Flushing(task) => {
                self.state = match std::task::ready!(task.poll_unpin(cx)) {
                    Ok(resource) => AsyncWriteState::Ready(resource, 0),
                    Err(e) => AsyncWriteState::Closed(StreamError::LastOperationFailed(e.into())),
                }
            }
            AsyncWriteState::Closing(task) => {
                self.state = match std::task::ready!(task.poll_unpin(cx)) {
                    Ok(_) => AsyncWriteState::Closed(StreamError::Closed),
                    Err(e) => AsyncWriteState::Closed(StreamError::LastOperationFailed(e.into())),
                }
            }
            AsyncWriteState::Canceling(task, _) => {
                _ = std::task::ready!(task.poll_unpin(cx));
                self.state = AsyncWriteState::Closed(self.take_canceling().unwrap());
            }
            AsyncWriteState::Ready(..) | AsyncWriteState::Closed(_) => {}
        }

        Poll::Ready(())
    }

    fn take_task(&mut self) -> Option<AbortOnDropJoinHandle<io::Result<IO>>> {
        match std::mem::replace(
            &mut self.state,
            AsyncWriteState::Closed(StreamError::Closed),
        ) {
            AsyncWriteState::Writing(task)
            | AsyncWriteState::Flushing(task)
            | AsyncWriteState::Closing(task)
            | AsyncWriteState::Canceling(task, _) => Some(task),
            AsyncWriteState::Ready(..) | AsyncWriteState::Closed(_) => None,
        }
    }

    fn take_ready(&mut self) -> Option<IO> {
        match std::mem::replace(
            &mut self.state,
            AsyncWriteState::Closed(StreamError::Closed),
        ) {
            AsyncWriteState::Ready(inner, _) => Some(inner),
            _ => None,
        }
    }

    fn take_canceling(&mut self) -> Option<StreamError> {
        match std::mem::replace(
            &mut self.state,
            AsyncWriteState::Closed(StreamError::Closed),
        ) {
            AsyncWriteState::Canceling(_, error) => Some(error),
            _ => None,
        }
    }

    fn take_error(&mut self) -> Option<StreamError> {
        match std::mem::replace(
            &mut self.state,
            AsyncWriteState::Closed(StreamError::Closed),
        ) {
            AsyncWriteState::Closed(e) => Some(e),
            _ => None,
        }
    }

    fn try_write(io: &mut IO, buf: &[u8]) -> io::Result<usize> {
        try_io(|cx| std::pin::pin!(io).poll_write(cx, buf))
    }

    fn try_flush(io: &mut IO) -> io::Result<()> {
        try_io(|cx| std::pin::pin!(io).poll_flush(cx))
    }

    fn try_shutdown(io: &mut IO) -> io::Result<()> {
        try_io(|cx| std::pin::pin!(io).poll_shutdown(cx))
    }
}

#[async_trait]
impl<IO> OutputStream for AsyncWriteStream<IO>
where
    IO: AsyncWrite + Unpin + Send + 'static,
{
    fn check_write(&mut self) -> StreamResult<usize> {
        self.check_write()
    }

    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.write(bytes)
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.flush()
    }

    async fn cancel(&mut self) {
        self.cancel(StreamError::Closed);
        self.ready().await;
    }
}

#[async_trait]
impl<IO> Pollable for AsyncWriteStream<IO>
where
    IO: AsyncWrite + Unpin + Send + 'static,
{
    async fn ready(&mut self) {
        std::future::poll_fn(|cx| self.poll_ready(cx)).await
    }
}

pub struct SharedAsyncWriteStream<IO>(Arc<Mutex<AsyncWriteStream<IO>>>);
pub struct SharedAsyncWriteStreamHandle<IO>(Arc<Mutex<AsyncWriteStream<IO>>>);

impl<IO> SharedAsyncWriteStream<IO>
where
    IO: AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(max_write: usize, resource: IO) -> Self {
        Self(Arc::new(Mutex::new(AsyncWriteStream::new(
            max_write, resource,
        ))))
    }

    pub fn handle(&self) -> SharedAsyncWriteStreamHandle<IO> {
        SharedAsyncWriteStreamHandle(self.0.clone())
    }

    fn lock(&mut self) -> std::sync::MutexGuard<'_, AsyncWriteStream<IO>> {
        self.0.lock().expect("other thread panicked")
    }
}
impl<IO> SharedAsyncWriteStreamHandle<IO>
where
    IO: AsyncWrite + Unpin + Send + 'static,
{
    pub fn shutdown(&self) -> StreamResult<()> {
        self.lock().shutdown()
    }

    pub fn cancel(&self, error: StreamError) {
        self.lock().cancel(error)
    }

    /// Wait for the associated WASI stream to be ready for reading or for the
    /// stream to be closed.
    pub async fn ready(&self) {
        std::future::poll_fn(|cx| self.lock().poll_ready(cx)).await
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AsyncWriteStream<IO>> {
        self.0.lock().expect("other thread panicked")
    }
}

#[async_trait]
impl<IO> OutputStream for SharedAsyncWriteStream<IO>
where
    IO: AsyncWrite + Unpin + Send + 'static,
{
    fn check_write(&mut self) -> StreamResult<usize> {
        self.lock().check_write()
    }

    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.lock().write(bytes)
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.lock().flush()
    }

    async fn cancel(&mut self) {
        self.lock().cancel(StreamError::Closed);
        self.ready().await;
    }
}

#[async_trait]
impl<IO> Pollable for SharedAsyncWriteStream<IO>
where
    IO: AsyncWrite + Unpin + Send + 'static,
{
    async fn ready(&mut self) {
        std::future::poll_fn(|cx| self.lock().poll_ready(cx)).await
    }
}

enum IoStreamState<IO> {
    Ready(IO),
    Pending(Pin<Box<dyn Future<Output = IO> + Send>>),
    Closed,
}

impl<IO: Pollable> IoStreamState<IO> {
    /// Call the provided function when the inner resource is ready for IO.
    /// The function should return [Poll::Pending] when it can not make progress.
    fn poll_ready<T, F>(
        &mut self,
        cx: &mut std::task::Context<'_>,
        mut f: F,
    ) -> Poll<StreamResult<T>>
    where
        F: FnMut(&mut IO) -> Poll<StreamResult<T>>,
    {
        const MAX_ATTEMPTS: usize = 10;
        let mut attempt = 0;
        loop {
            attempt += 1;

            let io = match self {
                Self::Ready(io) => io,
                Self::Pending(fut) => {
                    let io = std::task::ready!(fut.as_mut().poll(cx));
                    *self = Self::Ready(io);
                    if let Self::Ready(io) = self {
                        io
                    } else {
                        unreachable!()
                    }
                }
                Self::Closed => return Poll::Ready(Err(StreamError::Closed)),
            };

            match f(io) {
                Poll::Pending => {
                    let Self::Ready(mut io) = std::mem::replace(self, Self::Closed) else {
                        unreachable!()
                    };

                    *self = Self::Pending(Box::pin(async move {
                        io.ready().await;
                        io
                    }));

                    // The next iteration of the loop will poll the future.
                }
                Poll::Ready(Ok(val)) => return Poll::Ready(Ok(val)),
                Poll::Ready(Err(e)) => {
                    *self = IoStreamState::Closed;
                    return Poll::Ready(Err(e));
                }
            }

            if attempt > MAX_ATTEMPTS {
                *self = IoStreamState::Closed;
                return Poll::Ready(Err(StreamError::trap("bad stream implementation")));
            }
        }
    }
}

pub struct InputStreamReader<IO>(IoStreamState<IO>);

impl<IO: InputStream + Unpin> InputStreamReader<IO> {
    pub fn new(resource: IO) -> Self {
        Self(IoStreamState::Ready(resource))
    }

    fn map_result(r: StreamResult<()>) -> io::Result<()> {
        match r {
            Ok(()) => Ok(()),
            Err(StreamError::Closed) => Ok(()),
            Err(e) => Err(std::io::Error::other(e)),
        }
    }
}

impl<IO: InputStream + Unpin> AsyncRead for InputStreamReader<IO> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.0
            .poll_ready(cx, |stream| {
                if buf.remaining() == 0 {
                    return Poll::Ready(Ok(()));
                }

                match stream.read(buf.remaining()) {
                    Ok(bytes) if bytes.is_empty() => Poll::Pending,
                    Ok(bytes) => {
                        buf.put_slice(&bytes);

                        Poll::Ready(Ok(()))
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            })
            .map(Self::map_result)
    }
}

pub struct OutputStreamReader<IO>(IoStreamState<IO>);

impl<IO: OutputStream + Unpin> OutputStreamReader<IO> {
    pub fn new(resource: IO) -> Self {
        Self(IoStreamState::Ready(resource))
    }

    fn map_err(e: StreamError) -> io::Error {
        match e {
            StreamError::Closed => std::io::ErrorKind::BrokenPipe.into(),
            e => std::io::Error::other(e),
        }
    }
}

impl<IO: OutputStream + Unpin> AsyncWrite for OutputStreamReader<IO> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.0
            .poll_ready(cx, |stream| {
                if buf.is_empty() {
                    return Poll::Ready(Ok(0));
                }

                let permit = match stream.check_write() {
                    Ok(0) => return Poll::Pending,
                    Ok(permit) => permit,
                    Err(e) => return Poll::Ready(Err(e)),
                };

                let count = buf.len().min(permit);
                let data = Bytes::copy_from_slice(&buf[..count]);
                let result = stream.write(data).map(|_| count);
                Poll::Ready(result)
            })
            .map_err(Self::map_err)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        self.0
            .poll_ready(cx, |stream| {
                let result = stream.flush();

                // Immediately report `Ready` to prevent redundant readiness
                // checks. The next call to `poll_write` will do this for us.
                Poll::Ready(result)
            })
            .map_err(Self::map_err)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        // WASI's `output-stream` doesn't support asynchronous shutdown.
        // Best we can do here is to flush and then drop it.
        let poll = self
            .0
            .poll_ready(cx, |stream| {
                match stream.flush() {
                    Ok(()) => {}
                    Err(e) => return Poll::Ready(Err(e)),
                }

                match stream.check_write() {
                    Ok(0) => Poll::Pending,
                    r => Poll::Ready(r.map(|_| ())),
                }
            })
            .map_err(Self::map_err);

        if let Poll::Ready(_) = &poll {
            self.0 = IoStreamState::Closed;
        }

        poll
    }
}

fn noop_context() -> std::task::Context<'static> {
    std::task::Context::from_waker(futures::task::noop_waker_ref())
}

fn try_io<T, F>(f: F) -> io::Result<T>
where
    F: FnOnce(&mut std::task::Context<'_>) -> Poll<io::Result<T>>,
{
    match f(&mut noop_context()) {
        Poll::Ready(r) => r,
        Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
    }
}
