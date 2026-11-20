//! Per-file views: blame and the file's history.
//!
//! Both are read with the `git` CLI rather than libgit2: `git blame` is far
//! faster than libgit2's implementation, and `git log --follow` tracks the
//! file across renames.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context as _, Result, anyhow};
use git2::Oid;

use super::ops;

/// Upper bound on entries in a file's history.
pub const MAX_FILE_HISTORY: usize = 5_000;

#[derive(Debug, Clone)]
pub struct BlameCommit {
    /// All zeros for lines that aren't committed yet.
    pub id: Oid,
    pub short_id: String,
    pub author: String,
    pub time: i64,
    pub summary: String,
    /// The commit before this one and the file's path there, if any.
    pub previous: Option<(Oid, String)>,
    /// The file's path in this commit.
    pub path: String,
}

impl BlameCommit {
    pub fn is_uncommitted(&self) -> bool {
        self.id.is_zero()
    }
}

#[derive(Debug, Clone)]
pub struct BlameLine {
    /// Index into `Blame::commits`.
    pub commit: usize,
    /// Display text: no line ending, tabs expanded.
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Blame {
    pub path: String,
    /// Blamed revision; `None` = the working tree.
    pub rev: Option<Oid>,
    pub commits: Vec<BlameCommit>,
    pub lines: Vec<BlameLine>,
    pub max_cols: usize,
}

pub fn blame(root: &Path, path: &str, rev: Option<Oid>) -> Result<Blame> {
    let rev_text = rev.map(|r| r.to_string());
    let mut args = vec!["blame", "--porcelain"];
    if let Some(r) = &rev_text {
        args.push(r);
    }
    args.extend(["--", path]);
    let output = ops::git_bytes(root, &args)?;
    let (commits, lines) = parse_porcelain(&output)?;
    let max_cols = lines
        .iter()
        .map(|l| l.text.chars().count() + 1)
        .max()
        .unwrap_or(0);
    Ok(Blame {
        path: path.to_owned(),
        rev,
        commits,
        lines,
        max_cols,
    })
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn short(id: Oid) -> String {
    id.to_string()[..7].to_owned()
}

/// Parse `git blame --porcelain`: a header per line, commit details the first
/// time a commit shows up, then the line itself prefixed with a tab.
fn parse_porcelain(output: &[u8]) -> Result<(Vec<BlameCommit>, Vec<BlameLine>)> {
    let mut commits: Vec<BlameCommit> = Vec::new();
    let mut index: HashMap<Oid, usize> = HashMap::new();
    let mut lines = Vec::new();
    let mut current = None;

    for raw in output.split(|&b| b == b'\n') {
        if let Some(content) = raw.strip_prefix(b"\t") {
            let commit = current.context("blame output: line content before its header")?;
            let text = lossy(content);
            lines.push(BlameLine {
                commit,
                text: text.trim_end_matches('\r').replace('\t', "    "),
            });
            continue;
        }
        if raw.is_empty() {
            continue;
        }
        let text = lossy(raw);
        let (key, value) = text.split_once(' ').unwrap_or((&text, ""));
        if key.len() == 40 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
            let id = Oid::from_str(key)?;
            let at = *index.entry(id).or_insert_with(|| {
                commits.push(BlameCommit {
                    id,
                    short_id: short(id),
                    author: String::new(),
                    time: 0,
                    summary: String::new(),
                    previous: None,
                    path: String::new(),
                });
                commits.len() - 1
            });
            current = Some(at);
            continue;
        }
        let Some(c) = current.map(|at| &mut commits[at]) else {
            return Err(anyhow!("blame output: unexpected line {text:?}"));
        };
        match key {
            "author" => c.author = value.to_owned(),
            "author-time" => c.time = value.parse().unwrap_or(0),
            "summary" => c.summary = value.to_owned(),
            "filename" => c.path = value.to_owned(),
            "previous" => {
                if let Some((id, path)) = value.split_once(' ')
                    && let Ok(id) = Oid::from_str(id)
                {
                    c.previous = Some((id, path.to_owned()));
                }
            }
            _ => {}
        }
    }
    Ok((commits, lines))
}

/// One commit in a file's history.
#[derive(Debug, Clone)]
pub struct FileCommit {
    pub id: Oid,
    pub short_id: String,
    pub time: i64,
    pub author: String,
    pub summary: String,
    /// The file's path in this commit.
    pub path: String,
    /// Where it was renamed (or copied) from in this commit.
    pub old_path: Option<String>,
    /// A, M, D, R, C, T
    pub status: char,
}

/// Commits that changed `path`, newest first, following renames.
/// The flag is set when the list was cut at `MAX_FILE_HISTORY`.
pub fn file_history(root: &Path, path: &str) -> Result<(Vec<FileCommit>, bool)> {
    let limit = format!("--max-count={}", MAX_FILE_HISTORY + 1);
    let output = ops::git_bytes(
        root,
        &[
            "log",
            "--follow",
            &limit,
            "--format=%x1e%H%x1f%at%x1f%an%x1f%s",
            "--name-status",
            "-z",
            "--",
            path,
        ],
    )?;
    let mut commits = parse_log(&output, path)?;
    let truncated = commits.len() > MAX_FILE_HISTORY;
    commits.truncate(MAX_FILE_HISTORY);
    Ok((commits, truncated))
}

/// Records look like `\x1e<sha>\x1f<time>\x1f<author>\x1f<subject>\0\n<status>\0<path>\0[<path>\0]`.
fn parse_log(output: &[u8], path: &str) -> Result<Vec<FileCommit>> {
    let mut commits = Vec::new();
    // Path of the file in the commit seen last (we walk backwards in time).
    let mut tracked = path.to_owned();
    for record in output.split(|&b| b == 0x1e).skip(1) {
        let (header, rest) = match record.iter().position(|&b| b == 0) {
            Some(nul) => (&record[..nul], &record[nul + 1..]),
            None => (record, &[][..]),
        };
        let header = lossy(header);
        let mut fields = header.split('\x1f');
        let (Some(id), Some(time), Some(author), Some(summary)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(anyhow!("git log: unexpected record {header:?}"));
        };
        let id = Oid::from_str(id.trim())?;
        let names: Vec<String> = rest
            .strip_prefix(b"\n")
            .unwrap_or(rest)
            .split(|&b| b == 0)
            .filter(|f| !f.is_empty())
            .map(lossy)
            .collect();
        let status = names.first().and_then(|s| s.chars().next()).unwrap_or('M');
        let (file, old_path) = match (status, names.as_slice()) {
            ('R' | 'C', [_, old, new, ..]) => (new.clone(), Some(old.clone())),
            (_, [_, file, ..]) => (file.clone(), None),
            _ => (tracked.clone(), None),
        };
        tracked = old_path.clone().unwrap_or_else(|| file.clone());
        commits.push(FileCommit {
            id,
            short_id: short(id),
            time: time.parse().unwrap_or(0),
            author: author.to_owned(),
            summary: summary.to_owned(),
            path: file,
            old_path,
            status,
        });
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::ops::git;

    fn temp_repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gitloom-blame-{name}-{}", std::process::id()));
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

    fn commit(dir: &Path, message: &str) {
        git(dir, &["add", "-A"], None).unwrap();
        git(dir, &["commit", "-q", "-m", message], None).unwrap();
    }

    #[test]
    fn blame_attributes_lines_and_uncommitted_changes() {
        let dir = temp_repo("lines");
        std::fs::write(dir.join("f.txt"), "one\ntwo\nthree\n").unwrap();
        commit(&dir, "first");
        std::fs::write(dir.join("f.txt"), "one\nTWO\nthree\n").unwrap();
        commit(&dir, "shout two");
        std::fs::write(dir.join("f.txt"), "one\nTWO\nthree\n\tlocal\n").unwrap();

        let blame = blame(&dir, "f.txt", None).unwrap();
        let summaries: Vec<&str> = blame
            .lines
            .iter()
            .map(|l| blame.commits[l.commit].summary.as_str())
            .collect();
        assert_eq!(summaries[..3], ["first", "shout two", "first"]);
        let last = &blame.commits[blame.lines[3].commit];
        assert!(last.is_uncommitted());
        assert_eq!(blame.lines[3].text, "    local");
        let second = &blame.commits[blame.lines[1].commit];
        assert_eq!(second.author, "gitloom test");
        assert!(second.previous.is_some(), "the line existed before");

        // Blame at a revision ignores the working tree.
        let first_id = blame.commits[blame.lines[0].commit].id;
        let old = super::blame(&dir, "f.txt", Some(first_id)).unwrap();
        assert_eq!(old.lines.len(), 3);
        assert_eq!(old.lines[1].text, "two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_follows_renames() {
        let dir = temp_repo("history");
        std::fs::write(dir.join("old name.txt"), "a\nb\nc\nd\ne\n").unwrap();
        commit(&dir, "add");
        std::fs::write(dir.join("old name.txt"), "a\nb\nc\nd\ne\nf\n").unwrap();
        commit(&dir, "append f");
        git(&dir, &["mv", "old name.txt", "new.txt"], None).unwrap();
        commit(&dir, "rename");
        std::fs::write(dir.join("other.txt"), "unrelated\n").unwrap();
        commit(&dir, "unrelated");
        std::fs::write(dir.join("new.txt"), "a\nb\nc\nd\ne\nf\ng\n").unwrap();
        commit(&dir, "append g");

        let (history, truncated) = file_history(&dir, "new.txt").unwrap();
        assert!(!truncated);
        let summaries: Vec<&str> = history.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, ["append g", "rename", "append f", "add"]);
        assert_eq!(history[1].status, 'R');
        assert_eq!(history[1].old_path.as_deref(), Some("old name.txt"));
        assert_eq!(history[2].path, "old name.txt");
        assert_eq!(history[3].status, 'A');
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untracked_files_have_no_history() {
        let dir = temp_repo("untracked");
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        commit(&dir, "base");
        std::fs::write(dir.join("new.txt"), "n\n").unwrap();
        assert!(file_history(&dir, "new.txt").unwrap().0.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
