//! Integration tests for the findings store
//! (`openspec/changes/findings-backend` + the embedding-producer change).
//!
//! - 8.1 round-trip + provenance: store / flush / `get_finding`,
//!   `find_predecessors` reaching a code-graph node and terminating.
//! - 8.2 git-merge union: two branches each `store_finding` + `flush`,
//!   a plain `git merge`, both branches' findings present, no conflict.
//! - 8.3 supersede / tombstone / staleness branch-correctness.
//!
//! These exercise the lexical / structural surface only — embedding
//! stays off (no `init_shared_embedder` call) so the suite never loads a
//! model. Real-model hybrid retrieval is covered in `hybrid_search.rs`.

use std::collections::HashSet;
use std::path::Path;

use kenn_store::{CodeNodeResolver, FindingsStore, Layout, Store};
use tempfile::TempDir;

/// A trivial in-memory resolver — seeded code-node ids are "live".
struct SetResolver(HashSet<String>);

impl CodeNodeResolver for SetResolver {
    fn contains(&self, code_node_id: &str) -> bool {
        self.0.contains(code_node_id)
    }
}

fn resolver(ids: &[&str]) -> SetResolver {
    SetResolver(ids.iter().map(|s| (*s).to_owned()).collect())
}

/// Materialize a live run with a findings mirror built from the
/// currently-committed records and point `live` at it — the production
/// shape a findings read requires. Reads return "no live snapshot"
/// until a run is published, so each test calls this once its records
/// are on disk.
async fn publish_live_run(workspace: &Path) {
    let layout = Layout::default_for(workspace);
    Store::open(layout.clone()).expect("store");
    let run_id = "test-run";
    let run_dir = layout.run_dir(run_id);
    std::fs::create_dir_all(&run_dir).expect("run dir");
    let lock = kenn_store::stage_findings_for_publish(&layout, &run_dir)
        .await
        .expect("stage findings");
    let live = layout.live_path();
    drop(std::fs::remove_file(&live));
    std::fs::write(&live, format!("runs/{run_id}")).expect("live pointer");
    drop(lock);
}

// ── 8.1 round-trip + provenance ─────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn round_trip_and_provenance() {
    let dir = TempDir::new().unwrap();
    let mut store = FindingsStore::open_default(dir.path()).await.unwrap();

    // A base finding parenting a code-graph node.
    let (base, _) = store
        .store_finding(
            "the parser treats a leading bom as whitespace".to_owned(),
            vec!["rust:parser::scan".to_owned()],
            vec!["gotcha".to_owned(), "parser".to_owned()],
            None,
        )
        .await
        .unwrap();
    // A derived finding parenting the base finding.
    let (derived, _) = store
        .store_finding(
            "callers must strip the bom before hashing".to_owned(),
            vec![base.clone()],
            vec!["invariant".to_owned()],
            None,
        )
        .await
        .unwrap();
    store.flush().await.unwrap();
    // Publish a live run so reads resolve through the findings mirror.
    publish_live_run(dir.path()).await;

    // `get_finding` round-trips text, tags, parent_ids.
    let got = store.get_finding(&derived).await.unwrap().unwrap();
    assert_eq!(got.text, "callers must strip the bom before hashing");
    assert_eq!(got.tags, vec!["invariant".to_owned()]);
    assert_eq!(got.parent_ids, vec![base.clone()]);
    assert!(
        got.embedding.is_none(),
        "decode does not read the embedding column back into the record"
    );

    // `find_predecessors` reaches the originating code-graph node and
    // terminates (no cycle).
    let preds = store.find_predecessors(&derived).await.unwrap();
    assert!(preds.contains(&base), "reaches the parent finding");
    assert!(
        preds.contains(&"rust:parser::scan".to_owned()),
        "reaches the originating code-graph node"
    );

    // Survives a fresh open of the store.
    let reopened = FindingsStore::open_default(dir.path()).await.unwrap();
    let after = reopened.get_finding(&base).await.unwrap().unwrap();
    assert_eq!(after.text, "the parser treats a leading bom as whitespace");
}

// ── 8.2 git-merge union ─────────────────────────────────────────────

