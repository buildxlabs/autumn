//! Technical tags: video codec, bit depth, audio, source, streaming service and
//! subtitle languages. Torrent search (Phase 3) filters and scores on these, so
//! each closed vocabulary is an enum.

use crate::Release;

/// A video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VideoCodec {
    /// H.264 / AVC: `x264`, `H.264`, `AVC`.
    H264,
    /// H.265 / HEVC: `x265`, `H.265`, `HEVC`.
    H265,
    /// AV1.
    Av1,
    /// Xvid.
    Xvid,
    /// VP9.
    Vp9,
    /// MPEG-2, as in TV captures.
    Mpeg2,
}

/// An audio codec or track layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Audio {
    /// AAC.
    Aac,
    /// FLAC.
    Flac,
    /// Opus.
    Opus,
    /// AC-3 / Dolby Digital: `AC3`, `DD`.
    Ac3,
    /// E-AC-3 / Dolby Digital Plus: `EAC3`, `E-AC-3`, `DDP`.
    Eac3,
    /// DTS, including `DTS-HD`.
    Dts,
    /// Dolby `TrueHD`.
    TrueHd,
    /// MP3.
    Mp3,
    /// Two audio languages: `Dual Audio`, `Dual-Audio`, `DUAL`.
    DualAudio,
    /// More than two audio languages: `Multi Audio`, `MULTi`.
    MultiAudio,
}

/// Where the video was taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Source {
    /// A Blu-ray disc or remux: `BD`, `BluRay`, `BDRemux`.
    BluRay,
    /// A re-encode of a Blu-ray: `BDRip`.
    BdRip,
    /// A streaming download: `WEB-DL`, `WEB`.
    WebDl,
    /// A re-encode of a stream: `WEBRip`.
    WebRip,
    /// A DVD.
    Dvd,
    /// A re-encode of a DVD: `DVDRip`.
    DvdRip,
    /// A TV capture: `HDTV`, `TV`, `TVRip`.
    Tv,
}

/// The streaming service a web release came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Streaming {
    /// Crunchyroll: `CR`.
    Crunchyroll,
    /// Amazon Prime Video: `AMZN`.
    Amazon,
    /// Netflix: `NF`.
    Netflix,
    /// Disney+: `DSNP`.
    DisneyPlus,
    /// Hulu.
    Hulu,
    /// Bilibili: `BILI`, `BILIBILI`.
    Bilibili,
    /// Anime Digital Network: `ADN`.
    Adn,
    /// iQIYI: `IQ`, `IQIYI`.
    Iqiyi,
    /// Bahamut Anime Crazy: `Baha`.
    Bahamut,
    /// `TVer`.
    Tver,
    /// ABEMA.
    Abema,
    /// HIDIVE.
    Hidive,
}

/// A subtitle language tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Subtitle {
    /// Several languages: `Multi-Subs`, `MultiSub`, `Multiple Subtitle`.
    Multi,
    /// English: `ENG SUB`, `Eng-Subs`.
    English,
    /// French: `VOSTFR`, `SUBFRENCH`.
    French,
    /// Spanish: `Sub_Esp`, `Esp`.
    Spanish,
    /// Brazilian Portuguese: `PT-BR`.
    Portuguese,
    /// Simplified Chinese: `CHS`.
    ChineseSimplified,
    /// Traditional Chinese: `CHT`.
    ChineseTraditional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tech {
    Video(VideoCodec),
    Depth(u32),
    Audio(Audio),
    Source(Source),
    Streaming(Streaming),
    Subtitle(Subtitle),
}

/// Two-word tags, lowercase.
const PAIRS: &[(&str, &str, Tech)] = &[
    ("dual", "audio", Tech::Audio(Audio::DualAudio)),
    ("multi", "audio", Tech::Audio(Audio::MultiAudio)),
    ("multi", "sub", Tech::Subtitle(Subtitle::Multi)),
    ("multi", "subs", Tech::Subtitle(Subtitle::Multi)),
    ("multiple", "subtitle", Tech::Subtitle(Subtitle::Multi)),
    ("multiple", "subtitles", Tech::Subtitle(Subtitle::Multi)),
    ("eng", "sub", Tech::Subtitle(Subtitle::English)),
    ("eng", "subs", Tech::Subtitle(Subtitle::English)),
    ("english", "sub", Tech::Subtitle(Subtitle::English)),
    ("english", "subs", Tech::Subtitle(Subtitle::English)),
    ("sub", "esp", Tech::Subtitle(Subtitle::Spanish)),
];

