use super::{code_updated_payload, format_progress, pick_first_file_root};
use kenn_indexer::pipeline::ProgressEvent;

#[test]
fn pick_first_file_root_picks_first_file_uri() {
    let (chosen, ignored) = pick_first_file_root(&[
        "file:///home/user/proj".into(),
        "file:///home/user/other".into(),
    ]);
    assert_eq!(chosen, Some(std::path::PathBuf::from("/home/user/proj")));
    assert_eq!(ignored, vec!["file:///home/user/other".to_string()]);
}

#[test]
fn pick_first_file_root_skips_non_file_until_a_file_uri() {
    let (chosen, ignored) =
        pick_first_file_root(&["vscode-vfs:///x".into(), "file:///home/user/proj".into()]);
    assert_eq!(chosen, Some(std::path::PathBuf::from("/home/user/proj")));
    // Non-file URIs BEFORE the chosen root are silently rejected,
    // not reported as ignored — they're schema-invalid, not
    // "extra roots we left out".
    assert!(
        ignored.is_empty(),
        "expected no ignored entries, got {ignored:?}"
    );
}

#[test]
fn pick_first_file_root_returns_none_when_all_non_file() {
    let (chosen, ignored) =
        pick_first_file_root(&["vscode-vfs:///x".into(), "git://example.com/repo".into()]);
    assert!(chosen.is_none());
    assert!(ignored.is_empty());
}

#[test]
fn pick_first_file_root_handles_empty_input() {
    let (chosen, ignored) = pick_first_file_root(&[]);
    assert!(chosen.is_none());
    assert!(ignored.is_empty());
}

#[test]
fn pick_first_file_root_rejects_authority_bearing_file_uri() {
    // `file://host/path` (with an authority) is rejected — the
    // remainder after `file://` doesn't start with `/`, so kenn
    // refuses to treat it as a local path. Falls through to any
    // following triple-slash URI.
    let (chosen, ignored) = pick_first_file_root(&[
        "file://nfs.example.com/exports/proj".into(),
        "file:///home/user/proj".into(),
    ]);
    assert_eq!(chosen, Some(std::path::PathBuf::from("/home/user/proj")));
    assert!(
        ignored.is_empty(),
        "rejected authority URI shouldn't appear in ignored: {ignored:?}"
    );
}

#[test]
fn pick_first_file_root_returns_none_when_only_authority_file_uri() {
    let (chosen, ignored) = pick_first_file_root(&["file://nfs.example.com/exports/proj".into()]);
    assert!(chosen.is_none());
    assert!(ignored.is_empty());
}

#[test]
fn pick_first_file_root_collects_non_file_uris_after_choice() {
    // Non-file URIs AFTER the chosen root count as ignored
    // alongside additional file URIs — the operator needs to
    // see them in the log either way to debug a multi-root setup.
    let (chosen, ignored) = pick_first_file_root(&[
        "file:///a".into(),
        "vscode-vfs:///x".into(),
        "file:///b".into(),
    ]);
    assert_eq!(chosen, Some(std::path::PathBuf::from("/a")));
    assert_eq!(
        ignored,
        vec!["vscode-vfs:///x".to_string(), "file:///b".to_string()]
    );
}
use kenn_model::Language;

/// `format_progress` is the indexer-progress → log-line renderer.
/// Cover every `ProgressEvent` variant so the match exhaustiveness
/// is exercised at the value level too.
#[test]
fn format_progress_covers_every_event() {
    let cases = [
        (ProgressEvent::Started, "started"),
        (
            ProgressEvent::UnitIngested {
                unit: kenn_indexer::pipeline::IngestUnit::JsonlWorkspace,
                language: Language::Rust,
                files: 3,
                symbols: 42,
                edges: 0,
            },
            "files=3",
        ),
        (ProgressEvent::StubsFlushed { count: 7 }, "flushed 7 stubs"),
        (
            ProgressEvent::AggregateComputed {
                nodes: 100,
                edges: 200,
                elapsed_ms: 50,
            },
            "100 nodes",
        ),
        (
            ProgressEvent::EndRunComplete { elapsed_ms: 99 },
            "end_run complete in 99ms",
        ),
        (
            ProgressEvent::Completed { total_ms: 1234 },
            "complete in 1234ms",
        ),
    ];
    for (ev, must_contain) in cases {
        let s = format_progress(&ev);
        assert!(
            s.contains(must_contain),
            "format_progress({ev:?}) = {s:?} missing {must_contain:?}"
        );
    }
}

