#[inline(always)]
pub fn prefix_boundary_matches(path: &str, prefix_len: usize) -> bool {
    if prefix_len <= 1 {
        return true;
    }
    if path.len() == prefix_len {
        return true;
    }
    path.as_bytes().get(prefix_len) == Some(&b'/')
}

/// Returns whether a request path can be interpreted differently by routing
/// and an upstream server.
pub fn request_path_is_ambiguous(path_and_query: &str) -> bool {
    let path = path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path);
    if path.contains("//") || path.contains('\\') {
        return true;
    }

    let mut decoded = String::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return true;
            }
            let Some(high) = hex_digit(bytes[index + 1]) else {
                return true;
            };
            let Some(low) = hex_digit(bytes[index + 2]) else {
                return true;
            };
            decoded.push((high << 4 | low) as char);
            index += 3;
        } else {
            decoded.push(bytes[index] as char);
            index += 1;
        }
    }

    decoded
        .split('/')
        .any(|segment| segment == "." || segment == "..")
        || decoded.contains("//")
        || decoded.contains('\\')
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{prefix_boundary_matches, request_path_is_ambiguous};

    #[test]
    fn prefix_boundary_matches_exact_prefix_length() {
        assert!(prefix_boundary_matches("/api", "/api".len()));
    }

    #[test]
    fn prefix_boundary_matches_segment_boundary() {
        assert!(prefix_boundary_matches("/api/v1", "/api".len()));
    }

    #[test]
    fn prefix_boundary_rejects_mid_segment_match() {
        assert!(!prefix_boundary_matches("/apixyz", "/api".len()));
    }

    #[test]
    fn prefix_boundary_treats_root_prefix_as_match() {
        assert!(prefix_boundary_matches("/anything", 1));
    }

    #[test]
    fn request_path_rejects_ambiguous_dot_segments_and_slashes() {
        for path in [
            "/public/../admin/x",
            "/%2e%2e/admin/x",
            "/%2E./admin/x",
            "//admin/x",
            "/admin\\x",
        ] {
            assert!(
                request_path_is_ambiguous(path),
                "path should be rejected: {path}"
            );
        }
    }

    #[test]
    fn request_path_allows_unambiguous_percent_encoding_and_query_values() {
        assert!(!request_path_is_ambiguous(
            "/public/report%20final?next=../admin"
        ));
    }
}
