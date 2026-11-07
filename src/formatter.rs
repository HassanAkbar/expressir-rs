//! The SourceFormatter over the wire (issue #22): renders an attached
//! model-hash wire back to EXPRESS text, byte-identical to Ruby's
//! `Expressir::Express::Formatter` (formatters/*.rb). Dispatch is a
//! match on the node's `_class`; remark emission mirrors
//! RemarkFormatter/RemarkItemFormatter (tagged remarks, the untagged
//! placement model — leading/trailing/inline with opener/else/
//! otherwise regions — and the block-end multiset subtraction).

use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

const INDENT: &str = "  ";

fn cls(node: &Value) -> &str {
    node.get("_class").and_then(Value::as_str).unwrap_or("")
}

fn tail(node: &Value) -> &str {
    let c = cls(node);
    c.rsplit("::").next().unwrap_or(c)
}

fn is(node: &Value, suffix: &str) -> bool {
    cls(node).ends_with(&format!("::{suffix}"))
}

fn id_of(node: &Value) -> Option<&str> {
    node.get("id").and_then(Value::as_str)
}

fn empty_array() -> &'static Vec<Value> {
    static EMPTY: OnceLock<Vec<Value>> = OnceLock::new();
    EMPTY.get_or_init(Vec::new)
}

fn arr<'a>(node: &'a Value, name: &str) -> &'a Vec<Value> {
    match node.get(name) {
        Some(Value::Array(items)) => items,
        _ => empty_array(),
    }
}

fn obj<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get(name).filter(|v| v.is_object())
}

fn str_fld<'a>(node: &'a Value, name: &str) -> Option<&'a str> {
    node.get(name).and_then(Value::as_str)
}

fn truthy(node: &Value, name: &str) -> bool {
    matches!(node.get(name), Some(Value::Bool(true)))
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn indent(text: &str) -> String {
    text.split('\n')
        .map(|line| format!("{INDENT}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn attrs_of(class: &str) -> &'static Vec<String> {
    static TABLE: OnceLock<std::collections::HashMap<String, Vec<String>>> = OnceLock::new();
    static EMPTY: Vec<String> = Vec::new();
    let table = TABLE.get_or_init(|| {
        serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(include_str!(
            "checker_attr_table.json"
        ))
        .expect("checker_attr_table.json parses")
    });
    table.get(class).unwrap_or(&EMPTY)
}

fn index_parents(node: &Value, ptr: &str, parent: &mut HashMap<String, String>) {
    for attr in attrs_of(cls(node)) {
        if attr == "parent" {
            continue;
        }
        match node.get(attr.as_str()) {
            Some(Value::Array(items)) => {
                for (i, item) in items.iter().enumerate() {
                    if item.is_object() {
                        let child = format!("{ptr}/{attr}/{i}");
                        parent.insert(child.clone(), ptr.to_string());
                        index_parents(item, &child, parent);
                    }
                }
            }
            Some(v) if v.is_object() => {
                let child = format!("{ptr}/{attr}");
                parent.insert(child.clone(), ptr.to_string());
                index_parents(v, &child, parent);
            }
            _ => {}
        }
    }
}

struct PathCtx<'a> {
    root: &'a Value,
    parent: HashMap<String, String>,
}

impl<'a> PathCtx<'a> {
    fn new(root: &'a Value) -> Self {
        let mut parent = HashMap::new();
        let count = root
            .get("schemas")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        for i in 0..count {
            let ptr = format!("/schemas/{i}");
            if let Some(node) = root.pointer(&ptr) {
                index_parents(node, &ptr, &mut parent);
            }
        }
        Self { root, parent }
    }

    fn node(&self, ptr: &str) -> &'a Value {
        static NULL: OnceLock<Value> = OnceLock::new();
        self.root
            .pointer(ptr)
            .unwrap_or_else(|| NULL.get_or_init(|| Value::Null))
    }
}

/// Ruby `ModelElement#path` over the wire.
fn node_path(ctx: &PathCtx, ptr: &str) -> Option<String> {
    let node = ctx.node(ptr);
    if is(node, "Statements::Alias")
        || is(node, "Statements::Repeat")
        || is(node, "Expressions::QueryExpression")
    {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    let mut current = ptr.to_string();
    loop {
        let node = ctx.node(&current);
        if !is(node, "References::SimpleReference") {
            if let Some(id) = id_of(node) {
                let is_ipr_shadow = is(node, "Declarations::RemarkItem")
                    && ctx
                        .parent
                        .get(&current)
                        .map(|p| {
                            let pnode = ctx.node(p);
                            is(pnode, "Declarations::InformalPropositionRule")
                                && id_of(pnode) == Some(id)
                        })
                        .unwrap_or(false);
                if !is_ipr_shadow {
                    parts.push(id.to_string());
                }
            }
        }
        match ctx.parent.get(&current) {
            Some(p) if !p.is_empty() => current = p.clone(),
            _ => break,
        }
    }
    if parts.is_empty() {
        None
    } else {
        parts.reverse();
        Some(parts.join("."))
    }
}

/// Renders the wire (an ExpFile) to EXPRESS text.
pub fn format(root: &Value, no_remarks: bool) -> String {
    let ctx = PathCtx::new(root);
    let mut f = Formatter {
        ctx: &ctx,
        no_remarks,
    };
    let schemas = arr(ctx.root, "schemas");
    let body = schemas
        .iter()
        .enumerate()
        .map(|(i, _)| f.dispatch(&format!("/schemas/{i}")))
        .collect::<Vec<_>>()
        .join("\n\n");
    if body.is_empty() {
        String::new()
    } else {
        format!("{body}\n")
    }
}

struct Formatter<'a> {
    ctx: &'a PathCtx<'a>,
    no_remarks: bool,
}

/// A RemarkInfo from the wire.
struct Remark<'a> {
    text: Option<&'a str>,
    tag: Option<&'a str>,
    format: Option<&'a str>,
    placement: Option<&'a str>,
    region: Option<&'a str>,
}

impl<'a> Remark<'a> {
    fn from(node: &'a Value) -> Self {
        Self {
            text: str_fld(node, "text"),
            tag: str_fld(node, "tag"),
            format: str_fld(node, "format"),
            placement: str_fld(node, "placement"),
            region: str_fld(node, "region"),
        }
    }

    fn tagged(&self) -> bool {
        self.tag.is_some()
    }

    fn tail(&self) -> bool {
        self.format == Some("tail")
    }

    fn tail_annotation(&self) -> bool {
        self.format == Some("tail_annotation")
    }

    fn embedded_annotation(&self) -> bool {
        self.format == Some("embedded_annotation")
    }

