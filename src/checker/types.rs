//! The static-type lattice and the expression type checks
//! (`static_type`, `binary_domain_violation`, declared-type-node
//! resolution). Everything addresses wire nodes by pointer so child
//! fields and schema scoping stay unambiguous.

use super::*;
use serde_json::Value;
use std::sync::OnceLock;

pub(crate) fn type_symbol_for_class(class: &str) -> Option<&'static str> {
    Some(match class {
        "Expressir::Model::DataTypes::Integer" => "integer",
        "Expressir::Model::DataTypes::Real" => "real",
        "Expressir::Model::DataTypes::Number" => "number",
        "Expressir::Model::DataTypes::String" => "string",
        "Expressir::Model::DataTypes::Binary" => "binary",
        "Expressir::Model::DataTypes::Boolean" => "boolean",
        "Expressir::Model::DataTypes::Logical" => "logical",
        "Expressir::Model::DataTypes::List" => "aggregate",
        "Expressir::Model::DataTypes::Set" => "aggregate",
        "Expressir::Model::DataTypes::Bag" => "aggregate",
        "Expressir::Model::DataTypes::Array" => "aggregate",
        "Expressir::Model::Literals::Integer" => "integer",
        "Expressir::Model::Literals::Real" => "real",
        "Expressir::Model::Literals::String" => "string",
        "Expressir::Model::Literals::Binary" => "binary",
        "Expressir::Model::Literals::Logical" => "logical",
        "Expressir::Model::Expressions::AggregateInitializer" => "aggregate",
        "Expressir::Model::Expressions::QueryExpression" => "aggregate",
        "Expressir::Model::Expressions::EntityConstructor" => "entity",
        "Expressir::Model::References::GroupReference" => "entity",
        "Expressir::Model::DataTypes::Enumeration" => "enum",
        "Expressir::Model::DataTypes::Select" => "select",
        "Expressir::Model::DataTypes::Generic" => "generic",
        "Expressir::Model::DataTypes::GenericEntity" => "generic",
        _ => return None,
    })
}

/// The synthesized `GenericEntity.new` Ruby hands back from
/// `declared_type_node`; addressed by the reserved "#generic" pointer.
fn generic_entity_value() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| serde_json::json!({"_class": "Expressir::Model::DataTypes::GenericEntity"}))
}

