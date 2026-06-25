//! Ticket storage: load/save tickets, scaffold a repo, and generate ids.
//!
//! All writes are atomic (temp file + rename); new tickets are created with
//! `O_EXCL` so concurrent agents never clobber each other (R15).

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::config::{Config, CONFIG_FILE};
use crate::error::{Error, Result};
use crate::ticket::Ticket;

/// A repository handle: the root directory plus its loaded config.
pub struct Store {
    /// Repository root directory.
    pub repo_root: PathBuf,
    /// Loaded configuration.
    pub config: Config,
}

/// Outcome of creating a ticket with an explicit id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateOutcome {
    /// A new file was written.
    Created,
    /// An identical file already existed (idempotent no-op).
    Unchanged,
}

impl Store {
    /// Open a repository, loading its config (errors if not initialized).
    pub fn open(repo_root: &Path) -> Result<Self> {
        let config = Config::load(repo_root)?;
        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            config,
        })
    }

    /// Absolute path to the tickets directory.
    #[must_use]
    pub fn tickets_dir(&self) -> PathBuf {
        self.config.tickets_path(&self.repo_root)
    }

    /// Absolute path to a ticket file by id.
    #[must_use]
    pub fn path_for(&self, id: &str) -> PathBuf {
        self.tickets_dir().join(format!("{id}.md"))
    }

    /// Sorted list of `*.md` ticket files (empty if the directory is absent).
    pub fn ticket_files(&self) -> Result<Vec<PathBuf>> {
        let dir = self.tickets_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for entry in fs::read_dir(&dir)
            .map_err(|e| Error::Invalid(format!("cannot read {}: {e}", dir.display())))?
        {
            let path = entry.map_err(Error::Io)?.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                files.push(path);
            }
        }
        files.sort();
        Ok(files)
    }

    /// Load and parse every ticket (sorted by id). Fails if any file is invalid.
    pub fn load_all(&self) -> Result<Vec<Ticket>> {
        let mut tickets = Vec::new();
        for path in self.ticket_files()? {
            let ticket = Ticket::load(&path)
                .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
            tickets.push(ticket);
        }
        tickets.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(tickets)
    }

    /// Load a single ticket by id.
    pub fn load(&self, id: &str) -> Result<Ticket> {
        let path = self.path_for(id);
        if !path.exists() {
            return Err(Error::NotFound(id.to_string()));
        }
        Ticket::load(&path)
    }

    /// Atomically overwrite a ticket file.
    pub fn save(&self, ticket: &Ticket) -> Result<()> {
        write_atomic(&self.path_for(&ticket.id), &ticket.render())
    }

    /// Create a ticket with an explicit id (idempotent + atomic). Re-creating
    /// with byte-identical content is a no-op; differing content is an error.
    pub fn create_exact(&self, id: &str, contents: &str) -> Result<CreateOutcome> {
        let path = self.path_for(id);
        match create_exclusive(&path, contents) {
            Ok(()) => Ok(CreateOutcome::Created),
            Err(Error::Io(ref e)) if e.kind() == ErrorKind::AlreadyExists => {
                let existing = fs::read_to_string(&path).map_err(Error::Io)?;
                if existing == contents {
                    Ok(CreateOutcome::Unchanged)
                } else {
                    Err(Error::Invalid(format!(
                        "ticket `{id}` already exists with different content"
                    )))
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Create a ticket choosing a unique id from `base_id` (`-2`, `-3`, ... on
    /// collision). `render` builds the file contents for the chosen id. Atomic.
    pub fn create_unique(
        &self,
        base_id: &str,
        render: impl Fn(&str) -> Result<String>,
    ) -> Result<String> {
        for n in 1u32.. {
            let id = if n == 1 {
                base_id.to_string()
            } else {
                format!("{base_id}-{n}")
            };
            match create_exclusive(&self.path_for(&id), &render(&id)?) {
                Ok(()) => return Ok(id),
                Err(Error::Io(ref e)) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        unreachable!("u32 id-suffix range is effectively unbounded")
    }
}

/// Outcome of [`init_repo`].
pub struct InitOutcome {
    /// The tickets directory that now exists.
    pub tickets_dir: PathBuf,
    /// Whether a fresh config file was written.
    pub wrote_config: bool,
}

/// Scaffold a repository: create the tickets directory and (unless one exists) a
/// templated `ticketsplease.toml`. Idempotent unless `force`.
pub fn init_repo(
    repo_root: &Path,
    tickets_dir: &str,
    config_body: &str,
    force: bool,
) -> Result<InitOutcome> {
    let dir = repo_root.join(tickets_dir);
    fs::create_dir_all(&dir).map_err(Error::Io)?;
    let config_path = repo_root.join(CONFIG_FILE);
    let wrote_config = if force || !config_path.exists() {
        write_atomic(&config_path, config_body)?;
        true
    } else {
        false
    };
    Ok(InitOutcome {
        tickets_dir: dir,
        wrote_config,
    })
}

/// Derive a slug id from a title: lowercase ASCII alphanumerics, with other runs
/// collapsed to single `-`. Empty results fall back to `ticket`.
#[must_use]
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "ticket".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The default `ticketsplease.toml` body (path-glob backend, commented examples).
#[must_use]
pub fn default_config_template(tickets_dir: &str) -> String {
    format!(
        "schema_version = 1\n\
         tickets_dir = \"{tickets_dir}\"\n\
         default_base = \"main\"\n\
         \n\
         [language]\n\
         # \"none\" = path-glob scopes only; \"rust\" = also expand via the cargo crate graph.\n\
         backend = \"none\"\n\
         \n\
         # Map abstract scope names to path globs. Tickets reference these stable names.\n\
         [scopes]\n\
         # \"datafusion/session\" = [\"quiltdb-datafusion/src/session/**\"]\n\
         \n\
         # Optionally map a scope to its owning crate so the Rust backend can expand\n\
         # reverse-dependents (requires `cargo` at runtime).\n\
         [scope_crates]\n\
         # \"core\" = \"quiltdb-core\"\n"
    )
}

pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("ticket.md");
    let tmp = dir.join(format!(".{file_name}.tmp.{}", std::process::id()));
    {
        let mut f = File::create(&tmp).map_err(Error::Io)?;
        f.write_all(contents.as_bytes()).map_err(Error::Io)?;
        f.sync_all().map_err(Error::Io)?;
    }
    fs::rename(&tmp, path).map_err(Error::Io)?;
    Ok(())
}

fn create_exclusive(path: &Path, contents: &str) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(Error::Io)?;
    f.write_all(contents.as_bytes()).map_err(Error::Io)?;
    f.sync_all().map_err(Error::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Add Vector Index"), "add-vector-index");
        assert_eq!(slugify("  Hello,  World!! "), "hello-world");
        assert_eq!(slugify("***"), "ticket");
        assert_eq!(slugify("Already-slug"), "already-slug");
    }
}
