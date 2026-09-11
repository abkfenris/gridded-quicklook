//! Icechunk repository metadata reader.
//!
//! Icechunk repos are version-controlled Zarr v3 hierarchies. This module
//! opens a *local-filesystem* repo read-only, resolves a ref — the `main`
//! branch's tip by default, or any [`IcechunkRef`] (branch, tag, or bare
//! snapshot) a caller names — to a snapshot, and turns that snapshot's node
//! metadata into the same format-agnostic [`DatasetSummary`] the other
//! readers produce — plus a [`VersionInfo`] describing the commit history
//! and the repo's full branch/tag lists.
//!
//! Each node's `user_data` is verbatim the Zarr v3 `zarr.json` document that
//! the writer stored, so the node conversion here reuses [`crate::zarr`]'s
//! v3 helpers rather than re-implementing them.
//!
//! Only metadata is read: chunk data is never fetched, and variable
//! `preview`s are always `None`, matching the plain-Zarr reader.
//!
//! Everything except [`is_icechunk_repo`] requires the `icechunk` cargo
//! feature.

use std::path::Path;

/// Cheap, dependency-free layout sniff: does `path` look like an Icechunk
/// repository root?
///
/// An Icechunk repo keeps immutable snapshot files under `snapshots/` and
/// its refs either in a `refs/` directory (spec v1) or in a single `repo`
/// file (spec v2), alongside a `transactions/` log. Checking for the
/// snapshot store plus one of the ref stores is enough to distinguish a repo
/// from a plain Zarr store or an unrelated directory, and costs a couple of
/// `stat` calls — no recursion, no store open.
///
/// This is deliberately available without the `icechunk` cargo feature so
/// callers can route on directory kind before paying for the reader.
pub fn is_icechunk_repo(path: &Path) -> bool {
    path.join("snapshots").is_dir()
        && (path.join("refs").is_dir()
            || path.join("repo").is_file()
            || path.join("transactions").is_dir())
}

#[cfg(feature = "icechunk")]
pub use enabled::{IcechunkRef, summarize_icechunk, summarize_icechunk_at};

#[cfg(feature = "icechunk")]
mod enabled {
    use std::collections::BTreeMap;
    use std::path::Path;

    use icechunk::Repository;
    use icechunk::format::snapshot::NodeSnapshot;
    use icechunk::format::{Path as IcePath, SnapshotId};
    use icechunk::repository::VersionInfo as IceVersionInfo;
    use zarrs_metadata::v3::NodeMetadataV3;

    use crate::error::MetaError;
    use crate::model::{DatasetSummary, GroupSummary, SnapshotInfo, SourceFormat, VersionInfo};
    use crate::zarr::build_v3_tree;

    /// The branch previewed for a repo when no [`IcechunkRef`] is given.
    /// Icechunk has no configurable default branch (unlike git), so `main`
    /// is the only tip worth resolving by default.
    const DEFAULT_BRANCH: &str = "main";

