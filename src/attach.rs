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
