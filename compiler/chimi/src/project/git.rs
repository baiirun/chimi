//! The dependency cache (packages.md, "Dependency cache"): git packages,
//! fetched with the system `git`. Each repository has one bare clone under
//! `git/db/`, and each locked commit is checked out once under
//! `git/checkouts/<repository>/<commit>/`, which never changes after that, so
//! every build that locks the same commit shares it.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Makes each scratch checkout's name unique within the process.
static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

/// Whether the cache may fetch, as `--offline` says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Network {
    Allowed,
    Offline,
}

/// Where fetched packages live, and whether a missing one may be fetched.
#[derive(Clone, Debug)]
pub struct GitCache {
    /// `None` when no cache directory can be named; the first git package
    /// reports it.
    directory: Option<PathBuf>,
    network: Network,
}

#[derive(Debug)]
pub enum GitError {
    /// No `CHIMI_CACHE`, `XDG_CACHE_HOME` or `HOME` to put the cache in.
    NoCacheDirectory,
    /// Offline, and the commit is not in the cache.
    NotCached { url: String, commit: String },
    /// `rev` names nothing in the repository.
    UnknownRevision { url: String, rev: String },
    /// `git` failed, or could not be started.
    Command { action: String, detail: String },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for GitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCacheDirectory => formatter.write_str(
                "no directory for the dependency cache: set CHIMI_CACHE, XDG_CACHE_HOME or HOME",
            ),
            Self::NotCached { url, commit } => write!(
                formatter,
                "commit {commit} of {url} is not in the dependency cache, and --offline \
                 forbids fetching it"
            ),
            Self::UnknownRevision { url, rev } => {
                write!(formatter, "`{rev}` is not a tag, branch or commit of {url}")
            }
            Self::Command { action, detail } => write!(formatter, "{action}: {detail}"),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for GitError {}

impl GitCache {
    /// The cache at `$CHIMI_CACHE`, else `$XDG_CACHE_HOME/chimi`, else
    /// `~/.cache/chimi`.
    pub fn from_environment(network: Network) -> Self {
        let variable = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        let directory = variable("CHIMI_CACHE")
            .map(PathBuf::from)
            .or_else(|| variable("XDG_CACHE_HOME").map(|base| PathBuf::from(base).join("chimi")))
            .or_else(|| variable("HOME").map(|home| PathBuf::from(home).join(".cache/chimi")));
        Self { directory, network }
    }

    pub fn at(directory: PathBuf, network: Network) -> Self {
        Self {
            directory: Some(directory),
            network,
        }
    }

    /// The commit `rev` points to in `url` now, fetching first. Only
    /// `chimi deps update` asks, so this always uses the network.
    pub(crate) fn resolve(&self, url: &str, rev: &str) -> Result<String, GitError> {
        let database = self.database(url)?;
        fetch_refs(&database, url)?;
        if is_commit_id(rev) && commit_of(&database, rev)?.is_none() {
            fetch_commit(&database, url, rev)?;
        }
        commit_of(&database, rev)?.ok_or_else(|| GitError::UnknownRevision {
            url: url.to_owned(),
            rev: rev.to_owned(),
        })
    }