/// One-word tags, lowercase.
const WORDS: &[(&str, Tech)] = &[
    ("x264", Tech::Video(VideoCodec::H264)),
    ("h264", Tech::Video(VideoCodec::H264)),
    ("h.264", Tech::Video(VideoCodec::H264)),
    ("264", Tech::Video(VideoCodec::H264)),
    ("avc", Tech::Video(VideoCodec::H264)),
    ("x265", Tech::Video(VideoCodec::H265)),
    ("h265", Tech::Video(VideoCodec::H265)),
    ("h.265", Tech::Video(VideoCodec::H265)),
    ("265", Tech::Video(VideoCodec::H265)),
    ("hevc", Tech::Video(VideoCodec::H265)),
    ("av1", Tech::Video(VideoCodec::Av1)),
    ("xvid", Tech::Video(VideoCodec::Xvid)),
    ("vp9", Tech::Video(VideoCodec::Vp9)),
    ("mpeg2", Tech::Video(VideoCodec::Mpeg2)),
    ("10bit", Tech::Depth(10)),
    ("10bits", Tech::Depth(10)),
    ("10-bit", Tech::Depth(10)),
    ("hi10p", Tech::Depth(10)),
    ("hi10", Tech::Depth(10)),
    ("8bit", Tech::Depth(8)),
    ("8-bit", Tech::Depth(8)),
    ("eac3", Tech::Audio(Audio::Eac3)),
    ("e-ac3", Tech::Audio(Audio::Eac3)),
    ("ac-3", Tech::Audio(Audio::Ac3)),
    ("e-ac-3", Tech::Audio(Audio::Eac3)),
    ("ac3", Tech::Audio(Audio::Ac3)),
    ("truehd", Tech::Audio(Audio::TrueHd)),
    ("mp3", Tech::Audio(Audio::Mp3)),
    ("dts-hd", Tech::Audio(Audio::Dts)),
    ("dts-hdma", Tech::Audio(Audio::Dts)),
    ("dual", Tech::Audio(Audio::DualAudio)),
    ("dual-audio", Tech::Audio(Audio::DualAudio)),
    ("multi", Tech::Audio(Audio::MultiAudio)),
    ("multi-audio", Tech::Audio(Audio::MultiAudio)),
    ("multi-Áudio", Tech::Audio(Audio::MultiAudio)),
    ("bd", Tech::Source(Source::BluRay)),
    ("blu-ray", Tech::Source(Source::BluRay)),
    ("bluray", Tech::Source(Source::BluRay)),
    ("bdremux", Tech::Source(Source::BluRay)),
    ("bdmv", Tech::Source(Source::BluRay)),
    ("usbd", Tech::Source(Source::BluRay)),
    ("bdrip", Tech::Source(Source::BdRip)),
    ("uhdbdrip", Tech::Source(Source::BdRip)),
    ("web-dl", Tech::Source(Source::WebDl)),
    ("webdl", Tech::Source(Source::WebDl)),
    ("web", Tech::Source(Source::WebDl)),
    ("webrip", Tech::Source(Source::WebRip)),
    ("web-rip", Tech::Source(Source::WebRip)),
    ("dvd", Tech::Source(Source::Dvd)),
    ("dvdrip", Tech::Source(Source::DvdRip)),
    ("hdtv", Tech::Source(Source::Tv)),
    ("tv", Tech::Source(Source::Tv)),
    ("tvrip", Tech::Source(Source::Tv)),
    ("cr", Tech::Streaming(Streaming::Crunchyroll)),
    ("amzn", Tech::Streaming(Streaming::Amazon)),
    ("nf", Tech::Streaming(Streaming::Netflix)),
    ("dsnp", Tech::Streaming(Streaming::DisneyPlus)),
    ("hulu", Tech::Streaming(Streaming::Hulu)),
    ("bili", Tech::Streaming(Streaming::Bilibili)),
    ("bilibili", Tech::Streaming(Streaming::Bilibili)),
    ("adn", Tech::Streaming(Streaming::Adn)),
    ("iq", Tech::Streaming(Streaming::Iqiyi)),
    ("iqiyi", Tech::Streaming(Streaming::Iqiyi)),
    ("baha", Tech::Streaming(Streaming::Bahamut)),
    ("tver", Tech::Streaming(Streaming::Tver)),
    ("abema", Tech::Streaming(Streaming::Abema)),
    ("hidive", Tech::Streaming(Streaming::Hidive)),
    ("multi-subs", Tech::Subtitle(Subtitle::Multi)),
    ("multi-sub", Tech::Subtitle(Subtitle::Multi)),
    ("multisub", Tech::Subtitle(Subtitle::Multi)),
    ("multisubs", Tech::Subtitle(Subtitle::Multi)),
    ("multi-subtitle", Tech::Subtitle(Subtitle::Multi)),
    ("multi-subtitles", Tech::Subtitle(Subtitle::Multi)),
    ("msubs", Tech::Subtitle(Subtitle::Multi)),
    ("m-sub", Tech::Subtitle(Subtitle::Multi)),
    ("eng-subs", Tech::Subtitle(Subtitle::English)),
    ("eng-sub", Tech::Subtitle(Subtitle::English)),
    ("english-sub", Tech::Subtitle(Subtitle::English)),
    ("vostfr", Tech::Subtitle(Subtitle::French)),
    ("subfrench", Tech::Subtitle(Subtitle::French)),
    ("esp", Tech::Subtitle(Subtitle::Spanish)),
    ("pt-br", Tech::Subtitle(Subtitle::Portuguese)),
    ("chs", Tech::Subtitle(Subtitle::ChineseSimplified)),
    ("cht", Tech::Subtitle(Subtitle::ChineseTraditional)),
];

