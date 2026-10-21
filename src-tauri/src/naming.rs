//! Tier-1 naming (N-T1, #14): metadata + template → a safe file name stem (no extension). Pure:
//! no I/O, so it's fully unit-tested. [`sanitize`] is also the safety net for AI and hand-edited
//! names (M4, #15). Windows' filename rules apply on every OS (AGENTS.md). Illegal characters get
//! the same fullwidth forms yt-dlp's `--windows-filenames` gives them, so a title keeps the name
//! it had before the engine existed (D-029).
use serde::{Deserialize, Serialize};

use crate::meta::Meta;

/// The naming templates (N3). v1 has exactly these three presets, no template language (D-029).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Template {
    /// `{title}`
    #[default]
    Title,
    /// `{channel} - {title}`
    ChannelTitle,
    /// `{upload_date} {title}`
    DateTitle,
}

/// Longest name kept, in characters (PRD §6.3: "about 150").
pub const MAX_CHARS: usize = 150;
/// Longest name kept, in UTF-8 bytes: room under the 255-byte limit for yt-dlp's suffixes
/// (`.f137`, `.part`, `.temp`), ` (n)` and the extension (#77).
pub const MAX_BYTES: usize = 200;
/// The name when nothing usable is left.
pub const UNTITLED: &str = "Untitled";

/// Every template with its name for a fixed sample item, for the live preview (N3, #17).
pub fn previews() -> Vec<(Template, String)> {
    let sample = Meta {
        title: Some("Me at the zoo [HD]".into()),
        channel: Some("jawed".into()),
        upload_date: Some("20050423".into()),
        ..Default::default()
    };
    [Template::Title, Template::ChannelTitle, Template::DateTitle]
        .into_iter()
        .map(|t| (t, name(&sample, t)))
        .collect()
}

/// The proposed name for an item: its subject (artist – track, series SxxEyy, else the title)
/// cleaned of noise and emoji, put in `template`, then [`sanitize`]d.
pub fn name(meta: &Meta, template: Template) -> String {
    apply(meta, template, clean(&subject(meta)))
}

/// [`name`] with an AI-chosen title in place of the engine's subject (#25, D-029): the AI name
/// replaces `{title}`, and the channel or date still come from the metadata, so the template
/// means the same under every provider.
pub fn with_title(meta: &Meta, template: Template, title: &str) -> String {
    apply(meta, template, collapse(title))
}

/// Puts `subject` into `template`, then sanitizes.
fn apply(meta: &Meta, template: Template, subject: String) -> String {
    let named = match template {
        Template::Title => subject,
        Template::ChannelTitle => match meta.channel.as_deref().map(clean) {
            Some(channel) if subject.is_empty() => channel,
            Some(channel) if !channel.is_empty() && !starts_with_ci(&subject, &channel) => {
                format!("{channel} - {subject}")
            }
            _ => subject,
        },
        Template::DateTitle => match meta.upload_date.as_deref().and_then(date) {
            Some(date) => format!("{date} {subject}"),
            None => subject,
        },
    };
    sanitize(&named)
}

/// Makes any text a safe file name stem on every OS: no control or illegal characters
/// (`"*:<>?|/\` become fullwidth forms; a timestamp's `01:02` becomes `01_02`), single spaces,
/// at most [`MAX_CHARS`] and [`MAX_BYTES`], no leading or trailing dots or spaces, and never a Windows
/// device name (`CON`, `nul.txt`, `COM¹` get a `_`). Bidirectional overrides, which can
/// disguise a name (`…\u{202E}4pm.exe`), are removed. Empty input gives [`UNTITLED`].
pub fn sanitize(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        let digit = |j: Option<usize>| {
            j.and_then(|j| chars.get(j))
                .is_some_and(char::is_ascii_digit)
        };
        match c {
            ':' if digit(i.checked_sub(1)) && digit(Some(i + 1)) => out.push('_'),
            c if c.is_control() => out.push(' '),
            c if is_bidi(c) => {}
            c => out.push(fullwidth(c).unwrap_or(c)),
        }
    }
    // A leading dot would hide the file on macOS and Linux.
    let mut name = unreserved(collapse(&out).trim_start_matches(['.', ' ']).to_owned());
    cap(&mut name);
    let name = name.trim_end_matches(['.', ' ']);
    if name.is_empty() {
        UNTITLED.to_owned()
    } else {
        name.to_owned()
    }
}