    /// The directory holding `url` at `commit`, checking it out the first
    /// time, and fetching it first if the cache lacks it and the network is
    /// allowed.
    pub(crate) fn checkout(&self, url: &str, commit: &str) -> Result<PathBuf, GitError> {
        let base = self.base()?.join("git/checkouts").join(repository_key(url));
        let checkout = base.join(commit);
        if checkout.is_dir() {
            return Ok(checkout);
        }
        let not_cached = || GitError::NotCached {
            url: url.to_owned(),
            commit: commit.to_owned(),
        };
        // Offline, a repository the cache has never seen is not created.
        if self.network == Network::Offline && !self.database_path(url)?.join("HEAD").is_file() {
            return Err(not_cached());
        }
        let database = self.database(url)?;
        if commit_of(&database, commit)?.as_deref() != Some(commit) {
            if self.network == Network::Offline {
                return Err(not_cached());
            }
            fetch_refs(&database, url)?;
            if commit_of(&database, commit)?.is_none() {
                fetch_commit(&database, url, commit)?;
            }
        }
        // Written beside its final name and renamed into place, so a
        // checkout is either complete or absent, even when two builds race.
        let unique = format!(
            "{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        );
        let scratch = base.join(format!(".{commit}.{unique}"));
        let _ = std::fs::remove_dir_all(&scratch);
        create_directory(&scratch)?;
        let index = base.join(format!(".{commit}.{unique}.index"));
        let written = Command::new("git")
            .arg("--git-dir")
            .arg(&database)
            .arg("--work-tree")
            .arg(&scratch)
            .args(["checkout", "--quiet", "--force", commit, "--", "."])
            .env("GIT_INDEX_FILE", &index)
            .output();
        let _ = std::fs::remove_file(&index);
        if let Err(detail) = finished(written) {
            let _ = std::fs::remove_dir_all(&scratch);
            return Err(command_error(
                format!("check out {commit} of {url}"),
                detail,
            ));
        }
        if std::fs::rename(&scratch, &checkout).is_err() {
            // Another build finished the same checkout first.
            let _ = std::fs::remove_dir_all(&scratch);
            if !checkout.is_dir() {
                return Err(GitError::Io {
                    path: checkout,
                    source: std::io::Error::other("could not move the checkout into place"),
                });
            }
        }
        Ok(checkout)
    }

    fn database_path(&self, url: &str) -> Result<PathBuf, GitError> {
        Ok(self
            .base()?
            .join("git/db")
            .join(format!("{}.git", repository_key(url))))
    }

    fn base(&self) -> Result<&Path, GitError> {
        self.directory.as_deref().ok_or(GitError::NoCacheDirectory)
    }

    /// The bare repository that collects `url`'s objects, created empty the
    /// first time.
    fn database(&self, url: &str) -> Result<PathBuf, GitError> {
        let database = self.database_path(url)?;
        if !database.join("HEAD").is_file() {
            create_directory(&database)?;
            finished(
                Command::new("git")
                    .args(["init", "--quiet", "--bare"])
                    .arg(&database)
                    .output(),
            )
            .map_err(|detail| command_error(format!("create the cache for {url}"), detail))?;
        }
        Ok(database)
    }
}

/// Every branch and tag of `url`, moved to where they point now.
fn fetch_refs(database: &Path, url: &str) -> Result<(), GitError> {
    git(
        database,
        &[
            "fetch",
            "--quiet",
            "--force",
            "--prune",
            "--end-of-options",
            url,
            "+refs/heads/*:refs/heads/*",
            "+refs/tags/*:refs/tags/*",
        ],
    )
    .map(drop)
    .map_err(|detail| command_error(format!("fetch {url}"), detail))
}

/// A commit no branch or tag reaches any more, fetched by its id.
fn fetch_commit(database: &Path, url: &str, commit: &str) -> Result<(), GitError> {
    git(
        database,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--end-of-options",
            url,
            commit,
        ],
    )
    .map(drop)
    .map_err(|detail| command_error(format!("fetch {commit} from {url}"), detail))
}

/// A full commit id, which a repository can be asked for directly.
pub(crate) fn is_commit_id(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The commit `rev` names in `database`, or `None` when it names none.
fn commit_of(database: &Path, rev: &str) -> Result<Option<String>, GitError> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(database)
        .args(["rev-parse", "--quiet", "--verify", "--end-of-options"])
        .arg(format!("{rev}^{{commit}}"))
        .output()
        .map_err(|source| command_error("run git".to_owned(), source.to_string()))?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

/// Runs `git` on `database`. Two builds fetching into one cached
/// repository at once can collide on a ref's lock file; the loser waits and
/// tries again, a few times, before reporting the failure.
fn git(database: &Path, arguments: &[&str]) -> Result<Vec<u8>, String> {
    let run = || {
        finished(
            Command::new("git")
                .arg("--git-dir")
                .arg(database)
                // Only transports that fetch; `ext::` runs commands.
                .args(["-c", "protocol.ext.allow=never"])
                .args(arguments)
                // Untranslated messages, which `is_lock_collision` reads.
                .env("LC_ALL", "C")
                .output(),
        )
    };
    let mut wait = std::time::Duration::from_millis(100);
    for _ in 0..LOCK_RETRIES {
        match run() {
            Err(detail) if is_lock_collision(&detail) => {
                std::thread::sleep(wait);
                wait *= 2;
            }
            finished => return finished,
        }
    }
    run()
}

/// Retries after a lock collision, waiting 0.1, 0.2, 0.4 and 0.8 seconds.
const LOCK_RETRIES: usize = 4;

/// Whether `git` failed because another process held a lock file in the
/// repository, which it reports as `cannot lock ref` or `Unable to create
/// '….lock': File exists`.
fn is_lock_collision(detail: &str) -> bool {
    detail.contains("cannot lock ref")
        || (detail.contains(".lock") && detail.contains("File exists"))
}

fn finished(output: std::io::Result<std::process::Output>) -> Result<Vec<u8>, String> {
    let output = output.map_err(|error| format!("cannot run git: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn command_error(action: String, detail: String) -> GitError {
    GitError::Command { action, detail }
}

fn create_directory(path: &Path) -> Result<(), GitError> {
    std::fs::create_dir_all(path).map_err(|source| GitError::Io {
        path: path.to_owned(),
        source,
    })
}

/// A directory name for `url`: its last segment, readable, then a hash of
/// the whole URL, so two repositories never share one.
fn repository_key(url: &str) -> String {
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default()
        .trim_end_matches(".git");
    let readable: String = last
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .take(32)
        .collect();
    // FNV-1a, written out so the name stays the same across compiler builds.
    let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{readable}-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::{GitCache, Network, is_lock_collision, repository_key};
    use std::path::Path;
    use std::process::Command;

    fn git(directory: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .current_dir(directory)
            .args([
                "-c",
                "user.name=chimi",
                "-c",
                "user.email=chimi@example.com",
            ])
            .args(["-c", "commit.gpgsign=false"])
            .args(arguments)
            .output()
            .expect("run git");
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    #[test]
    fn a_fetch_that_meets_another_fetchs_lock_waits_and_retries() {
        let scratch = std::env::temp_dir().join(format!("chimi-git-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let repo = scratch.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        std::fs::write(repo.join("file"), "1").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "first"]);
        let url = format!("file://{}", repo.display());
        let cache = GitCache::at(scratch.join("cache"), Network::Allowed);
        let first = cache.resolve(&url, "main").unwrap();

        std::fs::write(repo.join("file"), "2").unwrap();
        git(&repo, &["commit", "--quiet", "-am", "second"]);
        // Another fetch holds the branch's lock for a moment.
        let database = cache.database(&url).unwrap();
        let lock = database.join("refs/heads/main.lock");
        std::fs::write(&lock, "").unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            std::fs::remove_file(lock).unwrap();
        });
        let second = cache.resolve(&url, "main").unwrap();
        release.join().unwrap();
        assert_ne!(first, second, "the retry fetched the new commit");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn only_lock_collisions_are_retried() {
        assert!(is_lock_collision(
            "error: cannot lock ref 'refs/heads/main': Unable to create '/c/refs/heads/main.lock': File exists."
        ));
        assert!(!is_lock_collision(
            "fatal: repository '/missing' does not exist"
        ));
    }

    #[test]
    fn a_repository_key_is_readable_and_unique_per_url() {
        let key = repository_key("https://github.com/example/json_tools.git");
        assert!(key.starts_with("json_tools-"), "{key}");
        assert_eq!(
            key,
            repository_key("https://github.com/example/json_tools.git")
        );
        assert_ne!(
            key,
            repository_key("https://github.com/other/json_tools.git")
        );
        assert!(repository_key("file:///tmp/a b/..").starts_with("-"));
    }
}