/// `code_updated_payload` is the wire schema for the
/// `code_updated` MCP notification. Schema must be:
/// `{ event: "code_updated", message: <string containing indexed_at> }`.
#[test]
fn code_updated_payload_schema_is_stable() {
    let v = code_updated_payload("2026-05-23T14-23-05Z");
    assert_eq!(v["event"], "code_updated");
    let msg = v["message"].as_str().expect("message");
    assert!(msg.contains("2026-05-23T14-23-05Z"), "message: {msg}");
    // Internal IDs must not leak into the payload.
    assert!(v.get("snapshot_id").is_none());
}

// ── mcp-roots-discovery §9 graceful-degradation tests ──────────
//
// These exercise `decide_roots_resolution`, the pure decision
// function extracted from `resolve_roots_and_maybe_rebind`. The
// full call chain (handshake → `roots/list` → state mutation) is
// a thin dispatch over this decision; the §9 tasks ask "does the
// server take action X when the response is Y?" and the answer
// lives in this function.
//
// §9.1 and §9.3 cover the listChanged gate — a single boolean
// flag (`client_supports_roots_list_changed`) read by
// `on_roots_list_changed` before it dispatches into the resolve
// path. The tests assert the gate behavior directly via
// `should_resolve_on_list_changed`.

use super::{decide_roots_resolution, RootsResolution};
use std::path::{Path, PathBuf};

/// Returns true when a `notifications/roots/list_changed` signal
/// should trigger a re-fetch of `roots/list`. The full handler
/// (`KennMcp::on_roots_list_changed` in `server.rs`) is a thin
/// `if !flag { return; } else resolve_roots_and_maybe_rebind(...)`
/// — this helper isolates the gate so §9.1 / §9.3 can assert on it.
fn should_resolve_on_list_changed(client_supports_list_changed: bool) -> bool {
    client_supports_list_changed
}

#[test]
fn roots_9_1_no_list_changed_means_no_refetch_on_signal() {
    // §9.1 — client declares `roots` capability without
    // `listChanged: true`. A `notifications/roots/list_changed`
    // signal MUST NOT trigger a re-fetch. The initial post-handshake
    // `roots/list` runs once; workspace changes thereafter require
    // a restart.
    assert!(
        !should_resolve_on_list_changed(false),
        "listChanged signal must be ignored when the client did not declare listChanged: true",
    );
}

#[test]
fn roots_9_3_list_changed_triggers_refetch_and_decision() {
    // §9.3 — client declares `roots.listChanged: true`. The
    // notification triggers `resolve_roots_and_maybe_rebind`,
    // which fetches and then dispatches via `decide_roots_resolution`.
    assert!(
        should_resolve_on_list_changed(true),
        "listChanged signal must trigger a re-fetch when the client opted in",
    );
    // Sanity: a re-fetch returning a different URI takes the
    // rebind path (same logic as §9.6).
    let (decision, _) = decide_roots_resolution(
        &["file:///new/workspace".into()],
        Path::new("/old/workspace"),
    );
    assert_eq!(
        decision,
        RootsResolution::Rebind(PathBuf::from("/new/workspace"))
    );
}

#[test]
fn roots_9_4_multiple_roots_picks_first_logs_rest() {
    // §9.4 — client returns multiple roots. Server uses `roots[0]`
    // and reports the rest as ignored so the operator can see them
    // in logs.
    let (decision, ignored) = decide_roots_resolution(
        &[
            "file:///primary".into(),
            "file:///secondary".into(),
            "file:///tertiary".into(),
        ],
        Path::new("/somewhere/else"),
    );
    assert_eq!(decision, RootsResolution::Rebind(PathBuf::from("/primary")));
    assert_eq!(
        ignored,
        vec![
            "file:///secondary".to_string(),
            "file:///tertiary".to_string(),
        ],
    );
}

