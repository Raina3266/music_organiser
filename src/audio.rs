//! How long an MPEG audio file plays, counted off the file itself.
//!
//! The lyrics lookup needs this and nothing in the tag can supply it. LRCLIB
//! answers only for a recording whose length matches the one asked about, which
//! is exactly what stops it handing back a remaster's timings for the album
//! cut; `TLEN` would say, but it is not in the download whitelist and spotDL
//! does not write it. So the number is counted off the frames.
//!
//! Both shapes a spotDL download arrives in are read: a constant-bitrate file,
//! whose length is its size over its rate, and a variable-bitrate one, which
//! carries a Xing or VBRI header giving the frame count outright. Anything this
//! cannot read is `None`, and a caller holding no duration asks no question
//! rather than asking a loose one.

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

/// How far past the tag to hunt for the first frame. A well-formed file starts
/// one immediately; the slack covers the junk some encoders leave behind.
const PROBE_BYTES: usize = 16 * 1024;
/// An ID3v1 trailer is a fixed 128 bytes and is not audio.
const ID3V1_LENGTH: u64 = 128;

const BITRATES_V1_L1: [u32; 15] = [
    0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
];
const BITRATES_V1_L2: [u32; 15] = [
    0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
];
const BITRATES_V1_L3: [u32; 15] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];
const BITRATES_V2_L1: [u32; 15] = [
    0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
];
const BITRATES_V2_L23: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

const RATES_V1: [u32; 3] = [44100, 48000, 32000];
const RATES_V2: [u32; 3] = [22050, 24000, 16000];
const RATES_V25: [u32; 3] = [11025, 12000, 8000];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    One,
    Two,
    /// The unofficial MPEG-2.5, which differs from MPEG-2 only in sample rate.
    TwoFive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    One,
    Two,
    Three,
}

/// The four bytes that open every MPEG audio frame, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame {
    version: Version,
    layer: Layer,
    /// Bits per second, already multiplied out.
    bitrate: u32,
    sample_rate: u32,
    padded: bool,
    /// Mono streams carry less side information, which moves the Xing header.
    mono: bool,
}

impl Frame {
    /// Samples carried by one frame, which with the sample rate is how long
    /// that frame plays.
    fn samples(self) -> u32 {
        match (self.version, self.layer) {
            (_, Layer::One) => 384,
            (_, Layer::Two) => 1152,
            (Version::One, Layer::Three) => 1152,
            (_, Layer::Three) => 576,
        }
    }

    /// The whole frame's size on disk, needed to walk from one to the next.
    fn length(self) -> u32 {
        let padding = u32::from(self.padded);
        match self.layer {
            Layer::One => (12 * self.bitrate / self.sample_rate + padding) * 4,
            _ => self.samples() / 8 * self.bitrate / self.sample_rate + padding,
        }
    }

    /// Where a Xing header sits inside this frame, past the side information
    /// whose size depends on the version and the channel count.
    fn xing_offset(self) -> usize {
        let side_info = match (self.version, self.mono) {
            (Version::One, true) => 17,
            (Version::One, false) => 32,
            (_, true) => 9,
            (_, false) => 17,
        };
        4 + side_info
    }
}

/// Decode a frame header, or `None` when these four bytes are not one.
fn parse_frame(bytes: &[u8]) -> Option<Frame> {
    let header: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    // Eleven sync bits, all set.
    if header[0] != 0xFF || header[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = match (header[1] >> 3) & 0b11 {
        0b00 => Version::TwoFive,
        0b10 => Version::Two,
        0b11 => Version::One,
        // 0b01 is reserved and marks this as not a frame after all.
        _ => return None,
    };
    let layer = match (header[1] >> 1) & 0b11 {
        0b01 => Layer::Three,
        0b10 => Layer::Two,
        0b11 => Layer::One,
        _ => return None,
    };

    let bitrate_index = usize::from(header[2] >> 4);
    // 15 is invalid outright, and 0 means "free format", whose rate is not
    // written down anywhere this can read.
    if bitrate_index == 0 || bitrate_index >= 15 {
        return None;
    }
    let table = match (version, layer) {
        (Version::One, Layer::One) => BITRATES_V1_L1,
        (Version::One, Layer::Two) => BITRATES_V1_L2,
        (Version::One, Layer::Three) => BITRATES_V1_L3,
        (_, Layer::One) => BITRATES_V2_L1,
        (_, _) => BITRATES_V2_L23,
    };
    let bitrate = table[bitrate_index] * 1000;

    let rate_index = usize::from((header[2] >> 2) & 0b11);
    if rate_index >= 3 {
        return None;
    }
    let sample_rate = match version {
        Version::One => RATES_V1,
        Version::Two => RATES_V2,
        Version::TwoFive => RATES_V25,
    }[rate_index];

    Some(Frame {
        version,
        layer,
        bitrate,
        sample_rate,
        padded: header[2] & 0b10 != 0,
        mono: header[3] >> 6 == 0b11,
    })
}

/// The frame count a variable-bitrate header declares, if this frame carries
/// one. A VBR file's size says nothing about its length, so without this there
/// is no honest answer for one.
fn vbr_frame_count(frame: Frame, audio: &[u8]) -> Option<u32> {
    let read_u32 = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(audio.get(at..at + 4)?.try_into().ok()?))
    };

    // Xing (variable) and Info (constant, written by the same encoders) share
    // a layout: the tag, then flags, then the fields the flags claim.
    let offset = frame.xing_offset();
    if let Some(tag) = audio.get(offset..offset + 4)
        && (tag == b"Xing" || tag == b"Info")
    {
        let flags = read_u32(offset + 4)?;
        // Bit 0 is the only field this needs, and it comes first.
        if flags & 1 != 0 {
            return read_u32(offset + 8).filter(|count| *count > 0);
        }
        return None;
    }

    // VBRI is Fraunhofer's equivalent and always sits at a fixed offset.
    if let Some(tag) = audio.get(36..40)
        && tag == b"VBRI"
    {
        return read_u32(36 + 14).filter(|count| *count > 0);
    }
    None
}

