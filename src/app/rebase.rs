//! Interactive rebase dialog: reorder commits by dragging (or with the arrow
//! buttons), choose what happens to each one, then let git replay them.

use eframe::egui::{self, Button, Color32, Id, Key, RichText, ScrollArea, Stroke, Ui};
use git2::Oid;

use super::{Action, GitloomApp, plural};
use crate::git::ops::GitOp;
use crate::git::rebase::{Action as Step, Plan};
use crate::ui::Chip;

const DANGER: Color32 = Color32::from_rgb(239, 83, 80);
const WARNING: Color32 = Color32::from_rgb(255, 179, 0);

pub(super) struct RebaseDialog {
    pub from: Oid,
    /// `None` while loading.
    pub plan: Option<Result<Plan, String>>,
    /// Highlighted row (index into `plan.items`); keys act on it.
    pub selected: Option<usize>,
    /// Uncommitted changes exist, so offer `--autostash`.
    pub dirty: bool,
}

/// Drag payload: the dragged row's index into `plan.items`.
struct RowDrag(usize);

/// Move `items[from]` to sit right after (`after == true`) or before `items[target]`.
fn move_item<T>(items: &mut Vec<T>, from: usize, target: usize, after: bool) -> usize {
    let item = items.remove(from);
    let mut to = if after { target + 1 } else { target };
    if from < to {
        to -= 1;
    }
    items.insert(to, item);
    to
}

impl GitloomApp {
    pub(super) fn rebase_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.rebase_dialog else {
            return;
        };
        let mut close = false;
        let mut start = None;
        let modal = egui::Modal::new(Id::unique("rebase_dialog")).show(ctx, |ui| {
            ui.set_width(780.0);
            ui.heading("Interactive rebase");
            match &mut dialog.plan {
                None => {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| ui.spinner());
                    ui.add_space(20.0);
                }
                Some(Err(problem)) => {
                    ui.add_space(6.0);
                    ui.colored_label(DANGER, problem.as_str());
                    ui.add_space(6.0);
                }
                Some(Ok(plan)) => {
                    let problem = plan_editor(ui, plan, &mut dialog.selected, dialog.dirty);
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let button = Button::new(RichText::new("Start rebase").strong());
                        if ui.add_enabled(problem.is_none(), button).clicked() {
                            start = Some(plan.clone());
                        }
                        close |= ui.button("Cancel").clicked();
                        if let Some(problem) = &problem {
                            ui.colored_label(WARNING, problem);
                        }
                    });
                    return;
                }
            }
            ui.add_space(6.0);
            close |= ui.button("Cancel").clicked();
        });
        // Escape cancels, but a stray click next to the dialog must not throw
        // away a carefully arranged list.
        close |= modal.is_top_modal
            && !modal.any_popup_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
        if let Some(plan) = start {
            self.actions
                .push(Action::Op(GitOp::InteractiveRebase(plan)));
            close = true;
        }
        if close {
            self.rebase_dialog = None;
        }
    }
}

