//! Remark attachment over the parsed named tree (issue #20, part 2).
//!
//! This slice ports the scope-resolution substrate of the Ruby
//! `RemarkAttacher`/`ScopeResolver` pair: a line → innermost-scope-name
//! map built from the source (strategy 1) and the entity/type/rule
//! source-text fallback (strategy 3). Attachment target resolution over
//! the tree and wire emission land in later slices.

/// The scope kinds the resolver tracks — the open/close keyword pairs of
/// ISO 10303-11 declarations that can own remarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Schema,
    Function,
    Procedure,
    Rule,
    Entity,
    Type,
}

impl ScopeKind {
    fn open_matches(line: &str) -> Option<(ScopeKind, String)> {
        // ^\s*(SCHEMA|FUNCTION|PROCEDURE|RULE|ENTITY|TYPE)\s+(\w+) —
        // case-insensitive, first match wins in the Ruby table's order.
        const KEYWORDS: [(&str, ScopeKind); 6] = [
            ("SCHEMA", ScopeKind::Schema),
            ("FUNCTION", ScopeKind::Function),
            ("PROCEDURE", ScopeKind::Procedure),
            ("RULE", ScopeKind::Rule),
            ("ENTITY", ScopeKind::Entity),
            ("TYPE", ScopeKind::Type),
        ];
        let trimmed = line.trim_start();
        for (keyword, kind) in KEYWORDS {
            if trimmed.len() >= keyword.len()
                && trimmed[..keyword.len()].eq_ignore_ascii_case(keyword)
            {
                let rest = &trimmed[keyword.len()..];
                // \s+ then the name (\w+ to end of the word)
                let mut chars = rest.char_indices().peekable();
                let mut spaces_end = 0usize;
                let mut saw_space = false;
                for (i, c) in chars.by_ref() {
                    if c.is_whitespace() {
                        saw_space = true;
                        spaces_end = i + c.len_utf8();
                    } else {
                        break;
                    }
                }
                if !saw_space {
                    continue;
                }
                let name_start = spaces_end;
                let rest = &rest[name_start..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if name.is_empty() {
                    continue;
                }
                return Some((kind, name));
            }
        }
        None
    }

    fn close_keyword(kind: ScopeKind) -> &'static str {
        match kind {
            ScopeKind::Schema => "END_SCHEMA",
            ScopeKind::Function => "END_FUNCTION",
            ScopeKind::Procedure => "END_PROCEDURE",
            ScopeKind::Rule => "END_RULE",
            ScopeKind::Entity => "END_ENTITY",
            ScopeKind::Type => "END_TYPE",
        }
    }

    /// The Ruby close patterns are unanchored substring matches
    /// (`/END_SCHEMA/i.match?(line)`).
    fn close_matches(line: &str, kind: ScopeKind) -> bool {
        let kw = Self::close_keyword(kind);
        line.to_ascii_lowercase().contains(&kw.to_ascii_lowercase())
    }
}

/// One stack entry of the scope walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeEntry {
    pub kind: ScopeKind,
    pub name: String,
}

/// The line → innermost-scope-name table (1-based lines; index 0 unused),
/// mirroring `ScopeResolver#build_scope_map`: opens push in the fixed
/// keyword order, closes pop only when they match the stack top, and the
/// entry recorded for a line is the stack top AFTER processing that line.
pub fn build_scope_map(source: &str) -> Vec<Option<String>> {
    let mut map = vec![None]; // 1-based
    let mut stack: Vec<ScopeEntry> = Vec::new();

    for line in source.lines() {
        if let Some((kind, name)) = ScopeKind::open_matches(line) {
            stack.push(ScopeEntry { kind, name });
        }
        for kind in [
            ScopeKind::Schema,
            ScopeKind::Function,
            ScopeKind::Procedure,
            ScopeKind::Rule,
            ScopeKind::Entity,
            ScopeKind::Type,
        ] {
            if ScopeKind::close_matches(line, kind)
                && stack.last().is_some_and(|top| top.kind == kind)
            {
                stack.pop();
            }
        }
        map.push(stack.last().map(|entry| entry.name.clone()));
    }

    map
}

/// The innermost scope name containing the given 1-based line, if any
/// (`ScopeResolver#find_by_name`'s table half).
pub fn containing_scope_name(map: &[Option<String>], line: usize) -> Option<&str> {
    map.get(line)?.as_deref()
}

/// The entity/type/rule fallback (`ScopeResolver#find_by_source_text`):
/// scan forward, track open spans, and when a matching END closes a span
/// that contains `line`, return its kind and name.
pub fn find_span_scope_by_source_text(source: &str, line: usize) -> Option<(ScopeKind, String)> {
    const TRACKED: [ScopeKind; 3] = [ScopeKind::Entity, ScopeKind::Type, ScopeKind::Rule];
    let mut spans: Vec<Option<(ScopeKind, String, usize)>> = TRACKED
        .iter()
        .map(|kind| Some((*kind, String::new(), 0usize)))
        .collect();

    for (idx, raw) in source.lines().enumerate() {
        let line_num = idx + 1;
        for (i, kind) in TRACKED.iter().enumerate() {
            let kw = ScopeKind::close_keyword(*kind)
                .strip_prefix("END_")
                .unwrap();
            let lower = raw.to_ascii_lowercase();
            let starts = lower.trim_start().starts_with(&kw.to_ascii_lowercase());
            let ends = lower.contains(&ScopeKind::close_keyword(*kind).to_ascii_lowercase());

            let state = spans[i].take();
            if starts {
                let name = raw.trim_start()[kw.len()..]
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect::<String>();
                spans[i] = Some((*kind, name, line_num));
            } else if ends {
                if let Some((_, name, start)) = state {
                    if line >= start && line <= line_num {
                        return Some((*kind, name));
                    }
                }
                spans[i] = None;
            } else {
                spans[i] = state;
            }
        }
    }

    None
}

/// Informal-proposition tag check (`/^\s*IP\d+$/` on the Ruby side —
/// anchored, uppercase IP, digits only).
pub fn is_ip_tag(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    bytes.len() >= 3
        && bytes[0] == b'I'
        && bytes[1] == b'P'
        && bytes[2..].iter().all(|b| b.is_ascii_digit())
}

/// `\A\s*WHERE\s+(\w+)\s*:` — the WHERE-clause label opener
/// (case-insensitive, like the Ruby regex).
fn where_label(line: &str) -> Option<&str> {
    const KEYWORD: &str = "WHERE";
    let trimmed = line.trim_start();
    if trimmed.len() < KEYWORD.len() || !trimmed[..KEYWORD.len()].eq_ignore_ascii_case(KEYWORD) {
        return None;
    }
    let rest = &trimmed[KEYWORD.len()..];
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let rest = rest.trim_start();
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return None;
    }
    let after = &rest[name.len()..];
    let after_trimmed = after.trim_start();
    if after_trimmed.starts_with(':') {
        Some(&rest[..name.len()])
    } else {
        None
    }
}

/// Single scan mapping each `WHERE <label>:` occurrence to its 1-based
/// line numbers (`RemarkAttacher#where_clause_line_index`).
pub fn where_clause_line_index(source: &str) -> std::collections::BTreeMap<String, Vec<usize>> {
    let mut index: std::collections::BTreeMap<String, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (idx, line) in source.lines().enumerate() {
        if let Some(label) = where_label(line) {
            index.entry(label.to_string()).or_default().push(idx + 1);
        }
    }
    index
}

/// The WHERE-clause ownership window: a label line owns remark lines
/// strictly after it and within 5 lines (`line_num < remark_line <=
/// line_num + 5`).
pub fn where_clause_window_contains(
    index: &std::collections::BTreeMap<String, Vec<usize>>,
    label: &str,
    remark_line: usize,
) -> bool {
    index.get(label).is_some_and(|lines| {
        lines
            .iter()
            .any(|&l| l < remark_line && remark_line <= l + 5)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_scope_wraps_lines() {
        let source = "SCHEMA a;\n  ENTITY b;\n  END_ENTITY;\nEND_SCHEMA;\n";
        let map = build_scope_map(source);
        assert_eq!(containing_scope_name(&map, 1), Some("a"));
        assert_eq!(containing_scope_name(&map, 2), Some("b"));
    }

    #[test]
    fn innermost_scope_wins() {
        let source = "SCHEMA a;\n  ENTITY b;\n    -- remark\n  END_ENTITY;\nEND_SCHEMA;\n";
        let map = build_scope_map(source);
        assert_eq!(containing_scope_name(&map, 3), Some("b"));
        assert_eq!(containing_scope_name(&map, 2), Some("b"));
        assert_eq!(containing_scope_name(&map, 1), Some("a"));
        assert_eq!(containing_scope_name(&map, 5), None);
    }

    #[test]
    fn function_inside_schema() {
        let source = "SCHEMA s;\nFUNCTION f;\nEND_FUNCTION;\nEND_SCHEMA;\n";
        let map = build_scope_map(source);
        assert_eq!(containing_scope_name(&map, 2), Some("f"));
        // After the END_FUNCTION pop, the schema owns line 3; the
        // END_SCHEMA line itself closes the last scope.
        assert_eq!(containing_scope_name(&map, 3), Some("s"));
        assert_eq!(containing_scope_name(&map, 4), None);
    }

    #[test]
    fn mismatched_close_does_not_pop() {
        // stack [Schema, Entity]; an END_SCHEMA line must NOT pop Entity.
        let source = "SCHEMA s;\n  ENTITY e;\nEND_SCHEMA;\nEND_ENTITY;\n";
        let map = build_scope_map(source);
        // line 3: END_SCHEMA — top is Entity, so no pop; scope stays "e".
        assert_eq!(containing_scope_name(&map, 3), Some("e"));
        // line 4: END_ENTITY pops Entity; scope becomes Schema "s".
        assert_eq!(containing_scope_name(&map, 4), Some("s"));
    }

    #[test]
    fn close_is_case_insensitive_substring() {
        let source = "SCHEMA s;\nend_schema\n";
        let map = build_scope_map(source);
        assert_eq!(containing_scope_name(&map, 2), None);
    }

    #[test]
    fn open_requires_whitespace_then_word() {
        assert_eq!(ScopeKind::open_matches("SCHEMAFOO;"), None);
        assert_eq!(
            ScopeKind::open_matches("  schema my_schema"),
            Some((ScopeKind::Schema, "my_schema".to_string()))
        );
        assert_eq!(
            ScopeKind::open_matches("TYPE box_t;"),
            Some((ScopeKind::Type, "box_t".to_string()))
        );
    }

    #[test]
    fn source_text_fallback_finds_entity_span() {
        let source = "SCHEMA s;\n  ENTITY wide;\n    -- target\n  END_ENTITY;\nEND_SCHEMA;\n";
        assert_eq!(
            find_span_scope_by_source_text(source, 3),
            Some((ScopeKind::Entity, "wide".to_string()))
        );
        assert_eq!(find_span_scope_by_source_text(source, 1), None);
    }

    #[test]
    fn source_text_fallback_requires_line_inside_span() {
        let source = "ENTITY a;\nEND_ENTITY;\n-- after\n";
        assert_eq!(find_span_scope_by_source_text(source, 3), None);
        assert_eq!(
            find_span_scope_by_source_text(source, 2),
            Some((ScopeKind::Entity, "a".to_string()))
        );
    }

    #[test]
    fn ip_tag_detection() {
        assert!(is_ip_tag("IP1"));
        assert!(is_ip_tag("IP42"));
        assert!(!is_ip_tag("ip1"));
        assert!(!is_tag_not_ip());
    }

    fn is_tag_not_ip() -> bool {
        is_ip_tag("IP") || is_ip_tag("IPx") || is_ip_tag("WR1")
    }

    #[test]
    fn where_label_matches_ruby_regex() {
        assert_eq!(where_label("  WHERE wr1 :"), Some("wr1"));
        assert_eq!(where_label("where wr1:"), Some("wr1"));
        assert_eq!(where_label("Where wr1  :"), Some("wr1"));
        assert_eq!(where_label("WHERELIKE x:"), None);
        assert_eq!(where_label("WHERE :"), None);
        assert_eq!(where_label("x WHERE y:"), None);
    }

    #[test]
    fn where_index_and_window() {
        let source = "ENTITY e;\n  WHERE wr1 : x > 0;\n  a := 1;\n  -- near\nWR1: ...";
        let index = where_clause_line_index(source);
        assert_eq!(index.get("wr1"), Some(&vec![2usize]));
        // Strictly after, within 5 lines.
        assert!(where_clause_window_contains(&index, "wr1", 3));
        assert!(where_clause_window_contains(&index, "wr1", 7));
        assert!(!where_clause_window_contains(&index, "wr1", 8));
        assert!(!where_clause_window_contains(&index, "wr1", 2));
        assert!(!where_clause_window_contains(&index, "wr2", 3));
    }
}

// ----- Wire-tree path resolution (S1: exact paths) -----

use serde_json::Value;
use std::collections::HashMap;

/// The named-child collections each wire `_class` exposes for path
/// resolution — the Ruby collection registry + Schema's
/// SCOPE_DECL_COLLECTIONS, keyed by the class-name suffix.
/// The per-class child collections `ModelElement#children` exposes —
/// the population `children_by_id` resolves tag paths against. The
/// virtual entries (an interface's items, a type's enumeration items)
/// are expanded by `child_by_id`, never listed here.
fn find_children(class: &str) -> &'static [&'static str] {
    match class.rsplit("::").next().unwrap_or(class) {
        "ExpFile" => &["schemas"],
        "Schema" => &[
            "constants",
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "rules",
            "procedures",
            "remark_items",
        ],
        "Entity" => &[
            "attributes",
            "derived_attributes",
            "inverse_attributes",
            "unique_rules",
            "where_rules",
            "informal_propositions",
            "remark_items",
        ],
        "Type" => &["where_rules", "informal_propositions", "remark_items"],
        "Function" | "Procedure" => &[
            "parameters",
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "procedures",
            "constants",
            "variables",
            "remark_items",
        ],
        "Rule" => &[
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "procedures",
            "constants",
            "variables",
            "where_rules",
            "informal_propositions",
            "remark_items",
        ],
        "SubtypeConstraint"
        | "Alias"
        | "Repeat"
        | "Parameter"
        | "Variable"
        | "Constant"
        | "WhereRule"
        | "UniqueRule"
        | "InformalPropositionRule"
        | "EnumerationItem"
        | "RemarkItem"
        | "InterfacedItem"
        | "Aggregate"
        | "Generic"
        | "GenericEntity"
        | "QueryExpression" => &["remark_items"],
        _ => &[],
    }
}

/// One segment of `ModelElement#children_by_id`: the LAST child with
/// this id wins (Ruby's `to_h` overwrites), matching the class's
/// `children` order. The wire pointer suffix of the match is returned
/// relative to `base_ptr`.
fn child_by_id(node: &Value, base_ptr: &str, part: &str) -> Option<String> {
    let class = class_suffix(node)?.to_string();
    let mut found: Option<String> = None;
    // A schema's interfaces expose their ITEMS as path children.
    if class == "Schema" {
        if let Some(interfaces) = node.get("interfaces").and_then(Value::as_array) {
            for (i, iface) in interfaces.iter().enumerate() {
                if let Some(items) = iface.get("items").and_then(Value::as_array) {
                    for (j, item) in items.iter().enumerate() {
                        if item
                            .get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|v| v.eq_ignore_ascii_case(part))
                        {
                            found = Some(format!("{base_ptr}/interfaces/{i}/items/{j}"));
                        }
                    }
                }
            }
        }
    }
    for attr in find_children(&class) {
        if let Some(items) = node.get(*attr).and_then(Value::as_array) {
            for (i, item) in items.iter().enumerate() {
                if item
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.eq_ignore_ascii_case(part))
                {
                    found = Some(format!("{base_ptr}/{attr}/{i}"));
                }
            }
        }
        // A type's enumeration items are path children too — on the wire
        // they live in the underlying ENUMERATION. A schema carries them
        // through its types; a type through itself.
        let owned: Vec<(String, &Value)> = if *attr == "types" {
            node.get("types")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .enumerate()
                        .map(|(i, t)| (format!("/types/{i}"), t))
                        .collect()
                })
                .unwrap_or_default()
        } else if class == "Type" && *attr == "where_rules" {
            vec![(String::new(), node)]
        } else {
            Vec::new()
        };
        for (t_suffix, t) in owned {
            let Some(ut) = t.get("underlying_type").filter(|v| !v.is_null()) else {
                continue;
            };
            if !class_suffix(ut).is_some_and(|c| c == "Enumeration") {
                continue;
            }
            let Some(enum_items) = ut.get("items").and_then(Value::as_array) else {
                continue;
            };
            for (j, item) in enum_items.iter().enumerate() {
                if item
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.eq_ignore_ascii_case(part))
                {
                    found = Some(format!("{base_ptr}{t_suffix}/underlying_type/items/{j}"));
                }
            }
        }
    }
    found
}

