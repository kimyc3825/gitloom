//! Build patches that stage, unstage or discard parts of a file diff.
//!
//! The patch is generated from the diff gitloom displays and handed to
//! `git apply`, the same approach git-gui and most GUIs use. Partial hunks
//! are rewritten line by line:
//!
//! | line      | selected | forward (stage) | reverse (unstage, discard) |
//! |-----------|----------|-----------------|----------------------------|
//! | added     | yes      | `+`             | `+`                        |
//! | added     | no       | dropped         | context                    |
//! | removed   | yes      | `-`             | `-`                        |
//! | removed   | no       | context         | dropped                    |
//!
//! Forward patches apply on top of the old side of the diff (the index), so
//! the old side is kept intact. Reverse patches are matched against the new
//! side (index or working tree), so that side is kept intact instead.

use std::collections::BTreeSet;

use anyhow::{Result, bail};

use super::repo::{FileDiff, Hunk, LineKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchMode {
    /// Index -> working tree diff, applied to the index.
    Stage,
    /// HEAD -> index diff, reverse-applied to the index.
    Unstage,
    /// Index -> working tree diff, reverse-applied to the working tree.
    Discard,
}

impl PatchMode {
    fn reverse(self) -> bool {
        matches!(self, PatchMode::Unstage | PatchMode::Discard)
    }

    /// Arguments for `git` to apply a patch read from stdin.
    pub fn git_args(self) -> &'static [&'static str] {
        match self {
            PatchMode::Stage => &["apply", "--cached", "--recount", "--whitespace=nowarn", "-"],
            PatchMode::Unstage => &[
                "apply",
                "--cached",
                "--reverse",
                "--recount",
                "--whitespace=nowarn",
                "-",
            ],
            PatchMode::Discard => &[
                "apply",
                "--reverse",
                "--recount",
                "--whitespace=nowarn",
                "-",
            ],
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            PatchMode::Stage => "Stage",
            PatchMode::Unstage => "Unstage",
            PatchMode::Discard => "Discard",
        }
    }
}

/// Lines that can be picked one by one.
pub fn is_selectable(kind: LineKind) -> bool {
    matches!(kind, LineKind::Added | LineKind::Removed)
}

/// Whether hunks and lines of this diff can be staged separately.
///
/// Limited to plain modifications: new, deleted and renamed files need extra
/// patch headers and are staged as whole files instead.
pub fn supports_line_ops(file: &FileDiff) -> bool {
    file.status == 'M' && !file.binary && !file.truncated && !file.hunks.is_empty()
}

/// Every selectable line of hunk number `hunk`.
pub fn hunk_lines(file: &FileDiff, hunk: usize) -> BTreeSet<usize> {
    file.hunks.get(hunk).map_or_else(BTreeSet::new, |h| {
        (h.header + 1..h.end)
            .filter(|&i| is_selectable(file.lines[i].kind))
            .collect()
    })
}

