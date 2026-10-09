//! Bulk reader thread and the frame queue it feeds.

use amscope_protocol::frame::{Feed, FrameAssembler, RawFrame, SampleFormat};
use amscope_protocol::model::Resolution;
use nusb::transfer::{Buffer, Bulk, In};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const ENDPOINT: u8 = 0x81;
/// Matches the vendor SDK. Frames are several transfers long and end with a short one.
const TRANSFER_SIZE: usize = 512 * 1024;
/// Transfers kept in flight. 8 × 512 KiB covers ~40 ms at the full USB3 rate.
const IN_FLIGHT: usize = 8;
/// Frames buffered for a slow consumer before the oldest is dropped.
const QUEUE_DEPTH: usize = 3;

/// Counters for a running stream.
#[derive(Debug, Clone, Default)]
pub struct StreamStats {
    pub frames: u64,
    /// Frames dropped because the consumer did not keep up.
    pub dropped: u64,
    /// Transfers or partial frames discarded while (re)synchronising.
    pub discarded: u64,
    pub bytes: u64,
    pub transfer_errors: u64,
    /// Frame rate over the last second.
    pub fps: f64,
    pub last_sequence: Option<u32>,
    /// Frames missing according to the trailer sequence counter.
    pub sequence_gaps: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum RecvError {
    #[error("stream stopped")]
    Stopped,
    #[error("timed out waiting for a frame")]
    Timeout,
}

struct Shared {
    stop: AtomicBool,
    geometry: Mutex<Option<(Resolution, SampleFormat, bool)>>,
    stats: Mutex<StreamStats>,
    queue: Mutex<(VecDeque<RawFrame>, bool)>,
    ready: Condvar,
}

impl Shared {
    fn push(&self, frame: RawFrame) {
        let mut q = self.queue.lock().unwrap();
        if q.0.len() >= QUEUE_DEPTH {
            q.0.pop_front();
            self.stats.lock().unwrap().dropped += 1;
        }
        q.0.push_back(frame);
        drop(q);
        self.ready.notify_one();
    }

    fn close(&self) {
        self.queue.lock().unwrap().1 = true;
        self.ready.notify_all();
    }
}

/// Receives frames from a running stream. Dropping it does not stop the camera.
pub struct FrameReceiver {
    shared: Arc<Shared>,
}

impl FrameReceiver {
    /// Block until the next frame arrives.
    pub fn recv(&self) -> Result<RawFrame, RecvError> {
        self.recv_inner(None)
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<RawFrame, RecvError> {
        self.recv_inner(Some(Instant::now() + timeout))
    }

    /// Return a frame if one is ready.
    pub fn try_recv(&self) -> Option<RawFrame> {
        self.shared.queue.lock().unwrap().0.pop_front()
    }

    /// Wait for a frame, discard any older queued ones, and return the newest.
    pub fn recv_latest(&self, timeout: Duration) -> Result<RawFrame, RecvError> {
        let mut f = self.recv_timeout(timeout)?;
        while let Some(newer) = self.try_recv() {
            f = newer;
        }
        Ok(f)
    }

    pub fn stats(&self) -> StreamStats {
        self.shared.stats.lock().unwrap().clone()
    }

    fn recv_inner(&self, deadline: Option<Instant>) -> Result<RawFrame, RecvError> {
        let mut q = self.shared.queue.lock().unwrap();
        loop {
            if let Some(f) = q.0.pop_front() {
                return Ok(f);
            }
            if q.1 {
                return Err(RecvError::Stopped);
            }
            q = match deadline {
                None => self.shared.ready.wait(q).unwrap(),
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        return Err(RecvError::Timeout);
                    }
                    self.shared.ready.wait_timeout(q, d - now).unwrap().0
                }
            };
        }
    }
}

