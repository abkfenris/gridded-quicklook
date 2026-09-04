//! Markup assembly, ported from xarray's `xarray/core/formatting_html.py`.
//!
//! Functions here are named and ordered to mirror the Python source so the
//! port stays line-comparable. Notable, deliberate deviations from xarray:
//!
//! - No JavaScript is used anywhere; all interactivity is CSS-only
//!   (`<input type=checkbox>` + `<label>`), same as xarray itself.
//! - xarray generates `id` attributes with `uuid.uuid4()`. That's fine for
//!   a live notebook kernel but makes snapshot tests non-deterministic, so
//!   this port uses a small monotonic [`IdGen`] instead.
//! - `gridlook-meta`'s [`GroupSummary`] never carries bulk array data or a
//!   separate `Index` object (xarray's `xindexes`), only metadata. So:
//!   - "index coord" status is inferred structurally: a coord is an index
//!     coord if it is 1-D and its name equals its one dimension's name
//!     (xarray's own convention for dimension coordinates).
//!   - There is no "Indexes:" section (xarray's `index_section`) since we
//!     have no `Index` objects to summarize distinctly from coordinates.
//!   - The "data repr" disclosure shows the precomputed `preview` string
//!     (or `[metadata only]` when absent) instead of a full array repr,
//!     since we never load bulk data.
//! - `summarize_coords` preserves the order coordinates already appear in
//!   the model rather than re-deriving xarray's internal `_coord_sort_key`.
//! - The DataTree child-truncation/collapse heuristics in
//!   `_build_datatree_displays` (based on `OPTIONS["display_max_*"]`) are
//!   not ported; every child is always rendered, expanded.

use std::cell::Cell;
use std::collections::HashSet;
use std::fmt::Write as _;

use gridlook_meta::model::{AttrValue, DimInfo, GroupSummary, VarSummary};

