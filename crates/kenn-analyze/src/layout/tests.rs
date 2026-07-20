use super::*;

use std::collections::{BTreeMap, HashMap};

use kenn_model::ShortId;

use crate::projection::{AggregatedGraph, AnchorMap, NodeInfo};

fn mk_graph(anchors: &[(&str, usize)]) -> AggregatedGraph {
    let mut g = AggregatedGraph {
        nodes: HashMap::new(),
        edges: Vec::new(),
        adj: HashMap::new(),
        total_weight: 0,
    };
    let mut sid: ShortId = 1;
    for (anchor, count) in anchors {
        for _ in 0..*count {
            g.nodes.insert(
                sid,
                NodeInfo {
                    kind: "class".into(),
                    name: format!("n{sid}"),
                    language: "rs".into(),
                    external: false,
                    test: false,
                    anchor_id: 1,
                    anchor_name: (*anchor).to_string(),
                },
            );
            g.adj.insert(sid, vec![]);
            sid += 1;
        }
    }
    g
}

#[test]
fn deterministic_across_calls() {
    let g = mk_graph(&[("a", 3), ("b", 5), ("c", 2)]);
    let am = AnchorMap::from_graph(&g);
    let l1 = compute(&g, &am, LayoutAlgo::Spectral);
    let l2 = compute(&g, &am, LayoutAlgo::Spectral);
    assert_eq!(l1.positions, l2.positions);
}

#[test]
fn places_every_node_exactly_once() {
    let g = mk_graph(&[("a", 4), ("b", 6)]);
    let am = AnchorMap::from_graph(&g);
    let l = compute(&g, &am, LayoutAlgo::Spectral);
    assert_eq!(l.positions.len(), 10);
    let mut ids: Vec<ShortId> = l.positions.iter().map(|(s, _, _)| *s).collect();
    ids.sort_unstable();
    assert_eq!(ids, (1..=10u32).collect::<Vec<_>>());
}

#[test]
fn anchor_discs_do_not_overlap_each_other() {
    // Three medium anchors; their disc edges should stay apart.
    let g = mk_graph(&[("a", 50), ("b", 80), ("c", 40)]);
    let am = AnchorMap::from_graph(&g);
    let l = compute(&g, &am, LayoutAlgo::Spectral);
    // Group positions by anchor (via the input).
    let mut by_anchor: BTreeMap<String, Vec<(f32, f32)>> = BTreeMap::new();
    for (sid, x, y) in &l.positions {
        let info = g.nodes.get(sid).unwrap();
        by_anchor
            .entry(info.anchor_name.clone())
            .or_default()
            .push((*x, *y));
    }
    let mut centers: Vec<(String, f32, f32, f32)> = Vec::new();
    for (name, pts) in by_anchor {
        let cx = pts.iter().map(|p| p.0).sum::<f32>() / pts.len() as f32;
        let cy = pts.iter().map(|p| p.1).sum::<f32>() / pts.len() as f32;
        // Max distance from centroid ≈ disc radius.
        let r = pts
            .iter()
            .map(|(x, y)| (x - cx).hypot(y - cy))
            .fold(0.0_f32, f32::max);
        centers.push((name, cx, cy, r));
    }
    for i in 0..centers.len() {
        for j in (i + 1)..centers.len() {
            let (_, ax, ay, ar) = centers[i];
            let (_, bx, by, br) = centers[j];
            let d = (ax - bx).hypot(ay - by);
            assert!(
                d > ar + br - 1.0,
                "anchors {} and {} overlap: dist={d} ar={ar} br={br}",
                centers[i].0,
                centers[j].0
            );
        }
    }
}

#[test]
fn empty_graph_yields_empty_layout() {
    let g = AggregatedGraph::default();
    let am = AnchorMap::from_graph(&g);
    let l = compute(&g, &am, LayoutAlgo::Spectral);
    assert!(l.positions.is_empty());
}