/// Where the audio starts, stepping over an ID3v2 tag if one opens the file.
fn audio_start(file: &mut File) -> io::Result<u64> {
    let mut header = [0u8; 10];
    if file.read_exact(&mut header).is_err() || &header[..3] != b"ID3" {
        return Ok(0);
    }
    // A syncsafe integer: seven bits of every byte, high bit always clear.
    let size = header[6..10]
        .iter()
        .fold(0u64, |total, byte| (total << 7) | u64::from(byte & 0x7F));
    // Bit 4 of the flags adds a footer of the same size as the header.
    let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
    Ok(10 + size + footer)
}

/// How long `path` plays, in whole seconds, or `None` when it cannot be read
/// as MPEG audio.
pub(crate) fn duration_seconds(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let total = file.metadata().ok()?.len();
    let start = audio_start(&mut file).ok()?;
    if start >= total {
        return None;
    }

    file.seek(SeekFrom::Start(start)).ok()?;
    let mut probe = vec![0u8; PROBE_BYTES.min((total - start) as usize)];
    file.read_exact(&mut probe).ok()?;

    // The first sync word that decodes, and whose successor also decodes: a
    // lone plausible header can appear inside album art or a stray byte, and
    // two in a row at the right spacing almost never do.
    let (at, frame) = (0..probe.len().saturating_sub(4)).find_map(|at| {
        let frame = parse_frame(&probe[at..])?;
        let next = at + frame.length() as usize;
        // A frame running past the probe is taken on trust: it is the only
        // one a short file has, and rejecting it would lose the whole answer.
        if next + 4 <= probe.len() && parse_frame(&probe[next..]).is_none() {
            return None;
        }
        Some((at, frame))
    })?;

    if let Some(frames) = vbr_frame_count(frame, &probe[at..]) {
        let samples = u64::from(frames) * u64::from(frame.samples());
        return Some(samples / u64::from(frame.sample_rate));
    }

    // Constant bitrate: what is left of the file, at the rate it plays.
    let mut audio_bytes = total - start - at as u64;
    if audio_bytes > ID3V1_LENGTH {
        let mut trailer = [0u8; 3];
        if file.seek(SeekFrom::End(-(ID3V1_LENGTH as i64))).is_ok()
            && file.read_exact(&mut trailer).is_ok()
            && &trailer == b"TAG"
        {
            audio_bytes -= ID3V1_LENGTH;
        }
    }
    Some(audio_bytes * 8 / u64::from(frame.bitrate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    /// MPEG-1 Layer III, 128 kbps, 44.1 kHz, stereo, unpadded: what ffmpeg
    /// writes for a spotDL download unless it is asked for something else.
    const HEADER: [u8; 4] = [0xFF, 0xFB, 0x90, 0x00];
    /// 1152 / 8 * 128000 / 44100, truncated, as the decoder computes it.
    const FRAME_LENGTH: usize = 417;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "music-tag-transfer-audio-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// `count` back-to-back frames, silent apart from their headers.
    fn frames(count: usize) -> Vec<u8> {
        let mut audio = Vec::new();
        for _ in 0..count {
            audio.extend_from_slice(&HEADER);
            audio.resize(audio.len() + FRAME_LENGTH - 4, 0);
        }
        audio
    }

    /// An ID3v2 header claiming `size` bytes of tag after it.
    fn id3v2(size: usize) -> Vec<u8> {
        let mut tag = vec![b'I', b'D', b'3', 4, 0, 0];
        for shift in [21, 14, 7, 0] {
            tag.push(((size >> shift) & 0x7F) as u8);
        }
        tag.resize(10 + size, 0);
        tag
    }

    #[test]
    fn a_constant_bitrate_file_is_its_size_over_its_rate() {
        let dir = scratch("cbr");
        let path = dir.join("song.mp3");
        let audio = frames(400);
        fs::write(&path, &audio).unwrap();

        // 400 * 417 bytes at 128 kbps.
        let expected = audio.len() as u64 * 8 / 128_000;
        assert_eq!(duration_seconds(&path), Some(expected));
        assert_eq!(expected, 10);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_id3v2_tag_is_not_counted_as_audio() {
        let dir = scratch("tagged");
        let bare = dir.join("bare.mp3");
        let tagged = dir.join("tagged.mp3");
        let audio = frames(400);
        fs::write(&bare, &audio).unwrap();

        // A cover-art-sized tag, far past anything a header scan would reach
        // by accident.
        let mut with_tag = id3v2(40_000);
        with_tag.extend_from_slice(&audio);
        fs::write(&tagged, &with_tag).unwrap();

        assert_eq!(duration_seconds(&tagged), duration_seconds(&bare));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_xing_header_is_believed_over_the_file_size() {
        let dir = scratch("xing");
        let path = dir.join("song.mp3");

        // One frame carrying a Xing header that declares far more frames than
        // the file holds, so a size-based answer cannot pass for this one.
        let mut first = vec![0u8; FRAME_LENGTH];
        first[..4].copy_from_slice(&HEADER);
        first[36..40].copy_from_slice(b"Xing");
        first[40..44].copy_from_slice(&1u32.to_be_bytes());
        first[44..48].copy_from_slice(&3000u32.to_be_bytes());
        let mut audio = first;
        audio.extend_from_slice(&frames(4));
        fs::write(&path, &audio).unwrap();

        // 3000 frames of 1152 samples at 44.1 kHz.
        assert_eq!(duration_seconds(&path), Some(3000 * 1152 / 44100));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_id3v1_trailer_is_not_counted_as_audio() {
        let dir = scratch("id3v1");
        let bare = dir.join("bare.mp3");
        let trailed = dir.join("trailed.mp3");
        // Enough frames that 128 bytes either way crosses a whole second.
        let audio = frames(400);
        fs::write(&bare, &audio).unwrap();

        let mut with_trailer = audio.clone();
        with_trailer.extend_from_slice(b"TAG");
        with_trailer.resize(audio.len() + ID3V1_LENGTH as usize, 0);
        fs::write(&trailed, &with_trailer).unwrap();

        assert_eq!(duration_seconds(&trailed), duration_seconds(&bare));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_mpeg_audio_has_no_duration() {
        let dir = scratch("junk");
        let path = dir.join("cover.png");
        fs::write(&path, vec![0u8; 4096]).unwrap();
        assert_eq!(duration_seconds(&path), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_lone_sync_word_in_the_middle_of_junk_is_not_a_frame() {
        let dir = scratch("false-sync");
        let path = dir.join("song.mp3");
        // A plausible header with nothing after it, the way album art can
        // happen to read. Taking it would give a wildly wrong length.
        let mut audio = vec![0u8; 200];
        audio.extend_from_slice(&HEADER);
        audio.resize(4096, 0);
        fs::write(&path, &audio).unwrap();
        assert_eq!(duration_seconds(&path), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn free_format_and_reserved_headers_are_refused() {
        // Bitrate index 0 is free format, whose rate is written nowhere.
        assert_eq!(parse_frame(&[0xFF, 0xFB, 0x00, 0x00]), None);
        // Bitrate index 15 is invalid outright.
        assert_eq!(parse_frame(&[0xFF, 0xFB, 0xF0, 0x00]), None);
        // Sample-rate index 3 is reserved.
        assert_eq!(parse_frame(&[0xFF, 0xFB, 0x9C, 0x00]), None);
        // Version 01 is reserved.
        assert_eq!(parse_frame(&[0xFF, 0xEB, 0x90, 0x00]), None);
    }

    #[test]
    fn a_mono_stream_moves_the_xing_header() {
        let stereo = parse_frame(&HEADER).unwrap();
        let mono = parse_frame(&[0xFF, 0xFB, 0x90, 0xC0]).unwrap();
        assert!(!stereo.mono);
        assert!(mono.mono);
        assert_eq!(stereo.xing_offset(), 36);
        assert_eq!(mono.xing_offset(), 21);
    }
}
