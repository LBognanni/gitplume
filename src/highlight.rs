//! Language registry, per-side parsing, and viewport-range highlight queries.

use std::ops::Range;
use std::sync::OnceLock;

use ratatui::style::Color;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Python,
    Rust,
    Go,
    C,
    Cpp,
    CSharp,
    Java,
    JavaScript,
    Jsx,
    TypeScript,
    Tsx,
    Ruby,
    Php,
    Bash,
    Html,
    Css,
    Json,
    Toml,
    Yaml,
    Markdown,
    Sql,
    Lua,
    Hcl,
    Dockerfile,
    Xml,
    Zig,
    Glsl,
    PowerShell,
}

const LANG_COUNT: usize = Lang::PowerShell as usize + 1;

impl Lang {
    fn from_name(name: &str) -> Option<Self> {
        if name == "dockerfile"
            || name.starts_with("dockerfile.")
            || name == "containerfile"
            || name.starts_with("containerfile.")
        {
            return Some(Self::Dockerfile);
        }
        Some(match name {
            "gemfile" | "rakefile" => Self::Ruby,
            ".bashrc" | ".bash_profile" | ".bash_login" | ".bash_logout" | ".zshrc" | ".zshenv"
            | ".zprofile" | ".zlogin" | ".zlogout" | ".profile" | "pkgbuild" => Self::Bash,
            "cargo.lock" | "uv.lock" => Self::Toml,
            _ => return None,
        })
    }

    fn from_extension(extension: &str) -> Option<Self> {
        Some(match extension {
            "py" | "pyi" => Self::Python,
            "rs" => Self::Rust,
            "go" => Self::Go,
            "c" | "h" => Self::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Self::Cpp,
            "cs" => Self::CSharp,
            "java" => Self::Java,
            "js" | "mjs" | "cjs" => Self::JavaScript,
            "jsx" => Self::Jsx,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "rb" => Self::Ruby,
            "php" => Self::Php,
            "sh" | "bash" | "zsh" => Self::Bash,
            "html" | "htm" => Self::Html,
            "css" => Self::Css,
            "json" | "jsonc" => Self::Json,
            "toml" => Self::Toml,
            "yaml" | "yml" => Self::Yaml,
            "md" | "markdown" => Self::Markdown,
            "sql" => Self::Sql,
            "lua" => Self::Lua,
            "tf" | "tfvars" | "hcl" => Self::Hcl,
            "dockerfile" | "containerfile" => Self::Dockerfile,
            "xml" | "xsd" | "xsl" | "xslt" | "svg" | "plist" => Self::Xml,
            "zig" | "zon" => Self::Zig,
            "glsl" | "vert" | "frag" | "geom" | "tesc" | "tese" | "comp" => Self::Glsl,
            "ps1" | "psm1" | "psd1" => Self::PowerShell,
            _ => return None,
        })
    }

