//! Forge (GitHub, later GitLab, ...) data shown next to the local history.

pub mod github;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Passed,
    Failed,
    Pending,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct CheckRun {
    pub name: String,
    pub state: CheckState,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CheckSummary {
    pub runs: Vec<CheckRun>,
}

impl CheckSummary {
    pub fn count(&self, state: CheckState) -> usize {
        self.runs.iter().filter(|r| r.state == state).count()
    }

    /// Overall state, the way GitHub's PR badge reports it.
    pub fn overall(&self) -> Option<CheckState> {
        if self.runs.is_empty() {
            None
        } else if self.count(CheckState::Failed) > 0 {
            Some(CheckState::Failed)
        } else if self.count(CheckState::Pending) > 0 {
            Some(CheckState::Pending)
        } else if self.count(CheckState::Passed) > 0 {
            Some(CheckState::Passed)
        } else {
            Some(CheckState::Skipped)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewState {
    #[default]
    None,
    Approved,
    ChangesRequested,
    ReviewRequired,
}

#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub head_ref: String,
    pub base_ref: String,
    pub is_draft: bool,
    /// Head branch lives in a fork.
    pub cross_repo: bool,
    pub url: String,
    pub updated_at: Option<i64>,
    pub review: ReviewState,
    pub checks: CheckSummary,
}

#[derive(Debug, Clone)]
pub struct Label {
    pub name: String,
    /// Hex color without `#`, e.g. `d73a4a`.
    pub color: String,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub labels: Vec<Label>,
    pub assignees: Vec<String>,
    pub url: String,
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ForgeSnapshot {
    /// `owner/name`
    pub repo: String,
    pub repo_url: String,
    pub pulls: Vec<PullRequest>,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone)]
pub enum ForgeError {
    /// The `gh` executable is not installed.
    GhMissing,
    /// `gh` is installed but not logged in.
    NotAuthenticated,
    /// No GitHub remote (or an ambiguous one) for this repository.
    NoRepository(String),
    Other(String),
}

impl std::fmt::Display for ForgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForgeError::GhMissing => write!(
                f,
                "GitHub CLI (gh) was not found. Install it from https://cli.github.com to see pull requests and issues."
            ),
            ForgeError::NotAuthenticated => {
                write!(
                    f,
                    "GitHub CLI is not logged in. Run `gh auth login` in a terminal."
                )
            }
            ForgeError::NoRepository(msg) => {
                write!(f, "No GitHub repository for this folder.\n{msg}")
            }
            ForgeError::Other(msg) => write!(f, "{msg}"),
        }
    }
}
