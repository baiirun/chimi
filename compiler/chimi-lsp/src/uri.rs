//! Conversion between `file:` URIs and filesystem paths.
//!
//! Only absolute `file:` URIs with an empty or `localhost` authority name
//! local documents; every other URI is outside the workspace.

use lsp_types::Uri;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub fn file_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let path = rest.split(['?', '#']).next()?;
    percent_decode(path).map(PathBuf::from)
}

/// Requires: `path` is absolute.
pub fn file_uri(path: &Path) -> Uri {
    let mut encoded = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    Uri::from_str(&encoded).expect("percent-encoded file paths are valid URIs")
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::{file_path, file_uri};
    use lsp_types::Uri;
    use std::path::Path;
    use std::str::FromStr;

    /// Guards percent-encoding of reserved and non-ASCII path bytes, and that
    /// only local `file:` URIs map to paths.
    #[test]
    fn round_trips_local_file_paths_and_rejects_other_uris() {
        let path = Path::new("/Users/me/my project/café#1.chimi");
        let uri = file_uri(path);
        assert_eq!(
            uri.as_str(),
            "file:///Users/me/my%20project/caf%C3%A9%231.chimi"
        );
        assert_eq!(file_path(&uri).as_deref(), Some(path));

        let localhost = Uri::from_str("file://localhost/tmp/a.chimi").unwrap();
        assert_eq!(
            file_path(&localhost).as_deref(),
            Some(Path::new("/tmp/a.chimi"))
        );
        for other in ["untitled:Untitled-1", "file://server/share/a.chimi"] {
            assert_eq!(file_path(&Uri::from_str(other).unwrap()), None, "{other}");
        }
    }
}