/// What the name is about. Structured fields win when the site has them (PRD §6.3).
fn subject(meta: &Meta) -> String {
    let title = meta.title.as_deref().unwrap_or("").trim();
    // A field counts only if something is left of it after cleanup: artist "🔥" with track
    // "(Official Audio)" falls back to the title, never to " - ".
    let field = |f: &Option<String>| f.as_deref().map(clean).filter(|f| !f.is_empty());
    if let (Some(artist), Some(track)) = (field(&meta.artist), field(&meta.track)) {
        return format!("{artist} - {track}");
    }
    if let (Some(series), Some(episode)) = (field(&meta.series), meta.episode_number) {
        let code = match meta.season_number {
            Some(season) => format!("S{season:02}E{episode:02}"),
            None => format!("E{episode:02}"),
        };
        // "The Office - Halloween" → "The Office S02E07 - Halloween".
        let rest = strip_prefix_ci(title, &series)
            .unwrap_or(title)
            .trim_start_matches([' ', '-', '–', '—', ':', '|']);
        return match rest {
            "" => format!("{series} {code}"),
            rest => format!("{series} {code} - {rest}"),
        };
    }
    title.to_owned()
}

/// Removes clickbait noise (bracketed "(Official Video)", "[4K]", "#shorts") and emoji, then
/// collapses whitespace. Brackets that say something real ("(Live at Wembley)") stay.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(['(', '[', '【']) {
        out.push_str(&rest[..open]);
        let group = &rest[open..];
        let first = group.chars().next().map_or(1, char::len_utf8);
        match balanced(group) {
            Some(len) => {
                let last = group[..len].chars().next_back().map_or(1, char::len_utf8);
                if !is_noise(&group[first..len - last]) {
                    out.push_str(&group[..len]);
                }
                rest = &group[len..];
            }
            // Unmatched: keep the bracket as text and keep looking for later groups.
            None => {
                out.push_str(&group[..first]);
                rest = &group[first..];
            }
        }
    }
    out.push_str(rest);
    let words: Vec<&str> = out
        .split_whitespace()
        .filter(|w| !w.eq_ignore_ascii_case("#shorts"))
        .collect();
    collapse(&strip_emoji(&words.join(" ")))
}

