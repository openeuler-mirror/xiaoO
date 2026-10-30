use std::borrow::Cow;
use std::sync::OnceLock;

use super::LlmError;

/// A request's credential values are distinct from its URL display policy.
#[derive(Default)]
pub(crate) struct ErrorRedactor {
    spellings: Vec<(String, bool)>,
    urls: Vec<(String, String, bool)>,
    secret_matcher: OnceLock<Result<regex::Regex, regex::Error>>,
    url_matcher: OnceLock<Result<regex::Regex, regex::Error>>,
}

impl ErrorRedactor {
    pub(crate) fn new(url: &str, api_key: &str) -> Self {
        let mut redactor = Self::default();
        redactor.add(api_key.as_bytes());
        redactor.add_url(url);
        redactor
    }

    pub(crate) fn add_url(&mut self, url: &str) {
        self.remember_url(url);
        // reqwest normalizes the URL and extracts userinfo into a Basic header.
        if let Ok(mut parsed) = reqwest::Url::parse(url) {
            self.remember_url(parsed.as_str());
            let _ = parsed.set_password(None);
            let _ = parsed.set_username("");
            self.remember_url(parsed.as_str());
        }
        if let Some((start, end)) = authority_bounds(url) {
            if let Some((userinfo, _)) = url[start..end].rsplit_once('@') {
                if let Some((_, password)) = userinfo.split_once(':') {
                    self.add(&decode_url_value(password));
                }
            }
        }
        let without_fragment = url.split('#').next().unwrap_or(url);
        if let Some((_, query)) = without_fragment.split_once('?') {
            for pair in query.split('&') {
                if let Some((name, value)) = pair.split_once('=') {
                    let name = String::from_utf8_lossy(&decode_url_value(&name.replace('+', " ")))
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
                        self.add(&decode_url_value(&value.replace('+', " ")));
                    }
                }
            }
        }
    }

    fn remember_url(&mut self, url: &str) {
        if url.is_empty() {
            return;
        }
        for (spelling, encoded) in [(url.to_string(), false), (json_spelling(url), true)] {
            if !self.urls.iter().any(|(known, _, _)| known == &spelling) {
                self.urls.push((spelling, url.to_string(), encoded));
                self.url_matcher.take();
            }
        }
    }

    fn add(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let lossy = String::from_utf8_lossy(bytes);
        let json = json_spelling(&lossy);
        let boundary = json.starts_with('\\');
        let mut spellings = vec![(lossy.to_string(), false), (json, boundary)];
        for value in [bytes, lossy.as_bytes()] {
            spellings.push((percent_spelling(value, false), false));
            spellings.push((percent_spelling(value, true), false));
        }
        for (spelling, boundary) in spellings {
            if let Some((_, known_boundary)) = self
                .spellings
                .iter_mut()
                .find(|(known, _)| known == &spelling)
            {
                // An exact raw credential can also be another credential's JSON spelling.
                *known_boundary &= boundary;
            } else {
                self.spellings.push((spelling, boundary));
                self.secret_matcher.take();
            }
        }
    }

    pub(crate) fn headers(&mut self, headers: &reqwest::header::HeaderMap) {
        for (name, value) in headers {
            if Self::is_auth_header(name.as_str()) {
                self.add(value.as_bytes());
                if let Some(token) = value.as_bytes().strip_prefix(b"Bearer ") {
                    self.add(token);
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

    /// Match only literal spellings produced by the supported encoders.
    pub(crate) fn text<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.spellings.is_empty() {
            return Cow::Borrowed(text);
        }
        let Ok(matcher) = self
            .secret_matcher
            .get_or_init(|| matcher(self.spellings.iter().map(|(spelling, _)| spelling.as_str())))
        else {
            // A credential set too large for the matcher must not be exposed.
            return Cow::Borrowed("[REDACTED]");
        };
        let mut boundary = EscapeBoundary::default();
        let mut output = None;
        let mut copied = 0;
        let mut search = 0;
        while let Some(found) = matcher.find_at(text, search) {
            let guarded = self
                .spellings
                .iter()
                .any(|(spelling, guarded)| *guarded && spelling == found.as_str());
            let end = if guarded && !boundary.allows(text, found.start()) {
                // Rejecting a JSON spelling must not hide a raw spelling at the same start.
                let alternative = self
                    .spellings
                    .iter()
                    .filter(|(spelling, guarded)| {
                        !guarded && text[found.start()..].starts_with(spelling)
                    })
                    .map(|(spelling, _)| spelling.len())
                    .max();
                if let Some(len) = alternative {
                    found.start() + len
                } else {
                    // Other credentials may still begin inside the rejected spelling.
                    search = found.start() + 1;
                    continue;
                }
            } else {
                found.end()
            };
            let result = output.get_or_insert_with(|| String::with_capacity(text.len()));
            result.push_str(&text[copied..found.start()]);
            result.push_str("[REDACTED]");
            copied = end;
            search = copied;
        }
        if let Some(mut output) = output {
            output.push_str(&text[copied..]);
            Cow::Owned(output)
        } else {
            Cow::Borrowed(text)
        }
    }

    pub(crate) fn url(&self, url: &str) -> String {
        url_edits(url)
            .and_then(|edits| replace_ranges(url, &edits))
            .unwrap_or_else(|| "[invalid URL]".to_string())
    }

    /// Only registered request URLs receive structural display treatment.
    /// Every other link remains ordinary text subject to credential replacement.
    pub(crate) fn diagnostic<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self
            .url_matcher
            .get_or_init(|| matcher(self.urls.iter().map(|(spelling, _, _)| spelling.as_str())))
            .is_err()
        {
            return Cow::Borrowed("[invalid URL]");
        }
        let mut output = None;
        let mut cursor = 0;
        while let Some((start, end, known, encoded)) = self.next_request_url(&text[cursor..]) {
            let result = output.get_or_insert_with(|| String::with_capacity(text.len()));
            result.push_str(&self.text(&text[cursor..cursor + start]));
            result.push_str(&request_url_display(known, encoded));
            cursor += end;
        }
        let tail = self.text(&text[cursor..]);
        if let Some(mut output) = output {
            output.push_str(&tail);
            Cow::Owned(output)
        } else {
            tail
        }
    }

    fn next_request_url<'a>(&'a self, text: &str) -> Option<(usize, usize, &'a str, bool)> {
        if self.urls.is_empty() {
            return None;
        }
        let matcher = self
            .url_matcher
            .get_or_init(|| matcher(self.urls.iter().map(|(spelling, _, _)| spelling.as_str())))
            .as_ref()
            .ok()?;
        let mut boundary = UrlBoundary::default();
        for found in matcher.find_iter(text) {
            let (start, end) = (found.start(), found.end());
            // A prefix or a value inside another URL remains free text.
            if boundary.allows(text, start) && url_right_boundary(&text[end..]) {
                let (_, known, encoded) = self
                    .urls
                    .iter()
                    .find(|(spelling, _, _)| spelling == found.as_str())?;
                return Some((start, end, known, *encoded));
            }
        }
        None
    }
    pub(crate) fn error_result<T>(&self, result: Result<T, LlmError>) -> Result<T, LlmError> {
        result.map_err(|error| self.error(error))
    }

    pub(crate) fn error(&self, mut error: LlmError) -> LlmError {
        let message = match &mut error {
            LlmError::RequestFailed { message }
            | LlmError::RetryExhausted { message, .. }
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
        *message = self.diagnostic(message).into_owned();
        error
    }
}

/// The regex engine scans all literal alternatives together in O(m * n),
/// where m is the fixed credential/URL pattern size and n is the text length.
fn matcher<'a>(spellings: impl Iterator<Item = &'a str>) -> Result<regex::Regex, regex::Error> {
    let mut spellings: Vec<_> = spellings.collect();
    spellings.sort_by_key(|spelling| std::cmp::Reverse(spelling.len()));
    let pattern = spellings
        .into_iter()
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join("|");
    regex::Regex::new(&pattern)
}

