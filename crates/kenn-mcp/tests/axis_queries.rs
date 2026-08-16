//! In-process coverage for the atlas axis queries — `list_contracts`,
//! `list_domains`, `list_packages`.
//!
//! **Why this file exists.** Before it, these three had *zero* in-process
//! coverage: the only thing exercising them was `cli_smoke.rs`, which spawns
//! the `kenn` binary as a child process, so `llvm-cov` never saw a line of
//! them. That stayed invisible while each query body sat inside an
//! `async move` closure — the closure carried the branches and the enclosing
//! function scored as though it had none. Moving the bodies onto `QueryCtx`
//! un-nested them and the CRAP gate reported what had always been true.
//!
//! So the fixture is deliberately built to REACH the branches, not merely to
//! call the functions. Each axis has a named-lookup path that a bare listing
//! never touches, and that path is the one that was uncovered:
//!
//! - contracts → `contract_row` + `resolve_implementers`
//! - domains   → `domain_row` + `fill_domain_detail`
//! - packages  → `resolve_root_symbols`, including the C# namespace fallback
//!
//! The graph below is the smallest one that satisfies every gate those paths
//! pass through — notably `MIN_CONTRACT_PKGS = 2`, which means a contract needs
//! implementers in two *different* packages before it is selected at all.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kenn_mcp::state::LifecycleState;
use kenn_mcp::tools::ServerState;
use kenn_model::{
    AggregateEdgeRecord, AggregateNodeRecord, AnalysisFlatCommunityRecord,
    AnalysisNodeMembershipRecord, DefRecord, EdgeKind, FileRecord, Kind, Language, SymbolRecord,
};
use kenn_query::snapshot_id_from_timestamp;
use kenn_query::{
    list_contracts, list_domains, list_packages, ListContractsArgs, ListDomainsArgs,
    ListPackagesArgs,
};
use kenn_store::api::WriteBatch;
use kenn_store::{open_writer, reader_from_writer, DbReader, DbWriter, WriterOptions};
use tempfile::TempDir;

// Rust side: a contract and two implementers, deliberately in two packages.
const HANDLER: u32 = 1;
const A_HANDLER: u32 = 2;
const B_HANDLER: u32 = 3;
// C# side: two types whose namespace is NOT derivable from the assembly name.
const INVOICE: u32 = 4;
const LEDGER: u32 = 5;
const BILLING_NS: u32 = 6;
// Two more community members, deliberately NOT implementers. A domain has to
// clear three separate floors, and the first two drafts of this fixture each
// tripped one — both times the test failed with an empty listing rather than a
// wrong value, which is what "the fixture never reached the branch" looks like:
//
//   MIN_DOMAIN_SIZE  = 4   members in the community
//   MIN_PKG_MEMBERS  = 2   members in EACH spanned package
//   MIN_DOMAIN_LINKS = 2   distinct cross-package edges
//
// `pkg-b` needs a second member for the span to be supported at all, and a
// second cross-package edge to clear the link floor. Both join by `Calls`, not
// `Implements`, so the contract axis still sees exactly two implementers.
const ROUTER: u32 = 7;
const B_ROUTER: u32 = 8;

const PKG_A: &str = "pkg-a";
const PKG_B: &str = "pkg-b";
/// The assembly name differs from the namespace its types live in — the whole
/// point of the C# fallback.
const CS_PKG: &str = "Acme.Billing.Data";

fn symbol(id: u32, pub_id: &str, kind: Kind, language: Language, name: &str) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: pub_id.into(),
        language,
        pkg_id: 0,
        kind,
        name: name.into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

fn node(id: u32, kind: Kind, name: &str, language: Language, anchor: &str) -> AggregateNodeRecord {
    AggregateNodeRecord {
        id,
        kind,
        name: name.into(),
        language,
        external: false,
        test: false,
        example: false,
        anchor_id: 0,
        anchor_name: anchor.into(),
    }
}

