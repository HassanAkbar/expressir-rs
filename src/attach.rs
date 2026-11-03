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

/// The named-child collections each wire `_class` exposes for path
/// resolution — the Ruby collection registry + Schema's
/// SCOPE_DECL_COLLECTIONS, keyed by the class-name suffix.
fn child_collections(class: &str) -> &'static [&'static str] {
    match class.rsplit("::").next().unwrap_or(class) {
        "Schema" => &[
            "constants",
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "procedures",
            "rules",
        ],
        "Entity" => &[
            "attributes",
            "derived_attributes",
            "inverse_attributes",
            "unique_rules",
            "where_rules",
            "informal_propositions",
        ],
        "Type" => &["enumeration_items", "where_rules"],
        "Function" | "Procedure" => &["parameters"],
        "Rule" => &["where_rules"],
        "Constant" => &["items"],
        "SubtypeConstraint" => &["where_rules"],
        _ => &[],
    }
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
            | "Interface"
            | "Aggregate"
            | "Generic"
            | "GenericEntity"
            | "QueryExpression"
            | "AliasStatement"
            | "RepeatStatement"
    )
}

/// `ModelElement#find` over the wire: downcase the parts, strip
/// `prefix:`es, then resolve schema → registered child collections by
/// id. Returns the JSON pointer to the resolved node.
fn exact_path_pointer(root: &Value, path: &str) -> Option<String> {
    let parts: Vec<String> = path
        .split('.')
        .map(|p| p.rsplit(':').next().unwrap_or(p).to_ascii_lowercase())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let schemas = root.get("schemas")?.as_array()?;
    let (index, _) = schemas.iter().enumerate().find(|(_, s)| {
        s.get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.eq_ignore_ascii_case(&parts[0]))
    })?;
    let mut pointer = format!("/schemas/{index}");
    let mut current = &schemas[index];
    for part in &parts[1..] {
        let class = current.get("_class")?.as_str()?.to_string();
        let mut found = None;
        for attr in child_collections(&class) {
            let Some(items) = current.get(*attr).and_then(Value::as_array) else {
                continue;
            };
            for (i, item) in items.iter().enumerate() {
                if item
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.eq_ignore_ascii_case(part))
                {
                    found = Some((attr, i));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        let (attr, i) = found?;
        pointer.push_str(&format!("/{attr}/{i}"));
        current = current.get(attr)?.get(i)?;
    }
    Some(pointer)
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

/// Append `text` to the node's `remarks` (Ruby `add_remark`: dropped
/// when the class carries no remarks or the text is blank).
pub fn add_remark_to(node: &mut Value, text: &str) -> bool {
    let Some(class) = node.get("_class").and_then(Value::as_str) else {
        return false;
    };
    if !supports_remarks(class) || text.trim().is_empty() {
        return false;
    }
    if node.get("remarks").and_then(Value::as_array).is_none() {
        node["remarks"] = Value::Array(Vec::new());
    }
    match node.get_mut("remarks").and_then(Value::as_array_mut) {
        Some(items) => {
            items.push(Value::String(text.to_string()));
            true
        }
        None => false,
    }
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
            assert!(add_remark_to(target, "a remark on the attribute"));
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
        assert!(!add_remark_to(schema, "   "));
        // The ExpFile wrapper node (no _class match) rejects.
        assert!(!add_remark_to(&mut wire, "nope"));
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

/// `create_item_at_parent`: IP-shaped ids redirect to informal
/// propositions; everything else finds-or-creates a RemarkItem.
fn create_or_find_item(root: &mut Value, parent_ptr: &str, item_id: &str) -> Option<String> {
    if let Some(ip_id) = extract_ip_id(item_id) {
        if let Some(ptr) = create_or_find_informal_proposition(root, parent_ptr, &ip_id) {
            return Some(ptr);
        }
    }
    let parent_ptr = parent_ptr.to_string();
    let item_id = item_id.to_string();
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
    let mut attached_spans: Vec<usize> = Vec::new();

    for remark in tagged {
        if attached_spans.contains(&remark.position) {
            continue;
        }
        let tag = remark.tag.as_deref().unwrap_or_default();
        let scope_name = containing_scope_name(&scope_map, remark.line);
        let mut scope_ptr = scope_name.and_then(|name| scope_node_pointer(root, name));
        // Source-text fallback when the name lookup missed (IP tags on
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
                        if let Some(schema_id) = scope_owner_schema_id(scope_ptr) {
                            let scope_path =
                                format!("{schema_id}.{}", scope_name.unwrap_or_default());
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
                    target = scope_child_pointer(root, scope_ptr, tag);
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
                        if let Some(schema_id) = scope_owner_schema_id(scope_ptr) {
                            let scope_path =
                                format!("{schema_id}.{}", scope_name.unwrap_or_default());
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
                if add_remark_to(target_node, &remark.text) {
                    attached_spans.push(remark.position);
                }
            }
        }
    }

    attached_spans
}

/// `find_node_in_scope`: the pointer of the scope child with this id.
fn scope_child_pointer(root: &Value, scope_ptr: &str, tag: &str) -> Option<String> {
    let scope = root.pointer(scope_ptr)?;
    let class = class_suffix(scope)?.to_string();
    for attr in child_collections(&class) {
        if let Some(items) = scope.get(*attr).and_then(Value::as_array) {
            for (i, item) in items.iter().enumerate() {
                if item
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.eq_ignore_ascii_case(tag))
                {
                    return Some(format!("{scope_ptr}/{attr}/{i}"));
                }
            }
        }
    }
    None
}

/// The id of the schema owning a scope pointer ("/schemas/i/...").
fn scope_owner_schema_id(scope_ptr: &str) -> Option<String> {
    if !scope_ptr.starts_with("/schemas/") {
        return None;
    }
    let index: usize = scope_ptr
        .trim_start_matches("/schemas/")
        .split('/')
        .next()?
        .parse()
        .ok()?;
    Some(format!("schemas/{index}"))
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
    pub line: usize,
    pub end_line: Option<usize>,
    pub offset: Option<u64>,
    pub order: usize,
}

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
        collect(root, "$", &starts, &mut entries);
        entries.sort_by_key(|e| (e.offset.unwrap_or(u64::MAX), e.order));
        for (i, entry) in entries.iter_mut().enumerate() {
            entry.order = i;
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
            // ExpFile is a file-level container: remarks inside a schema
            // belong to the schema.
            containing.retain(|e| e.class != "ExpFile");
            let candidates: Vec<&IndexEntry> = if containing.is_empty() {
                self.spanning(remark_line)
                    .into_iter()
                    .filter(|e| is_remark_scope(&e.class))
                    .collect()
            } else {
                containing
            };
            candidates
                .into_iter()
                .min_by_key(|e| e.end_line.unwrap_or(0) - e.line)
        } else {
            let before = self.semantic_ending_before(remark_line);
            if let Some(last) = before.last() {
                let max_end = last.end_line.unwrap_or(0);
                return before
                    .iter()
                    .rev()
                    .find(|e| e.end_line == Some(max_end))
                    .copied();
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

fn collect(root: &Value, ptr: &str, starts: &[usize], out: &mut Vec<IndexEntry>) {
    match root {
        Value::Object(map) => {
            if let (Some(class), Some(offset)) = (
                map.get("_class").and_then(Value::as_str),
                map.get("source_offset").and_then(Value::as_u64),
            ) {
                let suffix = class.rsplit("::").next().unwrap_or(class).to_string();
                if is_semantic(&suffix) {
                    let end_line = map
                        .get("source_end_offset")
                        .and_then(Value::as_u64)
                        .map(|end| line_of(starts, end));
                    out.push(IndexEntry {
                        ptr: ptr.to_string(),
                        class: suffix,
                        line: line_of(starts, offset),
                        end_line,
                        offset: Some(offset),
                        order: out.len(),
                    });
                }
            }
            // Only collection attributes are indexed — the Ruby
            // registry walk visits collection children, never single
            // (non-collection) subobjects like an attribute's datatype.
            for (key, value) in map {
                if let Value::Array(items) = value {
                    for (i, item) in items.iter().enumerate() {
                        collect(item, &format!("{ptr}/{key}/{i}"), starts, out);
                    }
                }
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                collect(item, &format!("{ptr}/{i}"), starts, out);
            }
        }
        _ => {}
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
        // After END_SCHEMA, the last-ended scope is the schema.
        let nearest = index.nearest_node_to(5).expect("nearest");
        assert_eq!(nearest.class, "Schema");
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
    class.contains("::Statements::") || class == "Interface"
}

/// Executable-statement owners for region lookup (`Model::Statement`
/// or function/rule/procedure heads).
#[allow(dead_code)] // staged: consumed by the untagged orchestrator
fn is_statement_or_body_owner(class: &str) -> bool {
    class.contains("::Statements::") || matches!(class, "Function" | "Procedure" | "Rule")
}

use regex::Regex;
use std::sync::LazyLock;

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
