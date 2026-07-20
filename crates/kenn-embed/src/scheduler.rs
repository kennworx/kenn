//! `PriorityEmbedScheduler` — the shared query-priority embedding worker.
//!
//! Inference is serialized (one encode in flight). A large bulk embed must
//! not monopolize the model and starve an interactive query. This scheduler
//! runs a **single dedicated OS thread** that owns one resident
//! [`BatchEncoder`] (the model/context, reused across encodes) and drains two
//! priority queues: it serves all ready **high** (query) jobs before advancing
//! a **low** (bulk) job by **one batch** (`encoder.batch_size()` =
//! `SEQS_PER_BATCH`). So a query waits at most one in-flight batch — the
//! model's own atomic unit (see the `embed-query-priority` design).
//!
//! The thread holds the context (not `Send`-friendly across thread migrations,
//! e.g. llama.cpp's Metal backend); inside it runs a `current_thread` tokio
//! runtime that drives the worker loop via `tokio::select!` over async mpsc
//! channels. Same thread affinity as the old `Mutex` + `Condvar` design,
//! cleaner queue semantics (closes propagate, idle uses `tokio::time::sleep`).
//! The model is lazy-loaded on the first job and released after `idle_ttl` of
//! inactivity; the thread persists and reloads on the next job. Both the
//! in-process producer and the `kenn server` daemon construct one of these
//! over their own encoder.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::{EmbedError, EmbedKind};

/// Scheduling class. A query is `High` (latency-critical, one-shot); a bulk
/// corpus embed is `Low`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    High,
    Low,
}

/// A stateful batch encoder: holds a resident model/context and encodes up to
/// [`batch_size`](BatchEncoder::batch_size) texts per call, **reusing** that
/// context. Created on the scheduler's dedicated thread and never moved off it,
/// so it need not be `Send` once built (only the [`EncoderLoader`] is).
///
/// `encode_batch` is `async fn` because the worker runs an async loop (its
/// own current-thread tokio runtime on the dedicated thread). CPU-bound
/// implementations (e.g. `LlamaBatchEncoder` over `LlamaEmbedder`) call sync
/// work inside — that blocks the scheduler thread's runtime, which is exactly
/// what we want: the thread is dedicated to encoding, with priority-queue
/// checks happening only between batches.
#[async_trait::async_trait(?Send)]
pub trait BatchEncoder {
    /// Encode at most [`batch_size`](Self::batch_size) texts in one model
    /// encode, one vector per input in input order. `kind` is the
    /// submitting job's [`EmbedKind`] — encoders whose model wants a task
    /// prompt for that kind apply it (see `embedding-gemma-prompts`).
    async fn encode_batch(
        &mut self,
        texts: &[String],
        kind: EmbedKind,
    ) -> Result<Vec<Vec<f32>>, EmbedError>;
    /// The model's internal batch size (`SEQS_PER_BATCH`) — the largest unit
    /// the worker will hand to a single [`encode_batch`](Self::encode_batch).
    fn batch_size(&self) -> usize;
}

/// Builds a [`BatchEncoder`] on demand (lazy model load). `None` means no model
/// is available — the scheduler then fails pending jobs with a backend error so
/// callers degrade rather than hang.
pub type EncoderLoader = Arc<dyn Fn() -> Option<Box<dyn BatchEncoder>> + Send + Sync>;

/// One submitted embed request: its texts, what they are (query vs
/// document — drives prompt selection in the encoder), and a one-shot
/// reply channel.
struct Job {
    texts: Vec<String>,
    kind: EmbedKind,
    reply: oneshot::Sender<Result<Vec<Vec<f32>>, EmbedError>>,
}

/// Out-of-band signals from the scheduler handle to the worker. Today only
/// `Release` — `Drop` closes the channels for shutdown.
enum Control {
    /// Drop the resident model on the next loop iteration without tearing
    /// down the worker. Sent by [`PriorityEmbedScheduler::release_blocking`].
    Release,
}

/// Query-priority embedding scheduler. Cheap to construct — spawns the worker
/// thread but loads no model until the first [`submit`](Self::submit).
#[expect(
    clippy::struct_field_names,
    reason = "`_tx` suffix distinguishes the three send halves from one another and from the rx halves owned by the worker"
)]
pub struct PriorityEmbedScheduler {
    high_tx: mpsc::UnboundedSender<Job>,
    low_tx: mpsc::UnboundedSender<Job>,
    control_tx: mpsc::UnboundedSender<Control>,
}

