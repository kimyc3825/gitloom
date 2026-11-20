//! Lane layout for the commit graph.
//!
//! Input: commits in display order (children before parents), each with its
//! parent ids. Output: for every row, the lane (column) of the commit node and
//! the line segments to draw inside that row.
//!
//! Lanes are never compacted, so a lane keeps its column for its whole life,
//! which makes every pass-through line a straight vertical segment. Freed
//! lanes are reused by later branches.

/// Index into the lane color palette (the UI picks `palette[color % len]`).
pub type LaneColor = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// A line crossing the row from top to bottom without touching the node.
    Pass,
    /// A line entering the node from the top of the row.
    In,
    /// A line leaving the node towards the bottom of the row.
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub kind: EdgeKind,
    /// `Pass`/`In`: lane at the top of the row. `Out`: lane at the bottom.
    pub lane: usize,
    pub color: LaneColor,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// Column of the commit node.
    pub lane: usize,
    pub color: LaneColor,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, Default)]
pub struct GraphLayout {
    pub rows: Vec<GraphRow>,
    /// Number of columns needed to draw every row.
    pub width: usize,
}

/// Compute the lane layout. `K` is the commit id type (`git2::Oid` in the app,
/// plain integers in tests).
pub fn layout<'a, K, I>(nodes: I) -> GraphLayout
where
    K: Copy + Eq + 'a,
    I: IntoIterator<Item = (K, &'a [K])>,
{
    // lanes[i] = the commit lane `i` is waiting for, and its color.
    let mut lanes: Vec<Option<(K, LaneColor)>> = Vec::new();
    let mut next_color: LaneColor = 0;
    let mut rows = Vec::new();
    let mut width = 0;

    for (id, parents) in nodes {
        let hits: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, lane)| matches!(lane, Some((k, _)) if *k == id).then_some(i))
            .collect();

        // The node takes the leftmost lane waiting for it, or opens a new one
        // (a branch tip nobody pointed at yet).
        let (lane, color) = match hits.first() {
            Some(&first) => (first, lanes[first].expect("hit lane is occupied").1),
            None => {
                let lane = take_free_lane(&mut lanes);
                let color = next_color;
                next_color += 1;
                (lane, color)
            }
        };

        let mut edges = Vec::new();
        for (i, slot) in lanes.iter().enumerate() {
            if let Some((_, c)) = slot {
                let kind = if hits.contains(&i) {
                    EdgeKind::In
                } else {
                    EdgeKind::Pass
                };
                edges.push(Edge {
                    kind,
                    lane: i,
                    color: *c,
                });
            }
        }

        // Every lane that was waiting for this commit ends here.
        for &i in &hits {
            lanes[i] = None;
        }
        if lane >= lanes.len() {
            lanes.resize(lane + 1, None);
        }

        let mut seen: Vec<K> = Vec::with_capacity(parents.len());
        for (n, &parent) in parents.iter().enumerate() {
            if seen.contains(&parent) {
                continue; // malformed commits can list a parent twice
            }
            seen.push(parent);

            if n == 0 {
                // The first parent continues straight down in the node's lane.
                // If another lane already waits for it, the two lanes merge
                // when the parent row is reached.
                lanes[lane] = Some((parent, color));
                edges.push(Edge {
                    kind: EdgeKind::Out,
                    lane,
                    color,
                });
            } else if let Some(j) = lanes
                .iter()
                .position(|slot| matches!(slot, Some((k, _)) if *k == parent))
            {
                // Merge parent already has a lane: join it.
                let c = lanes[j].expect("lane is occupied").1;
                edges.push(Edge {
                    kind: EdgeKind::Out,
                    lane: j,
                    color: c,
                });
            } else {
                let j = take_free_lane(&mut lanes);
                let c = next_color;
                next_color += 1;
                lanes[j] = Some((parent, c));
                edges.push(Edge {
                    kind: EdgeKind::Out,
                    lane: j,
                    color: c,
                });
            }
        }

        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }

        let row_width = edges
            .iter()
            .map(|e| e.lane + 1)
            .chain(std::iter::once(lane + 1))
            .max()
            .unwrap_or(1);
        width = width.max(row_width);
        rows.push(GraphRow { lane, color, edges });
    }

    GraphLayout { rows, width }
}

