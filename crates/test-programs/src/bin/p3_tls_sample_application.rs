// Run with:
// cargo test --package wasmtime-wasi-tls --no-default-features --features p3,rustls sample

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use test_programs::p3::wit_stream;
use wit_bindgen::{StreamWriter, block_on};

struct Component;

test_programs::p3::export!(Component);

impl test_programs::p3::exports::wasi::cli::run::Guest for Component {
    async fn run() -> Result<(), ()> {
        // Bug: polling a WaitableOperation under a noop Context while a
        // block_on sub-task is NOT the current task leaves a stale
        // CompletionStatus pointer in the caller's task waitable map.
        //
        // Sequence:
        //
        //   1. Poll `w1` with a noop Context while the async export task is
        //      current.  stream.write blocks → register_waker stores
        //      &w1.completion_status in export_task.waitables[H]
        //      (H = the StreamWriter's handle).
        //
        //   2. Complete the same write inside block_on.  block_on creates a
        //      fresh FutureState (sub_task).  Polling w1 there calls
        //      register_waker again → stores &w1.completion_status in
        //      sub_task.waitables[H] too.  When the write finishes,
        //      deliver_waitable_event removes the entry from sub_task only.
        //      export_task.waitables[H] still points to w1.completion_status,
        //      which is freed when w1 is dropped at the end of block_on.
        //
        //   3. Poll `w2` (a second write on the same StreamWriter) with a
        //      noop Context.  register_waker calls waitable_register with
        //      &w2.completion_status and gets back the freed
        //      &w1.completion_status as `prev`.  The assert in waitable.rs:201
        //      fires:
        //        assert_eq!(ptr, prev.cast())  →  left != right  →  panic
        //
        // Note: w2's async block is padded to 128 bytes larger than w1's so
        // the WASM allocator does not reuse w1's freed slot for w2.  If both
        // futures are the same size, &w2.completion_status == &w1.completion_status
        // (freed), the assert trivially passes, and the bug is hidden.
        let (writer, reader) = wit_stream::new::<u8>();
        let noop_cx = &mut Context::from_waker(Waker::noop());

        let mut w1: Pin<Box<dyn Future<Output = StreamWriter<u8>>>> =
            Box::pin(async {
                let mut w = writer;
                let _ = w.write(vec![1u8]).await;
                w
            });

        // Step 1 — register &w1.completion_status in export_task.waitables[H].
        assert!(matches!(w1.as_mut().poll(noop_cx), Poll::Pending));

        // Step 2 — block_on completes w1; export_task.waitables[H] goes stale.
        // _reader must stay alive so the step-3 write blocks (Dropped skips
        // register_waker and hides the bug).
        let (writer, _reader) = block_on(async move {
            let mut reader = reader;
            let (w, _) = futures::join!(w1, reader.read(Vec::with_capacity(1)));
            (w, reader)
        });

        // Step 3 — register &w2.completion_status; gets freed &w1.completion_status
        // back as prev → assert_eq!(ptr, prev.cast()) panics at waitable.rs:201.
        let mut w2: Pin<Box<dyn Future<Output = ()>>> =
            Box::pin(async move {
                let mut w = writer;
                let pad = [0u64; 16];
                let _ = w.write(vec![2u8]).await;
                let _ = pad; // explicit use after await keeps pad in the state machine
            });
        let _ = w2.as_mut().poll(noop_cx); // panics

        Ok(())
    }
}

fn main() {}
