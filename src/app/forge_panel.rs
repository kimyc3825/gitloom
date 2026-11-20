//! Right panel: pull requests (with CI and review state) and issues.

use std::collections::HashSet;

use eframe::egui::{
    self, Align, Button, Color32, Frame, Layout, OpenUrl, RichText, ScrollArea, Sense, Ui, vec2,
};

use super::{Action, ForgeState, ForgeTab, GitloomApp};
use crate::forge::{CheckState, Issue, PullRequest};
use crate::ui::{self, Chip, diff};
use crate::util;

impl GitloomApp {
    pub(super) fn forge_panel(&mut self, ui: &mut Ui) {
        let mut actions = Vec::new();

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong("GitHub");
            if let ForgeState::Loaded(forge) = &self.forge {
                ui.hyperlink_to(&forge.repo, &forge.repo_url);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.forge_loading {
                    ui.spinner();
                } else if ui.small_button("Reload").clicked() {
                    actions.push(Action::RefreshForge);
                }
            });
        });
        ui.separator();

        match &self.forge {
            ForgeState::Idle => diff::placeholder(ui, self.forge_loading, ""),
            ForgeState::Failed(err) => {
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(err.to_string()).color(Color32::from_rgb(255, 183, 77)),
                    )
                    .wrap(),
                );
                ui.add_space(8.0);
                if ui.button("Retry").clicked() {
                    actions.push(Action::RefreshForge);
                }
            }
            ForgeState::Loaded(forge) => {
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.forge_tab,
                        ForgeTab::Pulls,
                        format!("Pull requests ({})", forge.pulls.len()),
                    );
                    ui.selectable_value(
                        &mut self.forge_tab,
                        ForgeTab::Issues,
                        format!("Issues ({})", forge.issues.len()),
                    );
                });
                ui.separator();

                let now = util::now_secs();
                match self.forge_tab {
                    ForgeTab::Pulls => {
                        let linked: HashSet<u64> = self
                            .pr_for_ref
                            .values()
                            .filter_map(|&pi| forge.pulls.get(pi))
                            .map(|pr| pr.number)
                            .collect();
                        let selected = self
                            .selected_pr
                            .and_then(|n| forge.pulls.iter().find(|p| p.number == n));
                        if let Some(pr) = selected {
                            egui::Panel::bottom("pr_detail")
                                .resizable(true)
                                .default_size(280.0)
                                .min_size(120.0)
                                .show(ui, |ui| {
                                    pr_detail(ui, pr, linked.contains(&pr.number), &mut actions)
                                });
                        }
                        egui::CentralPanel::no_frame().show(ui, |ui| {
                            pr_list(
                                ui,
                                &forge.pulls,
                                self.selected_pr,
                                &linked,
                                now,
                                &mut actions,
                            );
                        });
                    }
                    ForgeTab::Issues => issue_list(ui, &forge.issues, now),
                }
            }
        }
        self.actions.extend(actions);
    }
}

fn card(ui: &mut Ui, selected: bool, add_contents: impl FnOnce(&mut Ui)) -> egui::Response {
    let fill = if selected {
        ui.visuals().selection.bg_fill.gamma_multiply(0.6)
    } else {
        ui.visuals().faint_bg_color
    };
    Frame::new()
        .inner_margin(6.0)
        .corner_radius(4.0)
        .fill(fill)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Selectable text would take the click instead of the card.
            ui.style_mut().interaction.selectable_labels = false;
            add_contents(ui);
        })
        .response
        .interact(Sense::click())
}

