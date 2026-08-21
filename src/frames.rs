use std::fmt;

/// A case-sensitive user-facing tag name and its ID3v2.3 frame identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TagSpec {
    pub name: &'static str,
    pub frame_id: &'static str,
}

impl fmt::Display for TagSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.name, self.frame_id)
    }
}

/// Supported names are deliberately case-sensitive.
///
/// The first two entries match the syntax from the feature request.
pub const SUPPORTED_TAGS: &[TagSpec] = &[
    TagSpec {
        name: "Encoded-by",
        frame_id: "TENC",
    },
    TagSpec {
        name: "Album Artist",
        frame_id: "TPE2",
    },
    TagSpec {
        name: "Album",
        frame_id: "TALB",
    },
    TagSpec {
        name: "Artist",
        frame_id: "TPE1",
    },
    TagSpec {
        name: "BPM",
        frame_id: "TBPM",
    },
    TagSpec {
        name: "Comment",
        frame_id: "COMM",
    },
    TagSpec {
        name: "Composer",
        frame_id: "TCOM",
    },
    TagSpec {
        name: "Conductor",
        frame_id: "TPE3",
    },
    TagSpec {
        name: "Copyright",
        frame_id: "TCOP",
    },
    TagSpec {
        name: "Date",
        frame_id: "TDAT",
    },
    TagSpec {
        name: "Disc Number",
        frame_id: "TPOS",
    },
    TagSpec {
        name: "Encoding Settings",
        frame_id: "TSSE",
    },
    TagSpec {
        name: "File Owner",
        frame_id: "TOWN",
    },
    TagSpec {
        name: "File Type",
        frame_id: "TFLT",
    },
    TagSpec {
        name: "Genre",
        frame_id: "TCON",
    },
    TagSpec {
        name: "Grouping",
        frame_id: "TIT1",
    },
    TagSpec {
        name: "Initial Key",
        frame_id: "TKEY",
    },
    TagSpec {
        name: "ISRC",
        frame_id: "TSRC",
    },
    TagSpec {
        name: "Language",
        frame_id: "TLAN",
    },
    TagSpec {
        name: "Length",
        frame_id: "TLEN",
    },
    TagSpec {
        name: "Lyrics",
        frame_id: "USLT",
    },
    TagSpec {
        name: "Lyricist",
        frame_id: "TEXT",
    },
    TagSpec {
        name: "Media Type",
        frame_id: "TMED",
    },
    TagSpec {
        name: "Original Album",
        frame_id: "TOAL",
    },
    TagSpec {
        name: "Original Artist",
        frame_id: "TOPE",
    },
    TagSpec {
        name: "Original Filename",
        frame_id: "TOFN",
    },
    TagSpec {
        name: "Original Lyricist",
        frame_id: "TOLY",
    },
    TagSpec {
        name: "Original Release Year",
        frame_id: "TORY",
    },
    TagSpec {
        name: "Picture",
        frame_id: "APIC",
    },
    TagSpec {
        name: "Playlist Delay",
        frame_id: "TDLY",
    },
    TagSpec {
        name: "Publisher",
        frame_id: "TPUB",
    },
    TagSpec {
        name: "Recording Dates",
        frame_id: "TRDA",
    },
    TagSpec {
        name: "Remixed By",
        frame_id: "TPE4",
    },
    TagSpec {
        name: "Subtitle",
        frame_id: "TIT3",
    },
    TagSpec {
        name: "Time",
        frame_id: "TIME",
    },
    TagSpec {
        name: "Title",
        frame_id: "TIT2",
    },
    TagSpec {
        name: "Track Number",
        frame_id: "TRCK",
    },
    TagSpec {
        name: "User Text",
        frame_id: "TXXX",
    },
    TagSpec {
        name: "User URL",
        frame_id: "WXXX",
    },
    TagSpec {
        name: "Year",
        frame_id: "TYER",
    },
];

pub fn find_tag(name: &str) -> Option<TagSpec> {
    SUPPORTED_TAGS.iter().copied().find(|tag| tag.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_case_sensitive() {
        assert_eq!(find_tag("Encoded-by").unwrap().frame_id, "TENC");
        assert!(find_tag("encoded-by").is_none());
        assert!(find_tag("Album artist").is_none());
    }
}
