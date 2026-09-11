//! `ndlook-ffi` is the C ABI entry point linked into the macOS QuickLook
//! app extension. It bridges the Swift/Objective-C preview and thumbnail
//! providers to the Rust core (`ndlook-meta` and `ndlook-html`).
//!
//! [`ndlook_render_html`]
//! turns a file path into a complete, self-contained HTML document, and
//! [`ndlook_free_string`] releases the string it returned. Failure
//! modes (a bad path, an unreadable file, an internal panic) are rendered as
//! styled HTML error cards rather than surfaced as a distinct error code,
//! so the Swift side never has to branch on anything but "do I have a
//! string to display".

use std::ffi::{CStr, CString};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::raw::c_char;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use ndlook_html::{html_escape, render_page};
use ndlook_meta::{
    DatasetSummary, IcechunkRef, MetaError, is_icechunk_repo, summarize_grib,
    summarize_icechunk_at, summarize_netcdf, summarize_zarr,
};
use serde::Serialize;

/// A fixed, dynamic-content-free fallback used only if we somehow fail to
/// build even the ordinary error card (e.g. because the underlying message
/// contained a NUL byte). This string is a compile-time constant, so it can
/// never itself trip that failure mode.
const FALLBACK_ERROR_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Preview unavailable</title></head><body><div class=\"gq-error\"><h1>Preview unavailable</h1><p>An internal error occurred while rendering this preview.</p></div></body></html>";

/// The JSON counterpart of [`FALLBACK_ERROR_HTML`], used only if we somehow
/// fail to build even the ordinary `{"error": ...}` envelope (e.g. because
/// the underlying message contained a NUL byte). This string is a
/// compile-time constant, so it can never itself trip that failure mode.
const FALLBACK_ERROR_JSON: &str =
    "{\"error\":\"An internal error occurred while summarizing this dataset.\"}";

/// File extensions (lowercased, without the leading dot) routed through
/// `ndlook-meta`'s NetCDF/HDF5 reader even when the file's signature
/// isn't recognized (see [`has_netcdf_signature`]), so that a truncated or
/// otherwise odd `.nc` still reaches libnetcdf and gets its diagnostic
/// rather than a generic "unsupported file type" card.
const NETCDF_LIKE_EXTENSIONS: &[&str] = &["nc", "nc4", "cdf", "h5", "hdf5", "he5"];

/// HDF5 file signature (the start of the superblock).
const HDF5_MAGIC: &[u8; 8] = b"\x89HDF\r\n\x1a\n";

/// File extensions routed through the GRIB reader when the file's
/// signature isn't recognized, the counterpart of
/// [`NETCDF_LIKE_EXTENSIONS`]. NCEP's own products carry no extension at
/// all, which is exactly why signature sniffing comes first.
const GRIB_EXTENSIONS: &[&str] = &["grib", "grib2", "grb", "grb2", "gb2"];

/// GRIB indicator-section signature, shared by both editions (the edition
/// number itself is byte 7).
const GRIB_MAGIC: &[u8; 4] = b"GRIB";

/// Root-level entries that mark a directory as a Zarr store: a v3 node
/// document, a v2 group marker, or v2 consolidated metadata. Checked with a
/// handful of `stat` calls -- never by walking the store.
const ZARR_ROOT_MARKERS: &[&str] = &["zarr.json", ".zgroup", ".zarray", ".zmetadata"];

/// Renders an HTML QuickLook preview for the file at `path`.
///
/// `path` must be a valid, NUL-terminated C string, or NULL. The returned
/// pointer is always non-null and always points to a valid, NUL-terminated
/// UTF-8 C string containing a complete HTML document -- on success, the
/// rendered preview; on any failure (bad input, unsupported file type,
/// unreadable file, or an internal panic), a small styled HTML error card
/// describing the problem. This function never panics across the FFI
/// boundary.
///
/// The caller owns the returned pointer and must release it with
/// [`ndlook_free_string`] exactly once.
///
/// # Safety
///
/// `path`, if non-null, must point to a valid, NUL-terminated C string that
/// remains valid for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndlook_render_html(path: *const c_char) -> *mut c_char {
    let html = panic::catch_unwind(AssertUnwindSafe(|| render_html_inner(path)))
        .unwrap_or_else(|_| error_card("ndlook-ffi panicked while rendering this preview."));
    string_to_c(html, FALLBACK_ERROR_HTML)
}

