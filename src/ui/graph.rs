//! Painting one row of the commit graph.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, epaint::CubicBezierShape, pos2};

use super::lane_color;
use crate::git::graph::{EdgeKind, GraphLayout, GraphRow};

pub const ROW_HEIGHT: f32 = 22.0;
pub const LANE_WIDTH: f32 = 14.0;
/// Wider graphs are clipped; the description column must stay readable.
const MAX_LANES_SHOWN: usize = 24;

pub fn graph_width(layout: &GraphLayout) -> f32 {
    layout.width.clamp(1, MAX_LANES_SHOWN) as f32 * LANE_WIDTH + 8.0
}

pub struct NodeStyle {
    pub is_merge: bool,
    pub is_head: bool,
    /// Background behind the row, used to hollow out merge nodes.
    pub background: Color32,
    pub head_ring: Color32,
}

/// Paint the lanes and the node of `row` into `rect` (the graph column of the row).
pub fn paint_row(painter: &Painter, rect: Rect, row: &GraphRow, style: &NodeStyle) {
    let x = |lane: usize| rect.left() + 4.0 + LANE_WIDTH * (lane as f32 + 0.5);
    let (top, mid, bottom) = (rect.top(), rect.center().y, rect.bottom());
    let node = pos2(x(row.lane), mid);

    for edge in &row.edges {
        let stroke = Stroke::new(1.6, lane_color(edge.color));
        match edge.kind {
            EdgeKind::Pass => {
                painter.line_segment(
                    [pos2(x(edge.lane), top), pos2(x(edge.lane), bottom)],
                    stroke,
                );
            }
            EdgeKind::In => curve(painter, pos2(x(edge.lane), top), node, stroke),
            EdgeKind::Out => curve(painter, node, pos2(x(edge.lane), bottom), stroke),
        }
    }

    let color = lane_color(row.color);
    if style.is_merge {
        painter.circle_filled(node, 4.0, style.background);
        painter.circle_stroke(node, 3.5, Stroke::new(2.0, color));
    } else {
        painter.circle_filled(node, 4.5, color);
    }
    if style.is_head {
        painter.circle_stroke(node, 7.0, Stroke::new(1.5, style.head_ring));
    }
}

/// Vertical lines stay straight; lane changes become a smooth S-curve.
fn curve(painter: &Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    if (from.x - to.x).abs() < 0.5 {
        painter.line_segment([from, to], stroke);
        return;
    }
    let dy = (to.y - from.y) * 0.6;
    let points = [from, pos2(from.x, from.y + dy), pos2(to.x, to.y - dy), to];
    painter.add(Shape::CubicBezier(CubicBezierShape::from_points_stroke(
        points,
        false,
        Color32::TRANSPARENT,
        stroke,
    )));
}
