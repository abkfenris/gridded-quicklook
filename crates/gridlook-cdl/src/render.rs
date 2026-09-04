//! Renders a [`DatasetSummary`] as an `ncdump -h`-style CDL header.
//!
//! Layout follows ncdump: `netcdf NAME {` ... `}`, sections `dimensions:` /
//! `variables:` / `// global attributes:` indented with tabs, attributes
//! with two tabs, nested groups as `group: NAME {` ... `} // group NAME`
//! blocks indented two spaces per level. Sections with nothing in them are
//! omitted, as ncdump does.

use std::fmt::Write as _;

use gridlook_meta::{AttrValue, DatasetSummary, GroupSummary, SourceFormat, VarSummary};

use crate::literal::{NumberPolicy, attr_literal, cdl_name};
use crate::specials::{global_specials, var_specials};
use crate::types::cdl_type_name;
use crate::{CdlError, CdlOptions};

/// Everything the recursive group writer needs besides the group itself.
struct Ctx<'a> {
    summary: &'a DatasetSummary,
    opts: &'a CdlOptions,
    policy: NumberPolicy,
}

pub(crate) fn render(summary: &DatasetSummary, opts: &CdlOptions) -> Result<String, CdlError> {
    let policy = match summary.format {
        SourceFormat::NetCdf | SourceFormat::Hdf5 => NumberPolicy::NetCdf,
        SourceFormat::ZarrV2 | SourceFormat::ZarrV3 | SourceFormat::Icechunk => NumberPolicy::Json,
    };
    let ctx = Ctx {
        summary,
        opts,
        policy,
    };

    if let Some(groups) = &opts.groups {
        for wanted in groups {
            if !group_exists(&summary.root, "", wanted) {
                return Err(CdlError::UnknownGroup(wanted.clone()));
            }
        }
    }

    let mut out = String::new();
    let _ = writeln!(out, "netcdf {} {{", cdl_name(&opts.name));
    let root_included = group_included(&ctx, "", "");
    write_group_body(&mut out, &ctx, &summary.root, 0, "", root_included);
    out.push_str("}\n");
    Ok(out)
}

/// Does `wanted` name this group or any descendant (by leaf name or full
/// path like `/group_a/nested`; `/` is the root)?
fn group_exists(group: &GroupSummary, path: &str, wanted: &str) -> bool {
    if matches_group(path, &group.name, wanted) {
        return true;
    }
    group
        .children
        .iter()
        .any(|child| group_exists(child, &format!("{path}/{}", child.name), wanted))
}

fn matches_group(path: &str, name: &str, wanted: &str) -> bool {
    let wanted = wanted.trim_end_matches('/');
    if path.is_empty() {
        return wanted.is_empty() || wanted == "/";
    }
    wanted == name
        || wanted == path
        || wanted.trim_start_matches('/') == path.trim_start_matches('/')
}

/// With a `-g` filter, a group's own contents print only when it (or an
/// ancestor, handled by the caller passing `true` down) is selected.
fn group_included(ctx: &Ctx, path: &str, name: &str) -> bool {
    match &ctx.opts.groups {
        None => true,
        Some(wanted) => wanted.iter().any(|w| matches_group(path, name, w)),
    }
}

/// Does this group or any descendant have contents that will print?
fn subtree_has_output(ctx: &Ctx, group: &GroupSummary, path: &str, included: bool) -> bool {
    if included {
        return true;
    }
    group.children.iter().any(|child| {
        let child_path = format!("{path}/{}", child.name);
        let child_included = group_included(ctx, &child_path, &child.name);
        subtree_has_output(ctx, child, &child_path, child_included)
    })
}

fn write_group_body(
    out: &mut String,
    ctx: &Ctx,
    group: &GroupSummary,
    depth: usize,
    path: &str,
    included: bool,
) {
    let indent = "  ".repeat(depth);

    if included {
        write_dimensions(out, group, &indent);
        write_variables(out, ctx, group, &indent);
        write_group_attributes(out, ctx, group, depth, &indent);
    }

    for child in &group.children {
        let child_path = format!("{path}/{}", child.name);
        let child_included = included || group_included(ctx, &child_path, &child.name);
        if !subtree_has_output(ctx, child, &child_path, child_included) {
            continue;
        }
        let _ = writeln!(out, "\n{indent}group: {} {{", cdl_name(&child.name));
        write_group_body(out, ctx, child, depth + 1, &child_path, child_included);
        let _ = writeln!(out, "{indent}  }} // group {}", cdl_name(&child.name));
    }
}

