//! Emits the sparse lutaml-model wire JSON for the surface the walker
//! covers: the exact shape `Expressir::Model::ExpFile#to_hash` produces
//! on the Ruby side, so Ruby can hydrate the Rust-extracted model with
//! `ExpFile.from_hash` and get a model indistinguishable from the Ruby
//! parse path.
//!
//! Submodules mirror the Ruby model hierarchy (MECE by concern):
//! - `declarations` — schema-level declarations and algorithm heads
//! - `data_types`   — parameter/instantiable/underlying types
//! - `expressions`  — the expression chain, literals, references
//! - `statements`   — algorithmic statements
//!
//! Sparse emission: lutaml-model omits nil/default attributes from
//! to_hash, so keys are written only when present, and every node
//! carries its `_class` marker. `base_path` is intentionally NOT
//! emitted — ResolveReferencesModelVisitor annotates resolved
//! references after hydration, exactly as it does for the Ruby-built
//! model.

mod data_types;
mod declarations;
mod expressions;
mod statements;

use serde_json::{Map, Value};

use parsanol::portable::{AstArena, AstNode};

use crate::extract::{rule_id_str, unquote};
use crate::walk::{as_list, hash_get, hash_pairs, nested_text, text};

pub(crate) const CLASS_EXP_FILE: &str = "Expressir::Model::ExpFile";
pub(crate) const CLASS_SCHEMA: &str = "Expressir::Model::Declarations::Schema";
pub(crate) const CLASS_SCHEMA_VERSION: &str = "Expressir::Model::Declarations::SchemaVersion";
pub(crate) const CLASS_SCHEMA_VERSION_ITEM: &str =
    "Expressir::Model::Declarations::SchemaVersionItem";
pub(crate) const CLASS_INTERFACE: &str = "Expressir::Model::Declarations::Interface";
pub(crate) const CLASS_INTERFACE_ITEM: &str = "Expressir::Model::Declarations::InterfaceItem";
pub(crate) const CLASS_SIMPLE_REFERENCE: &str = "Expressir::Model::References::SimpleReference";

#[derive(Debug)]
pub enum ModelJsonError {
    MissingNode,
}

pub fn model_json(arena: &AstArena, root: &AstNode, path: &str) -> Result<Value, ModelJsonError> {
    let mut schemas = Vec::new();

    if let Some(syntax) = hash_get(arena, root, "syntax") {
        if let Some(schema_decls) = hash_get(arena, &syntax, "schemaDecl") {
            for wrapped in as_list(arena, &schema_decls) {
                if let Some(inner) = hash_get(arena, &wrapped, "schemaDecl") {
                    schemas.push(schema_json(arena, &inner, path)?);
                }
            }
        }
    }
    // syntax_builder's result is attached like every registry dispatch,
    // so the ExpFile carries the syntax subtree's first-slice offset —
    // NodePositionIndex needs it to route pre-schema remarks to the file
    // (header transfer) instead of the schema.
    let mut root_map = node(CLASS_EXP_FILE);
    root_map.insert("path".into(), Value::String(path.into()));
    root_map.insert("schemas".into(), Value::Array(schemas));
    let mut root_value = Value::Object(root_map);
    if let Some(syntax) = hash_get(arena, root, "syntax") {
        attach_offset(arena, &syntax, &mut root_value);
    }
    Ok(root_value)
}

fn schema_json(arena: &AstArena, decl: &AstNode, path: &str) -> Result<Value, ModelJsonError> {
    let id = rule_id_str(arena, hash_get(arena, decl, "schemaId").as_ref());

    let mut map = node(CLASS_SCHEMA);
    map.insert("id".into(), Value::String(id));
    map.insert("file".into(), Value::String(path.into()));

    if let Some(version) = hash_get(arena, decl, "schemaVersionId") {
        let mut v = version_json(arena, &version);
        attach_offset(arena, &version, &mut v);
        map.insert("version".into(), v);
    }

    // The Ruby builder always passes interfaces: [] (schema_decl_builder)
    // and Schema#interfaced_items calls interfaces.flat_map directly, so
    // the key must hydrate to a collection, never nil. Real interfaces
    // replace the empty default below.
    map.insert("interfaces".into(), Value::Array(Vec::new()));

    if let Some(body) = hash_get(arena, decl, "schemaBody").as_ref() {
        put_list(
            &mut map,
            "interfaces",
            declarations::interfaces_json(arena, body),
        );
        put_list(
            &mut map,
            "constants",
            declarations::constants_json(arena, body),
        );
        let mut decls = declarations::Declarations::default();
        declarations::schema_declarations(arena, body, &mut decls);
        declarations::apply(&mut map, &decls);
    }

    let mut v = Value::Object(map);
    attach_offset(arena, decl, &mut v);
    Ok(v)
}

