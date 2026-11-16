//! The control-I/O worker (H4 in the architecture diagram).
//!
//! One worker serves every backend: it executes word bursts and vendor
//! requests in submission order and owns the device pacing, so the
//! service thread's event drain never waits on control I/O. Backends
//! supply a [`ControlSink`]; the orchestration is shared.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::error::Error;

use super::abi::HWDEP_MAX_WORDS;
use super::device::ControlTransport;
use super::hwdep::Direction;

/// Minimum gap between IF1 register writes. Without it, a full scene
/// restore can fill the device's receive FIFO and `usb_bulk_msg` reports
/// ETIMEDOUT even though the endpoint itself is healthy.
const IF1_WRITE_GAP: Duration = Duration::from_millis(5);
/// Pause after a multi-word IF1 burst. The device drains a 63-word
/// burst slower than a single register; the reference host spaces its
/// connect bursts by roughly 16-33 ms. 5 ms per burst overflows the
/// receive FIFO (ETIMEDOUT).
const IF1_WRITE_BURST_GAP: Duration = Duration::from_millis(16);
/// Largest IF1 burst the hwdep ABI accepts. Scene writes are coalesced
/// into chunks of this size so the pacing applies once per burst rather
/// than once per register.
const IF1_WRITE_CHUNK: usize = HWDEP_MAX_WORDS;

/// The control-I/O surface a backend supplies to the worker: vendor
/// requests and IF1 word bursts. Event reads stay with the backend.
pub(crate) trait ControlSink: Send {
    fn vendor(
        &self,
        request: u8,
        direction: Direction,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> Result<Vec<u8>, Error>;
    fn write_words(&self, words: &[(u16, u16)]) -> Result<(), Error>;
}

impl ControlSink for dyn ControlTransport {
    fn vendor(
        &self,
        request: u8,
        direction: Direction,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> Result<Vec<u8>, Error> {
        ControlTransport::vendor(self, request, direction, value, index, data)
    }

    fn write_words(&self, words: &[(u16, u16)]) -> Result<(), Error> {
        ControlTransport::write_words(self, words)
    }
}

/// One operation in an ordered scene batch: a register word or a vendor
/// write. The worker executes the sequence in order, pacing only the
/// word bursts, so a scene stays one queued command.
pub(crate) enum BatchOp {
    Word(u16, u16),
    Vendor { request: u8, value: u16, index: u16 },
}

pub(crate) enum WriterCommand {
    Words(Vec<(u16, u16)>),
    Batch(Vec<BatchOp>),
    Vendor {
        request: u8,
        direction: Direction,
        value: u16,
        index: u16,
        data: Vec<u8>,
        reply: Option<Sender<Result<Vec<u8>, String>>>,
    },
    /// Sync point for tests and explicit flushes; production paths rely
    /// on the ordered queue and the shutdown drain.
    #[allow(dead_code)]
    Flush(Sender<Result<(), String>>),
}

/// The worker queue is bounded and coalescing: consecutive word batches
/// merge with the latest value per register, while vendor requests and
/// flushes act as ordering barriers. A scene interleaves vendor ops, so
/// one scene occupies many commands; the cap holds several scenes. A
/// sender that outruns the device pace blocks (backpressure) instead of
/// building an unbounded backlog.
const WORKER_QUEUE_CAP: usize = 512;

struct WorkerQueue {
    q: Mutex<VecDeque<WriterCommand>>,
    cv: Condvar,
    closed: AtomicBool,
}

impl WorkerQueue {
    fn new() -> Self {
        Self {
            q: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
            closed: AtomicBool::new(false),
        }
    }

    fn push_words(&self, words: Vec<(u16, u16)>) -> Result<(), Error> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(writer_stopped());
        }
        {
            let mut q = self.q.lock().unwrap();
            if let Some(WriterCommand::Words(prev)) = q.back_mut() {
                for (addr, value) in words {
                    if let Some(slot) = prev.iter_mut().find(|(a, _)| *a == addr) {
                        slot.1 = value;
                    } else {
                        prev.push((addr, value));
                    }
                }
                self.cv.notify_one();
                return Ok(());
            }
        }
        self.push(WriterCommand::Words(words))
    }

