//! Link validation and duplicate keys (Q2, #5). Pure functions: no network, no filesystem.
use url::Url;

/// A link the queue accepts: `text` is what the user typed (trimmed, with `https://` added when
/// they left the scheme off) and is what yt-dlp gets; `key` identifies duplicates.
#[derive(Debug, PartialEq, Eq)]
pub struct Link {
    pub text: String,
    pub key: String,
}

/// Validates one pasted link. Only `http`/`https` with a host are accepted; a bare
/// `example.com/path` gets `https://` added.
pub fn parse(input: &str) -> Result<Link, String> {
    let input = input.trim();
    if input.chars().any(char::is_control) {
        return Err("That link contains invisible control characters.".into());
    }
    if input.chars().any(char::is_whitespace) {
        return Err("That isn't a link (it contains spaces).".into());
    }
    if input.contains('\\') {
        return Err("That isn't a valid link (it contains a backslash).".into());
    }
    // Let the URL parser decide whether there's a scheme. `mailto:x`, `javascript:x` and
    // `ftp://x` parse with their scheme and are rejected below; a bare `example.com/x` is a
    // relative URL (parse error) and gets `https://`. `example.com:8080/x` parses with the
    // "scheme" `example.com`, so a dotted scheme followed by a port number counts as a host.
    let text = match Url::parse(input) {
        Ok(url) if !looks_like_host_port(url.scheme(), input) => input.to_owned(),
        _ if looks_like_host(input) => format!("https://{input}"),
        Ok(_) => return Err("Only http and https links can be downloaded.".into()),
        Err(_) => return Err("That isn't a link. Paste a full address like https://…".into()),
    };
    let url = Url::parse(&text).map_err(|_| "That isn't a valid link.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http and https links can be downloaded.".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("That link has no website in it.".into());
    }
    // The text we keep is what yt-dlp gets, so it must already be in the form the parser would
    // produce: `https:example.test` or `https:/example.test` would be "repaired" here but not
    // by yt-dlp's Python parser.
    let lower = text.to_ascii_lowercase();
    let after_scheme = ["http://", "https://"]
        .iter()
        .find(|p| lower.starts_with(*p))
        .map(|p| &text[p.len()..]);
    // A host must follow `//` directly: `https:///example.test` is repaired by the URL parser
    // but has no host for yt-dlp's.
    if after_scheme.is_none_or(|rest| rest.is_empty() || rest.starts_with(['/', '?', '#'])) {
        return Err("That link is malformed. It should start with https:// and a website.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Links with a username or password aren't supported.".into());
    }
    let key = dedupe_key(&url);
    Ok(Link { text, key })
}

/// `example.com:8080/x` parses as scheme `example.com`; real schemes have no dots.
fn looks_like_host_port(scheme: &str, input: &str) -> bool {
    scheme.contains('.')
        && input[scheme.len() + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
}

/// `example.com`, `www.example.com/v`, `例え.jp/x`: a dot inside the part before the first `/`.
fn looks_like_host(input: &str) -> bool {
    let host = input.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
}

/// The identity used to reject duplicates. YouTube links collapse to the video id. Other links
/// keep their query exactly (same parameters, order, and empty `&&` pieces: an unknown site may
/// care), ignoring
/// only the scheme, `www.`, default port, fragment, trailing slash, `utm_*` parameters, and
/// spellings of the same bytes (`%3d`/`%3D`, `%7e`/`~`). `+` stays literal: only form encoding
/// treats it as a space, and an unknown site may not. A missed duplicate is better than a
/// distinct link rejected as one.
pub fn dedupe_key(url: &Url) -> String {
    if let Some(id) = youtube_id(url) {
        return format!("youtube:{id}");
    }
    let host = url.host_str().unwrap_or("").trim_start_matches("www.");
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let path = url.path().trim_end_matches('/');
    let query = url
        .query()
        .unwrap_or("")
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (canonical(name), Some(canonical(value))),
            None => (canonical(pair), None),
        })
        .filter(|(name, _)| !name.starts_with("utm_"))
        .map(|(name, value)| match value {
            Some(value) => format!("{name}={value}"),
            None => name,
        })
        .collect::<Vec<_>>()
        .join("&");
    if query.is_empty() {
        format!("{host}{port}{path}")
    } else {
        format!("{host}{port}{path}?{query}")
    }
}