/// Two branches each add findings; a plain `git merge` unions them with
/// no conflict and the reopened store covers both. Mirrors the
/// `merge_via_git_unions_rows` test in `db::lance::store`.
#[tokio::test(flavor = "multi_thread")]
async fn merge_via_git_unions_findings() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return; // git unavailable — skip
    }
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    // `.kenn/` so `findings_dir_for` resolves to `<repo>/.kenn/findings`.
    let workspace = repo.join(".kenn");
    std::fs::create_dir_all(&workspace).unwrap();

    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .current_dir(repo)
            .status()
            .expect("run git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-b", "main"]);

    // main: one finding.
    let main_id;
    {
        let mut s = FindingsStore::open_default(&workspace).await.unwrap();
        main_id = s
            .store_finding("main branch fact".to_owned(), vec![], vec![], None)
            .await
            .unwrap()
            .0;
        s.flush().await.unwrap();
    }
    git(&["add", "-A"]);
    git(&["commit", "-m", "main finding"]);

    // feature branch: a disjoint finding.
    git(&["checkout", "-b", "feature"]);
    let feature_id;
    {
        let mut s = FindingsStore::open_default(&workspace).await.unwrap();
        feature_id = s
            .store_finding("feature branch fact".to_owned(), vec![], vec![], None)
            .await
            .unwrap()
            .0;
        s.flush().await.unwrap();
    }
    git(&["add", "-A"]);
    git(&["commit", "-m", "feature finding"]);

    // back on main: another disjoint finding.
    git(&["checkout", "main"]);
    let main_id2;
    {
        let mut s = FindingsStore::open_default(&workspace).await.unwrap();
        main_id2 = s
            .store_finding("second main fact".to_owned(), vec![], vec![], None)
            .await
            .unwrap()
            .0;
        s.flush().await.unwrap();
    }
    git(&["add", "-A"]);
    git(&["commit", "-m", "second main finding"]);

    // plain merge — conflict-free (every Lance file is unique-named).
    git(&["merge", "feature", "--no-edit"]);

    // A fresh index pass rebuilds the mirror from the merged records.
    publish_live_run(&workspace).await;

    // Reopening reconciles the merge; the store covers all three.
    let merged = FindingsStore::open_default(&workspace).await.unwrap();
    assert!(merged.get_finding(&main_id).await.unwrap().is_some());
    assert!(merged.get_finding(&main_id2).await.unwrap().is_some());
    assert!(
        merged.get_finding(&feature_id).await.unwrap().is_some(),
        "feature branch finding present after merge"
    );
}

// ── 8.3 supersede / tombstone / staleness ───────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn supersede_tombstone_and_staleness() {
    let dir = TempDir::new().unwrap();
    let mut store = FindingsStore::open_default(dir.path()).await.unwrap();

    // Two originals.
    let (original, _) = store
        .store_finding(
            "the queue drops messages when full".to_owned(),
            vec![],
            vec![],
            None,
        )
        .await
        .unwrap();
    let (target, _) = store
        .store_finding(
            "the queue drops the oldest message when full".to_owned(),
            vec![],
            vec![],
            None,
        )
        .await
        .unwrap();
    store.flush().await.unwrap();

    // A correction supersedes `original`.
    let (correction, _) = store
        .store_finding(
            "the queue blocks the producer when full".to_owned(),
            vec![original.clone()],
            vec![format!("supersedes:{original}")],
            None,
        )
        .await
        .unwrap();
    // A tombstone removes `target`.
    store
        .store_finding(
            "removed".to_owned(),
            vec![target.clone()],
            vec![format!("tombstone:{target}")],
            None,
        )
        .await
        .unwrap();
    store.flush().await.unwrap();
    // Publish a live run so reads resolve; later flushes append to it.
    publish_live_run(dir.path()).await;

    // search hides the superseded original and the tombstoned target,
    // but shows the superseding correction.
    let hits = store
        .search_findings("queue full", None, 10, &resolver(&[]))
        .await
        .unwrap();
    let ids: HashSet<&str> = hits.iter().map(|h| h.finding.id.as_str()).collect();
    assert!(!ids.contains(original.as_str()), "superseded hidden");
    assert!(!ids.contains(target.as_str()), "tombstoned hidden");
    assert!(ids.contains(correction.as_str()), "correction shown");

    // `get_finding` ignores lifecycle — the raw record is still there.
    assert!(store.get_finding(&original).await.unwrap().is_some());
    assert!(store.get_finding(&target).await.unwrap().is_some());

    // Staleness branch-correctness: a finding over a code node is stale
    // under a resolver missing that node, live under one that has it.
    let (with_code, _) = store
        .store_finding(
            "the planner caches the cost model".to_owned(),
            vec!["rust:planner::cost".to_owned()],
            vec![],
            None,
        )
        .await
        .unwrap();
    store.flush().await.unwrap();

    let live = store
        .search_findings(
            "planner cost model",
            None,
            10,
            &resolver(&["rust:planner::cost"]),
        )
        .await
        .unwrap();
    let live_hit = live
        .iter()
        .find(|h| h.finding.id == with_code)
        .expect("found on the branch where its code exists");
    assert!(!live_hit.stale, "not stale where the code node resolves");

    let gone = store
        .search_findings("planner cost model", None, 10, &resolver(&[]))
        .await
        .unwrap();
    let gone_hit = gone
        .iter()
        .find(|h| h.finding.id == with_code)
        .expect("still returned — stale findings are flagged, not omitted");
    assert!(gone_hit.stale, "stale where the code node is gone");
}

