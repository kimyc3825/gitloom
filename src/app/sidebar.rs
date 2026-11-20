//! Left sidebar: branches, remote branches and tags.

use eframe::egui::{self, Color32, RichText, ScrollArea, Ui};

use super::{Action, ForgeState, GitloomApp};
use crate::git::ops::GitOp;
use crate::git::repo::{RefInfo, RefKind};
use crate::ui::{Chip, diff};

impl GitloomApp {
    pub(super) fn sidebar(&mut self, ui: &mut Ui) {
        let Some(snap) = &self.snapshot else {
            diff::placeholder(ui, self.snapshot_loading, "");
            return;
        };
        let pulls = match &self.forge {
            ForgeState::Loaded(forge) => forge.pulls.as_slice(),
            _ => &[],
        };
        let mut actions = Vec::new();

        ScrollArea::vertical()
            .id_salt("sidebar")
            .auto_shrink(false)
            .show(ui, |ui| {
                if let (true, Some(target)) = (snap.head.detached, snap.head.target) {
                    let text =
                        RichText::new(format!("HEAD (detached at {})", &target.to_string()[..7]))
                            .color(Color32::from_rgb(239, 83, 80));
                    if ui
                        .selectable_label(self.selected == Some(target), text)
                        .clicked()
                    {
                        actions.push(Action::SelectCommit {
                            id: target,
                            center: true,
                        });
                    }
                    ui.add_space(4.0);
                }

                for (kind, title, open) in [
                    (RefKind::LocalBranch, "Branches", true),
                    (RefKind::RemoteBranch, "Remotes", true),
                    (RefKind::Tag, "Tags", false),
                ] {
                    let refs: Vec<(usize, &RefInfo)> = snap
                        .refs
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.kind == kind)
                        .collect();
                    egui::CollapsingHeader::new(
                        RichText::new(format!("{title} ({})", refs.len())).strong(),
                    )
                    .id_salt(title)
                    .default_open(open)
                    .show(ui, |ui| {
                        if refs.is_empty() {
                            ui.weak("none");
                        }
                        for (ri, r) in refs {
                            ui.horizontal(|ui| {
                                let is_selected = self.selected == Some(r.target);
                                let mut text = RichText::new(&r.name);
                                if r.is_head {
                                    text = text.strong();
                                    // Green reads poorly on the selection highlight.
                                    if !is_selected {
                                        text = text.color(Color32::from_rgb(46, 160, 67));
                                    }
                                }
                                let response = ui.selectable_label(is_selected, text);
                                let response = if r.is_head {
                                    response.on_hover_text("Checked out")
                                } else if r.kind == RefKind::LocalBranch {
                                    response.on_hover_text("Double-click to switch")
                                } else {
                                    response
                                };
                                if let Some((ahead, behind)) =
                                    r.ahead_behind.filter(|&(a, b)| a + b > 0)
                                {
                                    ui.weak(format!("↑{ahead} ↓{behind}"))
                                        .on_hover_text(format!(
                                            "vs {}",
                                            r.upstream.as_deref().unwrap_or("upstream")
                                        ));
                                }
                                if let Some(pr) =
                                    self.pr_for_ref.get(&ri).and_then(|&pi| pulls.get(pi))
                                    && Chip::for_pr(pr).show(ui).on_hover_text(&pr.title).clicked()
                                {
                                    actions.push(Action::SelectPr(pr.number));
                                }

                                if response.double_clicked()
                                    && kind == RefKind::LocalBranch
                                    && !r.is_head
                                {
                                    actions.push(Action::Op(GitOp::Switch(r.name.clone())));
                                } else if response.clicked() {
                                    actions.push(Action::SelectCommit {
                                        id: r.target,
                                        center: true,
                                    });
                                }
                                response.context_menu(|ui| ref_menu(ui, r, &mut actions));
                            });
                        }
                    });
                }
            });
        self.actions.extend(actions);
    }
}

fn ref_menu(ui: &mut Ui, r: &RefInfo, actions: &mut Vec<Action>) {
    match r.kind {
        RefKind::LocalBranch
            if !r.is_head && ui.button(format!("Switch to {}", r.name)).clicked() =>
        {
            actions.push(Action::Op(GitOp::Switch(r.name.clone())));
            ui.close();
        }
        RefKind::RemoteBranch if ui.button("Check out as a local branch").clicked() => {
            actions.push(Action::Op(GitOp::TrackRemote(r.name.clone())));
            ui.close();
        }
        _ => {}
    }
    if ui.button("Create branch from here…").clicked() {
        actions.push(Action::OpenBranchDialog(r.target));
        ui.close();
    }
    if ui.button("Copy name").clicked() {
        ui.ctx().copy_text(r.name.clone());
        ui.close();
    }
}
