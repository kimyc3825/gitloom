//! Write operations, executed with the real `git` CLI.
//!
//! Going through the CLI (instead of libgit2) means hooks, commit signing,
//! credential helpers and every config option behave exactly as they do in
//! the terminal.

use std::io::Write as _;
use std::path::Path;
use std::process::{Output, Stdio};

use anyhow::{Context as _, Result, anyhow, bail};
use git2::{Oid, Repository, RepositoryState};

use super::patch::PatchMode;
use super::rebase::{self, Plan};
use super::repo::OpKind;
use crate::util;

/// One side of a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
}

impl Side {
    pub fn noun(self) -> &'static str {
        match self {
            Side::Ours => "ours",
            Side::Theirs => "theirs",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Side::Ours => "--ours",
            Side::Theirs => "--theirs",
        }
    }
}

#[derive(Debug, Clone)]
pub enum GitOp {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    StageAll,
    UnstageAll,
    /// Stage, unstage or discard part of a file (see `patch.rs`).
    ApplyPatch {
        patch: Vec<u8>,
        mode: PatchMode,
        /// What the patch covers, for messages: "hunk in src/main.rs".
        what: String,
    },
    /// Throw away working-tree changes of one file (deletes untracked files).
    DiscardFile {
        path: String,
        untracked: bool,
    },
    Commit {
        message: String,
        amend: bool,
    },
    Fetch,
    Pull,
    Push {
        set_upstream: Option<String>,
        /// `--force-with-lease`: overwrite the remote branch unless it moved.
        force: bool,
    },
    Switch(String),
    TrackRemote(String),
    Detach(Oid),
    CreateBranch {
        name: String,
        at: Oid,
    },
    DeleteBranch {
        name: String,
        force: bool,
    },
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },
    StashPush {
        message: String,
        include_untracked: bool,
    },
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),
    /// Merge a branch (or commit) into the checked-out branch.
    Merge(String),
    /// Rebase the checked-out branch onto a branch (or commit).
    RebaseOnto(String),
    CherryPick(Oid),
    Revert(Oid),
    /// Finish the merge, rebase, cherry-pick or revert in progress.
    Continue(OpKind),
    /// Leave out the commit a rebase/cherry-pick/revert stopped at.
    Skip(OpKind),
    /// Go back to where the operation started.
    Abort(OpKind),
    /// Mark a conflicted file resolved, after writing `content` into it.
    Resolve {
        path: String,
        content: Option<Vec<u8>>,
    },
    /// Resolve a conflict by taking one side's version of the whole file.
    ResolveSide {
        path: String,
        side: Side,
    },
    /// Resolve a conflict by deleting the file.
    ResolveDelete(String),
    InteractiveRebase(Plan),
}

/// How an operation ended, for the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Finished; git's output (its last line is shown).
    Done(String),
    /// Paused halfway, e.g. on conflicts: what the user should do next.
    Stopped(String),
}

impl GitOp {
    /// Short label for the status bar while/after running.
    pub fn label(&self) -> String {
        match self {
            GitOp::Stage(p) => format!("Stage {}", count(p)),
            GitOp::Unstage(p) => format!("Unstage {}", count(p)),
            GitOp::StageAll => "Stage all".to_owned(),
            GitOp::UnstageAll => "Unstage all".to_owned(),
            GitOp::ApplyPatch { mode, what, .. } => format!("{} {what}", mode.verb()),
            GitOp::DiscardFile {
                path,
                untracked: true,
            } => format!("Delete {path}"),
            GitOp::DiscardFile { path, .. } => format!("Discard changes in {path}"),
            GitOp::Commit { amend: true, .. } => "Amend commit".to_owned(),
            GitOp::Commit { .. } => "Commit".to_owned(),
            GitOp::Fetch => "Fetch".to_owned(),
            GitOp::Pull => "Pull".to_owned(),
            GitOp::Push { force: true, .. } => "Force push".to_owned(),
            GitOp::Push { .. } => "Push".to_owned(),
            GitOp::Switch(b) => format!("Switch to {b}"),
            GitOp::TrackRemote(b) => format!("Check out {b}"),
            GitOp::Detach(id) => format!("Check out {}", short(id)),
            GitOp::CreateBranch { name, .. } => format!("Create branch {name}"),
            GitOp::DeleteBranch { name, .. } => format!("Delete branch {name}"),
            GitOp::DeleteRemoteBranch { remote, branch } => format!("Delete {remote}/{branch}"),
            GitOp::StashPush { .. } => "Stash changes".to_owned(),
            GitOp::StashApply(i) => format!("Apply stash@{{{i}}}"),
            GitOp::StashPop(i) => format!("Pop stash@{{{i}}}"),
            GitOp::StashDrop(i) => format!("Drop stash@{{{i}}}"),
            GitOp::Merge(r) => format!("Merge {r}"),
            GitOp::RebaseOnto(r) => format!("Rebase onto {r}"),
            GitOp::CherryPick(id) => format!("Cherry-pick {}", short(id)),
            GitOp::Revert(id) => format!("Revert {}", short(id)),
            GitOp::Continue(OpKind::Merge) => "Commit merge".to_owned(),
            GitOp::Continue(kind) => format!("Continue {}", kind.noun()),
            GitOp::Skip(_) => "Skip commit".to_owned(),
            GitOp::Abort(kind) => format!("Abort {}", kind.noun()),
            GitOp::Resolve { path, .. } => format!("Resolve {path}"),
            GitOp::ResolveSide { path, side } => format!("Take {} for {path}", side.noun()),
            GitOp::ResolveDelete(path) => format!("Delete {path}"),
            GitOp::InteractiveRebase(plan) => {
                format!(
                    "Interactive rebase ({})",
                    plural(plan.items.len(), "commit")
                )
            }
        }
    }

