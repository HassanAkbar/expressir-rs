//! The name-resolution core: Ruby `ModelElement#find`, `ModelElement#path`,
//! `ResolveReferencesModelVisitor`, and the checker's schema-scope
//! lookups (`find_entity`, `find_type`, `resolve_type`, visible
//! declarations, alias/bound maps).

use super::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub(crate) static EMPTY_ALIAS_MAP: std::sync::LazyLock<HashMap<String, (usize, String)>> =
    std::sync::LazyLock::new(HashMap::new);
pub(crate) static EMPTY_BOUND: std::sync::LazyLock<HashSet<String>> =
    std::sync::LazyLock::new(HashSet::new);

impl<'a> Ctx<'a> {
    /// Fill the derived maps the families read: per-schema alias maps,
    /// bound-identifier sets, schema children_by_id caches.
    pub(crate) fn prepare_maps(&mut self) {
        for i in 0..self.schemas.len() {
            self.alias_map_mut(i);
            self.fill_bound_identifiers(i);
            let ptr = self.schema_ptr(i).to_string();
            let children = self.schema_children_by_id(i);
            self.children_cache.insert(ptr, children);
        }
    }

    // ---- reference resolution (ResolveReferencesModelVisitor) ----

    /// Walk the whole wire; for every `SimpleReference` not under an
    /// `AttributeReference` or an interface item list, fill `base_path`
    /// exactly like Ruby's visitor.
    pub(crate) fn collect_reference_fills(&self) -> Vec<(String, Option<String>)> {
        let mut fills: Vec<(String, Option<String>)> = Vec::new();
        for i in 0..self.schemas.len() {
            let schema_root = self.schema_ptr(i).to_string();
            for ptr in each_node(self.root, &schema_root) {
                let node = self.node(&ptr);
                if !is(node, "References::SimpleReference") {
                    continue;
                }
                let Some(parent_ptr) = self.parent.get(&ptr).cloned() else {
                    continue;
                };
                let parent = self.node(&parent_ptr);
                if is(parent, "References::AttributeReference") {
                    continue;
                }
                let Some(id) = id_of(node) else {
                    continue;
                };
                let search = if is(parent, "Declarations::InterfaceItem") {
                    // `node.find("#{iface.schema.id}.#{iface_item.ref.id}")`
                    let Some(iface_ptr) = self.parent.get(&parent_ptr).cloned() else {
                        continue;
                    };
                    let iface = self.node(&iface_ptr);
                    let iface_schema = match fld(iface, "schema") {
                        Some(Value::String(s)) => s.clone(),
                        Some(v) if v.is_object() => id_of(v).unwrap_or_default().to_string(),
                        _ => continue,
                    };
                    let item_ref_id = match fld(parent, "ref") {
                        Some(Value::String(s)) => s.clone(),
                        Some(r) if r.is_object() => id_of(r).unwrap_or_default().to_string(),
                        _ => continue,
                    };
                    format!("{iface_schema}.{item_ref_id}")
                } else {
                    id.to_string()
                };
                let base = self.find(&ptr, &search);
                fills.push((ptr, base.and_then(|b| self.node_path(&b))));
            }
        }
        fills
    }

    /// Ruby `ModelElement#find`: fold the path, then walk scopes from
    /// `from_ptr` upward until the parts resolve.
    #[doc(hidden)]
    pub fn find(&self, from_ptr: &str, path: &str) -> Option<String> {
        if path.is_empty() {
            return Some(from_ptr.to_string());
        }
        let parts: Vec<String> = path
            .to_lowercase()
            .split('.')
            .map(|part| {
                if part.starts_with("__") {
                    part.to_string()
                } else {
                    match part.rfind(':') {
                        Some(idx) => part[idx + 1..].to_string(),
                        None => part.to_string(),
                    }
                }
            })
            .collect();
        let mut scope = from_ptr.to_string();
        loop {
            if let Some(hit) = self.resolve_path_in_scope(&scope, &parts) {
                return Some(self.deref_interfaced(&hit));
            }
            match self.parent.get(&scope) {
                Some(p) if !p.is_empty() => scope = p.clone(),
                _ => {
                    // Last scope: the wire root.
                    return self
                        .resolve_path_in_scope("", &parts)
                        .map(|hit| self.deref_interfaced(&hit));
                }
            }
        }
    }