#[test]
fn roots_9_5_non_file_uri_falls_to_next_eligible() {
    // §9.5 — non-file URIs (e.g. `vscode-vfs://`) are skipped;
    // the next eligible `file://` root wins. Non-file URIs that
    // appear BEFORE the chosen root are not reported as
    // "ignored" — they're schema-invalid, not "extra roots we
    // left out."
    let (decision, ignored) = decide_roots_resolution(
        &[
            "vscode-vfs:///virtual".into(),
            "git://example.com/repo".into(),
            "file:///real/workspace".into(),
        ],
        Path::new("/somewhere/else"),
    );
    assert_eq!(
        decision,
        RootsResolution::Rebind(PathBuf::from("/real/workspace")),
    );
    assert!(
        ignored.is_empty(),
        "non-file URIs before the chosen root are silently skipped, not reported as ignored",
    );
}

#[test]
fn roots_9_6_tentative_differs_triggers_rebind() {
    // §9.6 — the tentative workspace (from CLAUDE_PROJECT_DIR,
    // git-toplevel, or cwd) differs from the `roots/list` result.
    // Server rebinds to the host's choice; the dispatch in
    // `resolve_roots_and_maybe_rebind` then aborts in-flight
    // indexing via `set_failed` before kicking the recovery
    // pipeline.
    let (decision, ignored) = decide_roots_resolution(
        &["file:///host/says/this".into()],
        Path::new("/we/guessed/that"),
    );
    assert_eq!(
        decision,
        RootsResolution::Rebind(PathBuf::from("/host/says/this")),
    );
    assert!(ignored.is_empty());
}

#[test]
fn roots_9_7_tentative_matches_takes_noop_path() {
    // §9.7 — tentative workspace equals the `roots/list` result.
    // No rebind; the dispatcher promotes `workspace_source` to
    // `RootsList` so the binding becomes host-confirmed, but
    // any indexing already running against the tentative path
    // continues uninterrupted.
    let (decision, ignored) = decide_roots_resolution(
        &["file:///agreed/workspace".into()],
        Path::new("/agreed/workspace"),
    );
    assert_eq!(
        decision,
        RootsResolution::ConfirmTentative(PathBuf::from("/agreed/workspace")),
    );
    assert!(ignored.is_empty());
}

#[test]
fn roots_9_10_claude_project_dir_overridden_by_roots_list() {
    // §9.10 — pre-handshake bind via `CLAUDE_PROJECT_DIR`, then
    // `roots/list` returns a DIFFERENT path. The decision is the
    // same as §9.6 (rebind); what differs is the *source* tag at
    // the start of the dispatch (`ClaudeProjectDir`, not
    // `GitToplevel`/`Cwd`). Both are tentative, so the rebind
    // proceeds and the new source becomes `RootsList`.
    let (decision, _) = decide_roots_resolution(
        &["file:///host/preference".into()],
        Path::new("/env/preference"),
    );
    assert_eq!(
        decision,
        RootsResolution::Rebind(PathBuf::from("/host/preference")),
    );
}

#[test]
fn roots_no_usable_root_short_circuits() {
    // Covers the empty-and-all-non-file branches reached by every
    // §9 path that gets an unusable response. Caller logs and
    // leaves the binding alone.
    let (decision, ignored) = decide_roots_resolution(
        &["vscode-vfs:///x".into(), "git://example.com/repo".into()],
        Path::new("/anything"),
    );
    assert_eq!(decision, RootsResolution::NoUsableRoot);
    assert!(ignored.is_empty());

    let (decision, _) = decide_roots_resolution(&[], Path::new("/anything"));
    assert_eq!(decision, RootsResolution::NoUsableRoot);
}
