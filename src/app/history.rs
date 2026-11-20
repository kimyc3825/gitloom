//! History view: the commit graph on top, the selected commit's details below.

use eframe::egui::{
    self, Align2, Color32, FontId, Key, Rect, RichText, ScrollArea, Sense, Ui, pos2, vec2,
};

use super::file_view::FileMode;
use super::{Action, ForgeState, GitloomApp};
use crate::git::ops::GitOp;
use crate::git::repo::{RefKind, RepoSnapshot};
use crate::search;
use crate::ui::graph::{self as graph_ui, NodeStyle, ROW_HEIGHT};
use crate::ui::{self, Chip, diff};
use crate::util;

const AUTHOR_W: f32 = 140.0;
const DATE_W: f32 = 100.0;
const SHA_W: f32 = 70.0;
/// Columns for author/date are hidden below this width.
const WIDE_LAYOUT: f32 = 820.0;
const MAX_CHIPS: usize = 4;
/// Marker next to commits that match the search.
const MATCH_COLOR: Color32 = Color32::from_rgb(255, 179, 0);

impl GitloomApp {
    pub(super) fn history_view(&mut self, ui: &mut Ui) {
        egui::Panel::bottom("commit_detail")
            .resizable(true)
            .default_size(340.0)
            .min_size(140.0)
            .show(ui, |ui| self.commit_detail(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.search_bar(ui);
            self.graph_list(ui);
        });
    }

