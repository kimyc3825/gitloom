//! Unified diff viewer with line numbers. Rows are virtualized, so large
//! diffs only cost what is on screen.
//!
//! For working-tree diffs the viewer also offers partial staging: buttons on
//! every hunk header and line selection (click, Shift+click, Ctrl+click).

use std::collections::BTreeSet;

use eframe::egui::{
    self, Align2, AsIdSalt, Color32, CursorIcon, FontId, Id, Painter, Rect, RichText, ScrollArea,
    Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::git::patch::{self, PatchMode};
use crate::git::repo::{FileDiff, LineKind};

const ROW_HEIGHT: f32 = 18.0;

/// Partial-staging controls for a working-tree or index diff.
pub struct LineOps<'a> {
    pub selection: &'a BTreeSet<usize>,
    /// The diff is HEAD -> index (offer "unstage") rather than index -> working tree.
    pub staged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffEvent {
    /// A selectable line was clicked; `extend` = Shift, `toggle` = Ctrl/Cmd.
    ClickLine {
        index: usize,
        extend: bool,
        toggle: bool,
    },
    /// A hunk header button.
    Hunk {
        hunk: usize,
        mode: PatchMode,
    },
    /// A button of the selection bar.
    Lines(PatchMode),
    ClearSelection,
}

struct Palette {
    added_bg: Color32,
    removed_bg: Color32,
    hunk_bg: Color32,
    hunk_text: Color32,
    selected_bg: Color32,
    selected_bar: Color32,
    hover_bg: Color32,
    text: Color32,
    weak: Color32,
    danger: Color32,
}

impl Palette {
    fn new(ui: &Ui) -> Self {
        let visuals = ui.visuals();
        let dark = visuals.dark_mode;
        Self {
            added_bg: Color32::from_rgba_unmultiplied(46, 160, 67, if dark { 45 } else { 40 }),
            removed_bg: Color32::from_rgba_unmultiplied(248, 81, 73, if dark { 45 } else { 40 }),
            hunk_bg: Color32::from_rgba_unmultiplied(56, 139, 253, 30),
            hunk_text: if dark {
                Color32::from_rgb(121, 184, 255)
            } else {
                Color32::from_rgb(9, 105, 218)
            },
            selected_bg: visuals.selection.bg_fill.gamma_multiply(0.55),
            selected_bar: visuals.selection.stroke.color,
            hover_bg: visuals.widgets.hovered.weak_bg_fill.gamma_multiply(0.5),
            text: visuals.text_color(),
            weak: visuals.weak_text_color(),
            danger: Color32::from_rgb(220, 60, 60),
        }
    }
}

/// File header: status, path and +/- counts.
pub fn header(ui: &mut Ui, file: &FileDiff) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(file.status.to_string())
                .monospace()
                .color(super::status_color(file.status)),
        );
        match &file.old_path {
            Some(old) => ui.label(RichText::new(format!("{old} → {}", file.path)).strong()),
            None => ui.label(RichText::new(&file.path).strong()),
        };
        ui.label(
            RichText::new(format!("+{}", file.additions)).color(Color32::from_rgb(102, 187, 106)),
        );
        ui.label(
            RichText::new(format!("−{}", file.deletions)).color(Color32::from_rgb(239, 83, 80)),
        );
    });
}