/// Summarizes the dataset at `path` as JSON.
///
/// `path` must be a valid, NUL-terminated C string, or NULL. `icechunk_ref`
/// selects which version of an Icechunk repository to preview:
///
/// - NULL means the default behavior -- `main`'s tip for an Icechunk repo,
///   and simply ignored for a regular file or a plain Zarr store, neither of
///   which has version history to select from.
/// - Non-NULL must be a valid, NUL-terminated C string of the form
///   `"kind:value"`, where `kind` is one of `branch`, `tag`, or `snapshot`
///   (e.g. `"branch:main"`, `"tag:v1"`, `"snapshot:ABC123"`); anything else
///   is a malformed-ref error. A well-formed ref given for a non-Icechunk
///   `path` is likewise ignored rather than treated as an error, matching
///   the NULL case -- the caller doesn't have to know what kind of store it
///   is pointing at before asking for its default preview.
///
/// The returned pointer is always non-null and always points to a valid,
/// NUL-terminated UTF-8 C string containing a JSON object: on success,
/// `{"summary": <summary>}`, where `<summary>` is a serialized
/// [`DatasetSummary`]; on any failure (bad input, a malformed
/// `icechunk_ref`, an unsupported file type, an unreadable file, or an
/// internal panic), `{"error": "<message>"}`. This function never panics
/// across the FFI boundary.
///
/// The caller owns the returned pointer and must release it with
/// [`ndlook_free_string`] exactly once.
///
/// # Safety
///
/// `path` and `icechunk_ref`, if non-null, must each point to a valid,
/// NUL-terminated C string that remains valid for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndlook_summarize_json(
    path: *const c_char,
    icechunk_ref: *const c_char,
) -> *mut c_char {
    // SAFETY: forwarding the same raw pointers this function received,
    // under the same non-null-or-valid-for-the-call contract documented
    // above.
    let json = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        summarize_json_inner(path, icechunk_ref)
    }))
    .unwrap_or_else(|_| error_envelope("ndlook-ffi panicked while summarizing this dataset."));
    string_to_c(json, FALLBACK_ERROR_JSON)
}

/// Releases a string previously returned by [`ndlook_render_html`] or
/// [`ndlook_summarize_json`].
///
/// Passing NULL is a no-op. Passing any other pointer not obtained from one
/// of those functions, or calling this more than once on the same pointer,
/// is undefined behavior.
///
/// # Safety
///
/// `ptr` must be either NULL or a pointer previously returned by
/// [`ndlook_render_html`] or [`ndlook_summarize_json`] that has not
/// already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndlook_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: `ptr` is either NULL (handled above) or, per this function's
    // contract, a pointer previously produced by `CString::into_raw` in
    // `string_to_c` and not yet freed.
    drop(unsafe { CString::from_raw(ptr) });
}

/// Does the actual work of turning a raw path pointer into an HTML string,
/// with every fallible step reduced to an error card rather than a `Result`
/// that could accidentally cross the FFI boundary.
fn render_html_inner(path: *const c_char) -> String {
    if path.is_null() {
        return error_card("No file path was provided.");
    }

    // SAFETY: `path` is non-null per the check above, and the caller's
    // contract guarantees it is a valid, NUL-terminated C string for the
    // duration of this call.
    let c_str = unsafe { CStr::from_ptr(path) };
    let path_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return error_card("The file path was not valid UTF-8."),
    };
    let path = Path::new(path_str);

    let summary = if path.is_dir() {
        match summarize_directory_store(path, None) {
            Some(result) => result,
            None => {
                return error_card(
                    "Unsupported folder: not a Zarr store or an Icechunk repository.",
                );
            }
        }
    } else {
        match summarize_file(path) {
            Some(result) => result,
            None => {
                return match path.extension().and_then(|e| e.to_str()) {
                    Some(ext) => error_card(&format!("Unsupported file type \".{ext}\".")),
                    None => error_card("Unsupported file: no recognizable file extension."),
                };
            }
        }
    };

    let summary = match summary {
        Ok(summary) => summary,
        Err(err) => return error_card(&format!("{err}")),
    };

    // Directory stores deliberately report no size: totalling one would mean
    // recursively stat-ing every chunk file, which for a real store can be
    // millions of entries. The renderer omits the size when it is `None`.
    let file_size = if path.is_dir() {
        None
    } else {
        fs::metadata(path).ok().map(|m| m.len())
    };
    let source_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path_str);

    render_page(&summary, source_name, file_size)
}

