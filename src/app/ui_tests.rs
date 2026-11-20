//! End-to-end UI tests: drive the real app headlessly with egui_kittest,
//! clicking widgets by their accessibility labels, against real repositories.

use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, Node};

use super::{ForgeState, GitloomApp, View};
use crate::forge::{CheckSummary, ForgeSnapshot, PullRequest, ReviewState};
use crate::git::ops::{git, git_bytes};

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gitloom-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "gitloom test"],
        &["config", "user.email", "test@example.invalid"],
        &["config", "commit.gpgsign", "false"],
        &["config", "core.autocrlf", "false"],
    ] {
        git(&dir, args, None).unwrap();
    }
    dir
}

fn commit(dir: &Path, file: &str, content: &str, message: &str) {
    std::fs::write(dir.join(file), content).unwrap();
    git(dir, &["add", "-A"], None).unwrap();
    git(dir, &["commit", "-q", "-m", message], None).unwrap();
}

fn numbered(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("line {i}")).collect()
}

fn join(lines: &[String]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

fn index_content(dir: &Path, file: &str) -> String {
    String::from_utf8(git_bytes(dir, &["show", &format!(":{file}")]).unwrap()).unwrap()
}

fn open(dir: &Path) -> Harness<'static, GitloomApp> {
    let dir = dir.to_path_buf();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        // The CJK fallback font is best effort; don't fail on machines without one.
        .allow_missing_glyphs()
        .build_eframe(move |cc| {
            crate::fonts::install(&cc.egui_ctx);
            GitloomApp::new(cc.egui_ctx.clone(), Some(dir))
        });
    wait_until(&mut harness, "the repository to load", |app| {
        app.snapshot.is_some() && app.status.is_some()
    });
    harness
}

/// A few frames: clicks turn into actions, actions into new UI.
fn settle(harness: &mut Harness<'_, GitloomApp>) {
    for _ in 0..3 {
        harness.step();
    }
}

