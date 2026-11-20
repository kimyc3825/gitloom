//! Read-only access to a repository through libgit2.
//!
//! Everything here opens its own `Repository` so it can run on a worker
//! thread. Writes go through the `git` CLI instead (see `ops.rs`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use git2::{BranchType, Delta, DiffFindOptions, DiffOptions, Oid, Repository, Sort, Status};

use super::graph::{self, GraphLayout};
use crate::util;

/// Upper bound on commits loaded into the graph.
pub const MAX_COMMITS: usize = 20_000;
/// Upper bound on diff lines kept for one commit (all files together).
const MAX_COMMIT_DIFF_LINES: usize = 20_000;
/// Upper bound on diff lines kept for one working-tree file.
const MAX_FILE_DIFF_LINES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefKind {
    LocalBranch,
    RemoteBranch,
    Tag,
}

#[derive(Debug, Clone)]
pub struct RefInfo {
    /// `main`, `origin/main`, `v1.0`
    pub name: String,
    pub kind: RefKind,
    /// The commit the ref points at (tags are peeled).
    pub target: Oid,
    /// Currently checked-out branch.
    pub is_head: bool,
    /// `origin/main` for a local branch with an upstream.
    pub upstream: Option<String>,
    /// (ahead, behind) relative to the upstream.
    pub ahead_behind: Option<(usize, usize)>,
}