fn write_dimensions(out: &mut String, group: &GroupSummary, indent: &str) {
    if group.dims.is_empty() {
        return;
    }
    let _ = writeln!(out, "{indent}dimensions:");
    for dim in &group.dims {
        let name = cdl_name(&dim.name);
        if dim.is_unlimited {
            let _ = writeln!(
                out,
                "{indent}\t{name} = UNLIMITED ; // ({} currently)",
                dim.size
            );
        } else {
            let _ = writeln!(out, "{indent}\t{name} = {} ;", dim.size);
        }
    }
}

fn write_variables(out: &mut String, ctx: &Ctx, group: &GroupSummary, indent: &str) {
    let vars = group.variables_in_order();
    if vars.is_empty() {
        return;
    }
    let _ = writeln!(out, "{indent}variables:");
    for var in vars {
        write_variable(out, ctx, var, indent);
    }
}

fn write_variable(out: &mut String, ctx: &Ctx, var: &VarSummary, indent: &str) {
    let name = cdl_name(&var.name);
    let type_name = cdl_type_name(&var.dtype);
    if var.dims.is_empty() {
        let _ = writeln!(out, "{indent}\t{type_name} {name} ;");
    } else {
        let dims = var
            .dims
            .iter()
            .map(|d| cdl_name(d))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "{indent}\t{type_name} {name}({dims}) ;");
    }
    for (attr_name, value) in &var.attrs {
        write_attribute(out, ctx, indent, &name, attr_name, value);
    }
    if ctx.opts.specials {
        for (attr_name, value) in var_specials(var, ctx.summary.format) {
            write_attribute(out, ctx, indent, &name, &attr_name, &value);
        }
    }
}

fn write_group_attributes(
    out: &mut String,
    ctx: &Ctx,
    group: &GroupSummary,
    depth: usize,
    indent: &str,
) {
    let mut attrs: Vec<(String, AttrValue)> = group.attrs.clone();
    if ctx.opts.specials && depth == 0 {
        attrs.extend(global_specials(ctx.summary));
    }
    if attrs.is_empty() {
        return;
    }
    let heading = if depth == 0 {
        "global attributes"
    } else {
        "group attributes"
    };
    let _ = writeln!(out, "\n{indent}// {heading}:");
    for (attr_name, value) in &attrs {
        write_attribute(out, ctx, indent, "", attr_name, value);
    }
}

fn write_attribute(
    out: &mut String,
    ctx: &Ctx,
    indent: &str,
    owner: &str,
    attr_name: &str,
    value: &AttrValue,
) {
    let _ = writeln!(
        out,
        "{indent}\t\t{owner}:{} = {} ;",
        cdl_name(attr_name),
        attr_literal(value, ctx.policy)
    );
}

#[cfg(test)]
mod tests {
    use gridlook_meta::{DimInfo, FileInfo, GroupSummary, VarSummary};

    use super::*;

    fn dim(name: &str, size: u64, is_unlimited: bool) -> DimInfo {
        DimInfo {
            name: name.into(),
            size,
            is_unlimited,
        }
    }

    fn var(name: &str, dtype: &str, dims: &[&str], attrs: Vec<(&str, AttrValue)>) -> VarSummary {
        VarSummary {
            name: name.into(),
            dtype: dtype.into(),
            dims: dims.iter().map(|d| (*d).into()).collect(),
            shape: dims.iter().map(|_| 3).collect(),
            chunks: None,
            attrs: attrs.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            preview: None,
            storage: None,
        }
    }

    fn opts(name: &str) -> CdlOptions {
        CdlOptions {
            name: name.into(),
            specials: false,
            groups: None,
        }
    }

    fn dataset(root: GroupSummary) -> DatasetSummary {
        DatasetSummary {
            format: SourceFormat::NetCdf,
            root,
            version_info: None,
            file_info: None,
        }
    }

