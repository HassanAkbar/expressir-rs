//! Ruby `Expressir::Express::Checker` over the wire (issue #21).
//!
//! Emits the canonical finding shape `validate check` serializes:
//! `{id, severity, schema, message}`, in the Ruby order. Walk-order
//! parity rides the generated class→ordered-attributes table
//! (`checker_attr_table.json`) plus Ruby's LIFO stack semantics in
//! `each_node`.
//!
//! The reference resolver (`ResolveReferencesModelVisitor`) is ported
//! alongside so `check_unresolved_refs` sees the same `base_path`
//! fills the Ruby pipeline produces (`Parser.from_files` resolves,
//! then checks).

mod families;
mod resolve;
mod types;

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct CheckerNote {
    pub id: String,
    pub severity: &'static str,
    pub schema: Option<String>,
    pub message: String,
}

/// `validate check`'s JSON finding shape.
impl CheckerNote {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "severity": self.severity,
            "schema": self.schema,
            "message": self.message,
        })
    }
}

static ATTR_TABLE: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
static EMPTY_ATTRS: Vec<String> = Vec::new();
static EMPTY_ARRAY: OnceLock<Vec<Value>> = OnceLock::new();

fn attrs_of(class: &str) -> &'static Vec<String> {
    let table = ATTR_TABLE.get_or_init(|| {
        serde_json::from_str::<HashMap<String, Vec<String>>>(include_str!(
            "../checker_attr_table.json"
        ))
        .expect("checker_attr_table.json parses")
    });
    table.get(class).unwrap_or(&EMPTY_ATTRS)
}

fn empty_array() -> &'static Vec<Value> {
    EMPTY_ARRAY.get_or_init(Vec::new)
}

pub(crate) fn cls(node: &Value) -> &str {
    node.get("_class").and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn class_tail(node: &Value) -> &str {
    let c = cls(node);
    c.rsplit("::").next().unwrap_or(c)
}

pub(crate) fn is(node: &Value, suffix: &str) -> bool {
    cls(node).ends_with(&format!("::{suffix}"))
}

pub(crate) fn safe_downcase(s: &str) -> String {
    s.to_lowercase()
}

pub(crate) fn id_of(node: &Value) -> Option<&str> {
    node.get("id").and_then(Value::as_str)
}

pub(crate) fn arr<'a>(node: &'a Value, name: &str) -> &'a Vec<Value> {
    match node.get(name) {
        Some(Value::Array(items)) => items,
        _ => empty_array(),
    }
}

pub(crate) fn obj<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get(name).filter(|v| v.is_object())
}

pub(crate) fn str_fld<'a>(node: &'a Value, name: &str) -> Option<&'a str> {
    node.get(name).and_then(Value::as_str)
}

pub(crate) fn fld<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get(name)
}

pub(crate) fn same_pair(left: &str, right: &str, kind: &str) -> bool {
    left == kind && right == kind
}

pub(crate) const BUILTINS: &[&str] = &[
    "integer",
    "real",
    "number",
    "string",
    "binary",
    "boolean",
    "logical",
    "generic",
    "generic_entity",
    "aggregate",
    "array",
    "bag",
    "list",
    "set",
    "true",
    "false",
    "unknown",
    "self",
    "const_e",
    "pi",
    "abs",
    "acos",
    "asin",
    "atan",
    "cos",
    "exp",
    "format",
    "hibound",
    "hiindex",
    "length",
    "log",
    "log2",
    "log10",
    "lobound",
    "loindex",
    "nvl",
    "odd",
    "rolesof",
    "sin",
    "sizeof",
    "sqrt",
    "tan",
    "typeof",
    "usedin",
    "value",
    "value_in",
    "value_unique",
    "exists",
    "blength",
    "insert",
    "remove",
];

pub(crate) const USE_ITEM_KINDS: &[&str] = &["Entity", "Type"];
pub(crate) const REFERENCE_ITEM_KINDS: &[&str] =
    &["Constant", "Entity", "Function", "Procedure", "Type"];

const NUMERIC_TYPES: &[&str] = &["integer", "real", "number"];
const LOGICAL_TYPES: &[&str] = &["logical", "boolean"];

const ORDERING_FAMILIES: &[&[&str]] = &[
    &["integer", "real", "number"],
    &["logical", "boolean"],
    &["string"],
    &["binary"],
    &["enum"],
];

pub(crate) fn builtin(id: &str) -> bool {
    BUILTINS.contains(&safe_downcase(id).as_str())
}

pub(crate) fn is_numeric(t: &str) -> bool {
    NUMERIC_TYPES.contains(&t)
}

pub(crate) fn is_logical(t: &str) -> bool {
    LOGICAL_TYPES.contains(&t)
}

