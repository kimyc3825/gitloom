//! Application state, background-result handling and the top-level layout.

mod changes;
mod forge_panel;
mod history;
mod sidebar;

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, Align, Color32, Key, Layout, RichText, Ui};
use git2::Oid;

use crate::forge::{ForgeError, ForgeSnapshot, github};
use crate::git::ops::GitOp;
use crate::git::repo::{self, CommitDetail, FileDiff, RefKind, RepoSnapshot, WorkingTree};
use crate::jobs::{JobResult, Jobs};
use crate::ui::graph::ROW_HEIGHT;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    History,
    Changes,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ForgeTab {
    Pulls,
    Issues,
}

enum ForgeState {
    Idle,
    Loaded(ForgeSnapshot),
    Failed(ForgeError),
}

/// Requests made while drawing. They are applied after the UI code of the
/// frame, so drawing code only ever needs shared borrows of the state.
enum Action {
    SelectCommit { id: Oid, center: bool },
    SelectChange { path: String, staged: bool },
    Op(GitOp),
    CheckoutPr(u64),
    SelectPr(u64),
    ShowPrInGraph(u64),
    OpenBranchDialog(Oid),
    Refresh,
    RefreshForge,
}

struct Message {
    text: String,
    error: bool,
}

struct BranchDialog {
    at: Oid,
    name: String,
    focus: bool,
}

pub struct GitloomApp {
    jobs: Jobs,
    root: Option<PathBuf>,
    path_input: String,
    open_error: Option<String>,

    snapshot: Option<RepoSnapshot>,
    snapshot_gen: u64,
    snapshot_loading: bool,

    view: View,
    selected: Option<Oid>,
    detail: Option<CommitDetail>,
    detail_loading: Option<Oid>,
    detail_file: usize,
    /// (scroll offset, viewport height) of the graph, from the last frame.
    graph_viewport: (f32, f32),
    graph_scroll_to: Option<f32>,

    status: Option<WorkingTree>,
    status_gen: u64,
    selected_change: Option<(String, bool)>,
    change_diff: Option<FileDiff>,
    commit_message: String,
    amend: bool,

    forge: ForgeState,
    forge_gen: u64,
    forge_loading: bool,
    forge_tab: ForgeTab,
    selected_pr: Option<u64>,
    /// Index into `snapshot.refs` -> index into the forge's pull requests.
    pr_for_ref: HashMap<usize, usize>,

    running_ops: usize,
    busy_label: Option<String>,
    message: Option<Message>,
    branch_dialog: Option<BranchDialog>,
    actions: Vec<Action>,
    was_focused: bool,
}

impl GitloomApp {
    pub fn new(ctx: egui::Context, initial_repo: Option<PathBuf>) -> Self {
        let mut app = Self {
            jobs: Jobs::new(ctx),
            root: None,
            path_input: String::new(),
            open_error: None,
            snapshot: None,
            snapshot_gen: 0,
            snapshot_loading: false,
            view: View::History,
            selected: None,
            detail: None,
            detail_loading: None,
            detail_file: 0,
            graph_viewport: (0.0, 0.0),
            graph_scroll_to: None,
            status: None,
            status_gen: 0,
            selected_change: None,
            change_diff: None,
            commit_message: String::new(),
            amend: false,
            forge: ForgeState::Idle,
            forge_gen: 0,
            forge_loading: false,
            forge_tab: ForgeTab::Pulls,
            selected_pr: None,
            pr_for_ref: HashMap::new(),
            running_ops: 0,
            busy_label: None,
            message: None,
            branch_dialog: None,
            actions: Vec::new(),
            was_focused: true,
        };
        if let Some(path) = initial_repo {
            app.open_repo(path);
        }
        app
    }

    fn open_repo(&mut self, path: PathBuf) {
        let root = match repo::discover(&path) {
            Ok(root) => root,
            Err(e) => {
                self.open_error = Some(format!("{e:#}"));
                return;
            }
        };
        self.path_input = root.display().to_string();
        self.open_error = None;
        self.root = Some(root);
        self.snapshot = None;
        self.selected = None;
        self.detail = None;
        self.detail_loading = None;
        self.detail_file = 0;
        self.graph_scroll_to = Some(0.0);
        self.status = None;
        self.selected_change = None;
        self.change_diff = None;
        self.commit_message.clear();
        self.amend = false;
        self.forge = ForgeState::Idle;
        self.selected_pr = None;
        self.pr_for_ref.clear();
        self.view = View::History;
        self.message = None;
        self.refresh(true);
    }