    fn search_bar(&mut self, ui: &mut Ui) {
        if self.snapshot.as_ref().is_none_or(|s| s.commits.is_empty()) {
            return;
        }
        ui.horizontal(|ui| {
            let id = egui::Id::unique("commit_search");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .id(id)
                    .hint_text("Search commits: message, author, SHA  (Ctrl+F)")
                    .desired_width(330.0),
            );
            if std::mem::take(&mut self.focus_search) {
                response.request_focus();
            }
            if response.changed() {
                self.update_search();
                self.step_search(0);
            }
            // Enter / Shift+Enter walk through the matches; Escape clears.
            if response.lost_focus() {
                let (enter, shift, escape) = ui.input(|i| {
                    (
                        i.key_pressed(Key::Enter),
                        i.modifiers.shift,
                        i.key_pressed(Key::Escape),
                    )
                });
                if enter {
                    self.step_search(if shift { -1 } else { 1 });
                    response.request_focus();
                } else if escape {
                    self.search_query.clear();
                    self.update_search();
                }
            }
            if self.search_query.trim().is_empty() {
                return;
            }
            let total = self.search_matches.len();
            if total == 0 {
                ui.colored_label(Color32::from_rgb(239, 83, 80), "No matches");
            } else {
                ui.weak(format!("{} of {total}", self.search_pos + 1));
                if ui
                    .small_button("Prev")
                    .on_hover_text("Shift+Enter")
                    .clicked()
                {
                    self.step_search(-1);
                }
                if ui.small_button("Next").on_hover_text("Enter").clicked() {
                    self.step_search(1);
                }
            }
            if ui.small_button("Clear").clicked() {
                self.search_query.clear();
                self.update_search();
            }
            if self.snapshot.as_ref().is_some_and(|s| s.truncated) {
                ui.weak("(searching the loaded commits only)");
            }
        });
    }

    /// Recompute the matches for the current query and snapshot.
    pub(super) fn update_search(&mut self) {
        let commits = self
            .snapshot
            .as_ref()
            .map_or(&[][..], |s| s.commits.as_slice());
        self.search_matches = search::find(commits, &self.search_query);
        self.search_mask = vec![false; commits.len()];
        for &row in &self.search_matches {
            self.search_mask[row] = true;
        }
        self.search_pos = self
            .search_pos
            .min(self.search_matches.len().saturating_sub(1));
    }

    /// Move to the next (`1`), previous (`-1`) or first (`0`) match and select it.
    fn step_search(&mut self, delta: isize) {
        let total = self.search_matches.len();
        if total == 0 {
            return;
        }
        self.search_pos = if delta == 0 {
            0
        } else {
            (self.search_pos as isize + delta).rem_euclid(total as isize) as usize
        };
        let row = self.search_matches[self.search_pos];
        if let Some(id) = self.snapshot.as_ref().map(|s| s.commits[row].id) {
            self.select_commit(id, true);
        }
    }

    fn graph_list(&mut self, ui: &mut Ui) {
        let Some(snap) = &self.snapshot else {
            diff::placeholder(ui, self.snapshot_loading, "No history");
            return;
        };
        if snap.commits.is_empty() {
            ui.weak(if snap.head.unborn {
                "No commits yet. Stage files in the Changes tab and make the first commit."
            } else {
                "No commits."
            });
            return;
        }

        let wide = ui.available_width() > WIDE_LAYOUT;
        let right_w = if wide {
            AUTHOR_W + DATE_W + SHA_W
        } else {
            SHA_W
        };
        // Repos with hundreds of branches can need dozens of lanes; keep room for text.
        let graph_w =
            graph_ui::graph_width(&snap.graph).min((ui.available_width() * 0.35).max(44.0));
        let visuals = ui.visuals().clone();
        let weak = visuals.weak_text_color();
        let now = util::now_secs();
        let pulls = match &self.forge {
            ForgeState::Loaded(forge) => forge.pulls.as_slice(),
            _ => &[],
        };

        // Column header.
        let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
        let hp = ui.painter_at(header);
        let font = FontId::proportional(11.0);
        let hy = header.center().y;
        hp.text(
            pos2(header.left() + 4.0, hy),
            Align2::LEFT_CENTER,
            "Graph",
            font.clone(),
            weak,
        );
        let mut hx = header.right() - right_w;
        if header.left() + graph_w + 80.0 < hx {
            hp.text(
                pos2(header.left() + graph_w + 6.0, hy),
                Align2::LEFT_CENTER,
                "Description",
                font.clone(),
                weak,
            );
        }
        if wide {
            hp.text(
                pos2(hx, hy),
                Align2::LEFT_CENTER,
                "Author",
                font.clone(),
                weak,
            );
            hx += AUTHOR_W;
            hp.text(
                pos2(hx, hy),
                Align2::LEFT_CENTER,
                "Date",
                font.clone(),
                weak,
            );
            hx += DATE_W;
        }
        hp.text(pos2(hx, hy), Align2::LEFT_CENTER, "Commit", font, weak);

        let mut scroll = ScrollArea::vertical().id_salt("graph").auto_shrink(false);
        if let Some(offset) = self.graph_scroll_to.take() {
            scroll = scroll.vertical_scroll_offset(offset);
        }

        let selected = self.selected;
        let pr_for_ref = &self.pr_for_ref;
        // Merging, rebasing etc. are offered only while nothing else is in progress.
        let busy = self
            .status
            .as_ref()
            .is_some_and(|s| s.in_progress.is_some());
        let searching = !self.search_query.trim().is_empty();
        let search_mask = &self.search_mask;
        let mut actions = Vec::new();
        let output = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                scroll.show_rows(ui, ROW_HEIGHT, snap.commits.len(), |ui, rows| {
                    for row in rows {
                        let commit = &snap.commits[row];
                        let (rect, response) = ui.allocate_exact_size(
                            vec2(ui.available_width(), ROW_HEIGHT),
                            Sense::click(),
                        );
                        let painter = ui.painter_at(rect);
                        let is_selected = selected == Some(commit.id);
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::Role::ListItem,
                                true,
                                is_selected,
                                &commit.summary,
                            )
                        });
                        let background = if is_selected {
                            visuals.selection.bg_fill
                        } else if response.hovered() {
                            visuals.widgets.hovered.weak_bg_fill
                        } else {
                            visuals.panel_fill
                        };
                        if is_selected || response.hovered() {
                            painter.rect_filled(rect, 0.0, background);
                        }
                        // While searching, rows that don't match fade out.
                        let is_match = search_mask.get(row).copied().unwrap_or(false);
                        let mut painter = painter;
                        if searching && !is_match {
                            painter.multiply_opacity(0.3);
                        }

                        let graph_rect = Rect::from_min_size(rect.min, vec2(graph_w, ROW_HEIGHT));
                        graph_ui::paint_row(
                            &painter.with_clip_rect(graph_rect),
                            graph_rect,
                            &snap.graph.rows[row],
                            &NodeStyle {
                                is_merge: commit.parents.len() > 1,
                                is_head: snap.head.target == Some(commit.id),
                                background,
                                head_ring: visuals.strong_text_color(),
                            },
                        );

                        let y = rect.center().y;
                        if searching && is_match {
                            let marker = Rect::from_min_size(
                                pos2(graph_rect.right(), rect.top() + 3.0),
                                vec2(3.0, ROW_HEIGHT - 6.0),
                            );
                            painter.rect_filled(marker, 1.0, MATCH_COLOR);
                        }
                        let text_right = rect.right() - right_w - 8.0;
                        let label_painter = painter.with_clip_rect(Rect::from_x_y_ranges(
                            rect.left()..=text_right,
                            rect.y_range(),
                        ));
                        let mut x = graph_rect.right() + 4.0;
                        if snap.head.detached && snap.head.target == Some(commit.id) {
                            x = Chip::detached_head().paint(&label_painter, x, y) + 4.0;
                        }
                        if let Some(indices) = snap.refs_at.get(&commit.id) {
                            let mut shown_prs = Vec::new();
                            for (n, &ri) in indices.iter().enumerate() {
                                if n == MAX_CHIPS {
                                    let more = Chip::new(
                                        format!("+{}", indices.len() - n),
                                        Color32::from_gray(90),
                                    );
                                    x = more.paint(&label_painter, x, y) + 4.0;
                                    break;
                                }
                                x = Chip::for_ref(&snap.refs[ri]).paint(&label_painter, x, y) + 4.0;
                                if let Some(&pi) = pr_for_ref.get(&ri)
                                    && let (false, Some(pr)) =
                                        (shown_prs.contains(&pi), pulls.get(pi))
                                {
                                    shown_prs.push(pi);
                                    x = Chip::for_pr(pr).paint(&label_painter, x, y) + 4.0;
                                }
                            }
                        }
                        if text_right > x {
                            painter
                                .with_clip_rect(Rect::from_x_y_ranges(
                                    x..=text_right,
                                    rect.y_range(),
                                ))
                                .text(
                                    pos2(x + 2.0, y),
                                    Align2::LEFT_CENTER,
                                    &commit.summary,
                                    FontId::proportional(13.0),
                                    visuals.text_color(),
                                );
                        }

                        let mut cx = rect.right() - right_w;
                        if wide {
                            painter
                                .with_clip_rect(Rect::from_x_y_ranges(
                                    cx..=cx + AUTHOR_W - 10.0,
                                    rect.y_range(),
                                ))
                                .text(
                                    pos2(cx, y),
                                    Align2::LEFT_CENTER,
                                    &commit.author_name,
                                    FontId::proportional(12.0),
                                    weak,
                                );
                            cx += AUTHOR_W;
                            painter.text(
                                pos2(cx, y),
                                Align2::LEFT_CENTER,
                                util::relative_time(commit.time, now),
                                FontId::proportional(12.0),
                                weak,
                            );
                            cx += DATE_W;
                        }
                        painter.text(
                            pos2(cx, y),
                            Align2::LEFT_CENTER,
                            &commit.short_id,
                            FontId::monospace(12.0),
                            weak,
                        );

                        if response.clicked() {
                            actions.push(Action::SelectCommit {
                                id: commit.id,
                                center: false,
                            });
                        }
                        response.context_menu(|ui| commit_menu(ui, snap, row, busy, &mut actions));
                    }
                })
            })
            .inner;

        self.graph_viewport = (output.state.clamped_offset().y, output.inner_rect.height());
        self.actions.extend(actions);
    }

    fn commit_detail(&mut self, ui: &mut Ui) {
        let loading = self.detail_loading.is_some();
        let Some(detail) = &self.detail else {
            diff::placeholder(ui, loading, "Select a commit");
            return;
        };
        let mut actions = Vec::new();
        let mut picked_file = None;

        egui::Panel::top("detail_header").show(ui, |ui| {
            let (summary, body) = detail
                .message
                .split_once('\n')
                .map_or((detail.message.as_str(), ""), |(s, b)| (s, b.trim()));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new(summary).strong().size(14.0)).wrap());
                if loading {
                    ui.spinner();
                }
            });
            ui.horizontal_wrapped(|ui| {
                let sha = detail.id.to_string();
                if ui
                    .link(RichText::new(&sha[..10]).monospace())
                    .on_hover_text("Copy full SHA")
                    .clicked()
                {
                    ui.ctx().copy_text(sha.clone());
                }
                ui.weak("·");
                ui.label(&detail.author);
                ui.weak("·");
                ui.weak(&detail.author_time);
                if !detail.parents.is_empty() {
                    ui.weak(if detail.parents.len() > 1 {
                        "· parents"
                    } else {
                        "· parent"
                    });
                    for parent in &detail.parents {
                        let short = parent.to_string();
                        if ui.link(RichText::new(&short[..7]).monospace()).clicked() {
                            actions.push(Action::SelectCommit {
                                id: *parent,
                                center: true,
                            });
                        }
                    }
                }
            });
            if detail.committer != detail.author {
                ui.weak(format!(
                    "committed by {} · {}",
                    detail.committer, detail.commit_time
                ));
            }
            if !body.is_empty() {
                ScrollArea::vertical()
                    .id_salt("commit_body")
                    .max_height(90.0)
                    .show(ui, |ui| {
                        ui.label(body);
                    });
            }
            ui.add_space(4.0);
        });

        let selected_file = self.detail_file;
        let is_stash = self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.stashes.iter().any(|stash| stash.id == detail.id));
        egui::Panel::left("detail_files")
            .resizable(true)
            .default_size(300.0)
            .min_size(160.0)
            .show(ui, |ui| {
                let (adds, dels) = detail
                    .files
                    .iter()
                    .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
                ui.horizontal(|ui| {
                    let n = detail.files.len();
                    ui.strong(format!("{n} file{}", if n == 1 { "" } else { "s" }));
                    ui.colored_label(Color32::from_rgb(102, 187, 106), format!("+{adds}"));
                    ui.colored_label(Color32::from_rgb(239, 83, 80), format!("−{dels}"));
                });
                if is_stash {
                    ui.weak("Stash: changes against the commit it was made on");
                } else if detail.parents.len() > 1 {
                    ui.weak("Merge commit: diff against the first parent");
                }
                if detail.truncated {
                    ui.weak("Large commit: diff truncated");
                }
                ScrollArea::vertical()
                    .id_salt("detail_files_list")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        for (i, file) in detail.files.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(file.status.to_string())
                                        .monospace()
                                        .color(ui::status_color(file.status)),
                                );
                                let item = egui::Button::selectable(selected_file == i, &file.path)
                                    .truncate();
                                let response = ui.add(item).on_hover_text(&file.path);
                                if response.clicked() {
                                    picked_file = Some(i);
                                }
                                response.context_menu(|ui| {
                                    if ui.button("File history").clicked() {
                                        actions.push(Action::OpenFile {
                                            path: file.path.clone(),
                                            mode: FileMode::History,
                                            rev: None,
                                        });
                                        ui.close();
                                    }
                                    // A stash isn't a real commit; there's nothing to blame there.
                                    if file.status != 'D'
                                        && !is_stash
                                        && ui.button("Blame at this commit").clicked()
                                    {
                                        actions.push(Action::OpenFile {
                                            path: file.path.clone(),
                                            mode: FileMode::Blame,
                                            rev: Some(detail.id),
                                        });
                                        ui.close();
                                    }
                                    if ui.button("Copy path").clicked() {
                                        ui.ctx().copy_text(file.path.clone());
                                        ui.close();
                                    }
                                });
                            });
                        }
                    });
            });

        diff::panel().show(ui, |ui| match detail.files.get(selected_file) {
            Some(file) => {
                diff::show(ui, file, ("commit_diff", detail.id, selected_file), None);
            }
            None => diff::placeholder(ui, false, "No file changes in this commit"),
        });

        if let Some(i) = picked_file {
            self.detail_file = i;
        }
        self.actions.extend(actions);
    }
}

