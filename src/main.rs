use std::{env, process::ExitCode};

use music_tag_transfer::{
    FileError,
    cli::{Command, HELP, parse_args},
    delete_tags_recursively, download, resolve, transfer_frame_recursively,
};

fn main() -> ExitCode {
    match run() {
        Ok(exit_code) => exit_code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let command =
        parse_args(env::args_os().skip(1)).map_err(|message| format!("{message}\n\n{HELP}"))?;

    match command {
        Command::Help => {
            print!("{HELP}");
            Ok(ExitCode::SUCCESS)
        }
        Command::DownloadHelp => {
            println!("{}", download::cli::help_text());
            Ok(ExitCode::SUCCESS)
        }
        Command::ResolveHelp => {
            print!("{}", resolve::HELP);
            Ok(ExitCode::SUCCESS)
        }
        Command::Version => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        Command::Download(config) => {
            let code = download::run(config)?;
            Ok(if code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Resolve(config) => {
            let report = resolve::run(config)?;
            println!(
                "Resolved {} of {} line(s); {} not found and {} error(s). Wrote {}.",
                report.resolved,
                report.total,
                report.not_found,
                report.errors,
                report.output.display()
            );
            Ok(if report.errors == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Delete {
            folder,
            tags,
            dry_run,
        } => {
            let report = delete_tags_recursively(&folder, &tags, dry_run)
                .map_err(|error| error.to_string())?;

            let verb = if dry_run { "Would update" } else { "Updated" };
            println!(
                "{verb} {} file(s); removed {} frame(s). Scanned {} music file(s), \
                 skipped {} unchanged and {} without an ID3 tag.",
                report.files_changed,
                report.frames_removed,
                report.files_scanned,
                report.files_unchanged,
                report.files_without_tag,
            );

            finish_with_file_errors(&report.errors)
        }
        Command::Transfer {
            source,
            destination,
            frame_id,
        } => {
            let report = transfer_frame_recursively(&source, &destination, &frame_id)
                .map_err(|error| error.to_string())?;
            println!(
                "Updated {} file(s); copied {} frame(s). Scanned {} source and {} destination music file(s).",
                report.files_changed,
                report.frames_copied,
                report.source_files_scanned,
                report.destination_files_scanned
            );

            finish_with_file_errors(&report.errors)
        }
    }
}

fn finish_with_file_errors(errors: &[FileError]) -> Result<ExitCode, String> {
    if errors.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }

    for error in errors {
        eprintln!("{}: {}", error.path.display(), error.message);
    }
    Err(format!("{} file(s) could not be processed", errors.len()))
}
