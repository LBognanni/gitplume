//! Git adapter: runs porcelain commands and parses their output.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::model::{Commit, CommitFile, DiffEntry, FileEntry, GrepMatch, RepoState, Side};

const MAX_COMMITS: usize = 100;
const CONTEXT: &str = "-U1000000";
const NON_UTF8_REASON: &str = "Non-UTF-8 paths are not supported.";
/// A grep match's text, trimmed to this many characters.
const MAX_MATCH_TEXT: usize = 200;
/// Pathspec argv budget per `grep` call, comfortably under each platform's
/// command-line length limit (macOS's is the tightest non-Windows one, at
/// about 1 MiB including the environment).
#[cfg(windows)]
const MAX_ARGV_BYTES: usize = 28_000;
#[cfg(not(windows))]
const MAX_ARGV_BYTES: usize = 256_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// Git exited with a code outside the allowed set.
    Failed { code: Option<i32>, stderr: String },
    /// Git could not be run, or its output was unusable.
    Other(String),
}

impl fmt::Display for GitError {
    /// The user-facing text: Git's trimmed stderr when present.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed { stderr, .. } if !stderr.trim().is_empty() => f.write_str(stderr.trim()),
            Self::Failed {
                code: Some(code), ..
            } => {
                write!(f, "git exited with status {code}")
            }
            Self::Failed { code: None, .. } => f.write_str("git was terminated by a signal"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for GitError {}

pub trait Runner: Send + Sync {
    /// Run `git --literal-pathspecs <args>` in `cwd` with GIT_OPTIONAL_LOCKS=0.
    fn run(&self, cwd: &Path, args: &[&str], allowed_codes: &[i32]) -> Result<String, GitError>;
}

/// Runs the real `git` executable.
pub struct SystemRunner;

impl SystemRunner {
    fn command(cwd: &Path, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .arg("--literal-pathspecs")
            .args(args)
            .current_dir(cwd)
            .env("GIT_OPTIONAL_LOCKS", "0");
        command
    }
}

fn check_output(
    code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    allowed_codes: &[i32],
) -> Result<String, GitError> {
    if code.is_some_and(|code| allowed_codes.contains(&code)) {
        Ok(String::from_utf8_lossy(stdout).into_owned())
    } else {
        Err(GitError::Failed {
            code,
            stderr: String::from_utf8_lossy(stderr).into_owned(),
        })
    }
}

impl Runner for SystemRunner {
    fn run(&self, cwd: &Path, args: &[&str], allowed_codes: &[i32]) -> Result<String, GitError> {
        let output = Self::command(cwd, args)
            .output()
            .map_err(|error| GitError::Other(error.to_string()))?;
        check_output(
            output.status.code(),
            &output.stdout,
            &output.stderr,
            allowed_codes,
        )
    }
}

pub trait GitApi: Send + Sync {
    fn status(&self, root: &Path) -> Result<RepoState, GitError>;
    fn files(&self, cwd: &Path) -> Result<Vec<String>, GitError>;
    fn commits(&self, root: &Path) -> Result<Vec<Commit>, GitError>;
    fn commit_files(&self, root: &Path, commit: &Commit) -> Result<Vec<CommitFile>, GitError>;
    fn diff(&self, root: &Path, entry: &DiffEntry) -> Result<String, GitError>;
    /// Search `paths` (relative to `cwd`) for `query`, up to `limit` matches
    /// in total. Returns the matches and whether more were cut off. `paths`
    /// are searched in pathspec-sized chunks, each as one `grep` call capped
    /// per file (not per call) at the matches still needed, so a single very
    /// repetitive file cannot flood one chunk's output.
    fn grep(
        &self,
        cwd: &Path,
        query: &str,
        paths: &[String],
        limit: usize,
    ) -> Result<(Vec<GrepMatch>, bool), GitError>;
    fn stage(&self, root: &Path, paths: &[&str]) -> Result<(), GitError>;
    fn unstage(&self, root: &Path, paths: &[&str]) -> Result<(), GitError>;
    fn restore(&self, root: &Path, paths: &[&str]) -> Result<(), GitError>;
    fn clean(&self, root: &Path, paths: &[&str]) -> Result<(), GitError>;
    fn git_dirs(&self, root: &Path) -> Result<(PathBuf, PathBuf), GitError>;
}

/// `GitApi` backed by Git's command line through a `Runner`.
pub struct CliGit<R: Runner> {
    pub runner: R,
}

impl<R: Runner> CliGit<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Result<String, GitError> {
        self.runner.run(cwd, args, &[0])
    }

    fn run_with_paths(&self, root: &Path, args: &[&str], paths: &[&str]) -> Result<(), GitError> {
        let args: Vec<&str> = args.iter().chain(paths).copied().collect();
        self.run(root, &args).map(drop)
    }

    /// HEAD's object ID, or an empty string for an unborn branch.
    fn head(&self, root: &Path) -> Result<String, GitError> {
        let output =
            self.runner
                .run(root, &["rev-parse", "--verify", "--quiet", "HEAD"], &[0, 1])?;
        Ok(output.trim().to_string())
    }

    /// Root of the repository containing `path`.
    pub fn repo_root(&self, path: &Path) -> Result<PathBuf, GitError> {
        let output = self.run(path, &["rev-parse", "--show-toplevel"])?;
        Ok(PathBuf::from(output.trim()))
    }

    /// `grep`'s implementation, with the pathspec argv budget as a parameter
    /// so tests can force chunking without huge inputs.
    fn grep_with_budget(
        &self,
        cwd: &Path,
        query: &str,
        paths: &[String],
        limit: usize,
        budget: usize,
    ) -> Result<(Vec<GrepMatch>, bool), GitError> {
        let mut matches = Vec::new();
        for chunk in chunk_paths(paths, budget) {
            let remaining = limit.saturating_sub(matches.len());
            if remaining == 0 {
                return Ok((matches, true));
            }
            // Ask each file for one match more than needed, so finding exactly
            // `remaining` never gets mistaken for a cut-off (as `matching_files`
            // does for file jump).
            let max_count = format!("--max-count={}", remaining + 1);
            let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
            let args: Vec<&str> = [
                "grep",
                "--untracked",
                "-z",
                "-n",
                "-I",
                "-F",
                "-i",
                "--no-color",
                "--no-column",
                "--no-full-name",
                &max_count,
                "-e",
                query,
                "--",
            ]
            .into_iter()
            .chain(refs)
            .collect();
            let output = self.runner.run(cwd, &args, &[0, 1])?;
            let parsed = parse_grep(&output);
            let hit_limit = parsed.len() > remaining;
            matches.extend(parsed.into_iter().take(remaining));
            if hit_limit {
                return Ok((matches, true));
            }
        }
        Ok((matches, false))
    }
}

impl<R: Runner> GitApi for CliGit<R> {
    fn status(&self, root: &Path) -> Result<RepoState, GitError> {
        let output = self.run(
            root,
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all",
            ],
        )?;
        Ok(parse_status(&output, root))
    }