    /// Whether refs, stashes or commits may change (the graph must reload),
    /// as opposed to the index/working tree only.
    pub fn changes_history(&self) -> bool {
        !matches!(
            self,
            GitOp::Stage(_)
                | GitOp::Unstage(_)
                | GitOp::StageAll
                | GitOp::UnstageAll
                | GitOp::ApplyPatch { .. }
                | GitOp::DiscardFile { .. }
                | GitOp::Resolve { .. }
                | GitOp::ResolveSide { .. }
                | GitOp::ResolveDelete(_)
        )
    }

    /// Whether the operation talks to a remote (pull requests may change).
    pub fn touches_remote(&self) -> bool {
        matches!(
            self,
            GitOp::Fetch | GitOp::Pull | GitOp::Push { .. } | GitOp::DeleteRemoteBranch { .. }
        )
    }

    /// Operations that can pause halfway and leave work for the user.
    fn may_stop(&self) -> bool {
        matches!(
            self,
            GitOp::Merge(_)
                | GitOp::RebaseOnto(_)
                | GitOp::CherryPick(_)
                | GitOp::Revert(_)
                | GitOp::Continue(_)
                | GitOp::Skip(_)
                | GitOp::InteractiveRebase(_)
                | GitOp::StashApply(_)
                | GitOp::StashPop(_)
        )
    }

    pub fn run(&self, root: &Path) -> Result<Outcome> {
        let result = self.exec(root);
        if !self.may_stop() {
            return result.map(Outcome::Done);
        }
        // Conflicts are an expected stop, not a failure: git left markers to resolve.
        let stash = matches!(self, GitOp::StashApply(_) | GitOp::StashPop(_));
        let conflicts = conflict_count(root);
        if conflicts > 0 {
            let mut text = format!(
                "conflicts in {}. Resolve them in the Changes tab{}.",
                plural(conflicts, "file"),
                if stash { "" } else { ", then continue" }
            );
            if matches!(self, GitOp::StashPop(_)) {
                text.push_str(" The stash was kept.");
            }
            return Ok(Outcome::Stopped(text));
        }
        let output = result?;
        if !stash && rebase_in_progress(root) {
            return Ok(Outcome::Stopped(
                "paused for editing. Change and amend the commit, then continue.".to_owned(),
            ));
        }
        Ok(Outcome::Done(output))
    }

