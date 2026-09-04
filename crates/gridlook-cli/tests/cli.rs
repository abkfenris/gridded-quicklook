//! End-to-end tests of the `gridlook` binary against the generated fixtures.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/data")
        .join(name)
        .canonicalize()
        .unwrap_or_else(|err| panic!("fixture {name}: {err} (run `mise run fixtures`)"))
}

fn gridlook() -> Command {
    Command::cargo_bin("gridlook").expect("gridlook binary is built")
}

fn stdout_of(args: &[&str]) -> String {
    let output = gridlook().args(args).output().expect("run gridlook");
    assert!(
        output.status.success(),
        "gridlook {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8 stdout")
}

#[test]
fn dump_prints_the_header_and_a_note_without_dash_h() {
    gridlook()
        .args(["dump", fixture("simple.nc").to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "netcdf simple {\ndimensions:\n",
        ))
        .stdout(predicate::str::contains(
            "\tfloat temperature(time, x, y) ;\n",
        ))
        .stdout(predicate::str::ends_with("}\n"))
        .stderr(predicate::str::contains("header only"));
}

#[test]
fn dash_h_is_silent_and_identical() {
    let path = fixture("simple.nc");
    let plain = stdout_of(&["dump", path.to_str().unwrap()]);
    gridlook()
        .args(["dump", "-h", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(plain)
        .stderr(predicate::str::is_empty());
}

#[test]
fn dash_s_adds_special_attributes() {
    gridlook()
        .args(["dump", "-hs", fixture("simple.nc").to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\t\ttemperature:_Storage = \"chunked\" ;\n\t\ttemperature:_ChunkSizes = 2, 3, 2 ;\n",
        ))
        .stdout(predicate::str::contains("\t\t:_Format = \"netCDF-4\" ;\n"))
        .stdout(predicate::str::contains("\t\t:_IsNetcdf4 = 1 ;\n"));
}

#[test]
fn dash_k_prints_the_kind_for_every_format() {
    let cases = [
        ("simple.nc", "netCDF-4\n"),
        ("groups.nc", "netCDF-4\n"),
        ("simple_classic.nc", "classic\n"),
        ("simple_v2.zarr", "Zarr v2\n"),
        ("simple_v3.zarr", "Zarr v3\n"),
        ("tree.zarr", "Zarr v3\n"),
        ("icechunk_repo.icechunk", "Icechunk\n"),
    ];
    for (name, kind) in cases {
        let path = fixture(name);
        assert_eq!(
            stdout_of(&["dump", "-k", path.to_str().unwrap()]),
            kind,
            "{name}"
        );
    }
}

#[test]
fn dash_n_renames_the_dataset() {
    gridlook()
        .args([
            "dump",
            "-h",
            "-n",
            "other",
            fixture("simple.nc").to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("netcdf other {\n"));
}

#[test]
fn zarr_and_icechunk_directories_dump_as_cdl() {
    let zarr = stdout_of(&["dump", "-h", fixture("simple_v3.zarr").to_str().unwrap()]);
    assert!(zarr.starts_with("netcdf simple_v3 {\n"));
    assert!(zarr.contains("\tfloat temperature(time, x, y) ;\n"));

    let icechunk = stdout_of(&[
        "dump",
        "-hs",
        fixture("icechunk_repo.icechunk").to_str().unwrap(),
    ]);
    assert!(icechunk.contains("\t\t:_Format = \"Icechunk\" ;\n"));
    assert!(icechunk.contains("\t\t:_IcechunkBranch = \"main\" ;\n"));
}

#[test]
fn file_urls_read_the_same_as_paths() {
    let path = fixture("simple_v2.zarr");
    let by_path = stdout_of(&["dump", "-hs", path.to_str().unwrap()]);
    let url = format!("file://{}", path.display());
    let by_url = stdout_of(&["dump", "-hs", &url]);
    assert_eq!(by_path, by_url);
}

#[test]
fn group_filter_selects_subtrees() {
    let path = fixture("groups.nc");
    let text = stdout_of(&["dump", "-h", "-g", "nested", path.to_str().unwrap()]);
    assert!(text.contains("group: group_a {"), "ancestor wrapper kept");
    assert!(text.contains("  group: nested {"));
    assert!(!text.contains("group_b"), "unselected sibling omitted");
    assert!(
        !text.contains("\n// global attributes:"),
        "root contents omitted when the root is not selected"
    );

    gridlook()
        .args(["dump", "-h", "-g", "nope", path.to_str().unwrap()])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("group \"nope\" not found"));
}

#[test]
fn format_override_forces_a_reader() {
    gridlook()
        .args([
            "dump",
            "-h",
            "--source-format",
            "zarr",
            fixture("simple.nc").to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not a recognizable Zarr store"));
}

#[test]
fn unimplemented_ncdump_flags_are_rejected_clearly() {
    gridlook()
        .args(["dump", "-c", fixture("simple.nc").to_str().unwrap()])
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "-c (coordinate variable data) is not implemented yet",
        ));
    gridlook()
        .args([
            "dump",
            "-v",
            "temperature",
            fixture("simple.nc").to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("-v"));
}

#[test]
fn missing_and_unsupported_sources_fail_with_exit_1() {
    gridlook()
        .args([
            "dump",
            "-h",
            fixture("").join("does-not-exist.nc").to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("gridlook dump: failed to open"));
    gridlook()
        .args(["dump", "-h", fixture("../generate.py").to_str().unwrap()])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("unsupported file type \".py\""));
}

#[test]
fn help_and_version_work() {
    gridlook()
        .args(["dump", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("-h, --header"))
        .stdout(predicate::str::contains("-s, --special"))
        .stdout(predicate::str::contains("--anonymous"))
        .stdout(predicate::str::contains("s3://"));
    gridlook()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("gridlook "));
}