    #[test]
    fn renders_unlimited_dims_scalars_and_attribute_types() {
        let root = GroupSummary::from_parts(
            String::new(),
            Some(vec![dim("record", 3, true), dim("x", 3, false)]),
            vec![("title".into(), AttrValue::Text("t".into()))],
            vec![
                var("crs", "int32", &[], vec![("epsg", AttrValue::Int32(4326))]),
                var(
                    "temp",
                    "float32",
                    &["record", "x"],
                    vec![
                        ("_FillValue", AttrValue::Float32(f32::NAN)),
                        ("valid_range", AttrValue::Int16List(vec![-10, 40])),
                        ("time", AttrValue::Int(5)),
                    ],
                ),
            ],
            Vec::new(),
        );
        let text = render(&dataset(root), &opts("simple")).unwrap();
        let expected = "\
netcdf simple {
dimensions:
\trecord = UNLIMITED ; // (3 currently)
\tx = 3 ;
variables:
\tint crs ;
\t\tcrs:epsg = 4326 ;
\tfloat temp(record, x) ;
\t\ttemp:_FillValue = NaNf ;
\t\ttemp:valid_range = -10s, 40s ;
\t\ttemp:time = 5LL ;

// global attributes:
\t\t:title = \"t\" ;
}
";
        assert_eq!(text, expected);
    }

    #[test]
    fn empty_sections_are_omitted() {
        let root =
            GroupSummary::from_parts(String::new(), None, Vec::new(), Vec::new(), Vec::new());
        assert_eq!(
            render(&dataset(root), &opts("empty")).unwrap(),
            "netcdf empty {\n}\n"
        );
    }

    #[test]
    fn nested_groups_indent_two_spaces_per_level() {
        let nested = GroupSummary::from_parts(
            "nested".into(),
            None,
            vec![("note".into(), AttrValue::Text("deep".into()))],
            vec![var("v", "float64", &["z"], Vec::new())],
            Vec::new(),
        );
        let child =
            GroupSummary::from_parts("group_a".into(), None, Vec::new(), Vec::new(), vec![nested]);
        let root =
            GroupSummary::from_parts(String::new(), None, Vec::new(), Vec::new(), vec![child]);
        let text = render(&dataset(root), &opts("tree")).unwrap();
        let expected = "\
netcdf tree {

group: group_a {

  group: nested {
    dimensions:
    \tz = 3 ;
    variables:
    \tdouble v(z) ;

    // group attributes:
    \t\t:note = \"deep\" ;
    } // group nested
  } // group group_a
}
";
        assert_eq!(text, expected);
    }

    #[test]
    fn group_filter_prints_only_selected_subtrees() {
        let nested = GroupSummary::from_parts(
            "nested".into(),
            None,
            Vec::new(),
            vec![var("v", "float64", &["z"], Vec::new())],
            Vec::new(),
        );
        let child_a = GroupSummary::from_parts(
            "group_a".into(),
            None,
            Vec::new(),
            vec![var("a", "int8", &["z"], Vec::new())],
            vec![nested],
        );
        let child_b = GroupSummary::from_parts(
            "group_b".into(),
            None,
            Vec::new(),
            vec![var("b", "int8", &["z"], Vec::new())],
            Vec::new(),
        );
        let root = GroupSummary::from_parts(
            String::new(),
            None,
            Vec::new(),
            vec![var("r", "int8", &["z"], Vec::new())],
            vec![child_a, child_b],
        );
        let ds = dataset(root);

        let mut o = opts("t");
        o.groups = Some(vec!["nested".into()]);
        let text = render(&ds, &o).unwrap();
        assert!(!text.contains("byte r("), "root contents skipped");
        assert!(
            !text.contains("byte a("),
            "unselected ancestor contents skipped"
        );
        assert!(!text.contains("group_b"), "unrelated group skipped");
        assert!(text.contains("group: group_a {"), "ancestor wrapper kept");
        assert!(text.contains("double v(z)"), "selected group printed");

        o.groups = Some(vec!["/group_a".into()]);
        let text = render(&ds, &o).unwrap();
        assert!(text.contains("byte a("));
        assert!(
            text.contains("double v(z)"),
            "descendants of a selected group print"
        );

        o.groups = Some(vec!["missing".into()]);
        assert!(matches!(render(&ds, &o), Err(CdlError::UnknownGroup(g)) if g == "missing"));
    }

    #[test]
    fn specials_append_after_user_attributes() {
        let mut ds = dataset(GroupSummary::from_parts(
            String::new(),
            None,
            Vec::new(),
            vec![var("v", "float32", &["x"], Vec::new())],
            Vec::new(),
        ));
        ds.file_info = Some(FileInfo {
            kind: "netCDF-4".into(),
            nc_properties: Some("version=2".into()),
            superblock_version: Some(2),
            is_netcdf4: Some(true),
        });
        let mut o = opts("s");
        o.specials = true;
        let text = render(&ds, &o).unwrap();
        assert!(text.contains("\n// global attributes:\n\t\t:_Format = \"netCDF-4\" ;\n"));
        assert!(text.contains("\t\t:_NCProperties = \"version=2\" ;\n"));
        assert!(text.contains("\t\t:_SuperblockVersion = 2 ;\n"));
        assert!(text.contains("\t\t:_IsNetcdf4 = 1 ;\n"));
    }
}
