//! `wartool`: extract the Warcraft II data for Wargus.

use std::process::ExitCode;

use waroxide::extract::{self, Options, NAME_LINE, VERSION};

fn usage(name: &str) {
    println!(
        "{NAME_LINE}
Usage: {name} [-e|-n] [-v] [-r] [-V] [-h|--help] archive-directory [destination-directory]
\t-e\tThe archive is expansion compatible (default: autodetect)
\t-n\tThe archive is not expansion compatible (default: autodetect)
\t-v\tExtract and convert videos
\t-r\tRip sound tracks from CD-ROM (needs original CD, no image/emulation)
\t-V\tShow version
\t-h\tShow usage (this text)
archive-directory\tDirectory which includes the archives maindat.war or the battle.net tomes...
destination-directory\tDirectory where the extracted files are placed."
    );
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let name = argv.first().map(String::as_str).unwrap_or("wartool");
    let args = argv.get(1..).unwrap_or(&[]).to_vec();

    let mut opts = Options {
        expansion: None,
        video: false,
        rip: false,
        archive_dir: String::new(),
        dest_dir: "data".into(),
        args: args.clone(),
        archive_arg: 0,
    };

    // Options come first; the first other argument starts the paths.
    let mut a = 0;
    while let Some(arg) = args.get(a) {
        match arg.as_str() {
            "-v" => opts.video = true,
            "-r" => opts.rip = true,
            "-e" => opts.expansion = Some(true),
            "-n" => opts.expansion = Some(false),
            "-V" => {
                println!("{VERSION}");
                return ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                usage(name);
                return ExitCode::SUCCESS;
            }
            _ => break,
        }
        a += 1;
    }
    let paths = &args[a..];
    if paths.is_empty() || paths.len() > 2 {
        usage(name);
        return ExitCode::from(1);
    }
    opts.archive_dir = paths[0].clone();
    opts.archive_arg = a;
    if let Some(dest) = paths.get(1) {
        opts.dest_dir = dest.clone();
    }

    match extract::run(&opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::from(255)
        }
    }
}
