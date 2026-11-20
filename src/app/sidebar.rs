//! Left sidebar: branches, remote branches, tags and stashes.

use eframe::egui::{self, Color32, RichText, ScrollArea, Ui};

use super::{Action, Confirm, ConfirmToggle, ForgeState, GitloomApp};
use crate::git::ops::GitOp;
use crate::git::repo::{RefInfo, RefKind, StashInfo};
use crate::ui::{Chip, diff};
use crate::util;

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
        let now = util::now_secs();
        let mut actions = Vec::new();
        let head = snap.head.target.map(|_| {
            snap.head
                .branch
                .clone()
                .unwrap_or_else(|| "HEAD".to_owned())
        });
        let busy = self
            .status
            .as_ref()
            .is_some_and(|s| s.in_progress.is_some());

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
                            let is_selected = self.selected == Some(r.target);
                            let mut text = RichText::new(&r.name);
                            if r.is_head {
                                text = text.strong();
                                // Green reads poorly on the selection highlight.
                                if !is_selected {
                                    text = text.color(Color32::from_rgb(46, 160, 67));
                                }
                            }
                            let pr = self.pr_for_ref.get(&ri).and_then(|&pi| pulls.get(pi));
                            // Badges stay on the right; long branch names get truncated.
                            let (response, pr_clicked) =
                                egui::Sides::new().shrink_left().truncate().show(
                                    ui,
                                    |ui| ui.selectable_label(is_selected, text),
                                    |ui| {
                                        let clicked = pr.is_some_and(|pr| {
                                            Chip::for_pr(pr)
                                                .show_clickable(ui)
                                                .on_hover_text(&pr.title)
                                                .clicked()
                                        });
                                        if let Some((ahead, behind)) =
                                            r.ahead_behind.filter(|&(a, b)| a + b > 0)
                                        {
                                            ui.weak(format!("↑{ahead} ↓{behind}")).on_hover_text(
                                                format!(
                                                    "vs {}",
                                                    r.upstream.as_deref().unwrap_or("upstream")
                                                ),
                                            );
                                        }
                                        clicked
                                    },
                                );
                            let response = if r.is_head {
                                response.on_hover_text(format!("{} (checked out)", r.name))
                            } else if r.kind == RefKind::LocalBranch {
                                response
                                    .on_hover_text(format!("{}\nDouble-click to switch", r.name))
                            } else {
                                response.on_hover_text(&r.name)
                            };

                            if let (true, Some(pr)) = (pr_clicked, pr) {
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
                            response.context_menu(|ui| {
                                ref_menu(ui, r, head.as_deref(), busy, &mut actions)
                            });
                        }
                    });
                }

                if !snap.stashes.is_empty() {
                    egui::CollapsingHeader::new(
                        RichText::new(format!("Stashes ({})", snap.stashes.len())).strong(),
                    )
                    .id_salt("Stashes")
                    .default_open(true)
                    .show(ui, |ui| {
                        for stash in &snap.stashes {
                            let label = format!("stash@{{{}}}  {}", stash.index, stash.message);
                            let response = ui
                                .add(
                                    egui::Button::selectable(
                                        self.selected == Some(stash.id),
                                        label,
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(format!(
                                    "{}\n{} · right-click to apply, pop or drop",
                                    stash.message,
                                    util::relative_time(stash.time, now)
                                ));
                            if response.clicked() {
                                actions.push(Action::ShowStash(stash.id));
                            }
                            response.context_menu(|ui| stash_menu(ui, stash, &mut actions));
                        }
                    });
                }
            });
        self.actions.extend(actions);
    }
}

