//! Conflicted files: read the conflict markers git wrote into the working
//! tree, let the user pick a side for every conflict, and render the result.
//!
//! The markers are the source of truth, so edits made in another editor are
//! kept, and the same code handles merges, rebases, cherry-picks, reverts and
//! stash pops. The common ancestor's text is shown when git wrote it
//! (`merge.conflictStyle = diff3` or `zdiff3`).

use std::path::Path;

use anyhow::{Result, bail};

/// Length of the `<<<<<<<` markers (git's default `conflict-marker-size`).
const MARKER_LEN: usize = 7;

/// Which index stages exist for a conflicted path, in `git status` terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    BothModified,
    BothAdded,
    DeletedByUs,
    DeletedByThem,
    AddedByUs,
    AddedByThem,
    BothDeleted,
}

impl ConflictKind {
    pub fn from_stages(base: bool, ours: bool, theirs: bool) -> Self {
        match (base, ours, theirs) {
            (true, true, true) => Self::BothModified,
            (false, true, true) => Self::BothAdded,
            (true, false, true) => Self::DeletedByUs,
            (true, true, false) => Self::DeletedByThem,
            (false, true, false) => Self::AddedByUs,
            (false, false, true) => Self::AddedByThem,
            _ => Self::BothDeleted,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::BothModified => "both modified",
            Self::BothAdded => "both added",
            Self::DeletedByUs => "deleted by us",
            Self::DeletedByThem => "deleted by them",
            Self::AddedByUs => "added by us",
            Self::AddedByThem => "added by them",
            Self::BothDeleted => "both deleted",
        }
    }

    /// Both sides have a version, so the file can be merged line by line.
    pub fn is_textual(self) -> bool {
        matches!(self, Self::BothModified | Self::BothAdded)
    }

    pub fn has_ours(self) -> bool {
        matches!(
            self,
            Self::BothModified | Self::BothAdded | Self::DeletedByThem | Self::AddedByUs
        )
    }

    pub fn has_theirs(self) -> bool {
        matches!(
            self,
            Self::BothModified | Self::BothAdded | Self::DeletedByUs | Self::AddedByThem
        )
    }
}

/// One `<<<<<<< ... >>>>>>>` region. Every text ends with a line break (or is empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub ours: String,
    /// The common ancestor's text, when git wrote it (diff3/zdiff3 style).
    pub base: Option<String>,
    pub theirs: String,
    /// Text after the markers: `HEAD`, `feature/x`, `1a2b3c4 (Add parser)`.
    pub ours_label: String,
    pub theirs_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Lines git merged on its own.
    Text(String),
    Conflict(Conflict),
}

/// How the user resolved one conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Ours,
    Theirs,
    OursThenTheirs,
    TheirsThenOurs,
    /// Hand-edited text (always uses `\n`; converted back for CRLF files).
    Custom(String),
}

impl Choice {
    pub fn label(&self) -> &'static str {
        match self {
            Choice::Ours => "ours",
            Choice::Theirs => "theirs",
            Choice::OursThenTheirs => "both, ours first",
            Choice::TheirsThenOurs => "both, theirs first",
            Choice::Custom(_) => "edited",
        }
    }
}

impl Conflict {
    fn crlf(&self) -> bool {
        self.ours.contains("\r\n") || self.theirs.contains("\r\n")
    }

    /// The text that replaces the conflict for `choice`.
    pub fn resolve(&self, choice: &Choice) -> String {
        match choice {
            Choice::Ours => self.ours.clone(),
            Choice::Theirs => self.theirs.clone(),
            Choice::OursThenTheirs => join(&self.ours, &self.theirs),
            Choice::TheirsThenOurs => join(&self.theirs, &self.ours),
            Choice::Custom(text) => {
                let mut text = text.replace("\r\n", "\n");
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                if self.crlf() {
                    text = text.replace('\n', "\r\n");
                }
                text
            }
        }
    }

    /// Starting point when the user edits the conflict by hand.
    pub fn edit_text(&self, current: Option<&Choice>) -> String {
        let text = match current {
            Some(choice) => self.resolve(choice),
            None => join(&self.ours, &self.theirs),
        };
        text.replace("\r\n", "\n")
    }
}

fn join(a: &str, b: &str) -> String {
    let mut out = a.to_owned();
    if !out.is_empty() && !out.ends_with('\n') && !b.is_empty() {
        out.push('\n');
    }
    out.push_str(b);
    out
}

