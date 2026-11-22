//! `chimi.lock` (packages.md, "Lockfile"): for each git package of a
//! workspace's builds, the source its manifest names and the commit that
//! source resolved to. Only `chimi deps update` writes it; builds read it.

use super::toml::{self, Value};
use super::{GitSource, ManifestError, PackageName};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

pub(crate) const LOCK_FILE: &str = "chimi.lock";

/// The format version this compiler reads and writes.
const VERSION: &str = "1";

/// The pins, sorted by package name, which is the order they are written in.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct Lockfile {
    pub(crate) entries: BTreeMap<PackageName, LockedGit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LockedGit {
    pub(crate) source: GitSource,
    /// The full commit id `source.rev` resolved to.
    pub(crate) commit: String,
}

impl Lockfile {
    /// The lockfile at `path`, or an empty one when there is no file.
    pub(crate) fn read(path: &Path) -> Result<Self, ManifestError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(ManifestError {
                    path: path.to_owned(),
                    line: None,
                    message: format!("cannot read: {error}"),
                });
            }
        };
        Self::parse(&text).map_err(|(line, message)| ManifestError {
            path: path.to_owned(),
            line,
            message,
        })
    }

    fn parse(text: &str) -> Result<Self, (Option<usize>, String)> {
        let document = toml::parse(text).map_err(|error| (Some(error.line), error.message))?;
        let mut version = None;
        for entry in &document.root.entries {
            match (entry.key.as_str(), &entry.value) {
                ("version", Value::String(text)) => version = Some(text.as_str()),
                _ => {
                    return Err((
                        Some(entry.line),
                        format!("unknown key `{}` outside any table", entry.key),
                    ));
                }
            }
        }
        match version {
            Some(VERSION) => {}
            Some(other) => {
                return Err((
                    None,
                    format!(
                        "lockfile version \"{other}\" is not one this compiler reads (\"{VERSION}\")"
                    ),
                ));
            }
            None => return Err((None, "the lockfile needs `version = \"1\"`".to_owned())),
        }
        let mut entries = BTreeMap::new();
        for table in &document.tables {
            let name = table
                .name
                .strip_prefix("git.")
                .filter(|_| !table.array_element)
                .and_then(PackageName::parse)
                .ok_or_else(|| (Some(table.line), format!("unknown table [{}]", table.name)))?;
            let mut fields: BTreeMap<&str, &str> = BTreeMap::new();
            for entry in &table.entries {
                match (entry.key.as_str(), &entry.value) {
                    ("url" | "rev" | "directory" | "commit", Value::String(text)) => {
                        fields.insert(entry.key.as_str(), text);
                    }
                    _ => {
                        return Err((
                            Some(entry.line),
                            format!("unknown key `{}` in [{}]", entry.key, table.name),
                        ));
                    }
                }
            }
            let required = |key: &str| {
                fields
                    .get(key)
                    .map(|text| (*text).to_owned())
                    .ok_or_else(|| (Some(table.line), format!("[{}] needs `{key}`", table.name)))
            };
            let commit = required("commit")?;
            if !crate::project::is_commit_id(&commit) {
                return Err((
                    Some(table.line),
                    format!(
                        "[{}]: `commit` must be a full 40-digit lowercase commit id",
                        table.name
                    ),
                ));
            }
            entries.insert(
                name,
                LockedGit {
                    source: GitSource {
                        url: required("url")?,
                        rev: required("rev")?,
                        directory: fields.get("directory").map(|text| (*text).to_owned()),
                    },
                    commit,
                },
            );
        }
        Ok(Self { entries })
    }

    pub(crate) fn render(&self) -> String {
        let mut text = format!(
            "# Written by `chimi deps update`. Commit this file.\nversion = \"{VERSION}\"\n"
        );
        for (name, locked) in &self.entries {
            let _ = write!(
                text,
                "\n[git.{name}]\nurl = {}\nrev = {}\n",
                quoted(&locked.source.url),
                quoted(&locked.source.rev)
            );
            if let Some(directory) = &locked.source.directory {
                let _ = writeln!(text, "directory = {}", quoted(directory));
            }
            let _ = writeln!(text, "commit = \"{}\"", locked.commit);
        }
        text
    }
}

