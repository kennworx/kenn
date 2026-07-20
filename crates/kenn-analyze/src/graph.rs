//! Self-contained HTML bird's-eye view of the aggregated graph.
//!
//! Two-level rendering:
//!
//! - **Overview** (default): one supernode per anchor, bundled
//!   anchor-to-anchor edges with summed weights. Drops a 12k/121k
//!   workspace to ~400 visible elements.
//! - **Detail** (click a supernode): that anchor's disc of nodes +
//!   intra-anchor edges + "bridge" edges from those nodes to other
//!   anchors' supernodes. Click another supernode to swap. Click
//!   background to collapse.
//!
//! Layout is precomputed in `crate::layout` (anchor-clustered
//! disc-packing). Cytoscape just paints with `name: 'preset'` — no
//! client-side layout, no force simulation, no animation.

use std::collections::HashMap;

use serde::Serialize;

use crate::layout::Layout;
use crate::projection::AggregatedGraph;

#[derive(Serialize)]
struct GraphJson<'a> {
    nodes: Vec<NodeJson<'a>>,
    edges: Vec<EdgeJson<'a>>,
    supernodes: Vec<SupernodeJson<'a>>,
    anchor_edges: Vec<AnchorEdgeJson<'a>>,
    anchors: Vec<&'a str>,
    /// Edge kinds that appear in the data, in the order the legend
    /// should list them.
    kinds: Vec<&'a str>,
}

#[derive(Serialize)]
struct NodeJson<'a> {
    id: u32,
    name: &'a str,
    kind: &'a str,
    language: &'a str,
    anchor: &'a str,
    external: bool,
    test: bool,
    weight: u64,
    /// MIME type for attachment nodes (e.g. `image/png`), derived from the
    /// filename; empty for code/markdown nodes. Drives attachment coloring.
    mime: String,
    x: f32,
    y: f32,
}

#[derive(Serialize)]
struct EdgeJson<'a> {
    a: u32,
    b: u32,
    kind: &'a str,
    weight: u32,
}

/// Overview-mode anchor representation. One supernode per anchor.
#[derive(Serialize)]
struct SupernodeJson<'a> {
    anchor: &'a str,
    node_count: usize,
    /// Sum of weighted degrees of every member node. Drives supernode
    /// size in the overview.
    total_weight: u64,
    x: f32,
    y: f32,
    /// Disc radius from layout — used as the supernode's painted size.
    radius: f32,
}

/// Overview-mode bundled edge between two anchors. Weight is the
/// summed weight of every aggregate edge that crosses these anchors.
#[derive(Serialize)]
struct AnchorEdgeJson<'a> {
    a: &'a str,
    b: &'a str,
    weight: u64,
    /// Distinct aggregate edges underneath this bundle.
    count: u32,
}