impl RefInfo {
    /// Branch name without the remote prefix (`origin/feat/x` -> `feat/x`).
    pub fn branch_name(&self) -> &str {
        match self.kind {
            RefKind::RemoteBranch => self.name.split_once('/').map_or(&*self.name, |(_, b)| b),
            _ => &self.name,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HeadInfo {
    /// Current branch, `None` when detached.
    pub branch: Option<String>,
    pub target: Option<Oid>,
    pub detached: bool,
    /// Fresh repository without commits.
    pub unborn: bool,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: Oid,
    pub short_id: String,
    pub summary: String,
    pub author_name: String,
    pub time: i64,
    pub parents: Vec<Oid>,
    /// Lowercased summary, author name and email, for commit search.
    pub search_text: String,
}

#[derive(Debug, Clone)]
pub struct StashInfo {
    /// Position in the stash list: `stash@{index}`.
    pub index: usize,
    pub id: Oid,
    pub message: String,
    pub time: i64,
}

pub struct RepoSnapshot {
    pub name: String,
    pub head: HeadInfo,
    pub commits: Vec<CommitInfo>,
    pub graph: GraphLayout,
    pub row_of: HashMap<Oid, usize>,
    pub refs: Vec<RefInfo>,
    /// Commit -> indices into `refs`.
    pub refs_at: HashMap<Oid, Vec<usize>>,
    pub stashes: Vec<StashInfo>,
    pub truncated: bool,
    pub load_time: Duration,
}

impl RepoSnapshot {
    pub fn head_ref(&self) -> Option<&RefInfo> {
        self.refs.iter().find(|r| r.is_head)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Hunk,
    Context,
    Added,
    Removed,
    Meta,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// Display text: no line ending, tabs expanded.
    pub text: String,
    /// Exact bytes from the diff, line ending included; used to build patches.
    pub raw: Vec<u8>,
}

/// Position of a hunk in `FileDiff::lines` and its header numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    /// Index of the `@@` header line.
    pub header: usize,
    /// One past the last line of the hunk.
    pub end: usize,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    /// A, M, D, R, C, T, ?
    pub status: char,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    pub lines: Vec<DiffLine>,
    /// Complete hunks only; a hunk cut off by the size limit is left out.
    pub hunks: Vec<Hunk>,
    pub truncated: bool,
    /// Longest line in characters, for horizontal scrolling.
    pub max_cols: usize,
}

impl FileDiff {
    fn empty(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            old_path: None,
            status: ' ',
            additions: 0,
            deletions: 0,
            binary: false,
            lines: Vec::new(),
            hunks: Vec::new(),
            truncated: false,
            max_cols: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommitDetail {
    pub id: Oid,
    pub message: String,
    pub author: String,
    pub author_time: String,
    pub committer: String,
    pub commit_time: String,
    pub parents: Vec<Oid>,
    pub files: Vec<FileDiff>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct StatusFile {
    pub path: String,
    /// M, A, D, R, T, ? (untracked), U (conflicted)
    pub status: char,
}

#[derive(Debug, Clone, Default)]
pub struct WorkingTree {
    pub unstaged: Vec<StatusFile>,
    pub staged: Vec<StatusFile>,
}

impl WorkingTree {
    pub fn is_clean(&self) -> bool {
        self.unstaged.is_empty() && self.staged.is_empty()
    }
}

/// Find the working directory of the repository containing `path`.
pub fn discover(path: &Path) -> Result<PathBuf> {
    let repo = Repository::discover(path)
        .with_context(|| format!("not a git repository: {}", path.display()))?;
    let root = repo.workdir().unwrap_or_else(|| repo.path());
    Ok(root.to_path_buf())
}

pub fn load_snapshot(root: &Path) -> Result<RepoSnapshot> {
    let started = Instant::now();
    let mut repo =
        Repository::open(root).with_context(|| format!("cannot open {}", root.display()))?;
    let stashes = read_stashes(&mut repo);
    let head = read_head(&repo);
    let refs = read_refs(&repo, &head);

    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    if let Some(target) = head.target {
        walk.push(target)?;
    }
    for r in &refs {
        // A ref pointing at a missing object should not break the whole view.
        let _ = walk.push(r.target);
    }

    let mut commits = Vec::new();
    let mut truncated = false;
    for oid in walk {
        if commits.len() >= MAX_COMMITS {
            truncated = true;
            break;
        }
        let commit = repo.find_commit(oid?)?;
        commits.push(commit_info(&commit));
    }

    let graph = graph::layout(commits.iter().map(|c| (c.id, c.parents.as_slice())));
    let row_of = commits.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    let mut refs_at: HashMap<Oid, Vec<usize>> = HashMap::new();
    for (i, r) in refs.iter().enumerate() {
        refs_at.entry(r.target).or_default().push(i);
    }
    // The checked-out branch is listed first on its commit.
    for indices in refs_at.values_mut() {
        indices.sort_by_key(|&i| (!refs[i].is_head, refs[i].kind));
    }

    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());

    Ok(RepoSnapshot {
        name,
        head,
        commits,
        graph,
        row_of,
        refs,
        refs_at,
        stashes,
        truncated,
        load_time: started.elapsed(),
    })
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn commit_info(commit: &git2::Commit<'_>) -> CommitInfo {
    let author = commit.author();
    let id = commit.id();
    let mut short_id = id.to_string();
    short_id.truncate(7);
    let summary = commit.summary_bytes().map(lossy).unwrap_or_default();
    let author_name = lossy(author.name_bytes());
    let search_text = search_text(&summary, &author_name, &lossy(author.email_bytes()));
    CommitInfo {
        id,
        short_id,
        summary,
        author_name,
        time: author.when().seconds(),
        parents: commit.parent_ids().collect(),
        search_text,
    }
}

/// Lowercased text that commit search matches against.
pub fn search_text(summary: &str, author_name: &str, author_email: &str) -> String {
    format!("{summary}\u{1f}{author_name} <{author_email}>").to_lowercase()
}

fn read_stashes(repo: &mut Repository) -> Vec<StashInfo> {
    let mut entries = Vec::new();
    // A broken stash ref should not take the whole view down.
    let _ = repo.stash_foreach(|index, message, id| {
        entries.push((index, message.to_owned(), *id));
        true
    });
    entries
        .into_iter()
        .map(|(index, message, id)| StashInfo {
            index,
            id,
            message,
            time: repo.find_commit(id).map_or(0, |c| c.time().seconds()),
        })
        .collect()
}

fn read_head(repo: &Repository) -> HeadInfo {
    match repo.head() {
        Ok(head) => {
            let detached = repo.head_detached().unwrap_or(false);
            HeadInfo {
                branch: (!detached).then(|| lossy(head.shorthand_bytes())),
                target: head.peel_to_commit().ok().map(|c| c.id()),
                detached,
                unborn: false,
            }
        }
        Err(_) => {
            // Unborn branch: HEAD is a symbolic ref to a branch with no commits.
            let branch = repo.find_reference("HEAD").ok().and_then(|r| {
                r.symbolic_target_bytes()
                    .map(|t| lossy(t).trim_start_matches("refs/heads/").to_owned())
            });
            HeadInfo {
                branch,
                target: None,
                detached: false,
                unborn: true,
            }
        }
    }
}

fn read_refs(repo: &Repository, head: &HeadInfo) -> Vec<RefInfo> {
    let mut out = Vec::new();
    let Ok(references) = repo.references() else {
        return out;
    };
    for reference in references.flatten() {
        // Skip symbolic refs such as `refs/remotes/origin/HEAD`.
        if reference.kind() != Some(git2::ReferenceType::Direct) {
            continue;
        }
        let full = lossy(reference.name_bytes());
        let kind = if full.starts_with("refs/heads/") {
            RefKind::LocalBranch
        } else if full.starts_with("refs/remotes/") {
            RefKind::RemoteBranch
        } else if full.starts_with("refs/tags/") {
            RefKind::Tag
        } else {
            continue;
        };
        let Ok(commit) = reference.peel_to_commit() else {
            continue;
        };
        let name = lossy(reference.shorthand_bytes());
        let is_head = kind == RefKind::LocalBranch && head.branch.as_deref() == Some(&*name);
        out.push(RefInfo {
            name,
            kind,
            target: commit.id(),
            is_head,
            upstream: None,
            ahead_behind: None,
        });
    }

    for info in out.iter_mut().filter(|r| r.kind == RefKind::LocalBranch) {
        let Ok(branch) = repo.find_branch(&info.name, BranchType::Local) else {
            continue;
        };
        let Ok(upstream) = branch.upstream() else {
            continue;
        };
        info.upstream = Some(lossy(upstream.get().shorthand_bytes()));
        if let Some(up) = upstream.get().target() {
            info.ahead_behind = repo.graph_ahead_behind(info.target, up).ok();
        }
    }

    out.sort_by(|a, b| {
        a.kind.cmp(&b.kind).then_with(|| match a.kind {
            // Newest-looking tags first (v1.10 before v1.9 is good enough for now).
            RefKind::Tag => b.name.cmp(&a.name),
            _ => a.name.cmp(&b.name),
        })
    });
    out
}

pub fn load_commit_detail(root: &Path, id: Oid) -> Result<CommitDetail> {
    let repo = Repository::open(root)?;
    let commit = repo.find_commit(id)?;
    let tree = commit.tree()?;
    // Merge commits are diffed against their first parent, like GitHub does.
    let parent_tree = match commit.parent(0) {
        Ok(parent) => Some(parent.tree()?),
        Err(_) => None,
    };
    let mut opts = DiffOptions::new();
    opts.context_lines(3);
    let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))?;
    diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;
    let (files, truncated) = collect_files(&diff, MAX_COMMIT_DIFF_LINES)?;

    let author = commit.author();
    let committer = commit.committer();
    Ok(CommitDetail {
        id,
        message: lossy(commit.message_bytes()).trim_end().to_owned(),
        author: format!(
            "{} <{}>",
            lossy(author.name_bytes()),
            lossy(author.email_bytes())
        ),
        author_time: util::format_time(author.when().seconds(), author.when().offset_minutes()),
        committer: format!(
            "{} <{}>",
            lossy(committer.name_bytes()),
            lossy(committer.email_bytes())
        ),
        commit_time: util::format_time(
            committer.when().seconds(),
            committer.when().offset_minutes(),
        ),
        parents: commit.parent_ids().collect(),
        files,
        truncated,
    })
}

pub fn load_status(root: &Path) -> Result<WorkingTree> {
    let repo = Repository::open(root)?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts))?;

    let mut tree = WorkingTree::default();
    for entry in statuses.iter() {
        let s = entry.status();
        let path = lossy(entry.path_bytes());
        if s.contains(Status::CONFLICTED) {
            tree.unstaged.push(StatusFile { path, status: 'U' });
            continue;
        }
        if let Some(status) = index_status(s) {
            tree.staged.push(StatusFile {
                path: path.clone(),
                status,
            });
        }
        if let Some(status) = worktree_status(s) {
            tree.unstaged.push(StatusFile { path, status });
        }
    }
    tree.staged.sort_by(|a, b| a.path.cmp(&b.path));
    tree.unstaged.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(tree)
}

fn index_status(s: Status) -> Option<char> {
    if s.contains(Status::INDEX_NEW) {
        Some('A')
    } else if s.contains(Status::INDEX_MODIFIED) {
        Some('M')
    } else if s.contains(Status::INDEX_DELETED) {
        Some('D')
    } else if s.contains(Status::INDEX_RENAMED) {
        Some('R')
    } else if s.contains(Status::INDEX_TYPECHANGE) {
        Some('T')
    } else {
        None
    }
}

fn worktree_status(s: Status) -> Option<char> {
    if s.contains(Status::WT_NEW) {
        Some('?')
    } else if s.contains(Status::WT_MODIFIED) {
        Some('M')
    } else if s.contains(Status::WT_DELETED) {
        Some('D')
    } else if s.contains(Status::WT_RENAMED) {
        Some('R')
    } else if s.contains(Status::WT_TYPECHANGE) {
        Some('T')
    } else {
        None
    }
}

/// Diff of one working-tree file: index -> workdir, or HEAD -> index when `staged`.
pub fn load_file_diff(root: &Path, path: &str, staged: bool) -> Result<FileDiff> {
    let repo = Repository::open(root)?;
    let mut opts = DiffOptions::new();
    opts.pathspec(path)
        .disable_pathspec_match(true)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .context_lines(3);
    let diff = if staged {
        let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
        repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))?
    } else {
        repo.diff_index_to_workdir(None, Some(&mut opts))?
    };
    let (mut files, _) = collect_files(&diff, MAX_FILE_DIFF_LINES)?;
    Ok(files.pop().unwrap_or_else(|| FileDiff::empty(path)))
}