/// A TOML basic string that [`toml::parse`] reads back as `text`.
fn quoted(text: &str) -> String {
    let mut quoted = String::from('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push('\t'),
            character if character.is_control() => {
                let _ = write!(quoted, "\\u{:04x}", u32::from(character));
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::{GitSource, LockedGit, Lockfile, PackageName};

    fn pin(url: &str, rev: &str, directory: Option<&str>, commit: char) -> LockedGit {
        LockedGit {
            source: GitSource {
                url: url.into(),
                rev: rev.into(),
                directory: directory.map(Into::into),
            },
            commit: commit.to_string().repeat(40),
        }
    }

    #[test]
    fn renders_entries_sorted_by_name_and_reads_them_back() {
        let mut lock = Lockfile::default();
        for (name, locked) in [
            ("zed", pin("file:///z \"quoted\"\\", "main", None, 'a')),
            (
                "json_tools",
                pin("https://example.com/json", "v0.3.0", Some("pkg"), 'b'),
            ),
        ] {
            lock.entries
                .insert(PackageName::parse(name).unwrap(), locked);
        }
        let text = lock.render();
        assert_eq!(
            text,
            concat!(
                "# Written by `chimi deps update`. Commit this file.\n",
                "version = \"1\"\n",
                "\n[git.json_tools]\nurl = \"https://example.com/json\"\nrev = \"v0.3.0\"\n",
                "directory = \"pkg\"\ncommit = \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"\n",
                "\n[git.zed]\nurl = \"file:///z \\\"quoted\\\"\\\\\"\nrev = \"main\"\n",
                "commit = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
            )
        );
        assert_eq!(Lockfile::parse(&text), Ok(lock));
    }

    #[test]
    fn rejects_what_it_cannot_read_naming_it() {
        let commit = "c".repeat(40);
        let entry = |extra: &str| {
            format!(
                "version = \"1\"\n[git.a]\nurl = \"u\"\nrev = \"r\"\ncommit = \"{commit}\"\n{extra}"
            )
        };
        let cases = [
            (
                String::from("[git.a]\n"),
                "the lockfile needs `version = \"1\"`",
            ),
            (
                String::from("version = \"2\"\n"),
                "lockfile version \"2\" is not one",
            ),
            (
                String::from("version = \"1\"\nextra = \"x\"\n"),
                "2: unknown key `extra` outside",
            ),
            (
                entry("branch = \"x\"\n"),
                "6: unknown key `branch` in [git.a]",
            ),
            (
                String::from("version = \"1\"\n[path.a]\n"),
                "2: unknown table [path.a]",
            ),
            (
                String::from("version = \"1\"\n[git.A]\n"),
                "2: unknown table [git.A]",
            ),
            (
                String::from("version = \"1\"\n[git.a]\nurl = \"u\"\nrev = \"r\"\n"),
                "2: [git.a] needs `commit`",
            ),
            (
                String::from(
                    "version = \"1\"\n[git.a]\nurl = \"u\"\nrev = \"r\"\ncommit = \"abc\"\n",
                ),
                "2: [git.a]: `commit` must be a full 40-digit lowercase commit id",
            ),
            (
                format!("version = \"1\"\n[git.a]\nrev = \"r\"\ncommit = \"{commit}\"\n"),
                "2: [git.a] needs `url`",
            ),
        ];
        for (text, expected) in cases {
            let (line, message) = Lockfile::parse(&text).expect_err(&text);
            let rendered = match line {
                Some(line) => format!("{line}: {message}"),
                None => message,
            };
            assert!(rendered.starts_with(expected), "{text:?}: {rendered}");
        }
    }

    #[test]
    fn a_missing_lockfile_is_empty() {
        let path = std::env::temp_dir().join(format!("chimi-no-lock-{}", std::process::id()));
        assert_eq!(
            Lockfile::read(&path.join("chimi.lock")).unwrap(),
            Lockfile::default()
        );
    }
}