    fn from_shebang(first_line: &str) -> Option<Self> {
        let mut words = first_line.strip_prefix("#!")?.split_whitespace();
        let mut program = words.next()?.rsplit('/').next()?;
        if program == "env" {
            program = words.find(|word| !word.starts_with('-'))?;
        }
        match program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.') {
            "python" => Some(Self::Python),
            "node" => Some(Self::JavaScript),
            "ruby" => Some(Self::Ruby),
            "sh" | "bash" | "zsh" => Some(Self::Bash),
            "pwsh" | "powershell" => Some(Self::PowerShell),
            _ => None,
        }
    }

    fn grammar_parts(self) -> (Language, Vec<&'static str>) {
        use tree_sitter_javascript as js;
        match self {
            Self::Python => (
                tree_sitter_python::LANGUAGE.into(),
                vec![tree_sitter_python::HIGHLIGHTS_QUERY],
            ),
            Self::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                vec![tree_sitter_rust::HIGHLIGHTS_QUERY],
            ),
            Self::Go => (
                tree_sitter_go::LANGUAGE.into(),
                vec![tree_sitter_go::HIGHLIGHTS_QUERY],
            ),
            Self::C => (
                tree_sitter_c::LANGUAGE.into(),
                vec![tree_sitter_c::HIGHLIGHT_QUERY],
            ),
            Self::Cpp => (
                tree_sitter_cpp::LANGUAGE.into(),
                vec![
                    tree_sitter_c::HIGHLIGHT_QUERY,
                    tree_sitter_cpp::HIGHLIGHT_QUERY,
                ],
            ),
            Self::CSharp => (
                tree_sitter_c_sharp::LANGUAGE.into(),
                vec![tree_sitter_c_sharp::HIGHLIGHTS_QUERY],
            ),
            Self::Java => (
                tree_sitter_java::LANGUAGE.into(),
                vec![tree_sitter_java::HIGHLIGHTS_QUERY],
            ),
            Self::JavaScript => (js::LANGUAGE.into(), vec![js::HIGHLIGHT_QUERY]),
            Self::Jsx => (
                js::LANGUAGE.into(),
                vec![js::HIGHLIGHT_QUERY, js::JSX_HIGHLIGHT_QUERY],
            ),
            Self::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                vec![
                    js::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                ],
            ),
            Self::Tsx => (
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                vec![
                    js::HIGHLIGHT_QUERY,
                    js::JSX_HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                ],
            ),
            Self::Ruby => (
                tree_sitter_ruby::LANGUAGE.into(),
                vec![tree_sitter_ruby::HIGHLIGHTS_QUERY],
            ),
            Self::Php => (
                tree_sitter_php::LANGUAGE_PHP.into(),
                vec![tree_sitter_php::HIGHLIGHTS_QUERY],
            ),
            Self::Bash => (
                tree_sitter_bash::LANGUAGE.into(),
                vec![tree_sitter_bash::HIGHLIGHT_QUERY],
            ),
            Self::Html => (
                tree_sitter_html::LANGUAGE.into(),
                vec![tree_sitter_html::HIGHLIGHTS_QUERY],
            ),
            Self::Css => (
                tree_sitter_css::LANGUAGE.into(),
                vec![tree_sitter_css::HIGHLIGHTS_QUERY],
            ),
            Self::Json => (
                tree_sitter_json::LANGUAGE.into(),
                vec![tree_sitter_json::HIGHLIGHTS_QUERY],
            ),
            Self::Toml => (
                tree_sitter_toml_ng::LANGUAGE.into(),
                vec![tree_sitter_toml_ng::HIGHLIGHTS_QUERY],
            ),
            Self::Yaml => (
                tree_sitter_yaml::LANGUAGE.into(),
                vec![tree_sitter_yaml::HIGHLIGHTS_QUERY],
            ),
            Self::Markdown => (
                tree_sitter_md::LANGUAGE.into(),
                vec![tree_sitter_md::HIGHLIGHT_QUERY_BLOCK],
            ),
            Self::Sql => (
                tree_sitter_sequel::LANGUAGE.into(),
                vec![tree_sitter_sequel::HIGHLIGHTS_QUERY],
            ),
            Self::Lua => (
                tree_sitter_lua::LANGUAGE.into(),
                vec![tree_sitter_lua::HIGHLIGHTS_QUERY],
            ),
            Self::Hcl => (
                tree_sitter_hcl::LANGUAGE.into(),
                vec![include_str!("queries/hcl.scm")],
            ),
            Self::Dockerfile => (
                tree_sitter_containerfile::LANGUAGE.into(),
                vec![tree_sitter_containerfile::HIGHLIGHTS_QUERY],
            ),
            Self::Xml => (
                tree_sitter_xml::LANGUAGE_XML.into(),
                vec![tree_sitter_xml::XML_HIGHLIGHT_QUERY],
            ),
            Self::Zig => (
                tree_sitter_zig::LANGUAGE.into(),
                vec![tree_sitter_zig::HIGHLIGHTS_QUERY],
            ),
            Self::Glsl => (
                tree_sitter_glsl::LANGUAGE_GLSL.into(),
                vec![
                    tree_sitter_c::HIGHLIGHT_QUERY,
                    tree_sitter_glsl::HIGHLIGHTS_QUERY,
                ],
            ),
            Self::PowerShell => (
                tree_sitter_powershell::LANGUAGE.into(),
                vec![tree_sitter_powershell::HIGHLIGHTS_QUERY],
            ),
        }
    }

    /// The compiled grammar, or `None` when the language is unusable (plain text).
    fn grammar(self) -> Option<&'static Grammar> {
        static GRAMMARS: [OnceLock<Option<Grammar>>; LANG_COUNT] =
            [const { OnceLock::new() }; LANG_COUNT];
        GRAMMARS[self as usize]
            .get_or_init(|| {
                let (language, parts) = self.grammar_parts();
                let query = Query::new(&language, &parts.join("\n")).ok()?;
                Some(Grammar { language, query })
            })
            .as_ref()
    }
}