/// Classes that carry `remarks` (the Ruby Identifier concern plus the
/// two direct HasRemarks includes). `add_remark_to` drops text for
/// everything else, matching `add_remark`.
fn supports_remarks(class: &str) -> bool {
    matches!(
        class.rsplit("::").next().unwrap_or(class),
        "Schema"
            | "Constant"
            | "ConstantItem"
            | "Type"
            | "Entity"
            | "SubtypeConstraint"
            | "Function"
            | "Procedure"
            | "Rule"
            | "Parameter"
            | "Variable"
            | "Attribute"
            | "DerivedAttribute"
            | "InverseAttribute"
            | "UniqueRule"
            | "WhereRule"
            | "InformalPropositionRule"
            | "EnumerationItem"
            | "RemarkItem"
            | "InterfacedItem"
            | "Aggregate"
            | "Generic"
            | "GenericEntity"
            | "QueryExpression"
            | "Alias"
            | "Repeat"
    )
}

/// `ModelElement#find` over the wire: downcase the parts, strip
/// `prefix:`es, resolve through `children_by_id` from the model root.
/// Returns the JSON pointer to the resolved node.
fn exact_path_pointer(root: &Value, path: &str) -> Option<String> {
    let parts: Vec<String> = path
        .split('.')
        .map(|p| p.rsplit(':').next().unwrap_or(p).to_ascii_lowercase())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let mut pointer = String::new();
    let mut current = root;
    let mut pending_supertype: Option<&str> = None;
    for part in &parts {
        if let Some(supertype) = part.strip_prefix("self\\") {
            pending_supertype = Some(supertype);
            continue;
        }
        let next = if let Some(supertype) = pending_supertype.take() {
            find_redeclared_by_supertype(current, &pointer, part, supertype)?
        } else {
            child_by_id(current, &pointer, part)?
        };
        pointer = next;
        current = root.pointer(&pointer)?;
    }
    // A path ending on the SELF qualifier resolves nowhere.
    if pending_supertype.is_some() {
        return None;
    }
    Some(pointer)
}

/// `find_redeclared_by_supertype`: the attribute among the entity's
/// attribute collections whose id matches and whose supertype_attribute
/// references the given supertype name.
fn find_redeclared_by_supertype(
    scope: &Value,
    scope_ptr: &str,
    attr_name: &str,
    supertype: &str,
) -> Option<String> {
    for attr in ["attributes", "derived_attributes", "inverse_attributes"] {
        if let Some(items) = scope.get(attr).and_then(Value::as_array) {
            for (i, item) in items.iter().enumerate() {
                if class_suffix(item).is_none_or(|c| !c.contains("Attribute")) {
                    continue;
                }
                if !item
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.eq_ignore_ascii_case(attr_name))
                {
                    continue;
                }
                // Ruby: group_ref = supertype_attr.ref; group_ref.entity.id.
                let entity_id = item
                    .get("supertype_attribute")
                    .and_then(|st| st.get("ref"))
                    .and_then(|r| r.get("entity"))
                    .and_then(|e| e.get("id"))
                    .and_then(Value::as_str);
                if entity_id.is_some_and(|v| v.eq_ignore_ascii_case(supertype)) {
                    return Some(format!("{scope_ptr}/{attr}/{i}"));
                }
            }
        }
    }
    None
}

/// Test hook: the resolved pointer for `path` (find_by_exact_path core).
pub fn find_pointer_debug(root: &Value, path: &str) -> Option<String> {
    exact_path_pointer(root, path)
}

/// Ruby `find_by_exact_path`: the path, then a `:`-normalized retry.
pub fn find_by_exact_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let ptr = exact_path_pointer(root, path)?;
    root.pointer(&ptr)
}

pub fn find_by_exact_path_mut<'a>(root: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    let ptr = exact_path_pointer(root, path)?;
    root.pointer_mut(&ptr)
}

/// Ruby `add_remark`: blank bodies drop; the plain `remarks` array is
/// written only on HasRemarks classes, but the untagged_remarks
/// RemarkInfo entry is written on EVERY model element ONLY for untagged
/// remarks — a tagged target keeps the bare-text store alone.
/// The Ruby RemarkInfo wire map carries text/format/tag/placement/region.
pub fn add_remark_to(node: &mut Value, text: &str, tag: Option<&str>) -> bool {
    add_remark_tagged(node, text, tag, None, None, None, None)
}

#[allow(clippy::too_many_arguments)]
pub fn add_remark_tagged(
    node: &mut Value,
    text: &str,
    tag: Option<&str>,
    format: Option<&str>,
    placement: Option<&'static str>,
    region: Option<&'static str>,
    source_offset: Option<usize>,
) -> bool {
    let Some(class) = node.get("_class").and_then(Value::as_str) else {
        return false;
    };
    if text.trim().is_empty() {
        return false;
    }
    let suffix = class.rsplit("::").next().unwrap_or(class).to_string();
    if supports_remarks(&suffix) {
        if node.get("remarks").and_then(Value::as_array).is_none() {
            node["remarks"] = Value::Array(Vec::new());
        }
        if let Some(items) = node.get_mut("remarks").and_then(Value::as_array_mut) {
            items.push(Value::String(text.to_string()));
        }
    }
    if tag.is_none() {
        let format = format.unwrap_or(crate::remark::FORMAT_TAIL);
        let mut info = serde_json::json!({ "text": text, "format": format });
        if let Some(placement) = placement {
            info["placement"] = Value::String(placement.to_string());
        }
        if let Some(region) = region {
            info["region"] = Value::String(region.to_string());
        }
        if let Some(offset) = source_offset {
            info["source_offset"] = Value::from(offset as u64);
        }
        if node
            .get("untagged_remarks")
            .and_then(Value::as_array)
            .is_none()
        {
            node["untagged_remarks"] = Value::Array(Vec::new());
        }
        if let Some(items) = node
            .get_mut("untagged_remarks")
            .and_then(Value::as_array_mut)
        {
            items.push(info);
        }
    }
    true
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::ParsedTree;

    #[allow(dead_code)] // staged: consumed by the untagged orchestrator
    const DEMO: &str = "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n  END_ENTITY;\nEND_SCHEMA;\n--\"demo.thing.name\" a remark on the attribute\n";

    fn demo_wire() -> Value {
        ParsedTree::parse(DEMO)
            .expect("demo parses")
            .to_model_json("demo.exp")
            .expect("wire emits")
    }

    #[test]
    fn exact_path_resolves_through_the_wire() {
        let wire = demo_wire();
        let target = find_by_exact_path(&wire, "demo.thing.name").expect("path resolves");
        assert!(target
            .get("_class")
            .and_then(Value::as_str)
            .unwrap()
            .ends_with("::Attribute"));
    }

    #[test]
    fn exact_path_is_case_and_prefix_insensitive() {
        let wire = demo_wire();
        assert!(find_by_exact_path(&wire, "DEMO.THING.NAME").is_some());
        assert!(find_by_exact_path(&wire, "demo.thing.wr:name").is_some());
        assert!(find_by_exact_path(&wire, "demo.nothere").is_none());
    }

    #[test]
    fn add_remark_lands_on_the_attribute() {
        let mut wire = demo_wire();
        {
            let target = find_by_exact_path_mut(&mut wire, "demo.thing.name").expect("resolves");
            assert!(add_remark_to(target, "a remark on the attribute", None));
        }
        let attrs = wire
            .pointer("/schemas/0/entities/0/attributes")
            .expect("attrs");
        let remarks = attrs[0]
            .get("remarks")
            .and_then(Value::as_array)
            .expect("remarks");
        assert_eq!(remarks.len(), 1);
        assert_eq!(
            remarks[0],
            Value::String("a remark on the attribute".to_string())
        );
    }

    #[test]
    fn add_remark_drops_blank_and_unsupported() {
        let mut wire = demo_wire();
        let schema = find_by_exact_path_mut(&mut wire, "demo").expect("resolves");
        // Blank remark bodies are dropped (GH-363 behavior).
        assert!(!add_remark_to(schema, "   ", None));
        // The ExpFile wrapper carries untagged_remarks like any model
        // element (Ruby supports_remarks? = ModelElement).
        assert!(add_remark_to(&mut wire, "nope", None));
    }
}

