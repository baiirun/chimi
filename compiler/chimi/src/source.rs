use crate::ids::{FileId, TextRange};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    file_id: FileId,
    path: PathBuf,
    text: Arc<str>,
}

impl SourceSnapshot {
    pub fn read(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)?;
        Ok(Self::from_text(path, text))
    }

    pub fn from_text(path: impl Into<PathBuf>, text: impl Into<Arc<str>>) -> Self {
        Self {
            file_id: FileId::new(0),
            path: path.into(),
            text: text.into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub(crate) const fn file_id(&self) -> FileId {
        self.file_id
    }

    pub(crate) fn assign_file_id(&mut self, file_id: FileId) {
        self.file_id = file_id;
    }
}

/// A module's fingerprint as a trace records it: FNV-1a 64, as 16 hex
/// digits, over `text` up to `end` with each of `blanked` counted as that many
/// spaces. It tells a viewer whether the file it was given is the one the run
/// was built from; it is not a security check.
pub(crate) fn source_fingerprint(text: &str, end: usize, blanked: &[TextRange]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |byte: u8| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    let bytes = &text.as_bytes()[..end.min(text.len())];
    let mut offset = 0;
    for range in blanked {
        let (start, stop) = (
            range.start() as usize,
            (range.end() as usize).min(bytes.len()),
        );
        if start < offset || start >= stop {
            continue;
        }
        bytes[offset..start].iter().copied().for_each(&mut mix);
        (start..stop).for_each(|_| mix(b' '));
        offset = stop;
    }
    bytes[offset..].iter().copied().for_each(&mut mix);
    format!("{hash:016x}")
}
