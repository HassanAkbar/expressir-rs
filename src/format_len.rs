//! Formatted-length mirror of `Expressir::Express::Formatter` (the
//! no-remarks mode the remark attacher always runs in: the model carries
//! no remarks at attach time, so every remark emitter is empty and the
//! base `format` is the identity).
//!
//! Ruby's NodePositionIndex derives node spans from the FORMATTED text —
//! `end_line = line_number(source_offset + source.length)`, maxed with
//! collection children's own ends — so the attacher's index consumes
//! these lengths instead of raw source slice ends. Parity with Ruby is
//! asserted per node by `tests/formatted_len_parity.rs`.

use serde_json::Value;
use std::collections::HashMap;

/// Rendered size of a node's formatted text: byte length plus the number
/// of newlines it contains (indentation cost is 2 spaces per line).
#[derive(Clone, Copy, Default)]
pub struct Piece {
    pub len: usize,
    pub nl: usize,
}

impl Piece {
    fn flat(len: usize) -> Piece {
        Piece { len, nl: 0 }
    }
}

fn join_sep(parts: &[Piece], sep: &str) -> Piece {
    let sep_len = sep.len();
    let sep_nl = sep.matches('\n').count();
    let mut len = 0usize;
    let mut nl = 0usize;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            len += sep_len;
            nl += sep_nl;
        }
        len += p.len;
        nl += p.nl;
    }
    Piece { len, nl }
}

fn join_nl(parts: &[Piece]) -> Piece {
    join_sep(parts, "\n")
}

fn concat(parts: &[Piece]) -> Piece {
    join_sep(parts, "")
}

fn indent(p: Piece) -> Piece {
    Piece {
        len: p.len + 2 * (p.nl + 1),
        nl: p.nl,
    }
}

fn opt(p: Option<Piece>) -> Piece {
    p.unwrap_or_default()
}

/// singular child accessor: absent or null reads as nil.
fn child<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).filter(|x| !x.is_null())
}

fn is_truthy(v: &Value, key: &str) -> bool {
    match child(v, key) {
        Some(Value::Bool(false)) | None => false,
        Some(_) => true,
    }
}

fn arr<'a>(v: &'a Value, key: &str) -> Vec<&'a Value> {
    match v.get(key) {
        Some(Value::Array(items)) => items.iter().collect(),
        _ => Vec::new(),
    }
}

fn positive(v: &Value, key: &str) -> bool {
    !arr(v, key).is_empty()
}

fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
    child(v, key).and_then(Value::as_str).unwrap_or("")
}

fn class_of(v: &Value) -> &str {
    v.get("_class")
        .and_then(Value::as_str)
        .and_then(|c| c.strip_prefix("Expressir::Model::"))
        .unwrap_or("")
}

/// Formatted length of every node in the wire, keyed by JSON pointer.
pub fn lengths_by_ptr(root: &Value) -> HashMap<String, Piece> {
    let mut map = HashMap::new();
    if root.is_object() {
        class_len(root, "", &mut map);
    }
    map
}

fn class_len(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let piece = dispatch(v, ptr, map);
    // Schema records its HEAD length (the index-visible `source`); the
    // returned piece is the full render for the parent ExpFile.
    if class_of(v) != "Declarations::Schema" {
        map.insert(ptr.to_string(), piece);
    }
    piece
}

fn fmt(v: Option<&Value>, key: &str, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    match v.and_then(|n| child(n, key)) {
        Some(node) => class_len(node, &format!("{ptr}/{key}"), map),
        None => Piece::default(),
    }
}

fn fmt_each(v: &Value, key: &str, ptr: &str, map: &mut HashMap<String, Piece>) -> Vec<Piece> {
    arr(v, key)
        .iter()
        .enumerate()
        .map(|(i, item)| class_len(item, &format!("{ptr}/{key}/{i}"), map))
        .collect()
}

fn dispatched_len(parts: &[Piece]) -> Piece {
    concat(parts)
}