/// The symbol/file/def half of the corpus. Split from the aggregate and
/// analysis halves purely for length — the three phases are independent.
fn corpus_batch() -> WriteBatch {
    WriteBatch {
        packages: Vec::new(),
        files: vec![FileRecord {
            id: 100,
            path: "src/lib.rs".into(),
            language: Language::Rust,
            test: false,
            external: false,
            content_hash: 1,
        }],
        symbols: vec![
            symbol(
                HANDLER,
                "rs:pkg-a::Handler",
                Kind::Trait,
                Language::Rust,
                "Handler",
            ),
            symbol(
                A_HANDLER,
                "rs:pkg-a::AHandler",
                Kind::Struct,
                Language::Rust,
                "AHandler",
            ),
            symbol(
                B_HANDLER,
                "rs:pkg-b::BHandler",
                Kind::Struct,
                Language::Rust,
                "BHandler",
            ),
            // The C# types live under `Acme.Billing`, NOT under the assembly
            // name `Acme.Billing.Data` — so `cs:Acme.Billing.Data` resolves to
            // nothing and the first pass of `resolve_root_symbols` leaves the
            // package's symbol empty, which is what reaches the fallback.
            symbol(
                INVOICE,
                "cs:Acme.Billing.Invoice",
                Kind::Class,
                Language::Csharp,
                "Invoice",
            ),
            symbol(
                LEDGER,
                "cs:Acme.Billing.Ledger",
                Kind::Class,
                Language::Csharp,
                "Ledger",
            ),
            // The namespace the fallback must find and verify.
            symbol(
                BILLING_NS,
                "cs:Acme.Billing",
                Kind::Namespace,
                Language::Csharp,
                "Acme.Billing",
            ),
            symbol(
                ROUTER,
                "rs:pkg-a::Router",
                Kind::Struct,
                Language::Rust,
                "Router",
            ),
            symbol(
                B_ROUTER,
                "rs:pkg-b::BRouter",
                Kind::Struct,
                Language::Rust,
                "BRouter",
            ),
        ],
        symbol_docs: Vec::new(),
        file_docs: Vec::new(),
        defs: vec![DefRecord {
            sym_id: HANDLER,
            file_id: 100,
            start_line: 1,
            start_col: 0,
            end_line: 3,
            end_col: 0,
            body_start_line: 0,
            body_end_line: 0,
        }],
        edges: Vec::new(),
    }
}