// ----- Tagged attachment (the S1-S4 strategy chain) -----

const DECL: &str = "Expressir::Model::Declarations::";

fn class_suffix(node: &Value) -> Option<&str> {
    node.get("_class")
        .and_then(Value::as_str)
        .map(|c| c.rsplit("::").next().unwrap_or(c))
}

fn schema_ids(root: &Value) -> Vec<String> {
    root.get("schemas")
        .and_then(Value::as_array)
        .map(|schemas| {
            schemas
                .iter()
                .filter_map(|s| s.get("id").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// The wire pointer of the scope with this exact id: a schema, or one
/// of its scope declarations (`ScopeResolver#find_by_name`).
fn scope_node_pointer(root: &Value, name: &str) -> Option<String> {
    let schemas = root.get("schemas")?.as_array()?;
    for (i, schema) in schemas.iter().enumerate() {
        if schema.get("id").and_then(Value::as_str) == Some(name) {
            return Some(format!("/schemas/{i}"));
        }
        for attr in ["functions", "procedures", "rules", "entities", "types"] {
            if let Some(items) = schema.get(attr).and_then(Value::as_array) {
                for (j, item) in items.iter().enumerate() {
                    if item.get("id").and_then(Value::as_str) == Some(name) {
                        return Some(format!("/schemas/{i}/{attr}/{j}"));
                    }
                }
            }
        }
    }
    None
}

#[allow(dead_code)] // staged: consumed by the untagged orchestrator
fn node_is_function_rule_procedure(node: &Value) -> bool {
    matches!(
        class_suffix(node).unwrap_or(""),
        "Function" | "Procedure" | "Rule"
    )
}

fn supports_informal_propositions(node: &Value) -> bool {
    matches!(
        class_suffix(node).unwrap_or(""),
        "Entity" | "Rule" | "Type" | "InformalPropositionRule"
    )
}

/// `extract_ip_id`: "wr:IP1" -> "IP1", "IP1" -> "IP1", "WR1" -> None.
fn extract_ip_id(item_id: &str) -> Option<String> {
    let bare = match item_id.split_once(':') {
        Some((_, rest)) => rest,
        None => item_id,
    };
    is_ip_tag(bare).then(|| bare.to_string())
}

fn array_push(root: &mut Value, container_ptr: &str, attr: &str, item: Value) -> Option<usize> {
    let container_ptr = container_ptr.to_string();
    let arr_ptr = format!("{container_ptr}/{attr}");
    if root.pointer(&arr_ptr).and_then(Value::as_array).is_none() {
        root.pointer(&container_ptr)?;
        root.pointer_mut(&container_ptr)?[attr] = Value::Array(Vec::new());
    }
    let arr = root.pointer_mut(&arr_ptr)?.as_array_mut()?;
    arr.push(item);
    Some(arr.len() - 1)
}

/// `create_or_find_informal_proposition`: the IP declaration plus its
/// inner RemarkItem; returns the RemarkItem pointer.
fn create_or_find_informal_proposition(
    root: &mut Value,
    parent_ptr: &str,
    id: &str,
) -> Option<String> {
    let parent = root.pointer(parent_ptr)?;
    if !supports_informal_propositions(parent) {
        return None;
    }
    let parent_ptr = parent_ptr.to_string();
    let id = id.to_string();
    // Find the existing IP by exact id.
    let ip_count = root
        .pointer(&format!("{parent_ptr}/informal_propositions"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    for i in 0..ip_count {
        let ip_ptr = format!("{parent_ptr}/informal_propositions/{i}");
        if root.pointer(&ip_ptr)?.get("id").and_then(Value::as_str) == Some(id.as_str()) {
            // Find or create the inner remark item.
            let item_count = root
                .pointer(&format!("{ip_ptr}/remark_items"))
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            for j in 0..item_count {
                let item_ptr = format!("{ip_ptr}/remark_items/{j}");
                if root.pointer(&item_ptr)?.get("id").and_then(Value::as_str) == Some(id.as_str()) {
                    return Some(item_ptr);
                }
            }
            let index = array_push(
                root,
                &ip_ptr,
                "remark_items",
                serde_json::json!({
                    "_class": format!("{DECL}RemarkItem"),
                    "id": id,
                    "remarks": [],
                }),
            )?;
            return Some(format!("{ip_ptr}/remark_items/{index}"));
        }
    }
    let ip_index = array_push(
        root,
        &parent_ptr,
        "informal_propositions",
        serde_json::json!({
            "_class": format!("{DECL}InformalPropositionRule"),
            "id": id,
            "expression": Value::Null,
            "remark_items": [],
        }),
    )?;
    let ip_ptr = format!("{parent_ptr}/informal_propositions/{ip_index}");
    let item_index = array_push(
        root,
        &ip_ptr,
        "remark_items",
        serde_json::json!({
            "_class": format!("{DECL}RemarkItem"),
            "id": id,
            "remarks": [],
        }),
    )?;
    Some(format!("{ip_ptr}/remark_items/{item_index}"))
}

/// `supports_remark_items?` — the classes that include
/// `Model::Identifier` (and therefore `HasRemarkItems`).
fn supports_remark_items(node: &Value) -> bool {
    matches!(
        class_suffix(node).unwrap_or(""),
        "Attribute"
            | "DerivedAttribute"
            | "Entity"
            | "Function"
            | "Constant"
            | "Parameter"
            | "InformalPropositionRule"
            | "Procedure"
            | "InverseAttribute"
            | "Schema"
            | "Variable"
            | "Type"
            | "Rule"
            | "UniqueRule"
            | "SubtypeConstraint"
            | "WhereRule"
            | "Aggregate"
            | "EnumerationItem"
            | "Generic"
            | "GenericEntity"
            | "QueryExpression"
            | "Alias"
            | "Repeat"
    )
}

/// `create_item_at_parent`: IP-shaped ids redirect to informal
/// propositions; everything else finds-or-creates a RemarkItem on a
/// remark_items-bearing parent (Ruby: RemarkItem itself does NOT
/// support nested items, so creation returns nil there).
fn create_or_find_item(root: &mut Value, parent_ptr: &str, item_id: &str) -> Option<String> {
    if let Some(ip_id) = extract_ip_id(item_id) {
        if let Some(ptr) = create_or_find_informal_proposition(root, parent_ptr, &ip_id) {
            return Some(ptr);
        }
    }
    let parent_ptr = parent_ptr.to_string();
    let item_id = item_id.to_string();
    if !root.pointer(&parent_ptr).is_some_and(supports_remark_items) {
        return None;
    }
    let count = root
        .pointer(&format!("{parent_ptr}/remark_items"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    for i in 0..count {
        let item_ptr = format!("{parent_ptr}/remark_items/{i}");
        if root.pointer(&item_ptr)?.get("id").and_then(Value::as_str) == Some(item_id.as_str()) {
            return Some(item_ptr);
        }
    }
    let index = array_push(
        root,
        &parent_ptr,
        "remark_items",
        serde_json::json!({
            "_class": format!("{DECL}RemarkItem"),
            "id": item_id,
            "remarks": [],
        }),
    )?;
    Some(format!("{parent_ptr}/remark_items/{index}"))
}

/// `normalize_path`: "a.wr:WR1" -> "a.WR1" (split on the LAST colon).
fn normalize_path(path: &str) -> String {
    match path.rsplit_once(':') {
        Some((path_part, item_suffix)) => format!("{path_part}.{item_suffix}"),
        None => path.to_string(),
    }
}

/// `handle_prefixed_tag`: wr:/ip:/ur: — the collection on the containing
/// scope first, then schema-prefixed exact paths with `:` -> `.`.
fn prefixed_tag_pointer(
    root: &Value,
    scope_ptr: Option<&str>,
    tag: &str,
    schema_ids: &[String],
) -> Option<String> {
    let (prefix, id) = tag.split_once(':')?;
    let collection = match prefix.to_ascii_lowercase().as_str() {
        "wr" => "where_rules",
        "ip" => "informal_propositions",
        "ur" => "unique_rules",
        _ => return None,
    };
    if let Some(scope_ptr) = scope_ptr {
        let scope = root.pointer(scope_ptr)?;
        if let Some(items) = scope.get(collection).and_then(Value::as_array) {
            for (i, item) in items.iter().enumerate() {
                if item.get("id").and_then(Value::as_str) == Some(id) {
                    return Some(format!("{scope_ptr}/{collection}/{i}"));
                }
            }
        }
    }
    for schema_id in schema_ids {
        let path = format!("{schema_id}.{}", tag.replace(':', "."));
        if let Some(ptr) = exact_path_pointer(root, &path) {
            return Some(ptr);
        }
    }
    None
}

/// `find_target_in_where_clause`: the WHERE-label window owns the tag;
/// the target is a (created) RemarkItem on the owning where rule.
fn where_clause_target(
    root: &mut Value,
    scope_ptr: &str,
    tag: &str,
    remark_line: usize,
    where_index: &std::collections::BTreeMap<String, Vec<usize>>,
) -> Option<String> {
    let scope = root.pointer(scope_ptr)?;
    let rules = scope.get("where_rules")?.as_array()?;
    if rules.is_empty() {
        return None;
    }
    for (i, wr) in rules.iter().enumerate() {
        let Some(id) = wr.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(lines) = where_index.get(id) else {
            continue;
        };
        for &line_num in lines {
            if line_num < remark_line && remark_line <= line_num + 5 {
                let wr_ptr = format!("{scope_ptr}/where_rules/{i}");
                return create_or_find_item(root, &wr_ptr, tag);
            }
        }
    }
    None
}

/// `resolve_longest_owned_prefix`: walk the tag's segments from a
/// schema root honoring the wr:/ip:/ur: prefixes find() strips, and
/// return [deepest node pointer, first unresolved segment].
fn resolve_longest_owned_prefix(
    root: &Value,
    path: &str,
    schema_ids: &[String],
) -> Option<(String, String)> {
    let mut candidates = vec![path.to_string()];
    for schema_id in schema_ids {
        candidates.push(format!("{schema_id}.{path}"));
    }
    for candidate in candidates {
        let segments: Vec<&str> = candidate.split('.').collect();
        if segments.is_empty() {
            continue;
        }
        let mut owned = segments[0].to_string();
        let Some(mut node_ptr) = exact_path_pointer(root, &owned) else {
            continue;
        };
        let mut resolved_any = false;
        for segment in &segments[1..] {
            // A lazily created RemarkItem is never a path owner.
            let stepped = exact_path_pointer(root, &format!("{owned}.{segment}")).filter(|ptr| {
                root.pointer(ptr)
                    .and_then(|n| n.get("_class").and_then(Value::as_str))
                    .is_none_or(|class| !class.ends_with("::RemarkItem"))
            });
            if let Some(ptr) = stepped {
                owned = format!("{owned}.{segment}");
                node_ptr = ptr;
                resolved_any = true;
            } else {
                if resolved_any {
                    return Some((node_ptr, (*segment).to_string()));
                }
                break;
            }
        }
    }
    None
}

/// `create_implicit_remark_item`: the IP-prefixed, segment-aware, and
/// shortest-parent fallbacks; returns the created/found item pointer.
fn create_implicit_remark_item(
    root: &mut Value,
    path: &str,
    schema_ids: &[String],
) -> Option<String> {
    // "a.b.ip:IP1" convention: the parent path + the bare IP id.
    if let Some((parent_path, ip_id)) = ip_prefixed_tail(path) {
        let mut parent = exact_path_pointer(root, &parent_path);
        if parent.is_none() {
            for schema_id in schema_ids {
                parent = exact_path_pointer(root, &format!("{schema_id}.{parent_path}"));
                if parent.is_some() {
                    break;
                }
            }
        }
        if let Some(parent_ptr) = parent {
            let supports = root
                .pointer(&parent_ptr)
                .is_some_and(supports_informal_propositions);
            if supports {
                return create_or_find_informal_proposition(root, &parent_ptr, &ip_id);
            }
        }
    }

    // A prefixed intermediate segment splits wrong under the generic
    // normalized walk; resolve the longest owned prefix instead.
    let segments: Vec<&str> = path.split('.').collect();
    if segments.len() > 2
        && segments[..segments.len() - 1]
            .iter()
            .any(|s| s.contains(':'))
    {
        if let Some((parent_ptr, item_id)) = resolve_longest_owned_prefix(root, path, schema_ids) {
            return create_or_find_item(root, &parent_ptr, &item_id);
        }
    }

    let clean = normalize_path(path);
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    for i in (1..parts.len()).rev() {
        let parent_path = parts[..i].join(".");
        let item_id = parts[i];
        let mut parent = exact_path_pointer(root, &parent_path);
        if parent.is_none() {
            for schema_id in schema_ids {
                parent = exact_path_pointer(root, &format!("{schema_id}.{parent_path}"));
                if parent.is_some() {
                    break;
                }
            }
        }
        if let Some(parent_ptr) = parent {
            return create_or_find_item(root, &parent_ptr, item_id);
        }
    }
    None
}

/// `path.match(/\A(.+)\.(\w+):(IP\d+)\z/)`.
fn ip_prefixed_tail(path: &str) -> Option<(String, String)> {
    let idx = path.rfind('.')?;
    let (parent_path, tail) = path.split_at(idx);
    let tail = &tail[1..];
    let (prefix, ip) = tail.split_once(':')?;
    if !prefix.chars().all(|c| c.is_alphanumeric() || c == '_') || !is_ip_tag(ip) {
        return None;
    }
    Some((parent_path.to_string(), ip.to_string()))
}

/// `create_implicit_remark_item_at_schema`.
fn create_implicit_remark_item_at_schema(
    root: &mut Value,
    item_id: &str,
    schema_id: &str,
) -> Option<String> {
    let schema_ptr = exact_path_pointer(root, schema_id)?;
    let is_schema = root
        .pointer(&schema_ptr)
        .and_then(|n| n.get("_class").and_then(Value::as_str))
        .is_some_and(|c| c.ends_with("::Schema"));
    if !is_schema {
        return None;
    }
    if let Some(ip_id) = extract_ip_id(item_id) {
        if let Some(ptr) = create_or_find_informal_proposition(root, &schema_ptr, &ip_id) {
            return Some(ptr);
        }
    }
    create_or_find_item(root, &schema_ptr, item_id)
}

/// The tagged-remark strategy chain (`attach_tagged_remarks`) over the
/// wire. Returns the byte spans that attached.
pub fn attach_tagged_remarks(root: &mut Value, source: &str) -> Vec<usize> {
    let remarks = crate::remark::scan(source);
    let tagged: Vec<_> = remarks.iter().filter(|r| r.tag.is_some()).collect();
    if tagged.is_empty() {
        return Vec::new();
    }

    let schema_id_list = schema_ids(root);
    let scope_map = build_scope_map(source);
    let where_index = where_clause_line_index(source);
    // Ruby's containing_scope_for falls back to the innermost
    // ScopeContainer whose index span covers the remark line — a remark
    // after END_SCHEMA can still sit inside the previous construct's
    // newline-aware span.
    let position_index = WireIndex::build(root, source);
    let mut attached_spans: Vec<usize> = Vec::new();

    for remark in tagged {
        if attached_spans.contains(&remark.position) {
            continue;
        }
        let tag = remark.tag.as_deref().unwrap_or_default();
        let mut scope_name: Option<String> =
            containing_scope_name(&scope_map, remark.line).map(String::from);
        let mut scope_ptr = scope_name
            .clone()
            .and_then(|name| scope_node_pointer(root, &name));
        // Position fallback (Ruby find_by_position): the LAST-listed
        // covering scope container wins; the scope's name comes from the
        // node's id, exactly as build_scope_path would read it.
        if scope_ptr.is_none() {
            if let Some(entry) = position_index
                .spanning(remark.line)
                .into_iter()
                .rev()
                .filter(|e| {
                    matches!(
                        e.class.as_str(),
                        "Schema" | "Function" | "Procedure" | "Rule" | "Entity" | "Type"
                    )
                })
                .next()
            {
                let id = root
                    .pointer(&entry.ptr)
                    .and_then(|n| n.get("id"))
                    .and_then(Value::as_str)
                    .map(String::from);
                scope_ptr = Some(entry.ptr.clone());
                scope_name = id;
            }
        }
        // Source-text fallback when both lookups missed (IP tags on
        // types/entities declared on a previous line).
        if scope_ptr.is_none() {
            if let Some((_, name)) = find_span_scope_by_source_text(source, remark.line) {
                scope_ptr = scope_node_pointer(root, &name);
            }
        }

        let mut target: Option<String> = None;

        // Informal-proposition tags (the resolved scope only).
        if is_ip_tag(tag) {
            if let Some(ref scope_ptr) = scope_ptr {
                let supports = root
                    .pointer(scope_ptr)
                    .is_some_and(supports_informal_propositions);
                if supports {
                    target = create_or_find_informal_proposition(root, scope_ptr, tag);
                }
            }
        }

        if target.is_none() {
            // Prefixed tags (wr:/ip:/ur: — colon, no dot).
            if tag.contains(':') && !tag.contains('.') {
                target = prefixed_tag_pointer(root, scope_ptr.as_deref(), tag, &schema_id_list);
            }

            // S1: exact path.
            if target.is_none() {
                target = exact_path_pointer(root, tag);
            }

            // S1b: dotted tags with the scope path or a schema prefix.
            if target.is_none() && tag.contains('.') {
                let scope_class = scope_ptr
                    .as_deref()
                    .and_then(|ptr| root.pointer(ptr))
                    .and_then(class_suffix)
                    .map(String::from)
                    .unwrap_or_default();
                let is_fn_rule_proc =
                    matches!(scope_class.as_str(), "Function" | "Procedure" | "Rule");
                if is_fn_rule_proc {
                    if let Some(scope_ptr) = scope_ptr.as_deref() {
                        if let Some(schema_id) = scope_owner_schema_id(root, scope_ptr) {
                            let scope_path = format!(
                                "{schema_id}.{}",
                                scope_name.as_deref().unwrap_or_default()
                            );
                            target = exact_path_pointer(root, &format!("{scope_path}.{tag}"));
                        }
                    }
                }
                if target.is_none() {
                    for schema_id in &schema_id_list {
                        target = exact_path_pointer(root, &format!("{schema_id}.{tag}"));
                        if target.is_some() {
                            break;
                        }
                    }
                }
            }

            // S2: simple tags in the containing scope first.
            if target.is_none() && !tag.contains('.') {
                if let Some(scope_ptr) = scope_ptr.as_deref() {
                    target = find_node_in_scope_pointer(root, scope_ptr, tag);
                    if target.is_none() {
                        target =
                            where_clause_target(root, scope_ptr, tag, remark.line, &where_index);
                    }
                    let scope_class = root
                        .pointer(scope_ptr)
                        .and_then(class_suffix)
                        .map(String::from)
                        .unwrap_or_default();
                    let is_fn_rule_proc =
                        matches!(scope_class.as_str(), "Function" | "Procedure" | "Rule");
                    if target.is_none() && !is_fn_rule_proc {
                        for schema_id in &schema_id_list {
                            target = exact_path_pointer(root, &format!("{schema_id}.{tag}"));
                            if target.is_some() {
                                break;
                            }
                        }
                    }
                } else {
                    for schema_id in &schema_id_list {
                        target = exact_path_pointer(root, &format!("{schema_id}.{tag}"));
                        if target.is_some() {
                            break;
                        }
                    }
                }
            }

            // S3: implicit items for qualified paths.
            if target.is_none() && tag.contains('.') {
                let scope_class = scope_ptr
                    .as_deref()
                    .and_then(|ptr| root.pointer(ptr))
                    .and_then(class_suffix)
                    .map(String::from)
                    .unwrap_or_default();
                let is_fn_rule_proc =
                    matches!(scope_class.as_str(), "Function" | "Procedure" | "Rule");
                if is_fn_rule_proc {
                    if let Some(scope_ptr) = scope_ptr.as_deref() {
                        if let Some(schema_id) = scope_owner_schema_id(root, scope_ptr) {
                            let scope_path = format!(
                                "{schema_id}.{}",
                                scope_name.as_deref().unwrap_or_default()
                            );
                            target = create_implicit_remark_item(
                                root,
                                &format!("{scope_path}.{tag}"),
                                &schema_id_list,
                            );
                        }
                    }
                }
                if target.is_none() {
                    target = create_implicit_remark_item(root, tag, &schema_id_list);
                }
            }

            // S4: implicit items for simple tags at schema level.
            if target.is_none() && !tag.contains('.') && !schema_id_list.is_empty() {
                target = create_implicit_remark_item_at_schema(root, tag, &schema_id_list[0]);
            }
        }

        if let Some(target_ptr) = target {
            if let Some(target_node) = root.pointer_mut(&target_ptr) {
                if add_remark_to(target_node, &remark.text, remark.tag.as_deref()) {
                    attached_spans.push(remark.position);
                }
            }
        }
    }

    attached_spans
}

/// `find_node_in_scope`: registry collections' HasRemarks items whose
/// id equals the tag exactly (case-sensitive), then the scope's types'
/// enumeration items, then statements (ALIAS/REPEAT/QUERY ids, plus a
/// QUERY search inside the statement subtree).
fn find_node_in_scope_pointer(root: &Value, scope_ptr: &str, tag: &str) -> Option<String> {
    let scope = root.pointer(scope_ptr)?;
    let class = class_suffix(scope)?.to_string();
    for attr in collection_registry(&format!("Declarations::{class}"))
        .iter()
        .chain(collection_registry(&format!("Statements::{class}")))
    {
        if let Some(items) = scope.get(*attr).and_then(Value::as_array) {
            for (i, item) in items.iter().enumerate() {
                let item_class = class_suffix(item).unwrap_or("");
                if supports_remarks(item_class)
                    && item.get("id").and_then(Value::as_str) == Some(tag)
                {
                    return Some(format!("{scope_ptr}/{attr}/{i}"));
                }
            }
        }
    }
    // Enumeration items of the scope's types (and a TYPE scope's own).
    let owned_types: Vec<(String, &Value)> = if class == "Type" {
        vec![(String::new(), scope)]
    } else {
        scope
            .get("types")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .enumerate()
                    .map(|(i, t)| (format!("/types/{i}"), t))
                    .collect()
            })
            .unwrap_or_default()
    };
    for (t_suffix, t) in owned_types {
        let Some(ut) = t.get("underlying_type").filter(|v| !v.is_null()) else {
            continue;
        };
        if !class_suffix(ut).is_some_and(|c| c == "Enumeration") {
            continue;
        }
        if let Some(enum_items) = ut.get("items").and_then(Value::as_array) {
            for (j, item) in enum_items.iter().enumerate() {
                if item.get("id").and_then(Value::as_str) == Some(tag) {
                    return Some(format!("{scope_ptr}{t_suffix}/underlying_type/items/{j}"));
                }
            }
        }
    }
    // Statements: direct ALIAS/REPEAT ids, then QUERY expressions inside.
    if let Some(stmts) = scope.get("statements").and_then(Value::as_array) {
        for (i, stmt) in stmts.iter().enumerate() {
            let stmt_class = class_suffix(stmt).unwrap_or("");
            if matches!(stmt_class, "Alias" | "Repeat" | "QueryExpression")
                && stmt.get("id").and_then(Value::as_str) == Some(tag)
            {
                return Some(format!("{scope_ptr}/statements/{i}"));
            }
            let stmt_ptr = format!("{scope_ptr}/statements/{i}");
            if let Some(hit) = find_query_in_subtree(root, &stmt_ptr, tag) {
                return Some(hit);
            }
        }
    }
    None
}

/// `find_query_in_expression`: the QUERY expression with this id inside
/// a statement subtree.
fn find_query_in_subtree(root: &Value, ptr: &str, tag: &str) -> Option<String> {
    let node = root.pointer(ptr)?;
    if class_suffix(node) == Some("QueryExpression")
        && node.get("id").and_then(Value::as_str) == Some(tag)
    {
        return Some(ptr.to_string());
    }
    if let Some(map) = node.as_object() {
        for (key, value) in map {
            match value {
                Value::Array(items) => {
                    for (i, _item) in items.iter().enumerate() {
                        if let Some(hit) =
                            find_query_in_subtree(root, &format!("{ptr}/{key}/{i}"), tag)
                        {
                            return Some(hit);
                        }
                    }
                }
                Value::Object(_) => {
                    if let Some(hit) = find_query_in_subtree(root, &format!("{ptr}/{key}"), tag) {
                        return Some(hit);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// The schema ID owning a scope pointer ("/schemas/i/...") — the schema's
/// NAME, the form `build_scope_path` prefixes onto tags.
fn scope_owner_schema_id(root: &Value, scope_ptr: &str) -> Option<String> {
    if !scope_ptr.starts_with("/schemas/") {
        return None;
    }
    let index: usize = scope_ptr
        .trim_start_matches("/schemas/")
        .split('/')
        .next()?
        .parse()
        .ok()?;
    root.get("schemas")
        .and_then(Value::as_array)
        .and_then(|a| a.get(index))
        .and_then(|s| s.get("id"))
        .and_then(Value::as_str)
        .map(String::from)
}

#[cfg(test)]
mod tagged_chain_tests {
    use super::*;
    use crate::ParsedTree;

    fn wire_with_tagged(source: &str) -> Value {
        let mut wire = ParsedTree::parse(source)
            .expect("parses")
            .to_model_json("demo.exp")
            .expect("wire");
        attach_tagged_remarks(&mut wire, source);
        wire
    }

    #[test]
    fn ip_tag_creates_an_informal_proposition_on_the_entity() {
        // The IP remark sits inside the entity scope.
        let source = "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n    -- IP1: an informal note\n  END_ENTITY;\nEND_SCHEMA;\n";
        let wire = wire_with_tagged(source);
        let entity = wire.pointer("/schemas/0/entities/0").expect("entity");
        let ips = entity
            .get("informal_propositions")
            .and_then(Value::as_array)
            .expect("ips");
        assert_eq!(ips.len(), 1);
        assert_eq!(ips[0].get("id").and_then(Value::as_str), Some("IP1"));
        let items = ips[0]
            .get("remark_items")
            .and_then(Value::as_array)
            .expect("items");
        assert_eq!(items[0].get("id").and_then(Value::as_str), Some("IP1"));
        let remarks = items[0]
            .get("remarks")
            .and_then(Value::as_array)
            .expect("remarks");
        assert_eq!(remarks[0], Value::String("an informal note".to_string()));
    }

    #[test]
    fn dotted_unknown_path_creates_a_remark_item_on_the_parent() {
        let wire = wire_with_tagged(
            "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n  END_ENTITY;\nEND_SCHEMA;\n--\"demo.thing.__note\" dotted item\n",
        );
        let entity = wire.pointer("/schemas/0/entities/0").expect("entity");
        let items = entity
            .get("remark_items")
            .and_then(Value::as_array)
            .expect("items");
        assert_eq!(items[0].get("id").and_then(Value::as_str), Some("__note"));
        let remarks = items[0]
            .get("remarks")
            .and_then(Value::as_array)
            .expect("remarks");
        assert_eq!(remarks[0], Value::String("dotted item".to_string()));
    }

    #[test]
    fn simple_unknown_tag_creates_a_schema_level_item() {
        let wire = wire_with_tagged(
            "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n  END_ENTITY;\nEND_SCHEMA;\n--\"demo.demo_extras\" schema item\n",
        );
        let schema = wire.pointer("/schemas/0").expect("schema");
        let items = schema
            .get("remark_items")
            .and_then(Value::as_array)
            .expect("items");
        assert_eq!(
            items[0].get("id").and_then(Value::as_str),
            Some("demo_extras")
        );
        let remarks = items[0]
            .get("remarks")
            .and_then(Value::as_array)
            .expect("remarks");
        assert_eq!(remarks[0], Value::String("schema item".to_string()));
    }

    #[test]
    fn tagged_where_rule_id_resolves_scope_locally() {
        let source = "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n    WHERE\n      wr1 : name <> '';\n    --\"demo.thing.wr1\" wr1 note text\n  END_ENTITY;\nEND_SCHEMA;\n";
        let wire = wire_with_tagged(source);
        // The tag resolves scope-locally to the rule itself, so the
        // remark lands on the rule's remarks (not an implicit item).
        let wr = wire
            .pointer("/schemas/0/entities/0/where_rules/0")
            .expect("wr");
        let remarks = wr
            .get("remarks")
            .and_then(Value::as_array)
            .expect("remarks");
        assert_eq!(remarks[0], Value::String("wr1 note text".to_string()));
    }

    #[test]
    fn prefixed_tag_resolves_inside_the_scope() {
        let source = "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n    WHERE\n      wr1 : name <> '';\n  END_ENTITY;\nEND_SCHEMA;\n--\"demo.thing.wr:WR1\" prefixed note\n";
        let wire = wire_with_tagged(source);
        let wr = wire
            .pointer("/schemas/0/entities/0/where_rules/0")
            .expect("wr");
        let remarks = wr
            .get("remarks")
            .and_then(Value::as_array)
            .expect("remarks");
        assert_eq!(remarks[0], Value::String("prefixed note".to_string()));
    }
}

#[cfg(test)]
mod span_tests {
    use crate::ParsedTree;

    const DEMO: &str =
        "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n  END_ENTITY;\nEND_SCHEMA;\n";

    #[test]
    fn wire_nodes_carry_start_and_end_offsets() {
        let tree = ParsedTree::parse(DEMO).expect("parses");
        let wire = tree.to_model_json("demo.exp").expect("wire");

        let line_of = |offset: u64| -> usize {
            // LineMap over the source.
            let mut line = 1usize;
            for (i, b) in DEMO.bytes().enumerate() {
                if i as u64 >= offset {
                    break;
                }
                if b == b'\n' {
                    line += 1;
                }
            }
            line
        };

        let entity = wire.pointer("/schemas/0/entities/0").expect("entity");
        let start = entity
            .get("source_offset")
            .and_then(serde_json::Value::as_u64);
        let end = entity
            .get("source_end_offset")
            .and_then(serde_json::Value::as_u64);
        assert_eq!(start.map(line_of), Some(2), "entity starts on line 2");
        // The entity's last token is END_ENTITY's semicolon (line 4).
        assert_eq!(end.map(line_of), Some(4), "entity ends on line 4");
    }
}

// ----- The wire node index (NodePositionIndex substrate) -----

/// One indexed wire node: its JSON pointer, class suffix, line span,
/// and document order.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub ptr: String,
    pub class: String,
    /// The full wire `_class` — the placement predicates test
    /// namespace membership (`::Statements::`), which the suffix loses.
    pub full_class: String,
    pub line: usize,
    pub end_line: Option<usize>,
    pub offset: Option<u64>,
    pub order: usize,
    /// The JSON pointer of the collection-attribute owner (Ruby's
    /// `owner`), and the collection attribute this node came from.
    pub owner_ptr: Option<String>,
    pub collection: Option<&'static str>,
}

/// Ruby `position_valid?`: a formatted source that opens with a
/// declaration keyword may legitimately start at byte 0.
const POSITION_VALID: &[&str] = &[
    "ExpFile",
    "Schema",
    "Entity",
    "Type",
    "Function",
    "Procedure",
    "Rule",
    "Constant",
    "Interface",
];

/// Line-start offsets over the source (the LineMap analog).
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn line_of(starts: &[usize], offset: u64) -> usize {
    starts.partition_point(|&s| s <= offset as usize)
}

/// The semantic/remark-scope predicates (`semantic?` / `remark_scope?`).
fn is_semantic(class: &str) -> bool {
    !matches!(class, "Repository" | "Cache")
}

fn is_remark_scope(class: &str) -> bool {
    is_semantic(class) && class != "Interface"
}

/// DFS over the wire in document order, indexing every `_class` node
/// that carries offsets. Sorted by (offset, order) like
/// `build_sorted_nodes`.
pub struct WireIndex {
    pub entries: Vec<IndexEntry>,
    starts: Vec<usize>,
}

impl WireIndex {
    pub fn build(root: &Value, source: &str) -> Self {
        let starts = line_starts(source);
        let mut entries = Vec::new();
        collect(root, "", None, None, &starts, &mut entries);
        entries.sort_by_key(|e| (e.offset.unwrap_or(u64::MAX), e.order));
        for (i, entry) in entries.iter_mut().enumerate() {
            entry.order = i;
        }
        // Ruby position_valid?: offset 0 is only trusted for nodes whose
        // formatted source starts with a declaration keyword (those
        // legitimately start at byte 0).
        entries.retain(|e| e.offset != Some(0) || POSITION_VALID.contains(&e.class.as_str()));
        // Ruby end_line = line_number(offset + source.length) over the
        // FORMATTED text, maxed with collection children's own ends
        // (NodePositionIndex#record_node_position). The formatted-length
        // mirror supplies `source.length`.
        let lens = crate::format_len::lengths_by_ptr(root);
        let mut spans: Vec<Option<usize>> = entries
            .iter()
            .map(|e| match (e.offset, lens.get(e.ptr.as_str())) {
                (Some(off), Some(p)) => Some(line_of(&starts, off + p.len as u64)),
                _ => None,
            })
            .collect();
        let mut children_by_owner: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            if let Some(owner) = e.owner_ptr.as_deref() {
                children_by_owner.entry(owner).or_default().push(i);
            }
        }
        for i in 0..entries.len() {
            let mut end = spans[i];
            if let Some(children) = children_by_owner.get(entries[i].ptr.as_str()) {
                for &j in children {
                    if let Some(c) = spans[j] {
                        end = Some(match end {
                            Some(e) => e.max(c),
                            None => c,
                        });
                    }
                }
            }
            spans[i] = end;
        }
        for (entry, end) in entries.iter_mut().zip(spans) {
            entry.end_line = end;
        }
        WireIndex { entries, starts }
    }

    pub fn line_of(&self, offset: u64) -> usize {
        line_of(&self.starts, offset)
    }

    fn semantic(&self) -> impl Iterator<Item = &IndexEntry> {
        self.entries.iter().filter(|e| is_semantic(&e.class))
    }

    pub fn starting_at(&self, line: usize) -> Vec<&IndexEntry> {
        self.semantic().filter(|e| e.line == line).collect()
    }

    pub fn ending_at(&self, line: usize) -> Vec<&IndexEntry> {
        self.semantic()
            .filter(|e| e.end_line == Some(line))
            .collect()
    }

    pub fn spanning(&self, line: usize) -> Vec<&IndexEntry> {
        self.semantic()
            .filter(|e| e.line <= line && e.end_line.is_some_and(|end| end >= line))
            .collect()
    }

    /// Semantic entries with end_line < line, ascending by end_line.
    pub fn semantic_ending_before(&self, line: usize) -> Vec<&IndexEntry> {
        let mut list: Vec<&IndexEntry> = self
            .semantic()
            .filter(|e| e.end_line.is_some_and(|end| end < line))
            .collect();
        list.sort_by_key(|e| e.end_line.unwrap_or(0));
        list
    }

    pub fn first_semantic(&self) -> Option<&IndexEntry> {
        self.semantic().next()
    }

    /// `nearest_node_to`: most-specific node whose span contains the
    /// remark line, preferring same-line starts/ends, then the smallest
    /// containing span; falls back to the last ended node, then the
    /// first semantic node.
    pub fn nearest_node_to(&self, remark_line: usize) -> Option<&IndexEntry> {
        let same_start: Vec<&IndexEntry> = self
            .starting_at(remark_line)
            .into_iter()
            .filter(|e| is_remark_scope(&e.class))
            .collect();
        if let Some(last) = same_start.last() {
            return Some(last);
        }
        let same_end: Vec<&IndexEntry> = self
            .ending_at(remark_line)
            .into_iter()
            .filter(|e| is_remark_scope(&e.class))
            .collect();
        if let Some(last) = same_end.last() {
            return Some(last);
        }
        let mut containing: Vec<&IndexEntry> = self
            .spanning(remark_line)
            .into_iter()
            .filter(|e| is_remark_scope(&e.class))
            .collect();
        if !containing.is_empty() {
            // A remark BEFORE the first schema belongs to the file
            // container even when containers span it.
            if containing.iter().any(|e| e.class == "ExpFile") {
                let first_schema_offset = self
                    .entries
                    .iter()
                    .find(|e| e.class == "Schema")
                    .and_then(|e| e.offset);
                if let Some(off) = first_schema_offset {
                    if remark_line < self.line_of(off) {
                        return containing.iter().find(|e| e.class == "ExpFile").copied();
                    }
                }
            }
            // Prefer non-ExpFile nodes (Schema, Entity, ...) when both
            // span the same range: remarks inside a schema belong to the
            // schema.
            let non_root: Vec<&IndexEntry> = containing
                .iter()
                .copied()
                .filter(|e| e.class != "ExpFile")
                .collect();
            if !non_root.is_empty() {
                containing = non_root;
            }
            containing
                .into_iter()
                .min_by_key(|e| e.end_line.unwrap_or(0) - e.line)
        } else {
            let before = self.semantic_ending_before(remark_line);
            if let Some(max_end) = before.iter().filter_map(|e| e.end_line).max() {
                // Ruby resolves max-end ties through its sort order, which
                // is stable (document order) for the small entry lists
                // fixtures produce; the first-recorded node wins.
                return before.iter().find(|e| e.end_line == Some(max_end)).copied();
            }
            self.first_semantic()
        }
    }

    /// `node_for_end_scope_at`: the node whose END_XXX is on the remark
    /// line (within a 2-line window), else the first node of that type.
    pub fn node_for_end_scope_at(
        &self,
        remark_line: usize,
        line_content: &str,
    ) -> Option<&IndexEntry> {
        let lowered = line_content.to_ascii_lowercase();
        let kind = [
            ("end_schema", "Schema"),
            ("end_entity", "Entity"),
            ("end_type", "Type"),
            ("end_function", "Function"),
            ("end_procedure", "Procedure"),
            ("end_rule", "Rule"),
        ]
        .into_iter()
        .find(|(kw, _)| lowered.contains(kw))
        .map(|(_, class)| class)?;
        let mut candidates: Vec<&IndexEntry> = (0..=2)
            .flat_map(|back| self.ending_at(remark_line - back))
            .filter(|e| {
                e.class == kind
                    && e.end_line
                        .is_some_and(|end| end <= remark_line && end + 2 >= remark_line)
            })
            .collect();
        candidates.sort_by_key(|e| e.order);
        candidates
            .into_iter()
            .next()
            .or_else(|| self.entries.iter().find(|e| e.class == kind))
    }
}

fn collect(
    root: &Value,
    ptr: &str,
    owner_ptr: Option<String>,
    collection: Option<&'static str>,
    starts: &[usize],
    out: &mut Vec<IndexEntry>,
) {
    match root {
        Value::Object(map) => {
            if let (Some(class), Some(offset)) = (
                map.get("_class").and_then(Value::as_str),
                map.get("source_offset").and_then(Value::as_u64),
            ) {
                let suffix = class.rsplit("::").next().unwrap_or(class).to_string();
                if is_semantic(&suffix) {
                    out.push(IndexEntry {
                        ptr: ptr.to_string(),
                        class: suffix.clone(),
                        full_class: class.to_string(),
                        line: line_of(starts, offset),
                        end_line: None,
                        offset: Some(offset),
                        order: out.len(),
                        owner_ptr: owner_ptr.clone(),
                        collection,
                    });
                }
            }
            // The registry walk visits exactly the per-class collection
            // attributes (NodePositionIndex::COLLECTION_REGISTRY), never
            // single subobjects like an attribute's datatype or an
            // aggregate initializer's items. Case's registry names are
            // VIRTUAL collections: the walk crosses each action's
            // statement (and the otherwise body) directly, never the
            // position-less CaseAction wrapper or its labels.
            let class = map
                .get("_class")
                .and_then(Value::as_str)
                .and_then(|c| c.strip_prefix("Expressir::Model::"))
                .unwrap_or("");
            if class == "Statements::Case" {
                if let Some(Value::Array(items)) = map.get("actions") {
                    for (idx, item) in items.iter().enumerate() {
                        if let Some(stmt) = item.get("statement").filter(|v| !v.is_null()) {
                            collect(
                                stmt,
                                &format!("{ptr}/actions/{idx}/statement"),
                                Some(ptr.to_string()),
                                Some("action_statements"),
                                starts,
                                out,
                            );
                        }
                    }
                }
                if let Some(v) = map.get("otherwise_statement").filter(|v| !v.is_null()) {
                    collect(
                        v,
                        &format!("{ptr}/otherwise_statement"),
                        Some(ptr.to_string()),
                        Some("otherwise_statements"),
                        starts,
                        out,
                    );
                }
                return;
            }
            for key in collection_registry(class) {
                if let Some(Value::Array(items)) = map.get(*key) {
                    for (i, item) in items.iter().enumerate() {
                        collect(
                            item,
                            &format!("{ptr}/{key}/{i}"),
                            Some(ptr.to_string()),
                            Some(key),
                            starts,
                            out,
                        );
                    }
                }
            }
            // Schema#children: the pseudo-collection Ruby's index walks —
            // an interface's items and each type's enumeration items are
            // indexed as the schema's children even though no registry
            // collection carries them.
            if class == "Declarations::Schema" {
                if let Some(Value::Array(interfaces)) = map.get("interfaces") {
                    for (i, iface) in interfaces.iter().enumerate() {
                        if let Some(Value::Array(items)) = iface.get("items") {
                            for (j, item) in items.iter().enumerate() {
                                collect(
                                    item,
                                    &format!("{ptr}/interfaces/{i}/items/{j}"),
                                    Some(ptr.to_string()),
                                    Some("children"),
                                    starts,
                                    out,
                                );
                            }
                        }
                    }
                }
                if let Some(Value::Array(types)) = map.get("types") {
                    for (i, t) in types.iter().enumerate() {
                        let enum_items = t
                            .get("underlying_type")
                            .filter(|v| !v.is_null())
                            .filter(|ut| class_suffix(ut).is_some_and(|c| c == "Enumeration"))
                            .and_then(|ut| ut.get("items"))
                            .and_then(Value::as_array);
                        if let Some(items) = enum_items {
                            for (j, item) in items.iter().enumerate() {
                                collect(
                                    item,
                                    &format!("{ptr}/types/{i}/underlying_type/items/{j}"),
                                    Some(ptr.to_string()),
                                    Some("children"),
                                    starts,
                                    out,
                                );
                            }
                        }
                    }
                }
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                collect(item, &format!("{ptr}/{i}"), None, None, starts, out);
            }
        }
        _ => {}
    }
}

/// The per-class collection registry (`collection_attributes`
/// declarations — the single source of truth the Ruby walkers share).
pub fn collection_registry(class: &str) -> &'static [&'static str] {
    match class {
        "ExpFile" => &["schemas"],
        "Declarations::Schema" => &[
            "interfaces",
            "constants",
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "rules",
            "procedures",
            "remark_items",
        ],
        "Declarations::Entity" => &[
            "attributes",
            "derived_attributes",
            "inverse_attributes",
            "unique_rules",
            "where_rules",
            "informal_propositions",
            "remark_items",
        ],
        "Declarations::Type" => &["where_rules", "informal_propositions", "remark_items"],
        "Declarations::Function" | "Declarations::Procedure" => &[
            "parameters",
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "procedures",
            "constants",
            "variables",
            "statements",
            "remark_items",
        ],
        "Declarations::Rule" => &[
            "applies_to",
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "procedures",
            "constants",
            "variables",
            "statements",
            "where_rules",
            "informal_propositions",
            "remark_items",
        ],
        "Declarations::Interface" => &["items"],
        "Statements::If" => &["statements", "else_statements"],
        "Statements::Repeat" | "Statements::Alias" | "Statements::Compound" => &["statements"],
        _ => &[],
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;
    use crate::ParsedTree;

    const DEMO: &str =
        "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n  END_ENTITY;\nEND_SCHEMA;\n";

    #[test]
    fn index_entries_have_line_spans_and_order() {
        let source = DEMO;
        let wire = ParsedTree::parse(source)
            .expect("parses")
            .to_model_json("demo.exp")
            .expect("wire");
        let index = WireIndex::build(&wire, source);
        let entity = index
            .entries
            .iter()
            .find(|e| e.class == "Entity")
            .expect("entity entry");
        assert_eq!(entity.line, 2);
        // Ruby end_line = line_number(offset + formatted length): the
        // entity's formatted text spans its END_ENTITY line.
        assert_eq!(entity.end_line, Some(4));
        // Document order: the schema precedes its entity.
        let schema = index
            .entries
            .iter()
            .find(|e| e.class == "Schema")
            .expect("schema");
        assert!(schema.order < entity.order);
    }

    #[test]
    fn nearest_node_resolves_the_enclosing_scope() {
        let wire = ParsedTree::parse(DEMO)
            .expect("parses")
            .to_model_json("demo.exp")
            .expect("wire");
        let index = WireIndex::build(&wire, DEMO);
        // A remark line starting together with a node belongs to that
        // node (same_start wins) — the attribute on line 3.
        let nearest = index.nearest_node_to(3).expect("nearest");
        assert_eq!(nearest.class, "Attribute");
        // After the entity's END_ENTITY the remark line carries no node
        // span except the file container's (Ruby's index over formatted
        // lengths) — same_end wins on the ExpFile.
        let nearest = index.nearest_node_to(5).expect("nearest");
        assert_eq!(nearest.class, "ExpFile");
    }

    #[test]
    fn end_scope_lookup_matches_the_closing_line() {
        let wire = ParsedTree::parse(DEMO)
            .expect("parses")
            .to_model_json("demo.exp")
            .expect("wire");
        let index = WireIndex::build(&wire, DEMO);
        let matched = index
            .node_for_end_scope_at(4, "  END_ENTITY;")
            .expect("matched");
        assert_eq!(matched.class, "Entity");
        let matched = index
            .node_for_end_scope_at(5, "END_SCHEMA;")
            .expect("matched");
        assert_eq!(matched.class, "Schema");
    }
}

// ----- Region machinery substrate (statement regions, scope walk) -----

/// Placement constants (`Model::RemarkPlacement`).
pub const PLACEMENT_LEADING: &str = "leading";
pub const PLACEMENT_TRAILING: &str = "trailing";
pub const PLACEMENT_INLINE: &str = "inline";
pub const REGION_OPENER: &str = "opener";
pub const REGION_ELSE: &str = "else";
pub const REGION_OTHERWISE: &str = "otherwise";

/// Wire classes that can take a remark written after them on the same
/// line (`Model::TakesInlineRemark` = Statement + Interface).
#[allow(dead_code)] // staged: consumed by the untagged orchestrator
fn takes_inline_remark(class: &str) -> bool {
    // `class` may be the suffix or the full wire name; accept either.
    let has_ns = class.contains("::Statements::");
    let is_interface = class == "Interface" || class.ends_with("::Interface");
    has_ns || is_interface
}

/// Executable-statement owners for region lookup (`Model::Statement`
/// or function/rule/procedure heads.
fn is_statement_or_body_owner_full(full_class: &str) -> bool {
    full_class.contains("::Statements::")
        || full_class.ends_with("::Function")
        || full_class.ends_with("::Procedure")
        || full_class.ends_with("::Rule")
}

use regex::Regex;
use std::sync::LazyLock;

/// `MID_KEYWORD_REGIONS`: line-head ELSE / OTHERWISE.
static MID_KEYWORD_ELSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\AELSE\b").unwrap());
static MID_KEYWORD_OTHERWISE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\AOTHERWISE\b").unwrap());

/// `CLOSING_KEYWORDS` — which closing keyword ends which region of
/// which owner. Ordered like the Ruby hash (first match wins).
#[allow(dead_code)] // staged: consumed by the untagged orchestrator
static CLOSING_KEYWORDS: LazyLock<Vec<(Regex, &'static str, &'static str)>> = LazyLock::new(|| {
    vec![
        (Regex::new(r"(?i)\AELSE\b").unwrap(), "If", "statements"),
        (
            Regex::new(r"(?i)\AEND_IF\b").unwrap(),
            "If",
            "else_statements",
        ),
        (
            Regex::new(r"(?i)\AOTHERWISE\b").unwrap(),
            "Case",
            "action_statements",
        ),
        (
            Regex::new(r"(?i)\AEND_CASE\b").unwrap(),
            "Case",
            "otherwise_statements",
        ),
        (
            Regex::new(r"(?i)\AEND_REPEAT\b").unwrap(),
            "Repeat",
            "statements",
        ),
        (
            Regex::new(r"(?i)\AEND_ALIAS\b").unwrap(),
            "Alias",
            "statements",
        ),
        (
            Regex::new(r"(?i)\AEND\s*;").unwrap(),
            "Compound",
            "statements",
        ),
        (Regex::new(r"(?i)\AWHERE\b").unwrap(), "Rule", "statements"),
        (
            Regex::new(r"(?i)\AEND_FUNCTION\b").unwrap(),
            "Function",
            "statements",
        ),
        (
            Regex::new(r"(?i)\AEND_PROCEDURE\b").unwrap(),
            "Procedure",
            "statements",
        ),
        (
            Regex::new(r"(?i)\AEND_RULE\b").unwrap(),
            "Rule",
            "where_rules",
        ),
    ]
});

/// `OPENERS`: nestable-construct openers paired with their wire class;
/// the empty class marks entity/type heads (tracked, but produce no
/// statement node).
#[allow(dead_code)] // staged: consumed by the untagged orchestrator
static OPENERS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (Regex::new(r"(?i)\bIF\b.*?\bTHEN\b").unwrap(), "If"),
        (Regex::new(r"(?i)\bCASE\b.*?\bOF\b").unwrap(), "Case"),
        (Regex::new(r"(?i)\bREPEAT\b").unwrap(), "Repeat"),
        (Regex::new(r"(?i)\bALIAS\b").unwrap(), "Alias"),
        (Regex::new(r"(?i)\bBEGIN\b").unwrap(), "Compound"),
        (Regex::new(r"(?i)\A\s*FUNCTION\b").unwrap(), "Function"),
        (Regex::new(r"(?i)\A\s*PROCEDURE\b").unwrap(), "Procedure"),
        (Regex::new(r"(?i)\A\s*RULE\b").unwrap(), "Rule"),
        (Regex::new(r"(?i)\A\s*ENTITY\b").unwrap(), ""),
        (Regex::new(r"(?i)\A\s*TYPE\b").unwrap(), ""),
    ]
});

/// `CLOSERS`.
#[allow(dead_code)] // staged: consumed by the untagged orchestrator
static CLOSERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bEND_IF\b|\bEND_CASE\b|\bEND_REPEAT\b|\bEND_ALIAS\b|\bEND_FUNCTION\b|\bEND_PROCEDURE\b|\bEND_RULE\b|\bEND_ENTITY\b|\bEND_TYPE\b|\bEND\s*;",
    )
    .unwrap()
});

#[allow(dead_code)] // staged: consumed by the untagged orchestrator
static STRING_LITERAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"'[^']*'").unwrap());

/// `keyword_scannable`: string literals blanked, trailing remark cut.
#[allow(dead_code)] // staged: consumed by the untagged orchestrator
fn keyword_scannable(content: &str) -> String {
    let without_strings = STRING_LITERAL.replace_all(content, "''").into_owned();
    match without_strings.find("--") {
        Some(tail) => without_strings[..tail].to_string(),
        None => without_strings,
    }
}

/// `line_events`: opener/closer events on one line, ordered by offset.
/// Every occurrence counts.
#[allow(dead_code)] // staged: consumed by the untagged orchestrator
fn line_events(content: &str) -> Vec<(usize, bool, &'static str)> {
    let mut events = Vec::new();
    for (regex, class) in OPENERS.iter() {
        for m in regex.find_iter(content) {
            events.push((m.start(), true, *class));
        }
    }
    for m in CLOSERS.find_iter(content) {
        events.push((m.start(), false, ""));
    }
    events.sort_by_key(|(offset, _, _)| *offset);
    events
}

// ----- Untagged attachment -----

/// The statement-region collections (`STATEMENT_REGIONS`).
const STATEMENT_REGIONS: [&str; 4] = [
    "statements",
    "else_statements",
    "action_statements",
    "otherwise_statements",
];

static REGION_OPENER_ELSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\A(?:.*;)?\s*ELSE(?:\s*--.*)?\z").unwrap());
static REGION_OPENER_OTHERWISE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\A\s*OTHERWISE\b").unwrap());

/// `active_scope_map`: line => the construct open at the START of the
/// line, as (class, opening_line). Stack events come from
/// `line_events` over `keyword_scannable` content; blank and comment
/// lines are skipped for events but still record the current top.
fn active_scope_map(lines: &[&str]) -> Vec<Option<(&'static str, usize)>> {
    let mut map = Vec::with_capacity(lines.len() + 1);
    let mut stack: Vec<(&'static str, usize)> = Vec::new();
    for (idx, raw) in lines.iter().enumerate() {
        map.push(stack.last().copied());
        let content = keyword_scannable(raw.trim());
        if content.trim().is_empty() || content.trim_start().starts_with("--") {
            continue;
        }
        for (_, kind, class) in line_events(&content) {
            if kind {
                if !class.is_empty() {
                    stack.push((class, idx + 1));
                }
            } else {
                stack.pop();
            }
        }
    }
    map
}

/// `closing_keyword_after`: the first non-blank, non-comment line after
/// `line`, matched against CLOSING_KEYWORDS.
fn closing_keyword_after(
    lines: &[&str],
    line: usize,
) -> Option<(&'static str, &'static str, usize)> {
    let mut probe = line + 1;
    while probe <= lines.len() {
        let content = lines[probe - 1].trim();
        if !(content.is_empty() || content.starts_with("--")) {
            break;
        }
        probe += 1;
    }
    if probe > lines.len() {
        return None;
    }
    let content = lines[probe - 1].trim();
    for (regex, class, region) in CLOSING_KEYWORDS.iter() {
        if regex.is_match(content) {
            return Some((class, region, probe));
        }
    }
    None
}

/// `innermost_candidate`: drop candidates that are ancestors of others
/// (ownership is exact via JSON-pointer prefixes), then take the
/// smallest span.
fn innermost(candidates: Vec<&IndexEntry>) -> Option<&IndexEntry> {
    if candidates.len() <= 1 {
        return candidates.first().copied();
    }
    let is_ancestor =
        |a: &IndexEntry, b: &IndexEntry| b.ptr.starts_with(&a.ptr) && b.ptr.len() > a.ptr.len();
    let leaves: Vec<&IndexEntry> = candidates
        .iter()
        .filter(|cand| !candidates.iter().any(|other| is_ancestor(cand, other)))
        .copied()
        .collect();
    let pool = if leaves.is_empty() {
        &candidates
    } else {
        &leaves
    };
    pool.iter()
        .copied()
        .min_by_key(|e| e.end_line.unwrap_or(0) - e.line)
}

/// `statement_region_for`: the innermost statement-bearing node spanning
/// `line`, plus (when the line sits between two of its bodies) the
/// region entries the line fronts.
/// The statement region: the owning entry, the region entries (when the
/// line fronts one), and the region attribute.
type Region<'a> = Option<(
    &'a IndexEntry,
    Option<Vec<&'a IndexEntry>>,
    Option<&'static str>,
)>;

fn statement_region_for<'a>(index: &'a WireIndex, lines: &[&str], line: usize) -> Region<'a> {
    let mut candidates: Vec<&IndexEntry> = index
        .spanning(line)
        .into_iter()
        .filter(|e| e.line <= line && e.end_line.is_some_and(|end| end >= line))
        .filter(|e| is_statement_or_body_owner_full(&e.full_class))
        .collect();
    candidates.sort_by_key(|e| e.order);
    #[cfg(test)]
    if std::env::var("REGION_DBG").is_ok() {
        eprintln!(
            "candidates@{line}: {:?}",
            candidates
                .iter()
                .map(|e| format!("{} {}..{:?}", e.class, e.line, e.end_line))
                .collect::<Vec<_>>()
        );
    }
    let enclosing = innermost(candidates)?;
    let mut children: Vec<&IndexEntry> = index
        .entries
        .iter()
        .filter(|e| {
            e.owner_ptr.as_deref() == Some(enclosing.ptr.as_str())
                && e.collection.is_some_and(|c| STATEMENT_REGIONS.contains(&c))
        })
        .collect();
    children.sort_by_key(|e| e.order);
    if children.is_empty() {
        return Some((enclosing, None, None));
    }
    let preceding = children
        .iter()
        .filter(|e| e.line < line)
        .max_by_key(|e| e.order);
    let following = children
        .iter()
        .filter(|e| e.line > line)
        .min_by_key(|e| e.order);
    let region_attr = region_attr_for(lines, line, preceding.copied(), following.copied());
    let Some(region_attr) = region_attr else {
        return Some((enclosing, None, None));
    };
    let region_entries: Vec<&IndexEntry> = children
        .into_iter()
        .filter(|e| e.collection == Some(region_attr))
        .collect();
    Some((enclosing, Some(region_entries), Some(region_attr)))
}

