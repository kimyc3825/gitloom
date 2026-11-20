//! Conflicts: the banner for a merge/rebase/cherry-pick/revert in progress,
//! the list of conflicted files, and the 3-way merge view.

use eframe::egui::{self, Align, Button, Color32, Frame, Layout, RichText, ScrollArea, Stroke, Ui};

use super::{Action, Confirm, GitloomApp, View, plural};
use crate::git::conflict::{self, Choice, Conflict, ConflictKind, MergeDoc, Segment};
use crate::git::ops::{GitOp, Side};
use crate::git::repo::{ConflictedFile, InProgress, OpKind};
use crate::ui::{self as widgets, Chip, diff};

const OURS: Color32 = Color32::from_rgb(66, 165, 245);
const THEIRS: Color32 = Color32::from_rgb(186, 104, 200);
const RESOLVED: Color32 = Color32::from_rgb(76, 175, 80);
const PENDING: Color32 = Color32::from_rgb(255, 179, 0);
/// Unchanged lines shown around each conflict.
const CONTEXT: usize = 3;

pub(super) struct MergeView {
    pub doc: MergeDoc,
    /// One per conflict of `doc.segments`, in order.
    pub choices: Vec<Option<Choice>>,
    /// The conflict being edited by hand.
    pub editing: Option<usize>,
}

impl MergeView {
    pub fn new(doc: MergeDoc) -> Self {
        let n = doc
            .segments
            .as_ref()
            .map_or(0, |s| conflict::conflict_count(s));
        Self {
            doc,
            choices: vec![None; n],
            editing: None,
        }
    }

    /// Same file content as `other`: a refresh that shouldn't reset the choices.
    pub fn same_content(&self, other: &MergeDoc) -> bool {
        self.doc.path == other.path && self.doc.segments == other.segments
    }
}

fn tint(color: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 28)
}

impl GitloomApp {
    /// What git is in the middle of, with the buttons to finish or leave it.
    pub(super) fn operation_banner(&mut self, ui: &mut Ui) {
        let Some(status) = &self.status else { return };
        let Some(op) = &status.in_progress else {
            return;
        };
        let conflicts = status.conflicted.len();
        let kind = op.kind;
        let mut actions = Vec::new();
        Frame::new()
            .fill(tint(PENDING))
            .stroke(Stroke::new(1.0, PENDING.gamma_multiply(0.6)))
            .corner_radius(4.0)
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(describe(op)).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(format!("Abort {}…", kind.noun())).clicked() {
                            actions.push(Action::confirm(abort(kind)));
                        }
                        if kind.can_skip()
                            && ui
                                .button("Skip commit")
                                .on_hover_text("Leave out the commit it stopped at")
                                .clicked()
                        {
                            actions.push(Action::Op(GitOp::Skip(kind)));
                        }
                        let label = if kind == OpKind::Merge {
                            "Commit merge"
                        } else {
                            "Continue"
                        };
                        let button = Button::new(RichText::new(label).strong());
                        if ui
                            .add_enabled(conflicts == 0, button)
                            .on_disabled_hover_text("Resolve every conflict first")
                            .clicked()
                        {
                            actions.push(Action::Op(GitOp::Continue(kind)));
                        }
                    });
                });
                ui.horizontal(|ui| {
                    if conflicts > 0 {
                        ui.label(format!(
                            "{} with conflicts. Resolve them, then continue.",
                            plural(conflicts, "file", "files")
                        ));
                        if self.view != View::Changes && ui.small_button("Show conflicts").clicked()
                        {
                            actions.push(Action::ShowConflicts);
                        }
                    } else {
                        ui.weak(match kind {
                            OpKind::Merge => "All conflicts are resolved. Commit the merge to finish.",
                            OpKind::Rebase => {
                                "Nothing left to resolve. Amend the commit if you need to, then continue."
                            }
                            _ => "Nothing left to resolve. Continue to finish.",
                        });
                    }
                });
            });
        ui.add_space(4.0);
        self.actions.extend(actions);
    }

    /// Right pane of the Changes view for a conflicted file.
    pub(super) fn merge_view(&mut self, ui: &mut Ui) {
        let Some(view) = &mut self.merge else {
            diff::placeholder(ui, true, "");
            return;
        };
        let mut actions = Vec::new();
        let path = view.doc.path.clone();
        let kind = view.doc.kind;

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("U")
                    .monospace()
                    .color(widgets::status_color('U')),
            );
            ui.label(RichText::new(&path).strong());
            ui.weak(kind.label());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button("Whole file", |ui| {
                    whole_file_menu(ui, &path, kind, &mut actions);
                });
            });
        });

        match &view.doc.segments {
            Err(reason) => {
                ui.separator();
                ui.add_space(6.0);
                ui.label(reason.as_str());
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| whole_file_buttons(ui, &path, kind, &mut actions));
            }
            Ok(_) if view.choices.is_empty() => {
                ui.separator();
                ui.add_space(6.0);
                ui.label("No conflict markers are left in this file.");
                if ui.button("Mark as resolved").clicked() {
                    actions.push(Action::Op(GitOp::Resolve {
                        path: path.clone(),
                        content: None,
                    }));
                }
            }
            Ok(segments) => {
                let total = view.choices.len();
                let resolved = view.choices.iter().filter(|c| c.is_some()).count();
                ui.horizontal(|ui| {
                    ui.label(format!("{resolved} of {total} resolved"));
                    ui.separator();
                    if ui
                        .small_button("All ours")
                        .on_hover_text("Take our side in every conflict")
                        .clicked()
                    {
                        view.choices.fill(Some(Choice::Ours));
                        view.editing = None;
                    }
                    if ui
                        .small_button("All theirs")
                        .on_hover_text("Take their side in every conflict")
                        .clicked()
                    {
                        view.choices.fill(Some(Choice::Theirs));
                        view.editing = None;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let rendered = conflict::render(segments, &view.choices);
                        let save = Button::new(RichText::new("Save and mark resolved").strong());
                        let response = ui
                            .add_enabled(rendered.is_some(), save)
                            .on_hover_text("Write the result to the file and stage it (git add)")
                            .on_disabled_hover_text("Resolve every conflict first");
                        if let (true, Some(text)) = (response.clicked(), rendered) {
                            actions.push(Action::Op(GitOp::Resolve {
                                path: path.clone(),
                                content: Some(text.into_bytes()),
                            }));
                        }
                    });
                });
                ui.separator();
                ScrollArea::vertical()
                    .id_salt(("merge_view", &path))
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let mut index = 0;
                        let last = segments.len().saturating_sub(1);
                        for (n, segment) in segments.iter().enumerate() {
                            match segment {
                                Segment::Text(text) => text_block(ui, text, n == 0, n == last),
                                Segment::Conflict(c) => {
                                    conflict_block(
                                        ui,
                                        index,
                                        total,
                                        c,
                                        &mut view.choices[index],
                                        &mut view.editing,
                                    );
                                    index += 1;
                                }
                            }
                        }
                        ui.add_space(8.0);
                    });
            }
        }
        self.actions.extend(actions);
    }
}

