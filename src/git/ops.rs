//! Write operations, executed with the real `git` CLI.
//!
//! Going through the CLI (instead of libgit2) means hooks, commit signing,
//! credential helpers and every config option behave exactly as they do in
//! the terminal.

use std::io::Write as _;
use std::path::Path;
use std::process::{Output, Stdio};

use anyhow::{Result, anyhow, bail};
use git2::Oid;

use super::patch::PatchMode;
use crate::util;

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
            GitOp::Push { .. } => "Push".to_owned(),
            GitOp::Switch(b) => format!("Switch to {b}"),
            GitOp::TrackRemote(b) => format!("Check out {b}"),
            GitOp::Detach(id) => format!("Check out {}", &id.to_string()[..7]),
            GitOp::CreateBranch { name, .. } => format!("Create branch {name}"),
            GitOp::DeleteBranch { name, .. } => format!("Delete branch {name}"),
            GitOp::DeleteRemoteBranch { remote, branch } => format!("Delete {remote}/{branch}"),
            GitOp::StashPush { .. } => "Stash changes".to_owned(),
            GitOp::StashApply(i) => format!("Apply stash@{{{i}}}"),
            GitOp::StashPop(i) => format!("Pop stash@{{{i}}}"),
            GitOp::StashDrop(i) => format!("Drop stash@{{{i}}}"),
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
        )
    }

    /// Whether the operation talks to a remote (pull requests may change).
    pub fn touches_remote(&self) -> bool {
        matches!(
            self,
            GitOp::Fetch | GitOp::Pull | GitOp::Push { .. } | GitOp::DeleteRemoteBranch { .. }
        )
    }

    pub fn run(&self, root: &Path) -> Result<String> {
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
                set_upstream: Some(remote),
            } => git(root, &["push", "-u", remote, "HEAD"], None),
            GitOp::Push { set_upstream: None } => git(root, &["push"], None),
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
        }
    }
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

fn run_git(root: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<Output> {
    let mut cmd = util::command("git");
    cmd.arg("-C")
        .arg(root)
        .args(args)
        // Never block on a terminal prompt; credential helpers still work.
        .env("GIT_TERMINAL_PROMPT", "0")
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
    let output = run_git(root, args, stdin)?;
    if !output.status.success() {
        return Err(failure(args, &output));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    // Many git commands report progress on stderr even on success.
    Ok(if stdout.is_empty() { stderr } else { stdout })
}

/// Exact stdout of a successful `git <args>`.
#[cfg(test)]
pub fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = run_git(root, args, None)?;
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
}