/// `region_attr_for`: a comment between two regions of the same owner
/// sits on one side of the keyword opening the second region.
fn region_attr_for(
    lines: &[&str],
    line: usize,
    preceding: Option<&IndexEntry>,
    following: Option<&IndexEntry>,
) -> Option<&'static str> {
    let Some(preceding) = preceding else {
        return following.and_then(|f| f.collection);
    };
    let following_attr = following.and_then(|f| f.collection);
    if let Some(following_attr) = following_attr {
        let opener_match = match following_attr {
            "else_statements" => Some(&REGION_OPENER_ELSE as &LazyLock<Regex>),
            "otherwise_statements" => Some(&REGION_OPENER_OTHERWISE),
            _ => None,
        };
        if let Some(opener) = opener_match {
            if preceding.collection != Some(following_attr) {
                let f_line = following.map(|f| f.line);
                if let (Some(p_end), Some(f_line)) = (preceding.end_line, f_line) {
                    let opener_line = (p_end..f_line).find(|&ln| {
                        let content = lines[ln - 1].trim();
                        !content.starts_with("--") && opener.is_match(content)
                    });
                    if let Some(opener_line) = opener_line {
                        if line > opener_line {
                            return Some(following_attr);
                        }
                    }
                }
            }
        }
    }
    preceding.collection
}