/// `Rebasing main onto 1a2b3c4 · step 2 of 5`
fn describe(op: &InProgress) -> String {
    let commit = || {
        op.commit
            .as_ref()
            .map(|(id, summary)| format!("{} {summary}", &id.to_string()[..7]))
            .unwrap_or_else(|| "a commit".to_owned())
    };
    match op.kind {
        OpKind::Merge => match &op.title {
            Some(title) => format!("Merge in progress: {title}"),
            None => format!("Merging {}", commit()),
        },
        OpKind::Rebase => {
            let mut text = format!("Rebasing {}", op.branch.as_deref().unwrap_or("HEAD"));
            if let Some(onto) = op.onto {
                text.push_str(&format!(" onto {}", &onto.to_string()[..7]));
            }
            if let Some((step, total)) = op.step {
                text.push_str(&format!(" · step {step} of {total}"));
            }
            if let Some((id, summary)) = &op.commit {
                text.push_str(&format!(" · at {} {summary}", &id.to_string()[..7]));
            }
            text
        }
        OpKind::CherryPick => format!("Cherry-picking {}", commit()),
        OpKind::Revert => format!("Reverting {}", commit()),
        OpKind::ApplyMailbox => "Applying patches (git am)".to_owned(),
    }
}

fn abort(kind: OpKind) -> Confirm {
    let noun = kind.noun();
    Confirm {
        title: format!("Abort the {noun}?"),
        message: format!(
            "Stop the {noun} and go back to how things were before it started? Conflict resolutions made so far are lost."
        ),
        confirm_label: format!("Abort {noun}"),
        op: GitOp::Abort(kind),
        toggle: None,
    }
}

fn delete_file(path: &str) -> Confirm {
    Confirm {
        title: "Delete file?".to_owned(),
        message: format!("Resolve the conflict by deleting {path} (git rm)?"),
        confirm_label: "Delete file".to_owned(),
        op: GitOp::ResolveDelete(path.to_owned()),
        toggle: None,
    }
}

