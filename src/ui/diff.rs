//! Unified diff viewer with line numbers. Rows are virtualized, so large
//! diffs only cost what is on screen.

use eframe::egui::AsIdSalt;

use eframe::egui::{self, Align2, Color32, FontId, RichText, ScrollArea, Sense, Ui, pos2, vec2};

use crate::git::repo::{FileDiff, LineKind};

const ROW_HEIGHT: f32 = 17.0;

struct Palette {
    added_bg: Color32,
    removed_bg: Color32,
    hunk_bg: Color32,
    hunk_text: Color32,
    text: Color32,
    weak: Color32,
}

impl Palette {
    fn new(ui: &Ui) -> Self {
        let dark = ui.visuals().dark_mode;
        Self {
            added_bg: if dark {
                Color32::from_rgba_unmultiplied(46, 160, 67, 45)
            } else {
                Color32::from_rgba_unmultiplied(46, 160, 67, 40)
            },
            removed_bg: if dark {
                Color32::from_rgba_unmultiplied(248, 81, 73, 45)
            } else {
                Color32::from_rgba_unmultiplied(248, 81, 73, 40)
            },
            hunk_bg: Color32::from_rgba_unmultiplied(56, 139, 253, 30),
            hunk_text: if dark {
                Color32::from_rgb(121, 184, 255)
            } else {
                Color32::from_rgb(9, 105, 218)
            },
            text: ui.visuals().text_color(),
            weak: ui.visuals().weak_text_color(),
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

pub fn show(ui: &mut Ui, file: &FileDiff, id_salt: impl AsIdSalt) {
    header(ui, file);
    ui.separator();

    if file.binary {
        ui.weak("Binary file — no text diff.");
        return;
    }
    if file.lines.is_empty() {
        ui.weak(if file.truncated {
            "Diff too large to show."
        } else {
            "No textual changes."
        });
        return;
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
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
                        let painter = ui.painter_at(rect);
                        let y = rect.center().y;
                        let Some(line) = file.lines.get(i) else {
                            painter.text(
                                pos2(rect.left() + gutter, y),
                                Align2::LEFT_CENTER,
                                "… diff truncated",
                                font.clone(),
                                palette.weak,
                            );
                            continue;
                        };
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
                        painter.text(
                            pos2(rect.left() + gutter, y),
                            Align2::LEFT_CENTER,
                            format!("{prefix}{}", line.text),
                            font.clone(),
                            color,
                        );
                    }
                },
            );
    });
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
