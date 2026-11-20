//! File view: one file's history (following renames) and its blame.

use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Frame, Rect, RichText, ScrollArea, Sense, Stroke,
    Ui, pos2, vec2,
};
use git2::Oid;

use super::{Action, GitloomApp, View, plural};
use crate::git::blame::{self, Blame, BlameCommit, FileCommit};
use crate::git::repo::{self, FileDiff};
use crate::jobs::JobResult;
use crate::ui::diff;
use crate::util;

const ENTRY_HEIGHT: f32 = 38.0;
const LINE_HEIGHT: f32 = 18.0;
const GUTTER: f32 = 240.0;
const OLD: Color32 = Color32::from_rgb(70, 84, 110);
const NEW: Color32 = Color32::from_rgb(255, 152, 0);
const UNCOMMITTED: Color32 = Color32::from_rgb(102, 187, 106);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FileMode {
    History,
    Blame,
}

pub(super) struct FileView {
    /// The file as it is named now; its history follows renames.
    pub path: String,
    pub mode: FileMode,
    pub history: Option<Result<(Vec<FileCommit>, bool), String>>,
    pub history_selected: Option<usize>,
    pub entry_diff: Option<Result<FileDiff, String>>,
    /// Path and revision being blamed; `None` = the working tree.
    pub blame_target: (String, Option<Oid>),
    pub blame: Option<Result<Blame, String>>,
    /// Commit (index into `blame.commits`) whose lines are highlighted.
    pub blame_commit: Option<usize>,
}

/// Requests from the file view's UI.
pub(super) enum FileAction {
    Mode(FileMode),
    SelectEntry(usize),
    Blame { path: String, rev: Option<Oid> },
    SelectBlameCommit(Option<usize>),
    Close,
}

impl GitloomApp {
    pub(super) fn open_file(&mut self, path: String, mode: FileMode, rev: Option<Oid>) {
        self.file_view = Some(FileView {
            path: path.clone(),
            mode,
            history: None,
            history_selected: None,
            entry_diff: None,
            blame_target: (path, rev),
            blame: None,
            blame_commit: None,
        });
        self.view = View::File;
        self.load_file_data();
    }

    /// Start loading what the current mode shows, unless it is loaded already.
    fn load_file_data(&mut self) {
        let (Some(root), Some(fv)) = (self.root.clone(), &self.file_view) else {
            return;
        };
        match fv.mode {
            FileMode::History if fv.history.is_none() => {
                let path = fv.path.clone();
                self.jobs.spawn(move || {
                    let result = blame::file_history(&root, &path);
                    JobResult::FileHistory { path, result }
                });
            }
            FileMode::Blame if fv.blame.is_none() => {
                let (path, rev) = fv.blame_target.clone();
                self.jobs.spawn(move || {
                    let result = blame::blame(&root, &path, rev);
                    JobResult::Blame { path, rev, result }
                });
            }
            _ => {}
        }
    }

    pub(super) fn handle_file_action(&mut self, action: FileAction) {
        match action {
            FileAction::Mode(mode) => {
                if let Some(fv) = &mut self.file_view {
                    fv.mode = mode;
                }
                self.load_file_data();
            }
            FileAction::SelectEntry(index) => self.select_file_entry(index),
            FileAction::Blame { path, rev } => {
                if let Some(fv) = &mut self.file_view {
                    fv.mode = FileMode::Blame;
                    fv.blame_target = (path, rev);
                    fv.blame = None;
                    fv.blame_commit = None;
                }
                self.load_file_data();
            }
            FileAction::SelectBlameCommit(commit) => {
                if let Some(fv) = &mut self.file_view {
                    fv.blame_commit = commit;
                }
            }
            FileAction::Close => {
                self.file_view = None;
                if self.view == View::File {
                    self.view = View::History;
                }
            }
        }
    }

    fn select_file_entry(&mut self, index: usize) {
        let (Some(root), Some(fv)) = (self.root.clone(), &mut self.file_view) else {
            return;
        };
        let Some(Ok((entries, _))) = &fv.history else {
            return;
        };
        let Some(entry) = entries.get(index) else {
            return;
        };
        let (id, path) = (entry.id, entry.path.clone());
        fv.history_selected = Some(index);
        fv.entry_diff = None;
        self.jobs.spawn(move || {
            let result = repo::load_commit_file_diff(&root, id, &path);
            JobResult::FileCommitDiff { id, result }
        });
    }