/// Detect a language from exact file name, then extension, then a `#!` line.
pub fn detect(path: &str, first_line: &str) -> Option<Lang> {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    Lang::from_name(&name)
        .or_else(|| {
            name.rsplit_once('.')
                .and_then(|(_, ext)| Lang::from_extension(ext))
        })
        .or_else(|| Lang::from_shebang(first_line))
}

struct Grammar {
    language: Language,
    query: Query,
}

/// Map a capture name to a color by its longest matching dotted prefix.
pub fn capture_color(name: &str) -> Color {
    let mut prefix = name;
    loop {
        let color = match prefix {
            "comment" => Some(0x75715e),
            "string" | "character" => Some(0xe6db74),
            "escape" | "number" | "float" | "boolean" | "constant" => Some(0xae81ff),
            "keyword.import" | "include" | "operator" | "tag" | "keyword.operator" => {
                Some(0xf92672)
            }
            "keyword" | "type" | "storage" => Some(0x66d9ef),
            "function" | "method" | "constructor" | "attribute" | "decorator" => Some(0xa6e22e),
            "variable.parameter" | "parameter" | "variable.builtin" => Some(0xfd971f),
            _ => None,
        };
        if let Some(color) = color {
            return Color::from_u32(color);
        }
        match prefix.rsplit_once('.') {
            Some((parent, _)) => prefix = parent,
            None => return Color::from_u32(0xf8f8f2),
        }
    }
}

/// One side of a document: its source, line starts, and syntax tree.
pub struct Source {
    text: String,
    line_starts: Vec<usize>,
    tree: Option<(Tree, &'static Grammar)>,
}

impl Source {
    /// Join `lines` with `\n` and parse them with the language's grammar.
    pub fn new(lang: Option<Lang>, lines: &[&str]) -> Self {
        let text = lines.join("\n");
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        let tree = lang
            .and_then(Lang::grammar)
            .filter(|_| !text.is_empty())
            .and_then(|grammar| {
                let mut parser = Parser::new();
                parser.set_language(&grammar.language).ok()?;
                Some((parser.parse(&text, None)?, grammar))
            });
        Self {
            text,
            line_starts,
            tree,
        }
    }

    fn line_range(&self, line: usize) -> Range<usize> {
        let end = self
            .line_starts
            .get(line + 1)
            .map_or(self.text.len(), |next| next - 1);
        self.line_starts[line]..end
    }

    /// Captures `(byte range, pattern index, color)` found by a query limited to one line.
    fn captures(&self, line: usize) -> Vec<(Range<usize>, usize, Color)> {
        let Some((tree, grammar)) = &self.tree else {
            return Vec::new();
        };
        let names = grammar.query.capture_names();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(self.line_range(line));
        let mut matches = cursor.matches(&grammar.query, tree.root_node(), self.text.as_bytes());
        let mut found = Vec::new();
        while let Some(found_match) = matches.next() {
            for capture in found_match.captures() {
                let name = names[capture.index as usize];
                if !name.starts_with('_') {
                    found.push((
                        capture.node.byte_range(),
                        found_match.pattern_index,
                        capture_color(name),
                    ));
                }
            }
        }
        found
    }