/// The aggregate + analysis halves: the rolled-up graph the axis selectors
/// actually read, and the community tables the domain axis needs.
async fn write_graph(writer: &DbWriter) {
    writer
        .write_aggregate_tables(
            &[
                node(HANDLER, Kind::Trait, "Handler", Language::Rust, PKG_A),
                node(A_HANDLER, Kind::Struct, "AHandler", Language::Rust, PKG_A),
                node(B_HANDLER, Kind::Struct, "BHandler", Language::Rust, PKG_B),
                node(INVOICE, Kind::Class, "Invoice", Language::Csharp, CS_PKG),
                node(LEDGER, Kind::Class, "Ledger", Language::Csharp, CS_PKG),
                node(ROUTER, Kind::Struct, "Router", Language::Rust, PKG_A),
                node(B_ROUTER, Kind::Struct, "BRouter", Language::Rust, PKG_B),
            ],
            &[
                // Direction is load-bearing: implementer → contract.
                AggregateEdgeRecord {
                    src_id: A_HANDLER,
                    dst_id: HANDLER,
                    kind: EdgeKind::Implements,
                    weight: 1,
                },
                AggregateEdgeRecord {
                    src_id: B_HANDLER,
                    dst_id: HANDLER,
                    kind: EdgeKind::Implements,
                    weight: 1,
                },
                // `Calls`, not `Implements` — Router makes the community reach
                // MIN_DOMAIN_SIZE without becoming a third implementer.
                AggregateEdgeRecord {
                    src_id: ROUTER,
                    dst_id: A_HANDLER,
                    kind: EdgeKind::Calls,
                    weight: 2,
                },
                // The SECOND cross-package edge: one is not enough.
                AggregateEdgeRecord {
                    src_id: B_ROUTER,
                    dst_id: ROUTER,
                    kind: EdgeKind::Calls,
                    weight: 2,
                },
            ],
        )
        .await
        .expect("write_aggregate_tables");

    writer
        .write_analysis_tables(
            &[],
            &[AnalysisFlatCommunityRecord {
                community_id: 0,
                // Four, not three: `MIN_DOMAIN_SIZE` is 4 and the community is
                // dropped below it — by the `keep` filter here and again inside
                // `select_domains`.
                size: 5,
                total_weight: 4,
                cross_anchor: true,
                primary_anchor_id: 0,
                primary_anchor_name: PKG_A.into(),
            }],
            &[],
            &[
                AnalysisNodeMembershipRecord {
                    short_id: HANDLER,
                    flat_community_id: 0,
                    anchored_leaf_community_id: 0,
                },
                AnalysisNodeMembershipRecord {
                    short_id: A_HANDLER,
                    flat_community_id: 0,
                    anchored_leaf_community_id: 0,
                },
                AnalysisNodeMembershipRecord {
                    short_id: B_HANDLER,
                    flat_community_id: 0,
                    anchored_leaf_community_id: 0,
                },
                AnalysisNodeMembershipRecord {
                    short_id: ROUTER,
                    flat_community_id: 0,
                    anchored_leaf_community_id: 0,
                },
                AnalysisNodeMembershipRecord {
                    short_id: B_ROUTER,
                    flat_community_id: 0,
                    anchored_leaf_community_id: 0,
                },
            ],
        )
        .await
        .expect("write_analysis_tables");
}

async fn build_corpus(dir: &Path) -> DbWriter {
    let writer = open_writer(dir, WriterOptions::default())
        .await
        .expect("open_writer");
    writer
        .write_batch(&corpus_batch())
        .await
        .expect("write_batch");
    write_graph(&writer).await;
    writer.finalize().await.expect("finalize");
    writer
}

fn ready_state(workspace: &Path, reader: DbReader) -> ServerState {
    let state = ServerState::new(workspace);
    let snap_path = PathBuf::from("in-process");
    let store = kenn_store::Store::open_default(workspace).expect("store");
    let pin = kenn_store::readers::register_reader(&store, &snap_path).expect("pin");
    *state.lifecycle.write().expect("lifecycle lock") = LifecycleState::Ready {
        snapshot_path: snap_path,
        snapshot_id: snapshot_id_from_timestamp("axis-queries-test"),
        indexed_at: "axis-queries-test".into(),
        read: arc_swap::ArcSwap::from(Arc::new(kenn_mcp::state::ReaderBinding::new(reader, pin))),
        fallback_from_parent: false,
        reindex: None,
        run_meta: None,
    };
    state
}

/// Build the workspace and return the pieces every test needs. The `ServerState`
/// is returned alongside so the caller can hold it — `QueryCtx` borrows from it.
async fn workspace(dir: &Path) -> ServerState {
    let writer = build_corpus(dir).await;
    ready_state(dir, reader_from_writer(&writer).await.expect("reader"))
}