    pub(super) fn apply_file_history(
        &mut self,
        path: String,
        result: anyhow::Result<(Vec<FileCommit>, bool)>,
    ) {
        let Some(fv) = self.file_view.as_mut().filter(|fv| fv.path == path) else {
            return;
        };
        let has_entries = result.as_ref().is_ok_and(|(e, _)| !e.is_empty());
        fv.history = Some(result.map_err(|e| format!("{e:#}")));
        fv.history_selected = None;
        if has_entries {
            self.select_file_entry(0);
        }
    }

    pub(super) fn apply_blame(
        &mut self,
        path: String,
        rev: Option<Oid>,
        result: anyhow::Result<Blame>,
    ) {
        if let Some(fv) = self
            .file_view
            .as_mut()
            .filter(|fv| fv.blame_target == (path, rev))
        {
            fv.blame = Some(result.map_err(|e| format!("{e:#}")));
        }
    }

    pub(super) fn apply_file_commit_diff(&mut self, id: Oid, result: anyhow::Result<FileDiff>) {
        let Some(fv) = &mut self.file_view else {
            return;
        };
        let selected = match (&fv.history, fv.history_selected) {
            (Some(Ok((entries, _))), Some(i)) => entries.get(i).map(|e| e.id),
            _ => None,
        };
        if selected == Some(id) {
            fv.entry_diff = Some(result.map_err(|e| format!("{e:#}")));
        }
    }

    pub(super) fn file_view_ui(&mut self, ui: &mut Ui) {
        let Some(fv) = &self.file_view else {
            return;
        };
        let mut actions = Vec::new();
        ui.horizontal(|ui| {
            let mut mode = fv.mode;
            ui.selectable_value(&mut mode, FileMode::History, "File history");
            ui.selectable_value(&mut mode, FileMode::Blame, "Blame");
            if mode != fv.mode {
                actions.push(Action::File(FileAction::Mode(mode)));
            }
            ui.separator();
            ui.add(egui::Label::new(RichText::new(&fv.path).monospace().strong()).truncate());
        });
        ui.separator();
        let now = util::now_secs();
        match fv.mode {
            FileMode::History => history_ui(ui, fv, now, &mut actions),
            FileMode::Blame => blame_ui(ui, fv, now, &mut actions),
        }
        self.actions.extend(actions);
    }
}

fn error(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.add(egui::Label::new(RichText::new(text).color(Color32::from_rgb(239, 83, 80))).wrap());
}

fn history_ui(ui: &mut Ui, fv: &FileView, now: i64, actions: &mut Vec<Action>) {
    let (entries, truncated) = match &fv.history {
        None => return diff::placeholder(ui, true, ""),
        Some(Err(e)) => return error(ui, e),
        Some(Ok((entries, _))) if entries.is_empty() => {
            ui.weak("No commits touch this file yet.");
            return;
        }
        Some(Ok((entries, truncated))) => (entries, *truncated),
    };
    egui::Panel::left("file_history_list")
        .resizable(true)
        .default_size(380.0)
        .min_size(220.0)
        .show(ui, |ui| {
            let mut count = plural(entries.len(), "commit", "commits");
            if truncated {
                count.push_str(" (the newest only)");
            }
            ui.weak(count);
            history_rows(ui, entries, fv.history_selected, now, actions);
        });
    diff::panel().show(ui, |ui| {
        let selected = fv.history_selected.and_then(|i| entries.get(i));
        match (&fv.entry_diff, selected) {
            (Some(Ok(file)), Some(entry)) => {
                diff::show(ui, file, ("file_history_diff", entry.id), None);
            }
            (Some(Err(e)), _) => error(ui, e),
            (None, Some(_)) => diff::placeholder(ui, true, ""),
            _ => diff::placeholder(ui, false, "Select a commit"),
        }
    });
}

