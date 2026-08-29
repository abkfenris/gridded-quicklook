//! `gridded-ffi` is the C ABI entry point linked into the macOS QuickLook
//! app extension. It bridges the Swift/Objective-C preview and thumbnail
//! providers to the Rust core (`gridded-meta` and `gridded-html`).
//!
//! [`gridded_render_html`]
//! turns a file path into a complete, self-contained HTML document, and
//! [`gridded_free_string`] releases the string it returned. Failure
//! modes (a bad path, an unreadable file, an internal panic) are rendered as
//! styled HTML error cards rather than surfaced as a distinct error code,
//! so the Swift side never has to branch on anything but "do I have a
//! string to display".

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use gridded_html::render_page;
use gridded_meta::summarize_netcdf;

/// A fixed, dynamic-content-free fallback used only if we somehow fail to
/// build even the ordinary error card (e.g. because the underlying message
/// contained a NUL byte). This string is a compile-time constant, so it can
/// never itself trip that failure mode.
const FALLBACK_ERROR_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Preview unavailable</title></head><body><div class=\"gq-error\"><h1>Preview unavailable</h1><p>An internal error occurred while rendering this preview.</p></div></body></html>";

/// File extensions (lowercased, without the leading dot) that we currently
/// route through `gridded-meta`'s NetCDF/HDF5 reader.
const NETCDF_LIKE_EXTENSIONS: &[&str] = &["nc", "nc4", "cdf", "h5", "hdf5", "he5"];

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
/// [`gridded_free_string`] exactly once.
///
/// # Safety
///
/// `path`, if non-null, must point to a valid, NUL-terminated C string that
/// remains valid for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn gridded_render_html(path: *const c_char) -> *mut c_char {
    let html = panic::catch_unwind(AssertUnwindSafe(|| render_html_inner(path)))
        .unwrap_or_else(|_| error_card("gridded-ffi panicked while rendering this preview."));
    string_to_c(html)
}

/// Releases a string previously returned by [`gridded_render_html`].
///
/// Passing NULL is a no-op. Passing any other pointer not obtained from
/// [`gridded_render_html`], or calling this more than once on the same
/// pointer, is undefined behavior.
///
/// # Safety
///
/// `ptr` must be either NULL or a pointer previously returned by
/// [`gridded_render_html`] that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn gridded_free_string(ptr: *mut c_char) {
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

    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);

    let is_netcdf_like = extension
        .as_deref()
        .is_some_and(|e| NETCDF_LIKE_EXTENSIONS.contains(&e));

    if !is_netcdf_like {
        return match extension {
            Some(ext) => error_card(&format!("Unsupported file type \".{ext}\".")),
            None => error_card("Unsupported file: no recognizable file extension."),
        };
    }

    let summary = match summarize_netcdf(path) {
        Ok(summary) => summary,
        Err(err) => return error_card(&format!("{err}")),
    };

    let file_size = std::fs::metadata(path).ok().map(|m| m.len());
    let source_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path_str);

    render_page(&summary, source_name, file_size)
}

/// Converts a Rust `String` into an owned, NUL-terminated C string pointer,
/// falling back to a fixed, content-free error page in the (essentially
/// impossible, but not `unwrap`-safe) case that the string contains an
/// interior NUL byte.
fn string_to_c(html: String) -> *mut c_char {
    match CString::new(html) {
        Ok(c_string) => c_string.into_raw(),
        Err(_) => CString::new(FALLBACK_ERROR_HTML)
            .expect("FALLBACK_ERROR_HTML is a fixed constant with no NUL bytes")
            .into_raw(),
    }
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
body {{ font-family: -apple-system, BlinkMacSystemFont, sans-serif; margin: 0; padding: 24px; color: #1a1a1a; background: #fff; }}\
.gq-error {{ border: solid 1px #e0b4b4; background: #fdf2f2; border-radius: 6px; padding: 16px 20px; }}\
.gq-error h1 {{ margin: 0 0 8px 0; font-size: 1.1em; color: #a33; }}\
.gq-error p {{ margin: 0; font-family: ui-monospace, Menlo, monospace; font-size: 0.9em; white-space: pre-wrap; word-break: break-word; }}\
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

/// Escapes the five HTML special characters. Kept local (rather than
/// depending on `gridded-html`'s private helper of the same name) since
/// this crate only needs it for the plain-text error message.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    /// Round-trips `path` through the C ABI: builds a `CString`, calls
    /// `gridded_render_html`, reads the result back out via `CStr`, frees
    /// it, and returns it as an owned Rust `String`.
    fn render(path: &str) -> String {
        let c_path = CString::new(path).expect("test path must not contain NUL bytes");
        // SAFETY: `c_path` is a valid, NUL-terminated C string kept alive
        // for the duration of the call; the returned pointer is freed
        // exactly once via `gridded_free_string` below.
        unsafe {
            let ptr = gridded_render_html(c_path.as_ptr());
            assert!(!ptr.is_null(), "gridded_render_html must never return NULL");

            let html = CStr::from_ptr(ptr)
                .to_str()
                .expect("gridded_render_html must return valid UTF-8")
                .to_owned();

            gridded_free_string(ptr);
            html
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
        // `gridded_render_html`, and the returned pointer is freed exactly
        // once via `gridded_free_string`.
        let html = unsafe {
            let ptr = gridded_render_html(std::ptr::null());
            assert!(!ptr.is_null());
            let html = CStr::from_ptr(ptr).to_str().unwrap().to_owned();
            gridded_free_string(ptr);
            html
        };

        assert!(html.contains("Preview unavailable"));
    }

    #[test]
    fn gridded_free_string_handles_null_gracefully() {
        // SAFETY: NULL is an explicitly documented no-op input.
        unsafe { gridded_free_string(std::ptr::null_mut()) };
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
}