/// schemaVersionId → SchemaVersion{value, items}: a `{...}` version
/// string is split into SchemaVersionItems exactly like
/// SchemaVersionBuilder (`name(value)`, bare numbers, bare names).
fn version_json(arena: &AstArena, v: &AstNode) -> Value {
    let value = hash_get(arena, v, "stringLiteral")
        .and_then(|l| hash_get(arena, &l, "simpleStringLiteral"))
        .and_then(|s| hash_get(arena, &s, "str"))
        .and_then(|n| text(arena, &n))
        .map(unquote)
        .unwrap_or_default();

    let mut map = node(CLASS_SCHEMA_VERSION);
    map.insert("value".into(), Value::String(value.clone()));

    if value.starts_with('{') && value.ends_with('}') {
        let items = value[1..value.len() - 1]
            .split_whitespace()
            .map(|part| {
                let mut item = node(CLASS_SCHEMA_VERSION_ITEM);
                match part
                    .rsplit_once('(')
                    .filter(|(_, tail)| tail.ends_with(')'))
                {
                    Some((name, tail)) => {
                        item.insert("name".into(), Value::String(name.to_string()));
                        item.insert(
                            "value".into(),
                            Value::String(tail.trim_end_matches(')').to_string()),
                        );
                    }
                    None if part.chars().all(|c| c.is_ascii_digit()) => {
                        item.insert("value".into(), Value::String(part.to_string()));
                    }
                    None => {
                        item.insert("name".into(), Value::String(part.to_string()));
                    }
                }
                Value::Object(item)
            })
            .collect::<Vec<_>>();
        if !items.is_empty() {
            map.insert("items".into(), Value::Array(items));
        }
    }

    Value::Object(map)
}

// ---------------------------------------------------------------------
// Shared emission helpers
// ---------------------------------------------------------------------

pub(crate) fn node(class: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("_class".into(), Value::String(class.into()));
    m
}

pub(crate) fn obj(m: Map<String, Value>) -> Value {
    Value::Object(m)
}

pub(crate) fn put(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(v) = value {
        map.insert(key.into(), v);
    }
}

pub(crate) fn put_list(map: &mut Map<String, Value>, key: &str, items: Vec<Value>) {
    if !items.is_empty() {
        map.insert(key.into(), Value::Array(items));
    }
}

/// Elements of a grammar repetition `key` under `host`.
///
/// Two shapes occur in the arena: `listOf_*` holder hashes carry the
/// repeated key alongside their tokens (single occurrence collapses to
/// the value, repetitions become arrays), while anonymous repetitions
/// (`rhs`, `stmt`, `declaration`, …) arrive as arrays of single-key
/// wrapper hashes. Both are normalized here.
pub(crate) fn children_of(arena: &AstArena, host: &AstNode, key: &str) -> Vec<AstNode> {
    let raw = match host {
        AstNode::Array { .. } => as_list(arena, host),
        // Holder form: the repeated key sits inside alongside tokens.
        _ => match hash_get(arena, host, key) {
            Some(v) => as_list(arena, &v),
            // Collapsed form: a single occurrence merged into the host
            // itself (e.g. constantBody, single listOf entries).
            None => vec![host.clone()],
        },
    };
    raw.into_iter()
        .map(|w| unwrap_child(arena, &w, key))
        .collect()
}

/// Unwrap a repetition element: {wrapper: inner} → inner; already-raw
/// nodes pass through.
pub(crate) fn unwrap_child(arena: &AstArena, w: &AstNode, wrapper: &str) -> AstNode {
    hash_get(arena, w, wrapper).unwrap_or_else(|| w.clone())
}

/// Mirror of Builder#attach_source_info: stamp the subtree's first
/// input-slice offset onto an emitted node (overwriting any inner
/// stamp, exactly like the Ruby outermost attach), plus the last
/// slice's end — the byte span the remark attacher's line ranges need.
pub(crate) fn attach_offset(arena: &AstArena, subtree: &AstNode, value: &mut Value) {
    let Value::Object(map) = value else { return };
    if let Some(offset) = crate::walk::first_slice_offset(arena, subtree, 0) {
        map.insert("source_offset".into(), Value::from(u64::from(offset)));
    }
    if let Some(end) = crate::walk::last_slice_end(arena, subtree, 0) {
        map.insert("source_end_offset".into(), Value::from(u64::from(end)));
    }
}

/// SimpleReference{id} — never emits base_path; the Ruby reference
/// resolver owns that attribute.
pub(crate) fn simple_ref(arena: &AstArena, host: &AstNode, id_keys: &[&str]) -> Option<Value> {
    let id = ref_id(arena, host, id_keys)?;
    let mut m = node(CLASS_SIMPLE_REFERENCE);
    m.insert("id".into(), Value::String(id));
    Some(obj(m))
}

/// extract_id_ref: text of the first present id key, else the first
/// hash value's nested text (helpers.rb fallback).
pub(crate) fn ref_id(arena: &AstArena, host: &AstNode, id_keys: &[&str]) -> Option<String> {
    for key in id_keys {
        if let Some(v) = hash_get(arena, host, key) {
            if let Some(t) = nested_text(arena, &v) {
                return Some(t);
            }
        }
    }
    hash_pairs(arena, host)
        .first()
        .and_then(|(_, v)| nested_text(arena, v))
}

/// The id keys every `*Ref` node is searched for, in helpers.rb order.
pub(crate) const REF_ID_KEYS: &[&str] = &[
    "functionId",
    "constantId",
    "parameterId",
    "variableId",
    "attributeId",
    "entityId",
    "typeId",
    "procedureId",
    "schemaId",
    "typeLabelId",
    "enumerationId",
    "renameId",
    "simpleId",
];