    fn push(&self, cmd: WriterCommand) -> Result<(), Error> {
        let mut q = self.q.lock().unwrap();
        while q.len() >= WORKER_QUEUE_CAP {
            if self.closed.load(Ordering::Relaxed) {
                return Err(writer_stopped());
            }
            q = self.cv.wait(q).unwrap();
        }
        if self.closed.load(Ordering::Relaxed) {
            return Err(writer_stopped());
        }
        q.push_back(cmd);
        self.cv.notify_one();
        Ok(())
    }

    fn recv(&self) -> Option<WriterCommand> {
        let mut q = self.q.lock().unwrap();
        while q.is_empty() {
            if self.closed.load(Ordering::Relaxed) {
                return None;
            }
            q = self.cv.wait(q).unwrap();
        }
        let cmd = q.pop_front();
        self.cv.notify_all();
        cmd
    }

    fn try_recv(&self) -> Option<WriterCommand> {
        let cmd = self.q.lock().unwrap().pop_front();
        if cmd.is_some() {
            self.cv.notify_all();
        }
        cmd
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.cv.notify_all();
    }
}

/// Control-I/O worker handle. The calling thread queues words and vendor
/// requests and returns; the worker executes them in submission order
/// and owns the burst cadence.
pub(crate) struct AsyncWriter {
    queue: Arc<WorkerQueue>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl AsyncWriter {
    pub(crate) fn new(sink: Arc<Mutex<Box<dyn ControlSink>>>) -> Option<Self> {
        let queue = Arc::new(WorkerQueue::new());
        let worker_queue = Arc::clone(&queue);
        match thread::Builder::new()
            .name("totalcontrol-control-io".into())
            .spawn(move || writer_loop(worker_queue, sink))
        {
            Ok(join) => Some(Self {
                queue,
                join: Mutex::new(Some(join)),
            }),
            Err(e) => {
                log::warn!("Starting the control-I/O worker failed: {e:?}");
                None
            }
        }
    }

    pub(crate) fn send_words(&self, words: Vec<(u16, u16)>) -> Result<(), Error> {
        self.queue.push_words(words)
    }

    pub(crate) fn send_batch(&self, ops: Vec<BatchOp>) -> Result<(), Error> {
        if ops.is_empty() {
            return Ok(());
        }
        self.queue.push(WriterCommand::Batch(ops))
    }

    pub(crate) fn send_vendor(
        &self,
        request: u8,
        direction: Direction,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> Result<(), Error> {
        self.queue.push(WriterCommand::Vendor {
            request,
            direction,
            value,
            index,
            data: data.to_vec(),
            reply: None,
        })
    }

    pub(crate) fn request_vendor(
        &self,
        request: u8,
        direction: Direction,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let (tx, rx) = mpsc::channel();
        self.queue.push(WriterCommand::Vendor {
            request,
            direction,
            value,
            index,
            data: data.to_vec(),
            reply: Some(tx),
        })?;
        match rx.recv() {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(e)) => Err(Error::Io(std::io::Error::other(e))),
            Err(_) => Err(writer_stopped()),
        }
    }

    /// Sync point for tests and explicit flushes.
    #[allow(dead_code)]
    pub(crate) fn flush(&self) -> Result<(), Error> {
        let (tx, rx) = mpsc::channel();
        self.queue.push(WriterCommand::Flush(tx))?;
        match rx.recv() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(Error::Io(std::io::Error::other(e))),
            Err(_) => Err(writer_stopped()),
        }
    }
}

impl Drop for AsyncWriter {
    fn drop(&mut self) {
        self.queue.close();
        if let Some(join) = self.join.lock().unwrap().take() {
            let _ = join.join();
        }
    }
}

fn writer_stopped() -> Error {
    Error::Io(std::io::Error::other("control-I/O worker stopped"))
}

fn writer_loop(queue: Arc<WorkerQueue>, sink: Arc<Mutex<Box<dyn ControlSink>>>) {
    let mut pending: Vec<(u16, u16)> = Vec::new();
    while let Some(cmd) = queue.recv() {
        match cmd {
            WriterCommand::Words(words) => {
                pending.extend(words);
                // Coalesce everything already queued before spending the
                // burst gap, then send it as one paced batch. A non-word
                // command ends the coalescing and executes in order.
                let mut next = None;
                while pending.len() < IF1_WRITE_CHUNK {
                    match queue.try_recv() {
                        Some(WriterCommand::Words(more)) => pending.extend(more),
                        Some(other) => {
                            next = Some(other);
                            break;
                        }
                        None => break,
                    }
                }
                let _ = flush_pending(&sink, &mut pending);
                if let Some(other) = next {
                    writer_exec(&sink, &mut pending, other);
                }
            }
            other => writer_exec(&sink, &mut pending, other),
        }
    }
    let _ = flush_pending(&sink, &mut pending);
}

