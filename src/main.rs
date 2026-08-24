use std::{env, process::ExitCode};

use music_tag_transfer::{
    cli::{Command, HELP, parse_args},
    delete_tags_recursively, download, export_frames_to_csv, refresh_copyrights,
    sources::{Chain, Source, menu},
    write_change_report,
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
        Command::Copyright {
            folder,
            sources,
            token,
            token_file,
            only_missing,
            dry_run,
            csv,
            overwrite,
            max_wait,
        } => {
            let interactive = menu::interactive();
            // Nobody has chosen yet: ask if there is anyone to ask, and
            // otherwise keep the default rather than stopping a scripted run.
            let sources = match sources {
                Some(sources) => sources,
                None if interactive => vec![menu::choose_source()?],
                None => vec![menu::DEFAULT],
            };

            // Only the first source can be given a credential on the command
            // line; the rest fall back to the environment, which is the only
            // way one flag could serve several sources unambiguously.
            let mut credentials = Vec::with_capacity(sources.len());
            for (index, source) in sources.iter().enumerate() {
                let (token, token_file) = if index == 0 {
                    (token.as_deref(), token_file.as_deref())
                } else {
                    (None, None)
                };
                credentials.push(menu::credential_for(
                    *source,
                    token,
                    token_file,
                    interactive,
                )?);
            }

            let mut chain = Chain::open(&sources, &credentials, max_wait)?;
            let names = sources
                .iter()
                .map(|source| source.title())
                .collect::<Vec<_>>()
                .join(", then ");
            println!("Looking copyrights up in {names}.");
            if max_wait != music_tag_transfer::sources::DEFAULT_MAX_WAIT {
                println!(
                    "Waiting out a rate limit of up to {max_wait}s before giving up on a source."
                );
            }

            let report = refresh_copyrights(&folder, &mut chain, only_missing, dry_run)
                .map_err(|error| error.to_string())?;

            let verb = if dry_run { "Would write" } else { "Wrote" };
            println!(
                "{verb} a copyright message to {} file(s) from {} album lookup(s). \
                 Scanned {} file(s): {} already had the same message, {} were left \
                 unchanged with no copyright to write, and {} were skipped as already \
                 carrying one.",
                report.files_updated,
                report.albums_looked_up,
                report.files_scanned,
                report.files_unchanged,
                report.files_without_copyright,
                report.files_skipped,
            );
            if report.albums_without_match > 0 || report.albums_failed > 0 {
                println!(
                    "{} album(s) matched nothing and {} lookup(s) failed; their files keep \
                     whatever they had.",
                    report.albums_without_match, report.albums_failed,
                );
            }
            if sources.len() > 1 {
                for (source, hits, answering) in chain.tally() {
                    let state = if answering {
                        ""
                    } else {
                        ", then stopped answering"
                    };
                    println!("  {}: {hits} album(s){state}", source.title());
                }
            }
            if report.stopped_early {
                println!(
                    "The scan stopped early, so some files were never visited. Everything \
                     written so far is written; re-run with --only-missing to carry on from \
                     here once the limit clears."
                );
                if sources.len() == 1 {
                    println!(
                        "  A fallback chain keeps a run going when one catalogue runs dry, \
                         for instance --source {},{}.",
                        sources[0].key(),
                        Source::ALL
                            .iter()
                            .find(|other| **other != sources[0])
                            .map_or("itunes", |other| other.key()),
                    );
                }
            }

            if let Some(csv) = &csv {
                write_change_report(&report, &folder, csv, overwrite)
                    .map_err(|error| error.to_string())?;
                println!(
                    "Wrote a before-and-after row for {} file(s) to {}.",
                    report.changes.len(),
                    csv.display()
                );
            }

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
