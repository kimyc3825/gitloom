//! Interactive rebase: the plan the user edits in the dialog and the todo
//! script handed to `git rebase -i`.
//!
//! git does the actual work (and stops on conflicts as usual); gitloom only
//! replaces the todo list. New messages for reworded commits are applied with
//! an `exec git commit --amend` line, so they survive a stop and `--continue`.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use git2::{BranchType, Oid, Repository, Sort};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Pick,
    Reword,
    Edit,
    Squash,
    Fixup,
    Drop,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::Pick,
        Action::Reword,
        Action::Edit,
        Action::Squash,
        Action::Fixup,
        Action::Drop,
    ];

    pub fn keyword(self) -> &'static str {
        match self {
            Action::Pick => "pick",
            Action::Reword => "reword",
            Action::Edit => "edit",
            Action::Squash => "squash",
            Action::Fixup => "fixup",
            Action::Drop => "drop",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Action::Pick => "Keep the commit",
            Action::Reword => "Keep the commit, change its message",
            Action::Edit => "Stop after this commit so you can amend it",
            Action::Squash => "Meld into the commit below, keeping both messages",
            Action::Fixup => "Meld into the commit below, dropping this message",
            Action::Drop => "Remove the commit",
        }
    }

    /// Joins the previous (older) commit instead of standing alone.
    pub fn melds(self) -> bool {
        matches!(self, Action::Squash | Action::Fixup)
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    pub id: Oid,
    pub short_id: String,
    pub summary: String,
    /// Full message; edited in place when the action is `reword`.
    pub message: String,
    pub action: Action,
    /// Already on the upstream branch, so rewriting it needs a force push.
    pub pushed: bool,
}

#[derive(Debug, Clone)]
pub struct Plan {
    /// Commit the list is replayed onto; `None` rewrites from the root commit.
    pub onto: Option<Oid>,
    /// Oldest first, like git's todo list.
    pub items: Vec<Item>,
    /// Branch being rebased, `None` on a detached HEAD.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    /// Stash uncommitted changes for the duration of the rebase.
    pub autostash: bool,
    original: Vec<(Oid, String)>,
}

impl Plan {
    /// Why the plan can't run, in words for the dialog.
    pub fn validate(&self) -> Result<(), String> {
        let Some(first) = self.items.iter().find(|i| i.action != Action::Drop) else {
            return Err("Every commit is dropped. Keep at least one.".to_owned());
        };
        if first.action.melds() {
            return Err(format!(
                "{} can't be squashed or fixed up: there is no older commit in the list to meld into.",
                first.short_id
            ));
        }
        if let Some(item) = self
            .items
            .iter()
            .find(|i| i.action == Action::Reword && i.message.trim().is_empty())
        {
            return Err(format!("The new message for {} is empty.", item.short_id));
        }
        Ok(())
    }

    /// Nothing would change: same order, everything picked, messages untouched.
    pub fn is_unchanged(&self) -> bool {
        self.items.len() == self.original.len()
            && self
                .items
                .iter()
                .zip(&self.original)
                .all(|(item, (id, message))| {
                    item.id == *id
                        && match item.action {
                            Action::Pick => true,
                            Action::Reword => item.message == *message,
                            _ => false,
                        }
                })
    }

    pub fn pushed_count(&self) -> usize {
        self.items.iter().filter(|i| i.pushed).count()
    }

    /// The todo list, plus message files (written to `dir`) it refers to.
    pub fn script(&self, dir: &Path) -> (String, Vec<(PathBuf, String)>) {
        let mut todo = String::new();
        let mut files = Vec::new();
        // A reword applies to the commit that comes out of its squash/fixup group.
        let mut pending: Option<PathBuf> = None;
        let flush = |todo: &mut String, pending: &mut Option<PathBuf>| {
            if let Some(path) = pending.take() {
                todo.push_str(&format!(
                    "exec git commit --amend --allow-empty -F {}\n",
                    sh_quote(&path)
                ));
            }
        };
        for (n, item) in self.items.iter().enumerate() {
            if matches!(item.action, Action::Pick | Action::Reword | Action::Edit) {
                flush(&mut todo, &mut pending);
            }
            let keyword = match item.action {
                Action::Reword => "pick",
                other => other.keyword(),
            };
            let summary = item.summary.replace(['\n', '\r'], " ");
            todo.push_str(&format!("{keyword} {} {summary}\n", item.id));
            if item.action == Action::Reword {
                let path = dir.join(format!("message-{n}.txt"));
                files.push((path.clone(), item.message.clone()));
                pending = Some(path);
            }
        }
        flush(&mut todo, &mut pending);
        (todo, files)
    }
}

