//! `gridlook-html` renders the metadata produced by `gridlook-meta` into an
//! HTML QuickLook preview, similar in spirit to xarray's `_repr_html_`.
//!
//! [`render_page`] is the public entry point: it produces a complete,
//! self-contained HTML document (inline CSS, inline SVG icon defs, no
//! JavaScript, no external references) suitable for use as a macOS
//! QuickLook preview or for saving straight to disk.

mod render;

pub use render::html_escape;

use gridlook_meta::model::{DatasetSummary, SourceFormat, VersionInfo};

/// xarray's HTML repr stylesheet, copied verbatim (see
/// `assets/xarray-style.css` for attribution) via `mise run
/// sync-xarray-assets` / `crates/gridlook-html/sync_xarray_assets.py`.
const XARRAY_CSS: &str = include_str!("../assets/xarray-style.css");

/// The inline `<svg>` icon defs xarray's repr markup references
/// (`#icon-database`, `#icon-file-text2`), copied verbatim alongside the
/// CSS above.
const XARRAY_ICONS_SVG: &str = include_str!("../assets/xarray-icons-svg-inline.html");

/// Extra styling for the parts of the page that aren't part of xarray's
/// repr (the footer and the Icechunk version-history card), namespaced
/// under `gq-` so it can't collide with xarray's `xr-*` classes.
const EXTRA_CSS: &str = r#"
.gq-footer {
  margin-top: 8px;
  padding-top: 6px;
  border-top: solid 1px var(--xr-border-color, #e0e0e0);
  font-size: 0.85em;
  color: var(--xr-font-color2, rgba(0, 0, 0, 0.54));
}
.gq-footer .gq-badge {
  display: inline-block;
  padding: 1px 6px;
  margin-right: 6px;
  border-radius: 3px;
  background: var(--xr-background-color-row-odd, #f0f0f0);
  font-weight: 500;
}
.gq-version-info {
  margin-top: 8px;
  padding: 6px 8px;
  border: solid 1px var(--xr-border-color, #e0e0e0);
  border-radius: 4px;
  font-size: 0.85em;
}
.gq-version-info dt {
  font-weight: 500;
  float: left;
  clear: left;
  margin-right: 6px;
}
.gq-version-info dd {
  margin: 0 0 2px 0;
}
.gq-ancestry {
  margin: 2px 0 0 0;
  padding-left: 1.2em;
  max-height: 8em;
  overflow-y: auto;
}
"#;

/// Renders a complete, self-contained HTML document previewing `summary`.
///
/// `source_name` is shown in the page `<title>` and footer (e.g. a file or
/// store name); `file_size`, if known, is shown as a human-readable size in
/// the footer.
pub fn render_page(summary: &DatasetSummary, source_name: &str, file_size: Option<u64>) -> String {
    let ids = render::IdGen::new();
    let repr_body = if summary.root.children.is_empty() {
        render::dataset_repr(&ids, &summary.root)
    } else {
        render::datatree_repr(&ids, &summary.root)
    };

    let footer = render_footer(summary, source_name, file_size);
    let title = render::html_escape(source_name);

    format!(
        "<!doctype html>\
<html>\
<head>\
<meta charset=\"utf-8\">\
<title>{title}</title>\
<style>{XARRAY_CSS}{EXTRA_CSS}</style>\
</head>\
<body>\
<div>{XARRAY_ICONS_SVG}{repr_body}</div>\
{footer}\
</body>\
</html>"
    )
}

fn format_badge(format: SourceFormat) -> &'static str {
    match format {
        SourceFormat::NetCdf => "netCDF",
        SourceFormat::Hdf5 => "HDF5",
        SourceFormat::ZarrV2 => "Zarr v2",
        SourceFormat::ZarrV3 => "Zarr v3",
        SourceFormat::Icechunk => "Icechunk",
    }
}

/// Formats a byte count as a human-readable binary (IEC) size, e.g.
/// `1.0 KiB`, `3.4 MiB`. Values under 1 KiB are shown as an exact byte
/// count.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = UNITS[0];
    for candidate in &UNITS[1..] {
        if size < 1024.0 {
            break;
        }
        size /= 1024.0;
        unit = candidate;
    }
    format!("{size:.1} {unit}")
}

fn render_footer(summary: &DatasetSummary, source_name: &str, file_size: Option<u64>) -> String {
    let mut footer = String::from("<div class='gq-footer'>");
    footer.push_str(&format!(
        "<span class='gq-badge'>{}</span>",
        render::html_escape(format_badge(summary.format))
    ));
    footer.push_str(&render::html_escape(source_name));
    if let Some(size) = file_size {
        footer.push_str(&format!(" &mdash; {}", human_size(size)));
    }
    footer.push_str("</div>");

    if let Some(v) = &summary.version_info {
        footer.push_str(&render_version_info(v));
    }

    footer
}

fn render_version_info(v: &VersionInfo) -> String {
    let mut s = String::from("<dl class='gq-version-info'>");
    s.push_str(&format!(
        "<dt>Branch</dt><dd>{}</dd>",
        render::html_escape(&v.branch)
    ));

    if let Some(tip) = v.ancestry.first() {
        let short_id: String = tip.id.chars().take(8).collect();
        s.push_str(&format!(
            "<dt>Snapshot</dt><dd><code>{}</code></dd>",
            render::html_escape(&short_id)
        ));
        if let Some(msg) = &tip.message {
            s.push_str(&format!(
                "<dt>Message</dt><dd>{}</dd>",
                render::html_escape(msg)
            ));
        }
        if let Some(ts) = &tip.wrote_at {
            s.push_str(&format!(
                "<dt>Written</dt><dd>{}</dd>",
                render::html_escape(ts)
            ));
        }
    }

    let count = if v.truncated {
        format!("{}+", v.ancestry.len())
    } else {
        v.ancestry.len().to_string()
    };
    s.push_str(&format!("<dt>Snapshots</dt><dd>{count}</dd>"));

    if !v.ancestry.is_empty() {
        s.push_str("<dt>Ancestry</dt><dd><ol class='gq-ancestry'>");
        for entry in &v.ancestry {
            let short: String = entry.id.chars().take(8).collect();
            match &entry.message {
                Some(m) => s.push_str(&format!(
                    "<li><code>{}</code> &mdash; {}</li>",
                    render::html_escape(&short),
                    render::html_escape(m)
                )),
                None => s.push_str(&format!(
                    "<li><code>{}</code></li>",
                    render::html_escape(&short)
                )),
            }
        }
        if v.truncated {
            s.push_str("<li>&hellip; (truncated)</li>");
        }
        s.push_str("</ol></dd>");
    }
    s.push_str("</dl>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_all_five_html_special_chars() {
        assert_eq!(
            render::html_escape(r#"a & b < c > d " e ' f"#),
            "a &amp; b &lt; c &gt; d &quot; e &#x27; f"
        );
    }

    #[test]
    fn html_escape_passes_through_plain_text() {
        assert_eq!(render::html_escape("plain text 123"), "plain text 123");
    }

    #[test]
    fn human_size_bytes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
    }

    #[test]
    fn human_size_kib() {
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1536), "1.5 KiB");
    }

    #[test]
    fn human_size_mib_and_gib() {
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MiB");
        assert_eq!(human_size(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    #[test]
    fn format_badge_covers_all_source_formats() {
        assert_eq!(format_badge(SourceFormat::NetCdf), "netCDF");
        assert_eq!(format_badge(SourceFormat::Hdf5), "HDF5");
        assert_eq!(format_badge(SourceFormat::ZarrV2), "Zarr v2");
        assert_eq!(format_badge(SourceFormat::ZarrV3), "Zarr v3");
        assert_eq!(format_badge(SourceFormat::Icechunk), "Icechunk");
    }
}