/// Run frames until background jobs have produced the expected state.
fn wait_until(
    harness: &mut Harness<'_, GitloomApp>,
    what: &str,
    done: impl Fn(&GitloomApp) -> bool,
) {
    for _ in 0..500 {
        harness.step();
        if done(harness.state()) {
            // One more frame so the UI reflects the new state.
            harness.step();
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let message = harness.state().message.as_ref().map(|m| m.text.clone());
    panic!("timed out waiting for {what} (last message: {message:?})");
}

#[test]
fn stage_a_hunk_then_a_single_line() {
    let dir = temp_repo("stage");
    let mut lines = numbered(20);
    commit(&dir, "f.txt", &join(&lines), "base");
    lines[1] = "line 2 changed".into();
    lines[16] = "line 17 changed".into();
    lines.insert(17, "line 17b added".into());
    std::fs::write(dir.join("f.txt"), join(&lines)).unwrap();

    let mut harness = open(&dir);
    harness.get_by_label_contains("Changes").click();
    settle(&mut harness);
    harness.get_by_label("f.txt").click();
    wait_until(&mut harness, "the diff", |app| app.change_diff.is_some());

    // The first hunk's header button stages exactly that hunk.
    harness
        .get_all_by_label("Stage hunk")
        .next()
        .unwrap()
        .click();
    wait_until(&mut harness, "the hunk to be staged", |app| {
        app.status.as_ref().is_some_and(|s| !s.staged.is_empty())
            && app.change_diff.as_ref().is_some_and(|d| d.hunks.len() == 1)
    });
    let staged = index_content(&dir, "f.txt");
    assert!(staged.contains("line 2 changed") && !staged.contains("line 17 changed"));

    // Pick a single added line of the remaining hunk and stage only that.
    harness.get_by_label("+line 17b added").click();
    settle(&mut harness);
    assert_eq!(harness.state().diff_selection.len(), 1);
    harness.get_by_label("Stage lines").click();
    wait_until(&mut harness, "the line to be staged", |_| {
        index_content(&dir, "f.txt").contains("line 17b added")
    });
    let staged = index_content(&dir, "f.txt");
    assert!(
        !staged.contains("line 17 changed"),
        "only the picked line is staged"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discarding_a_hunk_asks_first() {
    let dir = temp_repo("discard");
    commit(&dir, "f.txt", &join(&numbered(5)), "base");
    std::fs::write(
        dir.join("f.txt"),
        "line 1\nline 2\nscratch\nline 3\nline 4\nline 5\n",
    )
    .unwrap();

    let mut harness = open(&dir);
    harness.get_by_label_contains("Changes").click();
    settle(&mut harness);
    harness.get_by_label("f.txt").click();
    wait_until(&mut harness, "the diff", |app| app.change_diff.is_some());

    harness.get_by_label("Discard").click();
    settle(&mut harness);
    assert!(
        harness.state().confirm.is_some(),
        "a confirmation dialog opens"
    );
    assert!(
        std::fs::read_to_string(dir.join("f.txt"))
            .unwrap()
            .contains("scratch"),
        "nothing happens before confirming"
    );

    harness.get_by_label("Discard changes").click();
    wait_until(&mut harness, "the change to be discarded", |app| {
        app.status.as_ref().is_some_and(|s| s.is_clean())
    });
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).unwrap(),
        join(&numbered(5))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn search_selects_the_matching_commit() {
    let dir = temp_repo("search");
    commit(&dir, "a.txt", "1\n", "Add parser");
    commit(&dir, "a.txt", "2\n", "커밋 그래프 레이아웃 수정");
    commit(&dir, "a.txt", "3\n", "Update docs");

    let mut harness = open(&dir);
    let search = harness.get_by_role(egui::Role::TextInput);
    search.click();
    settle(&mut harness);
    harness
        .get_by_role(egui::Role::TextInput)
        .type_text("그래프");
    settle(&mut harness);

    let app = harness.state();
    assert_eq!(app.search_matches.len(), 1);
    let snapshot = app.snapshot.as_ref().unwrap();
    let expected = snapshot
        .commits
        .iter()
        .find(|c| c.summary.contains("그래프"))
        .unwrap()
        .id;
    assert_eq!(app.selected, Some(expected));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stash_from_the_changes_view_and_pop_from_the_sidebar() {
    let dir = temp_repo("stash");
    commit(&dir, "a.txt", "one\n", "base");
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();

    let mut harness = open(&dir);
    harness.get_by_label_contains("Changes").click();
    settle(&mut harness);
    harness.get_by_label("Stash…").click();
    settle(&mut harness);
    harness.get_by_label("Stash").click();
    wait_until(&mut harness, "the stash", |app| {
        app.snapshot.as_ref().is_some_and(|s| s.stashes.len() == 1)
            && app.status.as_ref().is_some_and(|s| s.is_clean())
    });

    harness.get_by_label_contains("stash@{0}").click_secondary();
    settle(&mut harness);
    harness.get_by_label("Pop (apply and drop)").click();
    wait_until(&mut harness, "the stash to be popped", |app| {
        app.snapshot.as_ref().is_some_and(|s| s.stashes.is_empty())
            && app.status.as_ref().is_some_and(|s| !s.is_clean())
    });
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "two\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_a_branch_from_the_sidebar() {
    let dir = temp_repo("delete");
    commit(&dir, "a.txt", "one\n", "base");
    git(&dir, &["branch", "topic"], None).unwrap();

    let mut harness = open(&dir);
    harness.get_by_label("topic").click_secondary();
    settle(&mut harness);
    harness.get_by_label("Delete branch…").click();
    settle(&mut harness);
    harness.get_by_label("Delete branch").click();
    wait_until(&mut harness, "the branch to be deleted", |app| {
        app.snapshot
            .as_ref()
            .is_some_and(|s| s.refs.iter().all(|r| r.name != "topic"))
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn commit_from_the_changes_view() {
    let dir = temp_repo("commit");
    commit(
        &dir, "a.txt", "one
", "base",
    );
    std::fs::write(
        dir.join("a.txt"),
        "two
",
    )
    .unwrap();

    let mut harness = open(&dir);
    harness.get_by_label_contains("Changes").click();
    settle(&mut harness);
    harness.get_by_label("Stage all").click();
    wait_until(&mut harness, "the file to be staged", |app| {
        app.status.as_ref().is_some_and(|s| s.staged.len() == 1)
    });
    harness.get_by_role(egui::Role::MultilineTextInput).click();
    settle(&mut harness);
    harness
        .get_by_role(egui::Role::MultilineTextInput)
        .type_text("Update a.txt (테스트)");
    settle(&mut harness);
    harness.get_by_label("Commit 1 file").click();
    wait_until(&mut harness, "the commit", |app| {
        app.snapshot.as_ref().is_some_and(|s| s.commits.len() == 2)
    });

    let app = harness.state();
    let newest = &app.snapshot.as_ref().unwrap().commits[0];
    assert_eq!(newest.summary, "Update a.txt (테스트)");
    assert!(app.commit_message.is_empty(), "the message box is cleared");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn create_a_branch_from_the_commit_menu() {
    let dir = temp_repo("branch");
    commit(
        &dir, "a.txt", "one
", "base",
    );

    let mut harness = open(&dir);
    harness
        .get_by_role_and_label(egui::Role::ListItem, "base")
        .click_secondary();
    settle(&mut harness);
    harness.get_by_label("Create branch here…").click();
    settle(&mut harness);
    harness
        .get_all_by_role(egui::Role::TextInput)
        .find(|node| node.is_focused())
        .expect("the dialog focuses its name field")
        .type_text("feature/ui-test");
    settle(&mut harness);
    harness.get_by_label("Create and switch").click();
    wait_until(&mut harness, "the new branch to be checked out", |app| {
        app.snapshot
            .as_ref()
            .is_some_and(|s| s.head.branch.as_deref() == Some("feature/ui-test"))
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pull_request_card_jumps_to_its_branch() {
    let dir = temp_repo("pr");
    commit(
        &dir, "a.txt", "one
", "base",
    );
    git(&dir, &["switch", "-q", "-c", "feature/login"], None).unwrap();
    commit(
        &dir,
        "a.txt",
        "two
",
        "Add login form",
    );
    git(&dir, &["switch", "-q", "main"], None).unwrap();

    let mut harness = open(&dir);
    // The real GitHub lookup fails here (no remote); wait for it, then inject a PR.
    wait_until(&mut harness, "the GitHub lookup to finish", |app| {
        !app.forge_loading
    });
    let target = harness
        .state()
        .snapshot
        .as_ref()
        .unwrap()
        .refs
        .iter()
        .find(|r| r.name == "feature/login")
        .unwrap()
        .target;
    {
        let app = harness.state_mut();
        app.forge = ForgeState::Loaded(ForgeSnapshot {
            repo: "octo/demo".into(),
            repo_url: "https://github.com/octo/demo".into(),
            pulls: vec![PullRequest {
                number: 7,
                title: "Login form".into(),
                author: "octocat".into(),
                head_ref: "feature/login".into(),
                base_ref: "main".into(),
                is_draft: false,
                cross_repo: false,
                url: "https://github.com/octo/demo/pull/7".into(),
                updated_at: None,
                review: ReviewState::Approved,
                checks: CheckSummary::default(),
            }],
            issues: Vec::new(),
        });
        app.recompute_pr_links();
    }
    settle(&mut harness);
    assert_eq!(
        harness.state().pr_for_ref.len(),
        1,
        "the PR is linked to its branch"
    );

    harness
        .get_by_role_and_label(egui::Role::ListItem, "#7 Login form")
        .click();
    settle(&mut harness);
    assert_eq!(harness.state().selected_pr, Some(7));
    harness.get_by_label("Show in graph").click();
    wait_until(&mut harness, "the PR's commit to be selected", |app| {
        app.selected == Some(target)
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// `main` and `topic` both change the middle line of f.txt.
fn diverged(name: &str) -> PathBuf {
    let dir = temp_repo(name);
    commit(&dir, "f.txt", "top\nmiddle\nbottom\n", "base");
    git(&dir, &["switch", "-q", "-c", "topic"], None).unwrap();
    commit(
        &dir,
        "f.txt",
        "top\nmiddle from topic\nbottom\n",
        "topic change",
    );
    git(&dir, &["switch", "-q", "main"], None).unwrap();
    commit(
        &dir,
        "f.txt",
        "top\nmiddle from main\nbottom\n",
        "main change",
    );
    dir
}

fn summaries(dir: &Path) -> Vec<String> {
    git(dir, &["log", "--format=%s", "--reverse"], None)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The clickable widget with this label: not a graph row (a list item) and
/// not the commit detail's summary text (a label).
fn button<'a>(harness: &'a Harness<'_, GitloomApp>, label: &'a str) -> Node<'a> {
    harness
        .get_all_by_label(label)
        .find(|n| {
            !matches!(
                n.accesskit_node().role(),
                egui::Role::ListItem | egui::Role::Label
            )
        })
        .unwrap_or_else(|| panic!("no button labelled {label:?}"))
}

#[test]
fn merge_from_the_sidebar_and_resolve_in_the_merge_view() {
    let dir = diverged("merge");
    let mut harness = open(&dir);
    harness.get_by_label("topic").click_secondary();
    settle(&mut harness);
    harness.get_by_label("Merge topic into main").click();
    wait_until(&mut harness, "the merge to stop on the conflict", |app| {
        app.merge.as_ref().is_some_and(|m| m.choices.len() == 1)
    });
    assert!(
        harness.state().view == View::Changes,
        "the conflict is shown right away"
    );
    harness.get_by_label_contains("Merge in progress");

    harness.get_by_label("Take theirs").click();
    settle(&mut harness);
    harness.get_by_label("Save and mark resolved").click();
    wait_until(&mut harness, "the file to be resolved", |app| {
        app.status.as_ref().is_some_and(|s| s.conflicted.is_empty())
    });
    harness.get_by_label("Commit merge").click();
    wait_until(&mut harness, "the merge commit", |app| {
        app.status.as_ref().is_some_and(|s| s.in_progress.is_none())
            && app
                .snapshot
                .as_ref()
                .is_some_and(|s| s.commits[0].parents.len() == 2)
    });
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).unwrap(),
        "top\nmiddle from topic\nbottom\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn abort_a_stopped_rebase_from_the_banner() {
    let dir = diverged("abort");
    git(&dir, &["switch", "-q", "topic"], None).unwrap();
    assert!(
        git(&dir, &["rebase", "main"], None).is_err(),
        "stops on the conflict"
    );

    let mut harness = open(&dir);
    harness.get_by_label_contains("Rebasing topic onto");
    harness.get_by_label("Abort rebase…").click();
    settle(&mut harness);
    harness.get_by_label("Abort rebase").click();
    wait_until(&mut harness, "the rebase to be aborted", |app| {
        app.status
            .as_ref()
            .is_some_and(|s| s.in_progress.is_none() && s.is_clean())
    });
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).unwrap(),
        "top\nmiddle from topic\nbottom\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn open_rebase_dialog(harness: &mut Harness<'_, GitloomApp>, from: &str) {
    harness
        .get_by_role_and_label(egui::Role::ListItem, from)
        .click_secondary();
    settle(harness);
    harness
        .get_by_label("Rebase interactively from here…")
        .click();
    wait_until(harness, "the rebase plan", |app| {
        app.rebase_dialog
            .as_ref()
            .is_some_and(|d| matches!(d.plan, Some(Ok(_))))
    });
}

fn plan_order(harness: &Harness<'_, GitloomApp>) -> Vec<String> {
    match &harness.state().rebase_dialog.as_ref().unwrap().plan {
        Some(Ok(plan)) => plan.items.iter().map(|i| i.summary.clone()).collect(),
        _ => Vec::new(),
    }
}

#[test]
fn interactive_rebase_with_keyboard_shortcuts() {
    let dir = temp_repo("rebase-keys");
    for word in ["one", "two", "three", "four"] {
        commit(&dir, &format!("{word}.txt"), &format!("{word}\n"), word);
    }
    let mut harness = open(&dir);
    open_rebase_dialog(&mut harness, "two");
    assert_eq!(plan_order(&harness), ["two", "three", "four"]);

    // Highlight a row, then use git's todo letters: squash three, drop four.
    button(&harness, "three").click();
    settle(&mut harness);
    harness.key_press(egui::Key::S);
    settle(&mut harness);
    button(&harness, "four").click();
    settle(&mut harness);
    harness.key_press(egui::Key::D);
    settle(&mut harness);
    harness.get_by_label("Start rebase").click();
    wait_until(&mut harness, "the rebase to finish", |app| {
        app.rebase_dialog.is_none() && app.snapshot.as_ref().is_some_and(|s| s.commits.len() == 2)
    });

    assert_eq!(summaries(&dir), ["one", "two"]);
    let message = git(&dir, &["log", "-1", "--format=%B"], None).unwrap();
    assert!(
        message.contains("three"),
        "squash keeps both messages: {message}"
    );
    assert!(dir.join("three.txt").exists() && !dir.join("four.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn drag_a_commit_to_reorder_it() {
    let dir = temp_repo("rebase-drag");
    for word in ["one", "two", "three"] {
        commit(&dir, &format!("{word}.txt"), &format!("{word}\n"), word);
    }
    let mut harness = open(&dir);
    open_rebase_dialog(&mut harness, "two");

    // Newest first: the handle of "three" sits above the one of "two".
    let mut handles: Vec<egui::Rect> = harness
        .get_all_by_label("Reorder")
        .map(|n| n.rect())
        .collect();
    handles.sort_by(|a, b| a.top().total_cmp(&b.top()));
    assert_eq!(handles.len(), 2);
    let from = handles[0].center();
    let below_two = egui::pos2(from.x + 150.0, handles[1].bottom() - 1.0);
    harness.drag_at(from);
    harness.step();
    harness.hover_at(from + egui::vec2(0.0, 12.0));
    harness.step();
    harness.hover_at(below_two);
    harness.step();
    harness.step();
    // Release without the pointer leaving, so the row under it takes the drop.
    harness.event(egui::Event::PointerButton {
        pos: below_two,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    settle(&mut harness);
    assert_eq!(
        plan_order(&harness),
        ["three", "two"],
        "three is now the oldest"
    );

    harness.get_by_label("Start rebase").click();
    wait_until(&mut harness, "the rebase to finish", |app| {
        app.rebase_dialog.is_none()
            && app
                .snapshot
                .as_ref()
                .is_some_and(|s| s.commits.first().is_some_and(|c| c.summary == "two"))
    });
    assert_eq!(summaries(&dir), ["one", "three", "two"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_history_and_blame_from_the_commit_detail() {
    let dir = temp_repo("blame");
    commit(&dir, "f.txt", "alpha\nbeta\n", "add f");
    commit(&dir, "f.txt", "alpha\nBETA\n", "shout beta");
    commit(&dir, "g.txt", "x\n", "unrelated");

    let mut harness = open(&dir);
    harness
        .get_by_role_and_label(egui::Role::ListItem, "shout beta")
        .click();
    wait_until(&mut harness, "the commit detail", |app| {
        app.detail
            .as_ref()
            .is_some_and(|d| d.files.iter().any(|f| f.path == "f.txt"))
    });
    button(&harness, "f.txt").click_secondary();
    settle(&mut harness);
    harness.get_by_label("File history").click();
    wait_until(&mut harness, "the file history", |app| {
        app.file_view
            .as_ref()
            .is_some_and(|f| f.entry_diff.is_some())
    });
    let entries: Vec<String> = match &harness.state().file_view.as_ref().unwrap().history {
        Some(Ok((entries, _))) => entries.iter().map(|e| e.summary.clone()).collect(),
        _ => Vec::new(),
    };
    assert_eq!(entries, ["shout beta", "add f"]);

    harness.get_by_label("Blame").click();
    wait_until(&mut harness, "the blame", |app| {
        app.file_view
            .as_ref()
            .is_some_and(|f| matches!(f.blame, Some(Ok(_))))
    });
    harness
        .get_by_role_and_label(egui::Role::ListItem, "BETA")
        .click();
    settle(&mut harness);
    harness.get_by_label("Show in graph").click();
    settle(&mut harness);
    let app = harness.state();
    assert!(app.view == View::History);
    let expected = app
        .snapshot
        .as_ref()
        .unwrap()
        .commits
        .iter()
        .find(|c| c.summary == "shout beta")
        .unwrap()
        .id;
    assert_eq!(app.selected, Some(expected));
    let _ = std::fs::remove_dir_all(&dir);
}
