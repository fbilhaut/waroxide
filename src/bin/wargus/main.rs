//! `wargus`: the game launcher.
//!
//! Checks that the Warcraft II data has been extracted (running `wartool`
//! if needed, after copying the Wargus scripts next to the data), then
//! starts Stratagus on it. Port of Stratagus' `stratagus-game-launcher.h`
//! configured for Wargus.

mod dialogs;

use std::env;
use std::fs;
use std::io::{ErrorKind, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

use dialogs::{Dialogs, Level};
use orcrust::extract::REEXTRACT_MARKER_FILE;

const GAME_NAME: &str = "Wargus";
const GAME_CD: &str = "Warcraft II DOS CD (REZDAT.WAR) or CD image (.cue/.bin/.iso)";
const GAME: &str = "wargus";
const EXTRACTOR_TOOL: &str = "wartool";
const EXTRACTOR_ARGS: &[&str] = &["-v", "-r"];

/// Directories of the Wargus distribution copied into the data directory
/// before extraction: (source, destination).
const CONTRIB_DIRECTORIES: &[(&str, &str)] = &[
    ("campaigns", "campaigns"),
    ("contrib", "graphics/ui"),
    ("maps", "maps"),
    ("shaders", "shaders"),
    ("scripts", "scripts"),
];

/// Installation paths, set at build time like the CMake build does.
const DATA_PATH: &str = match option_env!("WARGUS_DATA_PATH") {
    Some(p) => p,
    None => "/usr/local/share/games/stratagus/wargus",
};
const SCRIPTS_PATH: &str = match option_env!("WARGUS_SCRIPTS_PATH") {
    Some(p) => p,
    None => DATA_PATH,
};
const STRATAGUS_BIN: &str = match option_env!("STRATAGUS_BIN") {
    Some(p) => p,
    None => "stratagus",
};

const EXE_SUFFIX: &str = env::consts::EXE_SUFFIX;

fn data_not_extracted() -> String {
    format!("{GAME_NAME} data was not extracted, is corrupted, or outdated.\nYou need to extract it from original {GAME_CD}.")
}

fn stratagus_not_found() -> String {
    format!("Stratagus is not installed.\nYou need Stratagus to run {GAME_NAME}!\n")
}

/// `cmd` itself if it is a path to an existing file, else its location on
/// `PATH`.
fn find_in_path(cmd: &str) -> Option<PathBuf> {
    let p = Path::new(cmd);
    if p.components().count() > 1 {
        return p.exists().then(|| p.to_path_buf());
    }
    env::split_paths(&env::var_os("PATH")?)
        .find_map(|dir| [dir.join(cmd), dir.join(format!("{cmd}{EXE_SUFFIX}"))].into_iter().find(|c| c.is_file()))
}

/// Where Stratagus keeps per-user game data.
fn user_data_path(data_path: &Path) -> PathBuf {
    let base = format!("data.{GAME_NAME}");
    if cfg!(windows) {
        if data_path.join("portable-install").exists() {
            return data_path.to_path_buf();
        }
        let docs = env::var_os("USERPROFILE")
            .map(|h| PathBuf::from(h).join("Documents"))
            .or_else(|| env::var_os("APPDATA").map(PathBuf::from))
            .unwrap_or_default();
        return docs.join("Stratagus").join(base);
    }
    if let Some(appimage) = env::var_os("APPIMAGE").map(PathBuf::from) {
        let mut data = appimage.into_os_string();
        data.push(".data");
        if Path::new(&data).exists() {
            // Same (slash-less) layout as the original launcher.
            data.push(format!("/stratagus{base}"));
            return data.into();
        }
    }
    let root = match (env::var_os("XDG_DATA_HOME"), env::var_os("HOME")) {
        (Some(xdg), _) => PathBuf::from(xdg).join("stratagus"),
        (None, Some(home)) if cfg!(target_os = "macos") => {
            PathBuf::from(home).join("Library/Application Support/Stratagus")
        }
        (None, Some(home)) => PathBuf::from(home).join(".local/share/stratagus"),
        (None, None) => PathBuf::new(),
    };
    root.join(base)
}

/// Replace `target` with a recursive copy of `source`.
fn copy_dir(source: &Path, target: &Path) -> std::io::Result<()> {
    if target.exists() {
        if fs::canonicalize(source)? == fs::canonicalize(target)? {
            return Ok(());
        }
        fs::remove_dir_all(target)?;
    } else if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    copy_recursive(source, target)
}

fn copy_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &target.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(source, target).map(|_| ())
    }
}

