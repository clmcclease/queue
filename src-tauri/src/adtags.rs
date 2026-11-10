//! Advertising tags removed from AI answers (N9, #104, D-036, D-041): an exact, user-editable list
//! of phrases such as "Official Video" or "4K". A phrase is removed only where it fills a whole
//! bracket (`(Official Video)`, `[4K]`, `【MV】`), ignoring case; nothing else in the name is touched.
//! No general rules (D-032): "(Official Video Remix)" and a bare "… Official MV" stay. Pure, so
//! it's fully unit-tested.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The starter list (owner, 2026-11-10, D-041). Users can add, edit and remove phrases in
/// Settings and reset to this list.
pub const DEFAULTS: [&str; 57] = [
    // English
    "Official Video",
    "Official Music Video",
    "Official Audio",
    "Official Lyric Video",
    "Official Lyrics Video",
    "Official Visualizer",
    "Official Visualiser",
    "Official MV",
    "Official M/V",
    "Official Clip",
    "Official HD Video",
    "Official 4K Video",
    "Music Video",
    "Lyric Video",
    "Lyrics Video",
    "Lyrics",
    "Audio",
    "Visualizer",
    "Visualiser",
    "MV",
    "M/V",
    "Official",
    "Video",
    "HD",
    "HQ",
    "4K",
    "8K",
    "UHD",
    "HDR",
    "1080p",
    "60fps",
    "4K 60fps",
    // Spanish and Portuguese
    "Video Oficial",
    "Vídeo Oficial",
    "Videoclip Oficial",
    "Audio Oficial",
    "Letra",
    "Clipe Oficial",
    "Videoclipe Oficial",
    // French
    "Clip officiel",
    "Clip Officiel HD",
    "Vidéo officielle",
    "Audio officiel",
    "Clip",
    // German and Italian
    "Offizielles Musikvideo",
    "Offizielles Video",
    "Musikvideo",
    "Video Ufficiale",
    "Videoclip Ufficiale",
    "Audio Ufficiale",
    // Turkish and Russian
    "Resmi Video",
    "Resmi Klip",
    "Resmi Video Klip",
    "Video Klip",
    "Официальный клип",
    "Официальное видео",
    "Премьера клипа",
];

/// Longest phrase kept, in characters.
pub const MAX_CHARS: usize = 100;
/// Most phrases kept.
pub const MAX_TAGS: usize = 300;

/// The bracket pairs a tag can fill. `「」` isn't one: Japanese titles quote song names with it.
const PAIRS: [(char, char); 5] = [
    ('(', ')'),
    ('[', ']'),
    ('（', '）'),
    ('［', '］'),
    ('【', '】'),
];

/// The user's list of tag phrases: trimmed, single spaces, no empty or over-long phrases, no
/// duplicates (ignoring case), at most [`MAX_TAGS`]. A stored phrase is otherwise kept exactly;
/// only text typed in Settings has a surrounding bracket pair dropped ([`AdTags::with_added`],
/// [`AdTags::with_edited`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Vec<String>", into = "Vec<String>")]
pub struct AdTags(Vec<String>);

impl Default for AdTags {
    fn default() -> Self {
        Self::new(DEFAULTS)
    }
}

impl From<Vec<String>> for AdTags {
    fn from(list: Vec<String>) -> Self {
        Self::new(list)
    }
}

impl From<AdTags> for Vec<String> {
    fn from(tags: AdTags) -> Self {
        tags.0
    }
}

impl AdTags {
    pub fn new<S: AsRef<str>>(list: impl IntoIterator<Item = S>) -> Self {
        let mut seen = HashSet::new();
        let mut kept = Vec::new();
        for phrase in list {
            let phrase = collapse(phrase.as_ref());
            if phrase.is_empty() || phrase.chars().count() > MAX_CHARS {
                continue;
            }
            if kept.len() < MAX_TAGS && seen.insert(key(&phrase)) {
                kept.push(phrase);
            }
        }
        Self(kept)
    }

    pub fn list(&self) -> &[String] {
        &self.0
    }

    /// The position of `phrase` (tidied, ignoring case and spacing), if it's on the list.
    fn find(&self, phrase: &str) -> Option<usize> {
        // Exactly as stored: a stored "(Clip)" is never unwrapped again (#104 review round 3).
        let wanted = key(phrase);
        self.0.iter().position(|t| key(t) == wanted)
    }