/// Execute one non-word command in submission order. Words queued ahead
/// of a vendor request or flush are sent first.
fn writer_exec(
    sink: &Arc<Mutex<Box<dyn ControlSink>>>,
    pending: &mut Vec<(u16, u16)>,
    cmd: WriterCommand,
) {
    match cmd {
        WriterCommand::Words(words) => {
            pending.extend(words);
            let _ = flush_pending(sink, pending);
        }
        WriterCommand::Batch(ops) => {
            let _ = flush_pending(sink, pending);
            for op in ops {
                match op {
                    BatchOp::Word(addr, value) => pending.push((addr, value)),
                    BatchOp::Vendor {
                        request,
                        value,
                        index,
                    } => {
                        let _ = flush_pending(sink, pending);
                        if let Err(e) =
                            sink.lock()
                                .unwrap()
                                .vendor(request, Direction::Out, value, index, &[])
                        {
                            log::warn!("Batch vendor 0x{request:02x} failed: {e:?}");
                        }
                    }
                }
            }
            let _ = flush_pending(sink, pending);
        }
        WriterCommand::Vendor {
            request,
            direction,
            value,
            index,
            data,
            reply,
        } => {
            let _ = flush_pending(sink, pending);
            let result = sink
                .lock()
                .unwrap()
                .vendor(request, direction, value, index, &data)
                .map_err(|e| format!("{e:?}"));
            match reply {
                Some(reply) => {
                    let _ = reply.send(result);
                }
                None => {
                    if let Err(e) = result {
                        log::warn!("Vendor write 0x{request:02x} failed: {e}");
                    }
                }
            }
        }
        WriterCommand::Flush(reply) => {
            let result = flush_pending(sink, pending);
            let _ = reply.send(result);
        }
    }
}

fn flush_pending(
    sink: &Arc<Mutex<Box<dyn ControlSink>>>,
    pending: &mut Vec<(u16, u16)>,
) -> Result<(), String> {
    if pending.is_empty() {
        return Ok(());
    }
    let words = std::mem::take(pending);
    match paced_words(sink, &words) {
        Ok(()) => Ok(()),
        Err(e) => {
            let message = format!("{e:?}");
            log::warn!("Control-I/O worker failed: {message}");
            Err(message)
        }
    }
}

/// Execute an ordered batch without a worker (the synchronous fallback):
/// word runs are paced, vendor writes execute in their place.
pub(crate) fn exec_batch_inline<T: ControlSink + ?Sized>(
    sink: &Mutex<Box<T>>,
    ops: &[BatchOp],
) -> Result<(), Error> {
    let mut words: Vec<(u16, u16)> = Vec::new();
    for op in ops {
        match op {
            BatchOp::Word(addr, value) => words.push((*addr, *value)),
            BatchOp::Vendor {
                request,
                value,
                index,
            } => {
                paced_words(sink, &words)?;
                words.clear();
                sink.lock()
                    .unwrap()
                    .vendor(*request, Direction::Out, *value, *index, &[])?;
            }
        }
    }
    paced_words(sink, &words)
}

/// Send a word list in ABI-sized bursts, pacing each burst so the
/// device FIFO can drain.
pub(crate) fn paced_words<T: ControlSink + ?Sized>(
    sink: &Mutex<Box<T>>,
    words: &[(u16, u16)],
) -> Result<(), Error> {
    for chunk in words.chunks(IF1_WRITE_CHUNK) {
        let result = sink.lock().unwrap().write_words(chunk);
        if let Err(e) = &result {
            log::warn!(
                "IF1 burst of {} words starting 0x{:04x} failed: {e:?}",
                chunk.len(),
                chunk.first().map(|w| w.0).unwrap_or(0)
            );
            return result;
        }
        // Small bursts are the live-parameter path; a full 63-word
        // scene burst needs the device's drain time.
        let gap = if chunk.len() <= 8 {
            IF1_WRITE_GAP
        } else {
            IF1_WRITE_BURST_GAP
        };
        std::thread::sleep(gap);
    }
    Ok(())
}