/// Escapes the five characters HTML requires escaping in text/attribute
/// content, matching Python's `html.escape(s, quote=True)`.
pub fn html_escape(s: &str) -> String {
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

/// Monotonic id generator, standing in for xarray's `uuid.uuid4()` calls so
/// rendered output (and therefore snapshot tests) is deterministic.
pub struct IdGen(Cell<u64>);

impl IdGen {
    pub fn new() -> Self {
        Self(Cell::new(0))
    }

    pub fn next(&self, prefix: &str) -> String {
        let n = self.0.get();
        self.0.set(n + 1);
        format!("{prefix}-{n:04}")
    }
}

impl Default for IdGen {
    fn default() -> Self {
        Self::new()
    }
}

/// Names of coords that are structurally "index coords": 1-D, and their
/// name equals their sole dimension's name.
fn index_coord_names(coords: &[VarSummary]) -> HashSet<&str> {
    coords
        .iter()
        .filter(|v| v.dims.len() == 1 && v.dims[0] == v.name)
        .map(|v| v.name.as_str())
        .collect()
}

pub fn format_dims(dims: &[DimInfo], dims_with_index: &HashSet<&str>) -> String {
    if dims.is_empty() {
        return String::new();
    }
    let mut dims_li = String::new();
    for dim in dims {
        let cls = if dims_with_index.contains(dim.name.as_str()) {
            " class='xr-has-index'"
        } else {
            ""
        };
        let _ = write!(
            dims_li,
            "<li><span{cls}>{}</span>: {}</li>",
            html_escape(&dim.name),
            dim.size
        );
    }
    format!("<ul class='xr-dim-list'>{dims_li}</ul>")
}

/// Renders one [`AttrValue`] the way Python's `str()` would render the
/// equivalent scalar/list, since xarray's `summarize_attrs` just does
/// `str(v)` on each attribute value.
pub fn attr_value_display(v: &AttrValue) -> String {
    match v {
        AttrValue::Text(s) => s.clone(),
        AttrValue::Int(i) => i.to_string(),
        AttrValue::Float(f) => format_float(*f),
        AttrValue::IntList(items) => {
            let inner = items
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        AttrValue::FloatList(items) => {
            let inner = items
                .iter()
                .map(|f| format_float(*f))
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        AttrValue::TextList(items) => {
            let inner = items
                .iter()
                .map(|s| format!("'{s}'"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
    }
}

/// Formats a float the way Python's `str(float)` would (always with a
/// decimal point), since Rust's `Display` for `f64` omits it for whole
/// numbers (`1` instead of `1.0`).
fn format_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 {
        format!("{f:.1}")
    } else {
        f.to_string()
    }
}

pub fn summarize_attrs(attrs: &[(String, AttrValue)]) -> String {
    let mut attrs_dl = String::new();
    for (k, v) in attrs {
        let _ = write!(
            attrs_dl,
            "<dt><span>{} :</span></dt><dd>{}</dd>",
            html_escape(k),
            html_escape(&attr_value_display(v))
        );
    }
    format!("<dl class='xr-attrs'>{attrs_dl}</dl>")
}

fn icon(name: &str) -> String {
    format!("<svg class='icon xr-{name}'><use xlink:href='#{name}'></use></svg>")
}

pub fn summarize_variable(ids: &IdGen, name: &str, var: &VarSummary, is_index: bool) -> String {
    let cssclass_idx = if is_index {
        " class='xr-has-index'"
    } else {
        ""
    };
    let dims_str = format!(
        "({})",
        var.dims
            .iter()
            .map(|d| html_escape(d))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let name = html_escape(name);
    let dtype = html_escape(&var.dtype);

    let attrs_id = ids.next("attrs");
    let data_id = ids.next("data");
    let disabled = if var.attrs.is_empty() { "disabled" } else { "" };

    let preview_text = var.preview.as_deref().unwrap_or("[metadata only]");
    let preview = html_escape(preview_text);
    let attrs_ul = summarize_attrs(&var.attrs);

    // The data repr shows the same (escaped) preview text, prefixed with
    // the chunk shape when there is one.
    let mut data_body = preview.clone();
    if let Some(chunks) = &var.chunks {
        let chunks_str = chunks
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        data_body = format!("chunksize=({chunks_str})\n{data_body}");
    }
    let data_repr = format!("<pre>{data_body}</pre>");

    let attrs_icon = icon("icon-file-text2");
    let data_icon = icon("icon-database");

    format!(
        "<div class='xr-var-name'><span{cssclass_idx}>{name}</span></div>\
         <div class='xr-var-dims'>{dims_str}</div>\
         <div class='xr-var-dtype'>{dtype}</div>\
         <div class='xr-var-preview xr-preview'>{preview}</div>\
         <input id='{attrs_id}' class='xr-var-attrs-in' type='checkbox' {disabled}>\
         <label for='{attrs_id}' title='Show/Hide attributes'>{attrs_icon}</label>\
         <input id='{data_id}' class='xr-var-data-in' type='checkbox'>\
         <label for='{data_id}' title='Show/Hide data repr'>{data_icon}</label>\
         <div class='xr-var-attrs'>{attrs_ul}</div>\
         <div class='xr-var-data'>{data_repr}</div>"
    )
}

/// Shared body of `summarize_coords`/`summarize_vars`: renders a `<ul>` of
/// `summarize_variable` items, marking each as an index coord iff its name
/// is in `index_names`.
fn summarize_var_list(ids: &IdGen, vars: &[VarSummary], index_names: &HashSet<&str>) -> String {
    let mut vars_li = String::new();
    for v in vars {
        let li_content = summarize_variable(ids, &v.name, v, index_names.contains(v.name.as_str()));
        let _ = write!(vars_li, "<li class='xr-var-item'>{li_content}</li>");
    }
    format!("<ul class='xr-var-list'>{vars_li}</ul>")
}

pub fn summarize_coords(ids: &IdGen, coords: &[VarSummary]) -> String {
    let index_names = index_coord_names(coords);
    summarize_var_list(ids, coords, &index_names)
}

pub fn summarize_vars(ids: &IdGen, vars: &[VarSummary]) -> String {
    summarize_var_list(ids, vars, &HashSet::new())
}

#[allow(clippy::too_many_arguments)]
pub fn collapsible_section(
    ids: &IdGen,
    header: &str,
    inline_details: &str,
    details: &str,
    n_items: Option<usize>,
    enabled: bool,
    collapsed: bool,
    span_grid: bool,
) -> String {
    let data_id = ids.next("section");

    let has_items = n_items.is_some_and(|n| n > 0);
    let n_items_span = match n_items {
        None => String::new(),
        Some(n) => format!(" <span>({n})</span>"),
    };
    let enabled_attr = if enabled && has_items {
        ""
    } else {
        " disabled"
    };
    let collapsed_attr = if collapsed || !has_items {
        ""
    } else {
        " checked"
    };
    let span_grid_attr = if span_grid { " xr-span-grid" } else { "" };
    let tip = if enabled_attr.is_empty() {
        " title='Expand/collapse section'"
    } else {
        ""
    };

    let mut html = format!(
        "<input id='{data_id}' class='xr-section-summary-in' type='checkbox'{enabled_attr}{collapsed_attr} />\
         <label for='{data_id}' class='xr-section-summary{span_grid_attr}'{tip}>{header}{n_items_span}</label>\
         <div class='xr-section-inline-details'>{inline_details}</div>"
    );
    if !details.is_empty() {
        let _ = write!(html, "<div class='xr-section-details'>{details}</div>");
    }
    html
}

/// Port of xarray's `_mapping_section` + the four `partial(...)` sections
/// built from it (`coord_section`, `datavar_section`, `attr_section`).
/// xarray also derives `index_section` from this helper; we don't have an
/// analogous mapping to summarize (see module docs), so it's omitted.
fn mapping_section(
    ids: &IdGen,
    name: &str,
    n_items: usize,
    max_items_collapse: Option<usize>,
    details: String,
) -> String {
    let expanded = max_items_collapse.is_none_or(|max| n_items < max);
    let collapsed = !expanded;
    collapsible_section(
        ids,
        &format!("{name}:"),
        "",
        &details,
        Some(n_items),
        true,
        collapsed,
        false,
    )
}

pub fn dim_section(ids: &IdGen, dims: &[DimInfo], coords: &[VarSummary]) -> String {
    let index_names = index_coord_names(coords);
    let dim_list = format_dims(dims, &index_names);
    collapsible_section(ids, "Dimensions:", &dim_list, "", None, false, true, false)
}

pub fn coord_section(ids: &IdGen, coords: &[VarSummary]) -> String {
    mapping_section(
        ids,
        "Coordinates",
        coords.len(),
        Some(25),
        summarize_coords(ids, coords),
    )
}

pub fn datavar_section(ids: &IdGen, data_vars: &[VarSummary]) -> String {
    mapping_section(
        ids,
        "Data variables",
        data_vars.len(),
        Some(15),
        summarize_vars(ids, data_vars),
    )
}

pub fn attr_section(ids: &IdGen, attrs: &[(String, AttrValue)]) -> String {
    mapping_section(
        ids,
        "Attributes",
        attrs.len(),
        Some(10),
        summarize_attrs(attrs),
    )
}

pub fn sections_repr(sections: &[String]) -> String {
    let mut items = String::new();
    for s in sections {
        let _ = write!(items, "<li class='xr-section-item'>{s}</li>");
    }
    format!("<ul class='xr-sections'>{items}</ul>")
}

/// Plain-text fallback shown (via CSS, hidden by default) when the
/// stylesheet isn't injected — xarray's `xr-text-repr-fallback`. Kept
/// intentionally compact since we never hold bulk data to describe.
fn text_repr_fallback(obj_type: &str, group: &GroupSummary) -> String {
    let mut s = format!("<{obj_type}>");
    if !group.name.is_empty() {
        let _ = write!(s, " {}", group.name);
    }
    let _ = write!(
        s,
        "\nDimensions: {}",
        group
            .dims
            .iter()
            .map(|d| format!("{}: {}", d.name, d.size))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !group.coords.is_empty() {
        let _ = write!(
            s,
            "\nCoordinates: {}",
            group
                .coords
                .iter()
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !group.data_vars.is_empty() {
        let _ = write!(
            s,
            "\nData variables: {}",
            group
                .data_vars
                .iter()
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !group.attrs.is_empty() {
        let _ = write!(
            s,
            "\nAttributes: {}",
            group
                .attrs
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !group.children.is_empty() {
        let _ = write!(
            s,
            "\nGroups: {}",
            group
                .children
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    format!(
        "<pre class='xr-text-repr-fallback'>{}</pre>",
        html_escape(&s)
    )
}

fn obj_repr(header_components: &[String], sections: &[String], fallback: &str) -> String {
    let header = format!(
        "<div class='xr-header'>{}</div>",
        header_components.join("")
    );
    format!(
        "{fallback}<div class='xr-wrap' style='display:none'>{header}{}</div>",
        sections_repr(sections)
    )
}

/// Sections shown for one group: dims, coords, data vars, attrs. Shared by
/// the flat "Dataset" repr and each node of the "DataTree" repr.
fn group_sections(ids: &IdGen, group: &GroupSummary, always_show_dims: bool) -> Vec<String> {
    let mut sections = Vec::new();

    let show_dims = always_show_dims || !group.coords.is_empty() || !group.data_vars.is_empty();
    if show_dims {
        sections.push(dim_section(ids, &group.dims, &group.coords));
    }
    if !group.coords.is_empty() {
        sections.push(coord_section(ids, &group.coords));
    }
    if always_show_dims || !group.data_vars.is_empty() {
        sections.push(datavar_section(ids, &group.data_vars));
    }
    if !group.attrs.is_empty() {
        sections.push(attr_section(ids, &group.attrs));
    }
    sections
}

/// Shared body of `dataset_repr`/`datatree_repr`: assembles the header,
/// fallback, and given `sections` into the final top-level repr for `group`.
fn top_level_repr(obj_type: &str, group: &GroupSummary, sections: &[String]) -> String {
    let mut header_components = vec![format!("<div class='xr-obj-type'>{obj_type}</div>")];
    if !group.name.is_empty() {
        header_components.push(format!(
            "<div class='xr-obj-name'>{}</div>",
            html_escape(&group.name)
        ));
    }

    let fallback = text_repr_fallback(obj_type, group);
    obj_repr(&header_components, sections, &fallback)
}

/// Flat, xarray-`Dataset`-style repr for a group with no children. Mirrors
/// `dataset_repr` in the Python source.
pub fn dataset_repr(ids: &IdGen, group: &GroupSummary) -> String {
    let sections = group_sections(ids, group, true);
    top_level_repr("Dataset", group, &sections)
}

/// Total variable + attribute count for a group, including all descendant
/// groups. Mirrors `_tree_item_count`.
fn tree_item_count(group: &GroupSummary) -> usize {
    let own = group.coords.len() + group.data_vars.len() + group.attrs.len();
    let children: usize = group.children.iter().map(tree_item_count).sum();
    own + children
}

/// Sections for one DataTree node: an optional `children_section` (if it
/// has children) followed by its own dims/coords/data_vars/attrs sections.
/// Mirrors `datatree_sections`.
fn datatree_sections(ids: &IdGen, group: &GroupSummary) -> Vec<String> {
    let mut sections = Vec::new();
    if !group.children.is_empty() {
        sections.push(children_section(ids, group));
    }
    sections.extend(group_sections(ids, group, false));
    sections
}

/// Mirrors `children_section`: every child is rendered (no truncation —
/// see module docs).
fn children_section(ids: &IdGen, group: &GroupSummary) -> String {
    let n = group.children.len();
    let mut child_elements = String::new();
    for (i, child) in group.children.iter().enumerate() {
        let end = i == n - 1;
        child_elements.push_str(&datatree_child_repr(ids, child, end));
    }
    format!("<div class='xr-children'>{child_elements}</div>")
}

/// One child group box with its left-hand tee connector. Mirrors
/// `datatree_child_repr`.
fn datatree_child_repr(ids: &IdGen, node: &GroupSummary, end: bool) -> String {
    let vline_height = if end { "1.2em" } else { "100%" };
    let path = html_escape(&node.name);

    let group_id = ids.next("group");
    let item_count = tree_item_count(node);

    let sections = datatree_sections(ids, node);
    let sections_html = if sections.is_empty() {
        String::new()
    } else {
        sections_repr(&sections)
    };

    format!(
        "<div class='xr-group-box'>\
         <div class='xr-group-box-vline' style='height: {vline_height}'></div>\
         <div class='xr-group-box-hline'></div>\
         <div class='xr-group-box-contents'>\
         <input id='{group_id}' type='checkbox' />\
         <label for='{group_id}' title='Expand/collapse group'>{path} <span>({item_count})</span></label>\
         {sections_html}\
         </div>\
         </div>"
    )
}

/// Nested, xarray-`DataTree`-style repr for a group with children. Mirrors
/// `datatree_repr`.
pub fn datatree_repr(ids: &IdGen, root: &GroupSummary) -> String {
    let sections = datatree_sections(ids, root);
    top_level_repr("DataTree", root, &sections)
}
