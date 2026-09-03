use std::path::{Path, PathBuf};

use gridded_meta::is_icechunk_repo;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/data")
        .join(name)
}

#[test]
fn icechunk_repo_is_detected_by_layout() {
    assert!(is_icechunk_repo(&fixture("icechunk_repo.icechunk")));
}

#[test]
fn plain_zarr_stores_are_not_icechunk_repos() {
    assert!(!is_icechunk_repo(&fixture("simple_v3.zarr")));
    assert!(!is_icechunk_repo(&fixture("simple_v2.zarr")));
    assert!(!is_icechunk_repo(&fixture("does-not-exist")));
}

#[cfg(feature = "icechunk")]
mod with_reader {
    use super::fixture;

    use gridded_meta::{summarize_icechunk, SourceFormat};

    /// Snapshot ids and commit timestamps are regenerated every time
    /// `mise run fixtures` runs, so they are redacted: the snapshot asserts
    /// the *structure* (hierarchy, dims, dtypes, attrs, commit messages and
    /// history length), not the volatile identifiers.
    #[test]
    fn icechunk_repo_snapshot() {
        let summary =
            summarize_icechunk(&fixture("icechunk_repo.icechunk")).expect("summarize repo");
        insta::assert_json_snapshot!(summary, {
            ".version_info.snapshot_id" => "[snapshot-id]",
            ".version_info.wrote_at" => "[timestamp]",
            ".version_info.ancestry[][0]" => "[snapshot-id]",
        });
    }

    /// The fixture repo has two commits on `main` on top of the snapshot
    /// Icechunk creates when the repository is initialized.
    #[test]
    fn version_info_describes_the_main_branch_history() {
        let summary =
            summarize_icechunk(&fixture("icechunk_repo.icechunk")).expect("summarize repo");

        assert_eq!(summary.format, SourceFormat::Icechunk);

        let version = summary
            .version_info
            .as_ref()
            .expect("an Icechunk summary always carries version info");

        assert_eq!(version.branch, "main");
        assert_eq!(version.ancestry.len(), 3);
        assert_eq!(version.n_snapshots, 3);

        // Newest first: the tip is the most recent commit.
        assert_eq!(version.message.as_deref(), Some("update global attrs"));
        assert_eq!(version.snapshot_id, version.ancestry[0].0);

        let messages: Vec<&str> = version
            .ancestry
            .iter()
            .filter_map(|(_, message)| message.as_deref())
            .collect();
        assert!(
            messages.contains(&"update global attrs"),
            "ancestry should contain the second commit, got {messages:?}"
        );
        assert!(
            messages.contains(&"initial data"),
            "ancestry should contain the first commit, got {messages:?}"
        );
    }

    /// The repo holds the same logical dataset as the plain Zarr fixtures,
    /// so the hierarchy conversion should agree with the Zarr v3 reader.
    #[test]
    fn hierarchy_matches_the_equivalent_zarr_fixture() {
        let summary =
            summarize_icechunk(&fixture("icechunk_repo.icechunk")).expect("summarize repo");

        let coord_names: Vec<&str> = summary
            .root
            .coords
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        let data_var_names: Vec<&str> = summary
            .root
            .data_vars
            .iter()
            .map(|v| v.name.as_str())
            .collect();

        assert_eq!(coord_names, vec!["time", "x"]);
        assert_eq!(data_var_names, vec!["salinity", "temperature"]);
        assert!(summary.root.children.is_empty());
    }
}