/// The "Conflicts" list at the top of the Changes view.
pub(super) fn conflict_section(
    ui: &mut Ui,
    files: &[ConflictedFile],
    selected: Option<&str>,
    actions: &mut Vec<Action>,
) {
    ui.label(
        RichText::new(format!("Conflicts ({})", files.len()))
            .strong()
            .color(widgets::status_color('U')),
    );
    for file in files {
        let (response, _) = egui::Sides::new().shrink_left().truncate().show(
            ui,
            |ui| {
                ui.label(
                    RichText::new("U")
                        .monospace()
                        .color(widgets::status_color('U')),
                );
                ui.add(
                    Button::selectable(selected == Some(file.path.as_str()), &file.path).truncate(),
                )
                .on_hover_text(format!("{} — {}", file.path, file.kind.label()))
            },
            |ui| ui.weak(file.kind.label()),
        );
        if response.clicked() {
            actions.push(Action::SelectConflict(file.path.clone()));
        }
        response.context_menu(|ui| {
            whole_file_menu(ui, &file.path, file.kind, actions);
            ui.separator();
            if ui.button("Copy path").clicked() {
                ui.ctx().copy_text(file.path.clone());
                ui.close();
            }
        });
    }
}

fn whole_file_menu(ui: &mut Ui, path: &str, kind: ConflictKind, actions: &mut Vec<Action>) {
    let resolve_side = |side| {
        Action::Op(GitOp::ResolveSide {
            path: path.to_owned(),
            side,
        })
    };
    if kind.has_ours()
        && ui
            .button("Take ours (whole file)")
            .on_hover_text("Use our version of the file, dropping their changes to it")
            .clicked()
    {
        actions.push(resolve_side(Side::Ours));
        ui.close();
    }
    if kind.has_theirs()
        && ui
            .button("Take theirs (whole file)")
            .on_hover_text("Use their version of the file, dropping our changes to it")
            .clicked()
    {
        actions.push(resolve_side(Side::Theirs));
        ui.close();
    }
    if ui
        .button("Mark as resolved")
        .on_hover_text("Stage the file exactly as it is now (git add)")
        .clicked()
    {
        actions.push(Action::Op(GitOp::Resolve {
            path: path.to_owned(),
            content: None,
        }));
        ui.close();
    }
    if ui.button("Delete file…").clicked() {
        actions.push(Action::confirm(delete_file(path)));
        ui.close();
    }
}

/// Buttons for conflicts that can't be merged line by line.
fn whole_file_buttons(ui: &mut Ui, path: &str, kind: ConflictKind, actions: &mut Vec<Action>) {
    let side = |side| {
        Action::Op(GitOp::ResolveSide {
            path: path.to_owned(),
            side,
        })
    };
    match kind {
        ConflictKind::DeletedByUs => {
            if ui.button("Keep their version").clicked() {
                actions.push(side(Side::Theirs));
            }
        }
        ConflictKind::DeletedByThem => {
            if ui.button("Keep our version").clicked() {
                actions.push(side(Side::Ours));
            }
        }
        ConflictKind::AddedByUs | ConflictKind::AddedByThem => {
            if ui.button("Keep the file").clicked() {
                actions.push(Action::Op(GitOp::Resolve {
                    path: path.to_owned(),
                    content: None,
                }));
            }
        }
        ConflictKind::BothModified | ConflictKind::BothAdded => {
            if ui.button("Take ours (whole file)").clicked() {
                actions.push(side(Side::Ours));
            }
            if ui.button("Take theirs (whole file)").clicked() {
                actions.push(side(Side::Theirs));
            }
            if ui
                .button("Mark as resolved")
                .on_hover_text("Stage the file exactly as it is now (git add)")
                .clicked()
            {
                actions.push(Action::Op(GitOp::Resolve {
                    path: path.to_owned(),
                    content: None,
                }));
            }
        }
        ConflictKind::BothDeleted => {}
    }
    if ui.button("Delete file…").clicked() {
        actions.push(Action::confirm(delete_file(path)));
    }
}

fn display(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\t', "    ")
}

/// Merged text between conflicts, folded down to a few lines of context.
fn text_block(ui: &mut Ui, text: &str, first: bool, last: bool) {
    let shown = display(text);
    let lines: Vec<&str> = shown.lines().collect();
    let line = |ui: &mut Ui, l: &str| {
        ui.label(
            RichText::new(if l.is_empty() { " " } else { l })
                .monospace()
                .weak(),
        );
    };
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        if lines.len() <= CONTEXT * 2 + 1 {
            for l in &lines {
                line(ui, l);
            }
            return;
        }
        let head = if first { 0 } else { CONTEXT };
        let tail = if last { 0 } else { CONTEXT };
        for l in &lines[..head] {
            line(ui, l);
        }
        ui.weak(format!("… {} unchanged lines", lines.len() - head - tail));
        for l in &lines[lines.len() - tail..] {
            line(ui, l);
        }
    });
}

/// Leading spaces every non-blank line of `texts` shares.
fn common_indent(texts: &[&str]) -> usize {
    texts
        .iter()
        .flat_map(|t| t.lines())
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0)
}