fn dispatch(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    match class_of(v) {
        "Repository" => {
            let files = fmt_each(v, "files", ptr, map);
            if !files.is_empty() {
                let mut p = join_sep(&files, "\n\n");
                p.len += 1;
                p.nl += 1;
                p
            } else {
                Piece::default()
            }
        }
        "ExpFile" => {
            let schemas = fmt_each(v, "schemas", ptr, map);
            if is_truthy(v, "schemas") {
                let mut p = join_sep(&schemas, "\n\n");
                p.len += 1;
                p.nl += 1;
                p
            } else {
                Piece::default()
            }
        }
        "Declarations::Schema" => schema(v, ptr, map),
        "Declarations::SchemaVersion" => dispatched_len(&[
            Piece::flat(1),
            Piece::flat(str_field(v, "value").chars().count()),
            Piece::flat(1),
        ]),
        "Declarations::SchemaVersionItem" | "Declarations::InterfacedItem" | "Cache" => {
            Piece::default()
        }
        "Declarations::Attribute"
        | "Declarations::DerivedAttribute"
        | "Declarations::InverseAttribute" => attribute(v, ptr, map),
        "Declarations::Constant" => dispatched_len(&[
            Piece::flat(str_field(v, "id").chars().count()),
            Piece::flat(3),
            fmt(Some(v), "type", ptr, map),
            Piece::flat(4),
            fmt(Some(v), "expression", ptr, map),
            Piece::flat(1),
        ]),
        "Declarations::Entity" => entity(v, ptr, map),
        "Declarations::Function" => function(v, ptr, map, "FUNCTION", "END_FUNCTION"),
        "Declarations::Procedure" => function(v, ptr, map, "PROCEDURE", "END_PROCEDURE"),
        "Declarations::Interface" => interface(v, ptr, map),
        "Declarations::InterfaceItem" => {
            let mut parts = vec![fmt(Some(v), "ref", ptr, map)];
            if is_truthy(v, "id") {
                parts.push(Piece::flat(str_field(v, "id").chars().count() + 4));
            }
            dispatched_len(&parts)
        }
        "Declarations::Parameter" => dispatched_len(&[
            opt(is_truthy(v, "var").then(|| Piece::flat(4))),
            Piece::flat(str_field(v, "id").chars().count()),
            Piece::flat(3),
            fmt(Some(v), "type", ptr, map),
        ]),
        "Declarations::Rule" => rule(v, ptr, map),
        "Declarations::SubtypeConstraint" => subtype_constraint(v, ptr, map),
        "Declarations::Type" => type_decl(v, ptr, map),
        "Declarations::UniqueRule" => {
            let mut parts = Vec::new();
            if is_truthy(v, "id") {
                parts.push(Piece::flat(str_field(v, "id").chars().count() + 2));
            }
            let attrs = fmt_each(v, "attributes", ptr, map);
            if !attrs.is_empty() {
                parts.push(join_sep(&attrs, ", "));
            }
            parts.push(Piece::flat(1));
            dispatched_len(&parts)
        }
        "Declarations::Variable" => {
            let mut parts = vec![
                Piece::flat(str_field(v, "id").chars().count()),
                Piece::flat(3),
                fmt(Some(v), "type", ptr, map),
            ];
            if is_truthy(v, "expression") {
                parts.push(Piece::flat(4));
                parts.push(fmt(Some(v), "expression", ptr, map));
            }
            parts.push(Piece::flat(1));
            dispatched_len(&parts)
        }
        "Declarations::WhereRule" => {
            let mut parts = Vec::new();
            if is_truthy(v, "id") {
                parts.push(Piece::flat(str_field(v, "id").chars().count() + 2));
            }
            parts.push(fmt(Some(v), "expression", ptr, map));
            parts.push(Piece::flat(1));
            dispatched_len(&parts)
        }
        "Declarations::InformalPropositionRule" => {
            if is_truthy(v, "id") {
                Piece::flat(str_field(v, "id").chars().count() + 3)
            } else {
                Piece::default()
            }
        }
        "Declarations::RemarkItem" => Piece::default(),
        "Statements::Alias" => {
            let mut parts = vec![concat(&[
                Piece::flat(11 + str_field(v, "id").chars().count()),
                fmt(Some(v), "expression", ptr, map),
                Piece::flat(1),
            ])];
            if positive(v, "statements") {
                let stmts = fmt_each(v, "statements", ptr, map);
                parts.push(indent(join_nl(&stmts)));
            }
            parts.push(Piece::flat(10));
            join_nl(&parts)
        }
        "Statements::Assignment" => dispatched_len(&[
            fmt(Some(v), "ref", ptr, map),
            Piece::flat(4),
            fmt(Some(v), "expression", ptr, map),
            Piece::flat(1),
        ]),
        "Statements::Case" => case(v, ptr, map),
        "Statements::CaseAction" => {
            let labels = fmt_each(v, "labels", ptr, map);
            let head = if labels.is_empty() {
                Piece::flat(2)
            } else {
                let j = join_sep(&labels, ", ");
                Piece {
                    len: j.len + 2,
                    nl: j.nl,
                }
            };
            let body = fmt(Some(v), "statement", ptr, map);
            join_nl(&[head, indent(body)])
        }
        "Statements::Compound" => {
            let mut parts = vec![Piece::flat(5)];
            if positive(v, "statements") {
                let stmts = fmt_each(v, "statements", ptr, map);
                parts.push(indent(join_nl(&stmts)));
            }
            parts.push(Piece::flat(4));
            join_nl(&parts)
        }
        "Statements::Escape" => Piece::flat(7),
        "Statements::If" => if_stmt(v, ptr, map),
        "Statements::Null" => Piece::flat(1),
        "Statements::ProcedureCall" => {
            let mut parts = vec![fmt(Some(v), "procedure", ptr, map)];
            if positive(v, "parameters") {
                let params = fmt_each(v, "parameters", ptr, map);
                parts.push(Piece::flat(1));
                parts.push(join_sep(&params, ", "));
                parts.push(Piece::flat(1));
            }
            parts.push(Piece::flat(1));
            dispatched_len(&parts)
        }
        "Statements::Repeat" => repeat(v, ptr, map),
        "Statements::Return" => {
            let mut parts = vec![Piece::flat(6)];
            if is_truthy(v, "expression") {
                parts.push(Piece::flat(2));
                parts.push(fmt(Some(v), "expression", ptr, map));
                parts.push(Piece::flat(1));
            }
            parts.push(Piece::flat(1));
            dispatched_len(&parts)
        }
        "Statements::Skip" => Piece::flat(5),
        "Expressions::AggregateInitializer" => {
            let items = fmt_each(v, "items", ptr, map);
            let mut len = 2usize;
            let mut nl = 0usize;
            if !items.is_empty() {
                let j = join_sep(&items, ", ");
                len += j.len;
                nl += j.nl;
            }
            Piece { len, nl }
        }
        "Expressions::AggregateInitializerItem" => concat(&[
            fmt(Some(v), "expression", ptr, map),
            Piece::flat(1),
            fmt(Some(v), "repetition", ptr, map),
        ]),
        "Expressions::BinaryExpression" => binary_expression(v, ptr, map),
        "Expressions::EntityConstructor" => {
            let mut parts = vec![fmt(Some(v), "entity", ptr, map), Piece::flat(1)];
            let params = fmt_each(v, "parameters", ptr, map);
            if !params.is_empty() {
                let j = join_sep(&params, ", ");
                parts.push(j);
            }
            parts.push(Piece::flat(1));
            dispatched_len(&parts)
        }
        "Expressions::FunctionCall" => {
            let mut parts = vec![fmt(Some(v), "function", ptr, map)];
            if is_truthy(v, "parameters") {
                let params = fmt_each(v, "parameters", ptr, map);
                parts.push(Piece::flat(1));
                parts.push(join_sep(&params, ", "));
                parts.push(Piece::flat(1));
            }
            dispatched_len(&parts)
        }
        "Expressions::Interval" => concat(&[
            Piece::flat(1),
            fmt(Some(v), "low", ptr, map),
            Piece::flat(interval_op(str_field(v, "operator1")).len() + 2),
            fmt(Some(v), "item", ptr, map),
            Piece::flat(interval_op(str_field(v, "operator2")).len() + 2),
            fmt(Some(v), "high", ptr, map),
            Piece::flat(1),
        ]),
        "Expressions::QueryExpression" => concat(&[
            Piece::flat(6),
            Piece::flat(str_field(v, "id").chars().count() + 4),
            fmt(Some(v), "aggregate_source", ptr, map),
            Piece::flat(3),
            fmt(Some(v), "expression", ptr, map),
            Piece::flat(1),
        ]),
        "Expressions::UnaryExpression" => unary_expression(v, ptr, map),
        "Literals::Binary" | "Literals::Integer" | "Literals::Real" => {
            Piece::flat(str_field(v, "value").chars().count())
        }
        "Literals::Logical" => Piece::flat(str_field(v, "value").chars().count()),
        // Raw-wrapped EXPRESS strings keep their embedded newlines in
        // the formatted render.
        "Literals::String" => Piece {
            len: str_field(v, "value").chars().count() + 2,
            nl: str_field(v, "value").matches('\n').count(),
        },
        "References::AttributeReference" => concat(&[
            fmt(Some(v), "ref", ptr, map),
            Piece::flat(1),
            fmt(Some(v), "attribute", ptr, map),
        ]),
        "References::GroupReference" => concat(&[
            fmt(Some(v), "ref", ptr, map),
            Piece::flat(1),
            fmt(Some(v), "entity", ptr, map),
        ]),
        "References::IndexReference" => {
            let mut parts = vec![
                fmt(Some(v), "ref", ptr, map),
                Piece::flat(1),
                fmt(Some(v), "index1", ptr, map),
            ];
            if is_truthy(v, "index2") {
                parts.push(Piece::flat(1));
                parts.push(fmt(Some(v), "index2", ptr, map));
            }
            parts.push(Piece::flat(1));
            dispatched_len(&parts)
        }
        "References::SimpleReference" => {
            if is_truthy(v, "base_path") {
                // Resolved-reference hyperlinks never occur on the
                // attach-time wire (references resolve after attachment).
                let id = str_field(v, "id");
                let base = str_field(v, "base_path");
                Piece::flat(7 + 9 + base.len() + 1 + id.len() + 3)
            } else {
                Piece::flat(str_field(v, "id").chars().count())
            }
        }
        "SupertypeExpressions::BinarySupertypeExpression" => {
            binary_supertype_expression(v, ptr, map)
        }
        "SupertypeExpressions::OneofSupertypeExpression" => {
            let operands = fmt_each(v, "operands", ptr, map);
            let mut len = 6usize;
            let mut nl = 0usize;
            if !operands.is_empty() {
                let j = join_sep(&operands, ", ");
                len += j.len;
                nl += j.nl;
            }
            len += 1;
            Piece { len, nl }
        }
        "DataTypes::Aggregate" => {
            let mut parts = vec![Piece::flat(9)];
            if is_truthy(v, "base_type") {
                parts.push(Piece::flat(4));
                parts.push(fmt(Some(v), "base_type", ptr, map));
            }
            dispatched_len(&parts)
        }
        "DataTypes::Array" => {
            let mut parts = vec![Piece::flat(5)];
            if is_truthy(v, "bound1") && is_truthy(v, "bound2") {
                let b = concat(&[
                    Piece::flat(2),
                    fmt(Some(v), "bound1", ptr, map),
                    Piece::flat(1),
                    fmt(Some(v), "bound2", ptr, map),
                    Piece::flat(1),
                ]);
                parts.push(b);
            }
            parts.push(Piece::flat(3));
            if is_truthy(v, "optional") {
                parts.push(Piece::flat(9));
            }
            if is_truthy(v, "unique") {
                parts.push(Piece::flat(7));
            }
            parts.push(Piece::flat(1));
            parts.push(fmt(Some(v), "base_type", ptr, map));
            dispatched_len(&parts)
        }
        "DataTypes::Bag" | "DataTypes::Set" => {
            let mut parts = vec![Piece::flat(3)];
            if is_truthy(v, "bound1") && is_truthy(v, "bound2") {
                let b = concat(&[
                    Piece::flat(2),
                    fmt(Some(v), "bound1", ptr, map),
                    Piece::flat(1),
                    fmt(Some(v), "bound2", ptr, map),
                    Piece::flat(1),
                ]);
                parts.push(b);
            }
            parts.push(Piece::flat(4));
            parts.push(fmt(Some(v), "base_type", ptr, map));
            dispatched_len(&parts)
        }
        "DataTypes::List" => {
            let mut parts = vec![Piece::flat(4)];
            if is_truthy(v, "bound1") && is_truthy(v, "bound2") {
                let b = concat(&[
                    Piece::flat(2),
                    fmt(Some(v), "bound1", ptr, map),
                    Piece::flat(1),
                    fmt(Some(v), "bound2", ptr, map),
                    Piece::flat(1),
                ]);
                parts.push(b);
            }
            parts.push(Piece::flat(3));
            if is_truthy(v, "unique") {
                parts.push(Piece::flat(7));
            }
            parts.push(Piece::flat(1));
            parts.push(fmt(Some(v), "base_type", ptr, map));
            dispatched_len(&parts)
        }
        "DataTypes::Binary" => {
            let mut parts = vec![Piece::flat(6)];
            if is_truthy(v, "width") {
                parts.push(Piece::flat(2));
                parts.push(fmt(Some(v), "width", ptr, map));
                parts.push(Piece::flat(1));
            }
            if is_truthy(v, "fixed") {
                parts.push(Piece::flat(6));
            }
            dispatched_len(&parts)
        }
        "DataTypes::Boolean" => Piece::flat(7),
        "DataTypes::Integer" => Piece::flat(7),
        "DataTypes::Logical" => Piece::flat(7),
        "DataTypes::Number" => Piece::flat(6),
        "DataTypes::Real" => {
            let mut parts = vec![Piece::flat(4)];
            if is_truthy(v, "precision") {
                parts.push(Piece::flat(2));
                parts.push(fmt(Some(v), "precision", ptr, map));
                parts.push(Piece::flat(1));
            }
            dispatched_len(&parts)
        }
        "DataTypes::String" => {
            let mut parts = vec![Piece::flat(6)];
            if is_truthy(v, "width") {
                parts.push(Piece::flat(2));
                parts.push(fmt(Some(v), "width", ptr, map));
                parts.push(Piece::flat(1));
            }
            if is_truthy(v, "fixed") {
                parts.push(Piece::flat(6));
            }
            dispatched_len(&parts)
        }
        "DataTypes::Generic" => {
            let mut len = 7usize;
            if is_truthy(v, "id") {
                len += 3 + str_field(v, "id").chars().count();
            }
            Piece::flat(len)
        }
        "DataTypes::GenericEntity" => {
            let mut len = 14usize;
            if is_truthy(v, "id") {
                len += 3 + str_field(v, "id").chars().count();
            }
            Piece::flat(len)
        }
        "DataTypes::Enumeration" => enumeration(v, ptr, map),
        "DataTypes::EnumerationItem" => Piece::flat(str_field(v, "id").chars().count()),
        "DataTypes::Select" => select(v, ptr, map),
        _ => Piece::default(),
    }
}

