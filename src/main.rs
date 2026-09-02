use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

use music_tag_transfer::{
    FileError, TagSpec,
    cli::{Command, HELP, parse_args},
    delete_tags_recursively, download, export_frames_to_csv, refresh_copyrights,
    sources::{Chain, Limits, Source, menu},
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
        Command::Download(config) => Ok(exit_code_of(download::run(config)?)),
        Command::Resolve(config) => Ok(exit_code_of(download::resolve::run(config)?)),
        Command::Delete {
            folder,
            tags,
            dry_run,
        } => delete(&folder, &tags, dry_run),
        Command::Copyright {
            folder,
            sources,
            token,
            token_file,
            only_missing,
            dry_run,
            csv,
            overwrite,
            limits,
        } => copyright(CopyrightRun {
            folder,
            sources,
            token,
            token_file,
            only_missing,
            dry_run,
            csv,
            overwrite,
            limits,
        }),
        Command::Export {
            folder,
            output,
            overwrite,
        } => export(&folder, &output, overwrite),
    }
}

/// A subcommand that counted its own failures reports them the same way: every
/// path that could not be handled, and then a failing status because of them.
fn finish(errors: &[FileError], past_participle: &str) -> Result<ExitCode, String> {
    if errors.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    for error in errors {
        eprintln!("{}: {}", error.path.display(), error.message);
    }
    Err(format!(
        "{} file(s) could not be {past_participle}",
        errors.len()
    ))
}

/// spotDL and Odesli report in the shell's own currency, so their count only
/// has to be narrowed to success or failure.
fn exit_code_of(code: i32) -> ExitCode {
    if code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn delete(folder: &Path, tags: &[TagSpec], dry_run: bool) -> Result<ExitCode, String> {
    let report =
        delete_tags_recursively(folder, tags, dry_run).map_err(|error| error.to_string())?;

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

    finish(&report.errors, "processed")
}

fn export(folder: &Path, output: &Path, overwrite: bool) -> Result<ExitCode, String> {
    let report =
        export_frames_to_csv(folder, output, overwrite).map_err(|error| error.to_string())?;

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

    finish(&report.errors, "read")
}

/// Everything `copyright` was asked for, kept together so the run reads as one
/// argument rather than nine positional ones.
struct CopyrightRun {
    folder: PathBuf,
    sources: Option<Vec<Source>>,
    token: Option<String>,
    token_file: Option<PathBuf>,
    only_missing: bool,
    dry_run: bool,
    csv: Option<PathBuf>,
    overwrite: bool,
    limits: Limits,
}

fn copyright(run: CopyrightRun) -> Result<ExitCode, String> {
    let CopyrightRun {
        folder,
        sources,
        token,
        token_file,
        only_missing,
        dry_run,
        csv,
        overwrite,
        limits,
    } = run;

    let interactive = menu::interactive();
    // Nobody has chosen yet: ask if there is anyone to ask, and
    // otherwise keep the default rather than stopping a scripted run.
    let sources = match sources {
        Some(sources) => sources,
        None if interactive => vec![menu::choose_source()?],
        None => vec![menu::DEFAULT],
    };

    let credentials = credentials_for(
        &sources,
        token.as_deref(),
        token_file.as_deref(),
        interactive,
    )?;

    let mut chain = Chain::open(&sources, &credentials, limits)?;
    announce(&sources, limits);

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

    report_sources(&sources, &chain);

    if let Some(csv) = &csv {
        write_change_report(&report, &folder, csv, overwrite).map_err(|error| error.to_string())?;
        println!(
            "Wrote a before-and-after row for {} file(s) to {}.",
            report.changes.len(),
            csv.display()
        );
    }

    finish(&report.errors, "processed")
}

/// Only the first source can be given a credential on the command line; the
/// rest fall back to the environment, which is the only way one flag could
/// serve several sources unambiguously.
fn credentials_for(
    sources: &[Source],
    token: Option<&str>,
    token_file: Option<&Path>,
    interactive: bool,
) -> Result<Vec<Option<String>>, String> {
    let mut credentials = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        let (token, token_file) = if index == 0 {
            (token, token_file)
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
    Ok(credentials)
}

/// Say which catalogues are about to be asked, and how hard, before a scan
/// that may take a while starts.
fn announce(sources: &[Source], limits: Limits) {
    let names = sources
        .iter()
        .map(|source| source.title())
        .collect::<Vec<_>>()
        .join(", then ");
    println!("Looking copyrights up in {names}.");
    if limits != Limits::default() {
        println!(
            "Trying each request up to {} time(s), and waiting out a rate limit of up \
             to {}s before setting a source aside.",
            limits.max_attempts, limits.max_wait
        );
    }
}

/// What each catalogue answered, and what to do about the ones that stopped.
fn report_sources(sources: &[Source], chain: &Chain) {
    let set_aside: Vec<&'static str> = chain
        .tally()
        .iter()
        .filter(|(_, _, answering)| !answering)
        .map(|(source, _, _)| source.title())
        .collect();
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
    if set_aside.is_empty() {
        return;
    }
    println!(
        "Set aside for the rest of the run: {}. Every file was still visited; \
         re-run with --only-missing to fill in what they could not answer.",
        set_aside.join(", ")
    );
    if sources.len() == 1 {
        println!(
            "  A fallback chain answers from another catalogue when one runs \
             dry, for instance --source {},{}.",
            sources[0].key(),
            Source::ALL
                .iter()
                .find(|other| **other != sources[0])
                .map_or("itunes", |other| other.key()),
        );
    }
}
