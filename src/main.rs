use std::io;
use std::process::ExitCode;
use std::sync::Arc;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use gitplume::app::App;
use gitplume::git::{CliGit, SystemRunner};
use gitplume::runtime;

fn main() -> io::Result<ExitCode> {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("gitplume {}", env!("CARGO_PKG_VERSION"));
        return Ok(ExitCode::SUCCESS);
    }
    let git = CliGit::new(SystemRunner);
    let cwd = std::env::current_dir()?;
    let root = match git.repo_root(&cwd) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("{error}");
            return Ok(ExitCode::FAILURE);
        }
    };
    let show_shortcuts = runtime::default_marker().is_none_or(|m| runtime::claim_first_launch(&m));
    let app = App::new(root, cwd, show_shortcuts);

    let mut terminal = ratatui::init();
    let restore = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        restore(info);
    }));
    let result = execute!(io::stdout(), EnableMouseCapture)
        .and_then(|()| runtime::run(&mut terminal, Arc::new(git), app));
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result.map(|()| ExitCode::SUCCESS)
}