fn history_rows(
    ui: &mut Ui,
    entries: &[FileCommit],
    selected: Option<usize>,
    now: i64,
    actions: &mut Vec<Action>,
) {
    let visuals = ui.visuals().clone();
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical()
            .id_salt("file_history_rows")
            .auto_shrink(false)
            .show_rows(ui, ENTRY_HEIGHT, entries.len(), |ui, range| {
                for i in range {
                    let entry = &entries[i];
                    let (rect, response) = ui.allocate_exact_size(
                        vec2(ui.available_width(), ENTRY_HEIGHT),
                        Sense::click(),
                    );
                    let is_selected = selected == Some(i);
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::ListItem,
                            true,
                            is_selected,
                            &entry.summary,
                        )
                    });
                    let painter = ui.painter_at(rect);
                    if is_selected {
                        painter.rect_filled(rect, 0.0, visuals.selection.bg_fill);
                    } else if response.hovered() {
                        painter.rect_filled(rect, 0.0, visuals.widgets.hovered.weak_bg_fill);
                    }
                    let left = rect.left() + 6.0;
                    painter.text(
                        pos2(left, rect.top() + 11.0),
                        Align2::LEFT_CENTER,
                        &entry.summary,
                        FontId::proportional(13.0),
                        visuals.text_color(),
                    );
                    let mut meta = format!(
                        "{} · {} · {}",
                        entry.short_id,
                        entry.author,
                        util::relative_time(entry.time, now)
                    );
                    match (&entry.old_path, entry.status) {
                        (Some(old), 'C') => meta.push_str(&format!(" · copied from {old}")),
                        (Some(old), _) => meta.push_str(&format!(" · renamed from {old}")),
                        (None, 'A') => meta.push_str(" · added"),
                        (None, 'D') => meta.push_str(" · deleted"),
                        _ => {}
                    }
                    painter.text(
                        pos2(left, rect.top() + 27.0),
                        Align2::LEFT_CENTER,
                        meta,
                        FontId::proportional(11.5),
                        visuals.weak_text_color(),
                    );
                    if response.clicked() {
                        actions.push(Action::File(FileAction::SelectEntry(i)));
                    }
                    response.context_menu(|ui| {
                        if ui.button("Show in graph").clicked() {
                            actions.push(Action::ShowCommit(entry.id));
                            ui.close();
                        }
                        if entry.status != 'D' && ui.button("Blame at this commit").clicked() {
                            actions.push(Action::File(FileAction::Blame {
                                path: entry.path.clone(),
                                rev: Some(entry.id),
                            }));
                            ui.close();
                        }
                        if ui.button("Copy commit SHA").clicked() {
                            ui.ctx().copy_text(entry.id.to_string());
                            ui.close();
                        }
                    });
                }
            });
    });
}

fn blame_ui(ui: &mut Ui, fv: &FileView, now: i64, actions: &mut Vec<Action>) {
    let (path, rev) = &fv.blame_target;
    ui.horizontal(|ui| {
        match rev {
            Some(rev) => ui.weak(format!("{path} at {}", &rev.to_string()[..7])),
            None => ui.weak(format!("{path} in the working tree")),
        };
        if rev.is_some() && ui.small_button("Back to the working tree").clicked() {
            actions.push(Action::File(FileAction::Blame {
                path: fv.path.clone(),
                rev: None,
            }));
        }
    });
    let blame = match &fv.blame {
        None => return diff::placeholder(ui, true, ""),
        Some(Err(e)) => return error(ui, e),
        Some(Ok(blame)) => blame,
    };
    if let Some(commit) = fv.blame_commit.and_then(|c| blame.commits.get(c)) {
        commit_bar(ui, commit, now, actions);
    }
    if blame.lines.is_empty() {
        ui.weak("The file is empty.");
        return;
    }
    blame_rows(ui, blame, fv.blame_commit, now, actions);
}

/// Details of the highlighted commit, with what you can do from here.
fn commit_bar(ui: &mut Ui, commit: &BlameCommit, now: i64, actions: &mut Vec<Action>) {
    Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(4.0)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                if commit.is_uncommitted() {
                    ui.strong("Not committed yet");
                } else {
                    ui.label(RichText::new(&commit.short_id).monospace());
                    ui.strong(&commit.summary);
                    ui.weak(format!(
                        "{} · {}",
                        commit.author,
                        util::relative_time(commit.time, now)
                    ));
                    if ui.small_button("Show in graph").clicked() {
                        actions.push(Action::ShowCommit(commit.id));
                    }
                }
                if let Some((previous, path)) = &commit.previous
                    && ui
                        .small_button("Blame before this change")
                        .on_hover_text("Blame the file as it was before this commit")
                        .clicked()
                {
                    actions.push(Action::File(FileAction::Blame {
                        path: path.clone(),
                        rev: Some(*previous),
                    }));
                }
                if ui.small_button("×").on_hover_text("Clear").clicked() {
                    actions.push(Action::File(FileAction::SelectBlameCommit(None)));
                }
            });
        });
}

fn age_color(commit: &BlameCommit, oldest: i64, newest: i64) -> Color32 {
    if commit.is_uncommitted() {
        return UNCOMMITTED;
    }
    let t = if newest > oldest {
        (commit.time - oldest) as f32 / (newest - oldest) as f32
    } else {
        1.0
    };
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgb(
        mix(OLD.r(), NEW.r()),
        mix(OLD.g(), NEW.g()),
        mix(OLD.b(), NEW.b()),
    )
}