fn take_free_lane<K>(lanes: &mut Vec<Option<K>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(nodes: &[(u32, Vec<u32>)]) -> GraphLayout {
        layout(nodes.iter().map(|(id, parents)| (*id, parents.as_slice())))
    }

    fn kinds(row: &GraphRow) -> Vec<(EdgeKind, usize)> {
        row.edges.iter().map(|e| (e.kind, e.lane)).collect()
    }

    use EdgeKind::{In, Out, Pass};

    #[test]
    fn linear_history_stays_in_one_lane() {
        let g = run(&[(3, vec![2]), (2, vec![1]), (1, vec![])]);
        assert_eq!(g.width, 1);
        assert!(g.rows.iter().all(|r| r.lane == 0));
        assert_eq!(kinds(&g.rows[0]), vec![(Out, 0)]);
        assert_eq!(kinds(&g.rows[1]), vec![(In, 0), (Out, 0)]);
        assert_eq!(kinds(&g.rows[2]), vec![(In, 0)]);
    }

    #[test]
    fn branch_and_merge() {
        // A <- B <- C <- M, B <- D <- M (M merges C and D)
        let g = run(&[
            (5, vec![3, 4]), // M
            (3, vec![2]),    // C
            (4, vec![2]),    // D
            (2, vec![1]),    // B
            (1, vec![]),     // A
        ]);
        let lanes: Vec<usize> = g.rows.iter().map(|r| r.lane).collect();
        assert_eq!(lanes, vec![0, 0, 1, 0, 0]);
        assert_eq!(g.width, 2);
        assert_eq!(kinds(&g.rows[0]), vec![(Out, 0), (Out, 1)]);
        assert_eq!(kinds(&g.rows[1]), vec![(In, 0), (Pass, 1), (Out, 0)]);
        assert_eq!(kinds(&g.rows[2]), vec![(Pass, 0), (In, 1), (Out, 1)]);
        // Both lanes converge into B, lane 1 is freed afterwards.
        assert_eq!(kinds(&g.rows[3]), vec![(In, 0), (In, 1), (Out, 0)]);
        assert_eq!(kinds(&g.rows[4]), vec![(In, 0)]);
    }

    #[test]
    fn two_tips_sharing_a_parent_converge_at_the_parent() {
        let g = run(&[(4, vec![2]), (3, vec![2]), (2, vec![1]), (1, vec![])]);
        let lanes: Vec<usize> = g.rows.iter().map(|r| r.lane).collect();
        assert_eq!(lanes, vec![0, 1, 0, 0]);
        assert_eq!(kinds(&g.rows[2]), vec![(In, 0), (In, 1), (Out, 0)]);
        // Different tips get different colors.
        assert_ne!(g.rows[0].color, g.rows[1].color);
    }

    #[test]
    fn merge_parent_joins_an_existing_lane() {
        // X -> P, M -> (Q, P), Q -> P
        let g = run(&[(10, vec![1]), (20, vec![30, 1]), (30, vec![1]), (1, vec![])]);
        assert_eq!(g.rows[1].lane, 1);
        // M's second parent goes to lane 0, which already waits for P.
        assert_eq!(kinds(&g.rows[1]), vec![(Pass, 0), (Out, 1), (Out, 0)]);
        assert_eq!(g.width, 2);
        assert_eq!(kinds(&g.rows[3]), vec![(In, 0), (In, 1)]);
    }

    #[test]
    fn freed_lanes_are_reused() {
        // Two short-lived side branches should share column 1.
        let g = run(&[
            (6, vec![5, 4]),
            (4, vec![3]),
            (5, vec![3]),
            (3, vec![2, 1]),
            (1, vec![0]),
            (2, vec![0]),
            (0, vec![]),
        ]);
        assert_eq!(g.width, 2);
        assert_eq!(g.rows[1].lane, 1);
        assert_eq!(g.rows[4].lane, 1);
    }

    #[test]
    fn missing_parents_keep_their_lane_open() {
        // History truncated: parent 99 never shows up.
        let g = run(&[(2, vec![99]), (1, vec![])]);
        assert_eq!(g.rows[1].lane, 1);
        assert_eq!(kinds(&g.rows[1]), vec![(Pass, 0)]);
    }
}
