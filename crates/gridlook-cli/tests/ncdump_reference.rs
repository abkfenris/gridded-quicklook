//! Checks `gridlook dump` against the reference implementation.
//!
//! Two tools from the netCDF-C distribution (`apt install netcdf-bin`,
//! `brew install netcdf`) drive this:
//!
//! - `ncdump`: when it is on PATH, `fixtures/generate.py` writes
//!   `fixtures/reference/<fixture>.cdl` (`ncdump -h`) and
//!   `<fixture>.s.cdl` (`ncdump -hs`) for every NetCDF/HDF5 fixture, and the
//!   first test requires our header to match byte for byte.
//! - `ncgen`: parses CDL back into a file, so the second test proves every
//!   header we print (Zarr and Icechunk included, which ncdump cannot read)
//!   is valid CDL.
//!
//! Both are optional on a developer machine: without them the tests skip
//! with a note. CI sets `GRIDLOOK_REQUIRE_NCDUMP=1`, which turns a skip
//! into a failure so the comparison cannot quietly stop running.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use similar::TextDiff;

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn tooling_required() -> bool {
    std::env::var_os("GRIDLOOK_REQUIRE_NCDUMP").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Notes a skipped check, or fails when the reference tooling is required.
fn skip(reason: &str) {
    if tooling_required() {
        panic!("{reason} (GRIDLOOK_REQUIRE_NCDUMP is set, so this is a failure)");
    }
    eprintln!("skipping: {reason}");
}

fn dump(args: &[&str], path: &Path) -> String {
    let output = Command::cargo_bin("gridlook")
        .expect("gridlook binary is built")
        .arg("dump")
        .args(args)
        .arg(path)
        .output()
        .expect("run gridlook");
    assert!(
        output.status.success(),
        "gridlook dump {args:?} {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8 stdout")
}

/// Fixtures ncdump itself can open: NetCDF and HDF5 files.
fn ncdump_readable(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension().and_then(OsStr::to_str),
            Some("nc" | "nc4" | "cdf" | "h5" | "hdf5")
        )
}

/// Sorted entries of a directory, or empty when it does not exist.
fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map(|iter| iter.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    entries
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(OsStr::to_str)
        .expect("fixture names are utf-8")
}

#[test]
fn headers_match_ncdump_reference_output() {
    let data_dir = fixtures_root().join("data");
    let references: Vec<PathBuf> = sorted_entries(&fixtures_root().join("reference"))
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "cdl"))
        .collect();
    if references.is_empty() {
        skip(
            "no fixtures/reference/*.cdl: ncdump was not on PATH when the fixtures were generated",
        );
        return;
    }

    // Every fixture ncdump can read must have both references, so a
    // generator regression cannot silently shrink the comparison.
    let reference_names: Vec<&str> = references.iter().map(|p| file_name(p)).collect();
    for fixture in sorted_entries(&data_dir) {
        if !ncdump_readable(&fixture) {
            continue;
        }
        for suffix in [".cdl", ".s.cdl"] {
            let wanted = format!("{}{suffix}", file_name(&fixture));
            assert!(
                reference_names.contains(&wanted.as_str()),
                "fixtures/reference/{wanted} is missing; rerun `mise run fixtures` with ncdump on PATH"
            );
        }
    }

    let mut failures = Vec::new();
    for reference in &references {
        let name = file_name(reference);
        let stem = name.strip_suffix(".cdl").expect("filtered on .cdl");
        let (fixture_name, ours_args, ncdump_flags) = match stem.strip_suffix(".s") {
            Some(fixture_name) => (fixture_name, &["-h", "-s"][..], "-hs"),
            None => (stem, &["-h"][..], "-h"),
        };
        let fixture = data_dir.join(fixture_name);
        assert!(
            fixture.exists(),
            "{name} has no matching fixture {fixture_name}; rerun `mise run fixtures`"
        );
        let expected = fs::read_to_string(reference).expect("read reference CDL");
        let ours = dump(ours_args, &fixture);
        if ours != expected {
            let diff = TextDiff::from_lines(&expected, &ours);
            failures.push(format!(
                "`gridlook dump {} {fixture_name}` differs from `ncdump {ncdump_flags}`:\n{}",
                ours_args.join(" "),
                diff.unified_diff().header("ncdump", "gridlook")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ncgen_parses_every_header() {
    match StdCommand::new("ncgen").arg("-h").output() {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            skip("ncgen is not on PATH");
            return;
        }
        Err(err) => panic!("running ncgen: {err}"),
    }

    let scratch = tempfile::tempdir().expect("temp dir");
    let mut failures = Vec::new();
    for fixture in sorted_entries(&fixtures_root().join("data")) {
        let name = file_name(&fixture);
        // ncgen validates `_Format` against netCDF's own kinds, so the `-s`
        // header (with `_Format = "Zarr v3"` and the like) is only checked
        // for fixtures that are netCDF/HDF5 files.
        let mut arg_sets: Vec<&[&str]> = vec![&["-h"]];
        if ncdump_readable(&fixture) {
            arg_sets.push(&["-h", "-s"]);
        }
        for args in arg_sets {
            let cdl = dump(args, &fixture);
            let cdl_path = scratch.path().join(format!("{name}{}.cdl", args.join("")));
            let out_path = cdl_path.with_extension("nc");
            fs::write(&cdl_path, &cdl).expect("write CDL");
            let output = StdCommand::new("ncgen")
                .args(["-k", "nc4", "-o"])
                .arg(&out_path)
                .arg(&cdl_path)
                .output()
                .expect("run ncgen");
            if !output.status.success() {
                failures.push(format!(
                    "ncgen rejected `gridlook dump {} {name}`:\n{}\n--- CDL ---\n{cdl}",
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim_end()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
