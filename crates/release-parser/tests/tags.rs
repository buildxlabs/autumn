//! Technical tags: video codec, bit depth, audio, source, streaming service
//! and subtitle languages.

use release_parser::{Audio, Source, Streaming, Subtitle, VideoCodec, parse};

#[test]
fn video_codec_spellings() {
    for (tag, codec) in [
        ("x264", VideoCodec::H264),
        ("H.264", VideoCodec::H264),
        ("H264", VideoCodec::H264),
        ("AVC", VideoCodec::H264),
        ("x265", VideoCodec::H265),
        ("H.265", VideoCodec::H265),
        ("HEVC", VideoCodec::H265),
        ("HEVC-10bit", VideoCodec::H265),
        ("AV1", VideoCodec::Av1),
        ("XviD", VideoCodec::Xvid),
        ("VP9", VideoCodec::Vp9),
        ("MPEG2", VideoCodec::Mpeg2),
    ] {
        let release = parse(&format!("[Group] Title - 01 [1080p {tag}]"));

        assert_eq!(release.video_codec, Some(codec), "{tag}");
    }
}

#[test]
fn scene_codec_before_the_group_suffix() {
    for name in [
        "Title.S01E01.1080p.WEB-DL.H.264-GRP",
        "Title S01E01 1080p WEB-DL AAC2.0 H.264-Tsundere-Raws",
        "Title.S01E01.1080p.WEBRip.x265-KAF",
    ] {
        let release = parse(name);

        assert!(release.video_codec.is_some(), "{name}");
        assert!(release.group.is_some(), "{name}");
    }
}

#[test]
fn bit_depth_spellings() {
    for (tag, depth) in [
        ("10bit", 10),
        ("10-bit", 10),
        ("10Bit", 10),
        ("HEVC-10bit", 10),
        ("Hi10P", 10),
        ("8bit", 8),
        ("AVC-8bit", 8),
    ] {
        let release = parse(&format!("[Group] Title - 01 [1080p {tag}]"));

        assert_eq!(release.bit_depth, Some(depth), "{tag}");
    }
}

#[test]
fn audio_codecs_and_track_layouts() {
    for (tags, audio) in [
        ("AAC", vec![Audio::Aac]),
        ("AAC2.0", vec![Audio::Aac]),
        ("FLAC", vec![Audio::Flac]),
        ("Opus", vec![Audio::Opus]),
        ("AC3", vec![Audio::Ac3]),
        ("DDP5.1", vec![Audio::Eac3]),
        ("E-AC-3", vec![Audio::Eac3]),
        ("EAC3", vec![Audio::Eac3]),
        ("DTS", vec![Audio::Dts]),
        ("TrueHD", vec![Audio::TrueHd]),
        ("FLAC Dual Audio", vec![Audio::Flac, Audio::DualAudio]),
        ("AAC Dual-Audio", vec![Audio::Aac, Audio::DualAudio]),
        ("Opus Multi-Audio", vec![Audio::Opus, Audio::MultiAudio]),
        ("Multi Audio", vec![Audio::MultiAudio]),
    ] {
        let release = parse(&format!("[Group] Title - 01 [1080p {tags}]"));

        assert_eq!(release.audio, audio, "{tags}");
    }
}

#[test]
fn source_spellings() {
    for (tag, source) in [
        ("BD", Source::BluRay),
        ("Blu-ray", Source::BluRay),
        ("BluRay", Source::BluRay),
        ("BDRemux", Source::BluRay),
        ("BDRip", Source::BdRip),
        ("WEB-DL", Source::WebDl),
        ("WEB", Source::WebDl),
        ("WEBRip", Source::WebRip),
        ("WebRip", Source::WebRip),
        ("DVD", Source::Dvd),
        ("DVDRip", Source::DvdRip),
        ("HDTV", Source::Tv),
    ] {
        let release = parse(&format!("[Group] Title - 01 ({tag} 1080p)"));

        assert_eq!(release.source, Some(source), "{tag}");
    }
}

#[test]
fn streaming_service_tags() {
    for (tag, service) in [
        ("CR", Streaming::Crunchyroll),
        ("AMZN", Streaming::Amazon),
        ("NF", Streaming::Netflix),
        ("DSNP", Streaming::DisneyPlus),
        ("HULU", Streaming::Hulu),
        ("BILI", Streaming::Bilibili),
        ("BILIBILI", Streaming::Bilibili),
        ("ADN", Streaming::Adn),
        ("IQ", Streaming::Iqiyi),
        ("Baha", Streaming::Bahamut),
    ] {
        let release = parse(&format!("[Group] Title - 01 [1080p {tag} WEB-DL]"));

        assert_eq!(release.streaming, Some(service), "{tag}");
    }
}

