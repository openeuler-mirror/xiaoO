use std::borrow::Cow;

use super::LlmError;

/// A request's credential values are distinct from its URL display policy.
#[derive(Default)]
pub(crate) struct ErrorRedactor {
    secrets: Vec<Vec<u8>>,
    urls: Vec<String>,
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
                    self.add(&decode(password, true, false));
                }
            }
        }
        let without_fragment = url.split('#').next().unwrap_or(url);
        if let Some((_, query)) = without_fragment.split_once('?') {
            for pair in query.split('&') {
                if let Some((name, value)) = pair.split_once('=') {
                    let name =
                        String::from_utf8_lossy(&decode(&name.replace('+', " "), true, false))
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
                        self.add(&decode(&value.replace('+', " "), true, false));
                    }
                }
            }
        }
    }

    fn remember_url(&mut self, url: &str) {
        if !url.is_empty() && !self.urls.iter().any(|known| known == url) {
            self.urls.push(url.to_string());
        }
    }

    fn add(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if !self.secrets.iter().any(|known| known == bytes) {
            self.secrets.push(bytes.to_vec());
        }
        // HTTP text decoding can replace non-UTF-8 header bytes with U+FFFD.
        let lossy = String::from_utf8_lossy(bytes);
        if !self.secrets.iter().any(|known| known == lossy.as_bytes()) {
            self.secrets.push(lossy.as_bytes().to_vec());
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

    /// Decode only while matching; no body-sized position table or match list.
    pub(crate) fn text<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let escaped = text.contains('%') || text.contains('\\');
        let mut output = None;
        let mut cursor = 0;
        while let Some((start, end)) = self.next_secret(&text[cursor..], escaped) {
            let result = output.get_or_insert_with(|| String::with_capacity(text.len()));
            result.push_str(&text[cursor..cursor + start]);
            result.push_str("[REDACTED]");
            cursor += end;
        }
        if let Some(mut output) = output {
            output.push_str(&text[cursor..]);
            Cow::Owned(output)
        } else {
            Cow::Borrowed(text)
        }
    }

    fn next_secret(&self, text: &str, escaped: bool) -> Option<(usize, usize)> {
        if !escaped {
            return self
                .secrets
                .iter()
                .filter_map(|secret| {
                    let secret = std::str::from_utf8(secret).ok()?;
                    text.find(secret).map(|start| (start, start + secret.len()))
                })
                .min_by_key(|&(start, end)| (start, std::cmp::Reverse(end)));
        }
        let prefixes: Vec<_> = self
            .secrets
            .iter()
            .flat_map(|secret| first_spellings(secret, true))
            .collect();
        let mut cursor = 0;
        while let Some(start) = find_first(&text[cursor..], &prefixes) {
            let start = cursor + start;
            if let Some(len) = self
                .secrets
                .iter()
                .filter_map(|secret| credential_match(&text[start..], secret))
                .max()
            {
                return Some((start, start + len));
            }
            cursor = start
                + text[start..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(1);
        }
        None
    }

    pub(crate) fn url(&self, url: &str) -> String {
        url_edits(url)
            .and_then(|edits| replace_ranges(url, &edits))
            .unwrap_or_else(|| "[invalid URL]".to_string())
    }

    /// Only registered request URLs receive structural display treatment.
    /// Every other link remains ordinary text subject to credential replacement.
    pub(crate) fn diagnostic<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut output = None;
        let mut cursor = 0;
        while let Some((start, end, known)) = self.next_request_url(&text[cursor..]) {
            let result = output.get_or_insert_with(|| String::with_capacity(text.len()));
            result.push_str(&self.text(&text[cursor..cursor + start]));
            result.push_str(&request_url_display(
                &text[cursor + start..cursor + end],
                known,
            ));
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

    fn next_request_url<'a>(&'a self, text: &str) -> Option<(usize, usize, &'a str)> {
        let escaped = text.contains('\\');
        let mut best = None;
        for known in &self.urls {
            let mut cursor = 0;
            while cursor < text.len() {
                let found = if escaped {
                    let prefixes = first_spellings(known.as_bytes(), false);
                    let mut position = cursor;
                    let mut found = None;
                    while let Some(start) = find_first(&text[position..], &prefixes) {
                        let start = position + start;
                        if let Some(len) = literal_match(&text[start..], known.as_bytes())
                            .into_iter()
                            .chain(match_bytes(&text[start..], known.as_bytes(), false))
                            .max()
                        {
                            found = Some((start, start + len));
                            break;
                        }
                        position = start
                            + text[start..]
                                .chars()
                                .next()
                                .map(char::len_utf8)
                                .unwrap_or(1);
                    }
                    found
                } else {
                    text[cursor..]
                        .find(known)
                        .map(|start| (cursor + start, cursor + start + known.len()))
                };
                let Some((start, end)) = found else {
                    break;
                };
                // Do not protect a request URL used merely as another URL's prefix.
                if url_left_boundary(&text[..start]) && url_right_boundary(&text[end..]) {
                    let candidate = (start, end, known.as_str());
                    if best.as_ref().is_none_or(|&(s, e, _)| {
                        (start, std::cmp::Reverse(end)) < (s, std::cmp::Reverse(e))
                    }) {
                        best = Some(candidate);
                    }
                    break;
                }
                cursor = start
                    + text[start..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(1);
            }
        }
        best
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
        *message = self.diagnostic(message).into_owned();
        error
    }
}

fn find_first(text: &str, prefixes: &[String]) -> Option<usize> {
    prefixes.iter().filter_map(|prefix| text.find(prefix)).min()
}

fn first_spellings(bytes: &[u8], percent: bool) -> Vec<String> {
    let Some(&first) = bytes.first() else {
        return Vec::new();
    };
    let mut prefixes = Vec::new();
    if percent {
        prefixes.extend(hex_spellings(&format!("%{first:02x}")));
    }
    let valid = match std::str::from_utf8(bytes) {
        Ok(valid) => valid,
        Err(error) => std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or(""),
    };
    if let Some(first) = valid.chars().next() {
        prefixes.push(first.to_string());
        let mut utf16 = [0; 2];
        let first_unit = first.encode_utf16(&mut utf16)[0];
        prefixes.extend(hex_spellings(&format!("\\u{first_unit:04x}")));
        if let Ok(escaped) = serde_json::to_string(&first.to_string()) {
            prefixes.push(escaped[1..escaped.len() - 1].to_string());
        }
    }
    prefixes.sort();
    prefixes.dedup();
    prefixes
}

fn hex_spellings(input: &str) -> Vec<String> {
    let mut variants = vec![String::new()];
    for ch in input.chars() {
        if ch.is_ascii_hexdigit() && ch.is_ascii_alphabetic() {
            let mut upper = variants.clone();
            for prefix in &mut variants {
                prefix.push(ch.to_ascii_lowercase());
            }
            for prefix in &mut upper {
                prefix.push(ch.to_ascii_uppercase());
            }
            variants.extend(upper);
        } else {
            for prefix in &mut variants {
                prefix.push(ch);
            }
        }
    }
    variants
}

fn url_left_boundary(prefix: &str) -> bool {
    let separator = |ch: char| ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | '"' | '\'');
    let Some(last) = prefix.chars().next_back() else {
        return true;
    };
    if separator(last) {
        return true;
    }
    if !matches!(last, '=' | ':') {
        return false;
    }
    // A known URL embedded as the value of another URL remains free text.
    let head = prefix.rsplit(separator).next().unwrap_or(prefix);
    !head.contains("://") && !head.starts_with("//")
}

fn url_right_boundary(suffix: &str) -> bool {
    suffix.chars().next().is_none_or(|ch| {
        ch.is_whitespace() || matches!(ch, ')' | ']' | '}' | '"' | '\'' | ',' | ';')
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

/// Map edit boundaries only within one matched URL, using constant cursor state.
fn request_url_display(input: &str, known: &str) -> String {
    let Some(edits) = url_edits(known) else {
        return "[invalid URL]".to_string();
    };
    let mut mapped = Vec::with_capacity(edits.len());
    let mut cursor = ByteCursor::new(input, false, true);
    let mut consumed = 0;
    for (start, end, replacement) in edits {
        let Some(start) = map_offset(&mut cursor, &mut consumed, start) else {
            return "[invalid URL]".to_string();
        };
        let Some(end) = map_offset(&mut cursor, &mut consumed, end) else {
            return "[invalid URL]".to_string();
        };
        mapped.push((start, end, replacement));
    }
    replace_ranges(input, &mapped).unwrap_or_else(|| "[invalid URL]".to_string())
}

fn map_offset(cursor: &mut ByteCursor<'_>, consumed: &mut usize, offset: usize) -> Option<usize> {
    if offset < *consumed {
        return None;
    }
    while *consumed < offset {
        cursor.next()?;
        *consumed += 1;
    }
    cursor.at_boundary().then_some(cursor.position)
}

fn literal_match(text: &str, bytes: &[u8]) -> Option<usize> {
    (text.as_bytes().starts_with(bytes) && text.is_char_boundary(bytes.len()))
        .then_some(bytes.len())
}

fn credential_match(text: &str, bytes: &[u8]) -> Option<usize> {
    literal_match(text, bytes)
        .into_iter()
        .chain(match_bytes(text, bytes, true))
        .max()
}

fn match_bytes(text: &str, secret: &[u8], percent: bool) -> Option<usize> {
    let mut cursor = ByteCursor::new(text, percent, true);
    for byte in secret {
        if cursor.next()? != *byte {
            return None;
        }
    }
    cursor.at_boundary().then_some(cursor.position)
}

fn decode(text: &str, percent: bool, json: bool) -> Vec<u8> {
    ByteCursor::new(text, percent, json).collect()
}

/// A decoding view with at most one Unicode scalar buffered, never a position map.
struct ByteCursor<'a> {
    text: &'a str,
    position: usize,
    percent: bool,
    json: bool,
    pending: [u8; 4],
    read: usize,
    len: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(text: &'a str, percent: bool, json: bool) -> Self {
        Self {
            text,
            position: 0,
            percent,
            json,
            pending: [0; 4],
            read: 0,
            len: 0,
        }
    }
    fn at_boundary(&self) -> bool {
        self.read == self.len && self.text.is_char_boundary(self.position)
    }
}

impl Iterator for ByteCursor<'_> {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        if self.read < self.len {
            let byte = self.pending[self.read];
            self.read += 1;
            return Some(byte);
        }
        let bytes = self.text.as_bytes();
        if self.percent {
            if let Some(byte) = percent_byte(bytes.get(self.position..)?) {
                self.position += 3;
                return Some(byte);
            }
        }
        if self.json && bytes.get(self.position) == Some(&b'\\') {
            if let Some((ch, len)) = json_char(self.text.get(self.position..)?) {
                self.position += len;
                self.len = ch.encode_utf8(&mut self.pending).len();
                self.read = 1;
                return Some(self.pending[0]);
            }
        }
        let byte = *bytes.get(self.position)?;
        self.position += 1;
        Some(byte)
    }
}

fn percent_byte(input: &[u8]) -> Option<u8> {
    if input.len() < 3 || input[0] != b'%' {
        return None;
    }
    Some(((input[1] as char).to_digit(16)? * 16 + (input[2] as char).to_digit(16)?) as u8)
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