fn interval_op(op: &str) -> &'static str {
    match op {
        "LESS_THAN" => "<",
        _ => "<=",
    }
}

fn binary_op(op: &str) -> &'static str {
    match op {
        "ADDITION" => "+",
        "AND" => "AND",
        "COMBINE" => "||",
        "EQUAL" => "=",
        "EXPONENTIATION" => "**",
        "GREATER_THAN" => ">",
        "GREATER_THAN_OR_EQUAL" => ">=",
        "IN" => "IN",
        "INSTANCE_EQUAL" => "=:=",
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
        _ => "XOR",
    }
}

fn precedence(op: &str) -> u8 {
    match op {
        "EXPONENTIATION" => 1,
        "MULTIPLICATION" | "REAL_DIVISION" | "INTEGER_DIVISION" | "MODULO" | "AND" | "COMBINE" => 2,
        "SUBTRACTION" | "ADDITION" | "OR" | "XOR" => 3,
        _ => 4,
    }
}

fn binary_expression(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let op = str_field(v, "operator");
    let my_prec = precedence(op);
    let mut paren = |key: &str| -> Option<Piece> {
        let operand = child(v, key)?;
        if class_of(operand) == "Expressions::BinaryExpression"
            && precedence(str_field(operand, "operator")) >= my_prec
        {
            let inner = fmt(Some(v), key, ptr, map);
            Some(Piece {
                len: inner.len + 2,
                nl: inner.nl,
            })
        } else {
            Some(fmt(Some(v), key, ptr, map))
        }
    };
    let op1 = paren("operand1").unwrap_or_default();
    let op2 = paren("operand2").unwrap_or_default();
    let op_piece = Piece::flat(binary_op(op).len() + 2);
    concat(&[op1, op_piece, op2])
}

