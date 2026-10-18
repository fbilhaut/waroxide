//! Message boxes and file pickers, native when the `gui` feature is on and
//! a console prompt otherwise (or when forced, like `tinyfd_forceConsole`).

use std::io::{BufRead, Write};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Question,
    Error,
}

pub struct Dialogs {
    pub console: bool,
}

fn read_line() -> Option<String> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok()?;
    Some(line.trim().to_string())
}

impl Dialogs {
    pub fn new() -> Self {
        Self { console: !cfg!(feature = "gui") }
    }

    pub fn message(&self, title: &str, text: &str, level: Level) {
        #[cfg(feature = "gui")]
        if !self.console {
            let level = match level {
                Level::Error => rfd::MessageLevel::Error,
                Level::Info | Level::Question => rfd::MessageLevel::Info,
            };
            rfd::MessageDialog::new().set_title(title).set_description(text).set_level(level).show();
            return;
        }
        let out = &mut std::io::stderr();
        let _ = match level {
            Level::Error => writeln!(out, "\n{title}: {text}"),
            _ if title.is_empty() => writeln!(out, "\n{text}"),
            _ => writeln!(out, "\n{title}\n{text}"),
        };
    }

    pub fn ask_yes_no(&self, title: &str, text: &str) -> bool {
        #[cfg(feature = "gui")]
        if !self.console {
            let answer = rfd::MessageDialog::new()
                .set_title(title)
                .set_description(text)
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            return answer == rfd::MessageDialogResult::Yes;
        }
        eprint!("\n{title}\n{text} [y/N] ");
        let _ = std::io::stderr().flush();
        read_line().is_some_and(|a| a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes"))
    }

    pub fn pick_file(&self, title: &str) -> Option<PathBuf> {
        #[cfg(feature = "gui")]
        if !self.console {
            return rfd::FileDialog::new().set_title(title).pick_file();
        }
        eprint!("\n{title}\nPath to the file: ");
        let _ = std::io::stderr().flush();
        read_line().filter(|p| !p.is_empty()).map(PathBuf::from)
    }
}