    /// Colored byte ranges within one line, relative to the line start.
    ///
    /// Outer captures are painted before inner ones so the innermost wins; for
    /// identical ranges the lowest pattern index wins.
    pub fn highlight_line(&self, line: usize) -> Vec<(Range<usize>, Color)> {
        let bounds = self.line_range(line);
        let mut captures = self.captures(line);
        captures.sort_by_key(|(range, pattern, _)| std::cmp::Reverse((range.len(), *pattern)));
        let mut colors: Vec<Option<Color>> = vec![None; bounds.len()];
        for (range, _, color) in captures {
            let start = range.start.max(bounds.start) - bounds.start;
            let end = range.end.min(bounds.end).saturating_sub(bounds.start);
            for cell in colors.iter_mut().take(end).skip(start) {
                *cell = Some(color);
            }
        }
        let mut runs: Vec<(Range<usize>, Color)> = Vec::new();
        for (i, color) in colors.into_iter().enumerate() {
            let Some(color) = color else { continue };
            match runs.last_mut() {
                Some((range, last)) if range.end == i && *last == color => range.end += 1,
                _ => runs.push((i..i + 1, color)),
            }
        }
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Lang; LANG_COUNT] = [
        Lang::Python,
        Lang::Rust,
        Lang::Go,
        Lang::C,
        Lang::Cpp,
        Lang::CSharp,
        Lang::Java,
        Lang::JavaScript,
        Lang::Jsx,
        Lang::TypeScript,
        Lang::Tsx,
        Lang::Ruby,
        Lang::Php,
        Lang::Bash,
        Lang::Html,
        Lang::Css,
        Lang::Json,
        Lang::Toml,
        Lang::Yaml,
        Lang::Markdown,
        Lang::Sql,
        Lang::Lua,
        Lang::Hcl,
        Lang::Dockerfile,
        Lang::Xml,
        Lang::Zig,
        Lang::Glsl,
        Lang::PowerShell,
    ];

    #[test]
    fn every_registered_language_has_a_working_grammar_and_query() {
        let broken: Vec<Lang> = ALL
            .into_iter()
            .filter(|lang| lang.grammar().is_none())
            .collect();
        assert_eq!(broken, []);
    }

    #[test]
    fn detects_infrastructure_shader_and_shell_files() {
        for (path, lang) in [
            ("infra/main.tf", Lang::Hcl),
            ("terraform.tfvars", Lang::Hcl),
            ("terragrunt.hcl", Lang::Hcl),
            ("Dockerfile", Lang::Dockerfile),
            ("build/Dockerfile.dev", Lang::Dockerfile),
            ("Containerfile", Lang::Dockerfile),
            ("Containerfile.dev", Lang::Dockerfile),
            ("build/app.dockerfile", Lang::Dockerfile),
            ("build/app.containerfile", Lang::Dockerfile),
            ("config.XML", Lang::Xml),
            ("schema.xsd", Lang::Xml),
            ("style.xsl", Lang::Xml),
            ("style.xslt", Lang::Xml),
            ("icon.svg", Lang::Xml),
            ("Info.plist", Lang::Xml),
            ("src/main.zig", Lang::Zig),
            ("build.zig.zon", Lang::Zig),
            ("shader.glsl", Lang::Glsl),
            ("shader.vert", Lang::Glsl),
            ("shader.frag", Lang::Glsl),
            ("shader.geom", Lang::Glsl),
            ("shader.tesc", Lang::Glsl),
            ("shader.tese", Lang::Glsl),
            ("shader.comp", Lang::Glsl),
            ("script.ps1", Lang::PowerShell),
            ("module.psm1", Lang::PowerShell),
            ("module.psd1", Lang::PowerShell),
            ("script.sh", Lang::Bash),
            ("script.bash", Lang::Bash),
            ("script.zsh", Lang::Bash),
            (".bash_profile", Lang::Bash),
            (".bash_login", Lang::Bash),
            (".bash_logout", Lang::Bash),
            (".zshenv", Lang::Bash),
            (".zprofile", Lang::Bash),
            (".zlogin", Lang::Bash),
            (".zlogout", Lang::Bash),
        ] {
            assert_eq!(detect(path, ""), Some(lang), "{path}");
        }
        for (shebang, lang) in [
            ("#!/bin/zsh", Lang::Bash),
            ("#!/usr/bin/env bash", Lang::Bash),
            ("#!/usr/bin/env -S zsh -e", Lang::Bash),
            ("#!/usr/bin/pwsh", Lang::PowerShell),
            ("#!/usr/bin/env pwsh", Lang::PowerShell),
            ("#!/usr/bin/env powershell", Lang::PowerShell),
        ] {
            assert_eq!(detect("bin/script", shebang), Some(lang), "{shebang}");
        }
        assert_eq!(detect("main.tf.json", ""), Some(Lang::Json));
        assert_eq!(detect("terraform.tfvars.json", ""), Some(Lang::Json));
        assert_eq!(detect("Dockerfile.txt", ""), Some(Lang::Dockerfile));
        assert_eq!(detect("dockerfiles.txt", ""), None);
        assert_eq!(detect("script.ps1", "#!/bin/bash"), Some(Lang::PowerShell));
    }