fn commit_menu(
    ui: &mut Ui,
    snap: &RepoSnapshot,
    row: usize,
    busy: bool,
    actions: &mut Vec<Action>,
) {
    let commit = &snap.commits[row];
    let id = commit.id;
    let sha = id.to_string();
    if ui.button("Copy commit SHA").clicked() {
        ui.ctx().copy_text(sha.clone());
        ui.close();
    }
    ui.separator();
    let refs = snap.refs_at.get(&id).map(Vec::as_slice).unwrap_or_default();
    for &ri in refs {
        let r = &snap.refs[ri];
        match r.kind {
            RefKind::LocalBranch
                if !r.is_head && ui.button(format!("Switch to {}", r.name)).clicked() =>
            {
                actions.push(Action::Op(GitOp::Switch(r.name.clone())));
                ui.close();
            }
            RefKind::RemoteBranch
                if ui
                    .button(format!("Check out {} as a local branch", r.name))
                    .clicked() =>
            {
                actions.push(Action::Op(GitOp::TrackRemote(r.name.clone())));
                ui.close();
            }
            _ => {}
        }
    }
    if ui.button("Create branch here…").clicked() {
        actions.push(Action::OpenBranchDialog(id));
        ui.close();
    }
    if ui
        .button(format!("Check out {} (detached)", &sha[..7]))
        .clicked()
    {
        actions.push(Action::Op(GitOp::Detach(id)));
        ui.close();
    }

    let Some(head_id) = snap.head.target else {
        return;
    };
    let head = snap.head.branch.as_deref().unwrap_or("HEAD");
    let is_head = head_id == id;
    let on_head = snap.on_head.get(row).copied().unwrap_or(false);
    ui.separator();
    ui.add_enabled_ui(!busy, |ui| {
        for &ri in refs {
            let r = &snap.refs[ri];
            if r.kind == RefKind::Tag || r.is_head {
                continue;
            }
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
        }
        if !on_head && ui.button(format!("Cherry-pick onto {head}")).clicked() {
            actions.push(Action::Op(GitOp::CherryPick(id)));
            ui.close();
        }
        if on_head {
            let merge = commit.parents.len() > 1;
            let revert = ui
                .add_enabled(!merge, egui::Button::new("Revert this commit"))
                .on_hover_text("Make a new commit that undoes this one")
                .on_disabled_hover_text("Reverting a merge commit isn't supported yet");
            if revert.clicked() {
                actions.push(Action::Op(GitOp::Revert(id)));
                ui.close();
            }
        }
        if on_head
            && ui
                .button("Rebase interactively from here…")
                .on_hover_text(if is_head {
                    "Edit the latest commit"
                } else {
                    "Reorder, squash, reword or drop this commit and everything after it"
                })
                .clicked()
        {
            actions.push(Action::OpenRebase(id));
            ui.close();
        }
    })
    .response
    .on_disabled_hover_text("Finish or abort the operation in progress first");
}