    fn exec(&self, root: &Path) -> Result<String> {
        match self {
            GitOp::Stage(paths) => git(root, &with_paths(&["add", "-A", "--"], paths), None),
            GitOp::Unstage(paths) => git(root, &with_paths(&["reset", "-q", "--"], paths), None),
            GitOp::StageAll => git(root, &["add", "-A"], None),
            GitOp::UnstageAll => git(root, &["reset", "-q"], None),
            GitOp::ApplyPatch { patch, mode, .. } => git(root, mode.git_args(), Some(patch)),
            GitOp::DiscardFile {
                path,
                untracked: true,
            } => git(root, &["clean", "-f", "-q", "--", path], None),
            GitOp::DiscardFile { path, .. } => {
                git(root, &["restore", "--worktree", "--", path], None)
            }
            GitOp::Commit { message, amend } => {
                let mut args = vec!["commit"];
                if *amend {
                    args.push("--amend");
                }
                if message.trim().is_empty() {
                    if !*amend {
                        bail!("commit message is empty");
                    }
                    args.push("--no-edit");
                    git(root, &args, None)
                } else {
                    args.extend(["-F", "-"]);
                    git(root, &args, Some(message.as_bytes()))
                }
            }
            GitOp::Fetch => git(root, &["fetch", "--all", "--prune"], None),
            GitOp::Pull => git(root, &["pull", "--ff-only"], None),
            GitOp::Push {
                set_upstream,
                force,
            } => {
                let mut args = vec!["push"];
                if *force {
                    args.push("--force-with-lease");
                }
                if let Some(remote) = set_upstream {
                    args.extend(["-u", remote, "HEAD"]);
                }
                git(root, &args, None)
            }
            GitOp::Switch(branch) => git(root, &["switch", branch], None),
            GitOp::TrackRemote(remote_branch) => {
                git(root, &["switch", "--track", remote_branch], None)
            }
            GitOp::Detach(id) => git(root, &["switch", "--detach", &id.to_string()], None),
            GitOp::CreateBranch { name, at } => {
                git(root, &["switch", "-c", name, &at.to_string()], None)
            }
            GitOp::DeleteBranch { name, force } => {
                let flag = if *force { "-D" } else { "-d" };
                git(root, &["branch", flag, name], None)
            }
            GitOp::DeleteRemoteBranch { remote, branch } => {
                git(root, &["push", remote, "--delete", branch], None)
            }
            GitOp::StashPush {
                message,
                include_untracked,
            } => {
                let mut args = vec!["stash", "push"];
                if *include_untracked {
                    args.push("--include-untracked");
                }
                if !message.trim().is_empty() {
                    args.extend(["-m", message.trim()]);
                }
                git(root, &args, None)
            }
            GitOp::StashApply(i) => git(root, &["stash", "apply", &stash_ref(*i)], None),
            GitOp::StashPop(i) => git(root, &["stash", "pop", &stash_ref(*i)], None),
            GitOp::StashDrop(i) => git(root, &["stash", "drop", &stash_ref(*i)], None),
            GitOp::Merge(target) => git(root, &["merge", "--no-edit", target], None),
            GitOp::RebaseOnto(target) => git(root, &["rebase", target], None),
            GitOp::CherryPick(id) => git(root, &["cherry-pick", &id.to_string()], None),
            GitOp::Revert(id) => git(root, &["revert", "--no-edit", &id.to_string()], None),
            GitOp::Continue(OpKind::Merge) => git(root, &["commit", "--no-edit"], None),
            GitOp::Continue(kind) => git(root, &[command_of(*kind), "--continue"], None),
            GitOp::Skip(OpKind::Merge) => bail!("a merge can't skip commits"),
            GitOp::Skip(kind) => git(root, &[command_of(*kind), "--skip"], None),
            GitOp::Abort(kind) => git(root, &[command_of(*kind), "--abort"], None),
            GitOp::Resolve { path, content } => {
                if let Some(content) = content {
                    std::fs::write(root.join(path), content)
                        .with_context(|| format!("cannot write {path}"))?;
                }
                git(root, &["add", "--", path], None)
            }
            GitOp::ResolveSide { path, side } => {
                git(root, &["checkout", side.flag(), "--", path], None)?;
                git(root, &["add", "--", path], None)
            }
            GitOp::ResolveDelete(path) => git(root, &["rm", "-q", "--", path], None),
            GitOp::InteractiveRebase(plan) => interactive_rebase(root, plan),
        }
    }
}

fn command_of(kind: OpKind) -> &'static str {
    match kind {
        OpKind::Merge => "merge",
        OpKind::Rebase => "rebase",
        OpKind::CherryPick => "cherry-pick",
        OpKind::Revert => "revert",
        OpKind::ApplyMailbox => "am",
    }
}