/// The byte length of the bracket group `text` starts with, nested groups included
/// (`(Official (Video))`), or `None` if it never closes properly.
fn balanced(text: &str) -> Option<usize> {
    let mut expected = Vec::new();
    for (i, c) in text.char_indices() {
        match c {
            '(' => expected.push(')'),
            '[' => expected.push(']'),
            '【' => expected.push('】'),
            ')' | ']' | '】' => {
                if expected.pop() != Some(c) {
                    return None;
                }
                if expected.is_empty() {
                    return Some(i + c.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

/// A bracket's contents that only advertise the upload: every word is a noise word, as in
/// "Official Music Video", "Lyrics", "4K 60fps", "HD", or there's nothing left but emoji. A
/// bracket that also says something real ("Live at Wembley - Official Video") stays whole.
fn is_noise(inner: &str) -> bool {
    let lower: String = strip_emoji(inner).to_lowercase();
    if lower.trim() == "m/v" {
        return true;
    }
    const NOISE: [&str; 25] = [
        "official",
        "oficial",
        "video",
        "audio",
        "music",
        "lyric",
        "lyrics",
        "visualizer",
        "mv",
        "clip",
        "videoclip",
        "shorts",
        "hd",
        "hq",
        "fhd",
        "uhd",
        "hdr",
        "4k",
        "8k",
        "720p",
        "1080p",
        "1440p",
        "2160p",
        "60fps",
        "full",
    ];
    lower
        .split(|c: char| c.is_whitespace() || c == '-')
        .map(|w| w.trim_matches(['(', ')', '[', ']', '【', '】']))
        .filter(|w| !w.is_empty())
        .all(|w| NOISE.contains(&w))
}

/// `text` without emoji. A keycap (`1️⃣`: a digit, `#` or `*`, then U+FE0F and U+20E3) goes
/// as a whole, digit and all.
fn strip_emoji(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let keycap = match (chars.get(i + 1), chars.get(i + 2)) {
            (Some('\u{20E3}'), _) => Some(2),
            (Some('\u{FE0F}'), Some('\u{20E3}')) => Some(3),
            _ => None,
        };
        match keycap {
            Some(len) if c.is_ascii_digit() || c == '#' || c == '*' => i += len,
            _ => {
                if !is_emoji(c) {
                    out.push(c);
                }
                i += 1;
            }
        }
    }
    out
}

/// Emoji, pictographs and the joiners/selectors that build them. CJK, accents and symbols like
/// `&`, `#`, `♯`, `★` or `✓` are kept: in the older symbol blocks, only the characters Unicode
/// lists as emoji (the `Emoji` property, emoji-data.txt) go, and of those only the ones shown as
/// emoji by default: text-style ones like © ® ™ ‼ ↔ ▶ Ⓜ stay. Enclosed letters (`🄰`), game and
/// chess symbols, geometric shapes and arrows in the U+1F000 planes are kept too.
fn is_emoji(c: char) -> bool {
    let c = c as u32;
    matches!(c,
        0x1F300..=0x1F64F   // pictographs, emoticons
        | 0x1F680..=0x1F6FF // transport and map
        | 0x1F900..=0x1F9FF // supplemental pictographs
        | 0x1FA70..=0x1FAFF // pictographs extended-A
        | 0x1F1E6..=0x1F1FF // regional indicators (flags)
        | 0xFE00..=0xFE0F   // variation selectors
        | 0x200D            // zero-width joiner
        | 0x20E3            // keycap
        | 0xE0000..=0xE007F // tags
    ) || SYMBOL_EMOJI.iter().any(|&(lo, hi)| (lo..=hi).contains(&c))
}

/// The `Emoji` characters in U+2300–2BFF and outside the pictograph blocks of U+1F000–1FAFF
/// (Unicode emoji-data.txt), e.g. ⌚ ☀ ♥ ⚽ ✨ ❤ ⭐; the
/// rest of those blocks (⌘ ⌀ ♭ ♮ ♯ ★ ☆ ✓ arrows) are ordinary symbols.
const SYMBOL_EMOJI: [(u32, u32); 87] = [
    (0x231A, 0x231B),
    (0x2328, 0x2328),
    (0x23CF, 0x23CF),
    (0x23E9, 0x23F3),
    (0x23F8, 0x23FA),
    (0x2600, 0x2604),
    (0x260E, 0x260E),
    (0x2611, 0x2611),
    (0x2614, 0x2615),
    (0x2618, 0x2618),
    (0x261D, 0x261D),
    (0x2620, 0x2620),
    (0x2622, 0x2623),
    (0x2626, 0x2626),
    (0x262A, 0x262A),
    (0x262E, 0x262F),
    (0x2638, 0x263A),
    (0x2640, 0x2640),
    (0x2642, 0x2642),
    (0x2648, 0x2653),
    (0x265F, 0x2660),
    (0x2663, 0x2663),
    (0x2665, 0x2666),
    (0x2668, 0x2668),
    (0x267B, 0x267B),
    (0x267E, 0x267F),
    (0x2692, 0x2697),
    (0x2699, 0x2699),
    (0x269B, 0x269C),
    (0x26A0, 0x26A1),
    (0x26A7, 0x26A7),
    (0x26AA, 0x26AB),
    (0x26B0, 0x26B1),
    (0x26BD, 0x26BE),
    (0x26C4, 0x26C5),
    (0x26C8, 0x26C8),
    (0x26CE, 0x26CF),
    (0x26D1, 0x26D1),
    (0x26D3, 0x26D4),
    (0x26E9, 0x26EA),
    (0x26F0, 0x26F5),
    (0x26F7, 0x26FA),
    (0x26FD, 0x26FD),
    (0x2702, 0x2702),
    (0x2705, 0x2705),
    (0x2708, 0x270D),
    (0x270F, 0x270F),
    (0x2712, 0x2712),
    (0x2714, 0x2714),
    (0x2716, 0x2716),
    (0x271D, 0x271D),
    (0x2721, 0x2721),
    (0x2728, 0x2728),
    (0x2733, 0x2734),
    (0x2744, 0x2744),
    (0x2747, 0x2747),
    (0x274C, 0x274C),
    (0x274E, 0x274E),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2763, 0x2764),
    (0x2795, 0x2797),
    (0x27A1, 0x27A1),
    (0x27B0, 0x27B0),
    (0x27BF, 0x27BF),
    (0x2934, 0x2935),
    (0x2B05, 0x2B07),
    (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50),
    (0x2B55, 0x2B55),
    (0x3030, 0x3030),
    (0x303D, 0x303D),
    (0x3297, 0x3297),
    (0x3299, 0x3299),
    // Emoji outside the pictograph blocks of the U+1F000 planes.
    (0x1F004, 0x1F004),
    (0x1F0CF, 0x1F0CF),
    (0x1F170, 0x1F171),
    (0x1F17E, 0x1F17F),
    (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A),
    (0x1F201, 0x1F202),
    (0x1F21A, 0x1F21A),
    (0x1F22F, 0x1F22F),
    (0x1F232, 0x1F23A),
    (0x1F250, 0x1F251),
    (0x1F7E0, 0x1F7EB),
    (0x1F7F0, 0x1F7F0),
];

/// Bidirectional formatting characters (U+202A–202E, U+2066–2069, marks).
fn is_bidi(c: char) -> bool {
    matches!(c as u32, 0x202A..=0x202E | 0x2066..=0x2069 | 0x200E | 0x200F | 0x061C)
}

/// yt-dlp's `--windows-filenames` replacement for a character illegal on Windows.
fn fullwidth(c: char) -> Option<char> {
    Some(match c {
        '/' => '⧸',
        '\\' => '⧹',
        '"' | '*' | ':' | '<' | '>' | '?' | '|' => char::from_u32(c as u32 + 0xFEE0)?,
        _ => return None,
    })
}

/// Single spaces, trimmed.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cuts `name` to [`MAX_CHARS`] characters and [`MAX_BYTES`] bytes, at a character boundary.
fn cap(name: &mut String) {
    if let Some((at, _)) = name.char_indices().nth(MAX_CHARS) {
        name.truncate(at);
    }
    if name.len() > MAX_BYTES {
        let mut at = MAX_BYTES;
        while !name.is_char_boundary(at) {
            at -= 1;
        }
        name.truncate(at);
    }
}

/// Windows refuses device names as the part before the first dot, ignoring case and trailing
/// spaces (`CON`, `nul.txt`, `Com1 .mp3`); a `_` after the device word makes it a normal name.
fn unreserved(name: String) -> String {
    let base_end = name.find('.').unwrap_or(name.len());
    let word = name[..base_end].trim_end();
    let upper = word.to_uppercase();
    let device = ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str())
        || ["COM", "LPT"].iter().any(|dev| {
            upper
                .strip_prefix(dev)
                .is_some_and(|n| n.chars().count() == 1 && "0123456789¹²³".contains(n))
        });
    if device {
        format!("{word}_{}", &name[word.len()..])
    } else {
        name
    }
}

/// `YYYYMMDD` → `YYYY-MM-DD`.
fn date(raw: &str) -> Option<String> {
    (raw.len() == 8 && raw.bytes().all(|b| b.is_ascii_digit()))
        .then(|| format!("{}-{}-{}", &raw[..4], &raw[4..6], &raw[6..]))
}

fn starts_with_ci(text: &str, prefix: &str) -> bool {
    strip_prefix_ci(text, prefix).is_some()
}

/// `text` after `prefix`, ignoring case, only at a word boundary: "Art" isn't a prefix of
/// "Artificial", nor "Dark" of "Darkness".
fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    let rest = &text[prefix.len()..];
    let boundary = !rest.starts_with(char::is_alphanumeric);
    (boundary && head.to_lowercase() == prefix.to_lowercase()).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titled(title: &str) -> Meta {
        Meta {
            title: Some(title.into()),
            ..Default::default()
        }
    }

    /// Real titles → the Tier-1 name for `{title}`.
    #[test]
    fn real_titles_get_clean_safe_names() {
        let long_cjk = "日".repeat(230);
        let long_ascii = "a".repeat(300);
        let cases: &[(&str, &str)] = &[
            ("Me at the zoo", "Me at the zoo"),
            (
                "Rick Astley - Never Gonna Give You Up (Official Music Video)",
                "Rick Astley - Never Gonna Give You Up",
            ),
            (
                "Daft Punk - Get Lucky (Official Audio) ft. Pharrell Williams, Nile Rodgers",
                "Daft Punk - Get Lucky ft. Pharrell Williams, Nile Rodgers",
            ),
            (
                "Joe Rogan Experience #2045 - Mel Gibson",
                "Joe Rogan Experience #2045 - Mel Gibson",
            ),
            ("🔥 INSANE 🔥 Build [4K]", "INSANE Build"),
            (
                "lofi hip hop radio 📚 - beats to relax/study to",
                "lofi hip hop radio - beats to relax⧸study to",
            ),
            ("Song Name [Lyrics]", "Song Name"),
            ("Song Name (Lyric Video)", "Song Name"),
            ("Trailer (HD 1080p)", "Trailer"),
            ("Concert (4K 60FPS) [Official Video]", "Concert"),
            ("Despacito (Video Oficial)", "Despacito"),
            (
                "Wonderwall (Live at Wembley)",
                "Wonderwall (Live at Wembley)",
            ),
            (
                "Bohemian Rhapsody (Remastered 2011)",
                "Bohemian Rhapsody (Remastered 2011)",
            ),
            (
                "PSY - GANGNAM STYLE(강남스타일) M/V",
                "PSY - GANGNAM STYLE(강남스타일) M⧸V",
            ),
            ("How to cook rice #shorts", "How to cook rice"),
            ("👨‍👩‍👧 Family vlog ❤️", "Family vlog"),
            ("Podcast 01:02:03", "Podcast 01_02_03"),
            (
                "What is AI? | Explained: part 1",
                "What is AI？ ｜ Explained： part 1",
            ),
            (
                "AC/DC: \"Back\" <in> Black * \\",
                "AC⧸DC： ＂Back＂ ＜in＞ Black ＊ ⧹",
            ),
            ("   spaced \t\n  out   ", "spaced out"),
            ("Ends with dots...", "Ends with dots"),
            (
                "Café crème — édition spéciale",
                "Café crème — édition spéciale",
            ),
            ("(Official Video)", UNTITLED),
            ("🔥🔥🔥", UNTITLED),
            ("", UNTITLED),
            ("CON", "CON_"),
            ("nul.txt", "nul_.txt"),
            ("Com1 .mp3", "Com1_ .mp3"),
            ("LPT³", "LPT³_"),
            ("CONSOLE", "CONSOLE"),
            ("100% Legit", "100% Legit"),
            ("Song [M/V]", "Song"),
            // #14 review: music and other symbols aren't emoji.
            (
                "Prelude in C♯ minor (Official Audio)",
                "Prelude in C♯ minor",
            ),
            ("Prelude in C♯ minor 🔥", "Prelude in C♯ minor"),
            ("Sonata in B♭ ♮ ★ ☆ ✓ → ©", "Sonata in B♭ ♮ ★ ☆ ✓ → ©"),
            ("Sunny ☀️ day ✨ ⚽ ✔️ ⭐", "Sunny day"),
            ("Lesson 🄰 and 🄱 🄫 ♔ 🀄 🅰️ 🇯🇵", "Lesson 🄰 and 🄱 🄫 ♔"),
            ("Song (Official Video [4K])", "Song"),
            ("Song [Live (HD)]", "Song [Live (HD)]"),
            ("Song (Official (Video))", "Song"),
            ("Song [Official [4K]]", "Song"),
            ("Song (Live (Acoustic))", "Song (Live (Acoustic))"),
            ("Song (unclosed [Official Video]", "Song (unclosed"),
            ("Song ) stray (Lyrics)", "Song ) stray"),
            ("Invoice\u{202E}fdp.exe", "Invoicefdp.exe"),
        ];
        for (title, expected) in cases {
            assert_eq!(
                &name(&titled(title), Template::Title),
                expected,
                "{title:?}"
            );
        }
        let cjk = name(&titled(&long_cjk), Template::Title);
        assert!(
            cjk.len() <= MAX_BYTES && cjk.len() > MAX_BYTES - 3,
            "{}",
            cjk.len()
        );
        assert_eq!(
            name(&titled(&long_ascii), Template::Title).chars().count(),
            MAX_CHARS
        );
    }

    #[test]
    fn structured_fields_win_over_the_title() {
        let song = Meta {
            title: Some("DAFT PUNK GET LUCKY OFFICIAL!!!".into()),
            artist: Some("Daft Punk".into()),
            track: Some("Get Lucky".into()),
            ..Default::default()
        };
        assert_eq!(name(&song, Template::Title), "Daft Punk - Get Lucky");
        let key = Meta {
            artist: Some("Chopin".into()),
            track: Some("Nocturne in E♭ major 🎹".into()),
            ..Default::default()
        };
        assert_eq!(name(&key, Template::Title), "Chopin - Nocturne in E♭ major");
        let emptied = Meta {
            title: Some("A real title".into()),
            artist: Some("🔥".into()),
            track: Some("(Official Audio)".into()),
            series: Some("🎬".into()),
            episode_number: Some(1),
            ..Default::default()
        };
        assert_eq!(
            name(&emptied, Template::Title),
            "A real title",
            "fields that clean to nothing"
        );
        let episode = Meta {
            title: Some("The Office - Halloween".into()),
            series: Some("The Office".into()),
            season_number: Some(2),
            episode_number: Some(5),
            ..Default::default()
        };
        assert_eq!(
            name(&episode, Template::Title),
            "The Office S02E05 - Halloween"
        );
        let unnumbered_season = Meta {
            title: Some("Pilot".into()),
            series: Some("Show".into()),
            episode_number: Some(1),
            ..Default::default()
        };
        assert_eq!(
            name(&unnumbered_season, Template::Title),
            "Show E01 - Pilot"
        );
        let artist_only = Meta {
            title: Some("A title".into()),
            artist: Some("Someone".into()),
            ..Default::default()
        };
        assert_eq!(
            name(&artist_only, Template::Title),
            "A title",
            "needs both fields"
        );
    }

    /// #25: an AI title takes `{title}`'s place; the engine still adds the channel or date and
    /// makes it safe, but doesn't "clean" the AI's wording.
    #[test]
    fn an_ai_title_fills_the_template() {
        let meta = Meta {
            title: Some("daft punk get lucky OFFICIAL".into()),
            channel: Some("Daft Punk".into()),
            upload_date: Some("20130419".into()),
            ..Default::default()
        };
        let ai = "Daft Punk – Get Lucky (feat. Pharrell Williams)";
        assert_eq!(with_title(&meta, Template::Title, ai), ai);
        assert_eq!(
            with_title(&meta, Template::ChannelTitle, ai),
            ai,
            "the channel isn't repeated"
        );
        assert_eq!(
            with_title(&meta, Template::DateTitle, ai),
            format!("2013-04-19 {ai}")
        );
        assert_eq!(with_title(&meta, Template::Title, "a/b  c"), "a⧸b c");
    }

    #[test]
    fn previews_show_every_template_on_one_sample() {
        assert_eq!(
            previews(),
            [
                (Template::Title, "Me at the zoo".to_owned()),
                (Template::ChannelTitle, "jawed - Me at the zoo".to_owned()),
                (Template::DateTitle, "2005-04-23 Me at the zoo".to_owned()),
            ]
        );
        assert_eq!(
            serde_json::to_string(&Template::ChannelTitle).unwrap(),
            r#""channel_title""#
        );
    }

    #[test]
    fn templates_add_the_channel_or_the_date() {
        let meta = Meta {
            title: Some("Me at the zoo".into()),
            channel: Some("jawed".into()),
            upload_date: Some("20050424".into()),
            ..Default::default()
        };
        assert_eq!(name(&meta, Template::ChannelTitle), "jawed - Me at the zoo");
        assert_eq!(name(&meta, Template::DateTitle), "2005-04-24 Me at the zoo");
        let repeated = Meta {
            title: Some("Rick Astley - Never Gonna Give You Up".into()),
            channel: Some("Rick Astley".into()),
            ..Default::default()
        };
        assert_eq!(
            name(&repeated, Template::ChannelTitle),
            "Rick Astley - Never Gonna Give You Up",
            "the channel isn't repeated"
        );
        let bare = titled("No extras");
        assert_eq!(name(&bare, Template::ChannelTitle), "No extras");
        assert_eq!(name(&bare, Template::DateTitle), "No extras");
        let channel_only = Meta {
            channel: Some("Only/Channel".into()),
            ..Default::default()
        };
        assert_eq!(name(&channel_only, Template::ChannelTitle), "Only⧸Channel");
    }

    /// The safety net for AI and edited names (M4, #15): no cleanup of wording, only safety.
    #[test]
    fn sanitize_keeps_wording_and_only_makes_it_safe() {
        assert_eq!(
            sanitize("Daft Punk – Get Lucky (feat. Pharrell Williams & Nile Rodgers)"),
            "Daft Punk – Get Lucky (feat. Pharrell Williams & Nile Rodgers)"
        );
        assert_eq!(
            sanitize("Song (Official Video) 🔥"),
            "Song (Official Video) 🔥"
        );
        assert_eq!(sanitize("a/b\\c\u{0}d\u{7}e"), "a⧸b⧹c d e");
        assert_eq!(sanitize(" .. "), UNTITLED);
        assert_eq!(sanitize(".hidden"), "hidden", "no hidden files");
        assert_eq!(sanitize("../evil"), "⧸evil");
        assert_eq!(sanitize("aux"), "aux_");
        let long = format!("{}x", "é".repeat(120)); // 241 bytes
        let cut = sanitize(&long);
        assert!(cut.len() <= MAX_BYTES && cut.starts_with('é') && !cut.ends_with('x'));
        assert_eq!(
            sanitize(&format!("{}.  ", "b".repeat(149))),
            "b".repeat(149)
        );
    }

    /// Every output is safe whatever the input: a cheap property check over awkward strings.
    #[test]
    fn every_name_is_safe() {
        let parts = [
            "CON",
            "日本語",
            "🔥",
            "a/b",
            ":",
            "..",
            " ",
            "\u{202E}",
            "LPT1",
            "(HD)",
            "x",
        ];
        for a in parts {
            for b in parts {
                for c in parts {
                    let out = name(&titled(&format!("{a}{b}{c}")), Template::Title);
                    assert!(!out.is_empty() && out.len() <= MAX_BYTES, "{out:?}");
                    assert!(
                        !out.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']),
                        "{out:?}"
                    );
                    assert!(
                        !out.chars().any(|c| c.is_control() || is_bidi(c)),
                        "{out:?}"
                    );
                    assert!(
                        !out.ends_with(['.', ' ']) && !out.starts_with(['.', ' ']),
                        "{out:?}"
                    );
                    assert_eq!(unreserved(out.clone()), out, "a device name: {out:?}");
                }
            }
        }
    }
}
