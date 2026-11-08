//! API keys minted by the core: `lsk_<id>_<secret>`, where the 12-character id is public
//! and the secret is not. Keys are only ever shown as `lsk_<id>_…`; the full key is
//! printed once, by `api-keys create`.

/// The prefix of every API key.
pub const PREFIX: &str = "lsk_";
/// The length of a key's public id.
pub const ID_LEN: usize = 12;

fn is_id_char(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

fn is_secret_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

/// Whether a bearer token is an API key rather than a session token.
pub fn is_api_key(token: &str) -> bool {
    token.starts_with(PREFIX)
}

/// The public id of a key, or of its display form (`lsk_<id>_…`), or a bare id.
pub fn key_id(input: &str) -> Option<String> {
    let input = input.trim();
    let rest = input.strip_prefix(PREFIX).unwrap_or(input);
    let id = rest.get(..ID_LEN)?;
    let after = &rest[ID_LEN..];
    let valid_end = after.is_empty() || after.starts_with('_');
    (id.bytes().all(is_id_char) && valid_end).then(|| id.to_string())
}

/// How a key is shown: `lsk_<id>_…`. Anything that is not a key shows as `…`.
pub fn display(token: &str) -> String {
    match key_id(token).filter(|_| is_api_key(token)) {
        Some(id) => format!("{PREFIX}{id}_…"),
        None => "…".to_string(),
    }
}

/// Replaces the secret of every API key in `text` with `…`, so messages from the core or
/// from other programs can never show a key.
pub fn redact(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while let Some(found) = text[i..].find(PREFIX) {
        let start = i + found;
        let id_start = start + PREFIX.len();
        let id_end = id_start + ID_LEN;
        let is_key = bytes.len() > id_end
            && bytes[id_start..id_end].iter().all(|&c| is_id_char(c))
            && bytes[id_end] == b'_';
        if !is_key {
            i = id_start;
            continue;
        }
        let mut secret_end = id_end + 1;
        while secret_end < bytes.len() && is_secret_char(bytes[secret_end]) {
            secret_end += 1;
        }
        if secret_end == id_end + 1 {
            // `lsk_<id>_` with no secret is already the display form
            i = secret_end;
            continue;
        }
        out.push_str(&text[copied..id_end + 1]);
        out.push('…');
        copied = secret_end;
        i = secret_end;
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "lsk_ab12cd34ef56_Zm9vYmFyYmF6cXV4cXV1eHF1dXhxdXV4cXV1eHF1dXg";

    #[test]
    fn recognises_keys() {
        assert!(is_api_key(KEY));
        assert!(!is_api_key("eyJhbGciOiJIUzI1NiJ9.e30.x"));
        assert_eq!(display(KEY), "lsk_ab12cd34ef56_…");
        assert_eq!(display("eyJhbGciOiJIUzI1NiJ9.e30.x"), "…");
    }

    #[test]
    fn key_ids() {
        for input in [
            KEY,
            "lsk_ab12cd34ef56_…",
            "lsk_ab12cd34ef56",
            "ab12cd34ef56",
        ] {
            assert_eq!(key_id(input).as_deref(), Some("ab12cd34ef56"), "{input}");
        }
        for bad in [
            "",
            "lsk_short",
            "lsk_AB12CD34EF56",
            "ab12cd34ef567",
            "lsk_ab12cd34ef56x",
        ] {
            assert_eq!(key_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn redaction_hides_secrets_only() {
        let text = format!("refused {KEY}, also \"{KEY}\" and lsk_ab12cd34ef56_… and lsk_x");
        let redacted = redact(&text);
        assert!(!redacted.contains("Zm9v"), "{redacted}");
        assert_eq!(
            redacted,
            "refused lsk_ab12cd34ef56_…, also \"lsk_ab12cd34ef56_…\" and lsk_ab12cd34ef56_… and lsk_x"
        );
        assert_eq!(redact("no keys here"), "no keys here");
        assert_eq!(redact("ends with lsk_"), "ends with lsk_");
    }
}
