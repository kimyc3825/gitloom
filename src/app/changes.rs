//! Changes view: unstaged/staged files, the commit box, and the file diff.

use eframe::egui::{self, Align, Key, Layout, Modifiers, RichText, ScrollArea, Ui};

use super::{Action, GitloomApp};
use crate::git::ops::GitOp;
use crate::git::repo::StatusFile;
use crate::ui::{self, diff};

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

        diff::panel().show(ui, |ui| match (&self.selected_change, &self.change_diff) {
            (Some(_), Some(file)) => diff::show(ui, file, "change_diff"),
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
    }

    fn change_lists(&mut self, ui: &mut Ui) {
        let Some(status) = &self.status else {
            diff::placeholder(ui, true, "");
            return;
        };
        let selected = self.selected_change.as_ref();
        let mut actions = Vec::new();
        ScrollArea::vertical()
            .id_salt("change_lists")
            .auto_shrink(false)
            .show(ui, |ui| {
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
                    .color(ui::status_color(file.status)),
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
        });
    }
}