    fn deref_interfaced(&self, ptr: &str) -> String {
        let node = self.node(ptr);
        if is(node, "Declarations::InterfacedItem") {
            if let Some(base) = obj(node, "base_item") {
                if let Some(base_ptr) = self.ptr_of(base) {
                    return base_ptr;
                }
            }
        }
        ptr.to_string()
    }

    /// Reverse-lookup a wire pointer for a node reached through a
    /// non-collection attribute. Only used for `base_item` derefs.
    fn ptr_of(&self, node: &Value) -> Option<String> {
        fn walk(cur: &Value, ptr: &str, target: &Value, out: &mut Option<String>) -> bool {
            if std::ptr::eq(cur, target) {
                out.get_or_insert(ptr.to_string());
                return true;
            }
            if let Value::Object(map) = cur {
                for (k, v) in map {
                    match v {
                        Value::Array(items) => {
                            for (i, item) in items.iter().enumerate() {
                                if walk(item, &format!("{ptr}/{k}/{i}"), target, out) {
                                    return true;
                                }
                            }
                        }
                        Value::Object(_) if walk(v, &format!("{ptr}/{k}"), target, out) => {
                            return true;
                        }
                        _ => {}
                    }
                }
            }
            false
        }
        let mut out = None;
        walk(self.root, "", node, &mut out);
        out
    }

    /// Ruby `resolve_path_in_scope`: parts through the scope's
    /// children_by_id with SELF\supertype redeclaration handling.
    pub(crate) fn resolve_path_in_scope(
        &self,
        scope_ptr: &str,
        parts: &[String],
    ) -> Option<String> {
        let mut current = scope_ptr.to_string();
        let mut pending_supertype: Option<String> = None;
        for part in parts {
            if let Some(rest) = part.strip_prefix("self\\") {
                pending_supertype = Some(rest.to_string());
                continue;
            }
            if let Some(supertype) = pending_supertype.take() {
                let child = self.find_redeclared_by_supertype(&current, part, &supertype)?;
                current = child;
            } else {
                let by_id = self.children_by_id(&current);
                current = by_id.get(part.as_str())?.clone();
            }
        }
        if pending_supertype.is_some() {
            return None;
        }
        Some(current)
    }

    fn find_redeclared_by_supertype(
        &self,
        scope_ptr: &str,
        attr_name: &str,
        supertype: &str,
    ) -> Option<String> {
        let scope = self.node(scope_ptr);
        for attr_collection in ["attributes", "derived_attributes", "inverse_attributes"] {
            for (j, child) in arr(scope, attr_collection).iter().enumerate() {
                if !is(child, "Declarations::Attribute") {
                    continue;
                }
                let Some(id) = id_of(child) else {
                    continue;
                };
                if safe_downcase(id) != *attr_name {
                    continue;
                }
                let Some(sa) = obj(child, "supertype_attribute") else {
                    continue;
                };
                let Some(group) = obj(sa, "ref") else {
                    continue;
                };
                let Some(entity) = obj(group, "entity") else {
                    continue;
                };
                if let Some(eid) = id_of(entity) {
                    if safe_downcase(eid) == supertype {
                        return Some(format!("{scope_ptr}/{attr_collection}/{j}"));
                    }
                }
            }
        }
        None
    }

    /// Ruby `children_by_id` (LAST id wins) over the class-specific
    /// `#children` list. Schema interface expansions fold to their
    /// foreign base-item pointers (find derefs InterfacedItem at once).
    #[doc(hidden)]
    pub fn children_by_id(&self, scope_ptr: &str) -> HashMap<String, String> {
        if self.schema_index_of_ptr(scope_ptr).is_some() {
            if let Some(cached) = self.children_cache.get(scope_ptr) {
                return cached.clone();
            }
        }
        self.build_children_by_id(scope_ptr)
    }

