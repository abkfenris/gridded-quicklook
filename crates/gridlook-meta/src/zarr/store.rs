//! The minimal key/value view of a Zarr store the metadata reader needs.
//!
//! A Zarr hierarchy is just a set of small JSON documents at well-known keys
//! (`zarr.json`, `.zgroup`, `.zarray`, `.zattrs`, `.zmetadata`) plus, for
//! stores without consolidated metadata, the ability to list a prefix to
//! discover child nodes. [`ZarrStore`] captures exactly that, so the same
//! reader walks a local directory ([`FsStore`]) or an object store.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::MetaError;

/// The immediate children of one prefix, as leaf names, sorted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Listing {
    pub dirs: Vec<String>,
    pub files: Vec<String>,
}

/// Read-only access to a Zarr store's metadata keys.
///
/// Keys are store-relative, `/`-separated, with no leading slash
/// (`"zarr.json"`, `"group_a/nested/.zarray"`); the empty prefix is the
/// store root.
pub(crate) trait ZarrStore {
    /// Human-readable root of the store (a path or URL), for error messages.
    fn location(&self) -> &str;

    /// The bytes at `key`, or `Ok(None)` when no such object exists.
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, MetaError>;

    /// Immediate children of `prefix`. Stores that cannot list (e.g. plain
    /// HTTP servers) return [`MetaError::ListingUnsupported`].
    fn list_dir(&self, prefix: &str) -> Result<Listing, MetaError>;

    /// Name for a store whose *root* is a single array: an array node
    /// otherwise has no name of its own. Defaults to the store location's
    /// last path segment minus its extension (`root.zarr` → `root`), or
    /// `"array"` when there is no usable segment (`/`, `.`, `.zarr`).
    fn root_name(&self) -> String {
        let location = self.location().trim_end_matches('/');
        let last = location.rsplit('/').next().unwrap_or(location);
        Path::new(last)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| !stem.is_empty() && stem != "." && stem != "..")
            .unwrap_or_else(|| "array".to_owned())
    }

    /// `"{location}/{key}"`, for error messages about one key.
    fn describe(&self, key: &str) -> String {
        if key.is_empty() {
            self.location().to_owned()
        } else {
            format!("{}/{key}", self.location().trim_end_matches('/'))
        }
    }
}

/// Joins a store-relative prefix and a child name into a key.
pub(crate) fn join_key(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

/// A Zarr store rooted at a local directory.
#[derive(Debug, Clone)]
pub(crate) struct FsStore {
    root: PathBuf,
    location: String,
}

impl FsStore {
    pub(crate) fn new(root: &Path) -> Self {
        FsStore {
            root: root.to_path_buf(),
            location: root.display().to_string(),
        }
    }

    fn key_path(&self, key: &str) -> PathBuf {
        if key.is_empty() {
            self.root.clone()
        } else {
            self.root.join(key)
        }
    }
}

impl ZarrStore for FsStore {
    fn location(&self) -> &str {
        &self.location
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, MetaError> {
        let path = self.key_path(key);
        // `is_file` first: `fs::read` on a directory fails with a
        // platform-specific error, but a directory at a metadata key simply
        // means "no such document".
        if !path.is_file() {
            return Ok(None);
        }
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(MetaError::Io {
                location: path.display().to_string(),
                source,
            }),
        }
    }

    /// Lists `prefix`'s children. Subdirectories are checked for symlink
    /// loops: `Path::is_dir` follows symlinks, so a link such as `loop -> .`
    /// inside a group makes `loop/zarr.json` resolve to the group's own
    /// metadata and would send the walk back into a directory it is already
    /// inside (until the kernel's `ELOOP` limit, after ~40 phantom nested
    /// copies). A child whose canonical path is the listed directory or one
    /// of its ancestors is therefore reported as a malformed store rather
    /// than listed. A link to a *sibling* is not a loop and is walked once
    /// more under the link's name.
    fn list_dir(&self, prefix: &str) -> Result<Listing, MetaError> {
        let dir = self.key_path(prefix);
        let io_err = |source: std::io::Error| MetaError::Io {
            location: dir.display().to_string(),
            source,
        };
        // If the path can't be canonicalized (odd filesystem), fall back to
        // the raw path: loop detection degrades, the walkers' depth cap
        // still holds.
        let canonical_dir = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        let mut listing = Listing::default();
        for entry in fs::read_dir(&dir).map_err(io_err)? {
            let entry = entry.map_err(io_err)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = entry.path();
            if !child.is_dir() {
                listing.files.push(name);
                continue;
            }
            let canonical_child = fs::canonicalize(&child).unwrap_or_else(|_| child.clone());
            if canonical_dir.starts_with(&canonical_child) {
                return Err(MetaError::Invalid {
                    location: child.display().to_string(),
                    message: format!(
                        "symlink loop: resolves to the enclosing group {}",
                        canonical_child.display()
                    ),
                });
            }
            listing.dirs.push(name);
        }
        listing.dirs.sort();
        listing.files.sort();
        Ok(listing)
    }

    fn root_name(&self) -> String {
        self.root
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| "array".to_owned())
    }
}