    /// This list with `phrase` added at the end (a duplicate or empty phrase changes nothing).
    pub fn with_added(&self, phrase: &str) -> Self {
        Self::new(self.0.iter().cloned().chain([typed(phrase)]))
    }

    /// This list with `old` replaced by `new` in its place; `new` empty removes it, and `new`
    /// already on the list merges into it. If `old` isn't on the list (another change removed or
    /// replaced it first), nothing changes (#104 review).
    pub fn with_edited(&self, old: &str, new: &str) -> Self {
        let Some(at) = self.find(old) else {
            return self.clone();
        };
        let mut list = self.0.clone();
        list[at] = typed(new);
        Self::new(list)
    }

    /// This list without `phrase`; if it isn't on the list, nothing changes.
    pub fn without(&self, phrase: &str) -> Self {
        match self.find(phrase) {
            Some(at) => {
                let mut list = self.0.clone();
                list.remove(at);
                Self(list)
            }
            None => self.clone(),
        }
    }

    /// `title` without the brackets whose whole contents are one of these phrases, ignoring case
    /// and spacing. Unmatched or nested brackets that don't match as a whole stay. If nothing
    /// would be left, `title` is returned unchanged.
    pub fn strip(&self, title: &str) -> String {
        if self.0.is_empty() {
            return title.to_owned();
        }
        let keys: HashSet<String> = self.0.iter().map(|t| key(t)).collect();
        let mut out = String::with_capacity(title.len());
        let mut rest = title;
        let mut removed = false;
        while let Some(open) = rest.find(|c| PAIRS.iter().any(|&(o, _)| o == c)) {
            out.push_str(&rest[..open]);
            let group = &rest[open..];
            let first = group.chars().next().map_or(1, char::len_utf8);
            match balanced(group) {
                Some(len) => {
                    let last = group[..len].chars().next_back().map_or(1, char::len_utf8);
                    let inner = &group[first..len - last];
                    if !inner.trim().is_empty() && keys.contains(&key(inner)) {
                        out.push(' ');
                        removed = true;
                    } else {
                        out.push_str(&group[..len]);
                    }
                    rest = &group[len..];
                }
                None => {
                    out.push_str(&group[..first]);
                    rest = &group[first..];
                }
            }
        }
        out.push_str(rest);
        if !removed {
            return title.to_owned();
        }
        // A tag can hide a media extension a model added ("Song.mp4 (Official Video)"): the
        // answer's own check only sees the end of the answer, so it's removed here too (#104 QA).
        let mut stripped = collapse(&out);
        if let Some((stem, _)) = stripped.rsplit_once('.')
            && crate::ai::has_media_extension(&stripped)
        {
            stripped = collapse(stem);
        }
        if stripped.is_empty() {
            title.to_owned()
        } else {
            stripped
        }
    }
}

/// A phrase as typed in Settings, made ready to store: single spaces, and without one bracket
/// pair around the whole of it, so "(Official Video)" means "Official Video". Only new text gets
/// this; stored phrases are never unwrapped again, so a phrase saved as "(Clip)" keeps matching
/// "((Clip))" through later changes and restarts (#104 review round 3).
fn typed(phrase: &str) -> String {
    let phrase = collapse(phrase);
    let unwrapped = PAIRS.iter().find_map(|&(o, c)| {
        phrase
            .strip_prefix(o)
            .and_then(|p| p.strip_suffix(c))
            .filter(|inner| {
                balanced(&phrase).is_some_and(|len| len == phrase.len()) && !inner.is_empty()
            })
    });
    match unwrapped {
        Some(inner) => collapse(inner),
        None => phrase,
    }
}

/// What a phrase and a bracket's contents are compared by: single spaces, lower case, and the
/// characters sanitizing replaces in their replaced form, exactly as sanitizing replaces them (an
/// answer's "M/V" is "M⧸V" by then, and "01:02" is "01_02").
fn key(text: &str) -> String {
    collapse(&crate::naming::replace_illegal(text)).to_lowercase()
}

/// The byte length of the bracket group `text` starts with, nested groups included, or `None`
/// if it never closes properly.
fn balanced(text: &str) -> Option<usize> {
    let mut expected = Vec::new();
    for (i, c) in text.char_indices() {
        if let Some(&(_, close)) = PAIRS.iter().find(|&&(o, _)| o == c) {
            expected.push(close);
        } else if PAIRS.iter().any(|&(_, close)| close == c) {
            if expected.pop() != Some(c) {
                return None;
            }
            if expected.is_empty() {
                return Some(i + c.len_utf8());
            }
        }
    }
    None
}