/// Naming a contract returns its implementers grouped by package — the
/// `contract_row` + `resolve_implementers` path a bare listing never runs.
#[tokio::test(flavor = "multi_thread")]
async fn naming_a_contract_returns_its_implementers_by_package() {
    let dir = TempDir::new().expect("tempdir");
    let state = workspace(dir.path()).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    // Bare listing: the contract is selected because it spans two packages.
    let bare = list_contracts(
        &ctx,
        &ListContractsArgs {
            contract: None,
            pagination: None,
        },
    )
    .await
    .expect("list_contracts");
    assert_eq!(bare.items.len(), 1, "one cross-package contract: {bare:?}");
    let c = &bare.items[0];
    assert_eq!(c.title, "Handler");
    assert_eq!(c.package_span, 2);
    assert!(
        c.implementers.is_empty(),
        "a bare listing stays flat — detail is the named path"
    );

    // Named: the same contract, now carrying its implementers.
    let named = list_contracts(
        &ctx,
        &ListContractsArgs {
            contract: Some("Handler".into()),
            pagination: None,
        },
    )
    .await
    .expect("list_contracts named");
    assert_eq!(named.items.len(), 1);
    let mut pkgs: Vec<&str> = named.items[0]
        .implementers
        .iter()
        .map(|i| i.package.as_str())
        .collect();
    pkgs.sort_unstable();
    assert_eq!(
        pkgs,
        [PKG_A, PKG_B],
        "both implementers resolved, each tagged with its own package"
    );

    // A name that matches nothing is an empty answer, not an error — the
    // `return Ok(None)` arm of `contract_row`.
    let missing = list_contracts(
        &ctx,
        &ListContractsArgs {
            contract: Some("NoSuchContract".into()),
            pagination: None,
        },
    )
    .await
    .expect("list_contracts missing");
    assert!(missing.items.is_empty());
}

/// Naming a domain returns its spanned packages and central members — the
/// `domain_row` + `fill_domain_detail` path.
#[tokio::test(flavor = "multi_thread")]
async fn naming_a_domain_returns_its_packages_and_central_members() {
    let dir = TempDir::new().expect("tempdir");
    let state = workspace(dir.path()).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let bare = list_domains(
        &ctx,
        &ListDomainsArgs {
            domain: None,
            pagination: None,
        },
    )
    .await
    .expect("list_domains");
    assert_eq!(bare.items.len(), 1, "one community → one domain: {bare:?}");
    let title = bare.items[0].title.clone();
    assert!(
        bare.items[0].packages.is_empty() && bare.items[0].central.is_empty(),
        "bare listing stays flat"
    );

    let named = list_domains(
        &ctx,
        &ListDomainsArgs {
            domain: Some(title.clone()),
            pagination: None,
        },
    )
    .await
    .expect("list_domains named");
    assert_eq!(named.items.len(), 1, "named domain: {title}");
    let d = &named.items[0];
    assert!(
        !d.packages.is_empty(),
        "the named path fills the spanned packages"
    );
    assert!(
        !d.central.is_empty(),
        "and resolves central members to addressable ids"
    );
    assert!(
        d.central.iter().all(|m| m.id.starts_with("rs:")),
        "every central member is a resolvable pub_id: {:?}",
        d.central
    );

    let missing = list_domains(
        &ctx,
        &ListDomainsArgs {
            domain: Some("NoSuchDomain".into()),
            pagination: None,
        },
    )
    .await
    .expect("list_domains missing");
    assert!(missing.items.is_empty());
}

/// Every package row carries a resolvable root `symbol` — including the C#
/// project whose namespace cannot be constructed from its assembly name, which
/// is the fallback half of `resolve_root_symbols`.
#[tokio::test(flavor = "multi_thread")]
async fn package_rows_resolve_a_root_symbol_including_the_csharp_fallback() {
    let dir = TempDir::new().expect("tempdir");
    let state = workspace(dir.path()).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let all = list_packages(
        &ctx,
        &ListPackagesArgs {
            package: None,
            pagination: None,
        },
    )
    .await
    .expect("list_packages");
    let cs = all
        .items
        .iter()
        .find(|p| p.name == CS_PKG)
        .unwrap_or_else(|| panic!("the C# package is listed: {:?}", all.items));

    // `cs:Acme.Billing.Data` does not exist; the fallback derives the DEFAULT
    // namespace from where the project's types actually live and verifies it.
    assert_eq!(
        cs.symbol, "cs:Acme.Billing",
        "the assembly name is not the namespace — the fallback found the real one"
    );
}