/// `Some(label)` if `line` is a conflict marker made of `ch`.
fn marker(line: &str, ch: char) -> Option<&str> {
    let body = line.trim_end_matches(['\n', '\r']);
    let run = body.chars().take_while(|&c| c == ch).count();
    if run != MARKER_LEN {
        return None;
    }
    let rest = &body[MARKER_LEN..];
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix(' ')
    }
}

/// Split a file with conflict markers into merged text and conflicts.
pub fn parse(text: &str) -> Result<Vec<Segment>> {
    enum State {
        Text,
        Ours,
        Base,
        Theirs,
    }
    let mut segments = Vec::new();
    let mut state = State::Text;
    let mut plain = String::new();
    let mut current = Conflict {
        ours: String::new(),
        base: None,
        theirs: String::new(),
        ours_label: String::new(),
        theirs_label: String::new(),
    };

    for (n, line) in text.split_inclusive('\n').enumerate() {
        let line_no = n + 1;
        match state {
            State::Text => match marker(line, '<') {
                Some(label) => {
                    if !plain.is_empty() {
                        segments.push(Segment::Text(std::mem::take(&mut plain)));
                    }
                    current.ours_label = label.to_owned();
                    state = State::Ours;
                }
                None => plain.push_str(line),
            },
            State::Ours => {
                if marker(line, '|').is_some() {
                    current.base = Some(String::new());
                    state = State::Base;
                } else if marker(line, '=') == Some("") {
                    state = State::Theirs;
                } else if marker(line, '<').is_some() {
                    bail!("line {line_no}: conflict marker inside a conflict");
                } else {
                    current.ours.push_str(line);
                }
            }
            State::Base => {
                if marker(line, '=') == Some("") {
                    state = State::Theirs;
                } else if let Some(base) = &mut current.base {
                    base.push_str(line);
                }
            }
            State::Theirs => {
                if let Some(label) = marker(line, '>') {
                    current.theirs_label = label.to_owned();
                    let done = std::mem::replace(
                        &mut current,
                        Conflict {
                            ours: String::new(),
                            base: None,
                            theirs: String::new(),
                            ours_label: String::new(),
                            theirs_label: String::new(),
                        },
                    );
                    segments.push(Segment::Conflict(done));
                    state = State::Text;
                } else if marker(line, '<').is_some() {
                    bail!("line {line_no}: conflict marker inside a conflict");
                } else {
                    current.theirs.push_str(line);
                }
            }
        }
    }
    if !matches!(state, State::Text) {
        bail!("a conflict is missing its closing >>>>>>> marker");
    }
    if !plain.is_empty() {
        segments.push(Segment::Text(plain));
    }
    Ok(segments)
}

pub fn conflict_count(segments: &[Segment]) -> usize {
    segments
        .iter()
        .filter(|s| matches!(s, Segment::Conflict(_)))
        .count()
}

/// The file with every conflict replaced by its choice (one per conflict, in
/// order); `None` while a conflict is still unresolved.
pub fn render(segments: &[Segment], choices: &[Option<Choice>]) -> Option<String> {
    let mut out = String::new();
    let mut choices = choices.iter();
    for segment in segments {
        match segment {
            Segment::Text(text) => out.push_str(text),
            Segment::Conflict(conflict) => {
                let choice = choices.next()?.as_ref()?;
                out.push_str(&conflict.resolve(choice));
            }
        }
    }
    Some(out)
}

/// A conflicted file as the merge view needs it.
#[derive(Debug, Clone)]
pub struct MergeDoc {
    pub path: String,
    pub kind: ConflictKind,
    /// The parsed working-tree file, or why it can't be merged line by line.
    pub segments: Result<Vec<Segment>, String>,
}