impl<'a> Ctx<'a> {
    fn node_at(&self, ptr: &str) -> &'a Value {
        if ptr == "#generic" {
            return generic_entity_value();
        }
        self.node(ptr)
    }

    /// Ruby `static_type(schema, node, depth)`.
    pub(crate) fn static_type(&self, schema: usize, ptr: &str, depth: usize) -> &'static str {
        if ptr.is_empty() || depth > 8 {
            return "unknown";
        }
        let node = self.node_at(ptr);
        if node.is_null() {
            return "unknown";
        }
        if let Some(sym) = type_symbol_for_class(cls(node)) {
            return sym;
        }
        if is(node, "Expressions::BinaryExpression") {
            return self.binary_result_type(schema, ptr, depth);
        }
        if is(node, "Expressions::FunctionCall") {
            let Some(function) = self.schema_function_ptr(schema, ptr) else {
                return "unknown";
            };
            return self.static_type(schema, &format!("{function}/return_type"), depth + 1);
        }
        if is(node, "References::SimpleReference") {
            return self.reference_type(schema, ptr, depth);
        }
        if is(node, "References::AttributeReference") {
            return self.attribute_reference_type(schema, ptr, depth);
        }
        if is(node, "References::IndexReference") {
            return self.index_element_type(schema, ptr, depth);
        }
        "unknown"
    }

    fn binary_result_type(&self, schema: usize, ptr: &str, depth: usize) -> &'static str {
        let operator = str_fld(self.node_at(ptr), "operator").unwrap_or("");
        match operator {
            "EQUAL"
            | "NOT_EQUAL"
            | "LESS_THAN"
            | "LESS_THAN_OR_EQUAL"
            | "GREATER_THAN"
            | "GREATER_THAN_OR_EQUAL"
            | "IN"
            | "LIKE"
            | "INSTANCE_EQUAL"
            | "INSTANCE_NOT_EQUAL"
            | "AND"
            | "OR"
            | "XOR" => "logical",
            "ADDITION" => {
                let left = self.static_type(schema, &format!("{ptr}/operand1"), depth + 1);
                if is_numeric(left) {
                    match left {
                        "integer" => "integer",
                        "real" => "real",
                        _ => "number",
                    }
                } else {
                    "unknown"
                }
            }
            "REAL_DIVISION" => "real",
            "INTEGER_DIVISION" | "MODULO" => "integer",
            "EXPONENTIATION" => {
                let left = self.static_type(schema, &format!("{ptr}/operand1"), depth + 1);
                let right = self.static_type(schema, &format!("{ptr}/operand2"), depth + 1);
                if left == "integer" && right == "integer" {
                    "integer"
                } else {
                    "unknown"
                }
            }
            _ => "unknown",
        }
    }

    fn builtin_symbol(name: &str) -> &'static str {
        match name {
            "integer" => "integer",
            "real" => "real",
            "number" => "number",
            "string" => "string",
            "binary" => "binary",
            "boolean" => "boolean",
            "logical" => "logical",
            _ => "unknown",
        }
    }

    /// Ruby `schema_function`: the schema's own function matching the
    /// callee id (case-folded); returns the function pointer.
    pub(crate) fn schema_function_ptr(&self, schema: usize, call_ptr: &str) -> Option<String> {
        let callee = self.node_at(&format!("{call_ptr}/function"));
        if callee.is_null() || !is(callee, "References::SimpleReference") {
            return None;
        }
        let callee_id = id_of(callee)?;
        let key = safe_downcase(callee_id);
        let snode = self.schema(schema);
        let base = self.schema_ptr(schema).to_string();
        for (j, function) in arr(snode, "functions").iter().enumerate() {
            if id_of(function)
                .map(|x| safe_downcase(x) == key)
                .unwrap_or(false)
            {
                return Some(format!("{base}/functions/{j}"));
            }
        }
        None
    }

    fn reference_type(&self, schema: usize, ptr: &str, depth: usize) -> &'static str {
        let node = self.node_at(ptr);
        let Some(id) = id_of(node) else {
            return "unknown";
        };
        let key = safe_downcase(id);
        if builtin(id) {
            return Self::builtin_symbol(&key);
        }
        if let Some(env) = self.env() {
            if let Some(bound_ptr) = env.get(&key) {
                return self.static_type(schema, bound_ptr, depth + 1);
            }
        }
        if let Some(declaration) = self.find_type(schema, id) {
            return self.static_type(schema, &format!("{declaration}/underlying_type"), depth + 1);
        }
        if self.find_entity(schema, id).is_some() {
            "entity"
        } else {
            "unknown"
        }
    }

    /// `color.red` parses as an AttributeReference over a reference to
    /// the enumeration TYPE (base infers :enum). An entity-typed base
    /// resolves through the entity's own attributes.
    fn attribute_reference_type(&self, schema: usize, ptr: &str, depth: usize) -> &'static str {
        let base_ptr = format!("{ptr}/ref");
        if self.node_at(&base_ptr).is_null() {
            return "unknown";
        }
        let base = self.static_type(schema, &base_ptr, depth + 1);
        if base == "enum" {
            return "enum";
        }
        let base_node = self.node_at(&base_ptr);
        let Some(entity) = self.entity_for_reference(schema, base_node) else {
            return "unknown";
        };
        let attr_node = self.node_at(&format!("{ptr}/attribute"));
        let attr_id = id_of(attr_node).map(safe_downcase);
        let Some(attr_id) = attr_id else {
            return "unknown";
        };
        let entity_node = self.node(&entity);
        for (j, attr) in arr(entity_node, "attributes").iter().enumerate() {
            if id_of(attr)
                .map(|x| safe_downcase(x) == attr_id)
                .unwrap_or(false)
            {
                let type_ptr = format!("{entity}/attributes/{j}/type");
                if !self.node(&type_ptr).is_null() && self.node(&type_ptr).is_object() {
                    return self.static_type(schema, &type_ptr, depth + 1);
                }
                return "unknown";
            }
        }
        "unknown"
    }

    fn entity_for_reference(&self, schema: usize, node: &Value) -> Option<String> {
        if !is(node, "References::SimpleReference") {
            return None;
        }
        let id = id_of(node)?;
        self.find_entity(schema, id)
    }

    /// 12.6.1/12.3.1/12.5.1: indexing yields the aggregate's base
    /// element; string indexing yields a string; binary a binary.
    fn index_element_type(&self, schema: usize, ptr: &str, depth: usize) -> &'static str {
        let Some(element) =
            self.index_element_type_node_ptr(schema, &format!("{ptr}/ref"), depth + 1)
        else {
            return "unknown";
        };
        let symbol = self.static_type(schema, &element, depth + 1);
        if symbol != "unknown" {
            return symbol;
        }
        let element_node = self.node_at(&element);
        let class = cls(element_node);
        if class.contains("DataTypes::String") {
            return "string";
        }
        if class.contains("DataTypes::Binary") {
            return "binary";
        }
        "unknown"
    }

    pub(crate) fn index_element_type_node_ptr(
        &self,
        schema: usize,
        ptr: &str,
        depth: usize,
    ) -> Option<String> {
        if depth > 8 {
            return None;
        }
        let type_node = self.declared_type_node_ptr(schema, ptr, depth)?;
        let tnode = self.node_at(&type_node);
        let class = cls(tnode);
        if class.contains("DataTypes::List")
            || class.contains("DataTypes::Set")
            || class.contains("DataTypes::Bag")
            || class.contains("DataTypes::Array")
        {
            if obj(tnode, "base_type").is_some() {
                return Some(format!("{type_node}/base_type"));
            }
            return None;
        }
        if class.contains("DataTypes::String") || class.contains("DataTypes::Binary") {
            return Some(type_node);
        }
        None
    }

    /// The declared type NODE POINTER an expression position names:
    /// scope environment, schema types and entities, function returns.
    pub(crate) fn declared_type_node_ptr(
        &self,
        schema: usize,
        ptr: &str,
        depth: usize,
    ) -> Option<String> {
        if depth > 8 {
            return None;
        }
        let node = self.node_at(ptr);
        let class = cls(node);
        if class.ends_with("References::SimpleReference") {
            let key = id_of(node).map(safe_downcase)?;
            if let Some(env) = self.env() {
                if let Some(bound) = env.get(&key) {
                    return Some(bound.clone());
                }
            }
            let id = id_of(node)?;
            if let Some(t) = self.find_type(schema, id) {
                return Some(format!("{t}/underlying_type"));
            }
            if self.find_entity(schema, id).is_some() {
                return Some("#generic".to_string());
            }
            return None;
        }
        if class.ends_with("References::IndexReference") {
            return self.index_element_type_node_ptr(schema, &format!("{ptr}/ref"), depth + 1);
        }
        if class.ends_with("Expressions::FunctionCall") {
            let function = self.schema_function_ptr(schema, ptr)?;
            return Some(format!("{function}/return_type"));
        }
        None
    }
}
