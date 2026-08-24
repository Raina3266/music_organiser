//! Where a copyright message can be looked up.
//!
//! Four catalogues are wired in, and they do not agree. iTunes and Spotify
//! publish the ℗ line as a finished sentence; MusicBrainz and Discogs record
//! the copyright holder as a credit and the line is assembled from it. Coverage
//! differs too — a release missing from one is often complete in another — so
//! the source is a choice rather than a setting, and the `copyright` command
//! asks for it when it is not told.

pub mod discogs;
mod http;
pub mod itunes;
pub mod menu;
pub mod musicbrainz;
pub mod naming;
pub mod spotify;

pub use http::{DEFAULT_MAX_ATTEMPTS, DEFAULT_MAX_WAIT};

/// How hard a run tries before giving up, on one request and on one source.
///
/// Grouped rather than passed one by one because every source needs both and
/// the list was only going to grow.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Limits {
    /// Longest rate-limit wait to sit through before a source is spent.
    pub max_wait: u64,
    /// How many times to try one request before giving up on that album.
    pub max_attempts: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_wait: DEFAULT_MAX_WAIT,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }
}
#[cfg(test)]
mod testing;

use crate::{AlbumEvidence, CopyrightLookup, LookupError};

/// A catalogue that can be asked for an album's copyright message.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Source {
    Itunes,
    MusicBrainz,
    Discogs,
    Spotify,
}

/// What a source needs before it will answer.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Requirement {
    /// Nothing at all.
    Nothing,
    /// An email address or URL to identify whoever is running this. Optional,
    /// but MusicBrainz asks for it and blocks anonymous applications that
    /// misbehave.
    Contact,
    /// A token, without which the source will not answer at all.
    Token,
}

impl Source {
    /// Every source, in the order the chooser offers them: the two that need
    /// no account first.
    pub const ALL: [Source; 4] = [
        Source::Itunes,
        Source::MusicBrainz,
        Source::Discogs,
        Source::Spotify,
    ];

