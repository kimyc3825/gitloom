//! Write operations, executed with the real `git` CLI.
//!
//! Going through the CLI (instead of libgit2) means hooks, commit signing,
//! credential helpers and every config option behave exactly as they do in
//! the terminal.

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Result, bail};
use git2::Oid;

use crate::util;

#[derive(Debug, Clone)]
pub enum GitOp {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    StageAll,
    UnstageAll,
    Commit { message: String, amend: bool },
    Fetch,
    Pull,
    Push { set_upstream: Option<String> },
    Switch(String),
    TrackRemote(String),
    Detach(Oid),
    CreateBranch { name: String, at: Oid },
}

impl GitOp {
    /// Short label for the status bar while/after running.
    pub fn label(&self) -> String {
        match self {
            GitOp::Stage(p) => format!("Stage {}", count(p)),
            GitOp::Unstage(p) => format!("Unstage {}", count(p)),
            GitOp::StageAll => "Stage all".to_owned(),
            GitOp::UnstageAll => "Unstage all".to_owned(),
            GitOp::Commit { amend: true, .. } => "Amend commit".to_owned(),
            GitOp::Commit { .. } => "Commit".to_owned(),
            GitOp::Fetch => "Fetch".to_owned(),
            GitOp::Pull => "Pull".to_owned(),
            GitOp::Push { .. } => "Push".to_owned(),
            GitOp::Switch(b) => format!("Switch to {b}"),
            GitOp::TrackRemote(b) => format!("Check out {b}"),
            GitOp::Detach(id) => format!("Check out {}", &id.to_string()[..7]),
            GitOp::CreateBranch { name, .. } => format!("Create branch {name}"),
        }
    }

    /// Whether the commit graph has to be reloaded afterwards.
    pub fn changes_history(&self) -> bool {
        !matches!(
            self,
            GitOp::Stage(_) | GitOp::Unstage(_) | GitOp::StageAll | GitOp::UnstageAll
        )
    }

    pub fn run(&self, root: &Path) -> Result<String> {
        match self {
            GitOp::Stage(paths) => git(root, &with_paths(&["add", "-A", "--"], paths), None),
            GitOp::Unstage(paths) => git(root, &with_paths(&["reset", "-q", "--"], paths), None),
            GitOp::StageAll => git(root, &["add", "-A"], None),
            GitOp::UnstageAll => git(root, &["reset", "-q"], None),
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
                    git(root, &args, Some(message))
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
        }
    }
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

/// Run `git <args>` in `root`, returning stdout (or stderr as the error).
pub fn git(root: &Path, args: &[&str], stdin: Option<&str>) -> Result<String> {
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
            anyhow::anyhow!("`git` was not found on PATH")
        } else {
            anyhow::anyhow!("failed to run git: {e}")
        }
    })?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if output.status.success() {
        // Many git commands report progress on stderr even on success.
        Ok(if stdout.is_empty() { stderr } else { stdout })
    } else if stderr.is_empty() {
        bail!("git {} failed ({})", args.join(" "), output.status)
    } else {
        bail!("{stderr}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gitloom-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"], None).unwrap();
        git(&dir, &["config", "user.name", "gitloom test"], None).unwrap();
        git(
            &dir,
            &["config", "user.email", "test@example.invalid"],
            None,
        )
        .unwrap();
        git(&dir, &["config", "commit.gpgsign", "false"], None).unwrap();
        dir
    }

    #[test]
    fn stage_commit_and_read_back() {
        let dir = temp_repo("ops");
        std::fs::write(dir.join("hello.txt"), "hello\n").unwrap();

        let status = crate::git::repo::load_status(&dir).unwrap();
        assert_eq!(status.unstaged.len(), 1);
        assert_eq!(status.unstaged[0].status, '?');

        GitOp::Stage(vec!["hello.txt".into()]).run(&dir).unwrap();
        let status = crate::git::repo::load_status(&dir).unwrap();
        assert_eq!(status.staged.len(), 1);
        assert_eq!(status.staged[0].status, 'A');

        GitOp::Commit {
            message: "첫 커밋\n\nbody".into(),
            amend: false,
        }
        .run(&dir)
        .unwrap();
        let snap = crate::git::repo::load_snapshot(&dir).unwrap();
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
        let snap = crate::git::repo::load_snapshot(&dir).unwrap();
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
}
