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
    DatasetSummary, IcechunkRef, ListRefs, MetaError, is_icechunk_repo, summarize_grib,
    summarize_icechunk_at, summarize_netcdf, summarize_zarr,
};

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
/// An Icechunk summary produced here also carries the repo's full branch
/// and tag lists (unlike [`ndlook_render_html`]'s, which doesn't show
/// them), so the caller can offer a ref picker without a second call.
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
    // SAFETY: forwarding the same raw pointer this function received, under
    // the same non-null-or-valid-for-the-call contract `ndlook_render_html`
    // documents; `path_str` is used only within this call.
    let path_str = match unsafe { decode_path(path) } {
        Ok(path_str) => path_str,
        Err(message) => return error_card(message),
    };
    let path = Path::new(path_str);

    // The preview renders one ref's tree and history and never shows the
    // repo's other branches and tags, so it doesn't pay to enumerate them
    // -- see `ListRefs`.
    let summary = match summarize_path(path, None, ListRefs::No) {
        Ok(summary) => summary,
        Err(message) => return error_card(&message),
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
    // SAFETY: forwarding the same raw pointer this function received, under
    // the contract documented above; `path_str` is used only within this
    // call.
    let path_str = match unsafe { decode_path(path) } {
        Ok(path_str) => path_str,
        Err(message) => return error_envelope(message),
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
            // `IcechunkRef`'s `FromStr` owns both the `"kind:value"`
            // grammar and the message for anything that doesn't fit it,
            // which is already phrased for the caller.
            Ok(raw) => match raw.parse::<IcechunkRef>() {
                Ok(reference) => Some(reference),
                Err(err) => return error_envelope(&err.to_string()),
            },
            Err(_) => return error_envelope("The Icechunk ref was not valid UTF-8."),
        }
    };

    // Unlike the preview, the app's JSON consumer offers a ref picker, so
    // it does want the repo's branch and tag lists -- see `ListRefs`.
    match summarize_path(path, reference.as_ref(), ListRefs::Yes) {
        Ok(summary) => summary_envelope(&summary),
        Err(message) => error_envelope(&message),
    }
}

/// Decodes a raw path argument into a `&str`, or the user-facing message
/// explaining why it couldn't be.
///
/// Shared by both entry points so a NULL or non-UTF-8 path is reported
/// identically whichever one was called.
///
/// # Safety
///
/// `path`, if non-null, must point to a valid, NUL-terminated C string that
/// stays valid for as long as the returned `&str` is used.
unsafe fn decode_path<'a>(path: *const c_char) -> Result<&'a str, &'static str> {
    if path.is_null() {
        return Err("No file path was provided.");
    }
    // SAFETY: `path` is non-null per the check above, and the caller's
    // contract guarantees it is a valid, NUL-terminated C string for the
    // lifetime the caller asked for.
    let c_str = unsafe { CStr::from_ptr(path) };
    c_str
        .to_str()
        .map_err(|_| "The file path was not valid UTF-8.")
}