    fn files(&self, cwd: &Path) -> Result<Vec<String>, GitError> {
        let output = self.run(
            cwd,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
                "--",
                ".",
            ],
        )?;
        let mut files: Vec<String> = output
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect();
        files.sort();
        Ok(files)
    }

    fn commits(&self, root: &Path) -> Result<Vec<Commit>, GitError> {
        if self.head(root)?.is_empty() {
            return Ok(Vec::new());
        }
        let max_count = format!("--max-count={MAX_COMMITS}");
        let output = self.run(
            root,
            &["log", &max_count, "-z", "--format=%H%x00%h%x00%P%x00%s"],
        )?;
        Ok(parse_commits(&output))
    }

    fn commit_files(&self, root: &Path, commit: &Commit) -> Result<Vec<CommitFile>, GitError> {
        let output = match &commit.parent {
            None => self.run(
                root,
                &[
                    "diff-tree",
                    "--root",
                    "--no-commit-id",
                    "--name-status",
                    "--no-renames",
                    "-r",
                    "-z",
                    &commit.hash,
                ],
            )?,
            Some(parent) => self.run(
                root,
                &[
                    "diff",
                    "--name-status",
                    "--no-renames",
                    "-z",
                    parent,
                    &commit.hash,
                ],
            )?,
        };
        Ok(parse_commit_files(&output, commit))
    }

    fn diff(&self, root: &Path, entry: &DiffEntry) -> Result<String, GitError> {
        let base = [CONTEXT, "--no-color", "--no-ext-diff"];
        match entry {
            DiffEntry::Commit(file) => match &file.parent {
                None => self.run(
                    root,
                    &[
                        &["show", "--format="][..],
                        &base,
                        &[&file.commit_hash, "--", &file.path],
                    ]
                    .concat(),
                ),
                Some(parent) => self.run(
                    root,
                    &[
                        &["diff"][..],
                        &base,
                        &[parent, &file.commit_hash, "--", &file.path],
                    ]
                    .concat(),
                ),
            },
            DiffEntry::File(file) if file.side == Side::Staged => self.run(
                root,
                &[&["diff", "--cached"][..], &base, &["--", &file.path]].concat(),
            ),
            DiffEntry::File(file) if file.status == '?' => self.runner.run(
                root,
                &[
                    &["diff", "--no-index"][..],
                    &base,
                    &["--", "/dev/null", &file.path],
                ]
                .concat(),
                &[0, 1],
            ),
            DiffEntry::File(file) => {
                self.run(root, &[&["diff"][..], &base, &["--", &file.path]].concat())
            }
        }
    }

    fn grep(
        &self,
        cwd: &Path,
        query: &str,
        paths: &[String],
        limit: usize,
    ) -> Result<(Vec<GrepMatch>, bool), GitError> {
        self.grep_with_budget(cwd, query, paths, limit, MAX_ARGV_BYTES)
    }

    fn stage(&self, root: &Path, paths: &[&str]) -> Result<(), GitError> {
        self.run_with_paths(root, &["add", "--"], paths)
    }

    fn unstage(&self, root: &Path, paths: &[&str]) -> Result<(), GitError> {
        if self.head(root)?.is_empty() {
            self.run_with_paths(root, &["rm", "--cached", "-f", "--"], paths)
        } else {
            self.run_with_paths(root, &["restore", "--staged", "--"], paths)
        }
    }

    fn restore(&self, root: &Path, paths: &[&str]) -> Result<(), GitError> {
        self.run_with_paths(root, &["restore", "--worktree", "--"], paths)
    }

    fn clean(&self, root: &Path, paths: &[&str]) -> Result<(), GitError> {
        self.run_with_paths(root, &["clean", "-f", "--"], paths)
    }

    fn git_dirs(&self, root: &Path) -> Result<(PathBuf, PathBuf), GitError> {
        let output = self.run(
            root,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-dir",
                "--git-common-dir",
            ],
        )?;
        match output.lines().collect::<Vec<_>>()[..] {
            [git_dir, common_dir] => Ok((PathBuf::from(git_dir), PathBuf::from(common_dir))),
            _ => Err(GitError::Other(format!(
                "Unexpected rev-parse output: {output:?}"
            ))),
        }
    }
}