/// Stream the graph.html to `w`. Self-contained document: open in a
/// browser, no server, no dependencies beyond Cytoscape from CDN.
#[expect(
    clippy::too_many_lines,
    reason = "linear payload assembly: nodes → supernodes → bundled edges → catalogs"
)]
pub fn render<W: std::io::Write>(
    graph: &AggregatedGraph,
    layout: &Layout,
    workspace_name: &str,
    w: &mut W,
) -> std::io::Result<()> {
    let pos_by_id: HashMap<u32, (f32, f32)> = layout
        .positions
        .iter()
        .map(|(sid, x, y)| (*sid, (*x, *y)))
        .collect();
    let disc_by_anchor: HashMap<&str, (f32, f32, f32)> = layout
        .anchor_discs
        .iter()
        .map(|(n, x, y, r)| (n.as_str(), (*x, *y, *r)))
        .collect();

    let mut nodes: Vec<NodeJson<'_>> = graph
        .nodes
        .iter()
        .map(|(sid, info)| {
            let (x, y) = pos_by_id.get(sid).copied().unwrap_or((0.0, 0.0));
            NodeJson {
                id: *sid,
                name: &info.name,
                kind: &info.kind,
                language: &info.language,
                anchor: &info.anchor_name,
                external: info.external,
                test: info.test,
                weight: graph.weighted_degree(*sid),
                mime: attachment_mime(&info.kind, &info.name),
                x,
                y,
            }
        })
        .collect();
    nodes.sort_by_key(|n| n.id);

    let edges: Vec<EdgeJson<'_>> = graph
        .edges
        .iter()
        .map(|e| EdgeJson {
            a: e.a,
            b: e.b,
            kind: e.kind.db_name(),
            weight: e.weight,
        })
        .collect();

    // ── supernodes: one per anchor ────────────────────────────────
    // Bucket node weights by anchor, then emit one supernode each.
    let mut per_anchor_weight: HashMap<&str, u64> = HashMap::new();
    let mut per_anchor_count: HashMap<&str, usize> = HashMap::new();
    for n in &nodes {
        *per_anchor_weight.entry(n.anchor).or_insert(0) += n.weight;
        *per_anchor_count.entry(n.anchor).or_insert(0) += 1;
    }
    let mut supernodes: Vec<SupernodeJson<'_>> = per_anchor_weight
        .iter()
        .filter_map(|(name, &total_weight)| {
            let &(x, y, radius) = disc_by_anchor.get(name)?;
            let node_count = per_anchor_count.get(name).copied().unwrap_or(0);
            Some(SupernodeJson {
                anchor: name,
                node_count,
                total_weight,
                x,
                y,
                radius,
            })
        })
        .collect();
    supernodes.sort_by_key(|s| s.anchor);

    // ── bundled anchor → anchor edges ────────────────────────────
    // Group aggregate edges by (min_anchor, max_anchor) pair (sorted
    // alphabetically) so each pair appears once. Sum weights, count
    // underlying aggregate edges.
    let id_to_anchor: HashMap<u32, &str> = nodes.iter().map(|n| (n.id, n.anchor)).collect();
    let mut anchor_pairs: HashMap<(&str, &str), (u64, u32)> = HashMap::new();
    for e in &graph.edges {
        let Some(&a_anchor) = id_to_anchor.get(&e.a) else {
            continue;
        };
        let Some(&b_anchor) = id_to_anchor.get(&e.b) else {
            continue;
        };
        if a_anchor == b_anchor {
            continue; // intra-anchor; not part of the overview bundle
        }
        let (lo, hi) = if a_anchor <= b_anchor {
            (a_anchor, b_anchor)
        } else {
            (b_anchor, a_anchor)
        };
        let entry = anchor_pairs.entry((lo, hi)).or_insert((0, 0));
        entry.0 += u64::from(e.weight);
        entry.1 += 1;
    }
    let mut anchor_edges: Vec<AnchorEdgeJson<'_>> = anchor_pairs
        .into_iter()
        .map(|((a, b), (weight, count))| AnchorEdgeJson {
            a,
            b,
            weight,
            count,
        })
        .collect();
    anchor_edges.sort_by(|x, y| x.a.cmp(y.a).then(x.b.cmp(y.b)));

    // ── anchor / kind catalogs (for sidebar) ─────────────────────
    let mut anchors: Vec<&str> = graph
        .nodes
        .values()
        .map(|n| n.anchor_name.as_str())
        .collect();
    anchors.sort_unstable();
    anchors.dedup();

    let mut kinds: Vec<&str> = graph.edges.iter().map(|e| e.kind.db_name()).collect();
    kinds.sort_unstable();
    kinds.dedup();

    let payload = GraphJson {
        nodes,
        edges,
        supernodes,
        anchor_edges,
        anchors,
        kinds,
    };

    let title = if workspace_name.is_empty() {
        "kenn analyze — aggregated graph".to_string()
    } else {
        format!("kenn analyze — {workspace_name}")
    };
    let (prefix, suffix) = split_template();
    let prefix = prefix.replace("{{TITLE}}", &html_escape_text(&title));
    w.write_all(prefix.as_bytes())?;
    serde_json::to_writer(&mut *w, &payload).map_err(std::io::Error::other)?;
    w.write_all(suffix.as_bytes())?;
    Ok(())
}