/// Does the actual work of turning raw path/ref pointers into a JSON
/// envelope string, with every fallible step reduced to an `Err` envelope
/// rather than a `Result` that could accidentally cross the FFI boundary.
///
/// # Safety
///
/// Same contract as [`ndlook_summarize_json`]: `path` and `icechunk_ref`,
/// if non-null, must each point to a valid, NUL-terminated C string that
/// remains valid for the duration of this call.
unsafe fn summarize_json_inner(path: *const c_char, icechunk_ref: *const c_char) -> String {
    if path.is_null() {
        return error_envelope("No file path was provided.");
    }

    // SAFETY: `path` is non-null per the check above, and the caller's
    // contract guarantees it is a valid, NUL-terminated C string for the
    // duration of this call.
    let c_str = unsafe { CStr::from_ptr(path) };
    let path_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return error_envelope("The file path was not valid UTF-8."),
    };
    let path = Path::new(path_str);

    let reference = if icechunk_ref.is_null() {
        None
    } else {
        // SAFETY: `icechunk_ref` is non-null per the check above, and the
        // caller's contract guarantees it is a valid, NUL-terminated C
        // string for the duration of this call.
        let ref_c_str = unsafe { CStr::from_ptr(icechunk_ref) };
        match ref_c_str.to_str() {
            Ok(s) => match parse_icechunk_ref(s) {
                Ok(reference) => Some(reference),
                Err(message) => return error_envelope(&message),
            },
            Err(_) => return error_envelope("The Icechunk ref was not valid UTF-8."),
        }
    };

    let summary = if path.is_dir() {
        match summarize_directory_store(path, reference.as_ref()) {
            Some(result) => result,
            None => {
                return error_envelope(
                    "Unsupported folder: not a Zarr store or an Icechunk repository.",
                );
            }
        }
    } else {
        match summarize_file(path) {
            Some(result) => result,
            None => {
                return match path.extension().and_then(|e| e.to_str()) {
                    Some(ext) => error_envelope(&format!("Unsupported file type \".{ext}\".")),
                    None => error_envelope("Unsupported file: no recognizable file extension."),
                };
            }
        }
    };

    match summary {
        Ok(summary) => match serde_json::to_string(&SummaryEnvelope::Summary {
            summary: Box::new(summary),
        }) {
            Ok(json) => json,
            Err(err) => error_envelope(&format!("failed to serialize summary: {err}")),
        },
        Err(err) => error_envelope(&format!("{err}")),
    }
}

/// Parses an `icechunk_ref` argument's `"kind:value"` form (e.g.
/// `"branch:main"`, `"tag:v1"`, `"snapshot:ABC123"`) into an
/// [`IcechunkRef`]. The `Err` string is a user-facing message suitable for
/// an error envelope as-is.
fn parse_icechunk_ref(raw: &str) -> Result<IcechunkRef, String> {
    let (kind, value) = raw.split_once(':').ok_or_else(|| {
        format!(
            "invalid Icechunk ref \"{raw}\": expected \"kind:value\" \
             (kind one of branch, tag, snapshot)"
        )
    })?;
    if value.is_empty() {
        return Err(format!(
            "invalid Icechunk ref \"{raw}\": value must not be empty"
        ));
    }
    match kind {
        "branch" => Ok(IcechunkRef::Branch(value.to_owned())),
        "tag" => Ok(IcechunkRef::Tag(value.to_owned())),
        "snapshot" => Ok(IcechunkRef::Snapshot(value.to_owned())),
        _ => Err(format!(
            "invalid Icechunk ref \"{raw}\": unknown kind \"{kind}\" \
             (expected branch, tag, or snapshot)"
        )),
    }
}

