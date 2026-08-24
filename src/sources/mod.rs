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
mod naming;
pub mod spotify;
#[cfg(test)]
mod testing;

use crate::CopyrightLookup;

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
    pub fn open(self, credential: Option<&str>) -> Result<Box<dyn CopyrightLookup>, String> {
        match self {
            Source::Itunes => Ok(Box::new(itunes::Client::new()?)),
            Source::MusicBrainz => Ok(Box::new(musicbrainz::Client::new(credential)?)),
            Source::Discogs => Ok(Box::new(discogs::Client::new(
                credential.ok_or_else(|| self.missing_credential())?,
            )?)),
            Source::Spotify => Ok(Box::new(spotify::Client::new(
                credential.ok_or_else(|| self.missing_credential())?,
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

    #[test]
    fn a_keyed_source_refuses_to_open_without_its_token() {
        for (source, variable) in [
            (Source::Discogs, discogs::TOKEN_VARIABLE),
            (Source::Spotify, spotify::TOKEN_VARIABLE),
        ] {
            let Err(error) = source.open(None) else {
                panic!("{} opened without a token", source.title());
            };
            assert!(error.contains(variable));
        }
    }
}
