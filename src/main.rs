use std::{env, process::ExitCode};

use music_tag_transfer::{
    cli::{Command, HELP, parse_args},
    delete_tags_recursively, download, export_frames_to_csv,
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
                Ok(ExitCode::SUCCESS)
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
        Command::Export {
            folder,
            output,
            overwrite,
        } => {
            let report = export_frames_to_csv(&folder, &output, overwrite)
                .map_err(|error| error.to_string())?;

            println!(
                "Wrote {} row(s) and {} frame column(s) to {}. Exported {} frame(s) from {} \
                 tagged file(s); {} file(s) had no ID3 tag.",
                report.files_with_tag + report.files_without_tag,
                report.frame_columns,
                output.display(),
                report.frames_exported,
                report.files_with_tag,
                report.files_without_tag,
            );

            if report.errors.is_empty() {
                Ok(ExitCode::SUCCESS)
            } else {
                for error in &report.errors {
                    eprintln!("{}: {}", error.path.display(), error.message);
                }
                Err(format!("{} file(s) could not be read", report.errors.len()))
            }
        }
    }
}
