//! Filesystem watching that reports repository invalidations.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use notify_debouncer_full::new_debouncer;
use notify_debouncer_full::notify::{Event as FileEvent, EventKind, RecursiveMode};

use crate::app::Event;
use crate::git::GitApi;

const DEBOUNCE: Duration = Duration::from_millis(250);

/// What a batch of filesystem events made stale.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Invalidation {
    pub status: bool,
    pub history: bool,
    /// Worktree paths relative to the root, `/`-separated.
    pub changed_paths: BTreeSet<String>,
    pub index_changed: bool,
}

impl Invalidation {
    /// Extend `self` to cover `other` as well.
    pub fn merge(&mut self, other: Invalidation) {
        self.status |= other.status;
        self.history |= other.history;
        self.changed_paths.extend(other.changed_paths);
        self.index_changed |= other.index_changed;
    }
}

/// Watcher lifecycle and change messages.
#[derive(Debug)]
pub enum Watch {
    /// Watching succeeded.
    Started,
    Changed(Invalidation),
    /// Setup failed (before `Started`), or watching ended or failed after it.
    Stopped,
}

/// Classify changed absolute `paths` for the worktree at `root`.
pub fn classify<'a>(
    root: &Path,
    git_dir: &Path,
    common_dir: &Path,
    paths: impl IntoIterator<Item = &'a Path>,
) -> Invalidation {
    let mut result = Invalidation::default();
    for path in paths {
        if let Ok(name) = path.strip_prefix(git_dir) {
            if name == Path::new("index") {
                result.status = true;
                result.index_changed = true;
                continue;
            }
            if name == Path::new("HEAD") {
                result.status = true;
                result.history = true;
                continue;
            }
        }
        if let Ok(name) = path.strip_prefix(common_dir) {
            if name == Path::new("packed-refs") || name.starts_with("refs") {
                result.status = true;
                result.history = true;
            }
            continue;
        }
        if path == root.join(".git") {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(root) {
            result.status = true;
            result
                .changed_paths
                .insert(relative.to_string_lossy().into_owned());
        }
    }
    result
}

/// Reads (including our own searches and previews) do not invalidate anything.
fn classify_events<'a>(
    root: &Path,
    git_dir: &Path,
    common_dir: &Path,
    events: impl IntoIterator<Item = &'a FileEvent>,
) -> Invalidation {
    let paths = events
        .into_iter()
        .filter(|event| !matches!(event.kind, EventKind::Access(_)))
        .flat_map(|event| event.paths.iter());
    classify(root, git_dir, common_dir, paths.map(PathBuf::as_path))
}

/// Watch the repository at `root` on a thread, posting `Event::Watch` messages.
pub fn spawn(git: Arc<dyn GitApi>, root: PathBuf, tx: Sender<Event>) {
    thread::spawn(move || {
        let _ = watch(git.as_ref(), &root, &tx);
        let _ = tx.send(Event::Watch(Watch::Stopped));
    });
}

