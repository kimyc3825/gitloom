//! GitHub through the GitHub CLI (`gh`).
//!
//! Reusing `gh` means no OAuth flow or token storage of our own: whoever is
//! logged in to `gh` is who gitloom talks to GitHub as.

use std::path::Path;
use std::process::Stdio;

use serde::Deserialize;
use serde_json::Value;

use super::{
    CheckRun, CheckState, CheckSummary, ForgeError, ForgeSnapshot, Issue, Label, PullRequest,
    ReviewState,
};
use crate::util;

const LIST_LIMIT: &str = "100";
const PR_FIELDS: &str = "number,title,author,headRefName,baseRefName,isDraft,isCrossRepository,url,updatedAt,reviewDecision,statusCheckRollup";
const ISSUE_FIELDS: &str = "number,title,author,labels,assignees,url,updatedAt";

pub fn load(root: &Path) -> Result<ForgeSnapshot, ForgeError> {
    // The three calls are independent; run them side by side.
    let (repo, pulls, issues) = std::thread::scope(|s| {
        let repo = s.spawn(|| gh(root, &["repo", "view", "--json", "nameWithOwner,url"]));
        let pulls = s.spawn(|| {
            gh(
                root,
                &[
                    "pr", "list", "--state", "open", "--limit", LIST_LIMIT, "--json", PR_FIELDS,
                ],
            )
        });
        let issues = s.spawn(|| {
            gh(
                root,
                &[
                    "issue",
                    "list",
                    "--state",
                    "open",
                    "--limit",
                    LIST_LIMIT,
                    "--json",
                    ISSUE_FIELDS,
                ],
            )
        });
        let join = |h: std::thread::ScopedJoinHandle<'_, Result<String, ForgeError>>| {
            h.join()
                .unwrap_or_else(|_| Err(ForgeError::Other("gh worker panicked".into())))
        };
        (join(repo), join(pulls), join(issues))
    });

    let repo: GhRepo = parse(&repo?)?;
    Ok(ForgeSnapshot {
        repo: repo.name_with_owner,
        repo_url: repo.url,
        pulls: parse_pulls(&pulls?)?,
        // Issues can be disabled on a repository; that is not an error for us.
        issues: issues
            .ok()
            .map(|json| parse_issues(&json))
            .transpose()?
            .unwrap_or_default(),
    })
}

/// `gh pr checkout <number>`: creates/updates a local branch for the PR.
pub fn checkout_pr(root: &Path, number: u64) -> Result<String, ForgeError> {
    gh(root, &["pr", "checkout", &number.to_string()])
}

fn gh(root: &Path, args: &[&str]) -> Result<String, ForgeError> {
    let output = util::command("gh")
        .args(args)
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ForgeError::GhMissing,
            _ => ForgeError::Other(format!("failed to run gh: {e}")),
        })?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Ok(if stdout.is_empty() { stderr } else { stdout });
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(classify_error(stderr))
}

fn classify_error(stderr: String) -> ForgeError {
    let lower = stderr.to_lowercase();
    if lower.contains("gh auth login") || lower.contains("not logged in") {
        ForgeError::NotAuthenticated
    } else if lower.contains("no git remotes")
        || lower.contains("none of the git remotes")
        || lower.contains("set-default")
        || lower.contains("not a git repository")
    {
        ForgeError::NoRepository(stderr)
    } else {
        ForgeError::Other(stderr)
    }
}

fn parse<T: for<'de> Deserialize<'de>>(json: &str) -> Result<T, ForgeError> {
    serde_json::from_str(json).map_err(|e| ForgeError::Other(format!("unexpected gh output: {e}")))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRepo {
    name_with_owner: String,
    url: String,
}

#[derive(Deserialize, Default)]
struct GhUser {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPull {
    number: u64,
    title: String,
    #[serde(default)]
    author: Option<GhUser>,
    head_ref_name: String,
    base_ref_name: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    is_cross_repository: bool,
    url: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    status_check_rollup: Option<Vec<Value>>,
}

#[derive(Deserialize)]
struct GhLabel {
    name: String,
    #[serde(default)]
    color: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhIssue {
    number: u64,
    title: String,
    #[serde(default)]
    author: Option<GhUser>,
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default)]
    assignees: Vec<GhUser>,
    url: String,
    #[serde(default)]
    updated_at: String,
}

fn login(user: Option<GhUser>) -> String {
    user.map(|u| u.login)
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "ghost".to_owned())
}

fn parse_pulls(json: &str) -> Result<Vec<PullRequest>, ForgeError> {
    let raw: Vec<GhPull> = parse(json)?;
    Ok(raw
        .into_iter()
        .map(|p| PullRequest {
            number: p.number,
            title: p.title,
            author: login(p.author),
            head_ref: p.head_ref_name,
            base_ref: p.base_ref_name,
            is_draft: p.is_draft,
            cross_repo: p.is_cross_repository,
            url: p.url,
            updated_at: util::parse_github_time(&p.updated_at),
            review: match p.review_decision.as_deref() {
                Some("APPROVED") => ReviewState::Approved,
                Some("CHANGES_REQUESTED") => ReviewState::ChangesRequested,
                Some("REVIEW_REQUIRED") => ReviewState::ReviewRequired,
                _ => ReviewState::None,
            },
            checks: summarize_checks(p.status_check_rollup.as_deref().unwrap_or_default()),
        })
        .collect())
}