/// Routes a regular file by what it contains first (its signature), then
/// by extension. `None` means "no reader claims this file", which the
/// caller turns into an "unsupported file type" card.
///
/// Sniffing means anything Quick Look hands over previews regardless of
/// how its UTI happened to match, and the extension list stops having to
/// mirror the `UTTypeTagSpecification`s in the app's Info.plist.
fn summarize_file(path: &Path) -> Option<Result<DatasetSummary, MetaError>> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let netcdf_like_extension = extension
        .as_deref()
        .is_some_and(|ext| NETCDF_LIKE_EXTENSIONS.contains(&ext));

    if has_netcdf_signature(path) || netcdf_like_extension {
        return Some(summarize_netcdf(path));
    }

    let grib_extension = extension
        .as_deref()
        .is_some_and(|ext| GRIB_EXTENSIONS.contains(&ext));
    if has_grib_signature(path) || grib_extension {
        return Some(summarize_grib(path));
    }
    None
}

/// Does the file start with a GRIB indicator section?
///
/// Both editions begin with the ASCII bytes `GRIB` at offset 0, so one
/// four-byte read settles it. As with [`has_netcdf_signature`], any I/O
/// trouble simply reads as "no signature".
fn has_grib_signature(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    read_at(&mut file, 0, &mut magic) && &magic == GRIB_MAGIC
}

/// Does the file start like something libnetcdf can open?
///
/// - Classic netCDF: `CDF` followed by a version byte (1 = classic, 2 =
///   64-bit offset, 5 = CDF5) at offset 0.
/// - netCDF-4 / HDF5: the HDF5 superblock signature at offset 0, or, for a
///   file with a user block, at 512 · 2ⁿ. libhdf5 searches those offsets
///   until it runs off the end of the file; a handful of doublings covers
///   every user block size seen in practice, and each probe is one small
///   `read`.
///
/// Any I/O trouble simply reads as "no signature": the extension fallback
/// and, ultimately, libnetcdf's own error handling take it from there.
fn has_netcdf_signature(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 8];

    if read_at(&mut file, 0, &mut magic)
        && (matches!(&magic[..4], b"CDF\x01" | b"CDF\x02" | b"CDF\x05") || &magic == HDF5_MAGIC)
    {
        return true;
    }

    let mut offset = 512;
    for _ in 0..7 {
        if read_at(&mut file, offset, &mut magic) && &magic == HDF5_MAGIC {
            return true;
        }
        offset *= 2;
    }
    false
}

/// Fills `buf` from `offset`; `false` if the file is too short or unreadable.
fn read_at(file: &mut fs::File, offset: u64, buf: &mut [u8]) -> bool {
    file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(buf).is_ok()
}

/// Routes a directory by what it contains rather than by its extension:
/// Finder shows `.zarr`/`.icechunk` bundles as packages, but a store may
/// equally well be named anything at all. `None` means the directory is
/// neither an Icechunk repo nor a Zarr store.
///
/// `icechunk_ref` names the ref to preview when `path` turns out to be an
/// Icechunk repo (`None` means `main`'s tip); it is ignored entirely for a
/// Zarr store, which has no version history to select from.
fn summarize_directory_store(
    path: &Path,
    icechunk_ref: Option<&IcechunkRef>,
) -> Option<Result<DatasetSummary, MetaError>> {
    // Zarr's root markers are checked first: they are definitive files at
    // the root, while `is_icechunk_repo` is a directory-layout sniff that a
    // Zarr store could satisfy by coincidence (child groups named
    // `snapshots`/`transactions`/`refs`). An Icechunk repo root never
    // contains a Zarr root marker, so this ordering misroutes neither.
    if ZARR_ROOT_MARKERS
        .iter()
        .any(|marker| path.join(marker).is_file())
    {
        return Some(summarize_zarr(path));
    }
    if is_icechunk_repo(path) {
        return Some(summarize_icechunk_at(path, icechunk_ref));
    }
    None
}

/// Converts a Rust `String` into an owned, NUL-terminated C string pointer,
/// falling back to `fallback` -- a fixed, content-free error page or JSON
/// envelope -- in the (essentially impossible, but not `unwrap`-safe) case
/// that `content` contains an interior NUL byte.
fn string_to_c(content: String, fallback: &'static str) -> *mut c_char {
    match CString::new(content) {
        Ok(c_string) => c_string.into_raw(),
        Err(_) => CString::new(fallback)
            .expect("fallback is a fixed constant with no NUL bytes")
            .into_raw(),
    }
}