    /// Tail placement semantics — annotations sit where tails sit.
    fn tail_like(&self) -> bool {
        self.tail() || self.tail_annotation()
    }

    fn inline(&self) -> bool {
        self.placement == Some("inline")
    }

    fn leading(&self) -> bool {
        self.placement == Some("leading")
    }

    fn opener(&self) -> bool {
        self.inline() && self.region == Some("opener")
    }

    fn inline_region(&self, region: &str) -> bool {
        self.inline() && self.region == Some(region)
    }

    fn trailing_region(&self, region: &str) -> bool {
        self.placement == Some("trailing") && self.region == Some(region)
    }
}

impl<'a> Formatter<'a> {
    fn node(&self, ptr: &str) -> &'a Value {
        self.ctx.node(ptr)
    }

    fn dispatch(&mut self, ptr: &str) -> String {
        let node = self.node(ptr).clone();
        let body = self.format_body(ptr, &node);
        self.place_remarks(body, &node)
    }

    fn child(&mut self, base: &str, field: &str) -> String {
        self.dispatch(&format!("{base}/{field}"))
    }

    fn list_join(&mut self, base: &str, field: &str, sep: &str) -> String {
        let count = arr(self.node(base), field).len();
        (0..count)
            .map(|i| self.child(base, &format!("{field}/{i}")))
            .collect::<Vec<_>>()
            .join(sep)
    }

    fn format_body(&mut self, ptr: &str, node: &Value) -> String {
        let class = cls(node).to_string();
        let t = class.rsplit("::").next().unwrap_or("").to_string();
        match t.as_str() {
            "Schema" => self.format_schema(ptr, node),
            "Entity" => self.format_entity(ptr, node),
            "Type" => self.format_type(ptr, node),
            "Function" => self.format_function(ptr, node),
            "Procedure" => self.format_procedure(ptr, node),
            "Rule" => self.format_rule(ptr, node),
            "Interface" => self.format_interface(ptr, node),
            "InterfaceItem" => self.format_interface_item(ptr, node),
            "Constant" => self.format_constant(ptr, node),
            "Attribute" | "DerivedAttribute" | "InverseAttribute" => {
                self.format_attribute(ptr, node)
            }
            "Parameter" => self.format_parameter(ptr, node),
            "Variable" => self.format_variable(ptr, node),
            "UniqueRule" => self.format_unique_rule(ptr, node),
            "WhereRule" => self.format_where_rule(ptr, node),
            "InformalPropositionRule" => {
                id_of(node).map(|id| format!("{id}: ")).unwrap_or_default()
            }
            "SubtypeConstraint" => self.format_subtype_constraint(ptr, node),
            "SchemaVersion" => format!("'{}'", str_fld(node, "value").unwrap_or_default()),
            "RemarkItem" => self.format_remark_item(ptr, node),
            "EnumerationItem" => id_of(node).unwrap_or_default().to_string(),
            _ if class.contains("DataTypes::") => self.format_data_type(ptr, node, &class),
            _ if class.contains("Literals::") => self.format_literal(node),
            _ if class.contains("References::") => self.format_reference(ptr, node, &t),
            // SupertypeExpressions CONTAINS "Expressions::" as a
            // substring — match it before the expression dispatcher.
            _ if class.contains("SupertypeExpressions::") => self.format_supertype(ptr, node, &t),
            _ if class.contains("Expressions::") => self.format_expression(ptr, node, &t),
            _ if class.contains("Statements::") => self.format_statement(ptr, node, &t),
            _ => String::new(),
        }
    }

    // ---- declarations ----

    fn format_schema(&mut self, ptr: &str, node: &Value) -> String {
        let preamble = self.format_preamble_remarks(ptr, &indent(INDENT));
        let mut decls: Vec<String> = Vec::new();
        if !arr(node, "constants").is_empty() {
            let inner = self.list_join(ptr, "constants", "\n");
            decls.push(format!("CONSTANT\n{}\nEND_CONSTANT;", indent(&inner)));
        }
        for field in [
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "rules",
            "procedures",
        ] {
            let count = arr(node, field).len();
            for j in 0..count {
                decls.push(self.child(ptr, &format!("{field}/{j}")));
            }
        }
        let mut out = vec![self.format_schema_head(ptr, node)];
        if let Some(preamble) = preamble.filter(|p| !p.is_empty()) {
            out.push(String::new());
            out.push(preamble.trim_end().to_string());
            out.push(String::new());
        }
        if !decls.is_empty() {
            out.push(String::new());
            out.push(decls.join("\n\n"));
            out.push(String::new());
        }
        out.push(format!("END_SCHEMA;{}", self.format_end_scope_remark(node)));
        out.join("\n")
    }

    fn format_schema_head(&mut self, ptr: &str, node: &Value) -> String {
        let mut head = format!("SCHEMA {}", id_of(node).unwrap_or_default());
        if obj(node, "version").is_some() {
            head.push(' ');
            head.push_str(&self.child(ptr, "version"));
        }
        head.push(';');
        let mut out = vec![head];
        if !arr(node, "interfaces").is_empty() {
            out.push(String::new());
            out.push(self.list_join(ptr, "interfaces", "\n\n"));
        }
        out.join("\n")
    }

    fn format_entity(&mut self, ptr: &str, node: &Value) -> String {
        let mut items: Vec<String> = Vec::new();
        for field in ["attributes", "derived_attributes", "inverse_attributes"] {
            for (j, _a) in arr(node, field).iter().enumerate() {
                let item = format!("{ptr}/{field}/{j}");
                if !items.contains(&item) {
                    items.push(item);
                }
            }
        }
        let kind_of = |p: &str| {
            str_fld(self.node(p), "kind")
                .unwrap_or("EXPLICIT")
                .to_string()
        };
        let groups = |kind: &str| -> Vec<String> {
            items
                .iter()
                .filter(|p| kind_of(p) == kind)
                .cloned()
                .collect()
        };
        let explicit = groups("EXPLICIT");
        let derived = groups("DERIVED");
        let inverse = groups("INVERSE");

        let preamble = if !explicit.is_empty() {
            self.format_preamble_remarks(ptr, &indent(INDENT))
        } else {
            None
        };

        let mut header = format!("ENTITY {}", id_of(node).unwrap_or_default());
        let abstract_flag = truthy(node, "abstract");
        let has_supertype = obj(node, "supertype_expression").is_some();
        if abstract_flag && has_supertype {
            header.push_str(&format!(
                "\n{}",
                indent(&format!(
                    "ABSTRACT SUPERTYPE OF ({})",
                    self.child(ptr, "supertype_expression")
                ))
            ));
        } else if abstract_flag {
            header.push_str(&format!("\n{}", indent("ABSTRACT SUPERTYPE")));
        } else if has_supertype {
            header.push_str(&format!(
                "\n{}",
                indent(&format!(
                    "SUPERTYPE OF ({})",
                    self.child(ptr, "supertype_expression")
                ))
            ));
        }
        if !arr(node, "subtype_of").is_empty() {
            let subs = self.list_join(ptr, "subtype_of", ", ");
            header.push_str(&format!("\n{}", indent(&format!("SUBTYPE OF ({subs})"))));
        }
        header.push(';');

        let mut out = vec![header];
        if let Some(preamble) = preamble.filter(|p| !p.is_empty()) {
            out.push(preamble.trim_end().to_string());
        }
        if !explicit.is_empty() {
            let body = explicit
                .iter()
                .map(|p| self.dispatch(p))
                .collect::<Vec<_>>()
                .join("\n");
            out.push(indent(&body));
        }
        for (label, group) in [("DERIVE", derived), ("INVERSE", inverse)] {
            if !group.is_empty() {
                let body = group
                    .iter()
                    .map(|p| self.dispatch(p))
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push(format!("{label}\n{}", indent(&body)));
            }
        }
        for (label, field) in [("UNIQUE", "unique_rules"), ("WHERE", "where_rules")] {
            if !arr(node, field).is_empty() {
                let body = self.list_join(ptr, field, "\n");
                out.push(format!("{label}\n{}", indent(&body)));
            }
        }
        out.push(format!("END_ENTITY;{}", self.format_end_scope_remark(node)));
        out.join("\n")
    }

    fn format_type(&mut self, ptr: &str, node: &Value) -> String {
        let mut out = vec![format!(
            "TYPE {} = {};",
            id_of(node).unwrap_or_default(),
            self.child(ptr, "underlying_type")
        )];
        if !arr(node, "where_rules").is_empty() {
            let body = self.list_join(ptr, "where_rules", "\n");
            out.push(format!("WHERE\n{}", indent(&body)));
        }
        out.push(format!("END_TYPE;{}", self.format_end_scope_remark(node)));
        out.join("\n")
    }

    fn format_function(&mut self, ptr: &str, node: &Value) -> String {
        self.format_callable(ptr, node, "FUNCTION", "END_FUNCTION", true)
    }

    fn format_procedure(&mut self, ptr: &str, node: &Value) -> String {
        self.format_callable(ptr, node, "PROCEDURE", "END_PROCEDURE", false)
    }

    fn format_callable(
        &mut self,
        ptr: &str,
        node: &Value,
        keyword: &str,
        end_keyword: &str,
        is_function: bool,
    ) -> String {
        let id = id_of(node).unwrap_or_default();
        let mut head = format!("{keyword} {id}");
        if node
            .get("parameters")
            .map(|v| !v.is_null())
            .unwrap_or(false)
        {
            let param_indent = " ".repeat(format!("{keyword} {id}(").len());
            let count = arr(node, "parameters").len();
            let items = (0..count)
                .map(|i| self.child(ptr, &format!("parameters/{i}")))
                .collect::<Vec<_>>()
                .join(&format!(";\n{param_indent}"));
            head.push_str(&format!("({items})"));
        }
        if is_function {
            head.push_str(&format!(" : {};", self.child(ptr, "return_type")));
        } else {
            head.push(';');
        }

        let mut out = vec![head];
        for field in [
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "procedures",
        ] {
            if !arr(node, field).is_empty() {
                let body = self.list_join(ptr, field, "\n");
                out.push(indent(&body));
            }
        }
        if !arr(node, "constants").is_empty() {
            let inner = self.list_join(ptr, "constants", "\n");
            out.push(indent(&format!(
                "CONSTANT\n{}\nEND_CONSTANT;",
                indent(&inner)
            )));
        }
        if !arr(node, "variables").is_empty() {
            let inner = self.list_join(ptr, "variables", "\n");
            out.push(indent(&format!("LOCAL\n{}\nEND_LOCAL;", indent(&inner))));
        }
        if !arr(node, "statements").is_empty() {
            let body = self.list_join(ptr, "statements", "\n");
            out.push(indent(&body));
        }
        for remark in self.format_trailing_region_remarks(node, "statements") {
            out.push(indent(&remark));
        }
        out.push(format!(
            "{end_keyword};{}",
            self.format_end_scope_remark(node)
        ));
        out.join("\n")
    }

    fn format_rule(&mut self, ptr: &str, node: &Value) -> String {
        let applies = self.list_join(ptr, "applies_to", ", ");
        let mut out = vec![format!(
            "RULE {} FOR ({});",
            id_of(node).unwrap_or_default(),
            applies
        )];
        for field in [
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "procedures",
        ] {
            if !arr(node, field).is_empty() {
                let body = self.list_join(ptr, field, "\n");
                out.push(indent(&body));
            }
        }
        if !arr(node, "constants").is_empty() {
            let inner = self.list_join(ptr, "constants", "\n");
            out.push(indent(&format!(
                "CONSTANT\n{}\nEND_CONSTANT;",
                indent(&inner)
            )));
        }
        if !arr(node, "variables").is_empty() {
            let inner = self.list_join(ptr, "variables", "\n");
            out.push(indent(&format!("LOCAL\n{}\nEND_LOCAL;", indent(&inner))));
        }
        let stmt_count = arr(node, "statements").len();
        if stmt_count > 0 {
            let mut kept: Vec<String> = Vec::new();
            for j in 0..stmt_count {
                let stmt_ptr = format!("{ptr}/statements/{j}");
                if self.carries_placed_remark(&stmt_ptr) {
                    kept.push(stmt_ptr);
                    continue;
                }
                let stmt = self.node(&stmt_ptr).clone();
                if is(&stmt, "Statements::Alias") || is(&stmt, "Statements::Repeat") {
                    let all_null = arr(&stmt, "statements")
                        .iter()
                        .all(|s| is(s, "Statements::Null"));
                    if all_null {
                        continue;
                    }
                } else if is(&stmt, "Statements::Null") {
                    continue;
                }
                kept.push(stmt_ptr);
            }
            if !kept.is_empty() {
                let body = kept
                    .iter()
                    .map(|p| self.dispatch(p))
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push(indent(&body));
            }
        }
        for remark in self.format_trailing_region_remarks(node, "statements") {
            out.push(indent(&remark));
        }
        if !arr(node, "where_rules").is_empty() {
            let body = self.list_join(ptr, "where_rules", "\n");
            out.push(format!("WHERE\n{}", indent(&body)));
        }
        for remark in self.format_trailing_region_remarks(node, "where_rules") {
            out.push(indent(&remark));
        }
        out.push(format!("END_RULE;{}", self.format_end_scope_remark(node)));
        out.join("\n")
    }

    fn carries_placed_remark(&self, ptr: &str) -> bool {
        if self.no_remarks {
            return false;
        }
        arr(self.node(ptr), "untagged_remarks")
            .iter()
            .any(|r| Remark::from(r).placement.is_some())
    }

    fn format_interface(&mut self, ptr: &str, node: &Value) -> String {
        let kind = match str_fld(node, "kind") {
            Some("USE") => "USE",
            _ => "REFERENCE",
        };
        let mut out = format!("{kind} FROM {}", self.child(ptr, "schema"));
        if !arr(node, "items").is_empty() {
            let item_indent = " ".repeat("(".len());
            let count = arr(node, "items").len();
            let items = (0..count)
                .map(|i| self.child(ptr, &format!("items/{i}")))
                .collect::<Vec<_>>()
                .join(&format!(",\n{item_indent}"));
            out.push_str(&format!("\n{};", indent(&format!("({items})"))));
        } else {
            out.push(';');
        }
        out
    }

    fn format_interface_item(&mut self, ptr: &str, node: &Value) -> String {
        let mut out = self.child(ptr, "ref");
        if let Some(id) = id_of(node) {
            out.push_str(&format!(" AS {id}"));
        }
        out
    }

    fn format_constant(&mut self, ptr: &str, node: &Value) -> String {
        format!(
            "{} : {} := {};",
            id_of(node).unwrap_or_default(),
            self.child(ptr, "type"),
            self.child(ptr, "expression")
        )
    }

    fn format_attribute(&mut self, ptr: &str, node: &Value) -> String {
        let mut out = String::new();
        if obj(node, "supertype_attribute").is_some() {
            out.push_str(&self.child(ptr, "supertype_attribute"));
            out.push(' ');
            let supertype_attr_id = obj(node, "supertype_attribute")
                .and_then(|sa| obj(sa, "attribute"))
                .and_then(id_of);
            if supertype_attr_id.map(|attr_id| id_of(node) != Some(attr_id)) == Some(true) {
                out.push_str(&format!("RENAMED {} ", id_of(node).unwrap_or_default()));
            }
        } else if let Some(id) = id_of(node) {
            out.push_str(&format!("{id} "));
        }
        out.push(':');
        if truthy(node, "optional") {
            out.push_str(" OPTIONAL");
        }
        out.push(' ');
        out.push_str(&self.child(ptr, "type"));
        match str_fld(node, "kind") {
            Some("DERIVED") => out.push_str(&format!(" := {}", self.child(ptr, "expression"))),
            Some("INVERSE") => out.push_str(&format!(" FOR {}", self.child(ptr, "expression"))),
            _ => {}
        }
        out.push(';');
        out.push_str(&self.format_inline_tail_remark(node));
        out
    }

    fn format_parameter(&mut self, ptr: &str, node: &Value) -> String {
        let var = if truthy(node, "var") { "VAR " } else { "" };
        format!(
            "{var}{} : {}",
            id_of(node).unwrap_or_default(),
            self.child(ptr, "type")
        )
    }

    fn format_variable(&mut self, ptr: &str, node: &Value) -> String {
        let expr = if obj(node, "expression").is_some() {
            format!(" := {}", self.child(ptr, "expression"))
        } else {
            String::new()
        };
        format!(
            "{} : {}{expr};",
            id_of(node).unwrap_or_default(),
            self.child(ptr, "type")
        )
    }

    fn format_unique_rule(&mut self, ptr: &str, node: &Value) -> String {
        let label = id_of(node).map(|id| format!("{id}: ")).unwrap_or_default();
        format!("{label}{};", self.list_join(ptr, "attributes", ", "))
    }

    fn format_where_rule(&mut self, ptr: &str, node: &Value) -> String {
        let label = id_of(node).map(|id| format!("{id}: ")).unwrap_or_default();
        format!("{label}{};", self.child(ptr, "expression"))
    }

    fn format_subtype_constraint(&mut self, ptr: &str, node: &Value) -> String {
        let mut out = vec![format!(
            "SUBTYPE_CONSTRAINT {} FOR {};",
            id_of(node).unwrap_or_default(),
            self.child(ptr, "applies_to")
        )];
        if truthy(node, "abstract") {
            out.push(indent("ABSTRACT SUPERTYPE;"));
        }
        if !arr(node, "total_over").is_empty() {
            let items = self.list_join(ptr, "total_over", ", ");
            out.push(indent(&format!("TOTAL_OVER({items});")));
        }
        if obj(node, "supertype_expression").is_some() {
            out.push(indent(&format!(
                "{};",
                self.child(ptr, "supertype_expression")
            )));
        }
        out.push(format!(
            "END_SUBTYPE_CONSTRAINT;{}",
            self.format_end_scope_remark(node)
        ));
        out.join("\n")
    }

    fn format_remark_item(&self, ptr: &str, node: &Value) -> String {
        let remarks: Vec<String> = arr(node, "remarks")
            .iter()
            .filter_map(|r| r.as_str().map(str::to_string))
            .collect();
        if remarks.is_empty() {
            return String::new();
        }
        let path = node_path(self.ctx, ptr);
        let has_newlines = remarks.iter().any(|r| r.contains('\n'));
        if has_newlines {
            format!(
                "(*\"{}\"\n{}\n*)",
                path.unwrap_or_default(),
                remarks.join("\n")
            )
        } else {
            format!("--\"{}\"", path.unwrap_or_default())
        }
    }

    // ---- data types ----

    fn format_data_type(&mut self, ptr: &str, node: &Value, class: &str) -> String {
        match class.rsplit("::").next().unwrap_or("") {
            "Integer" => "INTEGER".to_string(),
            "Number" => "NUMBER".to_string(),
            "Boolean" => "BOOLEAN".to_string(),
            "Logical" => "LOGICAL".to_string(),
            "Real" => {
                if obj(node, "precision").is_some() {
                    format!("REAL ({})", self.child(ptr, "precision"))
                } else {
                    "REAL".to_string()
                }
            }
            "String" => {
                let mut out = "STRING".to_string();
                if obj(node, "width").is_some() {
                    out.push_str(&format!(" ({})", self.child(ptr, "width")));
                }
                if truthy(node, "fixed") {
                    out.push_str(" FIXED");
                }
                out
            }
            "Binary" => {
                let mut out = "BINARY".to_string();
                if obj(node, "width").is_some() {
                    out.push_str(&format!(" ({})", self.child(ptr, "width")));
                }
                if truthy(node, "fixed") {
                    out.push_str(" FIXED");
                }
                out
            }
            "List" => self.format_bound_type(ptr, "LIST", false),
            "Set" => self.format_bound_type(ptr, "SET", false),
            "Bag" => self.format_bound_type(ptr, "BAG", false),
            "Array" => self.format_bound_type(ptr, "ARRAY", true),
            "Enumeration" => self.format_enumeration(ptr, node),
            "Select" => self.format_select(ptr, node),
            "Aggregate" => {
                if obj(node, "base_type").is_some() {
                    format!("AGGREGATE OF {}", self.child(ptr, "base_type"))
                } else {
                    "AGGREGATE".to_string()
                }
            }
            "Generic" | "GenericEntity" => {
                let keyword = if class.ends_with("GenericEntity") {
                    "GENERIC_ENTITY"
                } else {
                    "GENERIC"
                };
                match id_of(node) {
                    Some(id) => format!("{keyword} : {id}"),
                    None => keyword.to_string(),
                }
            }
            _ => String::new(),
        }
    }

    fn format_bound_type(&mut self, ptr: &str, keyword: &str, is_array: bool) -> String {
        let node = self.node(ptr).clone();
        let mut out = keyword.to_string();
        if obj(&node, "bound1").is_some() && obj(&node, "bound2").is_some() {
            out.push_str(&format!(
                " [{}:{}]",
                self.child(ptr, "bound1"),
                self.child(ptr, "bound2")
            ));
        }
        out.push_str(" OF");
        if is_array {
            if truthy(&node, "optional") {
                out.push_str(" OPTIONAL");
            }
            if truthy(&node, "unique") {
                out.push_str(" UNIQUE");
            }
        } else if truthy(&node, "unique") {
            out.push_str(" UNIQUE");
        }
        out.push(' ');
        out.push_str(&self.child(ptr, "base_type"));
        out
    }

    fn format_enumeration(&mut self, ptr: &str, node: &Value) -> String {
        let mut out = String::new();
        if truthy(node, "extensible") {
            out.push_str("EXTENSIBLE ");
        }
        out.push_str("ENUMERATION");
        if obj(node, "based_on").is_some() {
            out.push_str(&format!(" BASED_ON {}", self.child(ptr, "based_on")));
            if !arr(node, "items").is_empty() {
                let items = self.indent_items(ptr, "items");
                out.push_str(&format!(" WITH\n{items}"));
            }
        } else if !arr(node, "items").is_empty() {
            out.push_str(" OF\n");
            out.push_str(&self.indent_items(ptr, "items"));
        }
        out
    }

    fn format_select(&mut self, ptr: &str, node: &Value) -> String {
        let mut out = String::new();
        if truthy(node, "extensible") {
            out.push_str("EXTENSIBLE ");
        }
        if truthy(node, "generic_entity") {
            out.push_str("GENERIC_ENTITY ");
        }
        out.push_str("SELECT");
        if obj(node, "based_on").is_some() {
            out.push_str(&format!(" BASED_ON {}", self.child(ptr, "based_on")));
            if !arr(node, "items").is_empty() {
                let items = self.indent_items(ptr, "items");
                out.push_str(&format!(" WITH\n{items}"));
            }
        } else if !arr(node, "items").is_empty() {
            out.push('\n');
            out.push_str(&self.indent_items(ptr, "items"));
        }
        out
    }

    /// `(item,\n  item)` with the one-space item indent, already wrapped
    /// by `indent(...)` — the shape every enumeration/select site emits.
    fn indent_items(&mut self, ptr: &str, field: &str) -> String {
        let item_indent = " ".repeat("(".len());
        let count = arr(self.node(ptr), field).len();
        let items = (0..count)
            .map(|i| self.child(ptr, &format!("{field}/{i}")))
            .collect::<Vec<_>>()
            .join(&format!(",\n{item_indent}"));
        indent(&format!("({items})"))
    }

    // ---- literals + references ----

    fn format_literal(&self, node: &Value) -> String {
        let t = tail(node);
        match t {
            "Logical" => match str_fld(node, "value") {
                Some("TRUE") => "TRUE".to_string(),
                Some("FALSE") => "FALSE".to_string(),
                _ => "UNKNOWN".to_string(),
            },
            "String" => {
                let value = node.get("value").map(value_text).unwrap_or_default();
                if truthy(node, "encoded") {
                    format!("\"{value}\"")
                } else {
                    format!("'{value}'")
                }
            }
            _ => node.get("value").map(value_text).unwrap_or_default(),
        }
    }

    fn format_reference(&mut self, ptr: &str, node: &Value, t: &str) -> String {
        match t {
            "SimpleReference" => id_of(node).unwrap_or_default().to_string(),
            "AttributeReference" => format!(
                "{}.{}",
                self.child(ptr, "ref"),
                self.child(ptr, "attribute")
            ),
            "GroupReference" => {
                format!("{}\\{}", self.child(ptr, "ref"), self.child(ptr, "entity"))
            }
            "IndexReference" => {
                let idx2 = if obj(node, "index2").is_some() {
                    format!(":{}", self.child(ptr, "index2"))
                } else {
                    String::new()
                };
                format!(
                    "{}[{}{}]",
                    self.child(ptr, "ref"),
                    self.child(ptr, "index1"),
                    idx2
                )
            }
            _ => String::new(),
        }
    }

    // ---- expressions ----

    fn operator_precedence(operator: &str) -> i32 {
        match operator {
            "EXPONENTIATION" => 1,
            "MULTIPLICATION" | "REAL_DIVISION" | "INTEGER_DIVISION" | "MODULO" | "AND"
            | "COMBINE" => 2,
            "SUBTRACTION" | "ADDITION" | "OR" | "XOR" => 3,
            "EQUAL"
            | "NOT_EQUAL"
            | "LESS_THAN_OR_EQUAL"
            | "GREATER_THAN_OR_EQUAL"
            | "LESS_THAN"
            | "GREATER_THAN"
            | "INSTANCE_EQUAL"
            | "INSTANCE_NOT_EQUAL"
            | "IN"
            | "LIKE" => 4,
            _ => 0,
        }
    }

    fn format_expression(&mut self, ptr: &str, node: &Value, t: &str) -> String {
        match t {
            "BinaryExpression" => {
                let operator = str_fld(node, "operator").unwrap_or("").to_string();
                let precedence = Self::operator_precedence(&operator);
                let wraps = |field: &str| {
                    obj(node, field)
                        .map(|o| {
                            is(o, "Expressions::BinaryExpression")
                                && Self::operator_precedence(str_fld(o, "operator").unwrap_or(""))
                                    >= precedence
                        })
                        .unwrap_or(false)
                };
                let op1 = wraps("operand1");
                let op2 = wraps("operand2");
                let spelling = match operator.as_str() {
                    "ADDITION" => "+",
                    "AND" => "AND",
                    "COMBINE" => "||",
                    "EQUAL" => "=",
                    "EXPONENTIATION" => "**",
                    "GREATER_THAN" => ">",
                    "GREATER_THAN_OR_EQUAL" => ">=",
                    "IN" => "IN",
                    "INSTANCE_EQUAL" => ":=:",
                    "INSTANCE_NOT_EQUAL" => ":<>:",
                    "INTEGER_DIVISION" => "DIV",
                    "LESS_THAN" => "<",
                    "LESS_THAN_OR_EQUAL" => "<=",
                    "LIKE" => "LIKE",
                    "MODULO" => "MOD",
                    "MULTIPLICATION" => "*",
                    "NOT_EQUAL" => "<>",
                    "OR" => "OR",
                    "REAL_DIVISION" => "/",
                    "SUBTRACTION" => "-",
                    "XOR" => "XOR",
                    other => other,
                };
                let left = if op1 {
                    format!("({})", self.child(ptr, "operand1"))
                } else {
                    self.child(ptr, "operand1")
                };
                let right = if op2 {
                    format!("({})", self.child(ptr, "operand2"))
                } else {
                    self.child(ptr, "operand2")
                };
                format!("{left} {spelling} {right}")
            }
            "UnaryExpression" => {
                let operator = str_fld(node, "operator").unwrap_or("").to_string();
                let operand = self.node(&format!("{ptr}/operand")).clone();
                let needs_parens = is(&operand, "Expressions::BinaryExpression")
                    || is(&operand, "Expressions::Interval")
                    || is(&operand, "Expressions::UnaryExpression");
                let sign = match operator.as_str() {
                    "MINUS" => "-",
                    "NOT" => "NOT",
                    "PLUS" => "+",
                    other => other,
                };
                let space = if operator == "NOT" { " " } else { "" };
                let inner = self.child(ptr, "operand");
                if needs_parens {
                    format!("{sign}{space}({inner})")
                } else {
                    format!("{sign}{space}{inner}")
                }
            }
            "Interval" => {
                let op = |v: Option<&str>| {
                    match v {
                        Some("LESS_THAN") => "<",
                        _ => "<=",
                    }
                    .to_string()
                };
                format!(
                    "{{{} {} {} {} {}}}",
                    self.child(ptr, "low"),
                    op(str_fld(node, "operator1")),
                    self.child(ptr, "item"),
                    op(str_fld(node, "operator2")),
                    self.child(ptr, "high"),
                )
            }
            "AggregateInitializer" => {
                let items = self.list_join(ptr, "items", ", ");
                format!("[{items}]")
            }
            "AggregateInitializerItem" => {
                format!(
                    "{}:{}",
                    self.child(ptr, "expression"),
                    self.child(ptr, "repetition")
                )
            }
            "EntityConstructor" => {
                let params = self.list_join(ptr, "parameters", ", ");
                format!("{}({})", self.child(ptr, "entity"), params)
            }
            "FunctionCall" => {
                let params = if !node
                    .get("parameters")
                    .map(|v| !v.is_null())
                    .unwrap_or(false)
                {
                    String::new()
                } else {
                    format!("({})", self.list_join(ptr, "parameters", ", "))
                };
                format!("{}{}", self.child(ptr, "function"), params)
            }
            "QueryExpression" => {
                let remarks = self.format_remarks(ptr, node);
                let remark_block = if remarks.is_empty() {
                    String::new()
                } else {
                    format!("\n{}\n", remarks.join("\n"))
                };
                format!(
                    "QUERY({} <* {} | {}{})",
                    id_of(node).unwrap_or_default(),
                    self.child(ptr, "aggregate_source"),
                    self.child(ptr, "expression"),
                    remark_block
                )
            }
            _ => String::new(),
        }
    }

    // ---- statements ----

    fn format_statement(&mut self, ptr: &str, node: &Value, t: &str) -> String {
        match t {
            "Alias" => {
                let mut out = vec![format!(
                    "ALIAS {} FOR {};",
                    id_of(node).unwrap_or_default(),
                    self.child(ptr, "expression")
                )];
                if !arr(node, "statements").is_empty() {
                    let body = self.list_join(ptr, "statements", "\n");
                    out.push(indent(&body));
                }
                for remark in self.format_block_end_remarks(ptr, node) {
                    out.push(indent(&remark));
                }
                out.push("END_ALIAS;".to_string());
                out.join("\n")
            }
            "Assignment" => format!(
                "{} := {};",
                self.child(ptr, "ref"),
                self.child(ptr, "expression")
            ),
            "Case" => {
                let mut out = vec![format!("CASE {} OF", self.child(ptr, "expression"))];
                let action_count = arr(node, "actions").len();
                for j in 0..action_count {
                    out.push(self.child(ptr, &format!("actions/{j}")));
                }
                for remark in self.format_trailing_region_remarks(node, "action_statements") {
                    out.push(indent(&remark));
                }
                if obj(node, "otherwise_statement").is_some() {
                    let mid = self.format_mid_keyword_remarks(node, "otherwise");
                    out.push(format!("OTHERWISE :{mid}"));
                    out.push(indent(&self.child(ptr, "otherwise_statement")));
                }
                for remark in self.format_trailing_region_remarks(node, "otherwise_statements") {
                    out.push(indent(&remark));
                }
                out.push("END_CASE;".to_string());
                out.join("\n")
            }
            "CaseAction" => format!(
                "{} :\n{}",
                self.list_join(ptr, "labels", ", "),
                indent(&self.child(ptr, "statement"))
            ),
            "Compound" => {
                let mut out = vec!["BEGIN".to_string()];
                if !arr(node, "statements").is_empty() {
                    let body = self.list_join(ptr, "statements", "\n");
                    out.push(indent(&body));
                }
                for remark in self.format_trailing_region_remarks(node, "statements") {
                    out.push(indent(&remark));
                }
                out.push("END;".to_string());
                out.join("\n")
            }
            "Escape" => "ESCAPE;".to_string(),
            "If" => {
                let has_else = !arr(node, "else_statements").is_empty();
                let then_trailing: Vec<String> = self
                    .format_trailing_region_remarks(node, "statements")
                    .into_iter()
                    .map(|x| indent(&x))
                    .collect();
                let else_trailing: Vec<String> = self
                    .format_trailing_region_remarks(node, "else_statements")
                    .into_iter()
                    .map(|x| indent(&x))
                    .collect();
                let mut out = vec![format!("IF {} THEN", self.child(ptr, "expression"))];
                if !arr(node, "statements").is_empty() {
                    let body = self.list_join(ptr, "statements", "\n");
                    out.push(indent(&body));
                }
                if has_else {
                    out.extend(then_trailing.clone());
                    let mid = self.format_mid_keyword_remarks(node, "else");
                    let else_body = self.list_join(ptr, "else_statements", "\n");
                    out.push(format!("ELSE{mid}\n{}", indent(&else_body)));
                    out.extend(else_trailing);
                } else {
                    out.extend(then_trailing);
                }
                out.push("END_IF;".to_string());
                out.join("\n")
            }
            "Null" => ";".to_string(),
            "ProcedureCall" => {
                let params = if arr(node, "parameters").is_empty() {
                    String::new()
                } else {
                    format!("({})", self.list_join(ptr, "parameters", ", "))
                };
                format!("{}{};", self.child(ptr, "procedure"), params)
            }
            "Repeat" => {
                let mut head = "REPEAT".to_string();
                if let Some(id) = id_of(node) {
                    head.push_str(&format!(
                        " {id} := {} TO {}",
                        self.child(ptr, "bound1"),
                        self.child(ptr, "bound2")
                    ));
                    if obj(node, "increment").is_some() {
                        head.push_str(&format!(" BY {}", self.child(ptr, "increment")));
                    }
                }
                if obj(node, "while_expression").is_some() {
                    head.push_str(&format!(" WHILE {}", self.child(ptr, "while_expression")));
                }
                if obj(node, "until_expression").is_some() {
                    head.push_str(&format!(" UNTIL {}", self.child(ptr, "until_expression")));
                }
                head.push(';');
                let mut out = vec![head];
                if !arr(node, "statements").is_empty() {
                    let body = self.list_join(ptr, "statements", "\n");
                    out.push(indent(&body));
                }
                for remark in self.format_block_end_remarks(ptr, node) {
                    out.push(indent(&remark));
                }
                out.push("END_REPEAT;".to_string());
                out.join("\n")
            }
            "Return" => {
                let expr = if obj(node, "expression").is_some() {
                    format!(" ({})", self.child(ptr, "expression"))
                } else {
                    String::new()
                };
                format!("RETURN{expr};")
            }
            "Skip" => "SKIP;".to_string(),
            _ => String::new(),
        }
    }

    fn format_supertype(&mut self, ptr: &str, node: &Value, t: &str) -> String {
        match t {
            "BinarySupertypeExpression" => {
                let precedence = |operator: &str| match operator {
                    "AND" => 1,
                    "ANDOR" => 2,
                    _ => 0,
                };
                let operator = str_fld(node, "operator").unwrap_or("").to_string();
                let wraps = |field: &str| {
                    obj(node, field)
                        .map(|o| {
                            is(o, "SupertypeExpressions::BinarySupertypeExpression")
                                && precedence(str_fld(o, "operator").unwrap_or(""))
                                    > precedence(&operator)
                        })
                        .unwrap_or(false)
                };
                let op1 = wraps("operand1");
                let op2 = wraps("operand2");
                let spelling = operator.as_str();
                let left = if op1 {
                    format!("({})", self.child(ptr, "operand1"))
                } else {
                    self.child(ptr, "operand1")
                };
                let right = if op2 {
                    format!("({})", self.child(ptr, "operand2"))
                } else {
                    self.child(ptr, "operand2")
                };
                format!("{left} {spelling} {right}")
            }
            "OneofSupertypeExpression" => {
                let operands = self.list_join(ptr, "operands", ", ");
                format!("ONEOF({operands})")
            }
            _ => String::new(),
        }
    }

    // ---- remarks ----

    fn remark_target(&self, ptr: &str, node: &Value) -> String {
        node_path(self.ctx, ptr)
            .or_else(|| id_of(node).map(str::to_string))
            .unwrap_or_default()
    }

    fn format_tagged_remark(&self, ptr: &str, node: &Value, remark: &str) -> String {
        let target = self.remark_target(ptr, node);
        if remark.contains('\n') {
            format!("(*\"{target}\"\n{remark}*)")
        } else if node_path(self.ctx, ptr).is_none() && id_of(node).unwrap_or("").contains("IP") {
            format!("--{} {}", id_of(node).unwrap_or_default(), remark)
        } else {
            format!("--\"{target}\" {remark}")
        }
    }

    fn format_untagged_remark(&self, remark: &Remark) -> String {
        let Some(text) = remark.text.filter(|t| !t.is_empty()) else {
            return String::new();
        };
        let formatted = if remark.tagged() {
            format!("\"{}\" {text}", remark.tag.unwrap_or_default())
        } else {
            text.to_string()
        };
        if remark.tail_annotation() {
            format!("--- {formatted}")
        } else if remark.tail() {
            format!("-- {formatted}")
        } else if remark.embedded_annotation() && text.contains('\n') {
            format!("(**\n{formatted}\n*)")
        } else if remark.embedded_annotation() {
            format!("(** {formatted} *)")
        } else if text.contains('\n') {
            format!("(*\n{formatted}\n*)")
        } else {
            format!("(* {formatted} *)")
        }
    }

    fn format_inline_tail_remark(&self, node: &Value) -> String {
        if self.no_remarks {
            return String::new();
        }
        let remark = arr(node, "untagged_remarks").first().map(Remark::from);
        let Some(remark) = remark else {
            return String::new();
        };
        let Some(text) = remark.text.filter(|t| !t.is_empty()) else {
            return String::new();
        };
        let formatted = if remark.tagged() {
            format!("\"{}\" {text}", remark.tag.unwrap_or_default())
        } else {
            text.to_string()
        };
        if remark.tail_annotation() {
            format!(" --- {formatted}")
        } else if remark.tail() {
            format!(" -- {formatted}")
        } else if remark.embedded_annotation() {
            format!(" (** {formatted} *)")
        } else {
            format!(" (* {formatted} *)")
        }
    }

    fn format_end_scope_remark(&self, node: &Value) -> String {
        if self.no_remarks {
            return String::new();
        }
        let remark = arr(node, "untagged_remarks")
            .iter()
            .map(Remark::from)
            .rfind(|r| r.placement.is_none());
        let Some(remark) = remark else {
            return String::new();
        };
        let Some(text) = remark.text.filter(|t| !t.is_empty()) else {
            return String::new();
        };
        if !remark.tail_like() {
            return String::new();
        }
        let formatted = if remark.tagged() {
            format!("\"{}\" {text}", remark.tag.unwrap_or_default())
        } else {
            text.to_string()
        };
        if remark.tail_annotation() {
            format!(" --- {formatted}")
        } else {
            format!(" -- {formatted}")
        }
    }

    fn format_preamble_remarks(&self, ptr: &str, indent_str: &str) -> Option<String> {
        if self.no_remarks {
            return None;
        }
        let node = self.node(ptr);
        let mut remarks: Vec<Remark> = arr(node, "untagged_remarks")
            .iter()
            .map(Remark::from)
            .filter(|r| r.text.map(|t| !t.is_empty()).unwrap_or(false))
            .collect();
        if remarks.is_empty() {
            return None;
        }
        let is_scope_container = matches!(
            tail(node),
            "Schema" | "Entity" | "Type" | "Function" | "Procedure" | "Rule"
        );
        if is_scope_container && remarks.last().unwrap().tail_like() {
            remarks.pop();
        }
        if remarks.is_empty() {
            return None;
        }
        Some(
            remarks
                .iter()
                .map(|remark| self.format_preamble_remark(remark, indent_str))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    fn format_preamble_remark(&self, remark: &Remark, indent_str: &str) -> String {
        let text = remark.text.unwrap_or_default();
        let text = if remark.tagged() {
            format!("\"{}\" {text}", remark.tag.unwrap_or_default())
        } else {
            text.to_string()
        };
        if remark.tail_annotation() {
            format!("{indent_str}--- {text}")
        } else if remark.tail() {
            format!("{indent_str}-- {text}")
        } else if remark.embedded_annotation() && text.contains('\n') {
            let lines: Vec<String> = text
                .split('\n')
                .map(|line| format!("{indent_str}  {}", line.trim()))
                .collect();
            format!("{indent_str}(**\n{}\n{indent_str}**)", lines.join("\n"))
        } else if remark.embedded_annotation() {
            format!("{indent_str}(** {text} *)")
        } else if text.contains('\n') {
            let lines: Vec<String> = text
                .split('\n')
                .map(|line| format!("{indent_str}  {}", line.trim()))
                .collect();
            format!("{indent_str}(*\n{}\n{indent_str}*)", lines.join("\n"))
        } else {
            format!("{indent_str}(* {text} *)")
        }
    }

    fn format_trailing_region_remarks(&self, node: &Value, region: &str) -> Vec<String> {
        if self.no_remarks {
            return Vec::new();
        }
        arr(node, "untagged_remarks")
            .iter()
            .map(Remark::from)
            .filter(|r| r.trailing_region(region))
            .map(|r| self.format_untagged_remark(&r))
            .filter(|t| !t.is_empty())
            .collect()
    }

    fn format_mid_keyword_remarks(&self, node: &Value, region: &str) -> String {
        self.formatted_inline_remarks(node, &|r: &Remark| r.inline_region(region))
    }

    fn formatted_inline_remarks(&self, node: &Value, accept: &dyn Fn(&Remark) -> bool) -> String {
        if self.no_remarks {
            return String::new();
        }
        arr(node, "untagged_remarks")
            .iter()
            .map(Remark::from)
            .filter(|r| r.inline() && accept(r))
            .map(|r| self.format_untagged_remark(&r))
            .filter(|t| !t.is_empty())
            .map(|t| format!(" {t}"))
            .collect::<Vec<_>>()
            .join("")
    }

    fn format_leading_statement_remarks(&self, node: &Value) -> Vec<String> {
        if self.no_remarks {
            return Vec::new();
        }
        arr(node, "untagged_remarks")
            .iter()
            .map(Remark::from)
            .filter(|r| r.leading())
            .map(|r| self.format_untagged_remark(&r))
            .filter(|t| !t.is_empty())
            .collect()
    }

    /// Block-end emission for ALIAS/REPEAT bodies: tagged remarks are
    /// stored as bare text in node.remarks alongside a mirror of every
    /// untagged text (add_remark dual-store), so "genuinely tagged" is
    /// derivable only by multiset-subtracting the untagged texts.
    fn format_block_end_remarks(&mut self, ptr: &str, node: &Value) -> Vec<String> {
        if self.no_remarks {
            return Vec::new();
        }
        let untagged: Vec<Remark> = arr(node, "untagged_remarks")
            .iter()
            .map(Remark::from)
            .collect();
        let mut remaining: HashMap<String, i32> = HashMap::new();
        for r in &untagged {
            *remaining
                .entry(r.text.unwrap_or_default().to_string())
                .or_insert(0) += 1;
        }
        let tagged: Vec<String> = arr(node, "remarks")
            .iter()
            .filter_map(|r| r.as_str())
            // Ruby's `reject { consume if remaining positive }` — a
            // text is genuinely tagged when the untagged mirror does
            // NOT cover it.
            .filter(|text| {
                let count = remaining.entry(text.to_string()).or_insert(0);
                if *count > 0 {
                    *count -= 1;
                    false
                } else {
                    true
                }
            })
            .map(|text| self.format_tagged_remark(ptr, node, text))
            .collect();
        let rest: Vec<String> = untagged
            .iter()
            .filter(|r| !r.leading() && !r.inline())
            .map(|r| self.format_untagged_remark(r))
            .filter(|t| !t.is_empty())
            .collect();
        let mut out = tagged;
        out.extend(rest);
        out
    }

    fn format_remarks(&mut self, ptr: &str, node: &Value) -> Vec<String> {
        let mut out = Vec::new();
        if !self.no_remarks {
            for text in arr(node, "remarks").iter().filter_map(|r| r.as_str()) {
                out.push(self.format_tagged_remark(ptr, node, text));
            }
            let t = tail(node);
            let skip_untagged = matches!(
                t,
                "Attribute"
                    | "DerivedAttribute"
                    | "InverseAttribute"
                    | "EnumerationItem"
                    | "Schema"
                    | "Entity"
                    | "Type"
                    | "Function"
                    | "Procedure"
                    | "Rule"
            );
            if !skip_untagged {
                for remark in arr(node, "untagged_remarks").iter() {
                    out.push(self.format_untagged_remark(&Remark::from(remark)));
                }
            }
        }
        out
    }

    // ---- placement glue (Formatter#format's place_remarks) ----

    fn place_remarks(&mut self, mut formatted: String, node: &Value) -> String {
        let inline = self.formatted_inline_remarks(node, &|r: &Remark| r.region.is_none());
        if !inline.is_empty() {
            formatted.push_str(&inline);
        }
        let opener = self.formatted_inline_remarks(node, &|r: &Remark| r.opener());
        if !opener.is_empty() {
            formatted = match formatted.split_once('\n') {
                Some((head, rest)) => format!("{head}{opener}\n{rest}"),
                None => format!("{formatted}{opener}"),
            };
        }
        let leading = self.format_leading_statement_remarks(node);
        if leading.is_empty() {
            formatted
        } else {
            let mut lines = leading;
            lines.push(formatted);
            lines.join("\n")
        }
    }
}