/// Audio codecs that carry a channel layout (`AAC2.0`, `DDP5.1`), lowercase.
const AUDIO_STEMS: &[(&str, Audio)] = &[
    ("aac", Audio::Aac),
    ("flac", Audio::Flac),
    ("opus", Audio::Opus),
    ("dd", Audio::Ac3),
    ("ddp", Audio::Eac3),
    ("dd+", Audio::Eac3),
    ("dts", Audio::Dts),
];

fn word_tag(lower: &str) -> Option<Tech> {
    if let Some(&(_, tech)) = WORDS.iter().find(|(word, _)| *word == lower) {
        return Some(tech);
    }
    let stem = lower.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    AUDIO_STEMS
        .iter()
        .find(|(word, _)| *word == stem)
        .map(|&(_, audio)| Tech::Audio(audio))
}

/// Reads the tags in `words`, the words after the title. An empty word marks
/// the edge of a bracket, which a two-word tag can't cross.
pub(crate) fn read(words: &[String], release: &mut Release) {
    let mut i = 0;
    while let Some(word) = words.get(i) {
        let lower = word.to_ascii_lowercase();
        let next = words.get(i + 1).map(|w| w.to_ascii_lowercase());
        let pair = PAIRS
            .iter()
            .find(|(first, second, _)| *first == lower && next.as_deref() == Some(*second));
        if let Some(&(_, _, tech)) = pair {
            apply(tech, release);
            i += 2;
            continue;
        }
        match word_tag(&lower) {
            Some(tech) => apply(tech, release),
            // A compound such as `HEVC-10bit`, `x265-KAF`, `TVER.WEB-DL` or `CHS&CHT`.
            None => {
                for part in lower.split(['&', '/']) {
                    match word_tag(part) {
                        Some(tech) => apply(tech, release),
                        // `Multi-Language` and `Dual-Subs` are not audio tags.
                        None => part
                            .split(['-', '.'])
                            .filter(|piece| !["multi", "dual"].contains(piece))
                            .filter_map(word_tag)
                            .for_each(|tech| apply(tech, release)),
                    }
                }
            }
        }
        i += 1;
    }
}

fn apply(tech: Tech, release: &mut Release) {
    match tech {
        Tech::Video(codec) => {
            release.video_codec.get_or_insert(codec);
        }
        Tech::Depth(depth) => {
            release.bit_depth.get_or_insert(depth);
        }
        // The first source wins, except that a disc or web source beats a bare
        // `TV`, which often names the TV series: `Title (TV) [BD 1080p]`.
        Tech::Source(source) => {
            if matches!(release.source, None | Some(Source::Tv)) {
                release.source = Some(source);
            }
        }
        Tech::Streaming(service) => {
            release.streaming.get_or_insert(service);
        }
        Tech::Audio(audio) => {
            if !release.audio.contains(&audio) {
                release.audio.push(audio);
            }
        }
        Tech::Subtitle(subtitle) => {
            if !release.subtitles.contains(&subtitle) {
                release.subtitles.push(subtitle);
            }
        }
    }
}