/// An in-memory store for tests: a map of keys to bytes, with listing that
/// can be switched off to mimic a plain HTTP server.
#[cfg(test)]
pub(crate) struct MemoryStore {
    pub entries: std::collections::BTreeMap<String, Vec<u8>>,
    pub can_list: bool,
    pub location: String,
}

#[cfg(test)]
impl MemoryStore {
    pub(crate) fn new(location: &str) -> Self {
        MemoryStore {
            entries: Default::default(),
            can_list: true,
            location: location.to_owned(),
        }
    }

    pub(crate) fn insert(&mut self, key: &str, value: impl AsRef<[u8]>) -> &mut Self {
        self.entries.insert(key.to_owned(), value.as_ref().to_vec());
        self
    }
}

#[cfg(test)]
impl ZarrStore for MemoryStore {
    fn location(&self) -> &str {
        &self.location
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, MetaError> {
        Ok(self.entries.get(key).cloned())
    }

    fn list_dir(&self, prefix: &str) -> Result<Listing, MetaError> {
        if !self.can_list {
            return Err(MetaError::ListingUnsupported {
                location: self.describe(prefix),
                message: "this test store cannot list".to_owned(),
            });
        }
        let prefix_slash = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        let mut listing = Listing::default();
        for key in self.entries.keys() {
            let Some(rest) = key.strip_prefix(&prefix_slash) else {
                continue;
            };
            match rest.split_once('/') {
                Some((dir, _)) => {
                    if !listing.dirs.iter().any(|d| d == dir) {
                        listing.dirs.push(dir.to_owned());
                    }
                }
                None if !rest.is_empty() => listing.files.push(rest.to_owned()),
                None => {}
            }
        }
        Ok(listing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gridlook-meta-store-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// `loop -> .` resolves to the listed directory itself: a loop.
    #[cfg(unix)]
    #[test]
    fn fs_store_reports_symlink_loops() {
        let dir = temp_dir("loop.zarr");
        std::os::unix::fs::symlink(".", dir.join("loop")).expect("create symlink loop");
        let err = FsStore::new(&dir)
            .list_dir("")
            .expect_err("a symlink loop must be an error");
        assert!(matches!(err, MetaError::Invalid { .. }), "{err:?}");
        assert!(err.to_string().contains("symlink loop"), "{err}");

        // A link pointing *up* out of a nested group is a loop too.
        fs::remove_file(dir.join("loop")).unwrap();
        fs::create_dir(dir.join("g")).unwrap();
        std::os::unix::fs::symlink("..", dir.join("g/up")).expect("create upward symlink");
        let err = FsStore::new(&dir)
            .list_dir("g")
            .expect_err("an upward symlink is a loop");
        assert!(err.to_string().contains("symlink loop"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A symlink to a sibling directory is finite and is listed normally.
    #[cfg(unix)]
    #[test]
    fn fs_store_lists_sibling_symlinks() {
        let dir = temp_dir("sibling.zarr");
        fs::create_dir(dir.join("a")).unwrap();
        std::os::unix::fs::symlink("a", dir.join("b")).expect("symlink b -> a");
        let listing = FsStore::new(&dir)
            .list_dir("")
            .expect("sibling symlink lists");
        assert_eq!(listing.dirs, vec!["a", "b"]);
        let _ = fs::remove_dir_all(&dir);
    }

    struct Located(&'static str);
    impl ZarrStore for Located {
        fn location(&self) -> &str {
            self.0
        }
        fn get(&self, _key: &str) -> Result<Option<Vec<u8>>, MetaError> {
            Ok(None)
        }
        fn list_dir(&self, _prefix: &str) -> Result<Listing, MetaError> {
            Ok(Listing::default())
        }
    }

    #[test]
    fn default_root_name_uses_last_segment_stem() {
        assert_eq!(Located("s3://bucket/data/root.zarr").root_name(), "root");
        assert_eq!(Located("s3://bucket/data/root.zarr/").root_name(), "root");
        assert_eq!(Located("https://host/plain").root_name(), "plain");
        assert_eq!(Located("/").root_name(), "array");
        assert_eq!(Located(".").root_name(), "array");
    }

    #[test]
    fn describe_joins_location_and_key() {
        assert_eq!(
            Located("s3://b/p/").describe("a/zarr.json"),
            "s3://b/p/a/zarr.json"
        );
        assert_eq!(Located("s3://b/p").describe(""), "s3://b/p");
    }

    #[test]
    fn join_key_skips_empty_prefix() {
        assert_eq!(join_key("", "zarr.json"), "zarr.json");
        assert_eq!(join_key("g", "zarr.json"), "g/zarr.json");
    }

    #[test]
    fn memory_store_lists_immediate_children() {
        let mut store = MemoryStore::new("mem://x");
        store
            .insert("zarr.json", b"{}")
            .insert("a/zarr.json", b"{}")
            .insert("a/b/zarr.json", b"{}")
            .insert("c/zarr.json", b"{}");
        let root = store.list_dir("").unwrap();
        assert_eq!(root.dirs, vec!["a", "c"]);
        assert_eq!(root.files, vec!["zarr.json"]);
        let a = store.list_dir("a").unwrap();
        assert_eq!(a.dirs, vec!["b"]);
    }
}