pub(crate) fn same_ordering_family(left: &str, right: &str) -> bool {
    ORDERING_FAMILIES
        .iter()
        .any(|family| family.contains(&left) && family.contains(&right))
}

pub(crate) fn numeric_pair(left: &str, right: &str) -> bool {
    is_numeric(left) && is_numeric(right)
}

pub(crate) fn undecidable_1(t: &str) -> bool {
    t == "unknown" || t == "generic"
}

pub(crate) fn undecidable_2(left: &str, right: &str) -> bool {
    undecidable_1(left) || undecidable_1(right)
}

/// Node pointers in the exact order Ruby's `each_node` yields: a LIFO
/// stack over the class attribute table — children push in table
/// order, pops take the last, so visiting reverses both attributes
/// and array items.
pub(crate) fn each_node(root: &Value, start_ptr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![start_ptr.to_string()];
    while let Some(ptr) = stack.pop() {
        out.push(ptr.clone());
        let Some(node) = root.pointer(&ptr) else {
            continue;
        };
        for attr in attrs_of(cls(node)) {
            if attr == "parent" {
                continue;
            }
            match node.get(attr.as_str()) {
                Some(Value::Array(items)) => {
                    for (i, item) in items.iter().enumerate() {
                        if item.is_object() {
                            stack.push(format!("{ptr}/{attr}/{i}"));
                        }
                    }
                }
                Some(v) if v.is_object() => {
                    stack.push(format!("{ptr}/{attr}"));
                }
                _ => {}
            }
        }
    }
    out
}

/// `each_string`: Literals::String values anywhere under `ptr`.
pub(crate) fn collect_strings(root: &Value, ptr: &str, out: &mut Vec<(String, String)>) {
    let Some(node) = root.pointer(ptr) else {
        return;
    };
    if is(node, "Literals::String") {
        let value = str_fld(node, "value").unwrap_or("").to_string();
        out.push((ptr.to_string(), value));
    }
    for attr in attrs_of(cls(node)) {
        if attr == "parent" {
            continue;
        }
        match node.get(attr.as_str()) {
            Some(Value::Array(items)) => {
                for (i, item) in items.iter().enumerate() {
                    if item.is_object() {
                        collect_strings(root, &format!("{ptr}/{attr}/{i}"), out);
                    }
                }
            }
            Some(v) if v.is_object() => {
                collect_strings(root, &format!("{ptr}/{attr}"), out);
            }
            _ => {}
        }
    }
}

/// `each_reference`: SimpleReferences anywhere under `ptr`, in the
/// same recursive attribute order as Ruby.
pub(crate) fn collect_simple_refs(root: &Value, ptr: &str, out: &mut Vec<String>) {
    let Some(node) = root.pointer(ptr) else {
        return;
    };
    if is(node, "References::SimpleReference") {
        out.push(ptr.to_string());
    }
    for attr in attrs_of(cls(node)) {
        if attr == "parent" {
            continue;
        }
        match node.get(attr.as_str()) {
            Some(Value::Array(items)) => {
                for (i, item) in items.iter().enumerate() {
                    if item.is_object() {
                        collect_simple_refs(root, &format!("{ptr}/{attr}/{i}"), out);
                    }
                }
            }
            Some(v) if v.is_object() => {
                collect_simple_refs(root, &format!("{ptr}/{attr}"), out);
            }
            _ => {}
        }
    }
}

pub fn check(root: &mut Value, dialect: Option<&str>) -> Vec<CheckerNote> {
    // Phase 1 (immutable): collect the resolver's base_path fills.
    let fills = {
        let ctx = Ctx::new(root);
        ctx.collect_reference_fills()
    };
    apply_fills(root, fills);
    // Phase 2: the checks over the resolved wire.
    let mut ctx = Ctx::new(root);
    ctx.prepare_maps();
    for i in 0..ctx.schemas.len() {
        ctx.check_schema(i, dialect);
    }
    ctx.notes
}

fn apply_fills(root: &mut Value, fills: Vec<(String, Option<String>)>) {
    for (ptr, path) in fills {
        if let Some(node) = root.pointer_mut(&ptr) {
            match path {
                Some(p) => node["base_path"] = Value::String(p),
                None => {
                    if let Some(obj) = node.as_object_mut() {
                        obj.remove("base_path");
                    }
                }
            }
        }
    }
}

/// Debug hook: expansion of schema `idx`'s interface `k`.
#[doc(hidden)]
pub fn debug_expansion(root: &Value, idx: usize, k: usize) -> Vec<(String, String)> {
    let ctx = Ctx::new(root);
    let iface_ptr = format!("/schemas/{idx}/interfaces/{k}");
    let mut visited = std::collections::HashSet::new();
    ctx.debug_expansion_pub(&iface_ptr, &mut visited)
}