fn delta_status(delta: Delta) -> char {
    match delta {
        Delta::Added => 'A',
        Delta::Deleted => 'D',
        Delta::Modified => 'M',
        Delta::Renamed => 'R',
        Delta::Copied => 'C',
        Delta::Typechange => 'T',
        Delta::Untracked => '?',
        Delta::Conflicted => 'U',
        _ => ' ',
    }
}

fn trim_eol(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.trim_end_matches(['\n', '\r']).replace('\t', "    ")
}

/// Turn a libgit2 diff into per-file line lists, keeping at most `budget` lines.
fn collect_files(diff: &git2::Diff<'_>, mut budget: usize) -> Result<(Vec<FileDiff>, bool)> {
    let mut files = Vec::new();
    let mut truncated_any = false;

    for idx in 0..diff.deltas().len() {
        let Some(delta) = diff.get_delta(idx) else {
            continue;
        };
        let path_of = |f: git2::DiffFile<'_>| f.path_bytes().map(|p| lossy(p).replace('\\', "/"));
        let path = path_of(delta.new_file())
            .or_else(|| path_of(delta.old_file()))
            .unwrap_or_default();
        let old_path = matches!(delta.status(), Delta::Renamed | Delta::Copied)
            .then(|| path_of(delta.old_file()))
            .flatten();

        let mut file = FileDiff {
            path,
            old_path,
            status: delta_status(delta.status()),
            ..FileDiff::empty("")
        };

        match git2::Patch::from_diff(diff, idx)? {
            None => file.binary = true,
            Some(patch) => {
                file.binary = patch.delta().flags().is_binary();
                let (_, additions, deletions) = patch.line_stats()?;
                file.additions = additions;
                file.deletions = deletions;

                'hunks: for h in 0..patch.num_hunks() {
                    let (hunk, line_count) = patch.hunk(h)?;
                    if budget == 0 {
                        file.truncated = true;
                        break;
                    }
                    budget -= 1;
                    let header = file.lines.len();
                    file.lines.push(DiffLine {
                        kind: LineKind::Hunk,
                        old_no: None,
                        new_no: None,
                        text: trim_eol(hunk.header()),
                        raw: hunk.header().to_vec(),
                    });
                    for l in 0..line_count {
                        if budget == 0 {
                            file.truncated = true;
                            break 'hunks;
                        }
                        budget -= 1;
                        let line = patch.line_in_hunk(h, l)?;
                        let kind = match line.origin() {
                            '+' => LineKind::Added,
                            '-' => LineKind::Removed,
                            ' ' => LineKind::Context,
                            _ => LineKind::Meta, // "\ No newline at end of file" markers
                        };
                        let (text, raw) = if kind == LineKind::Meta {
                            ("\\ No newline at end of file".to_owned(), Vec::new())
                        } else {
                            (trim_eol(line.content()), line.content().to_vec())
                        };
                        file.lines.push(DiffLine {
                            kind,
                            old_no: line.old_lineno(),
                            new_no: line.new_lineno(),
                            text,
                            raw,
                        });
                    }
                    // Reached only when the whole hunk fit in the budget.
                    file.hunks.push(Hunk {
                        header,
                        end: file.lines.len(),
                        old_start: hunk.old_start(),
                        old_lines: hunk.old_lines(),
                        new_start: hunk.new_start(),
                        new_lines: hunk.new_lines(),
                    });
                }
            }
        }
        file.max_cols = file
            .lines
            .iter()
            .map(|l| l.text.chars().count() + 1)
            .max()
            .unwrap_or(0);
        truncated_any |= file.truncated;
        files.push(file);
    }
    Ok((files, truncated_any))
}