    fn refresh(&mut self, include_forge: bool) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.snapshot_gen += 1;
        self.snapshot_loading = true;
        let generation = self.snapshot_gen;
        self.jobs.spawn(move || JobResult::Snapshot {
            generation,
            result: repo::load_snapshot(&root),
        });
        self.refresh_status();
        if include_forge {
            self.refresh_forge();
        }
    }

    fn refresh_status(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.status_gen += 1;
        let generation = self.status_gen;
        self.jobs.spawn(move || JobResult::Status {
            generation,
            result: repo::load_status(&root),
        });
    }

    fn refresh_forge(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.forge_gen += 1;
        self.forge_loading = true;
        let generation = self.forge_gen;
        self.jobs.spawn(move || JobResult::Forge {
            generation,
            result: github::load(&root),
        });
    }

    fn select_commit(&mut self, id: Oid, center: bool) {
        self.selected = Some(id);
        if center
            && let Some(row) = self
                .snapshot
                .as_ref()
                .and_then(|s| s.row_of.get(&id).copied())
        {
            self.scroll_to_row(row, true);
        }
        if self.detail.as_ref().is_some_and(|d| d.id == id) || self.detail_loading == Some(id) {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        self.detail_loading = Some(id);
        self.jobs.spawn(move || JobResult::CommitDetail {
            id,
            result: repo::load_commit_detail(&root, id),
        });
    }

    fn scroll_to_row(&mut self, row: usize, center: bool) {
        let top = row as f32 * ROW_HEIGHT;
        let (offset, height) = self.graph_viewport;
        if center || height <= 0.0 {
            self.graph_scroll_to = Some((top - height * 0.4).max(0.0));
        } else if top < offset {
            self.graph_scroll_to = Some(top);
        } else if top + ROW_HEIGHT > offset + height {
            self.graph_scroll_to = Some(top + ROW_HEIGHT - height);
        }
    }

    fn select_change(&mut self, path: String, staged: bool) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.selected_change = Some((path.clone(), staged));
        self.jobs.spawn(move || {
            let result = repo::load_file_diff(&root, &path, staged);
            JobResult::FileDiff {
                path,
                staged,
                result,
            }
        });
    }

    fn run_op(&mut self, op: GitOp) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.running_ops += 1;
        self.busy_label = Some(format!("{}…", op.label()));
        self.jobs.spawn(move || {
            let result = op.run(&root);
            JobResult::GitOp { op, result }
        });
    }

    fn checkout_pr(&mut self, number: u64) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.running_ops += 1;
        self.busy_label = Some(format!("Checking out #{number}…"));
        self.jobs.spawn(move || JobResult::PrCheckout {
            number,
            result: github::checkout_pr(&root, number),
        });
    }

    fn finish_op(&mut self) {
        self.running_ops = self.running_ops.saturating_sub(1);
        if self.running_ops == 0 {
            self.busy_label = None;
        }
    }

    /// Push to the upstream, or publish the branch to `origin` if it has none.
    fn push_op(&self) -> GitOp {
        let has_upstream = self
            .snapshot
            .as_ref()
            .and_then(|s| s.head_ref())
            .is_none_or(|r| r.upstream.is_some());
        GitOp::Push {
            set_upstream: (!has_upstream).then(|| "origin".to_owned()),
        }
    }

    fn set_info(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            error: false,
        });
    }

    fn set_error(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            error: true,
        });
    }

    /// Link pull requests to the local/remote branches they come from.
    fn recompute_pr_links(&mut self) {
        self.pr_for_ref.clear();
        let (Some(snap), ForgeState::Loaded(forge)) = (&self.snapshot, &self.forge) else {
            return;
        };
        for (pi, pr) in forge.pulls.iter().enumerate() {
            if pr.cross_repo {
                continue; // the head branch lives in a fork
            }
            for (ri, r) in snap.refs.iter().enumerate() {
                if r.kind != RefKind::Tag && r.branch_name() == pr.head_ref {
                    self.pr_for_ref.insert(ri, pi);
                }
            }
        }
    }

    /// Commit a PR points at in the graph: its remote branch, else the local one.
    fn pr_target(&self, number: u64) -> Option<Oid> {
        let snap = self.snapshot.as_ref()?;
        let ForgeState::Loaded(forge) = &self.forge else {
            return None;
        };
        let pi = forge.pulls.iter().position(|p| p.number == number)?;
        self.pr_for_ref
            .iter()
            .filter(|&(_, &p)| p == pi)
            .map(|(&ri, _)| &snap.refs[ri])
            .min_by_key(|r| (r.kind != RefKind::RemoteBranch, r.name.clone()))
            .map(|r| r.target)
    }

    fn poll_jobs(&mut self) {
        while let Some(result) = self.jobs.try_recv() {
            match result {
                JobResult::Snapshot { generation, result } => {
                    if generation != self.snapshot_gen {
                        continue;
                    }
                    self.snapshot_loading = false;
                    match result {
                        Ok(snap) => self.apply_snapshot(snap),
                        Err(e) => self.set_error(format!("Failed to load history: {e:#}")),
                    }
                }
                JobResult::CommitDetail { id, result } => {
                    if self.detail_loading == Some(id) {
                        self.detail_loading = None;
                    }
                    if self.selected != Some(id) {
                        continue;
                    }
                    match result {
                        Ok(detail) => {
                            self.detail = Some(detail);
                            self.detail_file = 0;
                        }
                        Err(e) => self.set_error(format!("Failed to load commit: {e:#}")),
                    }
                }
                JobResult::Status { generation, result } => {
                    if generation != self.status_gen {
                        continue;
                    }
                    match result {
                        Ok(status) => self.apply_status(status),
                        Err(e) => self.set_error(format!("Failed to read status: {e:#}")),
                    }
                }
                JobResult::FileDiff {
                    path,
                    staged,
                    result,
                } => {
                    if self.selected_change.as_ref() != Some(&(path, staged)) {
                        continue;
                    }
                    match result {
                        Ok(diff) => self.change_diff = Some(diff),
                        Err(e) => self.set_error(format!("Failed to load diff: {e:#}")),
                    }
                }
                JobResult::Forge { generation, result } => {
                    if generation != self.forge_gen {
                        continue;
                    }
                    self.forge_loading = false;
                    self.forge = match result {
                        Ok(forge) => ForgeState::Loaded(forge),
                        Err(e) => ForgeState::Failed(e),
                    };
                    self.recompute_pr_links();
                }
                JobResult::GitOp { op, result } => {
                    self.finish_op();
                    match result {
                        Ok(output) => {
                            if matches!(op, GitOp::Commit { .. }) {
                                self.commit_message.clear();
                                self.amend = false;
                            }
                            let detail = output
                                .lines()
                                .last()
                                .map(|l| format!(": {l}"))
                                .unwrap_or_default();
                            self.set_info(format!("{} — done{detail}", op.label()));
                        }
                        Err(e) => self.set_error(format!("{} failed: {e:#}", op.label())),
                    }
                    if op.changes_history() {
                        let remote = matches!(op, GitOp::Fetch | GitOp::Pull | GitOp::Push { .. });
                        self.refresh(remote);
                    } else {
                        self.refresh_status();
                    }
                }
                JobResult::PrCheckout { number, result } => {
                    self.finish_op();
                    match result {
                        Ok(_) => self.set_info(format!("Checked out pull request #{number}")),
                        Err(e) => self.set_error(format!("Checking out #{number} failed: {e}")),
                    }
                    self.refresh(false);
                }
            }
        }
    }

    fn apply_snapshot(&mut self, snap: RepoSnapshot) {
        // If the selection was following HEAD (e.g. right after a commit), keep following it.
        let old_head = self.snapshot.as_ref().and_then(|s| s.head.target);
        let follows_head = self.selected.is_some() && self.selected == old_head;
        let kept = self
            .selected
            .filter(|id| !follows_head && snap.row_of.contains_key(id));
        let first_load = self.snapshot.is_none();
        let target = kept
            .or(snap.head.target)
            .or_else(|| snap.commits.first().map(|c| c.id));
        self.snapshot = Some(snap);
        self.recompute_pr_links();
        match target {
            Some(id) => self.select_commit(id, first_load && kept.is_none()),
            None => {
                self.selected = None;
                self.detail = None;
            }
        }
    }

    fn apply_status(&mut self, status: WorkingTree) {
        if let Some((path, staged)) = self.selected_change.clone() {
            let list = if staged {
                &status.staged
            } else {
                &status.unstaged
            };
            if list.iter().any(|f| f.path == path) {
                // Contents may have changed on disk; reload the diff.
                self.select_change(path, staged);
            } else {
                self.selected_change = None;
                self.change_diff = None;
            }
        }
        self.status = Some(status);
    }

    fn process_actions(&mut self) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                Action::SelectCommit { id, center } => self.select_commit(id, center),
                Action::SelectChange { path, staged } => {
                    self.change_diff = None;
                    self.select_change(path, staged);
                }
                Action::Op(op) => self.run_op(op),
                Action::CheckoutPr(number) => self.checkout_pr(number),
                Action::SelectPr(number) => {
                    self.selected_pr = Some(number);
                    self.forge_tab = ForgeTab::Pulls;
                }
                Action::ShowPrInGraph(number) => match self.pr_target(number) {
                    Some(id) => {
                        self.view = View::History;
                        self.select_commit(id, true);
                    }
                    None => self.set_info(format!(
                        "The branch of #{number} is not in this clone yet. Fetch or check it out first."
                    )),
                },
                Action::OpenBranchDialog(at) => {
                    self.branch_dialog = Some(BranchDialog { at, name: String::new(), focus: true });
                }
                Action::Refresh => self.refresh(true),
                Action::RefreshForge => self.refresh_forge(),
            }
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.open_repo(path);
        }

        // Pick up changes made outside (terminal, editor) when the window regains focus.
        let focused = ctx.input(|i| i.focused);
        if focused && !self.was_focused && self.root.is_some() {
            self.refresh(false);
        }
        self.was_focused = focused;

        if ctx.input(|i| i.key_pressed(Key::F5)) {
            self.actions.push(Action::Refresh);
        }
        if self.view != View::History || ctx.egui_wants_keyboard_input() {
            return;
        }
        let Some(snap) = &self.snapshot else { return };
        let Some(last) = snap.commits.len().checked_sub(1) else {
            return;
        };
        let page = ((self.graph_viewport.1 / ROW_HEIGHT) as usize).max(1);
        let current = self.selected.and_then(|id| snap.row_of.get(&id).copied());
        let target = ctx.input(|i| {
            let from = |f: fn(usize) -> usize| Some(current.map_or(0, f));
            if i.key_pressed(Key::ArrowDown) {
                from(|r| r + 1)
            } else if i.key_pressed(Key::ArrowUp) {
                from(|r| r.saturating_sub(1))
            } else if i.key_pressed(Key::PageDown) {
                Some(current.map_or(0, |r| r + page))
            } else if i.key_pressed(Key::PageUp) {
                Some(current.map_or(0, |r| r.saturating_sub(page)))
            } else if i.key_pressed(Key::Home) {
                Some(0)
            } else if i.key_pressed(Key::End) {
                Some(last)
            } else {
                None
            }
        });
        if let Some(row) = target.map(|r| r.min(last)) {
            let id = snap.commits[row].id;
            self.scroll_to_row(row, false);
            self.select_commit(id, false);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("gitloom").strong().size(15.0));
            ui.separator();
            if let Some(snap) = &self.snapshot {
                ui.label(RichText::new(&snap.name).strong());
                let head = match (&snap.head.branch, snap.head.target) {
                    (Some(branch), _) => branch.clone(),
                    (None, Some(id)) => format!("detached at {}", &id.to_string()[..7]),
                    (None, None) => "no commits".to_owned(),
                };
                ui.label(format!("on {head}"));
                if let Some((ahead, behind)) = snap.head_ref().and_then(|r| r.ahead_behind) {
                    ui.weak(format!("↑{ahead} ↓{behind}"));
                }
                ui.separator();
            }
            if ui.button("Open…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Open repository")
                    .pick_folder()
            {
                self.open_repo(path);
            }
            ui.scope(|ui| {
                if self.root.is_none() {
                    ui.disable();
                }
                if ui
                    .button("Fetch")
                    .on_hover_text("git fetch --all --prune")
                    .clicked()
                {
                    self.actions.push(Action::Op(GitOp::Fetch));
                }
                if ui
                    .button("Pull")
                    .on_hover_text("git pull --ff-only")
                    .clicked()
                {
                    self.actions.push(Action::Op(GitOp::Pull));
                }
                if ui.button("Push").clicked() {
                    let op = self.push_op();
                    self.actions.push(Action::Op(op));
                }
                if ui.button("Refresh").on_hover_text("F5").clicked() {
                    self.actions.push(Action::Refresh);
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(label) = &self.busy_label {
                    ui.label(label);
                    ui.spinner();
                } else if self.snapshot_loading || self.forge_loading {
                    ui.spinner();
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let mut dismiss = false;
            if let Some(message) = &self.message {
                let first = message.text.lines().next().unwrap_or_default();
                let color = if message.error {
                    Color32::from_rgb(239, 83, 80)
                } else {
                    ui.visuals().text_color()
                };
                let response = ui.label(RichText::new(first).color(color));
                if message.text.lines().count() > 1 {
                    response.on_hover_text(&message.text);
                }
                dismiss = ui.small_button("×").on_hover_text("Dismiss").clicked();
            }
            if dismiss {
                self.message = None;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(snap) = &self.snapshot {
                    ui.weak(format!(
                        "{} commits · loaded in {} ms",
                        snap.commits.len(),
                        snap.load_time.as_millis()
                    ));
                    if snap.truncated {
                        ui.weak(format!("showing the newest {}", repo::MAX_COMMITS));
                    }
                }
            });
        });
    }

    fn welcome(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.heading(RichText::new("gitloom").size(28.0).strong());
            ui.label("Commit graph, changes, pull requests, issues and CI in one window.");
            ui.add_space(16.0);
            let mut open = false;
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.path_input)
                        .hint_text("Path to a git repository")
                        .desired_width(420.0),
                );
                open |= edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                open |= ui.button("Open").clicked();
                if ui.button("Browse…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title("Open repository")
                        .pick_folder()
                {
                    self.path_input = path.display().to_string();
                    open = true;
                }
            });
            if open {
                self.open_repo(PathBuf::from(self.path_input.trim()));
            }
            ui.add_space(6.0);
            ui.weak("…or drop a repository folder onto this window.");
            if let Some(err) = &self.open_error {
                ui.add_space(10.0);
                ui.colored_label(Color32::from_rgb(239, 83, 80), err);
            }
        });
    }

    fn view_tabs(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.view,
                View::History,
                RichText::new("History").strong(),
            );
            let changes = self
                .status
                .as_ref()
                .map_or(0, |s| s.staged.len() + s.unstaged.len());
            let label = if changes > 0 {
                format!("Changes ({changes})")
            } else {
                "Changes".to_owned()
            };
            ui.selectable_value(&mut self.view, View::Changes, RichText::new(label).strong());
        });
        ui.separator();
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
}

impl eframe::App for GitloomApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_jobs();
        self.handle_input(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));

        if self.root.is_none() {
            egui::CentralPanel::default_margins().show(ui, |ui| self.welcome(ui));
        } else {
            egui::Panel::left("sidebar")
                .resizable(true)
                .default_size(230.0)
                .min_size(150.0)
                .show(ui, |ui| self.sidebar(ui));
            egui::Panel::right("forge")
                .resizable(true)
                .default_size(360.0)
                .min_size(240.0)
                .show(ui, |ui| self.forge_panel(ui));
            egui::CentralPanel::default_margins().show(ui, |ui| {
                self.view_tabs(ui);
                match self.view {
                    View::History => self.history_view(ui),
                    View::Changes => self.changes_view(ui),
                }
            });
        }

        self.branch_dialog_ui(&ctx);
        if !self.actions.is_empty() {
            self.process_actions();
            ctx.request_repaint();
        }
    }
}