/// The JSON envelope returned by [`ndlook_summarize_json`]. `#[serde(untagged)]`
/// serializes whichever variant is constructed as a plain, tag-free object,
/// producing exactly `{"summary": ...}` or `{"error": "..."}`.
///
/// `Summary`'s field is boxed solely to keep the enum small: `DatasetSummary`
/// dwarfs `Error`'s `String`, and clippy flags that size gap on an enum that
/// is otherwise passed and matched on by value.
#[derive(Serialize)]
#[serde(untagged)]
enum SummaryEnvelope {
    Summary { summary: Box<DatasetSummary> },
    Error { error: String },
}

/// Builds a `{"error": "<message>"}` JSON envelope. Used for every failure
/// mode in [`ndlook_summarize_json`], mirroring [`error_card`]'s role for
/// [`ndlook_render_html`].
fn error_envelope(message: &str) -> String {
    let envelope = SummaryEnvelope::Error {
        error: message.to_owned(),
    };
    serde_json::to_string(&envelope).unwrap_or_else(|_| FALLBACK_ERROR_JSON.to_owned())
}

/// Renders a small, self-contained HTML document displaying `message` as a
/// styled error card. Used for every failure mode so the Swift side never
/// needs to distinguish "preview" from "error" -- it always just displays
/// whatever HTML string it gets back.
fn error_card(message: &str) -> String {
    let escaped = html_escape(message);
    format!(
        "<!doctype html>\
<html>\
<head>\
<meta charset=\"utf-8\">\
<title>Preview unavailable</title>\
<style>\
:root {{ color-scheme: light dark; }}\
body {{ font-family: -apple-system, BlinkMacSystemFont, sans-serif; margin: 0; padding: 24px; color: #1a1a1a; background: #fff; }}\
.gq-error {{ border: solid 1px #e0b4b4; background: #fdf2f2; border-radius: 6px; padding: 16px 20px; }}\
.gq-error h1 {{ margin: 0 0 8px 0; font-size: 1.1em; color: #a33; }}\
.gq-error p {{ margin: 0; font-family: ui-monospace, Menlo, monospace; font-size: 0.9em; white-space: pre-wrap; word-break: break-word; }}\
@media (prefers-color-scheme: dark) {{\
body {{ color: #f0f0f0; background: #111; }}\
.gq-error {{ border-color: #7a3b3b; background: #2a1717; }}\
.gq-error h1 {{ color: #ff8a80; }}\
}}\
</style>\
</head>\
<body>\
<div class=\"gq-error\">\
<h1>Preview unavailable</h1>\
<p>{escaped}</p>\
</div>\
</body>\
</html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    /// Round-trips `path` through the C ABI: builds a `CString`, calls
    /// `ndlook_render_html`, reads the result back out via `CStr`, frees
    /// it, and returns it as an owned Rust `String`.
    fn render(path: &str) -> String {
        let c_path = CString::new(path).expect("test path must not contain NUL bytes");
        // SAFETY: `c_path` is a valid, NUL-terminated C string kept alive
        // for the duration of the call; the returned pointer is freed
        // exactly once via `ndlook_free_string` below.
        unsafe {
            let ptr = ndlook_render_html(c_path.as_ptr());
            assert!(!ptr.is_null(), "ndlook_render_html must never return NULL");

            let html = CStr::from_ptr(ptr)
                .to_str()
                .expect("ndlook_render_html must return valid UTF-8")
                .to_owned();

            ndlook_free_string(ptr);
            html
        }
    }

    /// Round-trips `path`/`icechunk_ref` through the C ABI: builds the
    /// `CString`s (a `None` `icechunk_ref` stays a null pointer rather than
    /// a `CString`), calls `ndlook_summarize_json`, reads the result back
    /// out via `CStr`, frees it, and returns it as an owned Rust `String`.
    fn summarize_json(path: &str, icechunk_ref: Option<&str>) -> String {
        let c_path = CString::new(path).expect("test path must not contain NUL bytes");
        let c_ref =
            icechunk_ref.map(|r| CString::new(r).expect("test ref must not contain NUL bytes"));
        let ref_ptr = c_ref.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
        // SAFETY: `c_path` is a valid, NUL-terminated C string kept alive
        // for the duration of the call; `ref_ptr` is either NULL or
        // likewise a valid, NUL-terminated C string kept alive via `c_ref`
        // for the same duration. The returned pointer is freed exactly once
        // via `ndlook_free_string` below.
        unsafe {
            let ptr = ndlook_summarize_json(c_path.as_ptr(), ref_ptr);
            assert!(
                !ptr.is_null(),
                "ndlook_summarize_json must never return NULL"
            );

            let json = CStr::from_ptr(ptr)
                .to_str()
                .expect("ndlook_summarize_json must return valid UTF-8")
                .to_owned();

            ndlook_free_string(ptr);
            json
        }
    }

    fn fixture_path(relative: &str) -> String {
        format!(
            "{}/../../fixtures/data/{relative}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    #[test]
    fn round_trips_a_netcdf_fixture_through_the_c_abi() {
        let html = render(&fixture_path("simple.nc"));
        assert!(html.starts_with("<!doctype html>"));
        assert!(
            html.contains("xr-"),
            "expected xarray-style repr markup in successful output, got: {html}"
        );
        assert!(
            !html.contains("gq-error"),
            "a valid fixture must not render an error card"
        );
    }

    #[test]
    fn renders_a_zarr_directory_store() {
        let html = render(&fixture_path("simple_v3.zarr"));
        assert!(
            html.contains("xr-wrap"),
            "expected xarray-style repr markup for a Zarr store, got: {html}"
        );
        assert!(
            !html.contains("gq-error"),
            "a valid Zarr store must not render an error card"
        );
    }

    /// A Zarr store whose child nodes happen to be named like Icechunk
    /// internals must still be routed to the Zarr reader: the definitive
    /// root markers win over the icechunk directory-layout sniff.
    #[test]
    fn zarr_store_with_icechunk_like_children_routes_to_zarr() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let dir = tmp.path().join("ndlook_ffi_dispatch_test.zarr");
        for child in ["snapshots", "transactions"] {
            std::fs::create_dir_all(dir.join(child)).expect("create child dirs");
        }
        std::fs::write(
            dir.join("zarr.json"),
            r#"{"zarr_format":3,"node_type":"group","attributes":{}}"#,
        )
        .expect("write root zarr.json");

        let html = render(dir.to_str().expect("temp path is UTF-8"));
        assert!(
            !html.contains("gq-error"),
            "a Zarr store with icechunk-like child names must not error, got: {html}"
        );
        assert!(html.contains("xr-wrap"), "expected Zarr repr, got: {html}");
    }

    #[test]
    fn renders_an_icechunk_repository_with_its_version_history() {
        let html = render(&fixture_path("icechunk_repo.icechunk"));
        assert!(
            html.contains("xr-wrap"),
            "expected xarray-style repr markup for an Icechunk repo, got: {html}"
        );
        assert!(
            !html.contains("gq-error"),
            "a valid Icechunk repo must not render an error card"
        );
        assert!(
            html.contains("initial data"),
            "expected the version-history card to list the first commit message"
        );
        assert!(
            html.contains("update global attrs"),
            "expected the version-history card to list the latest commit message"
        );
    }

    /// Copies `fixture` into a fresh temp dir under `new_name`, renders it,
    /// and cleans up. Lets the routing tests exercise the signature sniff
    /// with extensions the extension list has never heard of.
    fn render_renamed_fixture(fixture: &str, new_name: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "ndlook_ffi_sniff_{}_{}",
            std::process::id(),
            new_name.replace('.', "_")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let copy = dir.join(new_name);
        std::fs::copy(fixture_path(fixture), &copy).expect("copy fixture");
        let html = render(copy.to_str().expect("temp path is UTF-8"));
        let _ = std::fs::remove_dir_all(&dir);
        html
    }

    #[test]
    fn netcdf4_file_with_unknown_extension_is_routed_by_its_hdf5_signature() {
        let html = render_renamed_fixture("simple.nc", "renamed.dat");
        assert!(
            html.contains("xr-wrap"),
            "expected a rendered preview, got: {html}"
        );
        assert!(!html.contains("gq-error"));
    }

    #[test]
    fn classic_netcdf_file_with_no_extension_is_routed_by_its_cdf_signature() {
        let html = render_renamed_fixture("simple_classic.nc", "no_extension");
        assert!(
            html.contains("xr-wrap"),
            "expected a rendered preview, got: {html}"
        );
        assert!(!html.contains("gq-error"));
    }

    /// A known extension still reaches libnetcdf even when the signature
    /// doesn't match, so the user sees libnetcdf's diagnostic rather than
    /// a generic "unsupported file type" card.
    #[test]
    fn netcdf_extension_without_a_signature_still_reaches_the_reader() {
        let html = render_renamed_fixture("../generate.py", "not_really.nc");
        assert!(html.contains("gq-error"));
        assert!(
            html.contains("failed to open"),
            "expected libnetcdf's open error, got: {html}"
        );
    }

    /// The GRIB sample is downloaded rather than generated, so it lives in
    /// `fixtures/samples` (mise task `samples`), not `fixtures/data`.
    fn sample_path(relative: &str) -> String {
        format!(
            "{}/../../fixtures/samples/{relative}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    const GRIB_SAMPLE: &str = "gfs.t00z.pgrb2.0p25.f003.sample.grib2";

    #[test]
    fn grib_file_renders_a_preview() {
        let html = render(&sample_path(GRIB_SAMPLE));
        assert!(
            html.contains("xr-wrap"),
            "expected a rendered preview, got: {html}"
        );
        assert!(!html.contains("gq-error"));
        assert!(html.contains("GRIB"), "expected the GRIB format badge");
    }

    /// NCEP publishes its GRIB products with no file extension at all, so
    /// the `GRIB` signature has to be what routes them.
    #[test]
    fn grib_file_with_no_extension_is_routed_by_its_signature() {
        // The copy's name must differ from every other renamed-fixture
        // test's: `render_renamed_fixture` derives its temp directory from
        // the name alone, and the tests run concurrently.
        let html =
            render_renamed_fixture(&format!("../samples/{GRIB_SAMPLE}"), "grib_no_extension");
        assert!(
            html.contains("xr-wrap"),
            "expected a rendered preview, got: {html}"
        );
        assert!(!html.contains("gq-error"));
    }

    /// A `.grib2` that holds no GRIB messages reaches the reader anyway,
    /// so the user sees what actually went wrong rather than a generic
    /// "unsupported file type" card.
    #[test]
    fn grib_extension_without_a_signature_still_reaches_the_reader() {
        let html = render_renamed_fixture("../generate.py", "not_really.grib2");
        assert!(html.contains("gq-error"));
        assert!(
            html.contains("no GRIB messages"),
            "expected the GRIB reader's own error, got: {html}"
        );
    }

    #[test]
    fn unrecognized_directory_renders_an_error_card() {
        let html = render(&fixture_path(".."));
        assert!(html.contains("gq-error"));
        assert!(html.contains("Unsupported folder"));
    }

    #[test]
    fn missing_file_renders_an_error_card() {
        let html = render(&fixture_path("does-not-exist.nc"));
        assert!(html.contains("Preview unavailable"));
        assert!(html.contains("gq-error"));
    }

    #[test]
    fn unsupported_extension_renders_an_error_card() {
        let html = render(&fixture_path("../generate.py"));
        assert!(html.contains("Preview unavailable"));
        assert!(html.contains("Unsupported file type"));
    }

    #[test]
    fn null_path_pointer_renders_an_error_card_instead_of_crashing() {
        // SAFETY: NULL is an explicitly documented valid input for
        // `ndlook_render_html`, and the returned pointer is freed exactly
        // once via `ndlook_free_string`.
        let html = unsafe {
            let ptr = ndlook_render_html(std::ptr::null());
            assert!(!ptr.is_null());
            let html = CStr::from_ptr(ptr).to_str().unwrap().to_owned();
            ndlook_free_string(ptr);
            html
        };

        assert!(html.contains("Preview unavailable"));
    }

    #[test]
    fn ndlook_free_string_handles_null_gracefully() {
        // SAFETY: NULL is an explicitly documented no-op input.
        unsafe { ndlook_free_string(std::ptr::null_mut()) };
    }

    #[test]
    fn rendered_output_never_contains_a_nul_byte() {
        for path in [fixture_path("simple.nc"), fixture_path("does-not-exist.nc")] {
            let html = render(&path);
            assert!(
                !html.as_bytes().contains(&0),
                "rendered HTML must never contain an embedded NUL byte"
            );
        }
    }

    #[test]
    fn summarize_json_returns_a_summary_envelope_for_a_netcdf_fixture() {
        let json = summarize_json(&fixture_path("simple.nc"), None);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap_or_else(|err| {
            panic!("ndlook_summarize_json must return valid JSON, got {json:?}: {err}")
        });
        let summary = value
            .get("summary")
            .unwrap_or_else(|| panic!("expected a \"summary\" field, got: {json}"));
        assert_eq!(summary["format"], "NetCdf");
        assert!(
            value.get("error").is_none(),
            "a valid fixture must not also carry an \"error\" field, got: {json}"
        );
    }

    #[test]
    fn summarize_json_resolves_an_icechunk_tag() {
        let json = summarize_json(&fixture_path("icechunk_repo.icechunk"), Some("tag:v1"));
        let value: serde_json::Value = serde_json::from_str(&json).expect("expected valid JSON");
        let version_info = &value["summary"]["version_info"];
        assert_eq!(version_info["branch"], "v1");
        assert_eq!(version_info["ref_kind"], "tag");
    }

    #[test]
    fn summarize_json_defaults_an_icechunk_repo_to_main() {
        let json = summarize_json(&fixture_path("icechunk_repo.icechunk"), None);
        let value: serde_json::Value = serde_json::from_str(&json).expect("expected valid JSON");
        let version_info = &value["summary"]["version_info"];
        assert_eq!(version_info["branch"], "main");
        assert_eq!(version_info["ref_kind"], "branch");
    }

    /// Parses `json` and returns its top-level `"error"` string, panicking
    /// with the raw JSON if the envelope wasn't an error envelope.
    fn expect_error(json: &str) -> String {
        let value: serde_json::Value = serde_json::from_str(json).expect("expected valid JSON");
        value
            .get("error")
            .unwrap_or_else(|| panic!("expected an \"error\" field, got: {json}"))
            .as_str()
            .expect("\"error\" field must be a string")
            .to_owned()
    }

    #[test]
    fn summarize_json_returns_an_error_envelope_for_a_missing_file() {
        let json = summarize_json(&fixture_path("does-not-exist.nc"), None);
        expect_error(&json);
    }

    #[test]
    fn summarize_json_returns_an_error_envelope_for_a_malformed_ref() {
        let json = summarize_json(&fixture_path("icechunk_repo.icechunk"), Some("bogus"));
        let error = expect_error(&json);
        assert!(
            error.contains("invalid Icechunk ref"),
            "expected a malformed-ref message, got: {error}"
        );
    }

    #[test]
    fn summarize_json_returns_an_error_envelope_for_an_unknown_branch() {
        let json = summarize_json(&fixture_path("icechunk_repo.icechunk"), Some("branch:nope"));
        expect_error(&json);
    }

    #[test]
    fn summarize_json_returns_an_error_envelope_for_a_null_path() {
        // SAFETY: NULL is an explicitly documented valid input for
        // `ndlook_summarize_json`, and the returned pointer is freed
        // exactly once via `ndlook_free_string`.
        let json = unsafe {
            let ptr = ndlook_summarize_json(std::ptr::null(), std::ptr::null());
            assert!(!ptr.is_null());
            let json = CStr::from_ptr(ptr).to_str().unwrap().to_owned();
            ndlook_free_string(ptr);
            json
        };
        expect_error(&json);
    }

    #[test]
    fn summarize_json_output_never_contains_a_nul_byte() {
        for (path, icechunk_ref) in [
            (fixture_path("simple.nc"), None),
            (fixture_path("does-not-exist.nc"), None),
            (fixture_path("icechunk_repo.icechunk"), Some("tag:v1")),
            (fixture_path("icechunk_repo.icechunk"), Some("bogus")),
        ] {
            let json = summarize_json(&path, icechunk_ref);
            assert!(
                !json.as_bytes().contains(&0),
                "summarize_json output must never contain an embedded NUL byte"
            );
        }
    }
}