/// Exercise every non-Spectral layout algorithm against a small
/// multi-anchor graph. Each algorithm places every node and
/// returns a finite, non-NaN coordinate.
#[test]
fn force_layout_places_every_node() {
    let g = mk_graph(&[("a", 4), ("b", 5)]);
    let am = AnchorMap::from_graph(&g);
    let l = compute(&g, &am, LayoutAlgo::Force);
    assert_eq!(l.positions.len(), 9);
    for (_, x, y) in &l.positions {
        assert!(
            x.is_finite() && y.is_finite(),
            "non-finite position: ({x},{y})"
        );
    }
}

#[test]
fn linlog_layout_places_every_node() {
    let g = mk_graph(&[("a", 3), ("b", 4), ("c", 3)]);
    let am = AnchorMap::from_graph(&g);
    let l = compute(&g, &am, LayoutAlgo::LinLog);
    assert_eq!(l.positions.len(), 10);
    for (_, x, y) in &l.positions {
        assert!(x.is_finite() && y.is_finite());
    }
}

#[test]
fn stress_layout_places_every_node() {
    let g = mk_graph(&[("a", 3), ("b", 4)]);
    let am = AnchorMap::from_graph(&g);
    let l = compute(&g, &am, LayoutAlgo::Stress);
    assert_eq!(l.positions.len(), 7);
    for (_, x, y) in &l.positions {
        assert!(x.is_finite() && y.is_finite());
    }
}

/// `all_pairs_dijkstra` is internal to the stress layout —
/// exercised through `compute(_, _, Stress)` above, plus a direct
/// call covering the degenerate cases (empty + single-node), the
/// connected branch, and the disconnected branch (which the
/// function replaces with a finite fallback).
#[test]
fn all_pairs_dijkstra_handles_degenerate_and_connected() {
    // Empty.
    let empty = all_pairs_dijkstra(&[], 0);
    assert!(empty.is_empty());
    // Single node.
    let one = all_pairs_dijkstra(&[vec![]], 1);
    assert_eq!(one, vec![0.0]);
    // Two-node disconnected: the would-be-INFINITY distance is
    // replaced with `max_finite * 1.5 + 1.0`. With no finite
    // distances in this graph, max_finite = 0, fallback = 1.0.
    let disjoint = all_pairs_dijkstra(&[vec![], vec![]], 2);
    assert!(disjoint[0].abs() < 1e-6, "self-distance: {}", disjoint[0]);
    assert!((disjoint[1] - 1.0).abs() < 1e-6, "got {}", disjoint[1]);
    // Two-node connected (symmetric coupling): distance = 1/sqrt(w).
    let w: f32 = 2.0;
    let expected = 1.0 / w.sqrt().max(0.01);
    let connected = all_pairs_dijkstra(&[vec![(1, w)], vec![(0, w)]], 2);
    assert!(connected[0].abs() < 1e-6, "self-distance: {}", connected[0]);
    assert!(
        (connected[1] - expected).abs() < 1e-6,
        "got {}",
        connected[1]
    );
}

/// `LayoutAlgo::parse` is the CLI-arg decoder. Case-insensitive,
/// `None` on anything unknown. Cover every match arm.
#[test]
fn layout_algo_parse_decodes_every_variant() {
    for (s, expected) in [
        ("spectral", LayoutAlgo::Spectral),
        ("force", LayoutAlgo::Force),
        ("stress", LayoutAlgo::Stress),
        ("linlog", LayoutAlgo::LinLog),
        // Case-insensitive.
        ("Spectral", LayoutAlgo::Spectral),
        ("FORCE", LayoutAlgo::Force),
        ("LinLog", LayoutAlgo::LinLog),
    ] {
        assert_eq!(LayoutAlgo::parse(s), Some(expected), "parse({s:?})");
    }
    for unknown in ["", "circular", "foo", " spectral", "spectral "] {
        assert!(
            LayoutAlgo::parse(unknown).is_none(),
            "{unknown:?} must not parse"
        );
    }
}
