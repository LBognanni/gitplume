//! Full-terminal handoff to a user-configured external editor.

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

fn command(editor: &str, path: &Path, cwd: &Path) -> Result<Command, String> {
    let words =
        shell_words::split(editor).map_err(|error| format!("Invalid GITPLUME_EDITOR: {error}"))?;
    let (program, args) = words
        .split_first()
        .filter(|(program, _)| !program.is_empty())
        .ok_or("GITPLUME_EDITOR must name an editor")?;
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

/// Terminal errors are fatal; editor failures are returned for an in-app toast.
pub fn open(
    terminal: &mut DefaultTerminal,
    path: &Path,
    cwd: &Path,
) -> io::Result<Result<(), String>> {
    let editor = match std::env::var("GITPLUME_EDITOR") {
        Ok(editor) => editor,
        Err(std::env::VarError::NotPresent) => "nano".into(),
        Err(error) => return Ok(Err(format!("Invalid GITPLUME_EDITOR: {error}"))),
    };
    let mut command = match command(&editor, path, cwd) {
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
            let error = command(editor, Path::new("/repo/file"), Path::new("/repo")).unwrap_err();
            assert!(error.contains("GITPLUME_EDITOR"), "{error}");
        }
    }
}
