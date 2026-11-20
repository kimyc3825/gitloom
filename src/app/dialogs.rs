//! Modal dialogs: create a branch, confirm a destructive operation, stash.

use eframe::egui::{self, Color32, Key, RichText};

use super::{Action, GitloomApp};
use crate::git::ops::GitOp;

const DANGER: Color32 = Color32::from_rgb(200, 55, 55);

impl GitloomApp {
    pub(super) fn dialogs_ui(&mut self, ctx: &egui::Context) {
        self.branch_dialog_ui(ctx);
        self.rebase_dialog_ui(ctx);
        self.confirm_ui(ctx);
        self.stash_dialog_ui(ctx);
    }

    fn branch_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.branch_dialog else {
            return;
        };
        let mut close = false;
        let mut create = None;
        let modal = egui::Modal::new(egui::Id::unique("create_branch")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading("Create branch");
            ui.weak(format!("at {}", &dialog.at.to_string()[..7]));
            let edit = ui.add(
                egui::TextEdit::singleline(&mut dialog.name)
                    .hint_text("feature/my-change")
                    .desired_width(f32::INFINITY),
            );
            if dialog.focus {
                edit.request_focus();
                dialog.focus = false;
            }
            let valid = !dialog.name.trim().is_empty();
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.horizontal(|ui| {
                let clicked = ui
                    .add_enabled(valid, egui::Button::new("Create and switch"))
                    .clicked();
                if valid && (clicked || enter) {
                    create = Some((dialog.name.trim().to_owned(), dialog.at));
                }
                close |= ui.button("Cancel").clicked();
            });
        });
        close |= modal.should_close();
        if let Some((name, at)) = create {
            self.actions
                .push(Action::Op(GitOp::CreateBranch { name, at }));
            close = true;
        }
        if close {
            self.branch_dialog = None;
        }
    }

    fn confirm_ui(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &mut self.confirm else {
            return;
        };
        let mut close = false;
        let mut run = None;
        let modal = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(&confirm.title);
            ui.add_space(4.0);
            ui.add(egui::Label::new(&confirm.message).wrap());
            if let Some(toggle) = &mut confirm.toggle {
                ui.add_space(4.0);
                ui.checkbox(&mut toggle.checked, toggle.label.as_str());
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let button =
                    egui::Button::new(RichText::new(&confirm.confirm_label).color(Color32::WHITE))
                        .fill(DANGER);
                if ui.add(button).clicked() {
                    run = Some(match &confirm.toggle {
                        Some(toggle) if toggle.checked => toggle.op.clone(),
                        _ => confirm.op.clone(),
                    });
                }
                close |= ui.button("Cancel").clicked();
            });
        });
        close |= modal.should_close();
        if let Some(op) = run {
            self.actions.push(Action::Op(op));
            close = true;
        }
        if close {
            self.confirm = None;
        }
    }

    fn stash_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.stash_dialog else {
            return;
        };
        let mut close = false;
        let mut stash = None;
        let modal = egui::Modal::new(egui::Id::unique("stash_dialog")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Stash changes");
            ui.weak("Saves your changes and cleans the working tree.");
            ui.add_space(4.0);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut dialog.message)
                    .hint_text("Message (optional)")
                    .desired_width(f32::INFINITY),
            );
            if dialog.focus {
                edit.request_focus();
                dialog.focus = false;
            }
            ui.checkbox(&mut dialog.include_untracked, "Include untracked files");
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Stash").clicked() || enter {
                    stash = Some(GitOp::StashPush {
                        message: dialog.message.clone(),
                        include_untracked: dialog.include_untracked,
                    });
                }
                close |= ui.button("Cancel").clicked();
            });
        });
        close |= modal.should_close();
        if let Some(op) = stash {
            self.actions.push(Action::Op(op));
            close = true;
        }
        if close {
            self.stash_dialog = None;
        }
    }
}
