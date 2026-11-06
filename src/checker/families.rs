//! The check families, in Ruby `check_schema` order, with messages
//! verbatim (consumer-parsed).

use super::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const DECL_COLLECTIONS: &[&str] = &[
    "types",
    "entities",
    "functions",
    "procedures",
    "constants",
    "rules",
    "subtype_constraints",
];

impl<'a> Ctx<'a> {
    pub(crate) fn check_schema(&mut self, schema: usize, dialect: Option<&str>) {
        self.check_duplicates(schema);
        self.check_interfaces(schema);
        self.check_type_resolutions(schema);
        self.check_self_schema_reference(schema);
        for e in self.entity_ptrs(schema) {
            self.check_entity(schema, &e);
        }
        let base = self.schema_ptr(schema).to_string();
        let tcount = arr(self.schema(schema), "types").len();
        for j in 0..tcount {
            let t = format!("{base}/types/{j}");
            self.check_type(schema, &t);
            self.check_select_items(schema, &t);
            let rules_ptr = format!("{t}/where_rules");
            self.check_where_rules(schema, &t, &rules_ptr);
        }
        let rcount = arr(self.schema(schema), "rules").len();
        for j in 0..rcount {
            let r = format!("{base}/rules/{j}");
            let rules_ptr = format!("{r}/where_rules");
            self.check_where_rules(schema, &r, &rules_ptr);
        }
        self.check_subtype_constraints(schema);
        for e in self.entity_ptrs(schema) {
            self.check_aggregate_bounds(schema, &e);
            self.check_attrib_name_fun(schema, &e);
            self.check_attribute_redeclarations(schema, &e);
        }
        self.check_string_references(schema);
        self.check_unresolved_refs(schema);
        self.check_expression_types(schema);
        self.check_statement_types(schema);
        self.check_rule_structure(schema);
        if dialect.is_some() {
            self.check_dialect(schema, dialect);
        }
    }

    // ---- duplicates ----

    fn check_duplicates(&mut self, schema: usize) {
        let mut seen: HashMap<String, String> = HashMap::new();
        let node = self.schema(schema);
        let _base = self.schema_ptr(schema).to_string();
        for coll in DECL_COLLECTIONS {
            for decl in arr(node, coll).iter() {
                let Some(id) = id_of(decl) else {
                    continue;
                };
                let key = safe_downcase(id);
                if let Some(first) = seen.get(&key) {
                    self.note(
                        "check_duplicate_declaration",
                        "error",
                        schema,
                        format!("duplicate declaration '{id}' (also declared in {first})"),
                    );
                } else {
                    seen.insert(key, coll.to_string());
                }
            }
        }
    }

    // ---- interfaces ----