fn json_spelling(text: &str) -> String {
    let encoded = serde_json::to_string(text).expect("string serialization cannot fail");
    encoded[1..encoded.len() - 1].to_string()
}

/// RFC 3986 percent encoding keeps only the unreserved bytes literal.
fn percent_spelling(bytes: &[u8], upper: bool) -> String {
    let hex = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut output = String::new();
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push('%');
            output.push(hex[(byte >> 4) as usize] as char);
            output.push(hex[(byte & 15) as usize] as char);
        }
    }
    output
}

/// Advance monotonically so a long run of backslashes is never rescanned.
#[derive(Default)]
struct EscapeBoundary {
    position: usize,
    odd: bool,
}

impl EscapeBoundary {
    fn allows(&mut self, text: &str, end: usize) -> bool {
        for byte in text.as_bytes()[self.position..end].iter().copied() {
            self.odd = byte == b'\\' && !self.odd;
        }
        self.position = end;
        !self.odd
    }
}

fn url_separator(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | '<' | '"' | '\'' | '`')
}

/// Track only the current token's context, once per character before a candidate.
#[derive(Default)]
struct UrlBoundary {
    position: usize,
    token_start: usize,
    has_scheme: bool,
    last: Option<char>,
}

impl UrlBoundary {
    fn allows(&mut self, text: &str, end: usize) -> bool {
        for (offset, ch) in text[self.position..end].char_indices() {
            let offset = self.position + offset;
            if url_separator(ch) {
                self.token_start = offset + ch.len_utf8();
                self.has_scheme = false;
            } else if ch == '/'
                && offset >= 2
                && text.as_bytes().get(offset - 2..offset + 1) == Some(b"://")
            {
                self.has_scheme = true;
            }
            self.last = Some(ch);
        }
        self.position = end;
        match self.last {
            None => true,
            Some(ch) if url_separator(ch) => true,
            Some('=' | ':') => !self.has_scheme && !text[self.token_start..end].starts_with("//"),
            _ => false,
        }
    }
}