/// Build an entry, marking paths that were not valid UTF-8 as unsupported.
fn entry(path: &str, side: Side, status: char, reason: Option<&str>) -> FileEntry {
    let reason = if path.contains(char::REPLACEMENT_CHARACTER) {
        Some(NON_UTF8_REASON)
    } else {
        reason
    };
    match reason {
        Some(reason) => FileEntry::unsupported(path, side, status, reason),
        None => FileEntry::new(path, side, status),
    }
}

/// Split the XY field of a porcelain v2 record into its two codes.
fn xy(field: &str) -> Option<(char, char)> {
    let mut chars = field.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(x), Some(y), None) => Some((x, y)),
        _ => None,
    }
}

fn parse_status(output: &str, root: &Path) -> RepoState {
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut branch = String::new();
    let mut oid = String::new();

    let mut records = output.split('\0');
    while let Some(record) = records.next() {
        if let Some(head) = record.strip_prefix("# branch.head ") {
            branch = head.to_string();
        } else if let Some(value) = record.strip_prefix("# branch.oid ") {
            oid = value.to_string();
        } else if record.starts_with("1 ") {
            let fields: Vec<&str> = record.splitn(9, ' ').collect();
            let (Some((x, y)), 9) = (xy(fields[1]), fields.len()) else {
                continue;
            };
            let path = fields[8];
            for (code, side, entries) in [
                (x, Side::Staged, &mut staged),
                (y, Side::Unstaged, &mut unstaged),
            ] {
                match code {
                    'A' | 'M' | 'D' => entries.push(entry(path, side, code, None)),
                    'T' => entries.push(entry(path, side, 'M', None)),
                    _ => {}
                }
            }
        } else if let Some(path) = record.strip_prefix("? ") {
            unstaged.push(entry(path, Side::Unstaged, '?', None));
        } else if record.starts_with("2 ") {
            let original = records.next();
            let fields: Vec<&str> = record.splitn(10, ' ').collect();
            let (Some((x, y)), 10, Some(original)) = (xy(fields[1]), fields.len(), original) else {
                continue;
            };
            let change = if fields[8].starts_with('C') {
                "Copy"
            } else {
                "Rename"
            };
            let reason = format!("{change} from {original} is not supported.");
            let path = fields[9];
            for (code, side, entries) in [
                (x, Side::Staged, &mut staged),
                (y, Side::Unstaged, &mut unstaged),
            ] {
                if code != '.' {
                    entries.push(entry(path, side, code, Some(&reason)));
                }
            }
        } else if record.starts_with("u ") {
            let fields: Vec<&str> = record.splitn(11, ' ').collect();
            if fields.len() != 11 || xy(fields[1]).is_none() {
                continue;
            }
            let reason = format!("Conflict ({}) resolution is not supported.", fields[1]);
            unstaged.push(entry(fields[10], Side::Unstaged, 'U', Some(&reason)));
        }
    }

    if branch == "(detached)" && !oid.is_empty() {
        branch = format!("detached at {}", oid.chars().take(7).collect::<String>());
    }
    RepoState {
        root: root.to_path_buf(),
        staged,
        unstaged,
        branch,
    }
}

fn parse_commits(output: &str) -> Vec<Commit> {
    let fields: Vec<&str> = output.split('\0').collect();
    fields
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|[hash, ..]| !hash.is_empty())
        .map(|[hash, short_hash, parents, subject]| Commit {
            hash: hash.to_string(),
            short_hash: short_hash.to_string(),
            parent: parents
                .split(' ')
                .next()
                .filter(|p| !p.is_empty())
                .map(str::to_string),
            subject: subject.to_string(),
        })
        .collect()
}