/// Reads whatever is at `path` into a [`DatasetSummary`], routing on what
/// the path actually is and reducing every failure to a user-facing
/// message.
///
/// Shared by both entry points: these messages are what
/// [`ndlook_render_html`] shows in its error card and what
/// [`ndlook_summarize_json`] returns in its error envelope, so the two
/// can't drift apart.
///
/// `reference` and `list_refs` reach the Icechunk reader and are ignored
/// for every other kind of path.
fn summarize_path(
    path: &Path,
    reference: Option<&IcechunkRef>,
    list_refs: ListRefs,
) -> Result<DatasetSummary, String> {
    let summary = if path.is_dir() {
        summarize_directory_store(path, reference, list_refs).ok_or_else(|| {
            "Unsupported folder: not a Zarr store or an Icechunk repository.".to_owned()
        })?
    } else {
        summarize_file(path).ok_or_else(|| match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => format!("Unsupported file type \".{ext}\"."),
            None => "Unsupported file: no recognizable file extension.".to_owned(),
        })?
    };
    summary.map_err(|err| format!("{err}"))
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
/// Icechunk repo (`None` means `main`'s tip), and `list_refs` says whether
/// to also enumerate that repo's branches and tags; both are ignored
/// entirely for a Zarr store, which has no version history to select from.
fn summarize_directory_store(
    path: &Path,
    icechunk_ref: Option<&IcechunkRef>,
    list_refs: ListRefs,
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
        return Some(summarize_icechunk_at(path, icechunk_ref, list_refs));
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

/// Builds the `{"summary": <summary>}` JSON envelope [`ndlook_summarize_json`]
/// returns on success.
///
/// The serialized summary is already a complete JSON object, so wrapping it
/// is plain concatenation: no second serialization pass over the whole tree,
/// and no fallible step that could turn a perfectly good summary into an
/// error.
fn summary_envelope(summary: &DatasetSummary) -> String {
    match serde_json::to_string(summary) {
        Ok(payload) => format!("{{\"summary\":{payload}}}"),
        Err(err) => error_envelope(&format!("failed to serialize summary: {err}")),
    }
}

/// Builds a `{"error": "<message>"}` JSON envelope. Used for every failure
/// mode in [`ndlook_summarize_json`], mirroring [`error_card`]'s role for
/// [`ndlook_render_html`]. Going through `serde_json` is what escapes a
/// message that contains quotes, backslashes or control characters.
fn error_envelope(message: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "error": message }))
        .unwrap_or_else(|_| FALLBACK_ERROR_JSON.to_owned())
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

    /// Runs `call` -- one of the C ABI entry points, or anything else that
    /// hands back a pointer they own -- and turns its result into an owned
    /// Rust `String`, asserting the never-NULL/always-UTF-8 half of the
    /// contract and freeing the pointer exactly once. `what` names the
    /// function in those assertions.
    fn c_string_result(what: &str, call: impl FnOnce() -> *mut c_char) -> String {
        let ptr = call();
        assert!(!ptr.is_null(), "{what} must never return NULL");
        // SAFETY: `ptr` is non-null per the assertion above and, per the
        // contract every caller of this helper relies on, points at a
        // NUL-terminated string produced by `string_to_c` that has not been
        // freed. It is freed exactly once, below, after the copy.
        unsafe {
            let owned = CStr::from_ptr(ptr)
                .to_str()
                .unwrap_or_else(|err| panic!("{what} must return valid UTF-8: {err}"))
                .to_owned();
            ndlook_free_string(ptr);
            owned
        }
    }

    /// Round-trips `path` through the C ABI.
    fn render(path: &str) -> String {
        let c_path = CString::new(path).expect("test path must not contain NUL bytes");
        // SAFETY: `c_path` is a valid, NUL-terminated C string kept alive
        // for the duration of the call.
        c_string_result("ndlook_render_html", || unsafe {
            ndlook_render_html(c_path.as_ptr())
        })
    }

    /// Round-trips `path`/`icechunk_ref` through the C ABI. A `None`
    /// `icechunk_ref` stays a null pointer rather than becoming a
    /// `CString`, since NULL is the documented "no ref given" input.
    fn summarize_json(path: &str, icechunk_ref: Option<&str>) -> String {
        let c_path = CString::new(path).expect("test path must not contain NUL bytes");
        let c_ref =
            icechunk_ref.map(|r| CString::new(r).expect("test ref must not contain NUL bytes"));
        let ref_ptr = c_ref.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
        // SAFETY: `c_path` is a valid, NUL-terminated C string kept alive
        // for the duration of the call; `ref_ptr` is either NULL or
        // likewise a valid, NUL-terminated C string kept alive via `c_ref`
        // for the same duration.
        c_string_result("ndlook_summarize_json", || unsafe {
            ndlook_summarize_json(c_path.as_ptr(), ref_ptr)
        })
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

    /// Parses `json` and returns its top-level `"summary"` object,
    /// panicking with the raw JSON if the envelope wasn't a success
    /// envelope.
    fn expect_summary(json: &str) -> serde_json::Value {
        let mut value: serde_json::Value = serde_json::from_str(json).expect("expected valid JSON");
        value
            .get_mut("summary")
            .unwrap_or_else(|| panic!("expected a \"summary\" field, got: {json}"))
            .take()
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
        // `ndlook_render_html`.
        let html = c_string_result("ndlook_render_html", || unsafe {
            ndlook_render_html(std::ptr::null())
        });

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
        let summary = expect_summary(&json);
        assert_eq!(summary["format"], "NetCdf");
        assert!(
            !json.contains("\"error\""),
            "a valid fixture must not also carry an \"error\" field, got: {json}"
        );
    }

    #[test]
    fn summarize_json_resolves_an_icechunk_tag() {
        let json = summarize_json(&fixture_path("icechunk_repo.icechunk"), Some("tag:v1"));
        let version_info = &expect_summary(&json)["version_info"];
        assert_eq!(version_info["branch"], "v1");
        assert_eq!(version_info["ref_kind"], "tag");
    }

    #[test]
    fn summarize_json_defaults_an_icechunk_repo_to_main() {
        let json = summarize_json(&fixture_path("icechunk_repo.icechunk"), None);
        let version_info = &expect_summary(&json)["version_info"];
        assert_eq!(version_info["branch"], "main");
        assert_eq!(version_info["ref_kind"], "branch");
    }

    /// The JSON path asks the Icechunk reader to enumerate refs (unlike the
    /// preview path), so the app has something to build a ref picker from.
    #[test]
    fn summarize_json_lists_an_icechunk_repos_branches_and_tags() {
        let json = summarize_json(&fixture_path("icechunk_repo.icechunk"), None);
        let version_info = &expect_summary(&json)["version_info"];
        assert_eq!(version_info["branches"], serde_json::json!(["main"]));
        assert_eq!(version_info["tags"], serde_json::json!(["v1"]));
    }

    /// The `snapshot:` arm, end to end: a real id read back out of a
    /// default summary must resolve, and must come back naming itself.
    #[test]
    fn summarize_json_resolves_a_bare_snapshot_id() {
        let path = fixture_path("icechunk_repo.icechunk");
        let default = summarize_json(&path, None);
        let tip = expect_summary(&default)["version_info"]["ancestry"][0]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a tip snapshot id, got: {default}"))
            .to_owned();

        let json = summarize_json(&path, Some(&format!("snapshot:{tip}")));
        let version_info = &expect_summary(&json)["version_info"];
        assert_eq!(version_info["ref_kind"], "snapshot");
        assert_eq!(version_info["branch"], tip);
        assert_eq!(version_info["ancestry"][0]["id"], tip);
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
        // SAFETY: NULL is an explicitly documented valid input for both of
        // `ndlook_summarize_json`'s arguments.
        let json = c_string_result("ndlook_summarize_json", || unsafe {
            ndlook_summarize_json(std::ptr::null(), std::ptr::null())
        });
        expect_error(&json);
    }

    #[test]
    fn summarize_json_returns_a_parseable_envelope_for_every_outcome() {
        for (path, icechunk_ref) in [
            (fixture_path("simple.nc"), None),
            (fixture_path("does-not-exist.nc"), None),
            (fixture_path("icechunk_repo.icechunk"), Some("tag:v1")),
            (fixture_path("icechunk_repo.icechunk"), Some("bogus")),
        ] {
            let json = summarize_json(&path, icechunk_ref);
            let value: serde_json::Value = serde_json::from_str(&json)
                .unwrap_or_else(|err| panic!("{path} must yield valid JSON, got {json:?}: {err}"));
            assert!(
                value.get("summary").is_some() || value.get("error").is_some(),
                "every envelope carries exactly one of summary/error, got: {json}"
            );
        }
    }

    /// The interior-NUL fallback, tested on [`string_to_c`] itself.
    ///
    /// It cannot be reached through the public entry points: nothing in the
    /// renderer or the serializer can emit a NUL, and reading the result
    /// back through `CStr` would stop at one anyway, so a test at that
    /// level could never fail. Here the returned pointer really is the
    /// fallback constant or the test fails.
    #[test]
    fn string_to_c_substitutes_the_fallback_for_content_with_an_interior_nul() {
        for fallback in [FALLBACK_ERROR_HTML, FALLBACK_ERROR_JSON] {
            let out = c_string_result("string_to_c", || {
                string_to_c("before\0after".to_owned(), fallback)
            });
            assert_eq!(out, fallback);
        }
    }
}