fn strip_whitespace(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Was the data extracted by the same version of the extractor?
fn check_version(extractor: &Path, data_path: &Path) -> bool {
    let Ok(data_version) = fs::read_to_string(data_path.join("extracted")) else {
        return false;
    };
    let Ok(out) = Command::new(extractor).arg("-V").output() else {
        return false;
    };
    let tool_version = String::from_utf8_lossy(&out.stdout);
    let first = |s: &str| strip_whitespace(&s.chars().take(19).collect::<String>());
    first(&data_version) == first(&tool_version)
}

/// How hard [`Launcher::extract_data`] should try.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Force {
    /// Data missing or outdated: migrate in place if possible.
    No,
    /// Explicit `--extract`.
    Yes,
    /// Extract from a given file, without copying the scripts again.
    Silent,
}

struct Launcher {
    dialogs: Dialogs,
    argv0: PathBuf,
    extractor: PathBuf,
}

impl Launcher {
    fn error(&self, text: &str) -> ! {
        self.dialogs.message(GAME_NAME, text, Level::Error);
        exit(-1);
    }

    /// Locate the Wargus scripts to copy next to the extracted data.
    fn scripts_source(&self, scripts_path: &Path) -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if let Some(p) = env::var_os("WARGUS_SCRIPTS_PATH") {
            candidates.push(PathBuf::from(p));
        }
        if scripts_path.is_absolute() {
            candidates.push(scripts_path.to_path_buf());
        } else {
            // Relative to the launcher, resolving `..` the same way.
            let mut p = self.argv0.clone();
            for c in scripts_path.components() {
                match c {
                    std::path::Component::ParentDir => {
                        p.pop();
                    }
                    c => p.push(c),
                }
            }
            candidates.push(p);
        }
        if let Some(dir) = self.extractor.parent() {
            candidates.push(dir.to_path_buf());
        }
        candidates.into_iter().find(|p| p.exists())
    }

    fn extract_data(&self, destination: &Path, scripts_path: &Path, force: Force, datafile: Option<PathBuf>) {
        let can_just_reextract = force == Force::No && destination.join(REEXTRACT_MARKER_FILE).exists();
        match force {
            _ if can_just_reextract => self.dialogs.message(
                "",
                &format!("{GAME} game data format changed, we can migrate in-place. Please be patient."),
                Level::Info,
            ),
            Force::No => self.dialogs.message(
                "Missing data",
                &format!("{} Please select the {GAME_CD}", data_not_extracted()),
                Level::Question,
            ),
            Force::Yes => self.dialogs.message("", &format!("Please select the {GAME_CD}"), Level::Question),
            Force::Silent => {}
        }

        let srcfolder = if !can_just_reextract || datafile.is_some() {
            let Some(datafile) = datafile.or_else(|| self.dialogs.pick_file(&format!("{GAME_CD} location"))) else {
                exit(-1);
            };
            self.source_folder(&datafile)
        } else {
            destination.to_path_buf()
        };

        let Some(sourcepath) = self.scripts_source(scripts_path) else {
            self.dialogs.message(
                "Error",
                &format!(
                    "There was an error copying the data, could not discover scripts path: {}",
                    scripts_path.display()
                ),
                Level::Error,
            );
            return;
        };

        let _ = fs::create_dir_all(destination);
        if force != Force::Silent {
            for (src, dst) in CONTRIB_DIRECTORIES {
                let src = sourcepath.join(src);
                if !src.exists() {
                    self.error(&format!(
                        "There was an error copying the data, could not discover contributed directory path: {}",
                        src.display()
                    ));
                }
                if let Err(e) = copy_dir(&src, &destination.join(dst)) {
                    self.error(&format!("There was an error copying {}: {e}", src.display()));
                }
            }
        }

        if !std::io::stdout().is_terminal() {
            self.dialogs.message(
                "",
                "Extracting data, this may take a few minutes.\nPlease be patient. If something fails, re-run from a terminal.",
                Level::Info,
            );
        }
        let mut cmd = Command::new(&self.extractor);
        cmd.args(EXTRACTOR_ARGS).arg(&srcfolder).arg(destination);
        let cmdline = format!(
            "{} {} {} {}",
            self.extractor.display(),
            EXTRACTOR_ARGS.join(" "),
            srcfolder.display(),
            destination.display()
        );
        println!("Running extractor as {cmdline}");
        let ok = cmd.status().map(|s| s.success()).unwrap_or(false);

        if !ok {
            self.dialogs.message(
                "Extraction failed!",
                &format!(
                    "The following command was used to extract the data (you can run it manually in a console to find out more):\n{cmdline}"
                ),
                Level::Error,
            );
        } else if !can_just_reextract
            && self.dialogs.ask_yes_no("Extract more?", "Extract from the additional DOS expansion CD?")
        {
            self.extract_data(destination, scripts_path, Force::Silent, None);
        }
    }

    /// What to extract from, given the file the user picked: CD images are
    /// read directly, other files stand for their directory. Single-file
    /// installers are unpacked with innoextract first.
    fn source_folder(&self, datafile: &Path) -> PathBuf {
        let parent = || datafile.parent().map(Path::to_path_buf).unwrap_or_default();
        let ext = datafile.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if matches!(ext.as_str(), "cue" | "bin" | "iso") {
            return datafile.to_path_buf();
        }
        let is_exe = ext == "exe";
        if !is_exe {
            return parent();
        }
        let innoextract = env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join(format!("innoextract{EXE_SUFFIX}"))))
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("innoextract"));
        let is_inno = Command::new(&innoextract).arg("-i").arg(datafile).status().is_ok_and(|s| s.success());
        if is_inno {
            let tmp = env::temp_dir().join(GAME);
            let _ = fs::create_dir_all(&tmp);
            let ok = Command::new(&innoextract)
                .args(["-m", "-d"])
                .arg(&tmp)
                .arg(datafile)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                self.error(
                    "You selected an innosetup installer, and we could not extract it. \
                     Please extract it manually and point the extraction tool there.",
                );
            }
            return tmp;
        }
        let name = datafile.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if name != "install.exe" && name != "setup.exe" {
            self.dialogs.message(
                "",
                &format!(
                    "You selected an exe file, but I cannot run innoextract to check if its a single-file \
                     installer. If it is, please extract/install manually first and then run {GAME} again."
                ),
                Level::Question,
            );
        }
        parent()
    }
}