/// Group `paths` into chunks whose total byte length (plus a separator per
/// path) stays under `budget`; always at least one path per chunk.
fn chunk_paths(paths: &[String], budget: usize) -> Vec<Vec<String>> {
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut size = 0;
    for path in paths {
        let cost = path.len() + 1;
        if !current.is_empty() && size + cost > budget {
            chunks.push(std::mem::take(&mut current));
            size = 0;
        }
        size += cost;
        current.push(path.clone());
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Parse `git grep -z -n` output: records of `path\0line\0text` per line.
fn parse_grep(output: &str) -> Vec<GrepMatch> {
    output
        .lines()
        .filter_map(|record| {
            let mut fields = record.splitn(3, '\0');
            let path = fields.next()?;
            let line: usize = fields.next()?.parse().ok()?;
            let text = fields.next().unwrap_or_default();
            let text = text.trim_start();
            let text = match text.char_indices().nth(MAX_MATCH_TEXT) {
                Some((end, _)) => &text[..end],
                None => text,
            };
            Some(GrepMatch {
                path: path.to_string(),
                line,
                text: text.to_string(),
            })
        })
        .collect()
}

fn parse_commit_files(output: &str, commit: &Commit) -> Vec<CommitFile> {
    let fields: Vec<&str> = output.split('\0').collect();
    fields
        .as_chunks::<2>()
        .0
        .iter()
        .filter_map(|[status, path]| {
            let status = status.chars().next()?;
            (!path.is_empty()).then(|| CommitFile {
                path: path.to_string(),
                status,
                commit_hash: commit.hash.clone(),
                parent: commit.parent.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Call = (PathBuf, Vec<String>, Vec<i32>);
    type Respond = Box<dyn Fn(&[&str]) -> Result<String, GitError> + Send + Sync>;

    struct Recording {
        calls: Mutex<Vec<Call>>,
        respond: Respond,
    }

    impl Runner for Recording {
        fn run(&self, cwd: &Path, args: &[&str], allowed: &[i32]) -> Result<String, GitError> {
            self.calls.lock().unwrap().push((
                cwd.to_path_buf(),
                args.iter().map(|a| a.to_string()).collect(),
                allowed.to_vec(),
            ));
            (self.respond)(args)
        }
    }

    fn git(
        respond: impl Fn(&[&str]) -> Result<String, GitError> + Send + Sync + 'static,
    ) -> CliGit<Recording> {
        CliGit::new(Recording {
            calls: Mutex::new(Vec::new()),
            respond: Box::new(respond),
        })
    }

    fn replying(output: &'static str) -> CliGit<Recording> {
        git(move |_| Ok(output.to_string()))
    }

    fn calls(git: &CliGit<Recording>) -> Vec<Call> {
        git.runner.calls.lock().unwrap().clone()
    }

    fn call(cwd: &str, args: &[&str], allowed: &[i32]) -> Call {
        (
            PathBuf::from(cwd),
            args.iter().map(|a| a.to_string()).collect(),
            allowed.to_vec(),
        )
    }

    fn root() -> &'static Path {
        Path::new("/repository")
    }

    fn staged(path: &str, status: char) -> FileEntry {
        FileEntry::new(path, Side::Staged, status)
    }

    fn unstaged(path: &str, status: char) -> FileEntry {
        FileEntry::new(path, Side::Unstaged, status)
    }

    fn commit(hash: &str, short: &str, parent: Option<&str>, subject: &str) -> Commit {
        Commit {
            hash: hash.into(),
            short_hash: short.into(),
            parent: parent.map(Into::into),
            subject: subject.into(),
        }
    }

    fn commit_file(path: &str, status: char, hash: &str, parent: Option<&str>) -> CommitFile {
        CommitFile {
            path: path.into(),
            status,
            commit_hash: hash.into(),
            parent: parent.map(Into::into),
        }
    }

    #[test]
    fn parse_status_maps_ordinary_changes() {
        let output = "1 M. N... 100644 100644 100644 hash hash staged.txt\0\
                      1 .M N... 100644 100644 100644 hash hash unstaged.txt\0\
                      1 AD N... 100644 100644 100644 hash hash both.txt\0\
                      1 T. N... 100644 100644 100644 hash hash type-change.txt\0";

        let state = parse_status(output, root());

        assert_eq!(state.root, root());
        assert_eq!(
            state.staged,
            [
                staged("staged.txt", 'M'),
                staged("both.txt", 'A'),
                staged("type-change.txt", 'M'),
            ]
        );
        assert_eq!(
            state.unstaged,
            [unstaged("unstaged.txt", 'M'), unstaged("both.txt", 'D')]
        );
    }

    #[test]
    fn parse_status_handles_untracked_rename_and_unmerged_records() {
        let output = "\0? path with spaces.txt\0\
                      2 RM N... 100644 100644 100644 hash hash R100 new.txt\0old.txt\0\
                      u UU N... 100644 100644 100644 100644 hash hash hash conflict.txt\0\
                      x unknown record\0\
                      1 D. N... 100644 100644 100644 hash hash deleted.txt\0\0";

        let state = parse_status(output, root());

        let rename = "Rename from old.txt is not supported.";
        assert_eq!(
            state.staged,
            [
                FileEntry::unsupported("new.txt", Side::Staged, 'R', rename),
                staged("deleted.txt", 'D'),
            ]
        );
        assert_eq!(
            state.unstaged,
            [
                unstaged("path with spaces.txt", '?'),
                FileEntry::unsupported("new.txt", Side::Unstaged, 'M', rename),
                FileEntry::unsupported(
                    "conflict.txt",
                    Side::Unstaged,
                    'U',
                    "Conflict (UU) resolution is not supported.",
                ),
            ]
        );
    }

    #[test]
    fn parse_status_treats_rename_continuation_as_original_path() {
        let state = parse_status(
            "2 R. N... 100644 100644 100644 hash hash R100 new.txt\0? old.txt\0",
            root(),
        );

        assert_eq!(
            state.staged,
            [FileEntry::unsupported(
                "new.txt",
                Side::Staged,
                'R',
                "Rename from ? old.txt is not supported.",
            )]
        );
        assert_eq!(state.unstaged, []);
    }

    #[test]
    fn parse_status_reports_copies_as_unsupported() {
        let state = parse_status(
            "2 C. N... 100644 100644 100644 hash hash C100 copy.txt\0source.txt\0",
            root(),
        );

        assert_eq!(
            state.staged,
            [FileEntry::unsupported(
                "copy.txt",
                Side::Staged,
                'C',
                "Copy from source.txt is not supported.",
            )]
        );
        assert_eq!(state.unstaged, []);
    }

    #[test]
    fn parse_status_reports_non_utf8_paths_as_unsupported() {
        let state = parse_status("? bad\u{FFFD}name.txt\0", root());

        assert_eq!(
            state.unstaged,
            [FileEntry::unsupported(
                "bad\u{FFFD}name.txt",
                Side::Unstaged,
                '?',
                NON_UTF8_REASON,
            )]
        );
    }

    #[test]
    fn parse_status_reports_branch() {
        for (output, expected) in [
            (
                "# branch.oid abcdef123456\0# branch.head feature/test\0",
                "feature/test",
            ),
            (
                "# branch.oid abcdef123456\0# branch.head (detached)\0",
                "detached at abcdef1",
            ),
            ("# branch.oid (initial)\0# branch.head main\0", "main"),
        ] {
            assert_eq!(parse_status(output, root()).branch, expected);
        }
    }

    #[test]
    fn status_requests_branch_metadata() {
        let git = replying("# branch.oid hash\0# branch.head main\0");

        let state = git.status(root()).unwrap();

        assert_eq!(state.branch, "main");
        assert_eq!(
            calls(&git),
            [call(
                "/repository",
                &[
                    "status",
                    "--porcelain=v2",
                    "--branch",
                    "-z",
                    "--untracked-files=all"
                ],
                &[0],
            )]
        );
    }

    #[test]
    fn run_preserves_command_settings_and_environment() {
        let command = SystemRunner::command(root(), &["status", "--short"]);

        assert_eq!(command.get_program(), "git");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--literal-pathspecs", "status", "--short"]
        );
        assert_eq!(command.get_current_dir(), Some(root()));
        // Only the lock override is set explicitly; everything else is inherited.
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            [("GIT_OPTIONAL_LOCKS".as_ref(), Some("0".as_ref()))]
        );
    }

    #[test]
    fn run_returns_output_for_allowed_code() {
        assert_eq!(
            check_output(Some(0), b"output", b"", &[0]).unwrap(),
            "output"
        );
    }

    #[test]
    fn run_rejects_nonzero_return_codes_by_default() {
        assert!(matches!(
            check_output(Some(1), b"diff", b"", &[0]),
            Err(GitError::Failed { code: Some(1), .. })
        ));
    }

    #[test]
    fn run_accepts_explicit_return_code_one() {
        assert_eq!(
            check_output(Some(1), b"diff", b"", &[0, 1]).unwrap(),
            "diff"
        );
    }

    #[test]
    fn run_rejects_return_code_two_when_one_is_allowed() {
        assert!(matches!(
            check_output(Some(2), b"error", b"", &[0, 1]),
            Err(GitError::Failed { code: Some(2), .. })
        ));
    }

    #[test]
    fn error_message_prefers_git_stderr() {
        let error = check_output(
            Some(128),
            b"",
            b"fatal: Unable to create index.lock\n",
            &[0],
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "fatal: Unable to create index.lock");
    }

    #[test]
    fn error_message_falls_back_to_error_text() {
        let error = check_output(Some(128), b"", b"  \n", &[0]).unwrap_err();

        assert_eq!(error.to_string(), "git exited with status 128");
    }

    #[test]
    fn diff_builds_command_and_forwards_output() {
        let cases: [(DiffEntry, &[&str], &[i32]); 5] = [
            (
                DiffEntry::File(staged("staged file.txt", 'M')),
                &[
                    "diff",
                    "--cached",
                    "-U1000000",
                    "--no-color",
                    "--no-ext-diff",
                    "--",
                    "staged file.txt",
                ],
                &[0],
            ),
            (
                DiffEntry::File(unstaged("-tracked file.txt", 'M')),
                &[
                    "diff",
                    "-U1000000",
                    "--no-color",
                    "--no-ext-diff",
                    "--",
                    "-tracked file.txt",
                ],
                &[0],
            ),
            (
                DiffEntry::File(unstaged("-untracked file.txt", '?')),
                &[
                    "diff",
                    "--no-index",
                    "-U1000000",
                    "--no-color",
                    "--no-ext-diff",
                    "--",
                    "/dev/null",
                    "-untracked file.txt",
                ],
                &[0, 1],
            ),
            (
                DiffEntry::Commit(commit_file(
                    "historical file.txt",
                    'M',
                    "commit",
                    Some("parent"),
                )),
                &[
                    "diff",
                    "-U1000000",
                    "--no-color",
                    "--no-ext-diff",
                    "parent",
                    "commit",
                    "--",
                    "historical file.txt",
                ],
                &[0],
            ),
            (
                DiffEntry::Commit(commit_file("initial file.txt", 'A', "root", None)),
                &[
                    "show",
                    "--format=",
                    "-U1000000",
                    "--no-color",
                    "--no-ext-diff",
                    "root",
                    "--",
                    "initial file.txt",
                ],
                &[0],
            ),
        ];
        for (entry, args, allowed) in cases {
            let git = replying("diff output\n");

            assert_eq!(git.diff(root(), &entry).unwrap(), "diff output\n");
            assert_eq!(calls(&git), [call("/repository", args, allowed)]);
        }
    }

    #[test]
    fn files_requests_scoped_tracked_and_non_ignored_paths() {
        let git = replying("z-last.txt\0directory/a file.py\0a-first.txt\0");

        let cwd = Path::new("/repository/subdirectory");
        assert_eq!(
            git.files(cwd).unwrap(),
            ["a-first.txt", "directory/a file.py", "z-last.txt"]
        );
        assert_eq!(
            calls(&git),
            [call(
                "/repository/subdirectory",
                &[
                    "ls-files",
                    "-z",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "--",
                    "."
                ],
                &[0],
            )]
        );
    }

    #[test]
    fn parse_commits_keeps_first_parent_and_subject() {
        let output = "hash-one\0short-one\0parent-one parent-two\0Merge a branch\0\
                      hash-two\0short-two\0\0Initial commit\0";

        assert_eq!(
            parse_commits(output),
            [
                commit(
                    "hash-one",
                    "short-one",
                    Some("parent-one"),
                    "Merge a branch"
                ),
                commit("hash-two", "short-two", None, "Initial commit"),
            ]
        );
    }

    #[test]
    fn commits_requests_at_most_100_from_current_branch() {
        let git = git(|args| {
            Ok(if args[0] == "rev-parse" {
                "hash\n".into()
            } else {
                "hash\0short\0\0Subject\0".into()
            })
        });

        assert_eq!(
            git.commits(root()).unwrap(),
            [commit("hash", "short", None, "Subject")]
        );
        assert_eq!(
            calls(&git),
            [
                call(
                    "/repository",
                    &["rev-parse", "--verify", "--quiet", "HEAD"],
                    &[0, 1]
                ),
                call(
                    "/repository",
                    &[
                        "log",
                        "--max-count=100",
                        "-z",
                        "--format=%H%x00%h%x00%P%x00%s"
                    ],
                    &[0],
                ),
            ]
        );
    }

    #[test]
    fn commits_skips_log_on_unborn_branch() {
        let git = replying("");

        assert_eq!(git.commits(root()).unwrap(), []);
        assert_eq!(
            calls(&git),
            [call(
                "/repository",
                &["rev-parse", "--verify", "--quiet", "HEAD"],
                &[0, 1]
            )]
        );
    }

    #[test]
    fn parse_commit_files_preserves_status_and_path() {
        let commit = commit("hash", "short", Some("parent"), "Subject");

        assert_eq!(
            parse_commit_files("M\0path with spaces.py\0D\0-old.txt\0", &commit),
            [
                commit_file("path with spaces.py", 'M', "hash", Some("parent")),
                commit_file("-old.txt", 'D', "hash", Some("parent")),
            ]
        );
    }

    #[test]
    fn commit_files_compares_with_first_parent_or_empty_tree() {
        let cases: [(Commit, &[&str]); 2] = [
            (
                commit("root-hash", "root", None, "Initial"),
                &[
                    "diff-tree",
                    "--root",
                    "--no-commit-id",
                    "--name-status",
                    "--no-renames",
                    "-r",
                    "-z",
                    "root-hash",
                ],
            ),
            (
                commit("hash", "short", Some("parent"), "Subject"),
                &[
                    "diff",
                    "--name-status",
                    "--no-renames",
                    "-z",
                    "parent",
                    "hash",
                ],
            ),
        ];
        for (commit, args) in cases {
            let git = replying("A\0added.txt\0");

            assert_eq!(
                git.commit_files(root(), &commit).unwrap(),
                [commit_file(
                    "added.txt",
                    'A',
                    &commit.hash,
                    commit.parent.as_deref()
                )]
            );
            assert_eq!(calls(&git), [call("/repository", args, &[0])]);
        }
    }

    fn grep_match(path: &str, line: usize, text: &str) -> GrepMatch {
        GrepMatch {
            path: path.to_string(),
            line,
            text: text.to_string(),
        }
    }

    #[test]
    fn grep_builds_argv_and_parses_null_delimited_output() {
        let git = replying("a.txt\x002\x00  hello world\0still.txt\nb c.txt\x005\x00hi\n");
        let paths = vec!["a.txt".to_string(), "b c.txt".to_string()];

        let (matches, truncated) = git.grep(root(), "hello", &paths, 50).unwrap();

        assert_eq!(
            matches,
            [
                grep_match("a.txt", 2, "hello world\0still.txt"),
                grep_match("b c.txt", 5, "hi"),
            ]
        );
        assert!(!truncated);
        assert_eq!(
            calls(&git),
            [call(
                "/repository",
                &[
                    "grep",
                    "--untracked",
                    "-z",
                    "-n",
                    "-I",
                    "-F",
                    "-i",
                    "--no-color",
                    "--no-column",
                    "--no-full-name",
                    "--max-count=51", // one more than the limit of 50
                    "-e",
                    "hello",
                    "--",
                    "a.txt",
                    "b c.txt",
                ],
                &[0, 1],
            )]
        );
    }

    #[test]
    fn grep_trims_leading_whitespace_and_caps_match_text() {
        let long = "x".repeat(250);
        let output = format!("a.txt\x001\x00   {long}\n");
        let git = git(move |_| Ok(output.clone()));

        let (matches, _) = git.grep(root(), "x", &["a.txt".to_string()], 10).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].text.chars().count(), 200);
        assert!(!matches[0].text.starts_with(' '));
    }

    #[test]
    fn grep_returns_empty_output_as_no_matches() {
        // Empty stdout is what the real runner returns for the allowed exit
        // code 1 (no match); `check_output`'s own tests cover that mapping.
        let git = replying("");

        let (matches, truncated) = git
            .grep(root(), "nope", &["a.txt".to_string()], 10)
            .unwrap();

        assert_eq!(matches, []);
        assert!(!truncated);
    }

    #[test]
    fn grep_stops_once_the_total_limit_is_reached() {
        let git = replying("a.txt\x001\x00one\nb.txt\x002\x00two\nc.txt\x003\x00three\n");
        let paths = vec![
            "a.txt".to_string(),
            "b.txt".to_string(),
            "c.txt".to_string(),
        ];

        let (matches, truncated) = git.grep(root(), "x", &paths, 2).unwrap();

        assert_eq!(
            matches,
            [grep_match("a.txt", 1, "one"), grep_match("b.txt", 2, "two")]
        );
        assert!(truncated);
        // One call only: the single chunk already filled the quota.
        assert_eq!(calls(&git).len(), 1);
    }

    #[test]
    fn grep_does_not_report_truncation_when_the_limit_is_met_exactly() {
        let git = replying("a.txt\x001\x00one\nb.txt\x002\x00two\n");
        let paths = vec!["a.txt".to_string(), "b.txt".to_string()];

        let (matches, truncated) = git.grep(root(), "x", &paths, 2).unwrap();

        assert_eq!(
            matches,
            [grep_match("a.txt", 1, "one"), grep_match("b.txt", 2, "two")]
        );
        assert!(
            !truncated,
            "exactly `limit` matches and nothing more is not a truncation"
        );
    }

    #[test]
    fn grep_requests_the_remaining_count_per_chunk() {
        let git = CliGit::new(Recording {
            calls: Mutex::new(Vec::new()),
            respond: Box::new(|args| {
                let max_count = args
                    .iter()
                    .find_map(|a| a.strip_prefix("--max-count="))
                    .unwrap();
                Ok(match max_count {
                    // The first chunk sees one more than the full limit;
                    // after it returns one match, the second chunk is asked
                    // for one less (and still one more than what's left).
                    "6" => "a.txt\x001\x00one\n".to_string(),
                    "5" => "b.txt\x002\x00two\n".to_string(),
                    other => panic!("unexpected --max-count={other}"),
                })
            }),
        });
        // A budget of 6 fits one "aN.txt"-shaped path per chunk.
        let paths = vec!["a.txt".to_string(), "b.txt".to_string()];

        let (matches, truncated) = git.grep_with_budget(root(), "x", &paths, 5, 6).unwrap();

        assert_eq!(
            matches,
            [grep_match("a.txt", 1, "one"), grep_match("b.txt", 2, "two")]
        );
        assert!(!truncated);
    }

    #[test]
    fn grep_chunks_paths_across_several_calls_and_stops_at_the_limit() {
        let git = CliGit::new(Recording {
            calls: Mutex::new(Vec::new()),
            respond: Box::new(|_| Ok("a.txt\x001\x00hit\nb.txt\x002\x00hit\n".to_string())),
        });
        // Each path costs 7 bytes ("aN.txt" + separator); a budget of 10
        // fits exactly one path per chunk, forcing several calls.
        let paths: Vec<String> = (0..4).map(|i| format!("a{i}.txt")).collect();

        let (matches, truncated) = git.grep_with_budget(root(), "x", &paths, 3, 10).unwrap();

        // The first chunk (one path) already returns 2 matches; the limit of
        // 3 is reached during the second chunk's single extra match.
        assert_eq!(matches.len(), 3);
        assert!(truncated);
        assert_eq!(calls(&git).len(), 2, "stops before a third chunk runs");
    }

    #[test]
    fn grep_sends_no_command_for_an_empty_path_list() {
        let git = replying("");

        let (matches, truncated) = git.grep(root(), "x", &[], 10).unwrap();

        assert_eq!(matches, []);
        assert!(!truncated);
        assert_eq!(calls(&git), []);
    }

    type Operation = fn(&CliGit<Recording>, &Path, &[&str]) -> Result<(), GitError>;

    #[test]
    fn stage_operations_build_commands_and_ignore_output() {
        let cases: [(Operation, &[&str]); 3] = [
            (
                |g, r, p| g.stage(r, p),
                &["add", "--", "-file with spaces.txt"],
            ),
            (
                |g, r, p| g.restore(r, p),
                &["restore", "--worktree", "--", "-file with spaces.txt"],
            ),
            (
                |g, r, p| g.clean(r, p),
                &["clean", "-f", "--", "-file with spaces.txt"],
            ),
        ];
        for (operation, args) in cases {
            let git = replying("nonempty output");

            assert_eq!(operation(&git, root(), &["-file with spaces.txt"]), Ok(()));
            assert_eq!(calls(&git), [call("/repository", args, &[0])]);
        }
    }

    #[test]
    fn stage_operations_propagate_run_errors() {
        let error = GitError::Failed {
            code: Some(1),
            stderr: String::new(),
        };
        let operations: [Operation; 4] = [
            |g, r, p| g.stage(r, p),
            |g, r, p| g.unstage(r, p),
            |g, r, p| g.restore(r, p),
            |g, r, p| g.clean(r, p),
        ];
        for operation in operations {
            let returned = error.clone();
            let git = git(move |_| Err(returned.clone()));

            assert_eq!(
                operation(&git, root(), &["-file with spaces.txt"]),
                Err(error.clone())
            );
        }
    }

    #[test]
    fn stage_operations_accept_multiple_paths() {
        let git = replying("");

        git.stage(root(), &["one.txt", "two.txt"]).unwrap();

        assert_eq!(
            calls(&git),
            [call(
                "/repository",
                &["add", "--", "one.txt", "two.txt"],
                &[0]
            )]
        );
    }

    #[test]
    fn unstage_handles_existing_and_unborn_heads() {
        let cases: [(&'static str, &[&str]); 2] = [
            (
                "commit-hash\n",
                &["restore", "--staged", "--", "one.txt", "two.txt"],
            ),
            ("", &["rm", "--cached", "-f", "--", "one.txt", "two.txt"]),
        ];
        for (head, args) in cases {
            let git =
                git(move |args| Ok(if args[0] == "rev-parse" { head } else { "" }.to_string()));

            git.unstage(root(), &["one.txt", "two.txt"]).unwrap();

            assert_eq!(
                calls(&git),
                [
                    call(
                        "/repository",
                        &["rev-parse", "--verify", "--quiet", "HEAD"],
                        &[0, 1]
                    ),
                    call("/repository", args, &[0]),
                ]
            );
        }
    }

    #[test]
    fn git_dirs_requests_absolute_worktree_and_common_directories() {
        let git = replying("/repo/.git/worktrees/w\n/repo/.git\n");

        assert_eq!(
            git.git_dirs(Path::new("/work/w")).unwrap(),
            (
                PathBuf::from("/repo/.git/worktrees/w"),
                PathBuf::from("/repo/.git")
            )
        );
        assert_eq!(
            calls(&git),
            [call(
                "/work/w",
                &[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-dir",
                    "--git-common-dir"
                ],
                &[0],
            )]
        );
    }

    #[test]
    fn repo_root_runs_show_toplevel_in_launch_directory() {
        let git = replying("/repository\n");

        assert_eq!(
            git.repo_root(Path::new("/repository/sub")).unwrap(),
            PathBuf::from("/repository")
        );
        assert_eq!(
            calls(&git),
            [call(
                "/repository/sub",
                &["rev-parse", "--show-toplevel"],
                &[0]
            )]
        );
    }
}