impl PriorityEmbedScheduler {
    /// Spawn the worker thread over `loader`, releasing the resident model
    /// after `idle_ttl` of inactivity (the thread itself persists).
    #[must_use]
    pub fn new(loader: EncoderLoader, idle_ttl: Duration) -> Self {
        let (high_tx, high_rx) = mpsc::unbounded_channel();
        let (low_tx, low_rx) = mpsc::unbounded_channel();
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        // A dedicated OS thread (not a tokio task on the host runtime): it
        // owns the non-`Send` context across encodes. The thread spins up
        // its own current-thread tokio runtime so the worker can use async
        // mpsc + `select!` while staying pinned to one OS thread.
        std::thread::Builder::new()
            .name("kenn-embed-scheduler".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build scheduler current-thread runtime");
                rt.block_on(worker_loop(high_rx, low_rx, control_rx, loader, idle_ttl));
            })
            .expect("spawn embedding scheduler thread");
        Self {
            high_tx,
            low_tx,
            control_tx,
        }
    }

    /// Submit a batch at `pri` and await its vectors (one per input, in input
    /// order). `kind` says what the texts are — query or document — which is
    /// orthogonal to `pri` (a bulk pass of queries would still be `Low`).
    /// An empty input returns immediately. Errors if no model is available
    /// or the worker thread is gone.
    pub async fn submit(
        &self,
        texts: Vec<String>,
        pri: Priority,
        kind: EmbedKind,
    ) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let (reply, rx) = oneshot::channel();
        let job = Job { texts, kind, reply };
        let send_result = match pri {
            Priority::High => self.high_tx.send(job),
            Priority::Low => self.low_tx.send(job),
        };
        send_result
            .map_err(|e| EmbedError::Backend(format!("embedding scheduler worker is gone: {e}")))?;
        rx.await
            .map_err(|e| EmbedError::Backend(format!("embedding scheduler stopped: {e}")))?
    }

    /// Signal the worker to drop its resident model on its next iteration —
    /// **without tearing down the worker**. Subsequent submits transparently
    /// re-load the model via the encoder loader. Best-effort, like
    /// [`crate::LazyEmbedder::release_blocking`]: used by
    /// [`crate::release_shared_embedder`] (between tests, at exit, etc.).
    pub fn release_blocking(&self) {
        // Send failure means the worker thread has already exited (`Drop`
        // closed every channel). Ignore — caller's intent is satisfied.
        drop(self.control_tx.send(Control::Release));
    }
}

// No explicit `Drop` impl needed: dropping `PriorityEmbedScheduler` drops the
// three senders by value, which closes the channels. The worker observes the
// close via `select!` (any branch yielding `None` on `recv` is shutdown) and
// exits.

/// The worker loop, driven by the scheduler thread's current-thread runtime.
/// Owns the resident encoder and the in-flight low job; uses `select!` to
/// preempt low batches when a high job arrives.
async fn worker_loop(
    mut high_rx: mpsc::UnboundedReceiver<Job>,
    mut low_rx: mpsc::UnboundedReceiver<Job>,
    mut control_rx: mpsc::UnboundedReceiver<Control>,
    loader: EncoderLoader,
    idle_ttl: Duration,
) {
    let mut encoder: Option<Box<dyn BatchEncoder>> = None;
    let mut current_low: Option<(Job, usize, Vec<Vec<f32>>)> = None;

    loop {
        // 1. Drain control signals (non-blocking). Release first; a closed
        //    control channel means scheduler dropped → exit.
        match control_rx.try_recv() {
            Ok(Control::Release) => {
                encoder = None;
                continue;
            }
            Err(mpsc::error::TryRecvError::Disconnected) => return,
            Err(mpsc::error::TryRecvError::Empty) => {}
        }

        // 2. Peek the high queue (non-blocking). A high job preempts any
        //    in-progress low job for one batch — the low job's accumulated
        //    state is preserved.
        match high_rx.try_recv() {
            Ok(job) => {
                run_job_to_completion(job, &mut encoder, &loader).await;
                continue;
            }
            Err(mpsc::error::TryRecvError::Disconnected) => return,
            Err(mpsc::error::TryRecvError::Empty) => {}
        }

        // 3. Advance the in-progress low job by exactly one batch, then loop
        //    back to re-check high.
        if current_low.is_some() {
            let done = {
                let (job, idx, acc) = current_low.as_mut().expect("present");
                advance_low_batch(job, idx, acc, &mut encoder, &loader).await
            };
            if done {
                let (job, _, acc) = current_low.take().expect("present");
                drop(job.reply.send(Ok(acc)));
            }
            continue;
        }

        // 4. Nothing in progress: wait for any signal or idle timeout.
        //    `biased` favours control / high over low / idle within one tick.
        tokio::select! {
            biased;
            ctl = control_rx.recv() => match ctl {
                Some(Control::Release) => encoder = None,
                None => return,
            },
            job = high_rx.recv() => match job {
                Some(j) => run_job_to_completion(j, &mut encoder, &loader).await,
                None => return,
            },
            job = low_rx.recv() => match job {
                Some(j) => current_low = Some((j, 0, Vec::new())),
                None => return,
            },
            () = tokio::time::sleep(idle_ttl) => encoder = None,
        }
    }
}

