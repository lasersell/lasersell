use std::collections::HashSet;
use std::io::{self, Write};
use std::sync::OnceLock;

use reqwest::Url;

static REDACTIONS: OnceLock<Vec<String>> = OnceLock::new();

pub fn init_redactions(values: Vec<String>) {
    if REDACTIONS.get().is_some() {
        return;
    }
    let mut seen = HashSet::new();
    let mut cleaned = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            cleaned.push(trimmed.to_string());
        }
    }
    let _ = REDACTIONS.set(cleaned);
}

pub fn scrub_sensitive(input: &str) -> String {
    let mut scrubbed = input.to_string();
    if let Some(values) = REDACTIONS.get() {
        for value in values {
            if !value.is_empty() && scrubbed.contains(value) {
                scrubbed = scrubbed.replace(value, "<redacted>");
            }
        }
    }

    scrubbed = scrub_value_after_marker(&scrubbed, "api-key=");
    scrubbed = scrub_value_after_marker(&scrubbed, "x-api-key=");
    scrubbed = scrub_value_after_marker(&scrubbed, "x-api-key:");
    scrubbed = scrub_value_after_marker(&scrubbed, "api_key=");
    scrubbed = scrub_value_after_marker(&scrubbed, "\"api_key\":");
    scrubbed = scrub_value_after_marker(&scrubbed, "\"apiKey\":");
    scrubbed = scrub_value_after_marker(&scrubbed, "api_key:");
    scrubbed = scrub_value_after_marker(&scrubbed, "Authorization: Bearer ");
    scrubbed = scrub_value_after_marker(&scrubbed, "Authorization: bearer ");
    scrubbed = scrub_value_after_marker(&scrubbed, "authorization: Bearer ");
    scrubbed = scrub_value_after_marker(&scrubbed, "authorization: bearer ");
    scrubbed
}

fn scrub_value_after_marker(input: &str, marker: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while let Some(pos) = input[index..].find(marker) {
        let start = index + pos;
        let marker_end = start + marker.len();
        output.push_str(&input[index..marker_end]);
        let mut cursor = marker_end;
        let bytes = input.as_bytes();

        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if byte == b' ' || byte == b'\t' {
                output.push(byte as char);
                cursor += 1;
            } else {
                break;
            }
        }

        let mut quote: Option<u8> = None;
        if cursor < bytes.len() {
            let byte = bytes[cursor];
            if byte == b'"' || byte == b'\'' {
                output.push(byte as char);
                quote = Some(byte);
                cursor += 1;
            }
        }

        if let Some(quote) = quote {
            while cursor < bytes.len() && bytes[cursor] != quote {
                cursor += 1;
            }
        } else {
            while cursor < bytes.len() {
                let byte = bytes[cursor];
                if byte == b'&'
                    || byte == b' '
                    || byte == b'\t'
                    || byte == b'\r'
                    || byte == b'\n'
                    || byte == b','
                    || byte == b'}'
                    || byte == b']'
                    || byte == b'"'
                    || byte == b'\''
                {
                    break;
                }
                cursor += 1;
            }
        }

        output.push_str("<redacted>");
        index = cursor;
    }
    output.push_str(&input[index..]);
    output
}

pub struct RedactingWriter<W: Write> {
    inner: W,
    buffer: Vec<u8>,
}

impl<W: Write> RedactingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
        }
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        while let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let scrubbed = scrub_sensitive(&line);
            self.inner.write_all(scrubbed.as_bytes())?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buffer.is_empty() {
            let line = String::from_utf8_lossy(&self.buffer);
            let scrubbed = scrub_sensitive(&line);
            self.inner.write_all(scrubbed.as_bytes())?;
            self.buffer.clear();
        }
        self.inner.flush()
    }
}

impl<W: Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

pub fn redact_url(raw: &str) -> String {
    if let Ok(parsed) = Url::parse(raw) {
        let scheme = parsed.scheme();
        if let Some(host) = parsed.host_str() {
            if let Some(port) = parsed.port() {
                return format!("{scheme}://{host}:{port}");
            }
            return format!("{scheme}://{host}");
        }
        return format!("{scheme}://");
    }
    "<invalid-url>".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    fn init_for_tests() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            init_redactions(vec![
                "SECRET_VALUE".to_string(),
                "https://private.example".to_string(),
            ]);
        });
    }

    #[test]
    fn scrub_exact_values() {
        init_for_tests();
        let input = "token=SECRET_VALUE";
        assert_eq!(scrub_sensitive(input), "token=<redacted>");
    }

    #[test]
    fn scrub_api_key_query() {
        init_for_tests();
        let input = "api_key=abc123&foo=bar";
        assert_eq!(scrub_sensitive(input), "api_key=<redacted>&foo=bar");
    }

    #[test]
    fn scrub_x_api_key_header() {
        init_for_tests();
        let input = "x-api-key: abc123";
        assert_eq!(scrub_sensitive(input), "x-api-key: <redacted>");
    }

    #[test]
    fn scrub_api_key_camel_case_json() {
        init_for_tests();
        let input = "{\"apiKey\":\"abc123\"}";
        assert_eq!(scrub_sensitive(input), "{\"apiKey\":\"<redacted>\"}");
    }

    #[test]
    fn scrub_api_key_yaml() {
        init_for_tests();
        let input = "api_key: \"abc123\"";
        assert_eq!(scrub_sensitive(input), "api_key: \"<redacted>\"");
    }

    #[test]
    fn scrub_authorization_bearer() {
        init_for_tests();
        let input = "Authorization: Bearer abc.def";
        assert_eq!(scrub_sensitive(input), "Authorization: Bearer <redacted>");
    }
}