/// Run the watcher until it fails, ends, or the app is gone.
fn watch(git: &dyn GitApi, root: &Path, tx: &Sender<Event>) -> Result<(), ()> {
    let (git_dir, common_dir) = git.git_dirs(root).map_err(|_| ())?;
    let (events, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(DEBOUNCE, None, events).map_err(|_| ())?;
    debouncer
        .watch(root, RecursiveMode::Recursive)
        .map_err(|_| ())?;
    for dir in [&git_dir, &common_dir] {
        if !dir.starts_with(root) {
            debouncer
                .watch(dir, RecursiveMode::Recursive)
                .map_err(|_| ())?;
        }
    }
    tx.send(Event::Watch(Watch::Started)).map_err(|_| ())?;
    for batch in rx {
        let batch = batch.map_err(|_| ())?;
        let invalidation = classify_events(
            root,
            &git_dir,
            &common_dir,
            batch.iter().map(|event| &event.event),
        );
        if invalidation != Invalidation::default() {
            tx.send(Event::Watch(Watch::Changed(invalidation)))
                .map_err(|_| ())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/work/repo";
    const LINKED_ROOT: &str = "/work/linked";
    const LINKED_GIT: &str = "/work/repo/.git/worktrees/linked";
    const COMMON: &str = "/work/repo/.git";

    fn normal(paths: &[&str]) -> Invalidation {
        let paths: Vec<&Path> = paths.iter().map(Path::new).collect();
        classify(Path::new(ROOT), Path::new(COMMON), Path::new(COMMON), paths)
    }

    fn linked(paths: &[&str]) -> Invalidation {
        let paths: Vec<&Path> = paths.iter().map(Path::new).collect();
        classify(
            Path::new(LINKED_ROOT),
            Path::new(LINKED_GIT),
            Path::new(COMMON),
            paths,
        )
    }

    fn edited(paths: &[&str]) -> Invalidation {
        Invalidation {
            status: true,
            changed_paths: paths.iter().map(|p| p.to_string()).collect(),
            ..Invalidation::default()
        }
    }

    fn index() -> Invalidation {
        Invalidation {
            status: true,
            index_changed: true,
            ..Invalidation::default()
        }
    }

    fn history() -> Invalidation {
        Invalidation {
            status: true,
            history: true,
            ..Invalidation::default()
        }
    }

    #[test]
    fn worktree_edit_invalidates_status_with_relative_path() {
        assert_eq!(normal(&["/work/repo/src/a.py"]), edited(&["src/a.py"]));
    }

    #[test]
    fn access_events_do_not_invalidate_worktree_or_git_metadata() {
        use notify_debouncer_full::notify::event::{AccessKind, AccessMode};

        let events: Vec<_> = [
            AccessKind::Read,
            AccessKind::Open(AccessMode::Read),
            AccessKind::Close(AccessMode::Read),
        ]
        .into_iter()
        .map(|kind| {
            FileEvent::new(EventKind::Access(kind))
                .add_path(PathBuf::from("/work/repo/src/a.py"))
                .add_path(PathBuf::from("/work/repo/.git/index"))
                .add_path(PathBuf::from("/work/repo/.git/HEAD"))
        })
        .collect();
        assert_eq!(
            classify_events(
                Path::new(ROOT),
                Path::new(COMMON),
                Path::new(COMMON),
                &events,
            ),
            Invalidation::default()
        );
    }

    #[test]
    fn changes_are_kept_in_a_batch_containing_access_events() {
        use notify_debouncer_full::notify::event::{
            AccessKind, CreateKind, ModifyKind, RemoveKind,
        };

        let events = [
            FileEvent::new(EventKind::Access(AccessKind::Read))
                .add_path(PathBuf::from("/work/repo/read.py")),
            FileEvent::new(EventKind::Modify(ModifyKind::Any))
                .add_path(PathBuf::from("/work/repo/edited.py")),
            FileEvent::new(EventKind::Create(CreateKind::File))
                .add_path(PathBuf::from("/work/repo/new.py")),
            FileEvent::new(EventKind::Remove(RemoveKind::File))
                .add_path(PathBuf::from("/work/repo/deleted.py")),
            FileEvent::new(EventKind::Modify(ModifyKind::Any))
                .add_path(PathBuf::from("/work/repo/.git/index")),
        ];
        let mut expected = edited(&["edited.py", "new.py", "deleted.py"]);
        expected.index_changed = true;
        assert_eq!(
            classify_events(
                Path::new(ROOT),
                Path::new(COMMON),
                Path::new(COMMON),
                &events,
            ),
            expected
        );
    }

    #[test]
    fn index_change_invalidates_status_and_index() {
        assert_eq!(normal(&["/work/repo/.git/index"]), index());
    }

    #[test]
    fn head_refs_and_packed_refs_invalidate_status_and_history() {
        for name in ["HEAD", "refs/heads/main", "packed-refs"] {
            let path = format!("{COMMON}/{name}");
            assert_eq!(normal(&[&path]), history(), "{name}");
        }
    }

    #[test]
    fn other_git_metadata_is_ignored() {
        let paths = [
            "/work/repo/.git/logs/HEAD",
            "/work/repo/.git/objects/ab/cdef",
            "/work/repo/.git/index.lock",
            "/work/repo/.git/config",
        ];
        assert_eq!(normal(&paths), Invalidation::default());
    }

    #[test]
    fn linked_worktree_splits_metadata_between_directories() {
        assert_eq!(linked(&[&format!("{LINKED_GIT}/index")]), index());
        assert_eq!(linked(&[&format!("{LINKED_GIT}/HEAD")]), history());
        assert_eq!(linked(&[&format!("{COMMON}/refs/heads/main")]), history());
        assert_eq!(linked(&[&format!("{COMMON}/packed-refs")]), history());
    }

    #[test]
    fn linked_worktree_ignores_other_metadata_and_git_file() {
        let paths = [
            "/work/linked/.git".to_string(),
            format!("{LINKED_GIT}/logs/HEAD"),
            format!("{COMMON}/index"),
            format!("{COMMON}/HEAD"),
            format!("{COMMON}/objects/ab"),
        ];
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        assert_eq!(linked(&paths), Invalidation::default());
    }

    #[test]
    fn paths_outside_repository_are_ignored() {
        assert_eq!(normal(&["/elsewhere/file"]), Invalidation::default());
    }

    #[test]
    fn batch_and_merge_combine_invalidations() {
        let mut merged = normal(&[
            "/work/repo/a.txt",
            "/work/repo/.git/index",
            "/work/repo/.git/HEAD",
        ]);
        merged.merge(Invalidation {
            changed_paths: ["b.txt".to_string()].into(),
            ..Invalidation::default()
        });
        assert_eq!(
            merged,
            Invalidation {
                status: true,
                history: true,
                changed_paths: ["a.txt".to_string(), "b.txt".to_string()].into(),
                index_changed: true,
            }
        );
    }
}
