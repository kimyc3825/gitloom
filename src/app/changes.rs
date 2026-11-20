//! Changes view: unstaged/staged files, the commit box, and the file diff
//! with hunk- and line-level staging.

use std::collections::BTreeSet;

use eframe::egui::{self, Align, Key, Layout, Modifiers, RichText, ScrollArea, Ui};

use super::file_view::FileMode;
use super::{Action, Confirm, GitloomApp, merge};
use crate::git::ops::GitOp;
use crate::git::patch::{self, PatchMode};
use crate::git::repo::StatusFile;
use crate::ui::diff::{self, DiffEvent, LineOps};
use crate::ui::{self as widgets};

impl GitloomApp {
    pub(super) fn changes_view(&mut self, ui: &mut Ui) {
        egui::Panel::left("changes_lists")
            .resizable(true)
            .default_size(360.0)
            .min_size(220.0)
            .show(ui, |ui| {
                egui::Panel::bottom("commit_box").show(ui, |ui| self.commit_box(ui));
                egui::CentralPanel::no_frame().show(ui, |ui| self.change_lists(ui));
            });

        if self.selected_conflict.is_some() {
            diff::panel().show(ui, |ui| self.merge_view(ui));
            return;
        }
        let mut event = None;
        diff::panel().show(ui, |ui| match (&self.selected_change, &self.change_diff) {
            (Some((_, staged)), Some(file)) => {
                let ops = LineOps {
                    selection: &self.diff_selection,
                    staged: *staged,
                };
                event = diff::show(ui, file, "change_diff", Some(ops));
            }
            (Some(_), None) => diff::placeholder(ui, true, ""),
            (None, _) => {
                let clean = self.status.as_ref().is_some_and(|s| s.is_clean());
                diff::placeholder(
                    ui,
                    false,
                    if clean {
                        "Working tree is clean"
                    } else {
                        "Select a file to see its diff"
                    },
                );
            }
        });
        if let Some(event) = event {
            self.actions.push(Action::Diff(event));
        }
    }

    fn change_lists(&mut self, ui: &mut Ui) {
        let Some(status) = &self.status else {
            diff::placeholder(ui, true, "");
            return;
        };
        let selected = self.selected_change.as_ref();
        let mut actions = Vec::new();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let stash = ui
                .add_enabled(!status.is_clean(), egui::Button::new("Stash…"))
                .on_hover_text("Save the changes for later and clean the working tree");
            if stash.clicked() {
                actions.push(Action::OpenStashDialog);
            }
        });
        ui.add_space(4.0);
        let selected_conflict = self.selected_conflict.as_deref();
        ScrollArea::vertical()
            .id_salt("change_lists")
            .auto_shrink(false)
            .show(ui, |ui| {
                if !status.conflicted.is_empty() {
                    merge::conflict_section(
                        ui,
                        &status.conflicted,
                        selected_conflict,
                        &mut actions,
                    );
                    ui.add_space(10.0);
                }
                section(
                    ui,
                    "Unstaged",
                    &status.unstaged,
                    false,
                    selected,
                    &mut actions,
                );
                ui.add_space(10.0);
                section(ui, "Staged", &status.staged, true, selected, &mut actions);
            });
        self.actions.extend(actions);
    }

    fn commit_box(&mut self, ui: &mut Ui) {
        let staged = self.status.as_ref().map_or(0, |s| s.staged.len());
        let target = match self.snapshot.as_ref().map(|s| &s.head) {
            Some(head) if head.detached => "detached HEAD".to_owned(),
            Some(head) => head.branch.clone().unwrap_or_else(|| "HEAD".to_owned()),
            None => "HEAD".to_owned(),
        };

        ui.add_space(6.0);
        ui.label(RichText::new(format!("Commit to {target}")).strong());

        let id = egui::Id::unique("commit_message");
        // Consume Ctrl+Enter before the text box sees it, so it doesn't add a newline.
        let submit = ui.memory(|m| m.has_focus(id))
            && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
        ui.add(
            egui::TextEdit::multiline(&mut self.commit_message)
                .id(id)
                .hint_text("Summary\n\nLonger description (optional)")
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );

        let can_commit = self.amend || (staged > 0 && !self.commit_message.trim().is_empty());
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.amend, "Amend last commit");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if self.amend {
                    "Amend".to_owned()
                } else {
                    format!("Commit {staged} file{}", if staged == 1 { "" } else { "s" })
                };
                let clicked = ui
                    .add_enabled(can_commit, egui::Button::new(RichText::new(label).strong()))
                    .on_hover_text("Ctrl+Enter")
                    .clicked();
                if can_commit && (clicked || submit) {
                    self.actions.push(Action::Op(GitOp::Commit {
                        message: self.commit_message.clone(),
                        amend: self.amend,
                    }));
                }
            });
        });
        ui.add_space(6.0);
    }

    pub(super) fn handle_diff_event(&mut self, event: DiffEvent) {
        let Some(path) = self.selected_change.as_ref().map(|(p, _)| p.clone()) else {
            return;
        };
        match event {
            DiffEvent::ClickLine {
                index,
                extend,
                toggle,
            } => self.click_diff_line(index, extend, toggle),
            DiffEvent::ClearSelection => {
                self.diff_selection.clear();
                self.diff_anchor = None;
            }
            DiffEvent::Hunk { hunk, mode } => {
                let Some(file) = &self.change_diff else {
                    return;
                };
                let lines = patch::hunk_lines(file, hunk);
                self.apply_lines(&lines, mode, format!("hunk in {path}"));
            }
            DiffEvent::Lines(mode) => {
                let lines = std::mem::take(&mut self.diff_selection);
                let n = lines.len();
                let plural = if n == 1 { "" } else { "s" };
                self.apply_lines(&lines, mode, format!("{n} line{plural} in {path}"));
                // Keep the selection until the operation actually ran.
                self.diff_selection = lines;
            }
        }
    }

    /// Click: select one line. Shift: select a range. Ctrl: add/remove a line.
    fn click_diff_line(&mut self, index: usize, extend: bool, toggle: bool) {
        let Some(file) = &self.change_diff else {
            return;
        };
        if let (true, Some(anchor)) = (extend, self.diff_anchor) {
            if !toggle {
                self.diff_selection.clear();
            }
            self.diff_selection
                .extend((anchor.min(index)..=anchor.max(index)).filter(|&i| {
                    file.lines
                        .get(i)
                        .is_some_and(|l| patch::is_selectable(l.kind))
                }));
            return;
        }
        if toggle {
            if !self.diff_selection.remove(&index) {
                self.diff_selection.insert(index);
            }
        } else if self.diff_selection.len() == 1 && self.diff_selection.contains(&index) {
            self.diff_selection.clear();
        } else {
            self.diff_selection.clear();
            self.diff_selection.insert(index);
        }
        self.diff_anchor = Some(index);
    }

    fn apply_lines(&mut self, lines: &BTreeSet<usize>, mode: PatchMode, what: String) {
        let built = match &self.change_diff {
            Some(file) => patch::build(file, lines, mode),
            None => return,
        };
        let patch = match built {
            Ok(patch) => patch,
            Err(e) => {
                self.set_error(format!("{e:#}"));
                return;
            }
        };
        let op = GitOp::ApplyPatch {
            patch,
            mode,
            what: what.clone(),
        };
        if mode == PatchMode::Discard {
            self.confirm = Some(Confirm {
                title: "Discard changes?".to_owned(),
                message: format!(
                    "Throw away the {what} from the working tree? This can't be undone."
                ),
                confirm_label: "Discard changes".to_owned(),
                op,
                toggle: None,
            });
        } else {
            self.run_op(op);
        }
    }
}