fn dedent(text: &str, indent: usize) -> String {
    text.lines()
        .map(|l| {
            l.get(indent..)
                .filter(|_| l.len() >= indent)
                .unwrap_or(l.trim_start())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Monospace text in a tinted box. `text` is display-ready (see `display`).
fn code_block(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, text: &str, color: Color32) {
    Frame::new()
        .fill(tint(color))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if text.is_empty() {
                ui.weak("(no lines)");
                return;
            }
            // Wrapped code is hard to read (and gets justified); scroll instead.
            ScrollArea::both()
                .id_salt(id)
                .max_height(260.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(text).monospace()).extend());
                });
        });
}

fn side_title(ui: &mut Ui, title: &str, label: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().color(color));
        if !label.is_empty() {
            ui.add(egui::Label::new(RichText::new(label).weak()).truncate());
        }
    });
}

fn conflict_block(
    ui: &mut Ui,
    index: usize,
    total: usize,
    conflict: &Conflict,
    choice: &mut Option<Choice>,
    editing: &mut Option<usize>,
) {
    ui.add_space(4.0);
    let border = if choice.is_some() { RESOLVED } else { PENDING };
    Frame::new()
        .stroke(Stroke::new(1.0, border.gamma_multiply(0.7)))
        .corner_radius(4.0)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.horizontal(|ui| {
                ui.strong(format!("Conflict {} of {total}", index + 1));
                match choice {
                    Some(c) => Chip::new(c.label(), RESOLVED).show(ui),
                    None => Chip::new("unresolved", PENDING).show(ui),
                };
            });
            // The shared indentation is hidden so narrow columns show the code.
            let ours = display(&conflict.ours);
            let theirs = display(&conflict.theirs);
            let base = conflict.base.as_deref().map(display);
            let indent = common_indent(&[&ours, &theirs, base.as_deref().unwrap_or("")]);
            let [ours, theirs] = [&ours, &theirs].map(|t| dedent(t, indent));
            let base = base.map(|b| dedent(&b, indent));
            let mut take = None;
            let mut sides = |ui: &mut Ui, side: usize| match side {
                0 => {
                    side_title(ui, "Ours", &conflict.ours_label, OURS);
                    code_block(ui, ("ours", index), &ours, OURS);
                    if ui.button("Take ours").clicked() {
                        take = Some(Choice::Ours);
                    }
                }
                1 => {
                    side_title(ui, "Base", "common ancestor", Color32::GRAY);
                    code_block(
                        ui,
                        ("base", index),
                        base.as_deref().unwrap_or(""),
                        Color32::GRAY,
                    );
                }
                _ => {
                    side_title(ui, "Theirs", &conflict.theirs_label, THEIRS);
                    code_block(ui, ("theirs", index), &theirs, THEIRS);
                    if ui.button("Take theirs").clicked() {
                        take = Some(Choice::Theirs);
                    }
                }
            };
            let order: &[usize] = if conflict.base.is_some() {
                &[0, 1, 2]
            } else {
                &[0, 2]
            };
            if ui.available_width() >= 720.0 {
                ui.columns(order.len(), |cols| {
                    for (col, &side) in cols.iter_mut().zip(order) {
                        sides(col, side);
                    }
                });
            } else {
                // Too narrow for columns: stack the sides.
                for &side in order {
                    sides(ui, side);
                }
            }
            if let Some(c) = take {
                *choice = Some(c);
                *editing = None;
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("Both: ours first").clicked() {
                    *choice = Some(Choice::OursThenTheirs);
                    *editing = None;
                }
                if ui.button("Both: theirs first").clicked() {
                    *choice = Some(Choice::TheirsThenOurs);
                    *editing = None;
                }
                if ui
                    .button("Edit…")
                    .on_hover_text("Write the result by hand")
                    .clicked()
                {
                    *choice = Some(Choice::Custom(conflict.edit_text(choice.as_ref())));
                    *editing = Some(index);
                }
                if choice.is_some() && ui.button("Reset").clicked() {
                    *choice = None;
                    *editing = None;
                }
            });
            if *editing == Some(index) {
                if let Some(Choice::Custom(text)) = choice {
                    ui.label(RichText::new("Result (editing)").weak());
                    let rows = text.lines().count().clamp(3, 20);
                    ui.add(
                        egui::TextEdit::multiline(text)
                            .id_salt(("conflict_edit", index))
                            .code_editor()
                            .desired_rows(rows)
                            .desired_width(f32::INFINITY),
                    );
                    if ui.button("Done editing").clicked() {
                        *editing = None;
                    }
                }
            } else if let Some(c) = choice {
                ui.label(RichText::new("Result").weak());
                let result = display(&conflict.resolve(c));
                let indent = indent.min(common_indent(&[&result]));
                code_block(ui, ("result", index), &dedent(&result, indent), RESOLVED);
            }
        });
    ui.add_space(4.0);
}