pub(crate) struct Reader {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Reader {
    pub fn spawn(
        interface: &nusb::Interface,
        res: Resolution,
        format: SampleFormat,
    ) -> Result<(Reader, FrameReceiver), nusb::Error> {
        let mut ep = interface.endpoint::<Bulk, In>(ENDPOINT)?;
        for _ in 0..IN_FLIGHT {
            let buf = ep.allocate(TRANSFER_SIZE);
            ep.submit(buf);
        }
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            geometry: Mutex::new(Some((res, format, false))),
            stats: Mutex::new(StreamStats::default()),
            queue: Mutex::new((VecDeque::new(), false)),
            ready: Condvar::new(),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("amscope-bulk".into())
            .spawn(move || run(ep, s))
            .expect("spawn bulk reader thread");
        Ok((Reader { shared: shared.clone(), thread: Some(thread) }, FrameReceiver { shared }))
    }

    /// Tell the reader the frame size changed (resolution, ROI or format), and whether
    /// the last row must be patched.
    pub fn set_geometry(&self, res: Resolution, format: SampleFormat, patch_last_row: bool) {
        *self.shared.geometry.lock().unwrap() = Some((res, format, patch_last_row));
    }

    pub fn stats(&self) -> StreamStats {
        self.shared.stats.lock().unwrap().clone()
    }

    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.shared.close();
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(mut ep: nusb::Endpoint<Bulk, In>, shared: Arc<Shared>) {
    let mut asm: Option<FrameAssembler> = None;
    let mut patch_last_row = false;
    let mut window = (Instant::now(), 0u32);
    while !shared.stop.load(Ordering::Acquire) {
        if let Some((res, fmt, patch)) = shared.geometry.lock().unwrap().take() {
            asm = Some(FrameAssembler::new(res.width, res.height, fmt));
            patch_last_row = patch;
        }
        let Some(c) = ep.wait_next_complete(Duration::from_millis(100)) else {
            continue;
        };
        let requested = c.buffer.requested_len();
        match c.status {
            Ok(()) => {
                let feed = asm.as_mut().map(|a| a.feed(&c.buffer[..c.actual_len], requested));
                let mut st = shared.stats.lock().unwrap();
                st.bytes += c.actual_len as u64;
                match feed {
                    Some(Feed::Frame) => {
                        let mut frame = asm.as_mut().unwrap().take().unwrap();
                        if patch_last_row {
                            frame.patch_row_from_two_above(frame.height - 1);
                        }
                        st.frames += 1;
                        let seq = frame.trailer.sequence;
                        if let Some(prev) = st.last_sequence {
                            st.sequence_gaps += seq.wrapping_sub(prev).saturating_sub(1) as u64;
                        }
                        st.last_sequence = Some(seq);
                        window.1 += 1;
                        let dt = window.0.elapsed();
                        if dt >= Duration::from_secs(1) {
                            st.fps = window.1 as f64 / dt.as_secs_f64();
                            window = (Instant::now(), 0);
                        }
                        drop(st);
                        shared.push(frame);
                    }
                    Some(Feed::Discarded { bytes }) => {
                        st.discarded += 1;
                        tracing::debug!(bytes, "discarded partial frame");
                    }
                    Some(Feed::Pending) | None => {}
                }
            }
            Err(nusb::transfer::TransferError::Cancelled) => {}
            Err(nusb::transfer::TransferError::Disconnected) => {
                tracing::warn!("camera disconnected");
                break;
            }
            Err(e) => {
                shared.stats.lock().unwrap().transfer_errors += 1;
                tracing::warn!("bulk transfer error: {e}");
            }
        }
        // Resubmit the same buffer.
        let mut buf: Buffer = c.buffer;
        buf.set_requested_len(TRANSFER_SIZE);
        ep.submit(buf);
    }
    ep.cancel_all();
    while ep.pending() > 0 {
        if ep.wait_next_complete(Duration::from_secs(1)).is_none() {
            tracing::warn!("bulk transfers did not cancel in time");
            break;
        }
    }
    shared.close();
}
