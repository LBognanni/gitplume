//! Find in files: in-memory glob filtering of the file list, so only the
//! paths the filter selects are ever handed to `git grep`.

/// Whether `path` (already lowercase) matches glob `pattern` (already
/// lowercase): `*` matches any run of characters except `/`, `**` matches
/// any run of characters including `/`, and `?` matches one character.
fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let path: Vec<char> = path.chars().collect();
    matches(&pattern, &path)
}

fn matches(pattern: &[char], path: &[char]) -> bool {
    match pattern.first() {
        None => path.is_empty(),
        Some('*') => {
            if pattern.get(1) == Some(&'*') {
                let rest = &pattern[2..];
                (0..=path.len()).any(|i| matches(rest, &path[i..]))
            } else {
                let rest = &pattern[1..];
                // `*` consumes characters up to (not including) the next `/`.
                (0..=path.len())
                    .take_while(|&i| i == 0 || path[i - 1] != '/')
                    .any(|i| matches(rest, &path[i..]))
            }
        }
        Some('?') => !path.is_empty() && matches(&pattern[1..], &path[1..]),
        Some(&c) => path.first() == Some(&c) && matches(&pattern[1..], &path[1..]),
    }
}

/// Paths of `files` (each paired with its lowercase form) selected by
/// comma-separated glob `filter`. An empty filter selects every file. A
/// pattern without `/` matches the file name; one with `/` matches the
/// whole path.
pub fn filter_paths(files: &[(String, String)], filter: &str) -> Vec<String> {
    let patterns: Vec<String> = filter
        .split(',')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    if patterns.is_empty() {
        return files.iter().map(|(path, _)| path.clone()).collect();
    }
    files
        .iter()
        .filter(|(_, lower)| {
            let name = lower.rsplit('/').next().unwrap_or(lower);
            patterns.iter().any(|pattern| {
                if pattern.contains('/') {
                    glob_match(pattern, lower)
                } else {
                    glob_match(pattern, name)
                }
            })
        })
        .map(|(path, _)| path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(paths: &[&str]) -> Vec<(String, String)> {
        paths
            .iter()
            .map(|path| (path.to_string(), path.to_lowercase()))
            .collect()
    }

    #[test]
    fn empty_filter_selects_every_file() {
        let files = index(&["src/app.rs", "README.md"]);
        assert_eq!(filter_paths(&files, ""), ["src/app.rs", "README.md"]);
        assert_eq!(filter_paths(&files, "  "), ["src/app.rs", "README.md"]);
    }

    #[test]
    fn pattern_without_slash_matches_the_file_name() {
        let files = index(&["src/app.rs", "src/git.rs", "README.md"]);
        assert_eq!(filter_paths(&files, "*.rs"), ["src/app.rs", "src/git.rs"]);
    }

    #[test]
    fn pattern_with_slash_matches_the_whole_path() {
        let files = index(&["src/app.rs", "tests/app.rs", "src/git.rs"]);
        assert_eq!(
            filter_paths(&files, "src/*.rs"),
            ["src/app.rs", "src/git.rs"]
        );
    }

    #[test]
    fn single_star_in_a_path_pattern_matches_one_folder_level() {
        let files = index(&["src/sub/mod.rs", "src/mod.rs", "src/deep/sub/mod.rs"]);
        assert_eq!(filter_paths(&files, "src/*/mod.rs"), ["src/sub/mod.rs"]);
    }

    #[test]
    fn double_star_spans_folders() {
        let files = index(&["src/a.rs", "src/sub/b.rs", "tests/c.rs"]);
        assert_eq!(filter_paths(&files, "src/**"), ["src/a.rs", "src/sub/b.rs"]);
        assert_eq!(
            filter_paths(&files, "**/*.rs"),
            files.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn comma_separated_patterns_union_their_matches() {
        let files = index(&["src/app.rs", "README.md", "Cargo.toml"]);
        assert_eq!(
            filter_paths(&files, "*.rs, Cargo.*"),
            ["src/app.rs", "Cargo.toml"]
        );
    }

    #[test]
    fn matching_is_case_insensitive() {
        let files = index(&["SRC/App.RS"]);
        assert_eq!(filter_paths(&files, "*.rs"), ["SRC/App.RS"]);
    }

    #[test]
    fn question_mark_matches_one_character() {
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(!glob_match("a?c", "abbc"));
    }

    #[test]
    fn single_star_matches_a_whole_folder_name_but_not_across_it() {
        assert!(glob_match("src/*/mod.rs", "src/foo/mod.rs"));
        assert!(glob_match("s*/a.rs", "src/a.rs"));
        assert!(!glob_match("a*", "a/b"), "* must not cross a /");
        assert!(!glob_match("src/*/mod.rs", "src/foo/bar/mod.rs"));
    }
}
