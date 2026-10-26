use std::sync::OnceLock;

use regex::Regex;

use super::LlmError;

/// A request's credential values are distinct from its URL display policy.
#[derive(Default)]
pub(crate) struct ErrorRedactor {
    secrets: Vec<String>,
    urls: Vec<String>,
}

impl ErrorRedactor {
    pub(crate) fn new(url: &str, api_key: &str) -> Self {
        let mut redactor = Self::default();
        redactor.add(api_key);
        redactor.add_url(url);
        redactor
    }

    pub(crate) fn add_url(&mut self, url: &str) {
        if !url.is_empty() && !self.urls.iter().any(|known| known == url) {
            self.urls.push(url.to_string());
        }
        if let Some((start, end)) = authority_bounds(url) {
            if let Some((userinfo, _)) = url[start..end].rsplit_once('@') {
                if let Some((_, password)) = userinfo.split_once(':') {
                    self.add(&Decoded::new(password, true, false).text);
                }
            }
        }
        let without_fragment = url.split('#').next().unwrap_or(url);
        if let Some((_, query)) = without_fragment.split_once('?') {
            for pair in query.split('&') {
                if let Some((name, value)) = pair.split_once('=') {
                    let name = Decoded::new(&name.replace('+', " "), true, false)
                        .text
                        .to_ascii_lowercase();
                    if [
                        "key",
                        "token",
                        "secret",
                        "password",
                        "passwd",
                        "auth",
                        "sig",
                        "credential",
                    ]
                    .iter()
                    .any(|part| name.contains(part))
                    {
                        self.add(&Decoded::new(&value.replace('+', " "), true, false).text);
                    }
                }
            }
        }
    }

    fn add(&mut self, value: &str) {
        if !value.is_empty() && !self.secrets.iter().any(|known| known == value) {
            self.secrets.push(value.to_string());
        }
    }

    pub(crate) fn headers(&mut self, headers: &reqwest::header::HeaderMap) {
        for (name, value) in headers {
            if Self::is_auth_header(name.as_str()) {
                if let Ok(value) = value.to_str() {
                    self.add(value);
                    if let Some(token) = value.strip_prefix("Bearer ") {
                        self.add(token);
                    }
                }
            }
        }
    }

    fn is_auth_header(name: &str) -> bool {
        matches!(
            name,
            "x-api-key" | "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
        )
    }

