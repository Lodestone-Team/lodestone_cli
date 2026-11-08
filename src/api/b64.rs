//! The base64 flavour the core's file routes use for paths: the URL-safe alphabet
//! (`-` and `_`) without `=` padding, as `js-base64`'s `Base64.encode(path, true)` in the
//! dashboard, and `URL_SAFE_NO_PAD` in the core.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Encodes `input` as URL-safe base64 without padding.
pub fn encode_url_safe_no_pad(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        let symbols = chunk.len() + 1;
        for i in 0..symbols {
            let index = (n >> (18 - 6 * i)) & 0x3f;
            out.push(ALPHABET[index as usize] as char);
        }
    }
    out
}

/// Decodes URL-safe base64 without padding. Only used by the tests and the mock server.
#[cfg(test)]
pub fn decode_url_safe_no_pad(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in input.bytes() {
        let value = ALPHABET.iter().position(|&a| a == c)? as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    // leftover bits must be zero padding of the last symbol
    if bits >= 6 || buffer != 0 {
        return None;
    }
    Some(out)
}

/// A path as it goes into a file route: the dashboard sends `.` for an instance's root,
/// and the core cannot route an empty segment.
pub fn encode_path(path: &str) -> String {
    let path = if path.is_empty() { "." } else { path };
    encode_url_safe_no_pad(path.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors_without_padding() {
        let cases = [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in cases {
            assert_eq!(encode_url_safe_no_pad(plain.as_bytes()), encoded);
            assert_eq!(
                decode_url_safe_no_pad(encoded).unwrap(),
                plain.as_bytes(),
                "{encoded}"
            );
        }
    }

    #[test]
    fn uses_the_url_safe_alphabet() {
        // 0xfb 0xff encodes to "+/8" in the standard alphabet
        assert_eq!(encode_url_safe_no_pad(&[0xfb, 0xff]), "-_8");
        assert_eq!(encode_url_safe_no_pad(&[0xff; 3]), "____");
        assert!(!encode_url_safe_no_pad(b"??>>").contains(['+', '/', '=']));
    }

    #[test]
    fn paths() {
        assert_eq!(encode_path(""), "Lg");
        assert_eq!(encode_path("."), "Lg");
        assert_eq!(encode_path("world/level.dat"), "d29ybGQvbGV2ZWwuZGF0");
        // non-ASCII names are encoded as UTF-8
        assert_eq!(encode_path("服务器"), "5pyN5Yqh5Zmo");
        assert_eq!(
            decode_url_safe_no_pad(&encode_path("C:\\Users\\a b\\server.properties")).unwrap(),
            b"C:\\Users\\a b\\server.properties"
        );
    }

    #[test]
    fn decoding_rejects_bad_input() {
        assert!(decode_url_safe_no_pad("Zm9v=").is_none());
        assert!(decode_url_safe_no_pad("Zm+v").is_none());
        // a single symbol cannot encode a byte
        assert!(decode_url_safe_no_pad("Z").is_none());
    }
}