/// MIME type of an attachment node, derived from its filename (e.g.
/// `image/png`). Empty for non-attachment kinds and for attachments whose
/// extension has no known MIME — the renderer falls back to a generic
/// attachment color in that case.
fn attachment_mime(kind: &str, name: &str) -> String {
    if kind != "attachment" {
        return String::new();
    }
    mime_guess::from_path(name)
        .first()
        .map(|m| m.essence_str().to_string())
        .unwrap_or_default()
}

fn html_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn split_template() -> (&'static str, &'static str) {
    let idx = HTML_TEMPLATE
        .find("{{DATA_JSON}}")
        .expect("template contains placeholder");
    // `{{DATA_JSON}}` is ASCII, so `idx` and `idx + len` always fall on
    // character boundaries — the slice can't panic.
    #[expect(clippy::string_slice, reason = "ASCII placeholder boundary")]
    (
        &HTML_TEMPLATE[..idx],
        &HTML_TEMPLATE[idx + "{{DATA_JSON}}".len()..],
    )
}

const HTML_TEMPLATE: &str = include_str!("graph.html");

#[cfg(test)]
mod tests {
    use super::render;
    use crate::layout::{compute, LayoutAlgo};
    use crate::projection::{AggregateEdge, AggregatedGraph, AnchorMap, NodeInfo};
    use std::collections::HashMap;

    fn mk_graph() -> AggregatedGraph {
        let mut g = AggregatedGraph {
            nodes: HashMap::new(),
            edges: Vec::new(),
            adj: HashMap::new(),
            total_weight: 0,
        };
        for (sid, anchor) in [(1u32, "alpha"), (2, "alpha"), (3, "beta"), (4, "beta")] {
            g.nodes.insert(
                sid,
                NodeInfo {
                    kind: "class".into(),
                    name: format!("Sym{sid}"),
                    language: "rs".into(),
                    external: false,
                    test: false,
                    anchor_id: 1,
                    anchor_name: anchor.into(),
                },
            );
            g.adj.insert(sid, vec![]);
        }
        // One within-anchor edge, one cross-anchor edge.
        g.edges.push(AggregateEdge {
            a: 1,
            b: 2,
            kind: kenn_model::EdgeKind::Calls,
            weight: 3,
        });
        g.edges.push(AggregateEdge {
            a: 2,
            b: 3,
            kind: kenn_model::EdgeKind::TypeUse,
            weight: 5,
        });
        g.total_weight = 8;
        g
    }

    /// `render` produces an HTML payload by string-substituting the
    /// `{{DATA_JSON}}` placeholder in the embedded template. Exercise
    /// every payload section (nodes, supernodes, bundled edges,
    /// catalogs) by feeding a small multi-anchor graph through it.
    #[test]
    fn render_emits_html_with_node_and_supernode_payload() {
        let g = mk_graph();
        let am = AnchorMap::from_graph(&g);
        let layout = compute(&g, &am, LayoutAlgo::Spectral);
        let mut buf = Vec::new();
        render(&g, &layout, "test-ws", &mut buf).expect("render");
        let html = String::from_utf8(buf).expect("utf8");
        // Workspace name is interpolated somewhere into the HTML.
        assert!(html.contains("test-ws"), "workspace name missing");
        // The data JSON includes node names, anchor names, and edge kinds.
        assert!(html.contains("Sym1"), "node names missing");
        assert!(html.contains("alpha"), "anchor names missing");
        assert!(
            html.contains("calls") && html.contains("type_use"),
            "edge kinds missing"
        );
    }

    #[test]
    fn render_handles_empty_graph() {
        let g = AggregatedGraph::default();
        let am = AnchorMap::from_graph(&g);
        let layout = compute(&g, &am, LayoutAlgo::Spectral);
        let mut buf = Vec::new();
        render(&g, &layout, "empty-ws", &mut buf).expect("render");
        let html = String::from_utf8(buf).expect("utf8");
        assert!(html.contains("empty-ws"));
    }
}