    /// Replace only credential values, preserving the spelling of all other text.
    pub(crate) fn text(&self, text: &str) -> String {
        let decoded = Decoded::new(text, true, true);
        let mut ranges = Vec::new();
        for secret in &self.secrets {
            for (start, _) in text.match_indices(secret) {
                ranges.push((start, start + secret.len()));
            }
            for (start, _) in decoded.text.match_indices(secret) {
                ranges.push(decoded.original_range(start, start + secret.len()));
            }
        }
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in ranges {
            if let Some(last) = merged.last_mut() {
                if start <= last.1 {
                    last.1 = last.1.max(end);
                    continue;
                }
            }
            merged.push((start, end));
        }
        replace_ranges(
            text,
            &merged
                .into_iter()
                .map(|(s, e)| (s, e, "[REDACTED]"))
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn url(&self, url: &str) -> String {
        match url_edits(url) {
            Some(edits) => replace_ranges(url, &edits),
            None => "[invalid URL]".to_string(),
        }
    }

    /// Protect URL authority/path fields from credential substring replacement.
    pub(crate) fn diagnostic(&self, text: &str) -> String {
        // Decode JSON syntax for URL recognition, but leave percent escapes intact:
        // an escaped '?' or '/' in a URL path is not a structural delimiter.
        let view = Decoded::new(text, false, true);
        let mut urls = Vec::new();
        for known in &self.urls {
            for (start, _) in view.text.match_indices(known) {
                urls.push((start, start + known.len()));
            }
        }
        static URL_RE: OnceLock<Regex> = OnceLock::new();
        for matched in URL_RE
            .get_or_init(|| {
                Regex::new(r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s<>"']+"#).expect("valid URL regex")
            })
            .find_iter(&view.text)
        {
            let candidate = matched
                .as_str()
                .trim_end_matches([')', ']', '}', ',', '.', ';']);
            urls.push((matched.start(), matched.start() + candidate.len()));
        }
        urls.sort_by_key(|&(start, end)| (start, std::cmp::Reverse(end)));
        let mut output = String::new();
        let mut cursor = 0;
        for (start, end) in urls {
            let (original_start, original_end) = view.original_range(start, end);
            if original_start < cursor {
                continue;
            }
            output.push_str(&self.text(&text[cursor..original_start]));
            let candidate = &view.text[start..end];
            if let Some(edits) = url_edits(candidate) {
                let edits: Vec<_> = edits
                    .into_iter()
                    .map(|(s, e, replacement)| {
                        let (s, e) = view.original_range(start + s, start + e);
                        (s - original_start, e - original_start, replacement)
                    })
                    .collect();
                output.push_str(&replace_ranges(&text[original_start..original_end], &edits));
            } else {
                output.push_str("[invalid URL]");
            }
            cursor = original_end;
        }
        output.push_str(&self.text(&text[cursor..]));
        output
    }

    pub(crate) fn error_result<T>(&self, result: Result<T, LlmError>) -> Result<T, LlmError> {
        result.map_err(|error| self.error(error))
    }

    pub(crate) fn error(&self, mut error: LlmError) -> LlmError {
        let message = match &mut error {
            LlmError::RequestFailed { message }
            | LlmError::HttpError(message)
            | LlmError::ApiError(message)
            | LlmError::ParseError(message)
            | LlmError::RateLimited { message, .. }
            | LlmError::AuthError { message }
            | LlmError::ModelNotFound { model: message }
            | LlmError::ProviderNotFound(message)
            | LlmError::ConfigError(message)
            | LlmError::ContextLengthExceeded { message }
            | LlmError::StreamError { message }
            | LlmError::IoError(message) => message,
            LlmError::Timeout | LlmError::Cancelled => return error,
        };
        *message = self.diagnostic(message);
        error
    }
}

fn authority_bounds(url: &str) -> Option<(usize, usize)> {
    let start = if let Some((scheme, _)) = url.split_once("://") {
        scheme.len() + 3
    } else if url.starts_with("//") {
        2
    } else {
        return None;
    };
    let end = url[start..]
        .find(['/', '?', '#', '\\'])
        .map(|n| start + n)
        .unwrap_or(url.len());
    Some((start, end))
}

/// Lexical component boundaries also work for builder errors such as invalid ports.
fn url_edits(url: &str) -> Option<Vec<(usize, usize, &'static str)>> {
    let (authority_start, authority_end) = authority_bounds(url)?;
    let mut edits = Vec::new();
    if let Some(at) = url[authority_start..authority_end].rfind('@') {
        edits.push((authority_start, authority_start + at + 1, ""));
    }
    let fragment = url.find('#').unwrap_or(url.len());
    if let Some(query) = url[..fragment].find('?') {
        let mut start = query + 1;
        for pair in url[start..fragment].split('&') {
            if let Some(eq) = pair.find('=') {
                edits.push((start + eq + 1, start + pair.len(), "[REDACTED]"));
            }
            start += pair.len() + 1;
        }
    }
    if fragment < url.len() {
        edits.push((fragment + 1, url.len(), "[REDACTED]"));
    }
    Some(edits)
}

fn replace_ranges(text: &str, edits: &[(usize, usize, &str)]) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    for &(start, end, replacement) in edits {
        output.push_str(&text[cursor..start]);
        output.push_str(replacement);
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    output
}

/// Decoding is a matching view only; byte spans retain the untouched input spelling.
struct Decoded {
    text: String,
    spans: Vec<(usize, usize)>,
}

impl Decoded {
    fn new(input: &str, percent: bool, json: bool) -> Self {
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut start = 0;
        while start < input.len() {
            let rest = &input[start..];
            let decoded = if percent { percent_char(rest) } else { None }.or_else(|| {
                if json {
                    json_char(rest)
                } else {
                    None
                }
            });
            let (ch, len) = decoded.unwrap_or_else(|| {
                let ch = rest.chars().next().expect("nonempty remainder");
                (ch, ch.len_utf8())
            });
            text.push(ch);
            spans.extend(std::iter::repeat_n((start, start + len), ch.len_utf8()));
            start += len;
        }
        Self { text, spans }
    }

    fn original_range(&self, start: usize, end: usize) -> (usize, usize) {
        let original_start = self
            .spans
            .get(start)
            .map(|span| span.0)
            .unwrap_or_else(|| self.spans.last().map(|span| span.1).unwrap_or(0));
        let original_end = if start == end {
            original_start
        } else {
            self.spans[end - 1].1
        };
        (original_start, original_end)
    }
}

fn percent_byte(input: &[u8]) -> Option<u8> {
    if input.len() < 3 || input[0] != b'%' {
        return None;
    }
    let hi = (input[1] as char).to_digit(16)?;
    let lo = (input[2] as char).to_digit(16)?;
    Some((hi * 16 + lo) as u8)
}

fn percent_char(input: &str) -> Option<(char, usize)> {
    let bytes = input.as_bytes();
    let first = percent_byte(bytes)?;
    let count = match first {
        0..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    let mut decoded = Vec::with_capacity(count);
    for i in 0..count {
        decoded.push(percent_byte(bytes.get(i * 3..)?)?);
    }
    Some((
        std::str::from_utf8(&decoded).ok()?.chars().next()?,
        count * 3,
    ))
}

fn json_char(input: &str) -> Option<(char, usize)> {
    let bytes = input.as_bytes();
    if bytes.first() != Some(&b'\\') {
        return None;
    }
    let ch = match *bytes.get(1)? {
        b'"' => '"',
        b'\\' => '\\',
        b'/' => '/',
        b'b' => '\u{0008}',
        b'f' => '\u{000c}',
        b'n' => '\n',
        b'r' => '\r',
        b't' => '\t',
        b'u' => {
            let first = u16::from_str_radix(input.get(2..6)?, 16).ok()?;
            if (0xd800..=0xdbff).contains(&first) {
                if input.get(6..8)? != "\\u" {
                    return None;
                }
                let second = u16::from_str_radix(input.get(8..12)?, 16).ok()?;
                return Some((char::decode_utf16([first, second]).next()?.ok()?, 12));
            }
            return Some((char::from_u32(first as u32)?, 6));
        }
        _ => return None,
    };
    Some((ch, 2))
}