/// Run a whole job (all its texts, batched) and send the reply. Used for high-
/// priority jobs, which are not preempted (they are small).
async fn run_job_to_completion(
    job: Job,
    encoder: &mut Option<Box<dyn BatchEncoder>>,
    loader: &EncoderLoader,
) {
    let mut idx = 0;
    let mut acc = Vec::with_capacity(job.texts.len());
    loop {
        if idx >= job.texts.len() {
            drop(job.reply.send(Ok(acc)));
            return;
        }
        match encode_one_batch(&job.texts, job.kind, idx, encoder, loader).await {
            Ok((vectors, next)) => {
                acc.extend(vectors);
                idx = next;
            }
            Err(e) => {
                drop(job.reply.send(Err(e)));
                return;
            }
        }
    }
}

/// Advance a low job by one batch. Returns `true` when the job is complete (the
/// caller then takes it and replies). On encode error, replies `Err` and
/// returns `true` (done).
async fn advance_low_batch(
    job: &mut Job,
    idx: &mut usize,
    acc: &mut Vec<Vec<f32>>,
    encoder: &mut Option<Box<dyn BatchEncoder>>,
    loader: &EncoderLoader,
) -> bool {
    if *idx >= job.texts.len() {
        return true;
    }
    match encode_one_batch(&job.texts, job.kind, *idx, encoder, loader).await {
        Ok((vectors, next)) => {
            acc.extend(vectors);
            *idx = next;
            *idx >= job.texts.len()
        }
        Err(e) => {
            // Replace the reply channel to send the error, marking done.
            let (dead, _) = oneshot::channel();
            let reply = std::mem::replace(&mut job.reply, dead);
            drop(reply.send(Err(e)));
            true
        }
    }
}

/// Ensure the encoder is loaded, then encode the batch starting at `idx`
/// (`encoder.batch_size()` texts). Returns the vectors and the next index.
async fn encode_one_batch(
    texts: &[String],
    kind: EmbedKind,
    idx: usize,
    encoder: &mut Option<Box<dyn BatchEncoder>>,
    loader: &EncoderLoader,
) -> Result<(Vec<Vec<f32>>, usize), EmbedError> {
    if encoder.is_none() {
        *encoder = loader();
    }
    let enc = encoder
        .as_mut()
        .ok_or_else(|| EmbedError::Backend("no embedder available".into()))?;
    let bs = enc.batch_size().max(1);
    let end = (idx + bs).min(texts.len());
    let batch = texts
        .get(idx..end)
        .ok_or_else(|| EmbedError::Backend("batch slice out of range".into()))?;
    let vectors = enc.encode_batch(batch, kind).await?;
    Ok((vectors, end))
}