/// Show `file`. With `ops`, hunks and lines can be picked for (un)staging;
/// the returned event says what the user asked for.
pub fn show(
    ui: &mut Ui,
    file: &FileDiff,
    id_salt: impl AsIdSalt,
    ops: Option<LineOps<'_>>,
) -> Option<DiffEvent> {
    header(ui, file);
    let mut event = None;
    let ops = match ops {
        Some(ops) if patch::supports_line_ops(file) => {
            selection_bar(ui, &ops, &mut event);
            Some(ops)
        }
        Some(_) if !file.binary && !file.lines.is_empty() => {
            ui.weak("New, deleted and renamed files are staged as a whole.");
            None
        }
        _ => None,
    };
    ui.separator();

    if file.binary {
        ui.weak("Binary file — no text diff.");
        return event;
    }
    if file.lines.is_empty() {
        ui.weak(if file.truncated {
            "Diff too large to show."
        } else {
            "No textual changes."
        });
        return event;
    }

    let palette = Palette::new(ui);
    let font = FontId::monospace(12.0);
    let char_w = ui
        .painter()
        .layout_no_wrap("0".repeat(10), font.clone(), palette.text)
        .size()
        .x
        / 10.0;
    let gutter = char_w * 12.0 + 10.0;
    let content_w = gutter + char_w * (file.max_cols as f32 + 2.0);
    let hunk_buttons: &[(&str, PatchMode)] = match &ops {
        Some(ops) if ops.staged => &[("Unstage hunk", PatchMode::Unstage)],
        Some(_) => &[
            ("Stage hunk", PatchMode::Stage),
            ("Discard", PatchMode::Discard),
        ],
        None => &[],
    };

    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::both()
            .id_salt(id_salt)
            .auto_shrink(false)
            .show_rows(
                ui,
                ROW_HEIGHT,
                file.lines.len() + usize::from(file.truncated),
                |ui, range| {
                    let width = content_w.max(ui.available_width());
                    for i in range {
                        let Some(line) = file.lines.get(i) else {
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
                            ui.painter().text(
                                pos2(rect.left() + gutter, rect.center().y),
                                Align2::LEFT_CENTER,
                                "… diff truncated",
                                font.clone(),
                                palette.weak,
                            );
                            continue;
                        };
                        let selectable = ops.is_some() && patch::is_selectable(line.kind);
                        let sense = if selectable {
                            Sense::click()
                        } else {
                            Sense::hover()
                        };
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(width, ROW_HEIGHT), sense);
                        let painter = ui.painter_at(rect);
                        let y = rect.center().y;
                        let selected = ops.as_ref().is_some_and(|o| o.selection.contains(&i));

                        let (bg, color, prefix) = match line.kind {
                            LineKind::Added => (palette.added_bg, palette.text, "+"),
                            LineKind::Removed => (palette.removed_bg, palette.text, "-"),
                            LineKind::Context => (Color32::TRANSPARENT, palette.text, " "),
                            LineKind::Hunk => (palette.hunk_bg, palette.hunk_text, ""),
                            LineKind::Meta => (Color32::TRANSPARENT, palette.weak, ""),
                        };
                        if bg != Color32::TRANSPARENT {
                            painter.rect_filled(rect, 0.0, bg);
                        }
                        if selected {
                            painter.rect_filled(rect, 0.0, palette.selected_bg);
                            let bar = Rect::from_min_size(rect.min, vec2(3.0, rect.height()));
                            painter.rect_filled(bar, 0.0, palette.selected_bar);
                        } else if selectable && response.hovered() {
                            painter.rect_filled(rect, 0.0, palette.hover_bg);
                        }

                        let mut text_x = rect.left() + gutter;
                        if line.kind == LineKind::Hunk && !hunk_buttons.is_empty() {
                            if let Some(hunk) = file.hunks.iter().position(|h| h.header == i) {
                                let mut x = rect.left() + 4.0;
                                for &(label, mode) in hunk_buttons {
                                    let id = ui.scope_id().with(("hunk_button", hunk, label));
                                    let danger = mode == PatchMode::Discard;
                                    let (clicked, right) = mini_button(
                                        ui, &painter, x, rect, id, label, danger, &palette,
                                    );
                                    if clicked {
                                        event = Some(DiffEvent::Hunk { hunk, mode });
                                    }
                                    x = right + 4.0;
                                }
                                text_x = text_x.max(x + 6.0);
                            }
                        } else {
                            for (number, column) in [(line.old_no, 5.0), (line.new_no, 11.0)] {
                                if let Some(n) = number {
                                    painter.text(
                                        pos2(rect.left() + char_w * column, y),
                                        Align2::RIGHT_CENTER,
                                        n,
                                        font.clone(),
                                        palette.weak,
                                    );
                                }
                            }
                        }
                        painter.text(
                            pos2(text_x, y),
                            Align2::LEFT_CENTER,
                            format!("{prefix}{}", line.text),
                            font.clone(),
                            color,
                        );

                        if selectable {
                            // Screen readers (and UI tests) see each line as a list item.
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::Role::ListItem,
                                    true,
                                    selected,
                                    format!("{prefix}{}", line.text),
                                )
                            });
                            let response = response.on_hover_cursor(CursorIcon::PointingHand);
                            if response.clicked() {
                                let modifiers = ui.input(|input| input.modifiers);
                                event = Some(DiffEvent::ClickLine {
                                    index: i,
                                    extend: modifiers.shift,
                                    toggle: modifiers.command,
                                });
                            }
                        }
                    }
                },
            );
    });
    event
}