fn ref_menu(ui: &mut Ui, r: &RefInfo, head: Option<&str>, busy: bool, actions: &mut Vec<Action>) {
    let local = r.kind == RefKind::LocalBranch;
    let remote = r.kind == RefKind::RemoteBranch;
    if let Some(head) = head.filter(|_| (local || remote) && !r.is_head) {
        ui.add_enabled_ui(!busy, |ui| {
            if ui.button(format!("Merge {} into {head}", r.name)).clicked() {
                actions.push(Action::Op(GitOp::Merge(r.name.clone())));
                ui.close();
            }
            if ui
                .button(format!("Rebase {head} onto {}", r.name))
                .clicked()
            {
                actions.push(Action::Op(GitOp::RebaseOnto(r.name.clone())));
                ui.close();
            }
        })
        .response
        .on_disabled_hover_text("Finish or abort the operation in progress first");
        ui.separator();
    }
    if r.is_head
        && let Some(upstream) = &r.upstream
        && ui
            .button("Force push (with lease)…")
            .on_hover_text("Overwrite the remote branch after rewriting history")
            .clicked()
    {
        actions.push(Action::confirm(force_push(r, upstream)));
        ui.close();
    }
    if local && !r.is_head && ui.button(format!("Switch to {}", r.name)).clicked() {
        actions.push(Action::Op(GitOp::Switch(r.name.clone())));
        ui.close();
    }
    if remote && ui.button("Check out as a local branch").clicked() {
        actions.push(Action::Op(GitOp::TrackRemote(r.name.clone())));
        ui.close();
    }
    if ui.button("Create branch from here…").clicked() {
        actions.push(Action::OpenBranchDialog(r.target));
        ui.close();
    }
    if ui.button("Copy name").clicked() {
        ui.ctx().copy_text(r.name.clone());
        ui.close();
    }

    if local && !r.is_head {
        ui.separator();
        if ui.button("Delete branch…").clicked() {
            actions.push(Action::confirm(delete_branch(r)));
            ui.close();
        }
    }
    if remote {
        ui.separator();
        if ui.button("Delete on remote…").clicked() {
            actions.push(Action::confirm(delete_remote_branch(r)));
            ui.close();
        }
    }
}

fn delete_branch(r: &RefInfo) -> Confirm {
    Confirm {
        title: "Delete branch?".to_owned(),
        message: format!(
            "Delete the local branch {}? Commits that only this branch points to will no longer be shown.",
            r.name
        ),
        confirm_label: "Delete branch".to_owned(),
        op: GitOp::DeleteBranch {
            name: r.name.clone(),
            force: false,
        },
        toggle: Some(ConfirmToggle {
            label: "Delete even if it isn't merged (git branch -D)".to_owned(),
            checked: false,
            op: GitOp::DeleteBranch {
                name: r.name.clone(),
                force: true,
            },
        }),
    }
}

fn force_push(r: &RefInfo, upstream: &str) -> Confirm {
    Confirm {
        title: "Force push?".to_owned(),
        message: format!(
            "Replace {upstream} with your local {}? Commits on the remote that aren't in your branch are lost. \
             With --force-with-lease git refuses if someone else pushed in the meantime.",
            r.name
        ),
        confirm_label: "Force push".to_owned(),
        op: GitOp::Push {
            set_upstream: None,
            force: true,
        },
        toggle: None,
    }
}

fn delete_remote_branch(r: &RefInfo) -> Confirm {
    let (remote, branch) = r
        .name
        .split_once('/')
        .unwrap_or(("origin", r.name.as_str()));
    Confirm {
        title: "Delete remote branch?".to_owned(),
        message: format!(
            "Delete {branch} on {remote}? This removes the branch from the remote for everyone."
        ),
        confirm_label: "Delete on remote".to_owned(),
        op: GitOp::DeleteRemoteBranch {
            remote: remote.to_owned(),
            branch: branch.to_owned(),
        },
        toggle: None,
    }
}

fn stash_menu(ui: &mut Ui, stash: &StashInfo, actions: &mut Vec<Action>) {
    if ui.button("Apply").clicked() {
        actions.push(Action::Op(GitOp::StashApply(stash.index)));
        ui.close();
    }
    if ui.button("Pop (apply and drop)").clicked() {
        actions.push(Action::Op(GitOp::StashPop(stash.index)));
        ui.close();
    }
    ui.separator();
    if ui.button("Drop…").clicked() {
        actions.push(Action::confirm(Confirm {
            title: "Drop stash?".to_owned(),
            message: format!(
                "Delete stash@{{{}}} ({})? This can't be undone.",
                stash.index, stash.message
            ),
            confirm_label: "Drop stash".to_owned(),
            op: GitOp::StashDrop(stash.index),
            toggle: None,
        }));
        ui.close();
    }
}