    /// The schema index for `/schemas/N` and `/files/I/schemas/J`
    /// pointers.
    pub(crate) fn schema_index_of_ptr(&self, ptr: &str) -> Option<usize> {
        let rest = ptr.strip_prefix("/schemas/").or_else(|| {
            ptr.strip_prefix("/files/")
                .and_then(|rest| rest.split_once("/schemas/"))
                .map(|(_, rest)| rest)
        })?;
        rest.parse::<usize>()
            .ok()
            .filter(|&i| i < self.schemas.len())
    }

    fn schema_children_by_id(&mut self, schema: usize) -> HashMap<String, String> {
        let ptr = self.schema_ptr(schema).to_string();
        let map = self.schema_children_by_id_inner(schema);
        self.children_cache.insert(ptr, map.clone());
        map
    }

    pub(crate) fn schema_children_by_id_inner(&self, schema: usize) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = HashMap::new();
        // Ruby Schema#children: interfaced_items first, then
        // safe_children — later collections win id collisions.
        let mut visited: HashSet<String> = HashSet::new();
        let iface_count = arr(self.schema(schema), "interfaces").len();
        let base = self.schema_ptr(schema).to_string();
        for k in 0..iface_count {
            let iface_ptr = format!("{base}/interfaces/{k}");
            for (name, base_ptr) in self.interface_expansion(&iface_ptr, &mut visited) {
                out.insert(name, base_ptr);
            }
        }
        let node = self.schema(schema);
        for attr in ["constants", "types"] {
            for (j, child) in arr(node, attr).iter().enumerate() {
                if let Some(id) = id_of(child) {
                    out.insert(safe_downcase(id), format!("{base}/{attr}/{j}"));
                }
            }
        }
        let tcount = arr(node, "types").len();
        for j in 0..tcount {
            let type_ptr = format!("{base}/types/{j}");
            self.push_enum_items(&type_ptr, &mut out);
        }
        for attr in [
            "entities",
            "subtype_constraints",
            "functions",
            "rules",
            "procedures",
            "remark_items",
        ] {
            for (j, child) in arr(node, attr).iter().enumerate() {
                if let Some(id) = id_of(child) {
                    out.insert(safe_downcase(id), format!("{base}/{attr}/{j}"));
                }
            }
        }
        out
    }

    fn push_enum_items(&self, type_ptr: &str, out: &mut HashMap<String, String>) {
        // Ruby Type#enumeration_items: ENUMERATION items only — a
        // SELECT's items are not children of the type.
        let Some(t) = self.root.pointer(type_ptr) else {
            return;
        };
        let Some(ut) = obj(t, "underlying_type") else {
            return;
        };
        if !cls(ut).ends_with("DataTypes::Enumeration") {
            return;
        }
        for (k, item) in arr(ut, "items").iter().enumerate() {
            if let Some(id) = id_of(item) {
                out.insert(
                    safe_downcase(id),
                    format!("{type_ptr}/underlying_type/items/{k}"),
                );
            }
        }
    }

    fn build_children_by_id(&self, scope_ptr: &str) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = HashMap::new();
        if scope_ptr.is_empty() {
            for (ptr, id) in self.schemas.iter() {
                out.insert(safe_downcase(id), ptr.clone());
            }
            return out;
        }
        let scope = self.node(scope_ptr);
        let tail = class_tail(scope);
        let base = scope_ptr.to_string();
        let push = |out: &mut HashMap<String, String>, attr: &str| {
            for (j, child) in arr(scope, attr).iter().enumerate() {
                if let Some(id) = id_of(child) {
                    out.insert(safe_downcase(id), format!("{base}/{attr}/{j}"));
                }
            }
        };
        match tail {
            "ExpFile" | "Repository" => {
                for (ptr, id) in self.schemas.iter() {
                    out.insert(safe_downcase(id), ptr.clone());
                }
            }
            "Schema" => {
                if let Some(idx) = self.schema_index_of_ptr(scope_ptr) {
                    let map = self.schema_children_by_id_inner(idx);
                    for (k, v) in map {
                        out.insert(k, v);
                    }
                }
            }
            "Entity" => {
                for attr in [
                    "attributes",
                    "derived_attributes",
                    "inverse_attributes",
                    "unique_rules",
                    "where_rules",
                    "informal_propositions",
                    "remark_items",
                ] {
                    push(&mut out, attr);
                }
            }
            "Function" | "Procedure" => {
                for attr in ["parameters", "types"] {
                    push(&mut out, attr);
                }
                let tcount = arr(scope, "types").len();
                for j in 0..tcount {
                    self.push_enum_items(&format!("{base}/types/{j}"), &mut out);
                }
                for attr in [
                    "entities",
                    "subtype_constraints",
                    "functions",
                    "procedures",
                    "constants",
                    "variables",
                    "remark_items",
                ] {
                    push(&mut out, attr);
                }
            }
            "Rule" => {
                {
                    let attr = "types";
                    push(&mut out, attr);
                }
                let tcount = arr(scope, "types").len();
                for j in 0..tcount {
                    self.push_enum_items(&format!("{base}/types/{j}"), &mut out);
                }
                for attr in [
                    "entities",
                    "subtype_constraints",
                    "functions",
                    "procedures",
                    "constants",
                    "variables",
                    "where_rules",
                    "informal_propositions",
                    "remark_items",
                ] {
                    push(&mut out, attr);
                }
            }
            "Type" => {
                self.push_enum_items(scope_ptr, &mut out);
                for attr in ["where_rules", "informal_propositions", "remark_items"] {
                    push(&mut out, attr);
                }
            }
            "Alias"
            | "Repeat"
            | "QueryExpression"
            | "SubtypeConstraint"
            | "InformalPropositionRule"
            | "InterfacedItem" => {
                push(&mut out, "remark_items");
            }
            _ => {}
        }
        out
    }

    /// Ruby `Schema#items_for_interface`: the interface's visible names
    /// folded to the FOREIGN base-item pointers (find derefs
    /// InterfacedItem), plus transitive USE visibility, with the
    /// visited-schema dedup.
    pub(crate) fn debug_expansion_pub(
        &self,
        iface_ptr: &str,
        visited: &mut HashSet<String>,
    ) -> Vec<(String, String)> {
        self.interface_expansion(iface_ptr, visited)
    }

    fn interface_expansion(
        &self,
        iface_ptr: &str,
        visited: &mut HashSet<String>,
    ) -> Vec<(String, String)> {
        let iface = self.node(iface_ptr);
        let target = match fld(iface, "schema") {
            Some(Value::String(s)) => s.clone(),
            Some(v) if v.is_object() => id_of(v).unwrap_or_default().to_string(),
            _ => return Vec::new(),
        };
        let Some(foreign) = self.resolve_schema(&target) else {
            return Vec::new();
        };
        let foreign_id = self.schemas[foreign].1.clone();
        if visited.contains(&safe_downcase(&foreign_id)) {
            return Vec::new();
        }
        visited.insert(safe_downcase(&foreign_id));

        let kind = str_fld(iface, "kind").unwrap_or("USE");
        let fbase = self.schema_ptr(foreign).to_string();
        let fnode = self.schema(foreign);

        // Foreign safe_children children_by_id. Ruby's order:
        // constants, types, types' enumeration items, entities, ... —
        // entities overwrite same-named enumeration items.
        let mut by_id: HashMap<String, String> = HashMap::new();
        for attr in ["constants", "types"] {
            for (j, child) in arr(fnode, attr).iter().enumerate() {
                if let Some(id) = id_of(child) {
                    by_id.insert(safe_downcase(id), format!("{fbase}/{attr}/{j}"));
                }
            }
        }
        let tcount = arr(fnode, "types").len();
        for j in 0..tcount {
            let type_ptr = format!("{fbase}/types/{j}");
            let Some(t) = self.root.pointer(&type_ptr) else {
                continue;
            };
            let Some(ut) = obj(t, "underlying_type") else {
                continue;
            };
            if !cls(ut).ends_with("DataTypes::Enumeration") {
                continue;
            }
            for (k, item) in arr(ut, "items").iter().enumerate() {
                if let Some(id) = id_of(item) {
                    by_id.insert(
                        safe_downcase(id),
                        format!("{type_ptr}/underlying_type/items/{k}"),
                    );
                }
            }
        }
        for attr in [
            "entities",
            "subtype_constraints",
            "functions",
            "rules",
            "procedures",
            "remark_items",
        ] {
            for (j, child) in arr(fnode, attr).iter().enumerate() {
                if let Some(id) = id_of(child) {
                    by_id.insert(safe_downcase(id), format!("{fbase}/{attr}/{j}"));
                }
            }
        }

        let mut out: Vec<(String, String)> = Vec::new();
        let item_count = arr(iface, "items").len();
        if item_count == 0 {
            if kind != "REFERENCE" {
                // Implicit USE: every foreign entity/type.
                for attr in ["entities", "types"] {
                    for (j, child) in arr(fnode, attr).iter().enumerate() {
                        if let Some(id) = id_of(child) {
                            out.push((safe_downcase(id), format!("{fbase}/{attr}/{j}")));
                        }
                    }
                }
            }
        } else {
            for item in arr(iface, "items").iter() {
                let ref_id = match fld(item, "ref") {
                    Some(Value::String(s)) => s.clone(),
                    Some(r) if r.is_object() => id_of(r).unwrap_or_default().to_string(),
                    _ => continue,
                };
                let Some(base_ptr) = by_id.get(safe_downcase(&ref_id).as_str()) else {
                    continue;
                };
                let base_cls = class_tail(self.node(base_ptr)).to_string();
                let kinds: &[&str] = if kind == "REFERENCE" {
                    REFERENCE_ITEM_KINDS
                } else {
                    USE_ITEM_KINDS
                };
                if !kinds.contains(&base_cls.as_str()) {
                    continue;
                }
                let visible = id_of(item).unwrap_or(&ref_id).to_string();
                out.push((safe_downcase(&visible), base_ptr.clone()));
            }
        }

        // Transitive USE visibility: the foreign schema's own USE
        // interfaces contribute unfiltered items.
        let foreign_iface_count = arr(fnode, "interfaces").len();
        for k in 0..foreign_iface_count {
            let foreign_iface = format!("{fbase}/interfaces/{k}");
            if str_fld(self.node(&foreign_iface), "kind").unwrap_or("USE") != "USE" {
                continue;
            }
            let mut sub_visited = visited.clone();
            let items = self.interface_expansion(&foreign_iface, &mut sub_visited);
            if !items.is_empty() {
                out.extend(items);
                *visited = sub_visited;
            }
        }
        out
    }

    /// Ruby `ModelElement#path`: dotted parent ids, skipping
    /// SimpleReference ids and Alias/Repeat/QueryExpression scopes.
    pub(crate) fn node_path(&self, ptr: &str) -> Option<String> {
        let node = self.node(ptr);
        if is(node, "Statements::Alias")
            || is(node, "Statements::Repeat")
            || is(node, "Expressions::QueryExpression")
        {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        let mut current = ptr.to_string();
        loop {
            let node = self.node(&current);
            if !is(node, "References::SimpleReference") {
                if let Some(id) = id_of(node) {
                    let is_ipr_shadow = is(node, "Declarations::RemarkItem")
                        && self
                            .parent
                            .get(&current)
                            .map(|p| {
                                let pnode = self.node(p);
                                is(pnode, "Declarations::InformalPropositionRule")
                                    && id_of(pnode) == Some(id)
                            })
                            .unwrap_or(false);
                    if !is_ipr_shadow {
                        parts.push(id.to_string());
                    }
                }
            }
            match self.parent.get(&current) {
                Some(p) if !p.is_empty() => current = p.clone(),
                _ => break,
            }
        }
        if parts.is_empty() {
            return None;
        }
        parts.reverse();
        Some(parts.join("."))
    }

    // ---- schema-scope lookups ----

    pub(crate) fn resolve_schema(&self, name: &str) -> Option<usize> {
        if name.is_empty() {
            return None;
        }
        let group = self.by_name.get(safe_downcase(name).as_str())?;
        Some(
            group
                .iter()
                .find(|&&i| self.schemas[i].1 == name)
                .copied()
                .unwrap_or(group[0]),
        )
    }

    pub(crate) fn find_entity(&self, schema: usize, id: &str) -> Option<String> {
        let key = safe_downcase(id);
        let base = self.schema_ptr(schema).to_string();
        let node = self.schema(schema);
        for (j, e) in arr(node, "entities").iter().enumerate() {
            if id_of(e).map(|x| safe_downcase(x) == key).unwrap_or(false) {
                return Some(format!("{base}/entities/{j}"));
            }
        }
        if let Some((foreign, original)) = self.alias_map(schema).get(&key) {
            let fnode = self.schema(*foreign);
            for (j, e) in arr(fnode, "entities").iter().enumerate() {
                if id_of(e)
                    .map(|x| safe_downcase(x) == *original)
                    .unwrap_or(false)
                {
                    return Some(format!("{}/entities/{j}", self.schema_ptr(*foreign)));
                }
            }
        }
        self.visible_from(schema, "entities").into_iter().find(|e| {
            id_of(self.node(e))
                .map(|x| safe_downcase(x) == key)
                .unwrap_or(false)
        })
    }

    pub(crate) fn find_type(&self, schema: usize, id: &str) -> Option<String> {
        let key = safe_downcase(id);
        let base = self.schema_ptr(schema).to_string();
        let node = self.schema(schema);
        for (j, t) in arr(node, "types").iter().enumerate() {
            if id_of(t).map(|x| safe_downcase(x) == key).unwrap_or(false) {
                return Some(format!("{base}/types/{j}"));
            }
        }
        None
    }

    /// `ref_id`: a String ref or a reference node's id.
    pub(crate) fn ref_id(node: &Value) -> Option<String> {
        match node {
            Value::String(s) => Some(s.clone()),
            v if v.is_object() => id_of(v).map(str::to_string),
            _ => None,
        }
    }

    pub(crate) fn resolve_type(&self, schema: usize, ref_node: &Value) -> bool {
        let Some(id) = Self::ref_id(ref_node) else {
            return false;
        };
        let key = safe_downcase(&id);
        if self.find_type(schema, &id).is_some() {
            return true;
        }
        if let Some((foreign, original)) = self.alias_map(schema).get(&key) {
            let fnode = self.schema(*foreign);
            if arr(fnode, "types").iter().any(|t| {
                id_of(t)
                    .map(|x| safe_downcase(x) == *original)
                    .unwrap_or(false)
            }) {
                return true;
            }
        }
        self.visible_from(schema, "types").iter().any(|t| {
            id_of(self.node(t))
                .map(|x| safe_downcase(x) == key)
                .unwrap_or(false)
        })
    }

    pub(crate) fn alias_map(&self, schema: usize) -> &HashMap<String, (usize, String)> {
        self.alias_maps
            .get(self.schema_ptr(schema))
            .unwrap_or(&EMPTY_ALIAS_MAP)
    }

    fn alias_map_mut(&mut self, schema: usize) {
        let ptr = self.schema_ptr(schema).to_string();
        if self.alias_maps.contains_key(&ptr) {
            return;
        }
        let map = {
            let mut map = HashMap::new();
            let node = self.schema(schema);
            let base = self.schema_ptr(schema).to_string();
            let count = arr(node, "interfaces").len();
            for k in 0..count {
                let iface_ptr = format!("{base}/interfaces/{k}");
                let iface = self.node(&iface_ptr);
                let target = match fld(iface, "schema") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) if v.is_object() => id_of(v).unwrap_or_default().to_string(),
                    _ => continue,
                };
                let Some(foreign) = self.resolve_schema(&target) else {
                    continue;
                };
                for item in arr(iface, "items").iter() {
                    let original = match fld(item, "ref") {
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(r) if r.is_object() => id_of(r).map(str::to_string),
                        _ => None,
                    };
                    let Some(original) = original else {
                        continue;
                    };
                    let Some(rename) = id_of(item) else {
                        continue;
                    };
                    map.insert(safe_downcase(rename), (foreign, safe_downcase(&original)));
                }
            }
            map
        };
        self.alias_maps.insert(ptr, map);
    }

    pub(crate) fn bound_identifiers(&self, schema: usize) -> &HashSet<String> {
        self.bound_ids
            .get(self.schema_ptr(schema))
            .unwrap_or(&EMPTY_BOUND)
    }

    fn fill_bound_identifiers(&mut self, schema: usize) {
        let mut bound = HashSet::new();
        bound.insert("?".to_string());
        let root = self.schema_ptr(schema).to_string();
        for node_ptr in each_node(self.root, &root) {
            let node = self.node(&node_ptr);
            if is(node, "Expressions::QueryExpression") || is(node, "Statements::Repeat") {
                if let Some(id) = id_of(node) {
                    bound.insert(safe_downcase(id));
                }
            }
        }
        for e in self.entity_ptrs(schema) {
            self.collect_attribute_names(&e, &mut bound);
        }
        for e in self.visible_from(schema, "entities") {
            self.collect_attribute_names(&e, &mut bound);
        }
        let ptr = self.schema_ptr(schema).to_string();
        self.bound_ids.insert(ptr, bound);
    }

    pub(crate) fn entity_ptrs(&self, schema: usize) -> Vec<String> {
        let base = self.schema_ptr(schema).to_string();
        let count = arr(self.schema(schema), "entities").len();
        (0..count).map(|j| format!("{base}/entities/{j}")).collect()
    }

    fn collect_attribute_names(&self, entity_ptr: &str, bound: &mut HashSet<String>) {
        let entity = self.node(entity_ptr);
        for attr in arr(entity, "attributes").iter() {
            if let Some(id) = id_of(attr) {
                bound.insert(safe_downcase(id));
            }
        }
    }

    /// Ruby `visible_from`: foreign declarations through this schema's
    /// interfaces, filtered by the interface's resource names when it
    /// carries a list.
    pub(crate) fn visible_from(&self, schema: usize, collection: &str) -> Vec<String> {
        let mut out = Vec::new();
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        let count = arr(node, "interfaces").len();
        for k in 0..count {
            let iface_ptr = format!("{base}/interfaces/{k}");
            let iface = self.node(&iface_ptr);
            let target = match fld(iface, "schema") {
                Some(Value::String(s)) => s.clone(),
                Some(v) if v.is_object() => id_of(v).unwrap_or_default().to_string(),
                _ => continue,
            };
            let Some(foreign) = self.resolve_schema(&target) else {
                continue;
            };
            let names = self.resource_names(iface);
            let fnode = self.schema(foreign);
            let fbase = self.schema_ptr(foreign).to_string();
            for (j, decl) in arr(fnode, collection).iter().enumerate() {
                let Some(id) = id_of(decl) else {
                    continue;
                };
                if !names.is_empty() && !names.contains(&safe_downcase(id)) {
                    continue;
                }
                out.push(format!("{fbase}/{collection}/{j}"));
            }
        }
        out
    }

    pub(crate) fn resource_names(&self, iface: &Value) -> Vec<String> {
        arr(iface, "items")
            .iter()
            .filter_map(|item| {
                let name = match fld(item, "ref") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(r) if r.is_object() => id_of(r).map(str::to_string),
                    _ => None,
                };
                name.or_else(|| id_of(item).map(str::to_string))
                    .map(|n| safe_downcase(&n))
            })
            .collect()
    }

    pub(crate) fn foreign_declaration(&self, schema: usize, name: &str) -> Option<String> {
        let key = safe_downcase(name);
        let node = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        for attr in [
            "types",
            "entities",
            "functions",
            "procedures",
            "constants",
            "rules",
            "subtype_constraints",
        ] {
            for (j, d) in arr(node, attr).iter().enumerate() {
                if id_of(d).map(|x| safe_downcase(x) == key).unwrap_or(false) {
                    return Some(format!("{base}/{attr}/{j}"));
                }
            }
        }
        None
    }

    /// Ruby `resolves_to_type_or_entity?`: a same-named child of the
    /// schema that is a Type, an Entity, or an interface expansion of
    /// one.
    pub(crate) fn resolves_to_type_or_entity(&self, schema: usize, name: &str) -> bool {
        let key = safe_downcase(name);
        let node = self.schema(schema);
        for attr in ["types", "entities"] {
            if arr(node, attr).iter().any(|child| {
                id_of(child)
                    .map(|x| safe_downcase(x) == key)
                    .unwrap_or(false)
            }) {
                return true;
            }
        }
        // Interface-expanded names whose base is a Type or Entity.
        let base = self.schema_ptr(schema).to_string();
        let iface_count = arr(node, "interfaces").len();
        let mut visited: HashSet<String> = HashSet::new();
        for k in 0..iface_count {
            let iface_ptr = format!("{base}/interfaces/{k}");
            for (visible, base_ptr) in self.interface_expansion(&iface_ptr, &mut visited) {
                if visible == key {
                    let tail = class_tail(self.node(&base_ptr));
                    if tail == "Type" || tail == "Entity" {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Ruby `Entity#supertype_closure`: BFS over SUBTYPE OF names,
    /// repo-wide entity resolution, cycle-safe.
    pub(crate) fn supertype_closure(&self, entity_ptr: &str) -> Vec<String> {
        let entity = self.node(entity_ptr);
        let mut visited: HashSet<String> = HashSet::new();
        if let Some(id) = id_of(entity) {
            visited.insert(safe_downcase(id));
        }
        let mut queue: std::collections::VecDeque<String> = arr(entity, "subtype_of")
            .iter()
            .filter_map(Self::ref_id)
            .collect();
        let mut out = Vec::new();
        while let Some(name) = queue.pop_front() {
            let key = safe_downcase(&name);
            if visited.contains(&key) {
                continue;
            }
            let Some(supertype) = self.find_supertype_entity(&key) else {
                continue;
            };
            visited.insert(key);
            out.push(supertype.clone());
            for r in arr(self.node(&supertype), "subtype_of").iter() {
                if let Some(id) = Self::ref_id(r) {
                    queue.push_back(id);
                }
            }
        }
        out
    }

    fn find_supertype_entity(&self, key: &str) -> Option<String> {
        for i in 0..self.schemas.len() {
            let base = self.schema_ptr(i).to_string();
            for (j, e) in arr(self.schema(i), "entities").iter().enumerate() {
                if id_of(e).map(|x| safe_downcase(x) == key).unwrap_or(false) {
                    return Some(format!("{base}/entities/{j}"));
                }
            }
        }
        None
    }

    /// Ruby `SelfSchemaReference.repository_of`: a Repository
    /// ancestor above the schema. The wire root is an ExpFile unless
    /// the caller passed a multi-file repository wire.
    pub(crate) fn has_repository_ancestor(&self, schema: usize) -> bool {
        let mut current = self.schema_ptr(schema).to_string();
        loop {
            match self.parent.get(&current) {
                Some(p) if !p.is_empty() => current = p.clone(),
                _ => return cls(self.node("")).ends_with("Model::Repository"),
            }
        }
    }

    /// `SelfSchemaReference.declared?` — any declaration collection
    /// holds the folded name.
    pub(crate) fn schema_declares(&self, schema: usize, key: &str) -> bool {
        let node = self.schema(schema);
        for attr in [
            "types",
            "entities",
            "functions",
            "procedures",
            "constants",
            "rules",
            "subtype_constraints",
        ] {
            if arr(node, attr)
                .iter()
                .any(|d| id_of(d).map(|x| safe_downcase(x) == key).unwrap_or(false))
            {
                return true;
            }
        }
        false
    }
}