    fn iface_schema_name(&self, iface: &Value) -> Option<String> {
        match fld(iface, "schema") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(v) if v.is_object() => id_of(v).map(str::to_string),
            _ => None,
        }
    }

    fn check_interfaces(&mut self, schema: usize) {
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        let count = arr(node, "interfaces").len();
        for i in 0..count {
            let iface_ptr = format!("{base}/interfaces/{i}");
            let iface = self.node(&iface_ptr);
            let Some(target) = self.iface_schema_name(iface) else {
                continue;
            };
            let kind = str_fld(iface, "kind").unwrap_or("USE");
            let a = self.resource_names(iface);
            for other_i in (i + 1)..count {
                let other_ptr = format!("{base}/interfaces/{other_i}");
                let other = self.node(&other_ptr);
                let Some(other_target) = self.iface_schema_name(other) else {
                    continue;
                };
                if safe_downcase(&other_target) != safe_downcase(&target) {
                    continue;
                }
                let other_kind = str_fld(other, "kind").unwrap_or("USE");
                if other_kind != kind {
                    continue;
                }
                let b = self.resource_names(other);
                let b_minus_a: Vec<&String> = b.iter().filter(|x| !a.contains(x)).collect();
                if a.is_empty() || b.is_empty() || b_minus_a.is_empty() {
                    self.note(
                        "check_schema_interface_redundant",
                        "warning",
                        schema,
                        format!("redundant {kind} interface to {target}"),
                    );
                } else {
                    let dup: Vec<String> = a.iter().filter(|x| b.contains(x)).cloned().collect();
                    if dup.is_empty() {
                        continue;
                    }
                    self.note(
                        "check_schema_iface_resource_duplicate",
                        "warning",
                        schema,
                        format!(
                            "duplicate resources {} on {kind} interface to {target}",
                            dup.join(", ")
                        ),
                    );
                }
            }

            let Some(foreign) = self.resolve_schema(&target) else {
                self.note(
                    "check_unresolved_ref",
                    "error",
                    schema,
                    format!("interface schema '{target}' not found"),
                );
                continue;
            };
            for name in self.resource_names(iface) {
                let Some(decl) = self.foreign_declaration(foreign, &name) else {
                    self.note(
                        "check_unresolved_ref",
                        "error",
                        schema,
                        format!("interface resource '{name}' not in schema '{target}'"),
                    );
                    continue;
                };
                let tail = class_tail(self.node(&decl));
                let kinds: &[&str] = if kind == "REFERENCE" {
                    REFERENCE_ITEM_KINDS
                } else {
                    USE_ITEM_KINDS
                };
                if kinds.contains(&tail) {
                    continue;
                }
                self.note(
                    "check_interface_kind_restriction",
                    "error",
                    schema,
                    format!(
                        "interface resource '{name}' is a {tail}: {kind} resolves {} only",
                        kinds.join(", ")
                    ),
                );
            }
        }
    }

    // ---- eeng check-p11 type resolutions ----

    fn check_type_resolutions(&mut self, schema: usize) {
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        for (j, constant) in arr(node, "constants").iter().enumerate() {
            self.check_type_ref(
                schema,
                &format!("{base}/constants/{j}/type"),
                "check_constant_type",
                id_of(constant).unwrap_or_default(),
                &format!("{base}/constants/{j}"),
            );
        }
        for (j, function) in arr(node, "functions").iter().enumerate() {
            let fptr = format!("{base}/functions/{j}");
            for (k, param) in arr(function, "parameters").iter().enumerate() {
                self.check_type_ref(
                    schema,
                    &format!("{fptr}/parameters/{k}/type"),
                    "check_parameter_type",
                    id_of(param).unwrap_or_default(),
                    &format!("{fptr}/parameters/{k}"),
                );
            }
            self.check_type_ref(
                schema,
                &format!("{fptr}/return_type"),
                "check_function_return_type",
                id_of(function).unwrap_or_default(),
                &fptr,
            );
            for (k, variable) in arr(function, "variables").iter().enumerate() {
                self.check_type_ref(
                    schema,
                    &format!("{fptr}/variables/{k}/type"),
                    "check_local_type",
                    id_of(variable).unwrap_or_default(),
                    &format!("{fptr}/variables/{k}"),
                );
            }
        }
        for (j, procedure) in arr(node, "procedures").iter().enumerate() {
            let pptr = format!("{base}/procedures/{j}");
            for (k, param) in arr(procedure, "parameters").iter().enumerate() {
                self.check_type_ref(
                    schema,
                    &format!("{pptr}/parameters/{k}/type"),
                    "check_parameter_type",
                    id_of(param).unwrap_or_default(),
                    &format!("{pptr}/parameters/{k}"),
                );
            }
            for (k, variable) in arr(procedure, "variables").iter().enumerate() {
                self.check_type_ref(
                    schema,
                    &format!("{pptr}/variables/{k}/type"),
                    "check_local_type",
                    id_of(variable).unwrap_or_default(),
                    &format!("{pptr}/variables/{k}"),
                );
            }
        }
        for (j, rule) in arr(node, "rules").iter().enumerate() {
            let rptr = format!("{base}/rules/{j}");
            for (k, _entity_ref) in arr(rule, "applies_to").iter().enumerate() {
                self.check_entity_ref(
                    schema,
                    &format!("{rptr}/applies_to/{k}"),
                    "check_parameter_type",
                    id_of(rule).unwrap_or_default(),
                    &rptr,
                );
            }
            for (k, variable) in arr(rule, "variables").iter().enumerate() {
                self.check_type_ref(
                    schema,
                    &format!("{rptr}/variables/{k}/type"),
                    "check_local_type",
                    id_of(variable).unwrap_or_default(),
                    &format!("{rptr}/variables/{k}"),
                );
            }
        }
    }

    fn check_type_ref(
        &mut self,
        schema: usize,
        type_ptr: &str,
        note_id: &str,
        name: &str,
        node_ptr: &str,
    ) {
        // Array(type): a singular type node wraps into a list.
        let type_node = self.node(type_ptr);
        let entries: Vec<String> = if type_node.is_object() {
            vec![type_ptr.to_string()]
        } else {
            Vec::new()
        };
        for entry in entries {
            let node = self.node(&entry);
            if !is(node, "References::SimpleReference") {
                continue;
            }
            let Some(id) = id_of(node) else {
                continue;
            };
            if builtin(id) {
                continue;
            }
            if self.resolves_to_type_or_entity(schema, id) {
                continue;
            }
            self.note(
                note_id,
                "error",
                schema,
                format!("type '{id}' of '{name}' does not resolve to a type or entity"),
            );
        }
        let _ = node_ptr;
    }

    fn check_entity_ref(
        &mut self,
        schema: usize,
        ref_ptr: &str,
        note_id: &str,
        name: &str,
        node_ptr: &str,
    ) {
        let ref_node = self.node(ref_ptr);
        let entries: Vec<String> = if ref_node.is_object() {
            vec![ref_ptr.to_string()]
        } else {
            Vec::new()
        };
        for entry in entries {
            let node = self.node(&entry);
            if !is(node, "References::SimpleReference") {
                continue;
            }
            let Some(id) = id_of(node) else {
                continue;
            };
            if self.resolves_to_type_or_entity(schema, id) {
                continue;
            }
            self.note(
                note_id,
                "error",
                schema,
                format!("entity '{id}' of '{name}' does not resolve to a type or entity"),
            );
        }
        let _ = node_ptr;
    }

    // ---- self-schema-qualified literals (#125) ----

    fn check_self_schema_reference(&mut self, schema: usize) {
        let schema_id = self.schemas[schema].1.clone();
        let prefix = format!("{}.", safe_downcase(&schema_id));
        let root = self.schema_ptr(schema).to_string();
        let mut literals = Vec::new();
        collect_strings(self.root, &root, &mut literals);
        for (_literal_ptr, value) in literals {
            let lower = safe_downcase(&value);
            // Ruby matches with /\A prefix\.(.+)\z/i — `.` never
            // crosses a newline, so a suffix spanning one rejects the
            // whole match.
            let Some(suffix_len) = lower.strip_prefix(&prefix).map(str::len) else {
                continue;
            };
            if lower[lower.len() - suffix_len..].contains('\n') {
                continue;
            }
            // Ruby keeps the literal's original case for the item.
            let raw_suffix = &value[value.len() - suffix_len..];
            let item = raw_suffix.trim().to_string();
            let declared_locally = self.schema_declares(schema, safe_downcase(&item).as_str());
            if declared_locally {
                self.note(
                    "check_self_schema_reference",
                    "warning",
                    schema,
                    format!(
                        "self-qualified '{schema_id}.{item}' — '{item}' is local; drop the prefix"
                    ),
                );
            } else {
                // Ruby defining_schema walks to a Repository ancestor;
                // an ExpFile root has none, so the defining schema is
                // nil and the literal is left untouched (:unknown).
                let mut source: Option<String> = None;
                if self.has_repository_ancestor(schema) {
                    for (k, (_, other_id)) in self.schemas.iter().enumerate() {
                        if k == schema {
                            continue;
                        }
                        if self.schema_declares(k, safe_downcase(&item).as_str()) {
                            source = Some(other_id.clone());
                            break;
                        }
                    }
                }
                if let Some(source_id) = source {
                    self.note(
                        "check_self_schema_reference",
                        "warning",
                        schema,
                        format!(
                            "self-qualified '{schema_id}.{item}' — item is defined in '{source_id}'"
                        ),
                    );
                }
            }
        }
    }

    // ---- entities ----

    fn check_entity(&mut self, schema: usize, entity_ptr: &str) {
        let entity = self.node(entity_ptr);
        let entity_id = id_of(entity).unwrap_or_default().to_string();
        for ref_ in arr(entity, "subtype_of").iter() {
            let id = Self::ref_id(ref_);
            let Some(id) = id else {
                continue;
            };
            if self.find_entity(schema, &id).is_some() {
                continue;
            }
            self.note(
                "check_subtype_ref",
                "error",
                schema,
                format!("ENTITY {entity_id}: subtype '{id}' not found"),
            );
            if self.find_type(schema, &id).is_some() {
                self.note(
                    "check_subtypeof_invalid",
                    "error",
                    schema,
                    format!("ENTITY {entity_id}: subtype '{id}' is a TYPE, not an entity"),
                );
            }
        }
        self.check_supertype_refs(schema, entity_ptr);
        self.check_subtype_cycles(schema, entity_ptr);
        let rules_ptr = format!("{entity_ptr}/where_rules");
        self.check_where_rules(schema, entity_ptr, &rules_ptr);
        self.check_inverse_attributes(schema, entity_ptr);
        self.check_group_references(schema, entity_ptr);
        for ur in arr(entity, "unique_rules").iter() {
            let Some(id) = id_of(ur) else {
                continue;
            };
            if unique_label_ok(id) {
                continue;
            }
            self.note(
                "check_unique_name_pattern",
                "warning",
                schema,
                format!("ENTITY {entity_id}: UNIQUE label '{id}' is not UR<n>"),
            );
        }
    }

    fn check_supertype_refs(&mut self, schema: usize, entity_ptr: &str) {
        let expr_ptr = format!("{entity_ptr}/supertype_expression");
        if self.node(&expr_ptr).is_null() {
            return;
        }
        let mut refs = Vec::new();
        collect_simple_refs(self.root, &expr_ptr, &mut refs);
        let entity_id = id_of(self.node(entity_ptr)).unwrap_or_default().to_string();
        for ref_ptr in refs {
            let Some(id) = id_of(self.node(&ref_ptr)) else {
                continue;
            };
            if self.find_entity(schema, id).is_some() {
                continue;
            }
            self.note(
                "check_supertype_ref",
                "error",
                schema,
                format!("ENTITY {entity_id}: supertype '{id}' not found"),
            );
        }
    }

    fn check_subtype_cycles(&mut self, schema: usize, entity_ptr: &str) {
        let mut clean: HashMap<String, bool> = HashMap::new();
        let start_id = id_of(self.node(entity_ptr))
            .map(safe_downcase)
            .unwrap_or_default();
        self.walk_subtype_edges(schema, entity_ptr, entity_ptr, &[start_id], &mut clean);
        let ptr = self.schema_ptr(schema).to_string();
        self.subtype_clean.insert(ptr, clean);
    }

    fn walk_subtype_edges(
        &mut self,
        schema: usize,
        start_ptr: &str,
        node_ptr: &str,
        path_keys: &[String],
        clean: &mut HashMap<String, bool>,
    ) -> bool {
        let mut cyclic = false;
        let node = self.node(node_ptr);
        let start_id = id_of(self.node(start_ptr)).unwrap_or_default().to_string();
        for ref_ in arr(node, "subtype_of").iter() {
            let Some(id) = Self::ref_id(ref_) else {
                continue;
            };
            let Some(nxt) = self.find_entity(schema, &id) else {
                continue;
            };
            let key = safe_downcase(&id);
            if path_keys.contains(&key) {
                self.note(
                    "check_subtype_cycle",
                    "error",
                    schema,
                    format!("ENTITY {start_id}: subtype inheritance cycle through '{id}'"),
                );
                cyclic = true;
            } else if clean.get(&key).copied().unwrap_or(false) {
                continue;
            } else {
                let mut next_path = path_keys.to_vec();
                next_path.push(key);
                cyclic =
                    self.walk_subtype_edges(schema, start_ptr, &nxt, &next_path, clean) || cyclic;
            }
        }
        if !cyclic {
            if let Some(node_id) = id_of(self.node(node_ptr)) {
                clean.insert(safe_downcase(node_id), true);
            }
        }
        cyclic
    }

    fn check_where_rules(&mut self, schema: usize, owner_ptr: &str, rules_ptr: &str) {
        let owner = self.node(owner_ptr);
        let owner_class = class_tail(owner).to_string();
        let owner_id = id_of(owner).unwrap_or_default().to_string();
        let Some(rules_val) = self.root.pointer(rules_ptr) else {
            return;
        };
        let Value::Array(items) = rules_val else {
            return;
        };
        for (j, _item) in items.iter().enumerate() {
            let rule_ptr = format!("{rules_ptr}/{j}");
            let rule = self.node(&rule_ptr);
            let Some(id) = id_of(rule) else {
                continue;
            };
            if where_label_ok(id) {
                continue;
            }
            self.note(
                "check_where_name_pattern",
                "warning",
                schema,
                format!("{owner_class} {owner_id}: WHERE label '{id}' is not WR<n>"),
            );
        }
    }

    fn check_inverse_attributes(&mut self, schema: usize, entity_ptr: &str) {
        // The Ruby parse path parks every attribute kind in
        // `attributes`; the core path fills inverse_attributes. Both
        // layouts hold the same items (Entity#inverse_attribute_items).
        let entity = self.node(entity_ptr);
        let own: Vec<String> = arr(entity, "inverse_attributes")
            .iter()
            .enumerate()
            .map(|(j, _)| format!("{entity_ptr}/inverse_attributes/{j}"))
            .collect();
        let ptrs = if own.is_empty() {
            arr(entity, "attributes")
                .iter()
                .enumerate()
                .filter(|(_, a)| cls(a).ends_with("Declarations::InverseAttribute"))
                .map(|(j, _)| format!("{entity_ptr}/attributes/{j}"))
                .collect()
        } else {
            own
        };
        for inv_ptr in ptrs {
            self.check_inverse_attribute(schema, entity_ptr, &inv_ptr);
        }
    }

    fn check_inverse_attribute(&mut self, schema: usize, entity_ptr: &str, inv_ptr: &str) {
        let inv = self.node(inv_ptr);
        let entity_id = id_of(self.node(entity_ptr)).unwrap_or_default().to_string();
        let inv_id = id_of(inv).unwrap_or_default().to_string();
        let target = self.inverse_target(schema, inv);
        if target.is_none() {
            let target_id = self.inverse_target_id(inv).unwrap_or_default();
            self.note(
                "check_inverse_entity",
                "error",
                schema,
                format!(
                    "ENTITY {entity_id}: INVERSE '{inv_id}' target '{target_id}' is not an entity"
                ),
            );
            return;
        }
        let target = target.unwrap();

        let Some(attr_id) = self.inverse_paired_attr_id(inv) else {
            self.note(
                "check_inverse_attrib_ref",
                "error",
                schema,
                format!("ENTITY {entity_id}: INVERSE '{inv_id}' names no attribute after FOR"),
            );
            return;
        };

        let owner = self.inverse_attr_owner(schema, inv_ptr, &target);
        let owner_for_probe = owner.clone().unwrap_or_else(|| target.clone());
        let paired = self.attribute_on_closure(schema, &owner_for_probe, &attr_id);
        let Some(paired_ptr) = paired else {
            let owner_label = owner
                .as_ref()
                .and_then(|o| id_of(self.node(o)))
                .map(str::to_string)
                .unwrap_or_else(|| id_of(self.node(&target)).unwrap_or_default().to_string());
            self.note(
                "check_inverse_attrib_ref",
                "error",
                schema,
                format!(
                    "ENTITY {entity_id}: INVERSE '{inv_id}': entity '{owner_label}' has no attribute '{attr_id}'"
                ),
            );
            return;
        };
        let paired_node = self.node(&paired_ptr);
        if is(paired_node, "Declarations::InverseAttribute") {
            let owner_label = owner
                .as_ref()
                .and_then(|o| id_of(self.node(o)))
                .unwrap_or_default();
            self.note(
                "check_inverse_inverse",
                "error",
                schema,
                format!(
                    "ENTITY {entity_id}: INVERSE '{inv_id}' pairs with '{owner_label}.{attr_id}', which is itself an INVERSE"
                ),
            );
        } else if str_fld(paired_node, "kind") == Some("DERIVED") {
            let owner_label = owner
                .as_ref()
                .and_then(|o| id_of(self.node(o)))
                .unwrap_or_default();
            self.note(
                "check_inverse_derive",
                "error",
                schema,
                format!(
                    "ENTITY {entity_id}: INVERSE '{inv_id}' pairs with '{owner_label}.{attr_id}', which is a DERIVE"
                ),
            );
        }
    }

    fn inverse_target(&self, schema: usize, inv: &Value) -> Option<String> {
        // Ruby unwraps when the type responds to base_type (the
        // aggregate kinds).
        let mut type_node = obj(inv, "type")?;
        if is_aggregate_type(type_node) {
            type_node = obj(type_node, "base_type")?;
        }
        let id = Self::ref_id(type_node)?;
        self.find_entity(schema, &id)
    }

    fn inverse_target_id(&self, inv: &Value) -> Option<String> {
        let mut type_node = obj(inv, "type")?;
        if is_aggregate_type(type_node) {
            type_node = obj(type_node, "base_type")?;
        }
        Self::ref_id(type_node)
    }

    fn inverse_attr_owner(&self, schema: usize, inv_ptr: &str, target: &str) -> Option<String> {
        let expr = obj(self.node(inv_ptr), "expression");
        if let Some(expr) = expr {
            if cls(expr).ends_with("References::AttributeReference") {
                if let Some(ref_node) = obj(expr, "ref") {
                    if let Some(id) = Self::ref_id(ref_node) {
                        return self
                            .find_entity(schema, &id)
                            .or_else(|| Some(target.to_string()));
                    }
                }
            }
        }
        Some(target.to_string())
    }

    fn inverse_paired_attr_id(&self, inv: &Value) -> Option<String> {
        let expr = obj(inv, "expression")?;
        if let Some(id) = id_of(expr) {
            return Some(id.to_string());
        }
        obj(expr, "attribute").and_then(Self::ref_id)
    }

    fn attribute_on_closure(
        &self,
        schema: usize,
        entity_ptr: &str,
        attr_id: &str,
    ) -> Option<String> {
        let key = safe_downcase(attr_id);
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(entity_ptr.to_string());
        while let Some(current) = queue.pop_front() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let node = self.node(&current);
            for (j, a) in arr(node, "attributes").iter().enumerate() {
                if id_of(a).map(|x| safe_downcase(x) == key).unwrap_or(false) {
                    return Some(format!("{current}/attributes/{j}"));
                }
            }
            for ref_ in arr(node, "subtype_of").iter() {
                let Some(id) = Self::ref_id(ref_) else {
                    continue;
                };
                if let Some(t) = self.find_entity(schema, &id) {
                    queue.push_back(t);
                }
            }
        }
        None
    }

    fn check_group_references(&mut self, schema: usize, entity_ptr: &str) {
        let entity_id = id_of(self.node(entity_ptr)).unwrap_or_default().to_string();
        // supertype_closure_ids: BFS collecting the raw names.
        let mut closure: HashSet<String> = HashSet::new();
        {
            let entity = self.node(entity_ptr);
            if let Some(id) = id_of(entity) {
                closure.insert(id.to_string());
            }
            let mut queue = std::collections::VecDeque::new();
            for ref_ in arr(entity, "subtype_of").iter() {
                if let Some(id) = Self::ref_id(ref_) {
                    queue.push_back(id);
                }
            }
            let mut seen: HashSet<String> = HashSet::new();
            while let Some(name) = queue.pop_front() {
                if !seen.insert(name.clone()) {
                    continue;
                }
                closure.insert(name.clone());
                if let Some(parent) = self.find_entity(schema, &name) {
                    for ref_ in arr(self.node(&parent), "subtype_of").iter() {
                        if let Some(id) = Self::ref_id(ref_) {
                            queue.push_back(id);
                        }
                    }
                }
            }
        }
        let mut redeclared: HashSet<String> = HashSet::new();
        {
            let entity = self.node(entity_ptr);
            for (j, attr) in arr(entity, "attributes").iter().enumerate() {
                let Some(sa) = obj(attr, "supertype_attribute") else {
                    continue;
                };
                let Some(group) = obj(sa, "ref") else {
                    continue;
                };
                if !is(group, "References::GroupReference") {
                    continue;
                }
                let group_ptr = format!("{entity_ptr}/attributes/{j}/supertype_attribute/ref");
                redeclared.insert(group_ptr.clone());
                let target = self.resolve_group(schema, entity_ptr, group, &group_ptr);
                let group_entity_id = obj(group, "entity")
                    .and_then(Self::ref_id)
                    .map(|x| safe_downcase(&x));
                if target.is_none() {
                    continue;
                }
                match group_entity_id {
                    Some(ge) if closure.contains(&ge) => continue,
                    _ => {}
                }
                let ge = group_entity_id.unwrap_or_default();
                self.note(
                    "qualified_attrib_group_not_ancestor",
                    "error",
                    schema,
                    format!("ENTITY {entity_id}: SELF\\'{ge}' is not a supertype of {entity_id}"),
                );
            }
        }
        for ptr in each_node(self.root, entity_ptr) {
            let node = self.node(&ptr);
            if !is(node, "References::GroupReference") {
                continue;
            }
            if redeclared.contains(&ptr) {
                continue;
            }
            let Some(target) = self.resolve_group(schema, entity_ptr, node, &ptr) else {
                continue;
            };
            let Some(parent_ptr) = self.parent.get(&ptr).cloned() else {
                continue;
            };
            let parent = self.node(&parent_ptr);
            let attr_id = obj(parent, "attribute").and_then(Self::ref_id);
            let Some(attr_id) = attr_id else {
                continue;
            };
            let key = safe_downcase(&attr_id);
            let target_node = self.node(&target);
            if arr(target_node, "attributes")
                .iter()
                .any(|a| id_of(a).map(|x| safe_downcase(x) == key).unwrap_or(false))
            {
                continue;
            }
            let group_entity = obj(node, "entity")
                .and_then(Self::ref_id)
                .unwrap_or_default();
            self.note(
                "qualified_attrib_attr_not_found",
                "error",
                schema,
                format!(
                    "ENTITY {entity_id}: '{group_entity}.{attr_id}': '{group_entity}' has no attribute '{attr_id}'"
                ),
            );
        }
    }

    fn resolve_group(
        &mut self,
        schema: usize,
        entity_ptr: &str,
        node: &Value,
        node_ptr: &str,
    ) -> Option<String> {
        let group_id = obj(node, "entity").and_then(Self::ref_id);
        if let Some(group_id) = &group_id {
            if let Some(target) = self.find_entity(schema, group_id) {
                return Some(target);
            }
        }
        let entity_id = id_of(self.node(entity_ptr)).unwrap_or_default().to_string();
        let group_label = group_id.unwrap_or_default();
        let _ = node_ptr;
        self.note(
            "qualified_attrib_group_not_found",
            "error",
            schema,
            format!(
                "ENTITY {entity_id}: group '{group_label}' in a qualified attribute does not resolve to an entity"
            ),
        );
        None
    }

    // ---- types ----

    fn check_type(&mut self, schema: usize, type_ptr: &str) {
        let tnode = self.node(type_ptr);
        let type_id = id_of(tnode).unwrap_or_default().to_string();
        let Some(ut) = obj(tnode, "underlying_type") else {
            return;
        };
        let ut_class = cls(ut).to_string();
        if (ut_class.ends_with("DataTypes::Select") || ut_class.ends_with("DataTypes::Enumeration"))
            && obj(ut, "based_on").is_some()
        {
            let based_on = fld(ut, "based_on").unwrap_or(&Value::Null);
            if !self.resolve_type(schema, based_on) {
                let id = Self::ref_id(based_on).unwrap_or_default();
                let note_id = if ut_class.ends_with("DataTypes::Select") {
                    "check_select_extended_type"
                } else {
                    "check_enumeration_extended_type"
                };
                self.note(
                    note_id,
                    "error",
                    schema,
                    format!("TYPE {type_id}: BASED_ON '{id}' not found"),
                );
            }
        }
    }

    fn check_select_items(&mut self, schema: usize, type_ptr: &str) {
        let tnode = self.node(type_ptr);
        let Some(ut) = obj(tnode, "underlying_type") else {
            return;
        };
        if !cls(ut).ends_with("DataTypes::Select") {
            return;
        }
        let type_id = id_of(tnode).unwrap_or_default().to_string();
        for (j, item) in arr(ut, "items").iter().enumerate() {
            let Some(id) = id_of(item) else {
                continue;
            };
            if self.select_item_resolves(schema, id) {
                continue;
            }
            let _ = j;
            self.note(
                "check_select_named_type",
                "error",
                schema,
                format!(
                    "In the SELECT TYPE {type_id}, '{id}' does not name an existing TYPE or ENTITY"
                ),
            );
        }
    }

    fn select_item_resolves(&self, schema: usize, id: &str) -> bool {
        if self.find_entity(schema, id).is_some() {
            return true;
        }
        let key = safe_downcase(id);
        if arr(self.schema(schema), "types")
            .iter()
            .any(|t| id_of(t).map(|x| safe_downcase(x) == key).unwrap_or(false))
        {
            return true;
        }
        for group in self.by_name.values() {
            for &other in group {
                let onode = self.schema(other);
                if arr(onode, "types")
                    .iter()
                    .any(|t| id_of(t).map(|x| safe_downcase(x) == key).unwrap_or(false))
                {
                    return true;
                }
                if arr(onode, "entities")
                    .iter()
                    .any(|e| id_of(e).map(|x| safe_downcase(x) == key).unwrap_or(false))
                {
                    return true;
                }
            }
        }
        false
    }

    // ---- subtype constraints ----

    fn check_subtype_constraints(&mut self, schema: usize) {
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        let count = arr(node, "subtype_constraints").len();
        for j in 0..count {
            let sc_ptr = format!("{base}/subtype_constraints/{j}");
            let sc = self.node(&sc_ptr);
            let expr_ptr = format!("{sc_ptr}/supertype_expression");
            if self.node(&expr_ptr).is_null() {
                continue;
            }
            let sc_id = id_of(sc).unwrap_or_default().to_string();
            let mut refs = Vec::new();
            collect_simple_refs(self.root, &expr_ptr, &mut refs);
            for ref_ptr in refs {
                let Some(id) = id_of(self.node(&ref_ptr)) else {
                    continue;
                };
                if !self.resolves_to_type_or_entity(schema, id) {
                    continue;
                }
                if self.find_entity(schema, id).is_some() {
                    continue;
                }
                self.note(
                    "check_supertype_ref",
                    "error",
                    schema,
                    format!("SUBTYPE_CONSTRAINT {sc_id}: supertype '{id}' is not an entity"),
                );
            }
        }
    }

    // ---- aggregate bounds / attrib-name-fun / redeclarations ----

    fn check_aggregate_bounds(&mut self, schema: usize, entity_ptr: &str) {
        let entity = self.node(entity_ptr);
        let entity_id = id_of(entity).unwrap_or_default().to_string();
        for attr in arr(entity, "attributes").iter() {
            let Some(attr_type) = obj(attr, "type") else {
                continue;
            };
            let class = cls(attr_type);
            if !(class.contains("DataTypes::List")
                || class.contains("DataTypes::Set")
                || class.contains("DataTypes::Bag")
                || class.contains("DataTypes::Array"))
            {
                continue;
            }
            let lower = integer_bound(fld(attr_type, "bound1"));
            let upper = integer_bound(fld(attr_type, "bound2"));
            let (Some(lower), Some(upper)) = (lower, upper) else {
                continue;
            };
            if lower <= upper {
                continue;
            }
            let attr_id = id_of(attr).unwrap_or_default();
            self.note(
                "check_agg_type",
                "error",
                schema,
                format!(
                    "ENTITY {entity_id}: aggregate '{attr_id}' bounds [{lower}:{upper}] are inverted"
                ),
            );
        }
    }

    fn check_attrib_name_fun(&mut self, schema: usize, entity_ptr: &str) {
        let funs: Vec<String> = arr(self.schema(schema), "functions")
            .iter()
            .filter_map(id_of)
            .map(safe_downcase)
            .collect();
        let entity = self.node(entity_ptr);
        let entity_id = id_of(entity).unwrap_or_default().to_string();
        for attr in arr(entity, "attributes").iter() {
            let Some(id) = id_of(attr) else {
                continue;
            };
            if !funs.contains(&safe_downcase(id)) {
                continue;
            }
            self.note(
                "check_attrib_name_fun",
                "warning",
                schema,
                format!("ENTITY {entity_id}: attribute '{id}' shadows a function of the same name"),
            );
        }
    }

    fn check_attribute_redeclarations(&mut self, schema: usize, entity_ptr: &str) {
        let entity = self.node(entity_ptr);
        let entity_id = id_of(entity).unwrap_or_default().to_string();
        for (j, attr) in arr(entity, "attributes").iter().enumerate() {
            if cls(attr) != "Expressir::Model::Declarations::Attribute" {
                continue;
            }
            let Some(tag) = obj(attr, "supertype_attribute") else {
                continue;
            };
            if !is(tag, "References::AttributeReference") {
                continue;
            }
            let _attr_ptr = format!("{entity_ptr}/attributes/{j}");
            let attr_id = obj(tag, "attribute").and_then(id_of).map(str::to_string);
            let sup_id = obj(tag, "ref")
                .and_then(|r| obj(r, "entity"))
                .and_then(id_of)
                .map(str::to_string);
            let Some(sup_id) = sup_id else {
                self.note(
                    "check_attribute_redeclaration",
                    "error",
                    schema,
                    format!(
                        "ENTITY {entity_id}: redeclaration of '{}' names no supertype",
                        id_of(attr).unwrap_or_default()
                    ),
                );
                continue;
            };
            let closure = self.supertype_closure(entity_ptr);
            let supertype = closure.iter().find(|st| {
                id_of(self.node(st))
                    .map(|x| safe_downcase(x) == safe_downcase(&sup_id))
                    .unwrap_or(false)
            });
            let Some(supertype) = supertype else {
                if self.find_entity(schema, &sup_id).is_some() {
                    self.note(
                        "check_attribute_redeclaration",
                        "error",
                        schema,
                        format!(
                            "ENTITY {entity_id}: redeclaration '{}' names '{sup_id}', which is not a supertype",
                            id_of(attr).unwrap_or_default()
                        ),
                    );
                }
                continue;
            };
            let attr_key = attr_id.as_deref().map(safe_downcase);
            let st_node = self.node(supertype);
            let has_attr = arr(st_node, "attributes").iter().any(|candidate| {
                cls(candidate) == "Expressir::Model::Declarations::Attribute"
                    && id_of(candidate)
                        .map(|x| Some(safe_downcase(x)) == attr_key)
                        .unwrap_or(false)
            });
            if has_attr {
                continue;
            }
            self.note(
                "check_attribute_redeclaration",
                "error",
                schema,
                format!(
                    "ENTITY {entity_id}: redeclaration '{}': supertype '{sup_id}' has no attribute '{}'",
                    id_of(attr).unwrap_or_default(),
                    attr_id.unwrap_or_default()
                ),
            );
        }
    }

    // ---- string references + unresolved refs ----

    fn check_string_references(&mut self, schema: usize) {
        let root = self.schema_ptr(schema).to_string();
        let mut literals = Vec::new();
        collect_strings(self.root, &root, &mut literals);
        for (literal_ptr, value) in literals {
            let Some((schema_id, item_id)) = split_schema_item(&value) else {
                continue;
            };
            let folded_schema = safe_downcase(&schema_id);
            let _folded_item = safe_downcase(&item_id);
            let Some(&target) = self.by_name.get(folded_schema.as_str()).and_then(|g| {
                g.iter()
                    .find(|&&i| safe_downcase(&self.schemas[i].1) == folded_schema)
            }) else {
                continue;
            };
            let folded_item = safe_downcase(&item_id);
            if self.schema_declares(target, &folded_item) {
                continue;
            }
            let target_id = self.schemas[target].1.clone();
            let _ = literal_ptr;
            self.note(
                "check_string_no_entity",
                "warning",
                schema,
                format!(
                    "string '{value}' references '{folded_item}' which is not declared in schema '{target_id}'"
                ),
            );
        }
    }

    fn check_unresolved_refs(&mut self, schema: usize) {
        let root = self.schema_ptr(schema).to_string();
        for ptr in each_node(self.root, &root) {
            let node = self.node(&ptr);
            if !is(node, "References::SimpleReference") {
                continue;
            }
            if node.get("base_path").is_some() {
                continue;
            }
            let Some(parent_ptr) = self.parent.get(&ptr).cloned() else {
                continue;
            };
            let parent = self.node(&parent_ptr);
            if is(parent, "Declarations::InterfaceItem")
                || is(parent, "References::AttributeReference")
            {
                continue;
            }
            let Some(id) = id_of(node) else {
                continue;
            };
            if builtin(id) {
                continue;
            }
            let key = safe_downcase(id);
            if self.alias_map(schema).contains_key(&key) {
                continue;
            }
            if self.bound_identifiers(schema).contains(&key) {
                continue;
            }
            // Skip self-ids of declarations.
            if id_of(parent)
                .map(|x| safe_downcase(x) == key)
                .unwrap_or(false)
            {
                continue;
            }
            self.note(
                "check_unresolved_ref",
                "error",
                schema,
                format!("unresolved reference '{id}'"),
            );
        }
    }

    // ---- expression types ----

    fn expression_env(&self, schema: usize, scope_ptr: Option<&str>) -> HashMap<String, String> {
        let mut env = HashMap::new();
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        for (j, constant) in arr(node, "constants").iter().enumerate() {
            if let Some(id) = id_of(constant) {
                env.insert(safe_downcase(id), format!("{base}/constants/{j}/type"));
            }
        }
        let Some(scope_ptr) = scope_ptr else {
            return env;
        };
        let scope = self.node(scope_ptr);
        let tail = class_tail(scope);
        if tail == "Entity" {
            for (j, attr) in arr(scope, "attributes").iter().enumerate() {
                if let Some(id) = id_of(attr) {
                    env.insert(
                        safe_downcase(id),
                        format!("{scope_ptr}/attributes/{j}/type"),
                    );
                }
            }
        } else if tail == "Function" || tail == "Procedure" {
            for (j, param) in arr(scope, "parameters").iter().enumerate() {
                if let Some(id) = id_of(param) {
                    env.insert(
                        safe_downcase(id),
                        format!("{scope_ptr}/parameters/{j}/type"),
                    );
                }
            }
            for (j, variable) in arr(scope, "variables").iter().enumerate() {
                if let Some(id) = id_of(variable) {
                    env.insert(safe_downcase(id), format!("{scope_ptr}/variables/{j}/type"));
                }
            }
        } else if tail == "Rule" {
            for (j, variable) in arr(scope, "variables").iter().enumerate() {
                if let Some(id) = id_of(variable) {
                    env.insert(safe_downcase(id), format!("{scope_ptr}/variables/{j}/type"));
                }
            }
        }
        env
    }

    fn check_expression_types(&mut self, schema: usize) {
        self.check_initializer_types(schema);
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        let mut scopes = Vec::new();
        for attr in [
            "constants",
            "types",
            "entities",
            "functions",
            "procedures",
            "rules",
        ] {
            for (j, _item) in arr(node, attr).iter().enumerate() {
                scopes.push(format!("{base}/{attr}/{j}"));
            }
        }
        for scope in scopes {
            let env = self.expression_env(schema, Some(&scope));
            self.env_stack.push(env);
            for ptr in each_node(self.root, &scope) {
                let node = self.node(&ptr);
                if is(node, "Expressions::BinaryExpression") {
                    self.check_binary_expression_types(schema, &ptr);
                } else if is(node, "Expressions::UnaryExpression") {
                    self.check_unary_expression_types(schema, &ptr);
                } else if is(node, "Expressions::Interval") {
                    self.check_interval_types(schema, &ptr);
                } else if is(node, "References::IndexReference") {
                    self.check_index_reference_types(schema, &ptr);
                }
            }
            self.env_stack.pop();
        }
    }

    fn check_binary_expression_types(&mut self, schema: usize, ptr: &str) {
        let left = self.static_type(schema, &format!("{ptr}/operand1"), 0);
        let right = self.static_type(schema, &format!("{ptr}/operand2"), 0);
        if undecidable_2(left, right) {
            return;
        }
        let node = self.node(ptr);
        let operator = str_fld(node, "operator").unwrap_or("").to_string();
        let Some(domain) = binary_domain_violation(&operator, left, right) else {
            return;
        };
        self.note(
            "check_expr_operand_domain",
            "error",
            schema,
            format!(
                "operator '{}': {domain} ({left} vs {right})",
                binary_operator_spelling(&operator)
            ),
        );
    }

    fn check_unary_expression_types(&mut self, schema: usize, ptr: &str) {
        let t = self.static_type(schema, &format!("{ptr}/operand"), 0);
        if undecidable_1(t) {
            return;
        }
        let node = self.node(ptr);
        let operator = str_fld(node, "operator").unwrap_or("").to_string();
        let violation = if operator == "NOT" {
            if is_logical(t) {
                None
            } else {
                Some("not requires a logical operand (12.4)".to_string())
            }
        } else if is_numeric(t) {
            None
        } else {
            Some(format!(
                "unary '{}' requires a numeric operand (12.1)",
                operator.to_lowercase()
            ))
        };
        if let Some(violation) = violation {
            self.note("check_expr_operand_domain", "error", schema, violation);
        }
    }

    fn check_interval_types(&mut self, schema: usize, ptr: &str) {
        let low = self.static_type(schema, &format!("{ptr}/low"), 0);
        let item = self.static_type(schema, &format!("{ptr}/item"), 0);
        let high = self.static_type(schema, &format!("{ptr}/high"), 0);
        let types = [low, item, high];
        if types.iter().any(|t| undecidable_1(t)) {
            return;
        }
        let all_same_family = types.iter().all(|t| same_ordering_family(types[0], t));
        let all_numeric = types.iter().all(|t| is_numeric(t));
        if all_same_family || all_numeric {
            return;
        }
        self.note(
            "check_expr_operand_domain",
            "error",
            schema,
            format!(
                "interval bounds must be mutually comparable (12.2.4) ({} vs {} vs {})",
                types[0], types[1], types[2]
            ),
        );
    }

    fn check_index_reference_types(&mut self, schema: usize, ptr: &str) {
        let base = self.static_type(schema, &format!("{ptr}/ref"), 0);
        if !(undecidable_1(base)
            || base == "aggregate"
            || base == "string"
            || base == "binary"
            || base == "select")
        {
            self.note(
                "check_expr_operand_domain",
                "error",
                schema,
                format!(
                    "indexing requires an aggregate, string or binary operand (12.6.1, 12.3.1, 12.5.1) ({base})"
                ),
            );
        }
        for field in ["index1", "index2"] {
            let position_ptr = format!("{ptr}/{field}");
            if self.node(&position_ptr).is_null() {
                continue;
            }
            let position_type = self.static_type(schema, &position_ptr, 0);
            if undecidable_1(position_type) || is_numeric(position_type) {
                continue;
            }
            self.note(
                "check_expr_operand_domain",
                "error",
                schema,
                format!("index position shall be numeric (12.6.1) ({position_type})"),
            );
        }
    }

    // ---- initializers ----

    fn initializers(&self, schema: usize) -> Vec<(Option<String>, String)> {
        let mut pairs = Vec::new();
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        for (j, constant) in arr(node, "constants").iter().enumerate() {
            if obj(constant, "expression").is_some() {
                pairs.push((None, format!("{base}/constants/{j}")));
            }
        }
        for (j, entity) in arr(node, "entities").iter().enumerate() {
            let eptr = format!("{base}/entities/{j}");
            for (k, attr) in arr(entity, "attributes").iter().enumerate() {
                // Ruby: attr.is_a?(Attribute) && !attr.is_a?(InverseAttribute)
                // — DerivedAttribute subclasses Attribute.
                if !cls(attr).ends_with("Declarations::Attribute")
                    || cls(attr).ends_with("Declarations::InverseAttribute")
                {
                    continue;
                }
                if obj(attr, "expression").is_none() {
                    continue;
                }
                pairs.push((Some(eptr.clone()), format!("{eptr}/attributes/{k}")));
            }
        }
        for (j, function) in arr(node, "functions").iter().enumerate() {
            let fptr = format!("{base}/functions/{j}");
            for (k, variable) in arr(function, "variables").iter().enumerate() {
                if obj(variable, "expression").is_some() {
                    pairs.push((Some(fptr.clone()), format!("{fptr}/variables/{k}")));
                }
            }
        }
        pairs
    }

    fn check_initializer_types(&mut self, schema: usize) {
        let initializers = self.initializers(schema);
        for (owner, decl_ptr) in initializers {
            let env = self.expression_env(schema, owner.as_deref());
            self.env_stack.push(env);
            let declared = self.static_type(schema, &format!("{decl_ptr}/type"), 0);
            let has_expression = obj(self.node(&decl_ptr), "expression").is_some();
            if !has_expression || undecidable_1(declared) {
                self.env_stack.pop();
                continue;
            }
            let expressed = self.static_type(schema, &format!("{decl_ptr}/expression"), 0);
            let compatible = if undecidable_1(expressed) {
                true
            } else {
                self.initializer_compatible(
                    schema,
                    &format!("{decl_ptr}/type"),
                    declared,
                    &format!("{decl_ptr}/expression"),
                    expressed,
                )
            };
            if !compatible {
                self.note(
                    "check_expr_initializer_type",
                    "error",
                    schema,
                    format!(
                        "initializer {expressed} is not compatible with the declared {declared} (12.11)"
                    ),
                );
            }
            self.env_stack.pop();
        }
    }

    pub(crate) fn initializer_compatible(
        &self,
        schema: usize,
        declared_ptr: &str,
        declared: &str,
        expression_ptr: &str,
        expressed: &str,
    ) -> bool {
        if declared == expressed && declared != "enum" && declared != "entity" {
            return true;
        }
        if declared == "number" && is_numeric(expressed) {
            return true;
        }
        if declared == "real" && expressed == "integer" {
            return true;
        }
        if is_logical(declared) && is_logical(expressed) {
            return true;
        }
        if declared == "select" || expressed == "select" {
            return true;
        }
        if declared == "aggregate" && expressed == "aggregate" {
            return true;
        }
        if declared == "enum" && expressed == "enum" {
            return self.same_enum_declaration(schema, declared_ptr, expression_ptr);
        }
        if declared == "entity" && expressed == "entity" {
            return self.entities_compatible(schema, declared_ptr, expression_ptr);
        }
        false
    }

    /// Ruby `resolved_declaration`: the Type or Entity declaration a
    /// type reference chain names.
    pub(crate) fn resolved_declaration(
        &self,
        schema: usize,
        ptr: &str,
        depth: usize,
    ) -> Option<String> {
        if ptr.is_empty() || depth > 6 || self.node(ptr).is_null() {
            return None;
        }
        let node = self.node(ptr);
        let class = cls(node);
        if class.ends_with("Declarations::Type") || class.ends_with("Declarations::Entity") {
            return Some(ptr.to_string());
        }
        if class.ends_with("References::SimpleReference") {
            let key = id_of(node).map(safe_downcase)?;
            let bound = self.env().and_then(|env| env.get(&key).cloned());
            let candidate = match bound {
                Some(b) => Some(b),
                None => self
                    .find_type(schema, id_of(node).unwrap_or_default())
                    .map(|t| format!("{t}/underlying_type")),
            };
            let candidate = candidate.or_else(|| self.find_entity(schema, id_of(node)?));
            return self.resolved_declaration(schema, &candidate?, depth + 1);
        }
        if class.ends_with("References::AttributeReference") {
            let owner = self.resolved_declaration(schema, &format!("{ptr}/ref"), depth + 1)?;
            let owner_node = self.node(&owner);
            if cls(owner_node).ends_with("Declarations::Type")
                && obj(owner_node, "underlying_type")
                    .map(|ut| cls(ut).ends_with("DataTypes::Enumeration"))
                    .unwrap_or(false)
            {
                return Some(owner);
            }
            if !cls(owner_node).ends_with("Declarations::Entity") {
                return None;
            }
            let attr_id = obj(node, "attribute").and_then(id_of).map(safe_downcase)?;
            for (j, candidate) in arr(owner_node, "attributes").iter().enumerate() {
                if id_of(candidate)
                    .map(|x| safe_downcase(x) == attr_id)
                    .unwrap_or(false)
                {
                    let type_ptr = format!("{owner}/attributes/{j}/type");
                    if self.node(&type_ptr).is_null() {
                        return None;
                    }
                    return self.resolved_declaration(schema, &type_ptr, depth + 1);
                }
            }
            return None;
        }
        if class.ends_with("References::IndexReference") {
            let element =
                self.index_element_type_node_ptr(schema, &format!("{ptr}/ref"), depth + 1)?;
            return self.resolved_declaration(schema, &element, depth + 1);
        }
        if class.ends_with("References::GroupReference") {
            return self.resolved_declaration(schema, &format!("{ptr}/entity"), depth + 1);
        }
        if class.ends_with("Expressions::FunctionCall") {
            let function = self.schema_function_ptr(schema, ptr)?;
            if self.node(&format!("{function}/return_type")).is_null() {
                return None;
            }
            return self.resolved_declaration(
                schema,
                &format!("{function}/return_type"),
                depth + 1,
            );
        }
        None
    }

    fn same_enum_declaration(
        &self,
        schema: usize,
        declared_ptr: &str,
        expression_ptr: &str,
    ) -> bool {
        let slot = self.resolved_declaration(schema, declared_ptr, 0);
        let expr = self.resolved_declaration(schema, expression_ptr, 0);
        let (Some(slot), Some(expr)) = (slot, expr) else {
            return false;
        };
        let slot_node = self.node(&slot);
        let expr_node = self.node(&expr);
        cls(slot_node).ends_with("Declarations::Type")
            && cls(expr_node).ends_with("Declarations::Type")
            && obj(slot_node, "underlying_type")
                .map(|ut| cls(ut).ends_with("DataTypes::Enumeration"))
                .unwrap_or(false)
            && obj(expr_node, "underlying_type")
                .map(|ut| cls(ut).ends_with("DataTypes::Enumeration"))
                .unwrap_or(false)
            && id_of(slot_node).map(safe_downcase) == id_of(expr_node).map(safe_downcase)
    }

    fn entities_compatible(&self, schema: usize, declared_ptr: &str, expression_ptr: &str) -> bool {
        let slot = self.resolved_declaration(schema, declared_ptr, 0);
        let expr = self.resolved_declaration(schema, expression_ptr, 0);
        let (Some(slot), Some(expr)) = (slot, expr) else {
            return false;
        };
        let slot_node = self.node(&slot);
        let expr_node = self.node(&expr);
        if !cls(slot_node).ends_with("Declarations::Entity")
            || !cls(expr_node).ends_with("Declarations::Entity")
        {
            return false;
        }
        let slot_id = id_of(slot_node).map(safe_downcase);
        let expr_id = id_of(expr_node).map(safe_downcase);
        if slot_id == expr_id {
            return true;
        }
        let closure = self.supertype_closure(&expr);
        closure
            .iter()
            .any(|e| id_of(self.node(e)).map(safe_downcase) == slot_id)
    }

    // ---- statement types ----

    fn check_statement_types(&mut self, schema: usize) {
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        let mut scopes = Vec::new();
        for attr in ["functions", "procedures", "rules"] {
            for (j, _item) in arr(node, attr).iter().enumerate() {
                scopes.push(format!("{base}/{attr}/{j}"));
            }
        }
        for scope in scopes {
            let env = self.expression_env(schema, Some(&scope));
            self.env_stack.push(env);
            for ptr in each_node(self.root, &scope) {
                let node = self.node(&ptr);
                if is(node, "Statements::Assignment") {
                    self.check_assignment_type(schema, &ptr);
                } else if is(node, "Statements::If") {
                    let expr_ptr = format!("{ptr}/expression");
                    if !self.node(&expr_ptr).is_null() {
                        self.check_logical_expression(
                            schema,
                            &expr_ptr,
                            "if condition (13.5)",
                            &ptr,
                        );
                    }
                } else if is(node, "Statements::Repeat") {
                    self.check_repeat_types(schema, &ptr);
                } else if is(node, "Statements::Return") {
                    self.check_return_type(schema, &scope, &ptr);
                }
            }
            self.env_stack.pop();
        }
    }

    fn check_assignment_type(&mut self, schema: usize, ptr: &str) {
        let declared = self.static_type(schema, &format!("{ptr}/ref"), 0);
        let expressed = self.static_type(schema, &format!("{ptr}/expression"), 0);
        if undecidable_2(declared, expressed) {
            return;
        }
        if self.initializer_compatible(
            schema,
            &format!("{ptr}/ref"),
            declared,
            &format!("{ptr}/expression"),
            expressed,
        ) {
            return;
        }
        self.note(
            "check_expr_assignment_type",
            "error",
            schema,
            format!(
                "assignment of {expressed} to a {declared} variable is not compatible (13.2.1, 12.11)"
            ),
        );
    }

    fn check_logical_expression(
        &mut self,
        schema: usize,
        expression_ptr: &str,
        label: &str,
        node_ptr: &str,
    ) {
        let t = self.static_type(schema, expression_ptr, 0);
        if undecidable_1(t) || is_logical(t) {
            return;
        }
        let _ = node_ptr;
        self.note(
            "check_expr_operand_domain",
            "error",
            schema,
            format!("{label} shall be a logical expression"),
        );
    }

    fn check_repeat_types(&mut self, schema: usize, ptr: &str) {
        for field in ["while_expression", "until_expression"] {
            let condition_ptr = format!("{ptr}/{field}");
            if self.node(&condition_ptr).is_null() {
                continue;
            }
            self.check_logical_expression(
                schema,
                &condition_ptr,
                "repeat control condition (13.6)",
                ptr,
            );
        }
        for field in ["bound1", "bound2", "increment"] {
            let bound_ptr = format!("{ptr}/{field}");
            if self.node(&bound_ptr).is_null() {
                continue;
            }
            let t = self.static_type(schema, &bound_ptr, 0);
            if undecidable_1(t) || is_numeric(t) {
                continue;
            }
            self.note(
                "check_expr_operand_domain",
                "error",
                schema,
                format!("repeat increment control bounds shall be numeric (13.6.1) ({t})"),
            );
        }
    }

    fn check_return_type(&mut self, schema: usize, owner_ptr: &str, ptr: &str) {
        let owner = self.node(owner_ptr);
        let is_function = cls(owner).ends_with("Declarations::Function");
        let has_expression = obj(self.node(ptr), "expression").is_some();
        if !is_function {
            if has_expression {
                self.note(
                    "check_expr_return",
                    "error",
                    schema,
                    "a procedure return shall not specify an expression (13.7)".to_string(),
                );
            }
            return;
        }
        if !has_expression {
            return;
        }
        if self.node(&format!("{owner_ptr}/return_type")).is_null() {
            return;
        }
        let declared = self.static_type(schema, &format!("{owner_ptr}/return_type"), 0);
        let expressed = self.static_type(schema, &format!("{ptr}/expression"), 0);
        if undecidable_2(declared, expressed) {
            return;
        }
        if self.initializer_compatible(
            schema,
            &format!("{owner_ptr}/return_type"),
            declared,
            &format!("{ptr}/expression"),
            expressed,
        ) {
            return;
        }
        self.note(
            "check_expr_return",
            "error",
            schema,
            format!(
                "return expression {expressed} is not compatible with the declared return {declared} (13.7, 12.11)"
            ),
        );
    }

    // ---- WHERE / UNIQUE structure ----

    fn check_rule_structure(&mut self, schema: usize) {
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        for (j, _entity) in arr(node, "entities").iter().enumerate() {
            let eptr = format!("{base}/entities/{j}");
            let rules_ptr = format!("{eptr}/where_rules");
            self.check_where_booleans(schema, &eptr, &rules_ptr);
            self.check_unique_labels(schema, &eptr);
        }
        for (j, _type_) in arr(node, "types").iter().enumerate() {
            let tptr = format!("{base}/types/{j}");
            let rules_ptr = format!("{tptr}/where_rules");
            self.check_where_booleans(schema, &tptr, &rules_ptr);
        }
        for (j, _rule) in arr(node, "rules").iter().enumerate() {
            let rptr = format!("{base}/rules/{j}");
            let rules_ptr = format!("{rptr}/where_rules");
            self.check_where_booleans(schema, &rptr, &rules_ptr);
        }
    }

    fn check_where_booleans(&mut self, schema: usize, owner_ptr: &str, rules_ptr: &str) {
        let env = self.expression_env(schema, Some(owner_ptr));
        self.env_stack.push(env);
        let Some(rules_val) = self.root.pointer(rules_ptr) else {
            self.env_stack.pop();
            return;
        };
        if let Value::Array(items) = rules_val {
            for (j, _item) in items.iter().enumerate() {
                let rule_ptr = format!("{rules_ptr}/{j}");
                let rule = self.node(&rule_ptr);
                let rule_id = id_of(rule).unwrap_or_default().to_string();
                let expr_ptr = format!("{rule_ptr}/expression");
                if self.node(&expr_ptr).is_null() {
                    continue;
                }
                let t = self.static_type(schema, &expr_ptr, 0);
                if undecidable_1(t) || is_logical(t) {
                    continue;
                }
                self.note(
                    "check_expr_where_boolean",
                    "error",
                    schema,
                    format!("WHERE '{rule_id}' shall be a logical expression (9.4)"),
                );
            }
        }
        self.env_stack.pop();
    }

    fn check_unique_labels(&mut self, schema: usize, entity_ptr: &str) {
        let mut seen: HashSet<String> = HashSet::new();
        let entity = self.node(entity_ptr);
        for rule in arr(entity, "unique_rules").iter() {
            let Some(id) = id_of(rule) else {
                continue;
            };
            let key = safe_downcase(id);
            if seen.contains(&key) {
                self.note(
                    "check_expr_unique_duplicate",
                    "error",
                    schema,
                    format!("duplicate UNIQUE label '{id}' (9.4)"),
                );
            }
            seen.insert(key);
        }
    }

    // ---- edition awareness ----

    fn check_dialect(&mut self, schema: usize, dialect: Option<&str>) {
        let is_1994 = dialect == Some("1994");
        let modern = self.dialect_2004_constructs(schema);
        let mut deprecated = Vec::new();
        {
            let node = self.schema(schema);
            let base = self.schema_ptr(schema).to_string();
            for (j, entity) in arr(node, "entities").iter().enumerate() {
                let expr_ptr = format!("{base}/entities/{j}/supertype_expression");
                let expr = self.node(&expr_ptr);
                if cls(expr).ends_with("SupertypeExpressions::OneofSupertypeExpression")
                    || cls(expr).ends_with("SupertypeExpressions::BinarySupertypeExpression")
                {
                    deprecated.push((
                        format!("{base}/entities/{j}"),
                        id_of(entity).unwrap_or_default().to_string(),
                    ));
                }
            }
        }
        if is_1994 {
            for (ptr, message) in &modern {
                let _ = ptr;
                self.note(
                    "check_dialect_2004_only",
                    "error",
                    schema,
                    format!("dialect 10303-11:1994: {message}"),
                );
            }
        } else {
            for (_ptr, entity_id) in &deprecated {
                self.note(
                    "check_dialect_deprecated",
                    "warning",
                    schema,
                    format!(
                        "dialect 10303-11:2004: {entity_id}: inline subtype constraint syntax is deprecated; use SUBTYPE_CONSTRAINT (Annex K)"
                    ),
                );
            }
        }
        if !modern.is_empty() && !deprecated.is_empty() {
            let first = deprecated.first().map(|(p, _)| p.clone());
            let _ = first;
            self.note(
                "check_dialect_mixed",
                "warning",
                schema,
                "dialect: schema mixes 2004-only declarations with edition-1 inline subtype constraint syntax".to_string(),
            );
        }
    }

    fn dialect_2004_constructs(&self, schema: usize) -> Vec<(String, String)> {
        let mut constructs = Vec::new();
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        for (j, type_) in arr(node, "types").iter().enumerate() {
            let type_id = id_of(type_).unwrap_or_default().to_string();
            let Some(underlying) = obj(type_, "underlying_type") else {
                continue;
            };
            let ut_ptr = format!("{base}/types/{j}/underlying_type");
            if fld(underlying, "extensible")
                .map(|v| v == &Value::Bool(true))
                .unwrap_or(false)
            {
                constructs.push((
                    ut_ptr.clone(),
                    format!("{type_id}: EXTENSIBLE is not part of ISO 10303-11:1994 (2004 edition, 8.4.1/8.4.2)"),
                ));
            }
            if fld(underlying, "based_on")
                .map(|v| !v.is_null())
                .unwrap_or(false)
            {
                constructs.push((
                    ut_ptr.clone(),
                    format!("{type_id}: BASED_ON is not part of ISO 10303-11:1994 (2004 edition, 8.4.1/8.4.2)"),
                ));
            }
            if fld(underlying, "generic_entity")
                .map(|v| v == &Value::Bool(true))
                .unwrap_or(false)
            {
                constructs.push((
                    ut_ptr,
                    format!("{type_id}: GENERIC_ENTITY is not part of ISO 10303-11:1994 (2004 edition, 8.4.2)"),
                ));
            }
        }
        for (j, constraint) in arr(node, "subtype_constraints").iter().enumerate() {
            let sc_id = id_of(constraint).unwrap_or_default().to_string();
            constructs.push((
                format!("{base}/subtype_constraints/{j}"),
                format!("{sc_id}: SUBTYPE_CONSTRAINT is not part of ISO 10303-11:1994 (2004 edition, 9; Annex G)"),
            ));
        }
        constructs
    }
}