pub fn load(root: &Path, path: &str, kind: ConflictKind) -> MergeDoc {
    let segments = if kind.is_textual() {
        read_text(&root.join(path)).and_then(|text| parse(&text).map_err(|e| e.to_string()))
    } else {
        Err(match kind {
            ConflictKind::DeletedByUs => "We deleted this file; they changed it.",
            ConflictKind::DeletedByThem => "They deleted this file; we changed it.",
            ConflictKind::AddedByUs => "Only our side has this file (it was renamed or moved).",
            ConflictKind::AddedByThem => "Only their side has this file (it was renamed or moved).",
            _ => "Both sides deleted this file.",
        }
        .to_owned())
    };
    MergeDoc {
        path: path.to_owned(),
        kind,
        segments,
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Can't read the file: {e}"))?;
    if bytes.contains(&0) {
        return Err("Binary file: pick one side's version.".to_owned());
    }
    String::from_utf8(bytes)
        .map_err(|_| "Not UTF-8 text: pick one side's version or edit it elsewhere.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MERGE_STYLE: &str = "\
fn main() {
<<<<<<< HEAD
    println!(\"hello\");
=======
    println!(\"hi\");
>>>>>>> feature
}
";

    #[test]
    fn parses_merge_style_markers() {
        let segments = parse(MERGE_STYLE).unwrap();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], Segment::Text("fn main() {\n".into()));
        let Segment::Conflict(c) = &segments[1] else {
            panic!("expected a conflict")
        };
        assert_eq!(c.ours, "    println!(\"hello\");\n");
        assert_eq!(c.theirs, "    println!(\"hi\");\n");
        assert_eq!(c.base, None);
        assert_eq!(
            (c.ours_label.as_str(), c.theirs_label.as_str()),
            ("HEAD", "feature")
        );
        assert_eq!(segments[2], Segment::Text("}\n".into()));
    }

    #[test]
    fn parses_diff3_base_and_keeps_crlf() {
        let text = "a\r\n<<<<<<< ours\r\nb1\r\n||||||| base\r\nb\r\n=======\r\nb2\r\n>>>>>>> theirs\r\nc\r\n";
        let segments = parse(text).unwrap();
        let Segment::Conflict(c) = &segments[1] else {
            panic!("expected a conflict")
        };
        assert_eq!(c.base.as_deref(), Some("b\r\n"));
        assert_eq!(c.ours, "b1\r\n");
        // Untouched text renders back byte for byte.
        let both = render(&segments, &[Some(Choice::OursThenTheirs)]).unwrap();
        assert_eq!(both, "a\r\nb1\r\nb2\r\nc\r\n");
        // Hand edits use \n and get the file's line endings back.
        let custom = render(&segments, &[Some(Choice::Custom("x\ny".into()))]).unwrap();
        assert_eq!(custom, "a\r\nx\r\ny\r\nc\r\n");
    }

    #[test]
    fn longer_markers_from_recursive_merges_are_content() {
        let text = "<<<<<<< HEAD\nours\n||||||| base\n<<<<<<<<< inner\nx\n>>>>>>>>> inner\n=======\ntheirs\n>>>>>>> other\n";
        let segments = parse(text).unwrap();
        assert_eq!(conflict_count(&segments), 1);
        let Segment::Conflict(c) = &segments[0] else {
            panic!("expected a conflict")
        };
        assert!(c.base.as_deref().unwrap().contains("<<<<<<<<< inner"));
    }

    #[test]
    fn render_needs_every_conflict_resolved() {
        let segments = parse(&format!("{MERGE_STYLE}{MERGE_STYLE}")).unwrap();
        assert_eq!(conflict_count(&segments), 2);
        assert_eq!(render(&segments, &[Some(Choice::Ours), None]), None);
        let out = render(&segments, &[Some(Choice::Ours), Some(Choice::Theirs)]).unwrap();
        assert_eq!(
            out,
            "fn main() {\n    println!(\"hello\");\n}\nfn main() {\n    println!(\"hi\");\n}\n"
        );
    }

    #[test]
    fn rejects_broken_markers() {
        assert!(parse("<<<<<<< HEAD\na\n=======\nb\n").is_err());
        assert!(parse("<<<<<<< HEAD\n<<<<<<< again\n").is_err());
        // Without markers the whole file is plain text.
        assert_eq!(
            parse("plain\n").unwrap(),
            vec![Segment::Text("plain\n".into())]
        );
    }

    #[test]
    fn kinds_from_stages() {
        assert_eq!(
            ConflictKind::from_stages(true, true, true),
            ConflictKind::BothModified
        );
        assert_eq!(
            ConflictKind::from_stages(true, false, true),
            ConflictKind::DeletedByUs
        );
        assert_eq!(
            ConflictKind::from_stages(true, true, false),
            ConflictKind::DeletedByThem
        );
        assert!(ConflictKind::DeletedByUs.has_theirs() && !ConflictKind::DeletedByUs.has_ours());
    }
}