fn url_right_boundary(suffix: &str) -> bool {
    if suffix.starts_with(r#"\""#) {
        return true;
    }
    suffix.chars().next().is_none_or(|ch| {
        ch.is_whitespace() || matches!(ch, ')' | ']' | '}' | '>' | '"' | '\'' | '`' | ',' | ';')
    })
}

fn authority_bounds(url: &str) -> Option<(usize, usize)> {
    let start = if url.starts_with("//") {
        2
    } else {
        let (scheme, _) = url.split_once("://")?;
        if !scheme.as_bytes().first()?.is_ascii_alphabetic()
            || !scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        {
            return None;
        }
        scheme.len() + 3
    };
    let end = url[start..]
        .find(['/', '?', '#', '\\'])
        .map(|n| start + n)
        .unwrap_or(url.len());
    if start == end {
        return None;
    }
    Some((start, end))
}

/// Lexical boundaries work even when URL parsing rejects a port or hostname.
fn url_edits(url: &str) -> Option<Vec<(usize, usize, &'static str)>> {
    let (start, end) = authority_bounds(url)?;
    let mut edits = Vec::new();
    if let Some(at) = url[start..end].rfind('@') {
        edits.push((start, start + at + 1, ""));
    }
    let fragment = url[end..].find('#').map(|n| end + n).unwrap_or(url.len());
    if let Some(query) = url[end..fragment].find('?') {
        let mut start = end + query + 1;
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

fn replace_ranges(text: &str, edits: &[(usize, usize, &str)]) -> Option<String> {
    let mut output = String::new();
    let mut cursor = 0;
    for &(start, end, replacement) in edits {
        if start < cursor || end < start {
            return None;
        }
        output.push_str(text.get(cursor..start)?);
        text.get(start..end)?;
        output.push_str(replacement);
        cursor = end;
    }
    output.push_str(text.get(cursor..)?);
    Some(output)
}

fn request_url_display(known: &str, encoded: bool) -> String {
    let display = url_edits(known)
        .and_then(|edits| replace_ranges(known, &edits))
        .unwrap_or_else(|| "[invalid URL]".to_string());
    if encoded {
        json_spelling(&display)
    } else {
        display
    }
}

/// Decode percent escapes only when collecting values from the request URL.
/// Diagnostic text is never decoded.
fn decode_url_value(text: &str) -> Vec<u8> {
    let mut output = Vec::with_capacity(text.len());
    let mut bytes = text.as_bytes().iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let mut following = bytes.clone();
            if let (Some(hi), Some(lo)) = (following.next(), following.next()) {
                if let (Some(hi), Some(lo)) = ((hi as char).to_digit(16), (lo as char).to_digit(16))
                {
                    output.push((hi * 16 + lo) as u8);
                    bytes = following;
                    continue;
                }
            }
        }
        output.push(byte);
    }
    output
}