#[test]
fn streaming_tag_glued_to_the_source() {
    let release = parse("[Gecko] Title - S01E15 [TVER.WEB-DL 1080P AVC, AAC][78C7401E]");

    assert_eq!(release.streaming, Some(Streaming::Tver));
    assert_eq!(release.source, Some(Source::WebDl));
}

#[test]
fn subtitle_language_tags() {
    for (tags, subtitles) in [
        ("[Multi-Subs]", vec![Subtitle::Multi]),
        ("[MultiSub]", vec![Subtitle::Multi]),
        ("[Multiple Subtitle]", vec![Subtitle::Multi]),
        ("[ENG SUB]", vec![Subtitle::English]),
        ("[Eng-Subs]", vec![Subtitle::English]),
        ("[VOSTFR]", vec![Subtitle::French]),
        ("[Sub_Esp]", vec![Subtitle::Spanish]),
        ("[PT-BR]", vec![Subtitle::Portuguese]),
        ("[CHT]", vec![Subtitle::ChineseTraditional]),
        ("[CHS]", vec![Subtitle::ChineseSimplified]),
    ] {
        let release = parse(&format!("[Group] Title - 01 [1080p] {tags}"));

        assert_eq!(release.subtitles, subtitles, "{tags}");
    }
}

#[test]
fn tag_words_inside_the_title_are_not_tags() {
    let release = parse("[Group] Web Title - 01 [1080p]");

    assert_eq!(release.title.as_deref(), Some("Web Title"));
    assert_eq!(release.source, None);
}

#[test]
fn a_full_name_gives_every_tag_family() {
    let release = parse(
        "[ToonsHub] PSYREN S01E05 1080p CR WEB-DL DUAL AAC2.0 H.264 (PSYЯEN, Dual-Audio, Multi-Subs)",
    );

    assert_eq!(release.video_codec, Some(VideoCodec::H264));
    assert_eq!(release.source, Some(Source::WebDl));
    assert_eq!(release.streaming, Some(Streaming::Crunchyroll));
    assert_eq!(release.audio, vec![Audio::DualAudio, Audio::Aac]);
    assert_eq!(release.subtitles, vec![Subtitle::Multi]);
}

#[test]
fn tags_joined_by_ampersand_or_slash() {
    let release = parse("[Group] Title - 01 [BDRip 1080p][CHS&CHT&JPN]");
    assert_eq!(
        release.subtitles,
        vec![Subtitle::ChineseSimplified, Subtitle::ChineseTraditional]
    );

    let release = parse("[Gecko] Title - S01-S04 [BILI/YTB.WEB-DL 1080P AVC, AAC, M-SUB][BATCH]");
    assert_eq!(release.streaming, Some(Streaming::Bilibili));
    assert_eq!(release.source, Some(Source::WebDl));
}

#[test]
fn audio_spellings_found_in_review() {
    for (tags, audio) in [
        ("E-AC3", vec![Audio::Eac3]),
        ("DD+5.1", vec![Audio::Eac3]),
        ("AC-3", vec![Audio::Ac3]),
    ] {
        let release = parse(&format!("[Group] Title - 01 [1080p {tags}]"));

        assert_eq!(release.audio, audio, "{tags}");
    }
}

#[test]
fn multi_or_dual_subtitles_are_not_audio() {
    for (tags, subtitles) in [
        ("[Multi-Subtitles]", vec![Subtitle::Multi]),
        ("[Multi-Language]", vec![]),
        ("[Dual-Subs]", vec![]),
    ] {
        let release = parse(&format!("[Group] Title - 01 [1080p] {tags}"));

        assert_eq!(release.audio, vec![], "{tags}");
        assert_eq!(release.subtitles, subtitles, "{tags}");
    }
}

#[test]
fn a_disc_source_beats_a_bare_tv_tag() {
    let release = parse("[Group] Title (TV) [BD 1080p]");
    assert_eq!(release.source, Some(Source::BluRay));

    let release = parse("[NanakoRaws] Title - 01 (CBC TV 1080p HEVC AAC)");
    assert_eq!(release.source, Some(Source::Tv));

    let release = parse("[Group] Title COMPLETE (001-155) [480p] [USBD Remux] [DD]");
    assert_eq!(release.source, Some(Source::BluRay));
}