/// Run `git rebase -i` with gitloom's todo list in place of the editor.
fn interactive_rebase(root: &Path, plan: &Plan) -> Result<String> {
    if let Err(problem) = plan.validate() {
        bail!(problem);
    }
    if rebase_in_progress(root) {
        bail!("a rebase is already in progress; continue or abort it first");
    }
    // Lives in the git dir so reword messages survive a stop and --continue.
    let dir = Repository::open(root)?.path().join("gitloom");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let (todo, messages) = plan.script(&dir);
    for (path, text) in &messages {
        std::fs::write(path, text)?;
    }
    let todo_path = dir.join("rebase-todo");
    std::fs::write(&todo_path, todo)?;

    // git runs the "editor" through sh with the todo file as its argument.
    let editor = format!("cp {}", rebase::sh_quote(&todo_path));
    let onto = plan.onto.map(|o| o.to_string());
    let mut args = vec!["rebase", "--interactive"];
    if plan.autostash {
        args.push("--autostash");
    }
    args.push(onto.as_deref().unwrap_or("--root"));
    git_env(root, &args, None, &[("GIT_SEQUENCE_EDITOR", &editor)])
}

fn conflict_count(root: &Path) -> usize {
    let Ok(index) = Repository::open(root).and_then(|r| r.index()) else {
        return 0;
    };
    if !index.has_conflicts() {
        return 0;
    }
    index.conflicts().map_or(0, |c| c.count())
}

fn rebase_in_progress(root: &Path) -> bool {
    Repository::open(root).is_ok_and(|r| {
        matches!(
            r.state(),
            RepositoryState::Rebase
                | RepositoryState::RebaseInteractive
                | RepositoryState::RebaseMerge
                | RepositoryState::ApplyMailboxOrRebase
        )
    })
}

fn short(id: &Oid) -> String {
    id.to_string()[..7].to_owned()
}

fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

fn stash_ref(index: usize) -> String {
    format!("stash@{{{index}}}")
}

fn count(paths: &[String]) -> String {
    match paths {
        [one] => one.clone(),
        many => format!("{} files", many.len()),
    }
}

fn with_paths<'a>(base: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
    base.iter()
        .copied()
        .chain(paths.iter().map(String::as_str))
        .collect()
}

fn run_git(
    root: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
    env: &[(&str, &str)],
) -> Result<Output> {
    let mut cmd = util::command("git");
    cmd.arg("-C")
        .arg(root)
        .args(args)
        // Never block on a terminal prompt; credential helpers still work.
        .env("GIT_TERMINAL_PROMPT", "0")
        // No editor can be shown: accept prepared messages (merge, squash, revert).
        // `:` is git's built-in no-op editor.
        .env("GIT_EDITOR", ":")
        .envs(env.iter().copied())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow!("`git` was not found on PATH")
        } else {
            anyhow!("failed to run git: {e}")
        }
    })?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // Write from another thread so a chatty git can't deadlock on a full pipe.
        let input = input.to_vec();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }
    Ok(child.wait_with_output()?)
}

fn failure(args: &[&str], output: &Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        anyhow!("git {} failed ({})", args.join(" "), output.status)
    } else {
        anyhow!(stderr)
    }
}

