//! Asking which source to use, and for whatever it needs.
//!
//! The four catalogues disagree often enough that picking one is a real
//! decision, so an interactive run makes it: the `copyright` command shows what
//! each source knows and what it costs to ask, and only then goes to work.
//! A run with no terminal cannot be asked anything, so it keeps the default.

use std::{
    env,
    io::{self, IsTerminal, Write},
    path::Path,
};

use crate::download::token::{clean_token, read_token_file};
use crate::sources::{Requirement, Source};

/// The source used when nobody is there to be asked.
pub const DEFAULT: Source = Source::Itunes;

/// Whether this run can ask a question and expect an answer.
pub fn interactive() -> bool {
    io::stdin().is_terminal()
}

/// Ask which source to look copyrights up in.
///
/// Answers are the number or the name; an empty answer takes the default, as
/// does a closed stdin, so this never blocks a run that cannot reply.
pub fn choose_source() -> Result<Source, String> {
    eprintln!();
    eprintln!("Where should the copyright message come from?");
    for (index, source) in Source::ALL.iter().enumerate() {
        eprintln!(
            "  {}) {:<12} {}",
            index + 1,
            source.title(),
            source.summary()
        );
    }
    eprintln!();

    loop {
        let answer = ask(&format!(
            "Choose 1-{} or a name, Enter for {}: ",
            Source::ALL.len(),
            DEFAULT.title()
        ))?;
        let Some(answer) = answer else {
            return Ok(DEFAULT);
        };

        if let Ok(number) = answer.parse::<usize>() {
            match Source::ALL.get(number.wrapping_sub(1)) {
                Some(source) => return Ok(*source),
                None => {
                    eprintln!("There is no source {number}.");
                    continue;
                }
            }
        }
        match Source::parse(&answer) {
            Ok(source) => return Ok(source),
            Err(error) => eprintln!("{error}"),
        }
    }
}

/// The credential a source needs, from wherever it has been put.
///
/// The command line wins, then a file, then the environment, and only then is
/// anyone asked. A token belongs in a file or the environment rather than in
/// `--token`, where every process on the machine can read it out of the
/// command line, which is why the prompt exists at all.
pub fn credential_for(
    source: Source,
    token: Option<&str>,
    token_file: Option<&Path>,
    may_ask: bool,
) -> Result<Option<String>, String> {
    let requirement = source.requirement();
    if requirement == Requirement::Nothing {
        if token.is_some() || token_file.is_some() {
            eprintln!("{} needs no token; ignoring the one given.", source.title());
        }
        return Ok(None);
    }

    if let Some(token) = token {
        return clean(source, token).map(Some);
    }
    if let Some(path) = token_file {
        return match requirement {
            // A contact address is not a token and must not be cleaned like
            // one: an email address is far too short to pass for one.
            Requirement::Contact => read_contact_file(path).map(Some),
            _ => read_token_file(path).map(Some),
        };
    }
    if let Some(variable) = source.variable()
        && let Ok(value) = env::var(variable)
        && !value.trim().is_empty()
    {
        return clean(source, &value).map(Some);
    }

    match requirement {
        // MusicBrainz works without one, so this is a note rather than a
        // question: an interactive prompt for an email address every run would
        // be worse than the default User-Agent.
        Requirement::Contact => {
            if let Some(variable) = source.variable() {
                eprintln!(
                    "Set {variable} to an email address or URL so MusicBrainz can reach you."
                );
            }
            Ok(None)
        }
        Requirement::Token if may_ask => prompt_for_token(source),
        Requirement::Token => Err(source.missing_credential()),
        Requirement::Nothing => Ok(None),
    }
}

/// Ask for a token, explaining what it is and where to get one.
fn prompt_for_token(source: Source) -> Result<Option<String>, String> {
    eprintln!();
    eprintln!("{} needs an access token", source.title());
    eprintln!("  {}", source.credential_hint());
    if let Some(variable) = source.variable() {
        eprintln!("  Set {variable} to skip this question next time.");
    }
    let Some(answer) = ask("Paste the token, or press Enter to cancel: ")? else {
        return Err(format!(
            "no {} token given; nothing was looked up or written.",
            source.title()
        ));
    };
    clean(source, &answer).map(Some)
}

fn clean(source: Source, value: &str) -> Result<String, String> {
    match source.requirement() {
        Requirement::Contact => Ok(value.trim().to_owned()),
        _ => clean_token(value).map_err(|error| format!("{}: {error}", source.title())),
    }
}

fn read_contact_file(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map(|contents| contents.trim().to_owned())
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

/// Ask one question. `None` is an empty answer, and so is a closed stdin.
fn ask(question: &str) -> Result<Option<String>, String> {
    eprint!("{question}");
    io::stderr()
        .flush()
        .map_err(|error| format!("cannot flush the prompt: {error}"))?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|error| format!("cannot read the answer: {error}"))?;
    let answer = input.trim();
    Ok((!answer.is_empty()).then(|| answer.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const TOKEN: &str = "test-token-12345678901234567890";

    fn temporary(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "music-tag-transfer-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn takes_the_token_from_the_command_line_first() {
        let credential = credential_for(Source::Discogs, Some(TOKEN), None, false).unwrap();
        assert_eq!(credential.as_deref(), Some(TOKEN));
    }

    #[test]
    fn reads_a_token_from_a_file() {
        let path = temporary("token", &format!("Bearer {TOKEN}\n"));
        let credential = credential_for(Source::Spotify, None, Some(&path), false).unwrap();
        assert_eq!(credential.as_deref(), Some(TOKEN));
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn reads_a_contact_address_without_treating_it_as_a_token() {
        let path = temporary("contact", "  music@example.com\n");
        let credential = credential_for(Source::MusicBrainz, None, Some(&path), false).unwrap();
        assert_eq!(credential.as_deref(), Some("music@example.com"));
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_source_that_needs_nothing_asks_for_nothing() {
        assert_eq!(
            credential_for(Source::Itunes, Some(TOKEN), None, true).unwrap(),
            None
        );
    }

    #[test]
    fn a_keyed_source_fails_rather_than_hanging_when_it_cannot_ask() {
        // With nothing given and no terminal there is nobody to ask, so it has
        // to say where to put a token and stop. A machine that happens to have
        // the variable set is testing the environment path instead.
        let answer = credential_for(Source::Discogs, None, None, false);
        match env::var(Source::Discogs.variable().unwrap()) {
            Ok(token) if !token.trim().is_empty() => assert!(answer.unwrap().is_some()),
            _ => assert!(answer.unwrap_err().contains("--token-file")),
        }
    }
}
