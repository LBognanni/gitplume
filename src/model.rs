use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Staged,
    Unstaged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub side: Side,
    pub status: char,
    pub unsupported_reason: Option<String>,
}

impl FileEntry {
    pub fn new(path: &str, side: Side, status: char) -> Self {
        Self {
            path: path.to_string(),
            side,
            status,
            unsupported_reason: None,
        }
    }

    pub fn unsupported(path: &str, side: Side, status: char, reason: &str) -> Self {
        Self {
            unsupported_reason: Some(reason.to_string()),
            ..Self::new(path, side, status)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub parent: Option<String>,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub path: String,
    pub status: char,
    pub commit_hash: String,
    pub parent: Option<String>,
}

/// One line of a `git grep` match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
    pub path: String,
    /// 1-based line number.
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoState {
    pub root: PathBuf,
    pub staged: Vec<FileEntry>,
    pub unstaged: Vec<FileEntry>,
    pub branch: String,
}

/// Anything whose diff can be shown: a working-tree entry or a historical file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffEntry {
    File(FileEntry),
    Commit(CommitFile),
}