    #[test]
    fn detects_by_name_then_extension_then_shebang() {
        assert_eq!(detect("example.PY", ""), Some(Lang::Python));
        assert_eq!(
            detect("src/dir with spaces/example.js", ""),
            Some(Lang::JavaScript)
        );
        assert_eq!(detect("app/Gemfile", ""), Some(Lang::Ruby));
        assert_eq!(detect("Cargo.lock", ""), Some(Lang::Toml));
        assert_eq!(detect(".bashrc", ""), Some(Lang::Bash));
        assert_eq!(detect("component.tsx", ""), Some(Lang::Tsx));
        assert_eq!(
            detect("bin/tool", "#!/usr/bin/env python3"),
            Some(Lang::Python)
        );
        assert_eq!(detect("bin/run", "#!/bin/bash -e"), Some(Lang::Bash));
        assert_eq!(
            detect("bin/serve", "#!/usr/bin/env -S node --flag"),
            Some(Lang::JavaScript)
        );
        assert_eq!(detect("notes.unknown", "plain words"), None);
    }

    #[test]
    fn capture_names_fall_back_to_their_longest_known_prefix() {
        assert_eq!(capture_color("function.method"), capture_color("function"));
        assert_eq!(capture_color("keyword.operator"), capture_color("operator"));
        assert_ne!(capture_color("keyword.operator"), capture_color("keyword"));
        assert_eq!(
            capture_color("punctuation.bracket"),
            capture_color("unknown")
        );
    }

    #[test]
    fn innermost_capture_wins_over_the_enclosing_one() {
        // The escape sequence sits inside a string capture.
        let source = Source::new(Some(Lang::Python), &[r#"x = "a\nb""#]);

        let runs = source.highlight_line(0);
        let color_at = |offset: usize| {
            runs.iter()
                .find(|(r, _)| r.contains(&offset))
                .map(|(_, c)| *c)
        };

        assert!(color_at(5).is_some());
        assert!(color_at(6).is_some());
        assert_ne!(color_at(5), color_at(6));
        assert_eq!(color_at(5), color_at(8));
    }

    #[test]
    fn querying_one_line_only_produces_captures_for_that_line() {
        let lines: Vec<String> = (0..2000)
            .map(|i| format!("value_{i} = call({i})  # note"))
            .collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let source = Source::new(Some(Lang::Python), &lines);

        let bounds = source.line_range(1000);
        let captures = source.captures(1000);

        assert!(!captures.is_empty());
        assert!(
            captures
                .iter()
                .all(|(r, _, _)| r.start < bounds.end && r.end > bounds.start)
        );
    }

    #[test]
    fn unknown_languages_produce_no_colors() {
        let source = Source::new(None, &["import os"]);

        assert_eq!(source.highlight_line(0), []);
    }
}