fn pr_list(
    ui: &mut Ui,
    pulls: &[PullRequest],
    selected: Option<u64>,
    linked: &HashSet<u64>,
    now: i64,
    actions: &mut Vec<Action>,
) {
    if pulls.is_empty() {
        ui.weak("No open pull requests.");
        return;
    }
    ScrollArea::vertical()
        .id_salt("pr_list")
        .auto_shrink(false)
        .show(ui, |ui| {
            for pr in pulls {
                let response = card(ui, selected == Some(pr.number), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("#{}", pr.number)).monospace().weak());
                        ui.add(egui::Label::new(RichText::new(&pr.title).strong()).wrap());
                    });
                    let mut meta = format!("{} · {} → {}", pr.author, pr.head_ref, pr.base_ref);
                    if let Some(t) = pr.updated_at {
                        meta.push_str(&format!(" · {}", util::relative_time(t, now)));
                    }
                    ui.add(egui::Label::new(RichText::new(meta).weak()).wrap());
                    ui.horizontal_wrapped(|ui| {
                        if let Some(state) = pr.checks.overall() {
                            let text = format!(
                                "CI {} {}/{}",
                                ui::check_label(state),
                                pr.checks.count(CheckState::Passed),
                                pr.checks.runs.len()
                            );
                            Chip::new(text, ui::check_color(state)).show(ui);
                        }
                        if let Some((label, color)) = ui::review_label(pr.review) {
                            Chip::new(label, color).show(ui);
                        }
                        if pr.is_draft {
                            Chip::new("draft", Color32::from_gray(110)).show(ui);
                        }
                        if pr.cross_repo {
                            Chip::new("fork", Color32::from_gray(110)).show(ui);
                        }
                        if linked.contains(&pr.number) {
                            Chip::new("in graph", Color32::from_rgb(33, 90, 145)).show(ui);
                        }
                    });
                });
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::Role::ListItem,
                        true,
                        selected == Some(pr.number),
                        format!("#{} {}", pr.number, pr.title),
                    )
                });
                if response.double_clicked() {
                    actions.push(Action::ShowPrInGraph(pr.number));
                } else if response.clicked() {
                    actions.push(Action::SelectPr(pr.number));
                }
                ui.add_space(4.0);
            }
        });
}

fn pr_detail(ui: &mut Ui, pr: &PullRequest, in_graph: bool, actions: &mut Vec<Action>) {
    ui.add_space(4.0);
    ui.add(egui::Label::new(RichText::new(format!("#{} {}", pr.number, pr.title)).strong()).wrap());
    ui.weak(format!(
        "{} wants to merge {} into {}",
        pr.author, pr.head_ref, pr.base_ref
    ));
    ui.horizontal_wrapped(|ui| {
        let show = ui
            .add_enabled(in_graph, Button::new("Show in graph"))
            .on_disabled_hover_text("Branch not in this clone yet. Fetch or check out first.");
        if show.clicked() {
            actions.push(Action::ShowPrInGraph(pr.number));
        }
        if ui
            .button("Check out")
            .on_hover_text("gh pr checkout")
            .clicked()
        {
            actions.push(Action::CheckoutPr(pr.number));
        }
        if ui.button("Open on GitHub").clicked() {
            ui.ctx().open_url(OpenUrl::new_tab(&pr.url));
        }
    });
    ui.separator();

    if pr.checks.runs.is_empty() {
        ui.weak("No CI checks reported.");
        return;
    }
    ui.strong(format!(
        "Checks: {} passed, {} failed, {} pending",
        pr.checks.count(CheckState::Passed),
        pr.checks.count(CheckState::Failed),
        pr.checks.count(CheckState::Pending)
    ));
    ScrollArea::vertical()
        .id_salt("pr_checks")
        .auto_shrink(false)
        .show(ui, |ui| {
            for run in &pr.checks.runs {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), 4.0, ui::check_color(run.state));
                    match &run.url {
                        Some(url) => {
                            ui.hyperlink_to(&run.name, url);
                        }
                        None => {
                            ui.label(&run.name);
                        }
                    }
                    ui.weak(ui::check_label(run.state));
                });
            }
        });
}

fn issue_list(ui: &mut Ui, issues: &[Issue], now: i64) {
    if issues.is_empty() {
        ui.weak("No open issues.");
        return;
    }
    ScrollArea::vertical()
        .id_salt("issue_list")
        .auto_shrink(false)
        .show(ui, |ui| {
            for issue in issues {
                let response = card(ui, false, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("#{}", issue.number))
                                .monospace()
                                .weak(),
                        );
                        ui.add(egui::Label::new(RichText::new(&issue.title).strong()).wrap());
                    });
                    if !issue.labels.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for label in &issue.labels {
                                let color =
                                    ui::hex_color(&label.color).unwrap_or(Color32::from_gray(110));
                                Chip::new(label.name.clone(), color).show(ui);
                            }
                        });
                    }
                    let mut meta = issue.author.clone();
                    if let Some(t) = issue.updated_at {
                        meta.push_str(&format!(" · updated {}", util::relative_time(t, now)));
                    }
                    if !issue.assignees.is_empty() {
                        meta.push_str(&format!(" · assigned to {}", issue.assignees.join(", ")));
                    }
                    ui.add(egui::Label::new(RichText::new(meta).weak()).wrap());
                });
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::ListItem,
                        true,
                        format!("#{} {}", issue.number, issue.title),
                    )
                });
                if response.on_hover_text("Open on GitHub").clicked() {
                    ui.ctx().open_url(OpenUrl::new_tab(&issue.url));
                }
                ui.add_space(4.0);
            }
        });
}
