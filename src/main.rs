use std::{env, process::ExitCode};

use music_tag_transfer::{
    cli::{Command, HELP, parse_args},
    delete_tags_recursively, transfer_frame_recursively,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    match parse_args(env::args_os().skip(1))? {
        Command::Help => {
            print!("{HELP}");
            Ok(())
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

            if report.errors.is_empty() {
                Ok(())
            } else {
                for error in &report.errors {
                    eprintln!("{}: {}", error.path.display(), error.message);
                }
                Err(format!(
                    "{} file(s) could not be processed",
                    report.errors.len()
                ))
            }
        }
        Command::Transfer {
            source,
            destination,
            frame_id,
        } => {
            let report = transfer_frame_recursively(&source, &destination, &frame_id)
                .map_err(|error| error.to_string())?;
            println!(
                "Updated {} file(s); copied {} frame(s). Scanned {} destination music file(s).",
                report.files_changed, report.frames_copied, report.destination_files_scanned
            );

            if report.errors.is_empty() {
                Ok(())
            } else {
                for error in &report.errors {
                    eprintln!("{}: {}", error.path.display(), error.message);
                }
                Err(format!(
                    "{} file(s) could not be processed",
                    report.errors.len()
                ))
            }
        }
    }
}
