use crate::ids::FileId;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
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
