//! Full-terminal handoff to a user-configured external editor.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::DefaultTerminal;

// A cooked terminal sends Ctrl+C to both processes in the foreground group.
// Let the editor handle it without terminating gitplume before it restores the UI.
#[cfg(unix)]
fn interrupt_flag() -> io::Result<&'static std::sync::atomic::AtomicBool> {
    use std::sync::{Arc, OnceLock, atomic::AtomicBool};
    static FLAG: OnceLock<io::Result<Arc<AtomicBool>>> = OnceLock::new();
    FLAG.get_or_init(|| {
        let flag = Arc::new(AtomicBool::new(true));
        signal_hook::flag::register_conditional_default(signal_hook::consts::SIGINT, flag.clone())?;
        Ok(flag)
    })
    .as_ref()
    .map(|flag| flag.as_ref())
    .map_err(|error| io::Error::other(error.to_string()))
}

/// The editor and the setting it came from: a non-empty `GITPLUME_EDITOR`
/// (`own`), else Git's resolved editor (`git var GIT_EDITOR`), else `nano`.
fn choose(own: Option<OsString>, git: Option<&str>) -> Result<(&'static str, String), String> {
    if let Some(own) = own.filter(|value| !value.is_empty()) {
        return own
            .into_string()
            .map(|value| ("GITPLUME_EDITOR", value))
            .map_err(|_| "Invalid GITPLUME_EDITOR: not valid unicode".into());
    }
    match git.filter(|value| !value.is_empty()) {
        Some(git) => Ok(("GIT_EDITOR", git.into())),
        None => Ok(("GITPLUME_EDITOR", "nano".into())),
    }
}

/// Build the editor command; `source` names the variable it came from in errors.
fn command(source: &str, editor: &str, path: &Path, cwd: &Path) -> Result<Command, String> {
    let words = shell_words::split(editor).map_err(|error| format!("Invalid {source}: {error}"))?;
    let (program, args) = words
        .split_first()
        .filter(|(program, _)| !program.is_empty())
        .ok_or(format!("{source} must name an editor"))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .arg(path)
        .current_dir(cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    Ok(command)
}

/// Open `path` in the editor; `git_editor` is Git's editor, once known.
/// Terminal errors are fatal; editor failures are returned for an in-app toast.
pub fn open(
    terminal: &mut DefaultTerminal,
    path: &Path,
    cwd: &Path,
    git_editor: Option<&str>,
) -> io::Result<Result<(), String>> {
    let mut command = match choose(std::env::var_os("GITPLUME_EDITOR"), git_editor)
        .and_then(|(source, editor)| command(source, &editor, path, cwd))
    {
        Ok(command) => command,
        Err(error) => return Ok(Err(error)),
    };
    if !path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_file())
    {
        return Ok(Err("The open file is no longer a regular file.".into()));
    }

    #[cfg(unix)]
    let interrupt = interrupt_flag()?;

    execute!(
        io::stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    )?;
    disable_raw_mode()?;
    #[cfg(unix)]
    interrupt.store(false, std::sync::atomic::Ordering::SeqCst);
    let result = command
        .status()
        .map_err(|error| error.to_string())
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!("Editor exited with {status}"))
            }
        });

    // Restore the UI even when spawning the editor failed or it exited unsuccessfully.
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture, Hide)?;
    terminal.clear()?;
    #[cfg(unix)]
    interrupt.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_arguments_are_parsed_without_a_shell_and_path_is_one_argument() {
        let command = command(
            "GITPLUME_EDITOR",
            "'/opt/My Editor/bin/editor' --wait --label 'two words'",
            Path::new("/repo/-a file;$(touch unwanted).txt"),
            Path::new("/repo"),
        )
        .unwrap();
        assert_eq!(command.get_program(), "/opt/My Editor/bin/editor");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--wait",
                "--label",
                "two words",
                "/repo/-a file;$(touch unwanted).txt",
            ]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/repo")));
    }

    #[test]
    fn invalid_editor_commands_have_actionable_errors() {
        for editor in ["", "   ", "''", "nano '"] {
            let error = command(
                "GIT_EDITOR",
                editor,
                Path::new("/repo/file"),
                Path::new("/repo"),
            )
            .unwrap_err();
            assert!(error.contains("GIT_EDITOR"), "{error}");
        }
    }

    #[test]
    fn gitplume_editor_wins_then_git_editor_then_nano() {
        let own = || Some(OsString::from("hx"));
        assert_eq!(
            choose(own(), Some("subl --wait")),
            Ok(("GITPLUME_EDITOR", "hx".into()))
        );
        assert_eq!(
            choose(None, Some("subl --wait")),
            Ok(("GIT_EDITOR", "subl --wait".into()))
        );
        assert_eq!(choose(None, None), Ok(("GITPLUME_EDITOR", "nano".into())));
    }

    #[test]
    fn empty_editor_settings_count_as_unset() {
        assert_eq!(
            choose(Some(OsString::new()), Some("vim")),
            Ok(("GIT_EDITOR", "vim".into()))
        );
        assert_eq!(
            choose(None, Some("")),
            Ok(("GITPLUME_EDITOR", "nano".into()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_non_unicode_gitplume_editor_is_reported_by_name() {
        use std::os::unix::ffi::OsStringExt;
        let error = choose(Some(OsString::from_vec(vec![0xff])), Some("vim")).unwrap_err();
        assert!(error.contains("GITPLUME_EDITOR"), "{error}");
    }
}