// ── claims decay (openspec/changes/claims-decay) ─────────────────────
//
// A RULE says how the codebase works and survives edits to the file it is
// anchored to. A CLAIM asserts the current state of the code, and whoever
// later changes that code has no reason to look for a finding describing it —
// so the claim quietly stops being true while still being served as fact.

/// Store one finding, anchor it to `file` with the sha of its CURRENT content,
/// then rewrite the file so the anchor drifts.
async fn drifted_finding(ws: &Path, tags: &[&str], file: &str) -> String {
    let mut store = FindingsStore::open_default(ws).await.expect("open");
    let (id, _) = store
        .store_finding(
            "an assertion".to_owned(),
            vec![],
            tags.iter().map(|t| (*t).to_owned()).collect(),
            None,
        )
        .await
        .expect("store");
    store.flush().await.expect("flush");

    let abs = ws.join(file);
    std::fs::write(&abs, "before").expect("seed");
    let sha = kenn_store::file_content_sha(&abs);
    store
        .record_anchor_event(
            &id,
            &kenn_store::AnchorEvent::Attach {
                anchor: file.to_owned(),
                ts: kenn_store::Timestamp::now(),
                sha,
            },
        )
        .await
        .expect("attach");
    // The change nobody linked back to the finding.
    std::fs::write(&abs, "after").expect("edit");
    id
}

#[tokio::test]
async fn a_drifted_claim_is_unverified_and_a_drifted_rule_is_not() {
    let dir = TempDir::new().expect("tmp");
    let ws = dir.path();
    let claim = drifted_finding(ws, &["bug", "deferred"], "claim.rs").await;
    let rule = drifted_finding(ws, &["directive", "polarity:do"], "rule.rs").await;

    let store = FindingsStore::open_default(ws).await.expect("open");
    let health = store.check_anchors().await.expect("check");

    assert!(
        health.unverified.iter().any(|u| u.finding_id == claim),
        "a claim whose code moved needs re-verifying, not a routine re-read"
    );
    assert!(
        health.drifted.iter().any(|d| d.finding_id == rule),
        "a rule still reports as ordinary drift"
    );
    assert!(
        !health.unverified.iter().any(|u| u.finding_id == rule),
        "a rule survives edits to its anchor — it is not an unverified claim"
    );
    assert!(
        !health.drifted.iter().any(|d| d.finding_id == claim),
        "a claim is reported once, in the bucket that says what to do about it"
    );
}

#[tokio::test]
async fn an_unmarked_finding_is_a_rule() {
    // The safe default: this store is overwhelmingly rules, and treating every
    // unmarked finding as a decaying claim would flood the re-verification
    // surface with entries that do not need it — which is how a signal stops
    // being read.
    let dir = TempDir::new().expect("tmp");
    let ws = dir.path();
    let id = drifted_finding(ws, &[], "unmarked.rs").await;

    let store = FindingsStore::open_default(ws).await.expect("open");
    let health = store.check_anchors().await.expect("check");

    assert!(health.drifted.iter().any(|d| d.finding_id == id));
    assert!(!health.unverified.iter().any(|u| u.finding_id == id));
}

#[tokio::test]
async fn a_superseded_finding_is_in_no_health_bucket() {
    // A superseded ancestor is already excluded from retrieval, so it can never
    // be served as guidance and repairing its anchors changes nothing. Measured
    // on the kenn repo, 26 of 127 drifted entries were superseded ancestors — a
    // fifth of the list, which is how a report trains its reader to skim.
    let dir = TempDir::new().expect("tmp");
    let ws = dir.path();
    let old = drifted_finding(ws, &["bug"], "old.rs").await;

    let mut store = FindingsStore::open_default(ws).await.expect("open");
    store
        .store_finding(
            "the current state".to_owned(),
            vec![old.clone()],
            vec!["directive".to_owned(), format!("supersedes:{old}")],
            None,
        )
        .await
        .expect("supersede");
    store.flush().await.expect("flush");

    let health = store.check_anchors().await.expect("check");
    assert!(
        !health.unverified.iter().any(|u| u.finding_id == old),
        "a superseded claim is history, not outstanding re-verification"
    );
    assert!(!health.drifted.iter().any(|d| d.finding_id == old));
    assert!(!health.broken.iter().any(|b| b.finding_id == old));
}