/// Build a patch with the selected lines (indices into `file.lines`).
pub fn build(file: &FileDiff, selected: &BTreeSet<usize>, mode: PatchMode) -> Result<Vec<u8>> {
    if !supports_line_ops(file) {
        bail!("Parts of this file can't be staged separately; use the whole file.");
    }
    let reverse = mode.reverse();
    let mut out = Vec::new();
    write_header(&mut out, &file.path);

    // How much the hunks written so far shift the lines of the other side.
    let mut offset: i64 = 0;
    let mut wrote_any = false;
    for hunk in &file.hunks {
        let body = hunk.header + 1..hunk.end;
        let picked = body
            .clone()
            .filter(|&i| selected.contains(&i) && is_selectable(file.lines[i].kind))
            .count();
        if picked == 0 {
            continue;
        }
        let selectable = body
            .clone()
            .filter(|&i| is_selectable(file.lines[i].kind))
            .count();
        let has_eof_marker = body.clone().any(|i| file.lines[i].kind == LineKind::Meta);
        if has_eof_marker && picked != selectable {
            bail!(
                "This hunk touches a line without a trailing newline; stage the whole hunk instead."
            );
        }

        let mut lines = Vec::new();
        let (mut old_count, mut new_count) = (0u32, 0u32);
        for i in body {
            let line = &file.lines[i];
            let chosen = selected.contains(&i);
            let prefix = match (line.kind, chosen, reverse) {
                (LineKind::Context, ..) => b' ',
                (LineKind::Added, true, _) => b'+',
                (LineKind::Removed, true, _) => b'-',
                (LineKind::Added, false, true) | (LineKind::Removed, false, false) => b' ',
                (LineKind::Added, false, false) | (LineKind::Removed, false, true) => continue,
                (LineKind::Meta, ..) => {
                    lines.extend_from_slice(b"\\ No newline at end of file\n");
                    continue;
                }
                (LineKind::Hunk, ..) => continue,
            };
            match prefix {
                b' ' => {
                    old_count += 1;
                    new_count += 1;
                }
                b'+' => new_count += 1,
                _ => old_count += 1,
            }
            lines.push(prefix);
            lines.extend_from_slice(&line.raw);
            if !line.raw.ends_with(b"\n") {
                lines.push(b'\n');
            }
        }

        let (old_start, new_start) = hunk_starts(hunk, old_count, new_count, offset, reverse);
        offset += i64::from(new_count) - i64::from(old_count);
        out.extend_from_slice(
            format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n").as_bytes(),
        );
        out.extend_from_slice(&lines);
        wrote_any = true;
    }

    if !wrote_any {
        bail!("Select added or removed lines first.");
    }
    Ok(out)
}

/// Start lines for the rewritten hunk. The side the patch is applied to keeps
/// its original numbers; the other side is derived from it.
fn hunk_starts(
    hunk: &Hunk,
    old_count: u32,
    new_count: u32,
    offset: i64,
    reverse: bool,
) -> (i64, i64) {
    // In unified diffs an empty range names the line *before* it.
    let position = |start: u32, count: u32| i64::from(start) - i64::from(count > 0);
    let start_at = |position: i64, count: u32| position + i64::from(count > 0);
    if reverse {
        let new_pos = position(hunk.new_start, new_count);
        (
            start_at(new_pos - offset, old_count),
            i64::from(hunk.new_start),
        )
    } else {
        let old_pos = position(hunk.old_start, old_count);
        (
            i64::from(hunk.old_start),
            start_at(old_pos + offset, new_count),
        )
    }
}

fn write_header(out: &mut Vec<u8>, path: &str) {
    let a = quote_path(&format!("a/{path}"));
    let b = quote_path(&format!("b/{path}"));
    out.extend_from_slice(format!("diff --git {a} {b}\n--- {a}\n+++ {b}\n").as_bytes());
}