/// Run `git <args>` in `root`, returning trimmed stdout (or stderr as the error).
pub fn git(root: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<String> {
    git_env(root, args, stdin, &[])
}

fn git_env(
    root: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
    env: &[(&str, &str)],
) -> Result<String> {
    let output = run_git(root, args, stdin, env)?;
    if !output.status.success() {
        return Err(failure(args, &output));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    // Many git commands report progress on stderr even on success.
    Ok(if stdout.is_empty() { stderr } else { stdout })
}

/// Exact stdout of a successful `git <args>`.
pub fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = run_git(root, args, None, &[])?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(failure(args, &output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo::{load_snapshot, load_status};

    fn temp_repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gitloom-test-{name}-{}", std::process::id()));
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

    fn commit_all(dir: &Path, message: &str) {
        git(dir, &["add", "-A"], None).unwrap();
        git(dir, &["commit", "-q", "-m", message], None).unwrap();
    }

    #[test]
    fn stage_commit_and_read_back() {
        let dir = temp_repo("ops");
        std::fs::write(dir.join("hello.txt"), "hello\n").unwrap();

        let status = load_status(&dir).unwrap();
        assert_eq!(status.unstaged.len(), 1);
        assert_eq!(status.unstaged[0].status, '?');

        GitOp::Stage(vec!["hello.txt".into()]).run(&dir).unwrap();
        let status = load_status(&dir).unwrap();
        assert_eq!(status.staged.len(), 1);
        assert_eq!(status.staged[0].status, 'A');

        GitOp::Commit {
            message: "첫 커밋\n\nbody".into(),
            amend: false,
        }
        .run(&dir)
        .unwrap();
        let snap = load_snapshot(&dir).unwrap();
        assert_eq!(snap.commits.len(), 1);
        assert_eq!(snap.commits[0].summary, "첫 커밋");
        assert_eq!(snap.head.branch.as_deref(), Some("main"));
        assert!(snap.head_ref().is_some());

        let detail = crate::git::repo::load_commit_detail(&dir, snap.commits[0].id).unwrap();
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].status, 'A');
        assert_eq!(detail.files[0].additions, 1);

        GitOp::CreateBranch {
            name: "feature/x".into(),
            at: snap.commits[0].id,
        }
        .run(&dir)
        .unwrap();
        let snap = load_snapshot(&dir).unwrap();
        assert_eq!(snap.head.branch.as_deref(), Some("feature/x"));
        assert_eq!(snap.refs.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_commit_message_is_rejected() {
        let dir = temp_repo("empty-msg");
        let err = GitOp::Commit {
            message: "  ".into(),
            amend: false,
        }
        .run(&dir)
        .unwrap_err();
        assert!(err.to_string().contains("empty"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stash_push_apply_pop_and_drop() {
        let dir = temp_repo("stash");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        commit_all(&dir, "base");
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        std::fs::write(dir.join("new.txt"), "untracked\n").unwrap();

        GitOp::StashPush {
            message: "작업 중".into(),
            include_untracked: true,
        }
        .run(&dir)
        .unwrap();
        assert!(load_status(&dir).unwrap().is_clean());
        let snap = load_snapshot(&dir).unwrap();
        assert_eq!(snap.stashes.len(), 1);
        assert!(snap.stashes[0].message.contains("작업 중"));
        // Stash commits are not part of the graph.
        assert_eq!(snap.commits.len(), 1);

        GitOp::StashApply(0).run(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "two\n");
        assert!(dir.join("new.txt").exists());
        assert_eq!(load_snapshot(&dir).unwrap().stashes.len(), 1);

        GitOp::StashDrop(0).run(&dir).unwrap();
        assert!(load_snapshot(&dir).unwrap().stashes.is_empty());

        GitOp::StashPush {
            message: String::new(),
            include_untracked: false,
        }
        .run(&dir)
        .unwrap();
        // Without --include-untracked the new file stays behind.
        assert_eq!(load_status(&dir).unwrap().unstaged.len(), 1);
        GitOp::StashPop(0).run(&dir).unwrap();
        assert!(load_snapshot(&dir).unwrap().stashes.is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "two\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_merged_and_unmerged_branches() {
        let dir = temp_repo("delete-branch");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        commit_all(&dir, "base");
        git(&dir, &["branch", "merged"], None).unwrap();
        git(&dir, &["switch", "-q", "-c", "unmerged"], None).unwrap();
        std::fs::write(dir.join("b.txt"), "two\n").unwrap();
        commit_all(&dir, "extra");
        git(&dir, &["switch", "-q", "main"], None).unwrap();

        GitOp::DeleteBranch {
            name: "merged".into(),
            force: false,
        }
        .run(&dir)
        .unwrap();
        let err = GitOp::DeleteBranch {
            name: "unmerged".into(),
            force: false,
        }
        .run(&dir)
        .unwrap_err();
        assert!(err.to_string().contains("not fully merged"), "{err}");
        GitOp::DeleteBranch {
            name: "unmerged".into(),
            force: true,
        }
        .run(&dir)
        .unwrap();

        let names: Vec<String> = load_snapshot(&dir)
            .unwrap()
            .refs
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(names, vec!["main".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discard_tracked_and_untracked_files() {
        let dir = temp_repo("discard-file");
        std::fs::write(dir.join("a.txt"), "original\n").unwrap();
        commit_all(&dir, "base");
        std::fs::write(dir.join("a.txt"), "edited\n").unwrap();
        std::fs::write(dir.join("scratch.txt"), "junk\n").unwrap();

        GitOp::DiscardFile {
            path: "a.txt".into(),
            untracked: false,
        }
        .run(&dir)
        .unwrap();
        assert_eq!(
            git_bytes(&dir, &["show", "HEAD:a.txt"]).unwrap(),
            std::fs::read(dir.join("a.txt")).unwrap()
        );

        GitOp::DiscardFile {
            path: "scratch.txt".into(),
            untracked: true,
        }
        .run(&dir)
        .unwrap();
        assert!(!dir.join("scratch.txt").exists());
        assert!(load_status(&dir).unwrap().is_clean());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write(dir: &Path, file: &str, content: &str) {
        std::fs::write(dir.join(file), content).unwrap();
    }

    fn read(dir: &Path, file: &str) -> String {
        std::fs::read_to_string(dir.join(file)).unwrap()
    }

    /// `main` and `topic` both change the middle line of f.txt.
    fn diverged(name: &str) -> std::path::PathBuf {
        let dir = temp_repo(name);
        write(&dir, "f.txt", "top\nmiddle\nbottom\n");
        commit_all(&dir, "base");
        git(&dir, &["switch", "-q", "-c", "topic"], None).unwrap();
        write(&dir, "f.txt", "top\nmiddle from topic\nbottom\n");
        commit_all(&dir, "topic change");
        git(&dir, &["switch", "-q", "main"], None).unwrap();
        write(&dir, "f.txt", "top\nmiddle from main\nbottom\n");
        commit_all(&dir, "main change");
        dir
    }

    fn summaries(dir: &Path) -> Vec<String> {
        git(dir, &["log", "--format=%s", "--reverse"], None)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn merge_conflict_resolved_and_committed() {
        use crate::git::conflict::{self, Choice, ConflictKind};
        use crate::git::repo::OpKind;

        let dir = diverged("merge");
        let outcome = GitOp::Merge("topic".into()).run(&dir).unwrap();
        assert!(
            matches!(outcome, Outcome::Stopped(ref t) if t.contains("1 file")),
            "{outcome:?}"
        );

        let status = load_status(&dir).unwrap();
        let state = status.in_progress.as_ref().expect("merge in progress");
        assert_eq!(state.kind, OpKind::Merge);
        assert!(state.title.as_deref().unwrap().contains("topic"));
        assert_eq!(status.conflicted.len(), 1);
        assert_eq!(status.conflicted[0].kind, ConflictKind::BothModified);
        assert!(
            status.unstaged.is_empty(),
            "conflicts are listed separately"
        );

        let doc = conflict::load(&dir, "f.txt", ConflictKind::BothModified);
        let segments = doc.segments.unwrap();
        assert_eq!(conflict::conflict_count(&segments), 1);
        let merged = conflict::render(&segments, &[Some(Choice::TheirsThenOurs)]).unwrap();
        assert_eq!(merged, "top\nmiddle from topic\nmiddle from main\nbottom\n");

        GitOp::Resolve {
            path: "f.txt".into(),
            content: Some(merged.clone().into_bytes()),
        }
        .run(&dir)
        .unwrap();
        assert!(load_status(&dir).unwrap().conflicted.is_empty());
        GitOp::Continue(OpKind::Merge).run(&dir).unwrap();

        let status = load_status(&dir).unwrap();
        assert!(status.in_progress.is_none() && status.is_clean());
        let snap = load_snapshot(&dir).unwrap();
        assert_eq!(snap.commits[0].parents.len(), 2, "a merge commit was made");
        assert_eq!(read(&dir, "f.txt"), merged);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebase_stops_on_conflict_then_continues() {
        use crate::git::conflict::{self, Choice, ConflictKind};
        use crate::git::repo::OpKind;

        let dir = diverged("rebase");
        git(&dir, &["switch", "-q", "topic"], None).unwrap();
        let outcome = GitOp::RebaseOnto("main".into()).run(&dir).unwrap();
        assert!(matches!(outcome, Outcome::Stopped(_)), "{outcome:?}");

        let status = load_status(&dir).unwrap();
        let state = status.in_progress.unwrap();
        assert_eq!(state.kind, OpKind::Rebase);
        assert_eq!(state.branch.as_deref(), Some("topic"));
        assert_eq!(state.step, Some((1, 1)));
        assert_eq!(state.commit.unwrap().1, "topic change");

        let segments = conflict::load(&dir, "f.txt", ConflictKind::BothModified)
            .segments
            .unwrap();
        let merged = conflict::render(&segments, &[Some(Choice::Theirs)]).unwrap();
        GitOp::Resolve {
            path: "f.txt".into(),
            content: Some(merged.into_bytes()),
        }
        .run(&dir)
        .unwrap();
        // GIT_EDITOR=: accepts the commit message instead of waiting for an editor.
        let outcome = GitOp::Continue(OpKind::Rebase).run(&dir).unwrap();
        assert!(matches!(outcome, Outcome::Done(_)), "{outcome:?}");
        assert!(load_status(&dir).unwrap().in_progress.is_none());
        assert_eq!(summaries(&dir), ["base", "main change", "topic change"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cherry_pick_conflict_can_be_aborted() {
        use crate::git::repo::OpKind;

        let dir = diverged("cherry");
        let topic = git(&dir, &["rev-parse", "topic"], None).unwrap();
        let outcome = GitOp::CherryPick(Oid::from_str(&topic).unwrap())
            .run(&dir)
            .unwrap();
        assert!(matches!(outcome, Outcome::Stopped(_)));
        let state = load_status(&dir).unwrap().in_progress.unwrap();
        assert_eq!(state.kind, OpKind::CherryPick);

        GitOp::Abort(OpKind::CherryPick).run(&dir).unwrap();
        let status = load_status(&dir).unwrap();
        assert!(status.in_progress.is_none() && status.is_clean());
        assert_eq!(read(&dir, "f.txt"), "top\nmiddle from main\nbottom\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn whole_file_and_delete_resolutions() {
        use crate::git::conflict::ConflictKind;

        let dir = temp_repo("resolve-side");
        write(&dir, "keep.txt", "base\n");
        write(&dir, "gone.txt", "base\n");
        commit_all(&dir, "base");
        git(&dir, &["switch", "-q", "-c", "topic"], None).unwrap();
        write(&dir, "keep.txt", "topic\n");
        git(&dir, &["rm", "-q", "gone.txt"], None).unwrap();
        commit_all(&dir, "topic");
        git(&dir, &["switch", "-q", "main"], None).unwrap();
        write(&dir, "keep.txt", "main\n");
        write(&dir, "gone.txt", "main edit\n");
        commit_all(&dir, "main");

        GitOp::Merge("topic".into()).run(&dir).unwrap();
        let status = load_status(&dir).unwrap();
        let kinds: Vec<(&str, ConflictKind)> = status
            .conflicted
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("gone.txt", ConflictKind::DeletedByThem),
                ("keep.txt", ConflictKind::BothModified)
            ]
        );

        GitOp::ResolveSide {
            path: "keep.txt".into(),
            side: Side::Theirs,
        }
        .run(&dir)
        .unwrap();
        assert_eq!(read(&dir, "keep.txt"), "topic\n");
        GitOp::ResolveDelete("gone.txt".into()).run(&dir).unwrap();
        assert!(!dir.join("gone.txt").exists());
        assert!(load_status(&dir).unwrap().conflicted.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Five commits, each adding one file: one.txt ... five.txt.
    fn five_commits(name: &str) -> (std::path::PathBuf, Vec<Oid>) {
        let dir = temp_repo(name);
        let mut ids = Vec::new();
        for word in ["one", "two", "three", "four", "five"] {
            write(&dir, &format!("{word}.txt"), &format!("{word}\n"));
            commit_all(&dir, word);
            ids.push(Oid::from_str(&git(&dir, &["rev-parse", "HEAD"], None).unwrap()).unwrap());
        }
        (dir, ids)
    }

    #[test]
    fn interactive_rebase_reorders_fixes_up_drops_and_rewords() {
        use crate::git::rebase::{self, Action};

        let (dir, ids) = five_commits("rebase-i");
        let mut plan = rebase::load(&dir, ids[1]).unwrap();
        let names: Vec<&str> = plan.items.iter().map(|i| i.summary.as_str()).collect();
        assert_eq!(names, ["two", "three", "four", "five"]);
        assert_eq!(plan.onto, Some(ids[0]));
        assert_eq!(plan.branch.as_deref(), Some("main"));

        // five first (reworded), then two with three folded in; four dropped.
        let five = plan.items.remove(3);
        plan.items.insert(0, five);
        plan.items[0].action = Action::Reword;
        plan.items[0].message = "five, reworded\n\nwith a body".into();
        plan.items[2].action = Action::Fixup; // three -> into two
        plan.items[3].action = Action::Drop; // four
        assert!(plan.validate().is_ok());
        assert!(!plan.is_unchanged());

        let outcome = GitOp::InteractiveRebase(plan).run(&dir).unwrap();
        assert!(matches!(outcome, Outcome::Done(_)), "{outcome:?}");
        assert_eq!(summaries(&dir), ["one", "five, reworded", "two"]);
        assert_eq!(
            git(&dir, &["log", "-1", "--skip=1", "--format=%B"], None).unwrap(),
            "five, reworded\n\nwith a body"
        );
        for (file, exists) in [
            ("one.txt", true),
            ("two.txt", true),
            ("three.txt", true),
            ("four.txt", false),
            ("five.txt", true),
        ] {
            assert_eq!(dir.join(file).exists(), exists, "{file}");
        }
        assert!(load_status(&dir).unwrap().is_clean());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interactive_rebase_squash_keeps_both_messages() {
        use crate::git::rebase::{self, Action};

        let (dir, ids) = five_commits("rebase-squash");
        let mut plan = rebase::load(&dir, ids[3]).unwrap();
        plan.items[1].action = Action::Squash; // five -> into four
        GitOp::InteractiveRebase(plan).run(&dir).unwrap();
        assert_eq!(summaries(&dir), ["one", "two", "three", "four"]);
        let message = git(&dir, &["log", "-1", "--format=%B"], None).unwrap();
        assert!(
            message.contains("four") && message.contains("five"),
            "{message}"
        );
        assert!(
            !message.contains('#'),
            "comment lines are stripped: {message}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interactive_rebase_edit_pauses_until_continue() {
        use crate::git::rebase::{self, Action};
        use crate::git::repo::OpKind;

        let (dir, ids) = five_commits("rebase-edit");
        let mut plan = rebase::load(&dir, ids[0]).unwrap();
        assert_eq!(plan.onto, None, "starting at the root commit");
        plan.items[2].action = Action::Edit; // three
        let outcome = GitOp::InteractiveRebase(plan).run(&dir).unwrap();
        assert!(
            matches!(outcome, Outcome::Stopped(ref t) if t.contains("editing")),
            "{outcome:?}"
        );
        let state = load_status(&dir).unwrap().in_progress.unwrap();
        assert_eq!(state.kind, OpKind::Rebase);

        write(&dir, "three.txt", "three, amended\n");
        GitOp::StageAll.run(&dir).unwrap();
        GitOp::Commit {
            message: "three (amended)".into(),
            amend: true,
        }
        .run(&dir)
        .unwrap();
        let outcome = GitOp::Continue(OpKind::Rebase).run(&dir).unwrap();
        assert!(matches!(outcome, Outcome::Done(_)), "{outcome:?}");
        assert_eq!(
            summaries(&dir),
            ["one", "two", "three (amended)", "four", "five"]
        );
        assert_eq!(read(&dir, "three.txt"), "three, amended\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interactive_rebase_refuses_merges_and_other_branches() {
        use crate::git::rebase;

        let (dir, ids) = five_commits("rebase-refuse");
        git(
            &dir,
            &["switch", "-q", "-c", "side", &ids[2].to_string()],
            None,
        )
        .unwrap();
        write(&dir, "side.txt", "side\n");
        commit_all(&dir, "side");
        let side = Oid::from_str(&git(&dir, &["rev-parse", "HEAD"], None).unwrap()).unwrap();
        git(&dir, &["switch", "-q", "main"], None).unwrap();
        let err = rebase::load(&dir, side).unwrap_err();
        assert!(err.to_string().contains("isn't part of"), "{err}");

        git(&dir, &["merge", "-q", "--no-edit", "side"], None).unwrap();
        let err = rebase::load(&dir, ids[3]).unwrap_err();
        assert!(err.to_string().contains("merge commit"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn force_push_with_lease_after_rewriting() {
        let dir = temp_repo("force-push");
        let remote = dir.with_extension("remote.git");
        let _ = std::fs::remove_dir_all(&remote);
        git(
            &dir,
            &["init", "-q", "--bare", &remote.to_string_lossy()],
            None,
        )
        .unwrap();
        git(
            &dir,
            &["remote", "add", "origin", &remote.to_string_lossy()],
            None,
        )
        .unwrap();
        write(&dir, "a.txt", "one\n");
        commit_all(&dir, "first");
        GitOp::Push {
            set_upstream: Some("origin".into()),
            force: false,
        }
        .run(&dir)
        .unwrap();

        GitOp::Commit {
            message: "first, reworded".into(),
            amend: true,
        }
        .run(&dir)
        .unwrap();
        let plain = GitOp::Push {
            set_upstream: None,
            force: false,
        };
        assert!(
            plain.run(&dir).is_err(),
            "a rewritten branch needs a force push"
        );
        GitOp::Push {
            set_upstream: None,
            force: true,
        }
        .run(&dir)
        .unwrap();
        assert_eq!(
            git(&remote, &["log", "-1", "--format=%s", "main"], None).unwrap(),
            "first, reworded"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&remote);
    }
}