    /// The name `--source` accepts.
    pub const fn key(self) -> &'static str {
        match self {
            Source::Itunes => "itunes",
            Source::MusicBrainz => "musicbrainz",
            Source::Discogs => "discogs",
            Source::Spotify => "spotify",
        }
    }

    /// The name to put in a message.
    pub const fn title(self) -> &'static str {
        match self {
            Source::Itunes => itunes::LABEL,
            Source::MusicBrainz => musicbrainz::LABEL,
            Source::Discogs => discogs::LABEL,
            Source::Spotify => spotify::LABEL,
        }
    }

    /// How this source gets to a copyright, and what it costs to ask.
    pub const fn summary(self) -> &'static str {
        match self {
            Source::Itunes => "no account needed; the \u{2117} line as Apple's store publishes it",
            Source::MusicBrainz => {
                "no account needed; built from the release's copyright and \
                 phonographic-copyright label relationships"
            }
            Source::Discogs => {
                "needs a personal access token; built from the release's \
                 Copyright (c) and Phonographic Copyright (p) credits"
            }
            Source::Spotify => {
                "needs an access token; the album's own copyright lines, taken as written"
            }
        }
    }

    pub const fn requirement(self) -> Requirement {
        match self {
            Source::Itunes => Requirement::Nothing,
            Source::MusicBrainz => Requirement::Contact,
            Source::Discogs | Source::Spotify => Requirement::Token,
        }
    }

    /// The environment variable this source reads its credential from.
    pub const fn variable(self) -> Option<&'static str> {
        match self {
            Source::Itunes => None,
            Source::MusicBrainz => Some(musicbrainz::CONTACT_VARIABLE),
            Source::Discogs => Some(discogs::TOKEN_VARIABLE),
            Source::Spotify => Some(spotify::TOKEN_VARIABLE),
        }
    }

    /// Where to get a token, for the message that asks for one.
    pub const fn credential_hint(self) -> &'static str {
        match self {
            Source::Itunes => "",
            Source::MusicBrainz => {
                "An email address or URL that reaches you. MusicBrainz asks every application \
                 to identify itself so it can contact whoever runs one that misbehaves, rather \
                 than blocking it outright."
            }
            Source::Discogs => {
                "Generate one at https://www.discogs.com/settings/developers under \
                 \"Generate new token\". It is a personal access token, not your password."
            }
            Source::Spotify => {
                "The same kind of token the download command asks for. One copied from the \
                 open.spotify.com web player works but expires within the hour. Never enter \
                 your password or Client Secret."
            }
        }
    }

    /// Several sources, in the order they should be tried.
    ///
    /// A single name is the common case; a comma-separated list builds a
    /// fallback chain, which is how a run survives one catalogue running out
    /// of rate limit part way through a library.
    pub fn parse_list(value: &str) -> Result<Vec<Self>, String> {
        let mut sources = Vec::new();
        for name in value.split(',') {
            let source = Self::parse(name)?;
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
        if sources.is_empty() {
            return Err("no copyright source given".to_owned());
        }
        Ok(sources)
    }

    /// The source named on the command line.
    ///
    /// The obvious short forms are accepted because the full names are long
    /// and one of them is easy to misspell.
    pub fn parse(value: &str) -> Result<Self, String> {
        let wanted = value.trim().to_lowercase().replace([' ', '-', '_'], "");
        match wanted.as_str() {
            "itunes" | "apple" | "applemusic" => Ok(Source::Itunes),
            "musicbrainz" | "mb" | "brainz" => Ok(Source::MusicBrainz),
            "discogs" => Ok(Source::Discogs),
            "spotify" => Ok(Source::Spotify),
            _ => Err(format!(
                "unknown copyright source {value:?}; choose one of {}",
                Self::names()
            )),
        }
    }

    /// The accepted names, for an error message or a help line.
    pub fn names() -> String {
        Self::ALL
            .iter()
            .map(|source| source.key())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Open a client for this source.
    ///
    /// `credential` is the token or contact address the source needs;
    /// [`Source::requirement`] says which, and whether it may be `None`.
    /// `limits` says how hard to try before giving up on a request and on the
    /// source itself.
    pub fn open(
        self,
        credential: Option<&str>,
        limits: Limits,
    ) -> Result<Box<dyn CopyrightLookup>, String> {
        match self {
            Source::Itunes => Ok(Box::new(itunes::Client::new(limits)?)),
            Source::MusicBrainz => Ok(Box::new(musicbrainz::Client::new(credential, limits)?)),
            Source::Discogs => Ok(Box::new(discogs::Client::new(
                credential.ok_or_else(|| self.missing_credential())?,
                limits,
            )?)),
            Source::Spotify => Ok(Box::new(spotify::Client::new(
                credential.ok_or_else(|| self.missing_credential())?,
                limits,
            )?)),
        }
    }

    fn missing_credential(self) -> String {
        let variable = self.variable().unwrap_or("");
        format!(
            "{} needs a token. Pass --token-file <PATH>, set {variable}, or leave --source off \
             and this command will ask for one.",
            self.title()
        )
    }
}

/// Several sources tried in turn, so one running dry does not end the run.
///
/// This exists because rate limits are not hypothetical: a library is hundreds
/// of albums, and a catalogue that answers the first fifty and then asks for a
/// five-hour wait would otherwise turn every remaining album into an identical
/// failure. Here it is simply dropped, and the albums it never got to are
/// asked of whoever is next.
pub struct Chain {
    links: Vec<Link>,
    /// The source that supplied the most recent answer, for the report.
    answered_by: Option<&'static str>,
}

struct Link {
    source: Source,
    lookup: Box<dyn CopyrightLookup>,
    /// Cleared when the source stops answering; it is never asked again.
    answering: bool,
    /// Albums this source supplied, for the closing summary.
    hits: usize,
}

impl Chain {
    /// Open every source in order. `credentials` supplies each one's token or
    /// contact address, in the same order.
    pub fn open(
        sources: &[Source],
        credentials: &[Option<String>],
        limits: Limits,
    ) -> Result<Self, String> {
        let mut links = Vec::with_capacity(sources.len());
        for (index, source) in sources.iter().enumerate() {
            let credential = credentials.get(index).and_then(Option::as_deref);
            links.push(Link {
                source: *source,
                lookup: source.open(credential, limits)?,
                answering: true,
                hits: 0,
            });
        }
        Ok(Self {
            links,
            answered_by: None,
        })
    }

    /// What each source contributed, for the closing summary. Sources that
    /// stopped answering are named, since that is why a run may have thinner
    /// results than expected.
    pub fn tally(&self) -> Vec<(Source, usize, bool)> {
        self.links
            .iter()
            .map(|link| (link.source, link.hits, link.answering))
            .collect()
    }

    /// The sources still willing to answer.
    pub fn still_answering(&self) -> usize {
        self.links.iter().filter(|link| link.answering).count()
    }
}

impl CopyrightLookup for Chain {
    fn copyright(&mut self, wanted: &AlbumEvidence) -> Result<Option<String>, LookupError> {
        // A source that merely does not know this album is not a problem; the
        // next one is asked. Only an outright failure is worth reporting, and
        // only if nobody after it succeeds.
        let mut failure = None;
        self.answered_by = None;
        for link in self.links.iter_mut().filter(|link| link.answering) {
            match link.lookup.copyright(wanted) {
                Ok(Some(copyright)) => {
                    link.hits += 1;
                    self.answered_by = Some(link.source.title());
                    return Ok(Some(copyright));
                }
                Ok(None) => continue,
                Err(LookupError::Album(error)) => {
                    failure.get_or_insert(error);
                }
                Err(LookupError::Exhausted(error)) => {
                    link.answering = false;
                    eprintln!("{error}.");
                    eprintln!(
                        "  Dropping {} for the rest of this run.",
                        link.source.title()
                    );
                }
            }
        }

        if self.links.iter().all(|link| !link.answering) {
            return Err(LookupError::Exhausted(
                "Every source has stopped answering; stopping here rather than \
                 failing each remaining album in turn."
                    .to_owned(),
            ));
        }
        match failure {
            Some(error) => Err(LookupError::Album(error)),
            None => Ok(None),
        }
    }

    fn answered_by(&self) -> Option<&'static str> {
        self.answered_by
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_names_and_their_short_forms() {
        assert_eq!(Source::parse("itunes").unwrap(), Source::Itunes);
        assert_eq!(Source::parse("  Apple Music ").unwrap(), Source::Itunes);
        assert_eq!(Source::parse("MusicBrainz").unwrap(), Source::MusicBrainz);
        assert_eq!(Source::parse("music-brainz").unwrap(), Source::MusicBrainz);
        assert_eq!(Source::parse("mb").unwrap(), Source::MusicBrainz);
        assert_eq!(Source::parse("Discogs").unwrap(), Source::Discogs);
        assert_eq!(Source::parse("SPOTIFY").unwrap(), Source::Spotify);
    }

    #[test]
    fn names_the_alternatives_when_it_cannot_parse_one() {
        let error = Source::parse("bandcamp").unwrap_err();
        assert!(error.contains("bandcamp"));
        for source in Source::ALL {
            assert!(error.contains(source.key()));
        }
    }

    #[test]
    fn every_source_says_what_it_needs() {
        for source in Source::ALL {
            assert!(!source.summary().is_empty());
            match source.requirement() {
                Requirement::Nothing => assert!(source.variable().is_none()),
                // Anything that wants a credential must say where it is read
                // from and where to get one.
                Requirement::Contact | Requirement::Token => {
                    assert!(source.variable().is_some());
                    assert!(!source.credential_hint().is_empty());
                }
            }
        }
    }

    /// A stand-in catalogue that answers from a script.
    struct Scripted {
        answers: Vec<Result<Option<String>, LookupError>>,
        asked: usize,
    }

    impl CopyrightLookup for Scripted {
        fn copyright(&mut self, _: &AlbumEvidence) -> Result<Option<String>, LookupError> {
            let answer = self.answers[self.asked.min(self.answers.len() - 1)].clone();
            self.asked += 1;
            answer
        }
    }

    fn chain_of(scripts: Vec<Vec<Result<Option<String>, LookupError>>>) -> Chain {
        Chain {
            answered_by: None,
            links: scripts
                .into_iter()
                .zip(Source::ALL)
                .map(|(answers, source)| Link {
                    source,
                    lookup: Box::new(Scripted { answers, asked: 0 }),
                    answering: true,
                    hits: 0,
                })
                .collect(),
        }
    }

    fn wanting(artist: &str, album: &str) -> AlbumEvidence {
        AlbumEvidence {
            artist: artist.to_owned(),
            album: album.to_owned(),
            isrc: None,
            year: None,
            total_tracks: None,
            track_title: None,
        }
    }

    fn found(copyright: &str) -> Result<Option<String>, LookupError> {
        Ok(Some(copyright.to_owned()))
    }

    #[test]
    fn a_source_that_does_not_know_an_album_defers_to_the_next() {
        let mut chain = chain_of(vec![vec![Ok(None)], vec![found("\u{2117} 2001 Daft Life")]]);

        assert_eq!(
            chain
                .copyright(&wanting("Daft Punk", "Discovery"))
                .unwrap()
                .as_deref(),
            Some("\u{2117} 2001 Daft Life")
        );
        let tally = chain.tally();
        assert_eq!(tally[0].1, 0);
        assert_eq!(tally[1].1, 1);
    }

    /// The whole point: one catalogue running out of rate limit must not end
    /// the run. It is dropped, and every remaining album goes to the next.
    #[test]
    fn an_exhausted_source_is_dropped_and_never_asked_again() {
        let mut chain = chain_of(vec![
            vec![Err(LookupError::Exhausted(
                "rate limited for 5h 21m".into(),
            ))],
            vec![found("\u{2117} 2001 Daft Life")],
        ]);

        assert!(
            chain
                .copyright(&wanting("Daft Punk", "Discovery"))
                .unwrap()
                .is_some()
        );
        assert_eq!(chain.still_answering(), 1);

        // A second album must not cost another request to the dead source.
        assert!(
            chain
                .copyright(&wanting("Daft Punk", "Homework"))
                .unwrap()
                .is_some()
        );
        let tally = chain.tally();
        assert_eq!(tally[0], (Source::Itunes, 0, false));
        assert_eq!(tally[1].1, 2);
    }

    #[test]
    fn the_chain_reports_exhaustion_only_once_nobody_is_left() {
        let mut chain = chain_of(vec![
            vec![Err(LookupError::Exhausted("spent".into()))],
            vec![Err(LookupError::Exhausted("spent too".into()))],
        ]);

        let error = chain
            .copyright(&wanting("Daft Punk", "Discovery"))
            .expect_err("with every source dead there is nothing to do");

        assert!(matches!(error, LookupError::Exhausted(_)), "{error}");
        assert_eq!(chain.still_answering(), 0);
    }

    /// One album failing on every source is still just that album failing.
    #[test]
    fn a_failure_everywhere_stays_a_per_album_failure() {
        let mut chain = chain_of(vec![
            vec![Err(LookupError::Album("timed out".into()))],
            vec![Ok(None)],
        ]);

        let error = chain
            .copyright(&wanting("Daft Punk", "Discovery"))
            .unwrap_err();

        assert!(matches!(error, LookupError::Album(_)), "{error}");
        assert_eq!(chain.still_answering(), 2);
    }

    #[test]
    fn a_keyed_source_refuses_to_open_without_its_token() {
        for (source, variable) in [
            (Source::Discogs, discogs::TOKEN_VARIABLE),
            (Source::Spotify, spotify::TOKEN_VARIABLE),
        ] {
            let Err(error) = source.open(None, Limits::default()) else {
                panic!("{} opened without a token", source.title());
            };
            assert!(error.contains(variable));
        }
    }
}
