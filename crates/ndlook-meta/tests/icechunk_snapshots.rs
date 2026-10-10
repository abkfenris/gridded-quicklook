use std::path::{Path, PathBuf};

use ndlook_meta::is_icechunk_repo;

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

    use ndlook_meta::{
        IcechunkRef, ListRefs, SourceFormat, summarize_icechunk, summarize_icechunk_at,
    };

    /// Snapshot ids and commit timestamps are regenerated every time
    /// `mise run fixtures` runs, so they are redacted: the snapshot asserts
    /// the *structure* (hierarchy, dims, dtypes, attrs, commit messages and
    /// history length), not the volatile identifiers.
    #[test]
    fn icechunk_repo_snapshot() {
        let summary =
            summarize_icechunk(&fixture("icechunk_repo.icechunk")).expect("summarize repo");
        insta::assert_json_snapshot!(summary, {
            ".version_info.ancestry[].id" => "[snapshot-id]",
            ".version_info.ancestry[].wrote_at" => "[timestamp]",
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
        assert_eq!(version.ref_kind.as_deref(), Some("branch"));
        assert_eq!(version.ancestry.len(), 3);
        assert!(!version.truncated);

        // `summarize_icechunk` is the `ListRefs::No` path, so the repo's
        // branch/tag lists are deliberately left unfilled — see
        // `listing_refs_fills_the_branch_and_tag_lists` for the other half.
        assert!(version.branches.is_empty());
        assert!(version.tags.is_empty());

        // Newest first: the tip is the most recent commit.
        assert_eq!(
            version.ancestry[0].message.as_deref(),
            Some("update global attrs")
        );
        assert!(
            version.ancestry[0].wrote_at.is_some(),
            "the tip snapshot should carry a wrote_at timestamp"
        );

        let messages: Vec<&str> = version
            .ancestry
            .iter()
            .filter_map(|entry| entry.message.as_deref())
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

    /// The fixture repo has exactly one branch (`main`) and one tag (`v1`,
    /// pointing at the "initial data" commit) — see
    /// `write_icechunk_fixture` in fixtures/generate.py. Both lists are
    /// only collected when the caller asks for them.
    #[test]
    fn listing_refs_fills_the_branch_and_tag_lists() {
        let summary =
            summarize_icechunk_at(&fixture("icechunk_repo.icechunk"), None, ListRefs::Yes)
                .expect("summarize repo");

        let version = summary
            .version_info
            .as_ref()
            .expect("an Icechunk summary always carries version info");

        assert_eq!(version.branches, vec!["main".to_owned()]);
        assert_eq!(version.tags, vec!["v1".to_owned()]);
        // Listing refs must not disturb which ref was actually previewed.
        assert_eq!(version.branch, "main");
        assert_eq!(version.ref_kind.as_deref(), Some("branch"));
    }

    /// `summarize_icechunk` is documented as a thin wrapper over
    /// `summarize_icechunk_at(path, None, ListRefs::No)` that previews
    /// `main`'s tip; this pins that equivalence down as a regression test.
    ///
    /// Compared via their JSON encoding rather than `PartialEq` directly:
    /// the fixture's `_FillValue` attribute is `NaN`, and `NaN != NaN` under
    /// `f64`'s `PartialEq`, which would make two structurally identical
    /// summaries compare unequal.
    #[test]
    fn default_summary_matches_explicit_none_reference() {
        let path = fixture("icechunk_repo.icechunk");
        let via_wrapper = summarize_icechunk(&path).expect("summarize repo");
        let via_at = summarize_icechunk_at(&path, None, ListRefs::No).expect("summarize repo");

        assert_eq!(
            serde_json::to_string(&via_wrapper).unwrap(),
            serde_json::to_string(&via_at).unwrap()
        );
    }

    /// Opening at the `v1` tag (created before the "update global attrs"
    /// commit, see `write_icechunk_fixture` in fixtures/generate.py) should
    /// yield the *older* tree state: no `revision_note` attribute, and an
    /// ancestry that starts at "initial data" rather than "update global
    /// attrs".
    #[test]
    fn opening_at_a_tag_yields_the_older_tree_state() {
        let summary = summarize_icechunk_at(
            &fixture("icechunk_repo.icechunk"),
            Some(&IcechunkRef::Tag("v1".to_owned())),
            ListRefs::Yes,
        )
        .expect("summarize repo at tag v1");

        let version = summary
            .version_info
            .as_ref()
            .expect("an Icechunk summary always carries version info");

        assert_eq!(version.branch, "v1");
        assert_eq!(version.ref_kind.as_deref(), Some("tag"));
        assert_eq!(version.branches, vec!["main".to_owned()]);
        assert_eq!(version.tags, vec!["v1".to_owned()]);

        // Only "initial data" and the repo-init snapshot precede the tag.
        assert_eq!(version.ancestry.len(), 2);
        assert_eq!(version.ancestry[0].message.as_deref(), Some("initial data"));
        assert!(!version.truncated);

        let attr_names: Vec<&str> = summary
            .root
            .attrs
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert!(
            !attr_names.contains(&"revision_note"),
            "the v1 tag predates the commit that added revision_note, got {attr_names:?}"
        );
    }

    /// Resolving a branch that doesn't exist should surface as a clean
    /// `MetaError`, not a panic.
    #[test]
    fn opening_at_a_bogus_branch_errors_cleanly() {
        let result = summarize_icechunk_at(
            &fixture("icechunk_repo.icechunk"),
            Some(&IcechunkRef::Branch("does-not-exist".to_owned())),
            ListRefs::No,
        );

        assert!(result.is_err(), "expected an error, got {result:?}");
    }

    /// Crockford base32 decodes case-insensitively, so a lowercase snapshot
    /// id resolves fine — but the id it *displays* has to be the canonical
    /// (uppercase) spelling, or it won't match the ids in `ancestry`, which
    /// is how a UI locates the previewed snapshot in the history list.
    #[test]
    fn a_lowercase_snapshot_id_is_displayed_canonically() {
        let path = fixture("icechunk_repo.icechunk");
        let tip = summarize_icechunk(&path)
            .expect("summarize repo")
            .version_info
            .expect("an Icechunk summary always carries version info")
            .ancestry[0]
            .id
            .clone();
        let lowercase = tip.to_lowercase();
        assert_ne!(lowercase, tip, "the fixture's ids should not be lowercase");

        let summary =
            summarize_icechunk_at(&path, Some(&IcechunkRef::Snapshot(lowercase)), ListRefs::No)
                .expect("summarize repo at a lowercase snapshot id");

        let version = summary
            .version_info
            .as_ref()
            .expect("an Icechunk summary always carries version info");

        assert_eq!(version.ref_kind.as_deref(), Some("snapshot"));
        assert_eq!(version.branch, tip);
        assert_eq!(version.ancestry[0].id, tip);
    }
}