/// Single spaces, trimmed.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(title: &str) -> String {
        AdTags::default().strip(title)
    }

    #[test]
    fn whole_bracket_tags_are_removed() {
        for (title, want) in [
            ("Cardi B – Up (Official Music Video)", "Cardi B – Up"),
            ("Cardi B – Up [official music video]", "Cardi B – Up"),
            (
                "Aya Nakamura – Copines (Clip officiel)",
                "Aya Nakamura – Copines",
            ),
            ("Song [4K] (Official Video)", "Song"),
            ("Song (Official Video) ft. Sia", "Song ft. Sia"),
            ("Song(Official Video)ft. Sia", "Song ft. Sia"),
            ("Song （Official Video）", "Song"),
            ("Song ［MV］", "Song"),
            ("Song 【MV】", "Song"),
            ("Song (  Official   Video )", "Song"),
            (
                "Дискотека Авария – Фантазёр (Официальный клип)",
                "Дискотека Авария – Фантазёр",
            ),
            ("Hırkamdaki İz (Resmi Klip)", "Hırkamdaki İz"),
            // Sanitizing has turned the slash into ⧸ by the time an answer gets here.
            ("Artist – Song (Official M⧸V)", "Artist – Song"),
            ("Artist – Song (Official M/V)", "Artist – Song"),
            // An extension the tag hid goes too (#104 QA).
            ("Song.mp4 (Official Video)", "Song"),
            ("Song.MKV [4K] (MV)", "Song"),
        ] {
            assert_eq!(strip(title), want, "{title}");
        }
    }

    #[test]
    fn anything_but_an_exact_whole_bracket_stays() {
        for title in [
            "Song (Official Video Remix)",
            "Song (Live at Wembley - Official Video)",
            "&TEAM – Good Boy Official MV",
            "Artist – Song – Official MV",
            "LSD – Audio",
            "Dune (2021)",
            "Radiohead – Creep (Live at Glastonbury 1997)",
            "FOURTH – ไม่ไหวแล้ว (Officially)",
            "NewJeans – Hype Boy (Performance ver.1)",
            "Song (Official (Video))",
            "Song (Official Video",
            "Song Official Video)",
            "Song (Official Video]",
            "Song 「MV」",
            "Song ()",
            "Song (Trailer)",
        ] {
            assert_eq!(strip(title), title, "{title}");
        }
    }

    /// Phrases are compared in the form sanitizing gives an answer (#104 QA): a timestamp's
    /// colon becomes `_`, other illegal characters their fullwidth forms.
    #[test]
    fn phrases_match_the_sanitized_answer() {
        let tags = AdTags::new(["Promo 01:02", "Why?", "A/B"]);
        for title in [
            "Song (Promo 01_02)",
            "Song (Promo 01:02)",
            "Song (Why？)",
            "Song [a⧸b]",
        ] {
            assert_eq!(tags.strip(title), "Song", "{title}");
        }
        assert_eq!(tags.strip("Song (Promo 01-02)"), "Song (Promo 01-02)");
    }

    #[test]
    fn an_extension_left_without_a_tag_stays_for_the_answer_check() {
        // Nothing removed: the answer's own checks handle its ending.
        assert_eq!(strip("Song.mp4"), "Song.mp4");
        // Only a whole extension goes; "v1.5" isn't one.
        assert_eq!(strip("Patch v1.5 (Official Video)"), "Patch v1.5");
    }

    #[test]
    fn a_name_that_is_only_tags_stays_whole() {
        for title in ["(Official Video)", "[4K] (MV)", "Official Video"] {
            assert_eq!(strip(title), title);
        }
    }

    #[test]
    fn an_empty_list_removes_nothing() {
        let none = AdTags::new(Vec::<String>::new());
        assert_eq!(none.strip("Song (Official Video)"), "Song (Official Video)");
    }

    #[test]
    fn user_phrases_are_tidied() {
        let tags = AdTags::new([
            "  Official   Video ",
            "official video",
            "",
            "   ",
            &"x".repeat(MAX_CHARS + 1),
            &"y".repeat(MAX_CHARS),
        ]);
        assert_eq!(
            tags.list(),
            ["Official Video".to_string(), "y".repeat(MAX_CHARS)]
        );
        // Typed text loses one bracket pair around the whole of it; nothing else does.
        let typed = AdTags::new(Vec::<String>::new())
            .with_added("(Clip)")
            .with_added("【Visualizer】")
            .with_added("()")
            .with_added("(a) (b)")
            .with_added("((Twice))");
        assert_eq!(
            typed.list(),
            ["Clip", "Visualizer", "()", "(a) (b)", "(Twice)"]
        );
        assert_eq!(typed.strip("Song (CLIP) (b)"), "Song (b)");
        assert_eq!(typed.strip("Song ((Twice))"), "Song");
        let many = AdTags::new((0..MAX_TAGS + 5).map(|i| format!("tag {i}")));
        assert_eq!(many.list().len(), MAX_TAGS);
    }

    /// #104 review: each change names its phrase, never a position, and applies to the list as
    /// it is when it runs; a phrase that's gone by then changes nothing.
    #[test]
    fn changes_name_their_phrase_and_a_missing_one_changes_nothing() {
        let tags = AdTags::new(["MV", "4K", "Audio"]);
        let list = |t: &AdTags| t.list().to_vec();
        assert_eq!(list(&tags.without("mv")), ["4K", "Audio"]);
        assert_eq!(list(&tags.without(" 4k ")), ["MV", "Audio"]);
        assert_eq!(
            list(&tags.without("[4K]")),
            ["MV", "4K", "Audio"],
            "named as stored"
        );
        assert_eq!(list(&tags.without("Clip")), ["MV", "4K", "Audio"]);
        assert_eq!(
            list(&tags.with_edited("MV", "((Clip))")),
            ["(Clip)", "4K", "Audio"]
        );
        let edited = tags.with_edited("MV", "((Clip))");
        assert_eq!(list(&edited.without("MV")), ["(Clip)", "4K", "Audio"]);
        assert_eq!(
            list(&edited.without("(Clip)")),
            ["4K", "Audio"],
            "named as stored"
        );
        assert_eq!(list(&tags.with_edited("4K", "  ")), ["MV", "Audio"]);
        assert_eq!(
            list(&tags.with_edited("4K", "mv")),
            ["MV", "Audio"],
            "merges"
        );
        assert_eq!(
            list(&tags.with_edited("Gone", "Clip")),
            ["MV", "4K", "Audio"]
        );
        assert_eq!(
            list(&tags.with_added("(Clip)")),
            ["MV", "4K", "Audio", "Clip"]
        );
        assert_eq!(list(&tags.with_added("audio")), ["MV", "4K", "Audio"]);
        assert_eq!(list(&tags.with_added(" ")), ["MV", "4K", "Audio"]);
    }

    /// #104 review round 3: a stored bracketed phrase is found exactly as stored, never
    /// unwrapped again, and survives unrelated changes and a restart unchanged.
    #[test]
    fn a_stored_bracketed_phrase_is_kept_and_found_exactly() {
        let tags = AdTags::new(["Clip"]).with_added("((Clip))");
        assert_eq!(tags.list(), ["Clip", "(Clip)"]);
        assert_eq!(
            tags.without("(Clip)").list(),
            ["Clip"],
            "the other Clip stays"
        );
        assert_eq!(tags.with_edited("(Clip)", "MV").list(), ["Clip", "MV"]);
        let later = tags.with_added("4K").with_edited("Clip", "Clips");
        assert_eq!(later.list(), ["Clips", "(Clip)", "4K"]);
        let json = serde_json::to_string(&later).unwrap();
        assert_eq!(serde_json::from_str::<AdTags>(&json).unwrap(), later);
        assert_eq!(
            later.strip("Song (Clip)"),
            "Song (Clip)",
            "still only ((Clip))"
        );
        assert_eq!(later.strip("Song ((Clip))"), "Song");
    }

    #[test]
    fn the_defaults_are_tidy_and_distinct() {
        let defaults = AdTags::default();
        assert_eq!(defaults.list().len(), DEFAULTS.len());
        for (kept, given) in defaults.list().iter().zip(DEFAULTS) {
            assert_eq!(kept, given);
        }
    }

    #[test]
    fn saved_lists_round_trip_through_json() {
        let tags = AdTags::new(["Official Video", "(Clip)"]);
        let json = serde_json::to_string(&tags).unwrap();
        assert_eq!(json, r#"["Official Video","(Clip)"]"#, "kept exactly");
        let back: AdTags = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tags);
        let back: AdTags = serde_json::from_str(r#"["  a ", "A", "[b]"]"#).unwrap();
        assert_eq!(back.list(), ["a".to_string(), "[b]".into()]);
    }
}