fn main() {
    let mut args: Vec<String> = env::args().collect();
    let mut dialogs = Dialogs::new();

    // Undocumented: `--argv0=<path>` gives our own location (AppImages).
    let mut argv0 = PathBuf::from(args.first().cloned().unwrap_or_else(|| GAME.into()));
    if let Some(p) = args.get(1).and_then(|a| a.strip_prefix("--argv0=")) {
        argv0 = fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
        args.remove(1);
    }
    let exe_dir = env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .or_else(|| argv0.parent().map(Path::to_path_buf))
        .unwrap_or_default();

    // Prefer the extractor installed next to us, then the one on PATH.
    let local_extractor = exe_dir.join(format!("{EXTRACTOR_TOOL}{EXE_SUFFIX}"));
    let extractor = if local_extractor.exists() {
        local_extractor
    } else {
        match find_in_path(EXTRACTOR_TOOL) {
            Some(p) => p,
            None => {
                dialogs.message(
                    GAME_NAME,
                    &format!(
                        "{GAME_NAME} could not find its extraction tool.\n{EXTRACTOR_TOOL}!\n (expected at {})",
                        local_extractor.display()
                    ),
                    Level::Error,
                );
                exit(-1);
            }
        }
    };

    let (mut data_path, scripts_path, mut stratagus_bin) = if cfg!(windows) {
        let local = exe_dir.join("stratagus.exe");
        let bin = if local.exists() {
            local
        } else {
            find_in_path("stratagus").or_else(|| find_in_path("stratagus-dbg")).unwrap_or(local)
        };
        let data = option_env!("WARGUS_DATA_PATH").map(PathBuf::from).unwrap_or_else(|| exe_dir.clone());
        (data, exe_dir.clone(), bin)
    } else {
        (PathBuf::from(DATA_PATH), PathBuf::from(SCRIPTS_PATH), PathBuf::from(STRATAGUS_BIN))
    };

    if args.len() == 2 {
        let launcher = |dialogs| Launcher { dialogs, argv0: argv0.clone(), extractor: extractor.clone() };
        match args[1].as_str() {
            path if Path::new(path).exists() => {
                // Extraction file given: extract from it and exit.
                dialogs.console = true;
                let dest = user_data_path(&data_path);
                launcher(dialogs).extract_data(&dest, &scripts_path, Force::Silent, Some(PathBuf::from(path)));
                return;
            }
            "--extract" => {
                let dest = user_data_path(&data_path);
                launcher(dialogs).extract_data(&dest, &scripts_path, Force::Yes, None);
                return;
            }
            "--extract-no-gui" => {
                dialogs.console = true;
                let dest = user_data_path(&data_path);
                launcher(dialogs).extract_data(&dest, &scripts_path, Force::Yes, None);
                return;
            }
            "--help" | "-h" => println!(
                "Usage: {} [path to extraction file|--extract|--extract-no-gui]\n\
                 \tpath to extraction file - will be used as file to start the extraction process on\n\
                 \t--extract - force extraction even if data is already extracted\n\
                 \t--extract-no-gui - force extraction even if data is already extracted, using the console only for prompts\n",
                args[0]
            ),
            _ => {}
        }
    }
    let launcher = Launcher { dialogs, argv0: argv0.clone(), extractor };

    if !stratagus_bin.exists() {
        match find_in_path(&stratagus_bin.to_string_lossy()) {
            Some(p) => stratagus_bin = p,
            None => {
                stratagus_bin = exe_dir.join(format!("stratagus{EXE_SUFFIX}"));
                if !stratagus_bin.exists() {
                    launcher.error(&format!("{} (expected in {})", stratagus_not_found(), stratagus_bin.display()));
                }
            }
        }
    }

    let title = |data: &Path| data.join("graphics").join("ui").join("title.png");
    if !title(&data_path).exists() {
        data_path = user_data_path(&data_path);
        if !title(&data_path).exists() {
            launcher.extract_data(&data_path, &scripts_path, Force::No, None);
        }
        if !title(&data_path).exists() {
            launcher
                .error(&format!("{} (extraction was attempted, but it seems an error occurred)", data_not_extracted()));
        }
    }
    if !check_version(&launcher.extractor, &data_path) {
        launcher.extract_data(&data_path, &scripts_path, Force::No, None);
    }

    let mut cmd = Command::new(&stratagus_bin);
    cmd.arg("-d").arg(&data_path).args(&args[1..]).env("OMP_WAIT_POLICY", "passive");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.arg0(&argv0);
    }
    if cfg!(windows) {
        let _ = env::set_current_dir(&data_path);
    }
    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => launcher.error(&format!(
            "Execution failed for: {} -d {} {}",
            stratagus_bin.display(),
            data_path.display(),
            args[1..].join(" ")
        )),
        Err(e) => launcher.error(&format!("Execution failed for {}: {e}", stratagus_bin.display())),
    };
    if !status.success() {
        launcher.error(&format!(
            "Stratagus failed to load game data.\n\
             If you just launched the game without any arguments, this may indicate a bug with the extraction process.\n\
             Please report this on https://github.com/Wargus/stratagus/issues/new,\n\
             and please give details, including: operating system, installation path, username, kind of source CD.\n\
             If you got an error message about the extraction command failing, please try to run it in a console\n\
             and post the output to the issue. A common problem is symbols in the path for the installation, the game data path,\n\
             or the username (like an ampersand or exclamation mark). Try changing these.\n\
             Try also to remove the folder {} and try the extraction again.\n",
            data_path.display()
        ));
    }
}