/// One query name or value in RFC 3986 normal form: `%XX` escapes of unreserved characters
/// (`A-Z a-z 0-9 - . _ ~`) are decoded, every other escape is upper-cased, and literal
/// characters are kept as they are. So `%3d` = `%3D` and `%7e` = `~`, but `+`, `%2B` and `%20`
/// stay distinct, and so do `%FF`, `%FE` and `%EF%BF%BD`.
fn canonical(part: &str) -> String {
    let bytes = part.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = String::with_capacity(part.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            let b = h * 16 + l;
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
            i += 3;
            continue;
        }
        // `url::Url::query()` is ASCII (non-ASCII is already percent-encoded), so this is exact.
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// The video id of a single YouTube video link, if it is one.
fn youtube_id(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    let host = host.trim_start_matches("www.").trim_start_matches("m.");
    let id = match host {
        "youtu.be" => url.path_segments()?.next().map(str::to_owned),
        "youtube.com" | "music.youtube.com" | "youtube-nocookie.com" => {
            let mut segments = url.path_segments()?;
            match segments.next()? {
                "watch" => url
                    .query_pairs()
                    .find(|(k, _)| k == "v")
                    .map(|(_, v)| v.into_owned()),
                "shorts" | "live" | "embed" | "v" => segments.next().map(str::to_owned),
                _ => None,
            }
        }
        _ => None,
    }?;
    // YouTube ids are 11 chars of [A-Za-z0-9_-]; anything else isn't a single video.
    (id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(input: &str) -> String {
        parse(input).unwrap().key
    }

    #[test]
    fn accepts_http_and_https_links_and_keeps_the_typed_text() {
        let link = parse("  https://www.youtube.com/watch?v=dQw4w9WgXcQ  ").unwrap();
        assert_eq!(link.text, "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(
            parse("http://example.com/a").unwrap().text,
            "http://example.com/a"
        );
        assert_eq!(
            parse("https://例え.jp/🎵").unwrap().text,
            "https://例え.jp/🎵",
            "not punycoded"
        );
    }

    #[test]
    fn adds_https_to_bare_domains() {
        assert_eq!(
            parse("vimeo.com/76979871").unwrap().text,
            "https://vimeo.com/76979871"
        );
        assert_eq!(parse("example.com").unwrap().text, "https://example.com");
        assert_eq!(
            parse("localhost.test:8080/x").unwrap().text,
            "https://localhost.test:8080/x"
        );
    }

    #[test]
    fn rejects_non_links_other_schemes_and_hostless_urls() {
        for bad in [
            "not a url",
            "hello",
            "-f bestvideo",
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "ftp://example.com/file",
            "file:///etc/passwd",
            "https://",
            "https:// example.com",
            ".com",
            "a.",
            "mailto:user@example.test",
            "data:text/html,hi",
            "blob:https://example.com/uuid",
            "https://example.test/video\0suffix",
            "https://example.test/\u{7}bell",
            "https:example.test/video",
            "https:/example.test/video",
            "https://example.test\\@other.test/video",
            "https://example.test\\video",
            "https://user:pass@example.test/video",
            "https://user@example.test/video",
            "https:///example.test/video",
            "http:////example.test/video",
            "https://?q=1",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn youtube_variants_of_one_video_share_a_key() {
        let k = "youtube:dQw4w9WgXcQ";
        for link in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
            "https://m.youtube.com/watch?feature=share&v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?si=abc123",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RD123",
            "youtu.be/dQw4w9WgXcQ",
            "HTTPS://WWW.YOUTUBE.COM/watch?v=dQw4w9WgXcQ",
        ] {
            assert_eq!(key(link), k, "{link}");
        }
        assert_ne!(key("https://youtu.be/aaaaaaaaaaa"), k, "different video");
    }

    #[test]
    fn generic_keys_ignore_cosmetic_differences_but_not_real_ones() {
        let base = key("https://example.com/video/1");
        for same in [
            "http://example.com/video/1",
            "https://www.example.com/video/1/",
            "https://EXAMPLE.com/video/1#comments",
            "https://example.com:443/video/1",
            "https://example.com/video/1?utm_source=x&utm_medium=y",
            "example.com/video/1",
        ] {
            assert_eq!(key(same), base, "{same}");
        }
        assert_ne!(key("https://example.com/video/2"), base);
        assert_ne!(
            key("https://example.com/Video/1"),
            base,
            "paths are case-sensitive"
        );
        assert_ne!(key("https://example.com/video/1?page=2"), base);
        assert_ne!(
            key("https://e.com/v?b=2&a=1"),
            key("https://e.com/v?a=1&b=2"),
            "order is kept: an unknown site may care"
        );
        assert_ne!(
            key("https://example.com:8080/video/1"),
            base,
            "non-default port"
        );
    }

    #[test]
    fn unknown_sites_keep_content_parameters_order_and_exact_bytes() {
        // #5 review round 2: these are different downloads and must all be addable.
        assert_ne!(
            key("https://e.test/v?feature=episode1"),
            key("https://e.test/v?feature=episode2")
        );
        assert_ne!(
            key("https://e.test/v?si=1"),
            key("https://e.test/v"),
            "si only ignored on YouTube"
        );
        assert_ne!(
            key("https://e.test/v?id=1&id=2"),
            key("https://e.test/v?id=2&id=1")
        );
        for spaced in ["?id=1&&part=2", "?&id=1&part=2", "?id=1&part=2&"] {
            assert_ne!(
                key(&format!("https://e.test/v{spaced}")),
                key("https://e.test/v?id=1&part=2"),
                "empty query pieces are kept: {spaced}"
            );
        }
        let bytes =
            ["%FF", "%FE", "%EF%BF%BD"].map(|t| key(&format!("https://e.test/v?token={t}")));
        assert_ne!(bytes[0], bytes[1]);
        assert_ne!(bytes[0], bytes[2]);
        assert_ne!(bytes[1], bytes[2]);
        // Same bytes, different spelling: still one link.
        assert_eq!(
            key("https://e.test/v?q=hello%20world&x=%26%3D"),
            key("https://e.test/v?q=hello%20world&x=%26%3d")
        );
        assert_ne!(
            key("https://e.test/v?file=A+B.mp4"),
            key("https://e.test/v?file=A%20B.mp4"),
            "+ is literal"
        );
        assert_eq!(key("https://e.test/v?a=%7e"), key("https://e.test/v?a=~"));
        assert_ne!(
            key("https://e.test/v?file=A+B.mp4"),
            key("https://e.test/v?file=A%2BB.mp4")
        );
        assert_ne!(
            key("https://e.test/v?A+B=1"),
            key("https://e.test/v?A%2BB=1"),
            "names too"
        );
        assert_eq!(
            key("https://e.test/v?file=A%2bB"),
            key("https://e.test/v?file=A%2BB")
        );
        assert_ne!(
            key("https://e.test/v?file=A%2BB"),
            key("https://e.test/v?file=A%20B")
        );
    }

    #[test]
    fn a_playlist_or_channel_url_is_not_mistaken_for_a_video() {
        assert!(
            key("https://www.youtube.com/playlist?list=PL123").starts_with("youtube.com/playlist")
        );
        assert!(key("https://www.youtube.com/@channel").starts_with("youtube.com/@channel"));
        assert!(!key("https://www.youtube.com/watch?v=short").starts_with("youtube:"));
    }
}