/// The editable list. Returns why the plan can't start yet, if it can't.
fn plan_editor(
    ui: &mut Ui,
    plan: &mut Plan,
    selected: &mut Option<usize>,
    dirty: bool,
) -> Option<String> {
    let n = plan.items.len();
    ui.label(format!(
        "{} on {}, newest at the top. Drag a row by its handle or use the arrows to reorder.",
        plural(n, "commit", "commits"),
        plan.branch.as_deref().unwrap_or("a detached HEAD")
    ));
    handle_keys(ui, plan, selected);

    // The commit each squash/fixup melds into: the nearest older one that is kept.
    let target: Vec<Option<String>> = (0..n)
        .map(|i| {
            plan.items[..i]
                .iter()
                .rev()
                .find(|it| it.action != Step::Drop)
                .map(|it| it.short_id.clone())
        })
        .collect();

    let mut moved = None;
    ui.add_space(4.0);
    egui::Frame::new()
        .stroke(Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(4.0)
        .inner_margin(4.0)
        .show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("rebase_rows")
                .max_height(430.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for display in 0..n {
                        let i = n - 1 - display;
                        let row =
                            ui.scope(|ui| row_ui(ui, plan, i, n, selected, &target, &mut moved));
                        let rect = row.response.rect;
                        if let Some(drag) = row.response.dnd_hover_payload::<RowDrag>() {
                            // Upper half: drop above (newer), lower half: below (older).
                            let pointer = ui.input(|i| i.pointer.interact_pos());
                            let above = pointer.is_some_and(|p| p.y < rect.center().y);
                            if drag.0 != i {
                                let y = if above { rect.top() } else { rect.bottom() };
                                ui.painter().hline(
                                    rect.x_range(),
                                    y,
                                    Stroke::new(2.0, ui.visuals().selection.stroke.color),
                                );
                            }
                            if let Some(drag) = row.response.dnd_release_payload::<RowDrag>()
                                && drag.0 != i
                            {
                                moved = Some((drag.0, i, above));
                            }
                        }
                        if plan.items[i].action == Step::Reword {
                            reword_editor(ui, plan, i);
                        }
                    }
                });
        });
    if let Some((from, target, above)) = moved {
        // "Above" in the newest-first list means "after" in todo order.
        let to = move_item(&mut plan.items, from, target, above);
        *selected = Some(to);
    }

    ui.add_space(2.0);
    ui.weak("Keys: P pick · R reword · E edit · S squash · F fixup · D drop · Alt+↑/↓ move");
    let pushed = plan.pushed_count();
    if pushed > 0 {
        ui.colored_label(
            WARNING,
            format!(
                "{} already on {}. After rebasing you'll have to force-push.",
                plural(pushed, "commit is", "commits are"),
                plan.upstream.as_deref().unwrap_or("the remote")
            ),
        );
    }
    if dirty {
        ui.checkbox(
            &mut plan.autostash,
            "Stash my uncommitted changes while rebasing (--autostash)",
        );
    }

    plan.validate().err().or_else(|| {
        plan.is_unchanged()
            .then(|| "Change an action or the order first.".to_owned())
    })
}

fn row_ui(
    ui: &mut Ui,
    plan: &mut Plan,
    i: usize,
    n: usize,
    selected: &mut Option<usize>,
    target: &[Option<String>],
    moved: &mut Option<(usize, usize, bool)>,
) {
    let item_id = plan.items[i].id;
    let is_selected = *selected == Some(i);
    // The right-hand side can't borrow the plan while the left edits it.
    let pushed = plan.items[i].pushed;
    let melds = plan.items[i].action.melds();
    egui::Sides::new().shrink_left().truncate().show(
        ui,
        |ui| {
            ui.dnd_drag_source(
                Id::unique(("rebase_drag", item_id)),
                RowDrag(i),
                drag_handle,
            )
            .response
            .on_hover_text("Drag to reorder");
            let item = &mut plan.items[i];
            egui::ComboBox::from_id_salt(("rebase_action", item_id))
                .width(76.0)
                .selected_text(item.action.keyword())
                .show_ui(ui, |ui| {
                    for step in Step::ALL {
                        ui.selectable_value(&mut item.action, step, step.keyword())
                            .on_hover_text(step.help());
                    }
                });
            ui.label(RichText::new(&item.short_id).monospace().weak());
            let mut text = RichText::new(&item.summary);
            match item.action {
                Step::Drop => text = text.strikethrough().weak(),
                Step::Squash | Step::Fixup => text = text.weak(),
                _ => {}
            }
            let summary = ui
                .add(Button::selectable(is_selected, text).truncate())
                .on_hover_text(item.action.help());
            if summary.clicked() {
                *selected = Some(i);
            }
        },
        |ui| {
            let down = ui
                .add_enabled(i > 0, Button::new("↓").small())
                .on_hover_text("Move down (older) · Alt+↓");
            if down.clicked() {
                *moved = Some((i, i - 1, false));
            }
            let up = ui
                .add_enabled(i + 1 < n, Button::new("↑").small())
                .on_hover_text("Move up (newer) · Alt+↑");
            if up.clicked() {
                *moved = Some((i, i + 1, true));
            }
            if pushed {
                Chip::new("pushed", WARNING)
                    .show(ui)
                    .on_hover_text("Already on the upstream branch");
            }
            if melds {
                match &target[i] {
                    Some(into) => ui.weak(format!("↓ into {into}")),
                    None => ui.colored_label(DANGER, "nothing to meld into"),
                };
            }
        },
    );
}