fn unary_expression(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let op = str_field(v, "operator");
    let kw_len = match op {
        "MINUS" => 1,
        "NOT" => 3,
        _ => 1,
    };
    let space = if op == "NOT" { 1 } else { 0 };
    let operand = child(v, "operand");
    let needs_parens = operand
        .map(|o| {
            matches!(
                class_of(o),
                "Expressions::BinaryExpression"
                    | "Expressions::Interval"
                    | "Expressions::UnaryExpression"
            )
        })
        .unwrap_or(false);
    let inner = fmt(Some(v), "operand", ptr, map);
    let mut len = kw_len + space + inner.len;
    if needs_parens {
        len += 2;
    }
    Piece { len, nl: inner.nl }
}

fn binary_supertype_expression(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let op = str_field(v, "operator");
    let my_prec: u8 = if op == "AND" { 1 } else { 2 };
    let kw = if op == "AND" { "AND" } else { "ANDOR" };
    let mut side = |key: &str| -> (bool, Piece) {
        let higher = child(v, key)
            .map(|o| {
                class_of(o) == "SupertypeExpressions::BinarySupertypeExpression"
                    && (if str_field(o, "operator") == "AND" {
                        1
                    } else {
                        2
                    }) > my_prec
            })
            .unwrap_or(false);
        (higher, fmt(Some(v), key, ptr, map))
    };
    let (p1, op1) = side("operand1");
    let (p2, op2) = side("operand2");
    let wrap = |paren: bool, p: Piece| {
        if paren {
            Piece {
                len: p.len + 2,
                nl: p.nl,
            }
        } else {
            p
        }
    };
    concat(&[wrap(p1, op1), Piece::flat(kw.len() + 2), wrap(p2, op2)])
}

