//! Commit search over the loaded history.

use crate::git::repo::CommitInfo;

/// Shortest query treated as a possible commit id prefix.
const MIN_SHA_PREFIX: usize = 4;

/// Rows whose summary, author name or author email contain `query`
/// (case-insensitive), or whose commit id starts with it.
pub fn find(commits: &[CommitInfo], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let sha_like = query.len() >= MIN_SHA_PREFIX && query.chars().all(|c| c.is_ascii_hexdigit());
    commits
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.search_text.contains(&query) || (sha_like && c.id.to_string().starts_with(&query))
        })
        .map(|(row, _)| row)
        .collect()
}

#[cfg(test)]
mod tests {
    use git2::Oid;

    use super::*;
    use crate::git::repo::search_text;

    fn commit(id: &str, summary: &str, author: &str, email: &str) -> CommitInfo {
        CommitInfo {
            id: Oid::from_str(id).unwrap(),
            short_id: id[..7].to_owned(),
            summary: summary.to_owned(),
            author_name: author.to_owned(),
            time: 0,
            parents: Vec::new(),
            search_text: search_text(summary, author, email),
        }
    }

    fn sample() -> Vec<CommitInfo> {
        vec![
            commit(
                "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
                "Fix crash on empty repository",
                "Mona Lisa",
                "mona@example.com",
            ),
            commit(
                "deadbeef00000000000000000000000000000000",
                "커밋 그래프 레이아웃 추가",
                "김하나",
                "hana@example.com",
            ),
            commit(
                "0123456789abcdef0123456789abcdef01234567",
                "Add search box",
                "Octo Cat",
                "octocat@example.com",
            ),
        ]
    }

    #[test]
    fn matches_message_author_and_email_case_insensitively() {
        let commits = sample();
        assert_eq!(find(&commits, "CRASH"), vec![0]);
        assert_eq!(find(&commits, "그래프"), vec![1]);
        assert_eq!(find(&commits, "octo cat"), vec![2]);
        assert_eq!(find(&commits, "example.com"), vec![0, 1, 2]);
        assert!(find(&commits, "   ").is_empty());
    }

    #[test]
    fn hex_queries_match_commit_id_prefixes_only() {
        let commits = sample();
        assert_eq!(find(&commits, "deadbeef"), vec![1]);
        assert_eq!(find(&commits, "A1B2"), vec![0]);
        // Too short to be treated as an id; "add" only matches the message.
        assert_eq!(find(&commits, "add"), vec![2]);
        // Hex digits in the middle of an id don't count.
        assert!(find(&commits, "beef0000").is_empty());
    }
}
