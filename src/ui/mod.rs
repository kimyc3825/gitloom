//! Reusable drawing helpers: colors, chips (branch/tag/PR labels), graph rows
//! and the diff viewer.

pub mod diff;
pub mod graph;

use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{
    self, Align2, Color32, FontId, Galley, Painter, Rect, Sense, TextFormat, Ui, pos2, vec2,
};

use crate::forge::{CheckState, PullRequest, ReviewState};
use crate::git::repo::{RefInfo, RefKind};

/// Chips never get wider than this (plus padding); longer names are elided.
const MAX_CHIP_TEXT: f32 = 190.0;

const LANE_COLORS: [Color32; 10] = [
    Color32::from_rgb(66, 165, 245),
    Color32::from_rgb(102, 187, 106),
    Color32::from_rgb(255, 167, 38),
    Color32::from_rgb(171, 71, 188),
    Color32::from_rgb(239, 83, 80),
    Color32::from_rgb(38, 198, 218),
    Color32::from_rgb(212, 225, 87),
    Color32::from_rgb(161, 136, 127),
    Color32::from_rgb(236, 64, 122),
    Color32::from_rgb(92, 107, 192),
];

pub fn lane_color(index: usize) -> Color32 {
    LANE_COLORS[index % LANE_COLORS.len()]
}

pub fn check_color(state: CheckState) -> Color32 {
    match state {
        CheckState::Passed => Color32::from_rgb(76, 175, 80),
        CheckState::Failed => Color32::from_rgb(229, 57, 53),
        CheckState::Pending => Color32::from_rgb(255, 179, 0),
        CheckState::Skipped => Color32::from_rgb(158, 158, 158),
    }
}

pub fn check_label(state: CheckState) -> &'static str {
    match state {
        CheckState::Passed => "passing",
        CheckState::Failed => "failing",
        CheckState::Pending => "pending",
        CheckState::Skipped => "skipped",
    }
}

pub fn review_label(review: ReviewState) -> Option<(&'static str, Color32)> {
    match review {
        ReviewState::None => None,
        ReviewState::Approved => Some(("approved", Color32::from_rgb(76, 175, 80))),
        ReviewState::ChangesRequested => {
            Some(("changes requested", Color32::from_rgb(229, 57, 53)))
        }
        ReviewState::ReviewRequired => Some(("review required", Color32::from_rgb(255, 179, 0))),
    }
}

/// Color of a file status letter (A, M, D, ...).
pub fn status_color(status: char) -> Color32 {
    match status {
        'A' | '?' => Color32::from_rgb(102, 187, 106),
        'M' => Color32::from_rgb(255, 183, 77),
        'D' => Color32::from_rgb(239, 83, 80),
        'R' | 'C' => Color32::from_rgb(79, 195, 247),
        'U' => Color32::from_rgb(255, 82, 82),
        _ => Color32::from_rgb(186, 104, 200),
    }
}

/// `d73a4a` -> color.
pub fn hex_color(hex: &str) -> Option<Color32> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some(Color32::from_rgb(channel(0)?, channel(2)?, channel(4)?))
}

/// Black or white, whichever reads better on `bg`.
pub fn text_on(bg: Color32) -> Color32 {
    let luma = 0.299 * f32::from(bg.r()) + 0.587 * f32::from(bg.g()) + 0.114 * f32::from(bg.b());
    if luma > 150.0 {
        Color32::from_rgb(20, 20, 20)
    } else {
        Color32::WHITE
    }
}

/// A small rounded label: branch, tag, PR number, ...
pub struct Chip {
    pub text: String,
    pub fill: Color32,
    pub text_color: Color32,
    /// Optional status dot drawn before the text (e.g. CI state).
    pub dot: Option<Color32>,
    pub strong: bool,
}

impl Chip {
    pub fn new(text: impl Into<String>, fill: Color32) -> Self {
        Self {
            text: text.into(),
            fill,
            text_color: text_on(fill),
            dot: None,
            strong: false,
        }
    }

    pub fn for_ref(r: &RefInfo) -> Self {
        let (fill, strong) = match (r.kind, r.is_head) {
            (RefKind::LocalBranch, true) => (Color32::from_rgb(46, 160, 67), true),
            (RefKind::LocalBranch, false) => (Color32::from_rgb(35, 110, 50), false),
            (RefKind::RemoteBranch, _) => (Color32::from_rgb(33, 90, 145), false),
            (RefKind::Tag, _) => (Color32::from_rgb(155, 115, 25), false),
        };
        let mut chip = Chip::new(r.name.clone(), fill);
        chip.strong = strong;
        if r.is_head {
            chip.dot = Some(Color32::WHITE);
        }
        chip
    }

    pub fn for_pr(pr: &PullRequest) -> Self {
        let mut chip = Chip::new(format!("#{}", pr.number), Color32::from_rgb(111, 66, 193));
        chip.dot = pr.checks.overall().map(check_color);
        chip
    }

    pub fn detached_head() -> Self {
        let mut chip = Chip::new("HEAD", Color32::from_rgb(190, 60, 60));
        chip.strong = true;
        chip
    }

    fn font(&self) -> FontId {
        FontId::proportional(if self.strong { 12.0 } else { 11.5 })
    }

    /// Single-line text, elided with "…" beyond `MAX_CHIP_TEXT` (long remote branch names).
    fn galley(&self, painter: &Painter) -> Arc<Galley> {
        let mut job = LayoutJob::single_section(
            self.text.clone(),
            TextFormat::simple(self.font(), self.text_color),
        );
        job.wrap = TextWrapping::truncate_at_width(MAX_CHIP_TEXT);
        painter.layout_job(job)
    }

    pub fn size(&self, painter: &Painter) -> egui::Vec2 {
        let dot = if self.dot.is_some() { 10.0 } else { 0.0 };
        vec2(self.galley(painter).size().x + 10.0 + dot, 17.0)
    }

    /// Paint with the left edge at `x`, vertically centered on `y`. Returns the right edge.
    pub fn paint(&self, painter: &Painter, x: f32, y: f32) -> f32 {
        let galley = self.galley(painter);
        let dot = if self.dot.is_some() { 10.0 } else { 0.0 };
        let size = vec2(galley.size().x + 10.0 + dot, 17.0);
        let rect = Rect::from_min_size(pos2(x, y - size.y / 2.0), size);
        painter.rect_filled(rect, 4.0, self.fill);
        let mut text_x = rect.left() + 5.0;
        if let Some(dot) = self.dot {
            painter.circle_filled(pos2(text_x + 3.5, y), 3.5, dot);
            text_x += 10.0;
        }
        let text_pos = Align2::LEFT_CENTER
            .anchor_size(pos2(text_x, y), galley.size())
            .min;
        painter.galley(text_pos, galley, self.text_color);
        rect.right()
    }

    /// Allocate space in a normal layout and paint the chip there (decoration only).
    pub fn show(&self, ui: &mut Ui) -> egui::Response {
        self.show_with_sense(ui, Sense::hover())
    }

    /// Like [`Self::show`], but the chip reacts to clicks.
    pub fn show_clickable(&self, ui: &mut Ui) -> egui::Response {
        self.show_with_sense(ui, Sense::click())
    }

    fn show_with_sense(&self, ui: &mut Ui, sense: Sense) -> egui::Response {
        let size = self.size(ui.painter());
        let (rect, response) = ui.allocate_exact_size(size, sense);
        self.paint(ui.painter(), rect.left(), rect.center().y);
        response
    }
}