fn blame_rows(
    ui: &mut Ui,
    blame: &Blame,
    highlighted: Option<usize>,
    now: i64,
    actions: &mut Vec<Action>,
) {
    let visuals = ui.visuals().clone();
    let font = FontId::monospace(12.0);
    let char_w = ui
        .painter()
        .layout_no_wrap("0".repeat(10), font.clone(), visuals.text_color())
        .size()
        .x
        / 10.0;
    let digits = blame.lines.len().to_string().len() as f32;
    let number_w = char_w * digits + 12.0;
    let content_w = GUTTER + number_w + char_w * (blame.max_cols as f32 + 2.0);
    let committed = || blame.commits.iter().filter(|c| !c.is_uncommitted());
    let oldest = committed().map(|c| c.time).min().unwrap_or(0);
    let newest = committed().map(|c| c.time).max().unwrap_or(0);

    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::both()
            .id_salt(("blame", &blame.path, blame.rev))
            .auto_shrink(false)
            .show_rows(ui, LINE_HEIGHT, blame.lines.len(), |ui, range| {
                let width = content_w.max(ui.available_width());
                for i in range {
                    let line = &blame.lines[i];
                    let commit = &blame.commits[line.commit];
                    let first = i == 0 || blame.lines[i - 1].commit != line.commit;
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(width, LINE_HEIGHT), Sense::click());
                    let painter = ui.painter_at(rect);
                    let is_highlighted = highlighted == Some(line.commit);
                    if is_highlighted {
                        painter.rect_filled(
                            rect,
                            0.0,
                            visuals.selection.bg_fill.gamma_multiply(0.45),
                        );
                    } else if response.hovered() {
                        painter.rect_filled(
                            rect,
                            0.0,
                            visuals.widgets.hovered.weak_bg_fill.gamma_multiply(0.6),
                        );
                    }
                    if first && i > 0 {
                        painter.hline(
                            rect.left()..=rect.left() + GUTTER,
                            rect.top(),
                            Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
                        );
                    }
                    let stripe = Rect::from_min_size(rect.min, vec2(3.0, LINE_HEIGHT));
                    painter.rect_filled(stripe, 0.0, age_color(commit, oldest, newest));
                    let y = rect.center().y;
                    if first {
                        let text = if commit.is_uncommitted() {
                            "Not committed yet".to_owned()
                        } else {
                            format!(
                                "{}  {}  {}",
                                commit.short_id,
                                util::relative_time(commit.time, now),
                                commit.author
                            )
                        };
                        painter
                            .with_clip_rect(Rect::from_x_y_ranges(
                                rect.left()..=rect.left() + GUTTER - 8.0,
                                rect.y_range(),
                            ))
                            .text(
                                pos2(rect.left() + 9.0, y),
                                Align2::LEFT_CENTER,
                                text,
                                FontId::proportional(11.5),
                                visuals.weak_text_color(),
                            );
                    }
                    painter.text(
                        pos2(rect.left() + GUTTER + number_w - 6.0, y),
                        Align2::RIGHT_CENTER,
                        i + 1,
                        font.clone(),
                        visuals.weak_text_color(),
                    );
                    painter.text(
                        pos2(rect.left() + GUTTER + number_w + 4.0, y),
                        Align2::LEFT_CENTER,
                        &line.text,
                        font.clone(),
                        visuals.text_color(),
                    );

                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::ListItem,
                            true,
                            is_highlighted,
                            &line.text,
                        )
                    });
                    let response = response
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_ui(|ui| {
                            if commit.is_uncommitted() {
                                ui.label("Not committed yet");
                            } else {
                                ui.strong(&commit.summary);
                                ui.weak(format!(
                                    "{} · {} · {}",
                                    commit.short_id,
                                    commit.author,
                                    util::relative_time(commit.time, now)
                                ));
                            }
                        });
                    if response.double_clicked() && !commit.is_uncommitted() {
                        actions.push(Action::ShowCommit(commit.id));
                    } else if response.clicked() {
                        let next = (!is_highlighted).then_some(line.commit);
                        actions.push(Action::File(FileAction::SelectBlameCommit(next)));
                    }
                    response.context_menu(|ui| {
                        if !commit.is_uncommitted() && ui.button("Show commit in graph").clicked() {
                            actions.push(Action::ShowCommit(commit.id));
                            ui.close();
                        }
                        if let Some((previous, path)) = &commit.previous
                            && ui.button("Blame before this change").clicked()
                        {
                            actions.push(Action::File(FileAction::Blame {
                                path: path.clone(),
                                rev: Some(*previous),
                            }));
                            ui.close();
                        }
                        if !commit.is_uncommitted() && ui.button("Copy commit SHA").clicked() {
                            ui.ctx().copy_text(commit.id.to_string());
                            ui.close();
                        }
                    });
                }
            });
    });
}
