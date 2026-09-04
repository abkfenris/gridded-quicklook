//! Rebuilding a group tree from a *flat* list of nodes keyed by path.
//!
//! Three readers end up with a flat node list rather than a directory to
//! walk: Zarr v2 consolidated metadata (`.zmetadata`), Zarr v3 consolidated
//! metadata (the root `zarr.json`'s `consolidated_metadata` field), and
//! Icechunk (whose sessions list every node in a snapshot). They all share
//! this builder, which recovers the hierarchy from path prefixes and also
//! discovers *implicit* intermediate groups that carry no node entry of
//! their own (e.g. `g` when only `g/arr` is listed).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::model::{GroupSummary, VarSummary};

use super::build_group_summary;

/// One node of a flattened hierarchy, keyed externally by its
/// store-relative path (`""` for the root, `"g/arr"` below it).
#[derive(Debug, Clone)]
pub(crate) enum FlatNode {
    Group {
        attrs: serde_json::Map<String, Value>,
    },
    /// An array, already summarized (its `name` is the path's leaf).
    Array(Box<VarSummary>),
}

/// Builds the root [`GroupSummary`] from `nodes`. A missing `""` entry is
/// treated as an attribute-less root group; an `Array` at `""` is a
/// root-is-array store, which callers handle before getting here (it is
/// treated as an empty root group otherwise).
pub(crate) fn build_tree_from_flat(nodes: &BTreeMap<String, FlatNode>) -> GroupSummary {
    let empty = serde_json::Map::new();
    let root_attrs = match nodes.get("") {
        Some(FlatNode::Group { attrs }) => attrs,
        _ => &empty,
    };
    build_group("", String::new(), root_attrs, nodes)
}

fn build_group(
    node_path: &str,
    name: String,
    attrs: &serde_json::Map<String, Value>,
    nodes: &BTreeMap<String, FlatNode>,
) -> GroupSummary {
    let prefix = if node_path.is_empty() {
        String::new()
    } else {
        format!("{node_path}/")
    };

    let mut vars = Vec::new();
    // Direct-child group paths → leaf names. Discovered from *every* key
    // under this prefix (not just group entries), so a deeper array path
    // like `g/arr` still names `g` as a child group even when `g` itself has
    // no entry; only direct-child arrays are excluded, since they are
    // variables of this group rather than subgroups.
    let mut child_groups: BTreeMap<String, String> = BTreeMap::new();
    for (path, node) in nodes {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        match rest.split_once('/') {
            None => match node {
                FlatNode::Array(var) => vars.push((**var).clone()),
                FlatNode::Group { .. } => {
                    child_groups.insert(path.clone(), rest.to_owned());
                }
            },
            Some((first, _)) => {
                child_groups.insert(format!("{prefix}{first}"), first.to_owned());
            }
        }
    }

    let empty = serde_json::Map::new();
    let children = child_groups
        .into_iter()
        .map(|(child_path, child_name)| {
            let child_attrs = match nodes.get(&child_path) {
                Some(FlatNode::Group { attrs }) => attrs,
                _ => &empty,
            };
            build_group(&child_path, child_name, child_attrs, nodes)
        })
        .collect();

    build_group_summary(name, attrs, vars, children)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str) -> VarSummary {
        VarSummary {
            name: name.to_owned(),
            dtype: "float32".to_owned(),
            dims: vec!["x".to_owned()],
            shape: vec![2],
            chunks: None,
            attrs: Vec::new(),
            preview: None,
            storage: None,
        }
    }

    fn group(attrs: &[(&str, &str)]) -> FlatNode {
        FlatNode::Group {
            attrs: attrs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), Value::String((*v).to_owned())))
                .collect(),
        }
    }

    #[test]
    fn rebuilds_nested_groups_and_implicit_intermediates() {
        let mut nodes = BTreeMap::new();
        nodes.insert(String::new(), group(&[("title", "root")]));
        nodes.insert("a".to_owned(), FlatNode::Array(Box::new(var("a"))));
        nodes.insert("g/arr".to_owned(), FlatNode::Array(Box::new(var("arr"))));
        nodes.insert("g/sub".to_owned(), group(&[("note", "sub")]));
        nodes.insert(
            "g/sub/deep".to_owned(),
            FlatNode::Array(Box::new(var("deep"))),
        );

        let root = build_tree_from_flat(&nodes);
        assert_eq!(root.attrs.len(), 1);
        assert_eq!(root.data_vars.len(), 1);
        assert_eq!(root.children.len(), 1);
        let g = &root.children[0];
        assert_eq!(g.name, "g");
        assert!(g.attrs.is_empty(), "implicit group has no attrs");
        assert_eq!(g.data_vars[0].name, "arr");
        let sub = &g.children[0];
        assert_eq!(sub.name, "sub");
        assert_eq!(sub.attrs.len(), 1);
        assert_eq!(sub.data_vars[0].name, "deep");
    }

    #[test]
    fn missing_root_entry_is_an_empty_root_group() {
        let mut nodes = BTreeMap::new();
        nodes.insert("x".to_owned(), FlatNode::Array(Box::new(var("x"))));
        let root = build_tree_from_flat(&nodes);
        assert!(root.attrs.is_empty());
        assert_eq!(root.coords[0].name, "x");
    }
}