fn parse_issues(json: &str) -> Result<Vec<Issue>, ForgeError> {
    let raw: Vec<GhIssue> = parse(json)?;
    Ok(raw
        .into_iter()
        .map(|i| Issue {
            number: i.number,
            title: i.title,
            author: login(i.author),
            labels: i
                .labels
                .into_iter()
                .map(|l| Label {
                    name: l.name,
                    color: l.color,
                })
                .collect(),
            assignees: i.assignees.into_iter().map(|a| a.login).collect(),
            url: i.url,
            updated_at: util::parse_github_time(&i.updated_at),
        })
        .collect())
}

/// `statusCheckRollup` mixes GitHub Actions check runs and legacy commit statuses.
fn summarize_checks(items: &[Value]) -> CheckSummary {
    let runs = items
        .iter()
        .map(|item| {
            let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or_default();
            if text("__typename") == "StatusContext" {
                let state = match text("state") {
                    "SUCCESS" => CheckState::Passed,
                    "FAILURE" | "ERROR" => CheckState::Failed,
                    _ => CheckState::Pending, // PENDING, EXPECTED
                };
                CheckRun {
                    name: text("context").to_owned(),
                    state,
                    url: Some(text("targetUrl").to_owned()).filter(|u| !u.is_empty()),
                }
            } else {
                let state = if text("status") != "COMPLETED" {
                    CheckState::Pending
                } else {
                    match text("conclusion") {
                        "SUCCESS" => CheckState::Passed,
                        "NEUTRAL" | "SKIPPED" | "STALE" => CheckState::Skipped,
                        "" => CheckState::Pending,
                        _ => CheckState::Failed, // FAILURE, CANCELLED, TIMED_OUT, ...
                    }
                };
                let name = match (text("workflowName"), text("name")) {
                    ("", name) => name.to_owned(),
                    (workflow, name) => format!("{workflow} / {name}"),
                };
                CheckRun {
                    name,
                    state,
                    url: Some(text("detailsUrl").to_owned()).filter(|u| !u.is_empty()),
                }
            }
        })
        .collect();
    CheckSummary { runs }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PULLS: &str = r#"[
      {
        "author": {"id": "1", "is_bot": false, "login": "octocat", "name": "Mona"},
        "baseRefName": "main",
        "headRefName": "feature/graph",
        "isCrossRepository": false,
        "isDraft": false,
        "number": 12,
        "reviewDecision": "APPROVED",
        "statusCheckRollup": [
          {"__typename": "CheckRun", "name": "build", "workflowName": "CI",
           "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "https://example.com/1"},
          {"__typename": "CheckRun", "name": "test", "workflowName": "CI",
           "status": "IN_PROGRESS", "conclusion": "", "detailsUrl": ""},
          {"__typename": "StatusContext", "context": "ci/legacy", "state": "FAILURE",
           "targetUrl": "https://example.com/2"}
        ],
        "title": "커밋 그래프 개선",
        "updatedAt": "2023-11-14T22:13:20Z",
        "url": "https://github.com/o/r/pull/12"
      },
      {
        "author": null,
        "baseRefName": "main",
        "headRefName": "patch-1",
        "isCrossRepository": true,
        "isDraft": true,
        "number": 13,
        "reviewDecision": "",
        "statusCheckRollup": [],
        "title": "Docs",
        "updatedAt": "",
        "url": "https://github.com/o/r/pull/13"
      }
    ]"#;

    #[test]
    fn parses_pull_requests_and_checks() {
        let pulls = parse_pulls(PULLS).unwrap();
        assert_eq!(pulls.len(), 2);

        let pr = &pulls[0];
        assert_eq!(pr.number, 12);
        assert_eq!(pr.author, "octocat");
        assert_eq!(pr.head_ref, "feature/graph");
        assert_eq!(pr.review, ReviewState::Approved);
        assert_eq!(pr.updated_at, Some(1_700_000_000));
        assert_eq!(pr.checks.runs.len(), 3);
        assert_eq!(pr.checks.runs[0].name, "CI / build");
        assert_eq!(pr.checks.count(CheckState::Passed), 1);
        assert_eq!(pr.checks.count(CheckState::Pending), 1);
        assert_eq!(pr.checks.count(CheckState::Failed), 1);
        assert_eq!(pr.checks.overall(), Some(CheckState::Failed));

        let draft = &pulls[1];
        assert_eq!(draft.author, "ghost");
        assert!(draft.is_draft && draft.cross_repo);
        assert_eq!(draft.review, ReviewState::None);
        assert_eq!(draft.checks.overall(), None);
        assert_eq!(draft.updated_at, None);
    }

    #[test]
    fn parses_issues() {
        let json = r#"[{"number": 7, "title": "Crash", "author": {"login": "a"},
            "labels": [{"name": "bug", "color": "d73a4a", "id": "x", "description": ""}],
            "assignees": [{"login": "b"}], "url": "https://github.com/o/r/issues/7",
            "updatedAt": "1970-01-01T00:00:00Z"}]"#;
        let issues = parse_issues(json).unwrap();
        assert_eq!(issues[0].labels[0].name, "bug");
        assert_eq!(issues[0].assignees, vec!["b".to_owned()]);
        assert_eq!(issues[0].updated_at, Some(0));
    }

    #[test]
    fn classifies_gh_errors() {
        assert!(matches!(
            classify_error("To get started with GitHub CLI, please run:  gh auth login".into()),
            ForgeError::NotAuthenticated
        ));
        assert!(matches!(
            classify_error("none of the git remotes configured for this repository point to a known GitHub host".into()),
            ForgeError::NoRepository(_)
        ));
        assert!(matches!(
            classify_error("HTTP 502".into()),
            ForgeError::Other(_)
        ));
    }
}