fn section(
    ui: &mut Ui,
    title: &str,
    files: &[StatusFile],
    staged: bool,
    selected: Option<&(String, bool)>,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.strong(format!("{title} ({})", files.len()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !files.is_empty() {
                let (label, op) = if staged {
                    ("Unstage all", GitOp::UnstageAll)
                } else {
                    ("Stage all", GitOp::StageAll)
                };
                if ui.small_button(label).clicked() {
                    actions.push(Action::Op(op));
                }
            }
        });
    });
    if files.is_empty() {
        ui.weak(if staged {
            "Nothing staged"
        } else {
            "No changes"
        });
        return;
    }
    for file in files {
        let toggle = || {
            let paths = vec![file.path.clone()];
            Action::Op(if staged {
                GitOp::Unstage(paths)
            } else {
                GitOp::Stage(paths)
            })
        };
        ui.horizontal(|ui| {
            let (icon, hint) = if staged {
                ("−", "Unstage")
            } else {
                ("+", "Stage")
            };
            if ui.small_button(icon).on_hover_text(hint).clicked() {
                actions.push(toggle());
            }
            ui.label(
                RichText::new(file.status.to_string())
                    .monospace()
                    .color(widgets::status_color(file.status)),
            );
            let is_selected = selected.is_some_and(|(path, s)| *s == staged && *path == file.path);
            let response = ui
                .add(egui::Button::selectable(is_selected, &file.path).truncate())
                .on_hover_text(&file.path);
            if response.double_clicked() {
                actions.push(toggle());
            } else if response.clicked() {
                actions.push(Action::SelectChange {
                    path: file.path.clone(),
                    staged,
                });
            }
            response.context_menu(|ui| file_menu(ui, file, staged, actions));
        });
    }
}

fn file_menu(ui: &mut Ui, file: &StatusFile, staged: bool, actions: &mut Vec<Action>) {
    let paths = || vec![file.path.clone()];
    if staged {
        if ui.button("Unstage").clicked() {
            actions.push(Action::Op(GitOp::Unstage(paths())));
            ui.close();
        }
    } else {
        if ui.button("Stage").clicked() {
            actions.push(Action::Op(GitOp::Stage(paths())));
            ui.close();
        }
        let untracked = file.status == '?';
        let label = if untracked {
            "Delete file…"
        } else {
            "Discard changes…"
        };
        if ui.button(label).clicked() {
            let message = if untracked {
                format!(
                    "Delete the untracked file {}? This can't be undone.",
                    file.path
                )
            } else {
                format!(
                    "Throw away all unstaged changes in {}? This can't be undone.",
                    file.path
                )
            };
            actions.push(Action::confirm(Confirm {
                title: if untracked {
                    "Delete file?"
                } else {
                    "Discard changes?"
                }
                .to_owned(),
                message,
                confirm_label: if untracked {
                    "Delete file"
                } else {
                    "Discard changes"
                }
                .to_owned(),
                op: GitOp::DiscardFile {
                    path: file.path.clone(),
                    untracked,
                },
                toggle: None,
            }));
            ui.close();
        }
    }
    ui.separator();
    if file.status != '?' {
        for (label, mode) in [
            ("File history", FileMode::History),
            ("Blame", FileMode::Blame),
        ] {
            // A deleted file has nothing left in the working tree to blame.
            if mode == FileMode::Blame && file.status == 'D' {
                continue;
            }
            if ui.button(label).clicked() {
                actions.push(Action::OpenFile {
                    path: file.path.clone(),
                    mode,
                    rev: None,
                });
                ui.close();
            }
        }
    }
    if ui.button("Copy path").clicked() {
        ui.ctx().copy_text(file.path.clone());
        ui.close();
    }
}