fn is_aggregate_type(node: &Value) -> bool {
    let class = cls(node);
    class.contains("DataTypes::List")
        || class.contains("DataTypes::Set")
        || class.contains("DataTypes::Bag")
        || class.contains("DataTypes::Array")
}

fn where_label_ok(id: &str) -> bool {
    let upper = id.to_uppercase();
    upper.starts_with("WR") && upper.len() > 2 && upper[2..].chars().all(|c| c.is_ascii_digit())
}

fn unique_label_ok(id: &str) -> bool {
    let upper = id.to_uppercase();
    upper.starts_with("UR") && upper.len() > 2 && upper[2..].chars().all(|c| c.is_ascii_digit())
}

fn integer_bound(bound: Option<&Value>) -> Option<i64> {
    let bound = bound?;
    if !cls(bound).ends_with("Literals::Integer") {
        return None;
    }
    match bound.get("value") {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    }
}

fn split_schema_item(value: &str) -> Option<(String, String)> {
    let mut parts = value.split('.');
    let schema = parts.next()?;
    let item = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let word = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !word(schema) || !word(item) {
        return None;
    }
    Some((schema.to_string(), item.to_string()))
}

fn binary_domain_violation(operator: &str, left: &str, right: &str) -> Option<String> {
    match operator {
        "ADDITION" => {
            if numeric_pair(left, right)
                || same_pair(left, right, "string")
                || same_pair(left, right, "binary")
                || left == "aggregate"
                || right == "aggregate"
            {
                return None;
            }
            Some(
                "addition requires numeric, string, binary or aggregate operands (12.1, 12.3.2, 12.5.2, 12.6)"
                    .to_string(),
            )
        }
        "SUBTRACTION" => {
            if numeric_pair(left, right)
                || (left == "aggregate" && right != "unknown")
                || same_pair(left, right, "aggregate")
            {
                return None;
            }
            Some("difference requires an aggregate left operand (12.1, 12.6)".to_string())
        }
        "MULTIPLICATION" => {
            if numeric_pair(left, right) || same_pair(left, right, "aggregate") {
                return None;
            }
            Some("intersection requires aggregate operands (12.1, 12.6)".to_string())
        }
        "REAL_DIVISION" | "EXPONENTIATION" | "INTEGER_DIVISION" | "MODULO" => {
            if numeric_pair(left, right) {
                None
            } else {
                Some("operator requires numeric operands (12.1)".to_string())
            }
        }
        "AND" | "OR" | "XOR" => {
            if is_logical(left) && is_logical(right) {
                None
            } else {
                Some("logical operator requires logical operands (12.4)".to_string())
            }
        }
        "EQUAL" | "NOT_EQUAL" => {
            if comparable_equality(left, right) {
                None
            } else {
                Some("value comparison requires comparable operands (12.2.1)".to_string())
            }
        }
        "LESS_THAN" | "LESS_THAN_OR_EQUAL" | "GREATER_THAN" | "GREATER_THAN_OR_EQUAL" => {
            if left == "aggregate" || right == "aggregate" {
                if left == "aggregate" && right == "aggregate" {
                    return None;
                }
                return Some("subset/superset requires aggregate operands (12.6)".to_string());
            }
            if same_ordering_family(left, right) {
                None
            } else {
                Some("ordering comparison requires same-family operands (12.2.1)".to_string())
            }
        }
        "INSTANCE_EQUAL" | "INSTANCE_NOT_EQUAL" => {
            let ok = |t: &str| t == "entity" || t == "aggregate" || t == "select";
            if ok(left) && ok(right) {
                None
            } else {
                Some(
                    "instance comparison requires entity or aggregate operands (12.2.2)"
                        .to_string(),
                )
            }
        }
        "IN" => {
            if right == "aggregate" {
                None
            } else {
                Some("membership requires an aggregate right operand (12.2.3)".to_string())
            }
        }
        "LIKE" => {
            if left == "string" && right == "string" {
                None
            } else {
                Some("like requires string operands (12.2.5)".to_string())
            }
        }
        "COMBINE" => {
            if left == "entity" && right == "entity" {
                None
            } else {
                Some(
                    "complex entity construction requires entity operands (12, precedence table note)"
                        .to_string(),
                )
            }
        }
        _ => None,
    }
}

fn comparable_equality(left: &str, right: &str) -> bool {
    let family_ok = |t: &str| t == "aggregate" || t == "entity" || t == "select";
    if family_ok(left) && family_ok(right) {
        return true;
    }
    same_ordering_family(left, right)
}

fn binary_operator_spelling(symbol: &str) -> String {
    match symbol {
        "ADDITION" => "+".to_string(),
        "SUBTRACTION" => "-".to_string(),
        "REAL_DIVISION" => "/".to_string(),
        "INTEGER_DIVISION" => "div".to_string(),
        "MODULO" => "mod".to_string(),
        "INSTANCE_EQUAL" => ":=:".to_string(),
        "INSTANCE_NOT_EQUAL" => ":<>:".to_string(),
        other => other.to_lowercase(),
    }
}