#[cfg(test)]
#[expect(
    clippy::cast_precision_loss,
    reason = "test encoders use `len() as f32` for deterministic fake vectors"
)]
mod tests {
    use super::{
        BatchEncoder, EmbedError, EmbedKind, EncoderLoader, Priority, PriorityEmbedScheduler,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A fake encoder: one fixed vector per text; batch size 16; records how
    /// many encode calls (batches) it ran and how many times it was built.
    struct FakeEncoder {
        batches: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait(?Send)]
    impl BatchEncoder for FakeEncoder {
        async fn encode_batch(
            &mut self,
            texts: &[String],
            _kind: EmbedKind,
        ) -> Result<Vec<Vec<f32>>, EmbedError> {
            assert!(texts.len() <= 16, "batch exceeds SEQS_PER_BATCH");
            self.batches.fetch_add(1, Ordering::SeqCst);
            Ok(texts.iter().map(|t| vec![t.len() as f32]).collect())
        }
        fn batch_size(&self) -> usize {
            16
        }
    }

    fn loader(builds: Arc<AtomicUsize>, batches: Arc<AtomicUsize>) -> EncoderLoader {
        Arc::new(move || {
            builds.fetch_add(1, Ordering::SeqCst);
            Some(Box::new(FakeEncoder {
                batches: Arc::clone(&batches),
            }) as Box<dyn BatchEncoder>)
        })
    }

    #[tokio::test]
    async fn returns_one_vector_per_input_in_order() {
        let sched = PriorityEmbedScheduler::new(
            loader(Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))),
            Duration::from_secs(60),
        );
        let texts: Vec<String> = (0..40).map(|i| "x".repeat(i)).collect();
        let out = sched
            .submit(texts.clone(), Priority::Low, EmbedKind::Document)
            .await
            .expect("ok");
        assert_eq!(out.len(), 40);
        for (i, v) in out.iter().enumerate() {
            assert_eq!(
                v,
                &vec![i as f32],
                "vector {i} matches input length, in order"
            );
        }
    }

    #[tokio::test]
    async fn large_low_request_is_split_into_model_batches() {
        let batches = Arc::new(AtomicUsize::new(0));
        let sched = PriorityEmbedScheduler::new(
            loader(Arc::new(AtomicUsize::new(0)), Arc::clone(&batches)),
            Duration::from_secs(60),
        );
        let texts: Vec<String> = (0..100).map(|_| "t".into()).collect();
        let out = sched
            .submit(texts, Priority::Low, EmbedKind::Document)
            .await
            .expect("ok");
        assert_eq!(out.len(), 100);
        // 100 texts / batch_size 16 = 7 batches.
        assert_eq!(batches.load(Ordering::SeqCst), 7);
    }

    #[tokio::test]
    async fn high_priority_interleaves_with_a_large_low_job() {
        // A slow encoder so the low job is still in flight when the high job
        // arrives; assert the high job completes well before the low one.
        struct SlowEncoder;
        #[async_trait::async_trait(?Send)]
        impl BatchEncoder for SlowEncoder {
            async fn encode_batch(
                &mut self,
                texts: &[String],
                _kind: EmbedKind,
            ) -> Result<Vec<Vec<f32>>, EmbedError> {
                std::thread::sleep(Duration::from_millis(10));
                Ok(texts.iter().map(|_| vec![0.0_f32]).collect())
            }
            fn batch_size(&self) -> usize {
                16
            }
        }
        let sched = Arc::new(PriorityEmbedScheduler::new(
            Arc::new(|| Some(Box::new(SlowEncoder) as Box<dyn BatchEncoder>)),
            Duration::from_secs(60),
        ));
        // Big low job: 16 batches * 10ms = ~160ms.
        let big: Vec<String> = (0..256).map(|_| "t".into()).collect();
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

        let s1 = Arc::clone(&sched);
        let o1 = Arc::clone(&order);
        let low = tokio::spawn(async move {
            s1.submit(big, Priority::Low, EmbedKind::Document)
                .await
                .expect("low ok");
            o1.lock().unwrap().push("low");
        });
        // Let the low job get underway, then submit a high (query) job.
        tokio::time::sleep(Duration::from_millis(25)).await;
        let s2 = Arc::clone(&sched);
        let o2 = Arc::clone(&order);
        let high = tokio::spawn(async move {
            s2.submit(vec!["q".into()], Priority::High, EmbedKind::Query)
                .await
                .expect("high ok");
            o2.lock().unwrap().push("high");
        });
        high.await.unwrap();
        low.await.unwrap();
        assert_eq!(
            order.lock().unwrap().first().copied(),
            Some("high"),
            "the high-priority query finished before the large low job"
        );
    }
}