/// Quote for `sh`, with forward slashes so Git for Windows' shell is happy.
pub fn sh_quote(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Everything from `from` (inclusive) up to HEAD, ready to be edited.
pub fn load(root: &Path, from: Oid) -> Result<Plan> {
    let repo = Repository::open(root)?;
    let head = repo
        .head()
        .context("the repository has no commits yet")?
        .peel_to_commit()?;
    let short = |id: Oid| id.to_string()[..7].to_owned();
    if from != head.id() && !repo.graph_descendant_of(head.id(), from)? {
        bail!(
            "{} isn't part of the checked-out branch's history",
            short(from)
        );
    }
    let onto = repo.find_commit(from)?.parent_id(0).ok();

    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?;
    walk.push(head.id())?;
    if let Some(onto) = onto {
        walk.hide(onto)?;
    }
    let mut items = Vec::new();
    for oid in walk {
        let commit = repo.find_commit(oid?)?;
        if commit.parent_count() > 1 {
            bail!(
                "{} is a merge commit. Interactive rebase would flatten it, which gitloom doesn't do yet.",
                short(commit.id())
            );
        }
        items.push(Item {
            id: commit.id(),
            short_id: short(commit.id()),
            summary: String::from_utf8_lossy(commit.summary_bytes().unwrap_or_default())
                .into_owned(),
            message: String::from_utf8_lossy(commit.message_bytes())
                .trim_end()
                .to_owned(),
            action: Action::Pick,
            pushed: false,
        });
    }

    let lossy = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    let detached = repo.head_detached().unwrap_or(false);
    let branch = if detached {
        None
    } else {
        repo.head().ok().map(|h| lossy(h.shorthand_bytes()))
    };
    let mut upstream = None;
    if let Some(name) = &branch
        && let Ok(up) = repo
            .find_branch(name, BranchType::Local)
            .and_then(|b| b.upstream())
    {
        upstream = Some(lossy(up.get().shorthand_bytes()));
        if let Some(tip) = up.get().target() {
            mark_pushed(&repo, tip, onto, &mut items)?;
        }
    }

    let original = items.iter().map(|i| (i.id, i.message.clone())).collect();
    Ok(Plan {
        onto,
        items,
        branch,
        upstream,
        autostash: false,
        original,
    })
}

fn mark_pushed(repo: &Repository, tip: Oid, onto: Option<Oid>, items: &mut [Item]) -> Result<()> {
    let mut walk = repo.revwalk()?;
    walk.push(tip)?;
    if let Some(onto) = onto {
        walk.hide(onto)?;
    }
    let on_upstream: std::collections::HashSet<Oid> = walk.filter_map(Result::ok).collect();
    for item in items {
        item.pushed = on_upstream.contains(&item.id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(n: u8, action: Action) -> Item {
        let id = Oid::from_bytes(&[n; 20]).unwrap();
        Item {
            id,
            short_id: id.to_string()[..7].to_owned(),
            summary: format!("commit {n}"),
            message: format!("commit {n}"),
            action,
            pushed: false,
        }
    }

    fn plan(items: Vec<Item>) -> Plan {
        let original = items.iter().map(|i| (i.id, i.message.clone())).collect();
        Plan {
            onto: None,
            items,
            branch: Some("main".into()),
            upstream: None,
            autostash: false,
            original,
        }
    }

    #[test]
    fn script_lists_actions_and_amends_rewords_after_their_group() {
        let mut items = vec![
            item(1, Action::Reword),
            item(2, Action::Squash),
            item(3, Action::Drop),
            item(4, Action::Fixup),
            item(5, Action::Edit),
            item(6, Action::Reword),
        ];
        items[0].message = "new message".into();
        items[5].message = "last one".into();
        let plan = plan(items);
        let (todo, files) = plan.script(Path::new("/repo/.git/gitloom"));
        let lines: Vec<&str> = todo.lines().collect();
        let id = |n: u8| Oid::from_bytes(&[n; 20]).unwrap().to_string();
        assert_eq!(lines[0], format!("pick {} commit 1", id(1)));
        assert_eq!(lines[1], format!("squash {} commit 2", id(2)));
        assert_eq!(lines[2], format!("drop {} commit 3", id(3)));
        assert_eq!(lines[3], format!("fixup {} commit 4", id(4)));
        // The first reword lands after its squash/fixup group, before the next pick.
        assert_eq!(
            lines[4],
            "exec git commit --amend --allow-empty -F '/repo/.git/gitloom/message-0.txt'"
        );
        assert_eq!(lines[5], format!("edit {} commit 5", id(5)));
        assert_eq!(lines[6], format!("pick {} commit 6", id(6)));
        assert!(lines[7].ends_with("message-5.txt'"));
        assert_eq!(lines.len(), 8);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].1, "new message");
    }

    #[test]
    fn validation() {
        assert!(
            plan(vec![item(1, Action::Pick), item(2, Action::Squash)])
                .validate()
                .is_ok()
        );
        let err = plan(vec![item(1, Action::Drop), item(2, Action::Fixup)])
            .validate()
            .unwrap_err();
        assert!(err.contains("no older commit"), "{err}");
        assert!(plan(vec![item(1, Action::Drop)]).validate().is_err());
        let mut reword = plan(vec![item(1, Action::Reword)]);
        reword.items[0].message = "  ".into();
        assert!(reword.validate().is_err());
    }

    #[test]
    fn unchanged_plans_are_detected() {
        let mut p = plan(vec![item(1, Action::Pick), item(2, Action::Pick)]);
        assert!(p.is_unchanged());
        p.items[1].action = Action::Reword;
        assert!(
            p.is_unchanged(),
            "reword with the same message changes nothing"
        );
        p.items[1].message = "different".into();
        assert!(!p.is_unchanged());
        p.items[1] = item(2, Action::Pick);
        p.items.swap(0, 1);
        assert!(!p.is_unchanged());
    }

    #[test]
    fn quotes_paths_for_sh() {
        assert_eq!(
            sh_quote(Path::new(r"C:\Users\it's\x")),
            r"'C:/Users/it'\''s/x'"
        );
    }
}
