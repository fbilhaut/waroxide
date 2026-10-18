//! `pudconvert`: convert a PUD map to Stratagus `.smp.gz` / `.sms.gz`.

use std::path::Path;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("pudconvert V1.0\nusage: pudconvert inputfile [ outputdir ]\n");
    ExitCode::from(255)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 2 || matches!(args[0].as_str(), "-h" | "--help" | "/?") {
        return usage();
    }

    let input = Path::new(&args[0]);
    if !input.exists() {
        eprintln!("error finding file: {}", input.display());
        return ExitCode::from(255);
    }
    let outdir = match args.get(1) {
        Some(dir) => dir.clone(),
        None => match input.parent().map(|p| p.to_string_lossy().into_owned()) {
            Some(p) if !p.is_empty() => p,
            _ => ".".into(),
        },
    };
    if !Path::new(&outdir).exists() {
        eprintln!("error finding directory: {outdir}");
        return ExitCode::from(255);
    }

    let file_name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match file_name.find(".pud") {
        Some(p) => &file_name[..p],
        None => &file_name,
    };

    match orcrust::pud::convert_file(input, name, &outdir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::from(255)
        }
    }
}