    /// A ref into an Icechunk repo's version history that a caller wants to
    /// preview: a branch tip, an immutable tag, or a specific snapshot.
    ///
    /// Mirrors the subset of [`IceVersionInfo`] meaningful for read-only
    /// browsing; the `AsOf` (browse-by-timestamp) variant is omitted since
    /// nothing in ndlook exposes that yet, and adding it later needs no
    /// changes to the other variants.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum IcechunkRef {
        Branch(String),
        Tag(String),
        Snapshot(String),
    }

    impl IcechunkRef {
        /// The name shown to users: the branch/tag name, or the raw
        /// snapshot id string. Used to fill [`VersionInfo::branch`] even
        /// when the ref isn't actually a branch — see that field's doc
        /// comment for why the field keeps its original name.
        fn display_name(&self) -> &str {
            match self {
                IcechunkRef::Branch(name)
                | IcechunkRef::Tag(name)
                | IcechunkRef::Snapshot(name) => name,
            }
        }

        /// Short label for [`VersionInfo::ref_kind`] and error messages.
        fn kind(&self) -> &'static str {
            match self {
                IcechunkRef::Branch(_) => "branch",
                IcechunkRef::Tag(_) => "tag",
                IcechunkRef::Snapshot(_) => "snapshot",
            }
        }

        /// Converts to the Icechunk crate's own ref type. A `Snapshot`'s id
        /// string is parsed into a real [`SnapshotId`] here (rather than
        /// left for `Repository::resolve_version` to reject) so a malformed
        /// id produces a message pointing at the id itself, not an opaque
        /// "not found" further down the pipeline.
        fn to_ice_version(&self, path: &Path) -> Result<IceVersionInfo, MetaError> {
            match self {
                IcechunkRef::Branch(name) => Ok(IceVersionInfo::BranchTipRef(name.clone())),
                IcechunkRef::Tag(name) => Ok(IceVersionInfo::TagRef(name.clone())),
                IcechunkRef::Snapshot(id) => {
                    let snapshot_id = SnapshotId::try_from(id.as_str()).map_err(|err| {
                        invalid(path, format!("invalid snapshot id \"{id}\": {err}"))
                    })?;
                    Ok(IceVersionInfo::SnapshotId(snapshot_id))
                }
            }
        }
    }

    /// Maximum number of snapshots (including the tip) walked back from the
    /// resolved ref's tip when building [`VersionInfo::ancestry`].
    ///
    /// Each step is a `lookup_snapshot`, and an Icechunk snapshot file
    /// carries the metadata of *every* node in the hierarchy, so a repo with
    /// thousands of arrays pays for a full multi-megabyte snapshot read per
    /// ancestor just to fill in a history footnote. The tip is already
    /// loaded for the read-only session (and cached), so the walk costs
    /// `ANCESTRY_LIMIT - 1` extra snapshot loads. Ten entries fill the
    /// scrollable ancestry list in the preview; anything older is summarized
    /// by the `N+` count.
    const ANCESTRY_LIMIT: usize = 10;

    /// Summarize the Zarr hierarchy at the tip of `main` in the Icechunk repo
    /// rooted at `path`, together with its commit history.
    ///
    /// A thin wrapper over [`summarize_icechunk_at`] with `reference: None`,
    /// kept so existing callers previewing the default branch don't need to
    /// name it.
    pub fn summarize_icechunk(path: &Path) -> Result<DatasetSummary, MetaError> {
        summarize_icechunk_at(path, None)
    }

    /// Summarize the Zarr hierarchy at `reference` (or `main`'s tip when
    /// `reference` is `None`) in the Icechunk repo rooted at `path`,
    /// together with its commit history and the repo's full branch/tag
    /// lists.
    ///
    /// The repo is opened read-only against local-filesystem storage; no
    /// network backends are contacted and no chunk data is read.
    ///
    /// The whole async pipeline is driven by a private current-thread Tokio
    /// runtime created per call, so this stays an ordinary blocking function
    /// callable straight from the FFI layer.
    pub fn summarize_icechunk_at(
        path: &Path,
        reference: Option<&IcechunkRef>,
    ) -> Result<DatasetSummary, MetaError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|source| MetaError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        runtime.block_on(summarize(path, reference))
    }

    async fn summarize(
        path: &Path,
        reference: Option<&IcechunkRef>,
    ) -> Result<DatasetSummary, MetaError> {
        let storage = icechunk::new_local_filesystem_storage(path)
            .await
            .map_err(|err| invalid(path, format!("cannot open Icechunk storage: {err}")))?;
        let repo = Repository::open(None, storage, Default::default())
            .await
            .map_err(|err| invalid(path, format!("cannot open Icechunk repository: {err}")))?;

        // `BTreeSet` already yields names in sorted order, matching
        // `VersionInfo::branches`/`tags`'s documented ordering.
        let branches: Vec<String> = repo
            .list_branches()
            .await
            .map_err(|err| invalid(path, format!("cannot list branches: {err}")))?
            .into_iter()
            .collect();
        let tags: Vec<String> = repo
            .list_tags()
            .await
            .map_err(|err| invalid(path, format!("cannot list tags: {err}")))?
            .into_iter()
            .collect();

        let default_ref = IcechunkRef::Branch(DEFAULT_BRANCH.to_owned());
        let reference = reference.unwrap_or(&default_ref);
        let ice_version = reference.to_ice_version(path)?;

        let tip = repo.resolve_version(&ice_version).await.map_err(|err| {
            invalid(
                path,
                format!(
                    "cannot resolve {} \"{}\": {err}",
                    reference.kind(),
                    reference.display_name()
                ),
            )
        })?;

        let (ancestry, truncated) = collect_ancestry(&repo, path, &tip).await?;

        let session = repo
            .readonly_session(&IceVersionInfo::SnapshotId(tip.clone()))
            .await
            .map_err(|err| invalid(path, format!("cannot open a read-only session: {err}")))?;

        let nodes: Vec<NodeSnapshot> = session
            .list_nodes(&IcePath::root())
            .await
            .map_err(|err| invalid(path, format!("cannot list repository nodes: {err}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| invalid(path, format!("cannot read a repository node: {err}")))?;

        let root = build_tree(path, &nodes)?;

        Ok(DatasetSummary {
            format: SourceFormat::Icechunk,
            root,
            version_info: Some(VersionInfo {
                branch: reference.display_name().to_owned(),
                ref_kind: Some(reference.kind().to_owned()),
                branches,
                tags,
                ancestry,
                truncated,
            }),
        })
    }

    /// Walks the parent chain from `tip` back to the repo's initial
    /// snapshot, newest first, capped at [`ANCESTRY_LIMIT`] entries
    /// (including the tip). Returns the walked snapshots plus whether the
    /// cap was hit before reaching the initial snapshot.
    ///
    /// Done by following `SnapshotInfo::parent_id` rather than with
    /// `Repository::ancestry`, which yields an async `Stream` and would drag
    /// in a `futures` dependency for a walk that is a handful of cheap
    /// lookups against already-cached repo metadata.
    async fn collect_ancestry(
        repo: &Repository,
        path: &Path,
        tip: &SnapshotId,
    ) -> Result<(Vec<SnapshotInfo>, bool), MetaError> {
        let mut entries = Vec::new();
        let mut cursor = Some(tip.clone());
        let mut truncated = false;
        while let Some(id) = cursor {
            if entries.len() >= ANCESTRY_LIMIT {
                truncated = true;
                break;
            }
            let info = repo
                .lookup_snapshot(&id)
                .await
                .map_err(|err| invalid(path, format!("cannot read snapshot {id}: {err}")))?;
            entries.push(SnapshotInfo {
                id: info.id.to_string(),
                message: if info.message.is_empty() {
                    None
                } else {
                    Some(info.message.clone())
                },
                wrote_at: Some(info.flushed_at.to_rfc3339()),
            });
            cursor = info.parent_id;
        }
        Ok((entries, truncated))
    }

    /// Rebuilds the group tree from icechunk's flat, fully-qualified node
    /// list.
    ///
    /// `list_nodes` returns every node in the snapshot (root included) keyed
    /// by absolute Zarr path. Each node's document is parsed exactly once
    /// into a map keyed by the plain-Zarr reader's relative-path convention,
    /// and [`build_v3_tree`] recovers the hierarchy from that.
    fn build_tree(path: &Path, nodes: &[NodeSnapshot]) -> Result<GroupSummary, MetaError> {
        let mut by_path: BTreeMap<String, NodeMetadataV3> = BTreeMap::new();
        for node in nodes {
            by_path.insert(
                normalize_path(&node.path.to_string()),
                parse_node(path, node)?,
            );
        }
        build_v3_tree(&by_path, path)
    }

    /// A node's `user_data` is the Zarr v3 `zarr.json` document verbatim, so
    /// it deserializes straight into the shared v3 node model.
    fn parse_node(repo_path: &Path, node: &NodeSnapshot) -> Result<NodeMetadataV3, MetaError> {
        serde_json::from_slice(&node.user_data).map_err(|source| MetaError::Json {
            path: repo_path.join(node.path.to_string().trim_start_matches('/')),
            source,
        })
    }

    /// Icechunk paths are absolute (`/`, `/group_a/nested`); strip the
    /// leading slash so the root becomes `""` and the prefix arithmetic
    /// above matches the plain-Zarr reader's relative-path convention.
    fn normalize_path(path: &str) -> String {
        path.trim_start_matches('/').to_owned()
    }

    fn invalid(path: &Path, message: String) -> MetaError {
        MetaError::Icechunk {
            path: path.to_path_buf(),
            message,
        }
    }
}