/// `closing_region_target`: the keyword after the comment names the
/// owner type and the body being closed; the owner is the innermost
/// construct of that class still open at the keyword line.
#[allow(clippy::type_complexity)]
fn closing_region_target(
    root: &Value,
    index: &WireIndex,
    lines: &[&str],
    line: usize,
) -> (Option<String>, Option<&'static str>, Option<&'static str>) {
    let Some((owner_class, region, keyword_line)) = closing_keyword_after(lines, line) else {
        return (None, None, None);
    };
    let map = active_scope_map(lines);
    // The map is 0-indexed by line: entry k holds line k+1.
    let Some(active) = map.get(keyword_line - 1).copied().flatten() else {
        return (None, None, None);
    };
    if active.0 != owner_class {
        return (None, None, None);
    }
    let opener_line = active.1;
    let owner = index
        .starting_at(opener_line)
        .into_iter()
        .find(|e| e.class == owner_class);
    let Some(owner) = owner else {
        return (None, None, None);
    };
    // END_IF closes the THEN body when there is no ELSE; END_CASE closes
    // the last action when there is no OTHERWISE; END_RULE closes the
    // statements when the rule has no WHERE section.
    let region = match (owner_class, region) {
        ("If", "else_statements") => {
            let else_len = body_len(root, &owner.ptr, "else_statements");
            if else_len == 0 {
                "statements"
            } else {
                region
            }
        }
        ("Case", "otherwise_statements") => {
            let otherwise = root
                .pointer(&owner.ptr)
                .and_then(|n| n.get("otherwise_statement"))
                .is_some_and(|v| !v.is_null());
            if otherwise {
                region
            } else {
                "action_statements"
            }
        }
        ("Rule", "where_rules") => {
            let where_len = body_len(root, &owner.ptr, "where_rules");
            if where_len == 0 {
                "statements"
            } else {
                region
            }
        }
        _ => region,
    };
    (Some(owner.ptr.clone()), Some("trailing"), Some(region))
}