/// `Schema#source` is the PLAIN SCHEMA HEAD (SchemaPlainSourceFormatter:
/// the declaration block without the body), while an enclosing ExpFile
/// render formats the whole schema. The index reads the head length; the
/// full render feeds the parent's length.
fn schema_head(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let mut head_parts = vec![Piece::flat(7 + str_field(v, "id").chars().count())];
    if is_truthy(v, "version") {
        let ver = fmt(Some(v), "version", ptr, map);
        head_parts.push(Piece::flat(1));
        head_parts.push(ver);
    }
    head_parts.push(Piece::flat(1));
    let mut head = concat(&head_parts);
    if positive(v, "interfaces") {
        let ifcs = fmt_each(v, "interfaces", ptr, map);
        head = join_nl(&[head, Piece::default(), join_sep(&ifcs, "\n\n")]);
    }
    head
}

fn schema(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let head = schema_head(v, ptr, map);

    let mut decls: Vec<Piece> = Vec::new();
    if positive(v, "constants") {
        let cs = fmt_each(v, "constants", ptr, map);
        decls.push(join_nl(&[
            Piece::flat(8),
            indent(join_nl(&cs)),
            Piece::flat(13),
        ]));
    }
    for key in [
        "types",
        "entities",
        "subtype_constraints",
        "functions",
        "rules",
        "procedures",
    ] {
        for p in fmt_each(v, key, ptr, map) {
            decls.push(p);
        }
    }

    let mut parts = vec![head];
    if !decls.is_empty() {
        parts.push(Piece::default());
        parts.push(join_sep(&decls, "\n\n"));
        parts.push(Piece::default());
    }
    parts.push(Piece::flat(11));
    let full = join_nl(&parts);
    // The index-visible length is the head-only render.
    let head_len = schema_head(v, ptr, map);
    map.insert(ptr.to_string(), head_len);
    full
}
fn attribute(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let mut parts: Vec<Piece> = Vec::new();
    if is_truthy(v, "supertype_attribute") {
        let st = fmt(Some(v), "supertype_attribute", ptr, map);
        parts.push(Piece {
            len: st.len + 1,
            nl: st.nl,
        });
        let st_attr_id = child(v, "supertype_attribute")
            .and_then(|s| child(s, "attribute"))
            .and_then(|a| child(a, "id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let id = str_field(v, "id");
        if id != st_attr_id {
            parts.push(Piece::flat(8 + id.len() + 1));
        }
    } else if is_truthy(v, "id") {
        parts.push(Piece::flat(str_field(v, "id").chars().count() + 1));
    }
    parts.push(Piece::flat(1));
    if is_truthy(v, "optional") {
        parts.push(Piece::flat(9));
    }
    parts.push(Piece::flat(1));
    parts.push(fmt(Some(v), "type", ptr, map));
    match str_field(v, "kind") {
        "DERIVED" => {
            parts.push(Piece::flat(4));
            parts.push(fmt(Some(v), "expression", ptr, map));
        }
        "INVERSE" => {
            parts.push(Piece::flat(5));
            parts.push(fmt(Some(v), "expression", ptr, map));
        }
        _ => {}
    }
    parts.push(Piece::flat(1));
    dispatched_len(&parts)
}

fn entity(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let mut header = vec![Piece::flat(7 + str_field(v, "id").chars().count())];
    let abstract_flag = is_truthy(v, "abstract");
    let has_supertype_expr = is_truthy(v, "supertype_expression");
    if abstract_flag && !has_supertype_expr {
        header.push(Piece {
            len: 1 + 2 + 18,
            nl: 1,
        });
    } else if abstract_flag && has_supertype_expr {
        let expr = fmt(Some(v), "supertype_expression", ptr, map);
        header.push(concat(&[
            Piece {
                len: 1 + 2 + 23,
                nl: 1,
            },
            expr,
            Piece::flat(1),
        ]));
    } else if has_supertype_expr {
        let expr = fmt(Some(v), "supertype_expression", ptr, map);
        header.push(concat(&[
            Piece {
                len: 1 + 2 + 14,
                nl: 1,
            },
            expr,
            Piece::flat(1),
        ]));
    }
    if positive(v, "subtype_of") {
        let subs = fmt_each(v, "subtype_of", ptr, map);
        let j = join_sep(&subs, ", ");
        header.push(concat(&[
            Piece {
                len: 1 + 2 + 12,
                nl: 1,
            },
            j,
            Piece::flat(1),
        ]));
    }
    header.push(Piece::flat(1));
    let header = concat(&header);

    let mut attrs: Vec<(&Value, String)> = Vec::new();
    for (i, a) in arr(v, "attributes").into_iter().enumerate() {
        attrs.push((a, format!("{ptr}/attributes/{i}")));
    }
    for (i, a) in arr(v, "derived_attributes").into_iter().enumerate() {
        attrs.push((a, format!("{ptr}/derived_attributes/{i}")));
    }
    for (i, a) in arr(v, "inverse_attributes").into_iter().enumerate() {
        attrs.push((a, format!("{ptr}/inverse_attributes/{i}")));
    }
    let mut explicit: Vec<Piece> = Vec::new();
    let mut derived: Vec<Piece> = Vec::new();
    let mut inverse: Vec<Piece> = Vec::new();
    for (a, aptr) in &attrs {
        let p = class_len(a, aptr, map);
        match str_field(a, "kind") {
            "DERIVED" => derived.push(p),
            "INVERSE" => inverse.push(p),
            _ => explicit.push(p),
        }
    }

    let mut parts = vec![header];
    if !explicit.is_empty() {
        parts.push(indent(join_nl(&explicit)));
    }
    if !derived.is_empty() {
        parts.push(join_nl(&[Piece::flat(6), indent(join_nl(&derived))]));
    }
    if !inverse.is_empty() {
        parts.push(join_nl(&[Piece::flat(7), indent(join_nl(&inverse))]));
    }
    if positive(v, "unique_rules") {
        let urs = fmt_each(v, "unique_rules", ptr, map);
        parts.push(join_nl(&[Piece::flat(6), indent(join_nl(&urs))]));
    }
    if positive(v, "where_rules") {
        let wrs = fmt_each(v, "where_rules", ptr, map);
        parts.push(join_nl(&[Piece::flat(5), indent(join_nl(&wrs))]));
    }
    parts.push(Piece::flat(11));
    join_nl(&parts)
}

fn scope_blocks(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Vec<Piece> {
    let mut parts: Vec<Piece> = Vec::new();
    for key in [
        "types",
        "entities",
        "subtype_constraints",
        "functions",
        "procedures",
    ] {
        if positive(v, key) {
            let items = fmt_each(v, key, ptr, map);
            parts.push(indent(join_nl(&items)));
        }
    }
    if positive(v, "constants") {
        let cs = fmt_each(v, "constants", ptr, map);
        parts.push(indent(join_nl(&[
            Piece::flat(8),
            indent(join_nl(&cs)),
            Piece::flat(13),
        ])));
    }
    if positive(v, "variables") {
        let vs = fmt_each(v, "variables", ptr, map);
        parts.push(indent(join_nl(&[
            Piece::flat(5),
            indent(join_nl(&vs)),
            Piece::flat(10),
        ])));
    }
    parts
}

fn function(
    v: &Value,
    ptr: &str,
    map: &mut HashMap<String, Piece>,
    kw: &str,
    end_kw: &str,
) -> Piece {
    let id = str_field(v, "id");
    let mut header_parts = vec![Piece::flat(kw.len() + 1 + id.len())];
    if is_truthy(v, "parameters") {
        let param_indent = " ".repeat(kw.len() + 1 + id.len() + 1);
        let params = fmt_each(v, "parameters", ptr, map);
        let joined = join_sep(&params, &format!(";\n{param_indent}"));
        header_parts.push(concat(&[Piece::flat(1), joined, Piece::flat(1)]));
    }
    if kw == "FUNCTION" {
        header_parts.push(Piece::flat(3));
        header_parts.push(fmt(Some(v), "return_type", ptr, map));
    }
    header_parts.push(Piece::flat(1));
    let header = concat(&header_parts);

    let mut parts = vec![header];
    parts.extend(scope_blocks(v, ptr, map));
    if positive(v, "statements") {
        let stmts = fmt_each(v, "statements", ptr, map);
        parts.push(indent(join_nl(&stmts)));
    }
    parts.push(Piece::flat(end_kw.len() + 1));
    join_nl(&parts)
}

fn rule(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let applies = fmt_each(v, "applies_to", ptr, map);
    let mut len = 4 + 1 + str_field(v, "id").chars().count() + 1 + 3 + 1 + 1;
    let mut nl = 0usize;
    if !applies.is_empty() {
        let j = join_sep(&applies, ", ");
        len += j.len;
        nl += j.nl;
    }
    len += 2;
    let header = Piece { len, nl };

    let mut kept: Vec<Piece> = Vec::new();
    let stmts: Vec<(&Value, String)> = arr(v, "statements")
        .into_iter()
        .enumerate()
        .map(|(i, s)| (s, format!("{ptr}/statements/{i}")))
        .collect();
    for (s, sptr) in &stmts {
        let cls = class_of(s);
        let drop = if cls == "Statements::Alias" || cls == "Statements::Repeat" {
            match child(s, "statements") {
                Some(Value::Array(items)) => {
                    items.iter().all(|x| class_of(x) == "Statements::Null")
                }
                _ => false,
            }
        } else {
            cls == "Statements::Null"
        };
        // Dropped statements still carry their own formatted source —
        // the index sees every registry-walked node.
        let piece = class_len(s, sptr, map);
        if !drop {
            kept.push(piece);
        }
    }

    let mut parts = vec![header];
    parts.extend(scope_blocks(v, ptr, map));
    if !kept.is_empty() {
        parts.push(indent(join_nl(&kept)));
    }
    if positive(v, "where_rules") {
        let wrs = fmt_each(v, "where_rules", ptr, map);
        parts.push(join_nl(&[Piece::flat(5), indent(join_nl(&wrs))]));
    }
    parts.push(Piece::flat(9));
    join_nl(&parts)
}

fn interface(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let kw_len = match str_field(v, "kind") {
        "USE" => 3,
        _ => 9,
    };
    let mut parts = vec![Piece::flat(kw_len + 6), fmt(Some(v), "schema", ptr, map)];
    if positive(v, "items") {
        let items = fmt_each(v, "items", ptr, map);
        let inner = concat(&[Piece::flat(1), join_sep(&items, ",\n "), Piece::flat(1)]);
        parts.push(Piece {
            len: 1 + indent(inner).len,
            nl: 1 + indent(inner).nl,
        });
    }
    parts.push(Piece::flat(1));
    dispatched_len(&parts)
}

fn subtype_constraint(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let applies = fmt(Some(v), "applies_to", ptr, map);
    let header = concat(&[
        Piece::flat(19 + str_field(v, "id").chars().count() + 5),
        applies,
        Piece::flat(1),
    ]);
    let mut parts = vec![header];
    if is_truthy(v, "abstract") {
        parts.push(indent(Piece::flat(19)));
    }
    if positive(v, "total_over") {
        let tos = fmt_each(v, "total_over", ptr, map);
        let inner = concat(&[
            Piece::flat(10),
            Piece::flat(1),
            join_sep(&tos, ", "),
            Piece::flat(2),
        ]);
        parts.push(indent(inner));
    }
    if is_truthy(v, "supertype_expression") {
        let expr = fmt(Some(v), "supertype_expression", ptr, map);
        parts.push(indent(concat(&[expr, Piece::flat(1)])));
    }
    parts.push(Piece::flat(23));
    join_nl(&parts)
}

fn type_decl(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let header = concat(&[
        Piece::flat(5 + str_field(v, "id").chars().count() + 3),
        fmt(Some(v), "underlying_type", ptr, map),
        Piece::flat(1),
    ]);
    let mut parts = vec![header];
    if positive(v, "where_rules") {
        let wrs = fmt_each(v, "where_rules", ptr, map);
        parts.push(join_nl(&[Piece::flat(5), indent(join_nl(&wrs))]));
    }
    parts.push(Piece::flat(9));
    join_nl(&parts)
}

fn enumeration(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let mut parts: Vec<Piece> = Vec::new();
    if is_truthy(v, "extensible") {
        parts.push(Piece::flat(11));
    }
    parts.push(Piece::flat(11));
    if is_truthy(v, "based_on") {
        parts.push(Piece::flat(10));
        parts.push(fmt(Some(v), "based_on", ptr, map));
        if positive(v, "items") {
            let items = fmt_each(v, "items", ptr, map);
            let inner = concat(&[Piece::flat(1), join_sep(&items, ",\n "), Piece::flat(1)]);
            parts.push(concat(&[Piece { len: 6, nl: 1 }, indent(inner)]));
        }
    } else if positive(v, "items") {
        let items = fmt_each(v, "items", ptr, map);
        let inner = concat(&[Piece::flat(1), join_sep(&items, ",\n "), Piece::flat(1)]);
        parts.push(concat(&[Piece { len: 4, nl: 1 }, indent(inner)]));
    }
    dispatched_len(&parts)
}

fn select(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let mut parts: Vec<Piece> = Vec::new();
    if is_truthy(v, "extensible") {
        parts.push(Piece::flat(11));
    }
    if is_truthy(v, "generic_entity") {
        parts.push(Piece::flat(15));
    }
    parts.push(Piece::flat(6));
    let items_positive = positive(v, "items");
    if is_truthy(v, "based_on") {
        parts.push(Piece::flat(10));
        parts.push(fmt(Some(v), "based_on", ptr, map));
        if items_positive {
            let items = fmt_each(v, "items", ptr, map);
            let inner = concat(&[Piece::flat(1), join_sep(&items, ",\n "), Piece::flat(1)]);
            parts.push(concat(&[Piece { len: 6, nl: 1 }, indent(inner)]));
        }
    } else if items_positive {
        let items = fmt_each(v, "items", ptr, map);
        let inner = concat(&[Piece::flat(1), join_sep(&items, ",\n "), Piece::flat(1)]);
        let ind = indent(inner);
        parts.push(Piece {
            len: 1 + ind.len,
            nl: 1 + ind.nl,
        });
    }
    dispatched_len(&parts)
}

fn case(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let mut parts = vec![concat(&[
        Piece::flat(5),
        fmt(Some(v), "expression", ptr, map),
        Piece::flat(3),
    ])];
    if positive(v, "actions") {
        for p in fmt_each(v, "actions", ptr, map) {
            parts.push(p);
        }
    }
    if is_truthy(v, "otherwise_statement") {
        let stmt = fmt(Some(v), "otherwise_statement", ptr, map);
        parts.push(join_nl(&[Piece::flat(11), indent(stmt)]));
    }
    parts.push(Piece::flat(9));
    join_nl(&parts)
}

fn if_stmt(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let has_else = positive(v, "else_statements");
    let mut parts = vec![concat(&[
        Piece::flat(3),
        fmt(Some(v), "expression", ptr, map),
        Piece::flat(5),
    ])];
    if positive(v, "statements") {
        let stmts = fmt_each(v, "statements", ptr, map);
        parts.push(indent(join_nl(&stmts)));
    }
    if has_else {
        let else_stmts = fmt_each(v, "else_statements", ptr, map);
        parts.push(Piece::flat(4));
        parts.push(indent(join_nl(&else_stmts)));
    }
    parts.push(Piece::flat(7));
    join_nl(&parts)
}

fn repeat(v: &Value, ptr: &str, map: &mut HashMap<String, Piece>) -> Piece {
    let mut header = vec![Piece::flat(6)];
    if is_truthy(v, "id") {
        let id = str_field(v, "id");
        let mut seg = vec![Piece::flat(5 + id.len())];
        seg.push(fmt(Some(v), "bound1", ptr, map));
        seg.push(Piece::flat(4));
        seg.push(fmt(Some(v), "bound2", ptr, map));
        if is_truthy(v, "increment") {
            seg.push(Piece::flat(4));
            seg.push(fmt(Some(v), "increment", ptr, map));
        }
        header.push(concat(&seg));
    }
    if is_truthy(v, "while_expression") {
        header.push(concat(&[
            Piece::flat(7),
            fmt(Some(v), "while_expression", ptr, map),
        ]));
    }
    if is_truthy(v, "until_expression") {
        header.push(concat(&[
            Piece::flat(7),
            fmt(Some(v), "until_expression", ptr, map),
        ]));
    }
    header.push(Piece::flat(1));
    let mut parts = vec![concat(&header)];
    if positive(v, "statements") {
        let stmts = fmt_each(v, "statements", ptr, map);
        parts.push(indent(join_nl(&stmts)));
    }
    parts.push(Piece::flat(11));
    join_nl(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn len_of(v: Value) -> usize {
        lengths_by_ptr(&v).get("").map(|p| p.len).unwrap_or(0)
    }

    #[test]
    fn assignment_length() {
        let v = json!({
            "_class": "Expressir::Model::Statements::Assignment",
            "ref": {"_class": "Expressir::Model::References::SimpleReference", "id": "total"},
            "expression": {"_class": "Expressir::Model::Literals::Integer", "value": "4"},
        });
        assert_eq!(len_of(v), "total := 4;".len());
    }

    #[test]
    fn repeat_full_construct_length() {
        let v = json!({
            "_class": "Expressir::Model::Statements::Repeat",
            "id": "i",
            "bound1": {"_class": "Expressir::Model::Literals::Integer", "value": "1"},
            "bound2": {"_class": "Expressir::Model::References::SimpleReference", "id": "n"},
            "statements": [
                {"_class": "Expressir::Model::Statements::Assignment",
                 "ref": {"_class": "Expressir::Model::References::SimpleReference", "id": "t"},
                 "expression": {"_class": "Expressir::Model::References::SimpleReference", "id": "i"}},
            ],
        });
        assert_eq!(
            len_of(v),
            "REPEAT i := 1 TO n;\n  t := i;\nEND_REPEAT;".len()
        );
    }

    #[test]
    fn if_with_else_length() {
        let v = json!({
            "_class": "Expressir::Model::Statements::If",
            "expression": {"_class": "Expressir::Model::References::SimpleReference", "id": "x"},
            "statements": [
                {"_class": "Expressir::Model::Statements::Assignment",
                 "ref": {"_class": "Expressir::Model::References::SimpleReference", "id": "t"},
                 "expression": {"_class": "Expressir::Model::Literals::Integer", "value": "1"}},
            ],
            "else_statements": [
                {"_class": "Expressir::Model::Statements::Assignment",
                 "ref": {"_class": "Expressir::Model::References::SimpleReference", "id": "t"},
                 "expression": {"_class": "Expressir::Model::Literals::Integer", "value": "0"}},
            ],
        });
        assert_eq!(
            len_of(v),
            "IF x THEN\n  t := 1;\nELSE\n  t := 0;\nEND_IF;".len()
        );
    }
}
