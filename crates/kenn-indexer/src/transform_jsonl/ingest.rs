//! JSONL ingest driving layer: error/stats types, the two public ingest
//! entry points, and the file-backed read loop that polls a growing file
//! against the still-running producer.

use std::io::BufRead;
use std::path::Path;
use std::process::Child;

use kenn_store::api::DbError;

use crate::canonicalize::Workspace;
use crate::parse_jsonl::{parse_jsonl_stream, Frame, ParseJsonlError};
use crate::sink::BatchSink;
use crate::transform::IdRegistry;
use crate::BENCH_ENABLED;

use super::{handle_frame, StreamState};

#[derive(Debug, thiserror::Error)]
pub enum JsonlTransformError {
    #[error("parse: {0}")]
    Parse(#[from] ParseJsonlError),
    #[error("sink: {0}")]
    Sink(#[from] DbError),
    #[error("unsupported language `{0}`")]
    UnknownLanguage(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Upper bound on per-stream attributions kept in
/// [`JsonlIngestStats::failed`] / [`JsonlIngestStats::warned`]. A
/// pathological producer (per-file msbuild errors on a large solution)
/// must not balloon the persisted report; overflow is rendered as one
/// `+N more` suffix downstream.
pub(crate) const JSONL_FAILED_ATTRIBUTION_CAP: usize = 32;

#[derive(Debug, Default, Clone)]
pub struct JsonlIngestStats {
    pub files: u64,
    pub packages: u64,
    pub symbols: u64,
    pub defs: u64,
    pub edges: u64,
    /// Every `ErrorFrame` observed, any severity.
    pub errors: u64,
    /// `ErrorFrame{severity: "error"}` frames only.
    pub failed_errors: u64,
    /// Formatted attributions for the first
    /// [`JSONL_FAILED_ATTRIBUTION_CAP`] `severity: "error"` frames,
    /// destined for the unit report's `failed_projects`.
    pub failed: Vec<String>,
    /// `ErrorFrame{severity: "warning"}` frames only.
    pub warning_total: u64,
    /// Formatted attributions for the first
    /// [`JSONL_FAILED_ATTRIBUTION_CAP`] warning frames, destined for the
    /// unit report's `warnings` — dropping them silenced diagnostics the
    /// producers promise (e.g. the Swift stale-unit notices).
    pub warned: Vec<String>,
    /// The producer's self-reported version, from the `meta` frame. Fills the
    /// unit report's `indexer_version`, which would otherwise stay `"?"`.
    pub tool_version: Option<String>,
    /// Toolchains the entrypoint provisioned, from `toolchain` frames. One per
    /// provisioned toolchain — an image may provision more than one (python +
    /// node). Fills the unit report's `toolchains`.
    pub toolchains: Vec<crate::report::ToolchainVersion>,
}

/// Read the JSONL stream from `reader` and push records into `sink`.
pub fn ingest_jsonl_into_sink<R: BufRead>(
    reader: &mut R,
    workspace: &Workspace,
    registry: &mut IdRegistry,
    sink: &mut BatchSink,
) -> Result<JsonlIngestStats, JsonlTransformError> {
    let mut state = StreamState::new(workspace);
    let mut counts = JsonlIngestStats::default();

    parse_jsonl_stream(reader, |frame| -> Result<(), JsonlTransformError> {
        handle_frame(frame, &mut state, registry, sink, &mut counts)
    })?;

    Ok(counts)
}

/// Ingest a JSONL stream from a file the producer is concurrently writing.
///
/// Returns `(stats, end_frame_seen)`. The producer's stdout was redirected
/// to `path` at spawn — every write goes through the page cache and never
/// blocks. We read with a `BufReader` and poll on transient EOF: if
/// `read_line` returns 0, we ask the kernel whether the producer is still
/// alive (`child.try_wait`) and either retry after a short sleep or stop.
///
/// File-backed handoff replaced the old threaded-pipe ingest because the
/// OS pipe (16-64 KiB on macOS) backed up walker threads when the
/// `SurrealDB` sink was mid-flush — pipe-full → `_stdout.Write` blocks →
/// `JsonlSink._sync` queue grows → walker threads stall. The file path
/// has no such limit; producer always wins.
pub fn ingest_jsonl_from_growing_file(
    path: &Path,
    child: &mut Child,
    workspace: &Workspace,
    registry: &mut IdRegistry,
    sink: &mut BatchSink,
) -> Result<(JsonlIngestStats, bool), JsonlTransformError> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut state = StreamState::new(workspace);
    let mut counts = JsonlIngestStats::default();
    let mut transient_eof_count: u64 = 0;
    let end_frame_seen = drive_jsonl_loop(
        &mut reader,
        child,
        &mut state,
        registry,
        sink,
        &mut counts,
        &mut transient_eof_count,
    )?;
    log_jsonl_bench(transient_eof_count);
    Ok((counts, end_frame_seen))
}

/// The JSONL ingest loop body. Returns true iff an `End` frame was
/// observed before the producer's stream closed.
fn drive_jsonl_loop(
    reader: &mut std::io::BufReader<std::fs::File>,
    child: &mut Child,
    state: &mut StreamState,
    registry: &mut IdRegistry,
    sink: &mut BatchSink,
    counts: &mut JsonlIngestStats,
    transient_eof_count: &mut u64,
) -> Result<bool, JsonlTransformError> {
    let mut line = String::new();
    let mut line_no: u64 = 0;
    loop {
        match next_jsonl_line(reader, child, &mut line, transient_eof_count)? {
            JsonlReadStep::Done => return Ok(false),
            JsonlReadStep::Retry => continue,
            JsonlReadStep::Got => {}
        }
        line_no += 1;
        let is_end = process_jsonl_line(&line, line_no, state, registry, sink, counts)?;
        // Clear only once the record has been consumed — `next_jsonl_line` relies
        // on `line` surviving a Retry so a partial record can accumulate.
        line.clear();
        if is_end {
            return Ok(true);
        }
    }
}

/// Trim + parse + dispatch one JSONL line. Returns `Ok(true)` when the
/// frame was `End` (the caller stops the loop). Empty/whitespace-only
/// lines return `Ok(false)` without touching state.
fn process_jsonl_line(
    line: &str,
    line_no: u64,
    state: &mut StreamState,
    registry: &mut IdRegistry,
    sink: &mut BatchSink,
    counts: &mut JsonlIngestStats,
) -> Result<bool, JsonlTransformError> {
    let trimmed = line.trim_end_matches(&['\n', '\r'][..]);
    if trimmed.is_empty() {
        return Ok(false);
    }
    let frame = parse_jsonl_frame(trimmed, line_no)?;
    let is_end = matches!(frame, Frame::End(_));
    handle_frame(frame, state, registry, sink, counts)?;
    Ok(is_end)
}

enum JsonlReadStep {
    /// Producer exited and the stream is fully drained — stop the loop.
    Done,
    /// Transient EOF — caller continues the loop after the inner sleep.
    Retry,
    /// `line` was populated; caller advances to the parsing path.
    Got,
}

/// One iteration of the JSONL read loop. Handles transient EOF polling
/// against the still-running child and the post-exit drain race.
fn next_jsonl_line(
    reader: &mut std::io::BufReader<std::fs::File>,
    child: &mut Child,
    line: &mut String,
    transient_eof_count: &mut u64,
) -> Result<JsonlReadStep, JsonlTransformError> {
    // Tuning knob: 5 ms × 200 polls/sec keeps wakeup overhead well under
    // 1% CPU even during sustained back-pressure, and the latency is far
    // below user perception thresholds (50s pipeline).
    let poll_interval = std::time::Duration::from_millis(5);
    // `line` is deliberately NOT cleared here: it may already hold a PARTIAL
    // record from a previous call. `read_line` appends, so the remainder lands
    // directly after it. The caller clears once a record has been consumed.
    reader.read_line(line)?;
    // A record is only complete when it is newline-terminated. `read_line` also
    // returns at EOF, so a non-empty result without '\n' means we caught the
    // producer MID-RECORD — writers flush on buffer boundaries (a pipe flushes
    // at 64KiB), which lands mid-record on any large stream. Handing that
    // fragment to the parser is what produced "EOF while parsing a string" and
    // failed the whole ingest on big workspaces.
    if line.ends_with('\n') {
        return Ok(JsonlReadStep::Got);
    }
    // Nothing new, or only a fragment. Either way the producer owes us bytes.
    // `try_wait` is a non-blocking `waitpid(WNOHANG)`; cheap to call frequently.
    if child.try_wait()?.is_none() {
        *transient_eof_count += 1;
        std::thread::sleep(poll_interval);
        return Ok(JsonlReadStep::Retry);
    }
    // Producer exited. Drain any final bytes that landed between the read above
    // and try_wait() — a short race window but not zero.
    reader.read_line(line)?;
    if line.ends_with('\n') {
        return Ok(JsonlReadStep::Got);
    }
    if line.is_empty() {
        Ok(JsonlReadStep::Done)
    } else {
        // Producer exited mid-record: surface the fragment so the caller reports
        // a truncated stream, rather than silently dropping the tail.
        Ok(JsonlReadStep::Got)
    }
}

fn parse_jsonl_frame(trimmed: &str, line_no: u64) -> Result<Frame, JsonlTransformError> {
    serde_json::from_str(trimmed).map_err(|source| {
        JsonlTransformError::Parse(ParseJsonlError::Json {
            line: line_no,
            source,
        })
    })
}

fn log_jsonl_bench(transient_eof_count: u64) {
    if *BENCH_ENABLED {
        eprintln!(
            "BENCH ingest: transient_eof_polls={transient_eof_count} (5ms each = {}ms wait)",
            transient_eof_count * 5
        );
    }
}

/// `meta` / `end` are run-bracket frames carrying producer-side ISO
/// timestamps (`MetaFrame.ts` / `EndFrame.ts`). Under `KENN_BENCH=1` we
/// print both producer's stamp and the consumer-side wall time at frame-
/// receipt, plus the lag in ms. Lag on `meta` is producer-startup-to-
/// first-bytes-readable; lag on `end` is producer-write-to-consumer-read,
/// which under file-backed handoff is the time the consumer needed to
/// catch up after the producer's final flush.
pub(crate) fn emit_ts_bench(label: &str, producer_ts: &str) {
    use time::format_description::BorrowedFormatItem;
    use time::macros::format_description;
    use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};

    // The trailing `Z` in the format is a literal — `time`'s
    // `OffsetDateTime::parse` won't recognise it as a UTC offset, so we
    // parse via `PrimitiveDateTime` and pin offset to UTC ourselves.
    const ISO_FMT: &[BorrowedFormatItem<'_>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

    if !*BENCH_ENABLED {
        return;
    }
    let now = OffsetDateTime::now_utc();
    let consumer_ts = now.format(&ISO_FMT).unwrap_or_default();
    let lag = match PrimitiveDateTime::parse(producer_ts, &ISO_FMT) {
        Ok(prim) => {
            let prod = prim.assume_offset(UtcOffset::UTC);
            format!("{}", (now - prod).whole_milliseconds())
        }
        Err(_) => "?".into(),
    };
    eprintln!(
        "BENCH ingest: {label} producer_ts={producer_ts} consumer_ts={consumer_ts} lag_ms={lag}"
    );
}