/// Six dots to grab a row by (painted: no font has the braille glyph).
fn drag_handle(ui: &mut Ui) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Reorder"));
    let color = if response.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    for row in 0..3 {
        for col in 0..2 {
            let center =
                rect.left_top() + egui::vec2(3.0 + col as f32 * 4.0, 4.0 + row as f32 * 4.0);
            ui.painter().circle_filled(center, 1.2, color);
        }
    }
}

fn reword_editor(ui: &mut Ui, plan: &mut Plan, i: usize) {
    let item = &mut plan.items[i];
    ui.horizontal(|ui| {
        ui.add_space(28.0);
        ui.add(
            egui::TextEdit::multiline(&mut item.message)
                .id_salt(("rebase_message", item.id))
                .hint_text("New commit message")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
    });
}

/// git's todo-list letters act on the highlighted row; Alt+arrows move it.
fn handle_keys(ui: &mut Ui, plan: &mut Plan, selected: &mut Option<usize>) {
    // Typing a reword message must not change actions.
    if ui.ctx().text_edit_focused() {
        return;
    }
    let n = plan.items.len();
    let Some(sel) = selected.filter(|&s| s < n) else {
        return;
    };
    let (step, alt, up, down) = ui.input(|input| {
        let step = [
            (Key::P, Step::Pick),
            (Key::R, Step::Reword),
            (Key::E, Step::Edit),
            (Key::S, Step::Squash),
            (Key::F, Step::Fixup),
            (Key::D, Step::Drop),
        ]
        .into_iter()
        .find(|(key, _)| input.key_pressed(*key) && input.modifiers.is_none())
        .map(|(_, step)| step);
        (
            step,
            input.modifiers.alt,
            input.key_pressed(Key::ArrowUp),
            input.key_pressed(Key::ArrowDown),
        )
    });
    if let Some(step) = step {
        plan.items[sel].action = step;
    }
    // The list is newest first: "up" is towards the end of the todo order.
    match (alt, up, down) {
        (true, true, _) if sel + 1 < n => {
            plan.items.swap(sel, sel + 1);
            *selected = Some(sel + 1);
        }
        (true, _, true) if sel > 0 => {
            plan.items.swap(sel, sel - 1);
            *selected = Some(sel - 1);
        }
        (false, true, _) if sel + 1 < n => *selected = Some(sel + 1),
        (false, _, true) if sel > 0 => *selected = Some(sel - 1),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::move_item;

    #[test]
    fn moving_rows() {
        let mut v = vec!['a', 'b', 'c', 'd'];
        assert_eq!(move_item(&mut v, 0, 2, true), 2);
        assert_eq!(v, ['b', 'c', 'a', 'd']);
        assert_eq!(move_item(&mut v, 3, 0, false), 0);
        assert_eq!(v, ['d', 'b', 'c', 'a']);
        assert_eq!(move_item(&mut v, 1, 2, false), 1);
        assert_eq!(v, ['d', 'b', 'c', 'a']);
        assert_eq!(move_item(&mut v, 2, 0, true), 1);
        assert_eq!(v, ['d', 'c', 'b', 'a']);
    }
}