fn selection_bar(ui: &mut Ui, ops: &LineOps<'_>, event: &mut Option<DiffEvent>) {
    ui.horizontal(|ui| {
        let n = ops.selection.len();
        if n == 0 {
            ui.weak(if ops.staged {
                "Click lines to unstage just those (Shift: range, Ctrl: add/remove)."
            } else {
                "Click lines to stage or discard just those (Shift: range, Ctrl: add/remove)."
            });
            return;
        }
        let plural = if n == 1 { "" } else { "s" };
        ui.label(RichText::new(format!("{n} line{plural} selected")).strong());
        if ops.staged {
            if ui.button("Unstage lines").clicked() {
                *event = Some(DiffEvent::Lines(PatchMode::Unstage));
            }
        } else {
            if ui.button("Stage lines").clicked() {
                *event = Some(DiffEvent::Lines(PatchMode::Stage));
            }
            if ui.button("Discard lines…").clicked() {
                *event = Some(DiffEvent::Lines(PatchMode::Discard));
            }
        }
        if ui.button("Clear").clicked() {
            *event = Some(DiffEvent::ClearSelection);
        }
    });
}

/// A compact button painted inside a virtualized row (no layout involved).
/// Returns whether it was clicked and its right edge.
#[allow(clippy::too_many_arguments)]
fn mini_button(
    ui: &Ui,
    painter: &Painter,
    x: f32,
    row: Rect,
    id: Id,
    label: &str,
    danger: bool,
    palette: &Palette,
) -> (bool, f32) {
    let font = FontId::proportional(11.0);
    let text_w = painter
        .layout_no_wrap(label.to_owned(), font.clone(), palette.text)
        .size()
        .x;
    let rect = Rect::from_min_size(
        pos2(x, row.top() + 1.0),
        vec2(text_w + 12.0, row.height() - 2.0),
    );
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let visuals = ui.style().interact(&response);
    painter.rect(
        rect,
        3.0,
        visuals.weak_bg_fill,
        Stroke::new(1.0, visuals.bg_stroke.color),
        StrokeKind::Inside,
    );
    let text_color = if danger {
        palette.danger
    } else {
        visuals.text_color()
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        font,
        text_color,
    );
    (response.clicked(), rect.right())
}

/// Central panel for a diff pane: no background, a little air next to the splitter.
pub fn panel() -> egui::CentralPanel {
    egui::CentralPanel::no_frame().frame(egui::Frame::new().inner_margin(egui::Margin {
        left: 8,
        right: 0,
        top: 2,
        bottom: 0,
    }))
}

/// Placeholder shown while a diff is loading or when nothing is selected.
pub fn placeholder(ui: &mut Ui, loading: bool, text: &str) {
    ui.centered_and_justified(|ui| {
        if loading {
            ui.spinner();
        } else {
            ui.weak(text);
        }
    });
}