fn body_len(root: &Value, ptr: &str, attr: &str) -> usize {
    root.pointer(ptr)
        .and_then(|n| n.get(attr))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

/// `inline_remark?`: code precedes the remark on its own line.
fn inline_remark(lines: &[&str], line: usize) -> bool {
    let content = lines.get(line - 1).copied().unwrap_or("");
    match content.find("--") {
        Some(opener) => !content[..opener].trim().is_empty(),
        None => false,
    }
}

/// `inline_target` + `single_line_owner`/`opener_owner`/`closing_owner`.
#[allow(clippy::type_complexity)]
fn inline_target(
    index: &WireIndex,
    _lines: &[&str],
    remark_line: usize,
    remark_position: usize,
) -> (Option<String>, Option<&'static str>, Option<&'static str>) {
    let started_here: Vec<&IndexEntry> = index
        .starting_at(remark_line)
        .into_iter()
        .filter(|e| {
            takes_inline_remark(&e.full_class)
                && e.offset.is_some_and(|o| o < remark_position as u64)
        })
        .collect();
    if started_here.is_empty() {
        return (None, None, None);
    }
    let single_line = started_here
        .iter()
        .filter(|e| e.end_line == Some(remark_line))
        .max_by_key(|e| e.offset.unwrap_or(0));
    let opener = started_here
        .iter()
        .filter(|e| e.end_line.is_some_and(|end| end > remark_line))
        .max_by_key(|e| e.offset.unwrap_or(0));
    let began = match (single_line, opener) {
        (Some(a), Some(b)) => {
            if a.offset.unwrap_or(0) >= b.offset.unwrap_or(0) {
                Some(a)
            } else {
                Some(b)
            }
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let owner = match began {
        Some(b) => Some(*b),
        None => started_here
            .iter()
            .filter(|e| {
                e.end_line == Some(remark_line)
                    && e.line < remark_line
                    && e.offset.is_some_and(|o| o < remark_position as u64)
            })
            .max_by_key(|e| e.offset.unwrap_or(0))
            .copied(),
    };
    let Some(owner) = owner else {
        return (None, None, None);
    };
    let region = if owner.end_line.is_some_and(|end| end > remark_line) {
        Some(REGION_OPENER)
    } else {
        None
    };
    (Some(owner.ptr.clone()), Some("inline"), region)
}

/// `mid_keyword_inline_target`: `ELSE -- why` / `OTHERWISE : -- why`.
#[allow(clippy::type_complexity)]
fn mid_keyword_inline_target(
    index: &WireIndex,
    lines: &[&str],
    remark_line: usize,
) -> (Option<String>, Option<&'static str>, Option<&'static str>) {
    let content = lines.get(remark_line - 1).copied().unwrap_or("");
    let Some(opener) = content.find("--") else {
        return (None, None, None);
    };
    let head = content[..opener].trim();
    let (class, region) = if MID_KEYWORD_ELSE.is_match(head) {
        ("If", REGION_ELSE)
    } else if MID_KEYWORD_OTHERWISE.is_match(head) {
        ("Case", REGION_OTHERWISE)
    } else {
        return (None, None, None);
    };
    let innermost = index
        .entries
        .iter()
        .filter(|e| {
            e.class == class
                && e.line < remark_line
                && e.end_line.is_some_and(|end| end > remark_line)
        })
        .max_by_key(|e| e.line);
    match innermost {
        Some(entry) => (Some(entry.ptr.clone()), Some("inline"), Some(region)),
        None => (None, None, None),
    }
}

/// `attach_untagged_remarks`: end-of-scope lines, body comments, then
/// the nearest-node fallback.
pub fn attach_untagged_remarks(root: &mut Value, source: &str) -> Vec<usize> {
    let remarks = crate::remark::scan(source);
    let untagged: Vec<_> = remarks.iter().filter(|r| r.tag.is_none()).collect();
    if untagged.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = source.lines().collect();
    let index = WireIndex::build(root, source);
    let mut attached: Vec<usize> = Vec::new();

    for remark in untagged {
        if attached.contains(&remark.position) {
            continue;
        }
        let line_content = lines.get(remark.line - 1).copied().unwrap_or("");

        // A remark right after a scope's closing keyword closes it.
        if line_content.contains("END_")
            && [
                "END_SCHEMA",
                "END_ENTITY",
                "END_TYPE",
                "END_FUNCTION",
                "END_PROCEDURE",
                "END_RULE",
            ]
            .iter()
            .any(|kw| {
                line_content
                    .to_ascii_lowercase()
                    .contains(&kw.to_ascii_lowercase())
            })
        {
            let matched = index
                .node_for_end_scope_at(remark.line, line_content)
                .map(|e| e.ptr.clone());
            if let Some(ptr) = matched {
                if let Some(node) = root.pointer_mut(&ptr) {
                    add_remark_with_offset(node, &remark.text, remark.format, remark.position);
                    attached.push(remark.position);
                    continue;
                }
            }
        }

        // Body comments: inline, closing, or leading/trailing.
        let (target, placement, region) =
            find_body_comment_target(root, &index, &lines, remark.line, remark.position);
        if let Some(ptr) = target {
            if let Some(node) = root.pointer_mut(&ptr) {
                add_remark_with_placement(node, &remark.text, remark.format, placement, region);
                attached.push(remark.position);
                continue;
            }
        }

        // Legacy fallback: the nearest node to the remark line.
        if let Some(entry) = index.nearest_node_to(remark.line) {
            let ptr = entry.ptr.clone();
            if let Some(node) = root.pointer_mut(&ptr) {
                add_remark_with_offset(node, &remark.text, remark.format, remark.position);
                attached.push(remark.position);
            }
        }
    }
    attached
}

#[allow(clippy::type_complexity)]
fn find_body_comment_target(
    root: &Value,
    index: &WireIndex,
    lines: &[&str],
    line: usize,
    position: usize,
) -> (Option<String>, Option<&'static str>, Option<&'static str>) {
    if inline_remark(lines, line) {
        let (target, placement, region) = inline_target(index, lines, line, position);
        if target.is_some() {
            return (target, placement, region);
        }
        // Ruby returns `mid if mid` where mid is always an array —
        // truthy even when empty — so an inline remark whose inline and
        // mid-keyword targets both fail NEVER reaches the statement
        // region: it goes straight to the legacy nearest fallback.
        let mid = mid_keyword_inline_target(index, lines, line);
        return mid;
    }
    let closing = closing_region_target(root, index, lines, line);
    if closing.0.is_some() {
        return closing;
    }
    let Some((enclosing, region_entries, _region_attr)) = statement_region_for(index, lines, line)
    else {
        return (None, None, None);
    };
    // Ruby treats missing region entries as "not in a statement region"
    // and hands the remark to the legacy nearest fallback instead.
    let Some(region_entries) = region_entries else {
        return (None, None, None);
    };
    let following = region_entries
        .iter()
        .filter(|e| e.line > line)
        .min_by_key(|e| e.order);
    if let Some(following) = following {
        return (Some(following.ptr.clone()), Some("leading"), None);
    }
    (Some(enclosing.ptr.clone()), None, None)
}

/// `add_remark` with a source offset (tail-flavored, no placement).
fn add_remark_with_offset(node: &mut Value, text: &str, format: &str, offset: usize) {
    add_remark_tagged(node, text, None, Some(format), None, None, Some(offset));
}

/// `add_remark` with placement/region (embedded or body comments).
fn add_remark_with_placement(
    node: &mut Value,
    text: &str,
    format: &str,
    placement: Option<&'static str>,
    region: Option<&'static str>,
) {
    add_remark_tagged(node, text, None, Some(format), placement, region, None);
}

#[cfg(test)]
mod untagged_tests {
    use super::*;
    use crate::ParsedTree;

    fn wire_with_untagged(source: &str) -> Value {
        let mut wire = ParsedTree::parse(source)
            .expect("parses")
            .to_model_json("demo.exp")
            .expect("wire");
        attach_untagged_remarks(&mut wire, source);
        wire
    }

    #[test]
    fn remark_between_scopes_lands_on_the_file_root() {
        // A remark on its own line after END_ENTITY: no node spans the
        // line under Ruby's formatted-length index, so the fallback
        // nearest is the file container — ExpFile takes it untagged,
        // unplaced, and nothing renders it (conservation counts it as
        // dropped).
        let source = "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n  END_ENTITY;\n  -- why\nEND_SCHEMA;\n";
        let wire = wire_with_untagged(source);
        let entity = wire.pointer("/schemas/0/entities/0").expect("entity");
        assert!(
            entity.get("remarks").is_none(),
            "no plain remarks on entity"
        );
        let root = wire
            .get("untagged_remarks")
            .and_then(Value::as_array)
            .expect("root untagged");
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].get("text"), Some(&Value::String("why".to_string())));
    }
    #[test]
    fn inline_remark_on_an_end_scope_line_attaches_to_the_scope() {
        let source = "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n  END_ENTITY; -- why\nEND_SCHEMA;\n";
        let wire = wire_with_untagged(source);
        let entity = wire.pointer("/schemas/0/entities/0").expect("entity");
        let remarks = entity
            .get("remarks")
            .and_then(Value::as_array)
            .expect("remarks");
        assert_eq!(remarks[0], Value::String("why".to_string()));
    }

    #[test]
    fn inline_trailing_remark_follows_its_statement() {
        let source =
            "SCHEMA demo;\n  PROCEDURE p;\n    x := 1; -- why\n  END_PROCEDURE;\nEND_SCHEMA;\n";
        let wire = wire_with_untagged(source);
        // `x := 1; -- why` trails the Assignment: the remark lives in
        // untagged_remarks ONLY — statement classes carry no plain
        // remarks attribute (the formatter reads placement from here).
        let stmt = wire
            .pointer("/schemas/0/procedures/0/statements/0")
            .expect("statement");
        assert!(
            stmt.get("remarks").is_none(),
            "no plain remarks on Assignment"
        );
        let infos = stmt
            .get("untagged_remarks")
            .and_then(Value::as_array)
            .expect("infos");
        assert_eq!(infos[0].get("text").and_then(Value::as_str), Some("why"));
        assert_eq!(
            infos[0].get("placement").and_then(Value::as_str),
            Some("inline")
        );
    }

    #[test]
    fn body_comment_before_the_next_statement_is_leading() {
        // The next statement must carry remarks — only Alias and Repeat
        // do — for the LEADING attachment to be visible. The comment sits
        // directly above the Repeat with no covering statement span
        // between them (the region machinery's contract).
        let source = "SCHEMA demo;\n  PROCEDURE p;\n    -- why\n    REPEAT i := 1 TO 2;\n      x := x + 1;\n    END_REPEAT;\n  END_PROCEDURE;\nEND_SCHEMA;\n";
        let wire = wire_with_untagged(source);
        // An own-line comment before the next statement is LEADING on
        // that statement (the Ruby region machinery's contract).
        let stmt = wire
            .pointer("/schemas/0/procedures/0/statements/0")
            .expect("first statement");
        let remarks = stmt
            .get("remarks")
            .and_then(Value::as_array)
            .expect("remarks");
        assert_eq!(remarks[0], Value::String("why".to_string()));
        let infos = stmt
            .get("untagged_remarks")
            .and_then(Value::as_array)
            .expect("infos");
        assert_eq!(
            infos[0].get("placement").and_then(Value::as_str),
            Some("leading")
        );
    }

    #[test]
    fn remark_after_end_schema_lands_on_the_file_root() {
        // After END_SCHEMA no node spans the closing-note line; the
        // fallback nearest is the ExpFile container (Ruby lane verified:
        // untagged, unplaced, never rendered).
        let source = "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n  END_ENTITY;\nEND_SCHEMA;\n-- closing note\n";
        let wire = wire_with_untagged(source);
        let schema = wire.pointer("/schemas/0").expect("schema");
        assert!(
            schema.get("remarks").is_none(),
            "no plain remarks on schema"
        );
        let root = wire
            .get("untagged_remarks")
            .and_then(Value::as_array)
            .expect("root untagged");
        assert_eq!(root.len(), 1);
        assert_eq!(
            root[0].get("text"),
            Some(&Value::String("closing note".to_string()))
        );
    }
}