/// Quote a path the way git does when it contains special characters.
fn quote_path(path: &str) -> String {
    let needs_quotes = path
        .chars()
        .any(|c| c == '"' || c == '\\' || c.is_control());
    if !needs_quotes {
        return path.to_owned();
    }
    let mut quoted = String::from("\"");
    for c in path.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\t' => quoted.push_str("\\t"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            c if c.is_control() => {
                let mut buf = [0u8; 4];
                for byte in c.encode_utf8(&mut buf).bytes() {
                    quoted.push_str(&format!("\\{byte:03o}"));
                }
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::git::ops::{GitOp, git, git_bytes};
    use crate::git::repo::load_file_diff;

    fn temp_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gitloom-patch-{name}-{}", std::process::id()));
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

    fn commit_file(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
        git(dir, &["add", name], None).unwrap();
        git(dir, &["commit", "-q", "-m", "base"], None).unwrap();
    }

    fn numbered(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line {i}")).collect()
    }

    fn join(lines: &[String]) -> String {
        lines.iter().map(|l| format!("{l}\n")).collect()
    }

    fn index_content(dir: &Path, name: &str) -> String {
        String::from_utf8(git_bytes(dir, &["show", &format!(":{name}")]).unwrap()).unwrap()
    }

    fn apply(dir: &Path, name: &str, staged: bool, picks: &BTreeSet<usize>, mode: PatchMode) {
        let diff = load_file_diff(dir, name, staged).unwrap();
        let patch = build(&diff, picks, mode).unwrap();
        GitOp::ApplyPatch {
            patch,
            mode,
            what: "test".into(),
        }
        .run(dir)
        .unwrap_or_else(|e| panic!("{mode:?} failed: {e:#}"));
    }

    /// Indices of the lines whose display text matches one of `texts`.
    fn lines_with(diff: &FileDiff, texts: &[&str]) -> BTreeSet<usize> {
        diff.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| is_selectable(l.kind) && texts.contains(&l.text.as_str()))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn stage_one_of_two_hunks() {
        let dir = temp_repo("hunk");
        let mut lines = numbered(20);
        commit_file(&dir, "f.txt", &join(&lines));
        lines[1] = "line 2 changed".into();
        lines[16] = "line 17 changed".into();
        std::fs::write(dir.join("f.txt"), join(&lines)).unwrap();

        let diff = load_file_diff(&dir, "f.txt", false).unwrap();
        assert_eq!(diff.hunks.len(), 2);
        apply(
            &dir,
            "f.txt",
            false,
            &hunk_lines(&diff, 1),
            PatchMode::Stage,
        );

        let staged = index_content(&dir, "f.txt");
        assert!(staged.contains("line 17 changed"));
        assert!(!staged.contains("line 2 changed"));
        // The working tree still has both changes.
        let unstaged = load_file_diff(&dir, "f.txt", false).unwrap();
        assert_eq!(unstaged.hunks.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_and_unstage_single_lines() {
        let dir = temp_repo("lines");
        let lines = numbered(6);
        commit_file(&dir, "f.txt", &join(&lines));
        // Replace line 3 and add two lines after line 4.
        let edited = [
            "line 1",
            "line 2",
            "line 3 new",
            "line 4",
            "added a",
            "added b",
            "line 5",
            "line 6",
        ]
        .map(String::from);
        std::fs::write(dir.join("f.txt"), join(&edited)).unwrap();

        // Stage only "added b": the index gets that line and nothing else.
        let diff = load_file_diff(&dir, "f.txt", false).unwrap();
        apply(
            &dir,
            "f.txt",
            false,
            &lines_with(&diff, &["added b"]),
            PatchMode::Stage,
        );
        assert_eq!(
            index_content(&dir, "f.txt"),
            join(
                &[
                    "line 1", "line 2", "line 3", "line 4", "added b", "line 5", "line 6"
                ]
                .map(String::from)
            )
        );

        // Stage the replacement of line 3 (both its - and + line).
        let diff = load_file_diff(&dir, "f.txt", false).unwrap();
        let picks = lines_with(&diff, &["line 3", "line 3 new"]);
        assert_eq!(picks.len(), 2);
        apply(&dir, "f.txt", false, &picks, PatchMode::Stage);
        assert!(index_content(&dir, "f.txt").contains("line 3 new\n"));

        // Unstage "added b" again, keeping the line 3 change staged.
        let staged = load_file_diff(&dir, "f.txt", true).unwrap();
        apply(
            &dir,
            "f.txt",
            true,
            &lines_with(&staged, &["added b"]),
            PatchMode::Unstage,
        );
        assert_eq!(
            index_content(&dir, "f.txt"),
            join(
                &[
                    "line 1",
                    "line 2",
                    "line 3 new",
                    "line 4",
                    "line 5",
                    "line 6"
                ]
                .map(String::from)
            )
        );
        // Unstaging never touches the working tree.
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            join(&edited)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discard_selected_lines_from_the_working_tree() {
        let dir = temp_repo("discard");
        commit_file(&dir, "f.txt", &join(&numbered(5)));
        let edited = [
            "line 1", "keep me", "line 2", "drop me", "line 3", "line 4", "line 5",
        ]
        .map(String::from);
        std::fs::write(dir.join("f.txt"), join(&edited)).unwrap();

        let diff = load_file_diff(&dir, "f.txt", false).unwrap();
        apply(
            &dir,
            "f.txt",
            false,
            &lines_with(&diff, &["drop me"]),
            PatchMode::Discard,
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            join(&["line 1", "keep me", "line 2", "line 3", "line 4", "line 5"].map(String::from))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn later_hunks_get_shifted_start_lines() {
        let dir = temp_repo("offset");
        let mut lines = numbered(30);
        commit_file(&dir, "f.txt", &join(&lines));
        // Hunk 1 grows the file by two lines, hunk 2 changes a line further down.
        lines.insert(2, "extra 1".into());
        lines.insert(3, "extra 2".into());
        lines[26] = "line 25 changed".into();
        std::fs::write(dir.join("f.txt"), join(&lines)).unwrap();

        let diff = load_file_diff(&dir, "f.txt", false).unwrap();
        assert_eq!(diff.hunks.len(), 2);
        let all: BTreeSet<usize> = (0..diff.lines.len()).collect();
        let patch = String::from_utf8(build(&diff, &all, PatchMode::Stage).unwrap()).unwrap();
        // Second hunk: same old start, new start moved down by two.
        let second = &diff.hunks[1];
        assert!(patch.contains(&format!(
            "@@ -{},{} +{},{} @@",
            second.old_start,
            second.old_lines,
            second.old_start + 2,
            second.new_lines
        )));
        apply(&dir, "f.txt", false, &all, PatchMode::Stage);
        assert_eq!(index_content(&dir, "f.txt"), join(&lines));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_final_newline_needs_the_whole_hunk() {
        let dir = temp_repo("eof");
        commit_file(&dir, "f.txt", "a\nb\nc");
        std::fs::write(dir.join("f.txt"), "a\nb\nc\nd\n").unwrap();

        let diff = load_file_diff(&dir, "f.txt", false).unwrap();
        let partial = lines_with(&diff, &["d"]);
        assert!(build(&diff, &partial, PatchMode::Stage).is_err());

        apply(
            &dir,
            "f.txt",
            false,
            &hunk_lines(&diff, 0),
            PatchMode::Stage,
        );
        assert_eq!(index_content(&dir, "f.txt"), "a\nb\nc\nd\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crlf_lines_and_spaces_in_paths_survive() {
        let dir = temp_repo("crlf");
        commit_file(&dir, "my file.txt", "one\r\ntwo\r\nthree\r\n");
        std::fs::write(dir.join("my file.txt"), "one\r\ntwo 2\r\nthree\r\nfour\r\n").unwrap();

        let diff = load_file_diff(&dir, "my file.txt", false).unwrap();
        apply(
            &dir,
            "my file.txt",
            false,
            &lines_with(&diff, &["four"]),
            PatchMode::Stage,
        );
        assert_eq!(
            index_content(&dir, "my file.txt"),
            "one\r\ntwo\r\nthree\r\nfour\r\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quotes_special_paths() {
        assert_eq!(quote_path("a/plain name.txt"), "a/plain name.txt");
        assert_eq!(quote_path("a/한글.txt"), "a/한글.txt");
        assert_eq!(quote_path("a/tab\there"), "\"a/tab\\there\"");
        assert_eq!(quote_path("a/q\"uote"), "\"a/q\\\"uote\"");
    }

    #[test]
    fn new_files_are_whole_file_only() {
        let mut diff = FileDiff {
            path: "x".into(),
            old_path: None,
            status: 'A',
            additions: 1,
            deletions: 0,
            binary: false,
            lines: Vec::new(),
            hunks: Vec::new(),
            truncated: false,
            max_cols: 0,
        };
        assert!(!supports_line_ops(&diff));
        diff.status = 'M';
        assert!(!supports_line_ops(&diff), "no hunks");
    }
}