/// Debug hook: the folded child ids of schema `idx`.
#[doc(hidden)]
pub fn debug_children(root: &Value, idx: usize) -> Vec<String> {
    let ctx = Ctx::new(root);
    let mut ids: Vec<String> = ctx.schema_children_by_id_inner(idx).into_keys().collect();
    ids.sort();
    ids
}

/// Debug hook: the resolver fills for every SimpleReference whose id
/// matches `name`.
#[doc(hidden)]
pub fn debug_fills(root: &Value, name: &str) -> Vec<(String, Option<String>)> {
    let ctx = Ctx::new(root);
    let mut out = Vec::new();
    for i in 0..ctx.schemas.len() {
        let root_ptr = ctx.schema_ptr(i).to_string();
        for ptr in each_node(root, &root_ptr) {
            let node = ctx.node(&ptr);
            if is(node, "References::SimpleReference") && id_of(node) == Some(name) {
                let base = ctx.find(&ptr, name);
                let path = base.as_ref().and_then(|b| ctx.node_path(b));
                let parent = ctx.parent.get(&ptr).cloned().unwrap_or_default();
                let parent_cls = if parent.is_empty() {
                    "?".to_string()
                } else {
                    class_tail(ctx.node(&parent)).to_string()
                };
                println!("ref ptr={ptr} parent={parent_cls} base={base:?} path={path:?}");
                out.push((ptr, path));
            }
        }
    }
    out
}

pub(crate) struct Ctx<'a> {
    pub root: &'a Value,
    pub parent: HashMap<String, String>,
    pub schemas: Vec<(String, String)>,
    pub by_name: HashMap<String, Vec<usize>>,
    pub notes: Vec<CheckerNote>,
    pub alias_maps: HashMap<String, HashMap<String, (usize, String)>>,
    pub bound_ids: HashMap<String, HashSet<String>>,
    pub children_cache: HashMap<String, HashMap<String, String>>,
    pub subtype_clean: HashMap<String, HashMap<String, bool>>,
    pub env_stack: Vec<HashMap<String, String>>,
}

impl<'a> Ctx<'a> {
    pub fn node(&self, ptr: &str) -> &'a Value {
        static NULL: OnceLock<Value> = OnceLock::new();
        let null = NULL.get_or_init(|| Value::Null);
        self.root.pointer(ptr).unwrap_or(null)
    }

    pub fn schema_ptr(&self, i: usize) -> &str {
        &self.schemas[i].0
    }

    pub fn schema(&self, i: usize) -> &'a Value {
        self.node(self.schema_ptr(i))
    }

    pub fn note(&mut self, id: &str, severity: &'static str, schema: usize, message: String) {
        self.notes.push(CheckerNote {
            id: id.to_string(),
            severity,
            schema: Some(self.schemas[schema].1.clone()),
            message,
        });
    }

    /// The active expression environment (scope name → type-node ptr).
    pub fn env(&self) -> Option<&HashMap<String, String>> {
        self.env_stack.last()
    }
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

impl<'a> Ctx<'a> {
    fn new(root: &'a Value) -> Self {
        let mut parent: HashMap<String, String> = HashMap::new();
        let mut schemas: Vec<(String, String)> = Vec::new();
        // Two wire shapes: a single ExpFile ({schemas: [...]}) or a
        // repository ({files: [{schemas: [...]}]}) — the Ruby Checker
        // sees the whole repository, so cross-file interfaces resolve.
        let mut schema_ptrs: Vec<String> = Vec::new();
        if let Some(files) = root.get("files").and_then(Value::as_array) {
            for (i, _file) in files.iter().enumerate() {
                let count = root
                    .pointer(&format!("/files/{i}/schemas"))
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                for j in 0..count {
                    schema_ptrs.push(format!("/files/{i}/schemas/{j}"));
                }
            }
        } else {
            let count = root
                .get("schemas")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            for i in 0..count {
                schema_ptrs.push(format!("/schemas/{i}"));
            }
        }
        for ptr in &schema_ptrs {
            if let Some(node) = root.pointer(ptr) {
                parent.insert(ptr.clone(), String::new());
                index_parents(node, ptr, &mut parent);
                let id = id_of(node).unwrap_or_default().to_string();
                schemas.push((ptr.clone(), id));
            }
        }
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, (_, id)) in schemas.iter().enumerate() {
            if id.is_empty() {
                continue;
            }
            by_name.entry(safe_downcase(id)).or_default().push(i);
        }
        Self {
            root,
            parent,
            schemas,
            by_name,
            notes: Vec::new(),
            alias_maps: HashMap::new(),
            bound_ids: HashMap::new(),
            children_cache: HashMap::new(),
            subtype_clean: HashMap::new(),
            env_stack: Vec::new(),
        }
    }
}
