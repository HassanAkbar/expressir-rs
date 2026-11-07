//! Wire-shape quirk tests: every observable Ruby builder behavior the
//! emitter mirrors deliberately. These pin the contract the expressir
//! parity spec (`parser_core_parity_spec`) holds across both parse
//! paths.

use expressir_rs::ParsedTree;
use serde_json::Value;

fn wire(source: &str) -> Value {
    ParsedTree::parse(source)
        .expect("parse")
        .to_model_json("test.exp")
        .expect("model json")
}

fn schema(v: &Value) -> &Value {
    &v["schemas"][0]
}

#[test]
fn schema_offset_points_at_the_schema_keyword() {
    let prefix = "\n  "; // leading whitespace must be skipped
    let v = wire(&format!("{prefix}SCHEMA s; END_SCHEMA;"));
    assert_eq!(schema(&v)["source_offset"], prefix.len());
}

#[test]
fn where_rule_offset_resolves_to_the_trailing_delimiter() {
    // The Ruby dispatch attaches where rules with the raw wrapper
    // element; with the depth-capped find_slice that resolves to the
    // op_delim's ";", not the expression.
    let src = "SCHEMA s;\nENTITY e; WHERE TRUE; END_ENTITY;\nEND_SCHEMA;\n";
    let v = wire(src);
    let rule = &schema(&v)["entities"][0]["where_rules"][0];
    assert!(rule.get("id").is_none());
    let offset = rule["source_offset"].as_u64().expect("offset") as usize;
    assert_eq!(&src[offset..offset + 1], ";");
}

#[test]
fn schema_version_items_split_like_the_ruby_builder() {
    let src = "SCHEMA s '{iso standard 10303 part(59) version(8)}';\nEND_SCHEMA;\n";
    let v = wire(src);
    let version = &schema(&v)["version"];
    assert_eq!(version["value"], "{iso standard 10303 part(59) version(8)}");
    let items = version["items"].as_array().expect("items");
    fn fields(item: &Value) -> (Option<&str>, Option<&str>) {
        (item["name"].as_str(), item["value"].as_str())
    }
    assert_eq!(fields(&items[0]), (Some("iso"), None));
    assert_eq!(fields(&items[1]), (Some("standard"), None));
    assert_eq!(fields(&items[2]), (None, Some("10303")));
    assert_eq!(fields(&items[3]), (Some("part"), Some("59")));
    assert_eq!(fields(&items[4]), (Some("version"), Some("8")));
}

#[test]
fn single_element_aggregate_drops_repetition() {
    // [4:2] — a single occurrence merges into the holder and dispatches
    // on its first key: bare expression, no AggregateInitializerItem.
    let src = "SCHEMA s;\nFUNCTION f : INTEGER;\n  LOCAL x : INTEGER; END_LOCAL;\n  x := [4:2];\nEND_FUNCTION;\nEND_SCHEMA;\n";
    let v = wire(src);
    let stmt = &schema(&v)["functions"][0]["statements"][0];
    let item = &stmt["expression"]["items"][0];
    assert_eq!(item["_class"], "Expressir::Model::Literals::Integer");
    assert!(item.get("repetition").is_none());
}

#[test]
fn multi_element_aggregate_keeps_repetition() {
    // [1:2, 3:4] — multi-occurrence elements are rule-key wrapped and
    // route through build_element, keeping the repetition.
    let src = "SCHEMA s;\nFUNCTION f : INTEGER;\n  LOCAL x : INTEGER; END_LOCAL;\n  x := [1:2, 3:4];\nEND_FUNCTION;\nEND_SCHEMA;\n";
    let v = wire(src);
    let stmt = &schema(&v)["functions"][0]["statements"][0];
    for item in stmt["expression"]["items"].as_array().expect("items") {
        assert_eq!(
            item["_class"],
            "Expressir::Model::Expressions::AggregateInitializerItem"
        );
        assert!(item.get("repetition").is_some());
    }
}

#[test]
fn subtype_constraint_always_renders_abstract() {
    let src = "SCHEMA s;\nENTITY e; END_ENTITY;\nSUBTYPE_CONSTRAINT c FOR e; END_SUBTYPE_CONSTRAINT;\nEND_SCHEMA;\n";
    let v = wire(src);
    assert_eq!(schema(&v)["subtype_constraints"][0]["abstract"], false);
}

#[test]
fn multi_ref_subtype_of_collects_all_refs() {
    // SUBTYPE OF (a, b) arrives as fragments merged with the op_comma
    // token; every entityRef still lands in subtype_of (GH-341).
    let src = "SCHEMA s;\nENTITY a; END_ENTITY;\nENTITY b; END_ENTITY;\nENTITY c SUBTYPE OF (a, b); END_ENTITY;\nEND_SCHEMA;\n";
    let v = wire(src);
    let c = &schema(&v)["entities"][2];
    assert_eq!(c["id"], "c");
    assert_eq!(c["subtype_of"][0]["id"], "a");
    assert_eq!(c["subtype_of"][1]["id"], "b");
}

#[test]
fn base_path_is_never_emitted() {
    // The Ruby reference resolver owns base_path after hydration.
    let src =
        "SCHEMA s;\nENTITY a; END_ENTITY;\nENTITY b SUBTYPE OF (a); END_ENTITY;\nEND_SCHEMA;\n";
    let v = wire(src);
    let b = &schema(&v)["entities"][1];
    assert_eq!(b["subtype_of"][0]["id"], "a");
    assert!(b["subtype_of"][0].get("base_path").is_none());
}

#[test]
fn interfaces_always_carry_an_items_array() {
    let src = "SCHEMA s;\nREFERENCE FROM other;\nEND_SCHEMA;\n";
    let v = wire(src);
    let interface = &schema(&v)["interfaces"][0];
    assert_eq!(interface["kind"], "REFERENCE");
    assert_eq!(interface["schema"]["id"], "other");
    assert_eq!(interface["items"].as_array().map(Vec::len), Some(0));
}

#[test]
fn general_aggregations_carry_element_modifiers() {
    // Attribute types route through the general aggregation types;
    // OPTIONAL/UNIQUE element modifiers must survive (GH-338).
    let src = "SCHEMA s;\nENTITY thing; END_ENTITY;\nENTITY holder;\nitems : LIST [1:?] OF UNIQUE thing;\narr : ARRAY [1:3] OF OPTIONAL thing;\nEND_ENTITY;\nEND_SCHEMA;\n";
    let v = wire(src);
    let attrs = &schema(&v)["entities"][1]["attributes"];
    assert_eq!(attrs[0]["id"], "items");
    assert_eq!(attrs[0]["type"]["unique"], true);
    assert_eq!(attrs[1]["id"], "arr");
    assert_eq!(attrs[1]["type"]["optional"], true);
}

#[test]
fn unique_rules_drop_phantom_fragments() {
    // The separator repetition can yield a label-less, attribute-less
    // fragment between real rules; it must not surface (GH-340).
    let src = "SCHEMA s;\nENTITY thing;\na : STRING;\nb : STRING;\nUNIQUE\nUR1: a;\nUR2: b;\nEND_ENTITY;\nEND_SCHEMA;\n";
    let v = wire(src);
    let u = &schema(&v)["entities"][0]["unique_rules"];
    assert_eq!(u.as_array().map(Vec::len), Some(2));
    assert_eq!(u[0]["id"], "UR1");
    assert_eq!(u[1]["id"], "UR2");
}

#[test]
fn empty_parameter_lists_wire_null_versus_empty() {
    // syntax 253/316: `f()` (explicit empty parens) wires [] while a
    // declaration without parentheses wires null — the Ruby side
    // round trips the distinction (expressir#558).
    let src = "SCHEMA s;\nFUNCTION f() : INTEGER;\nRETURN (1);\nEND_FUNCTION;\nFUNCTION g : INTEGER;\nRETURN (1);\nEND_FUNCTION;\nEND_SCHEMA;\n";
    let v = wire(src);
    let functions = &schema(&v)["functions"];
    assert_eq!(functions[0]["parameters"], serde_json::json!([]));
    assert_eq!(functions[1]["parameters"], serde_json::json!(null));
}

#[test]
fn generic_types_carry_their_labels() {
    // GENERIC : label / GENERIC_ENTITY : label — ISO 10303-11 9.5.3.2
    // (GH-339).
    let src = "SCHEMA s;\nFUNCTION pick(c1 : GENERIC : item) : GENERIC_ENTITY : item;\nRETURN (c1);\nEND_FUNCTION;\nEND_SCHEMA;\n";
    let v = wire(src);
    let params = &schema(&v)["functions"][0]["parameters"];
    assert_eq!(params[0]["type"]["id"], "item");
    let ret = &schema(&v)["functions"][0]["return_type"];
    assert_eq!(ret["id"], "item");
}

#[test]
fn total_over_reads_the_list_of_entity_ref_level() {
    // The Ruby builder read totalOver.entityRef directly, missing the
    // listOf_entityRef level, so real lists never surfaced; the wire
    // emitter mirrored the bug until GH-377 fixed the Ruby side. Pin the
    // fixed shape on both fragments and single-holder forms.
    let src = "SCHEMA s;\nENTITY e; END_ENTITY;\nENTITY f SUBTYPE OF (e); END_ENTITY;\n\
               SUBTYPE_CONSTRAINT c FOR e; TOTAL_OVER (e, f); END_SUBTYPE_CONSTRAINT;\n\
               END_SCHEMA;\n";
    let v = wire(src);
    let total_over = &schema(&v)["subtype_constraints"][0]["total_over"];
    let ids: Vec<&str> = total_over
        .as_array()
        .expect("total_over array")
        .iter()
        .map(|r| r["id"].as_str().expect("ref id"))
        .collect();
    assert_eq!(ids, ["e", "f"]);
}

#[test]
fn factor_keeps_the_exponentiation_operand() {
    // factor = simpleFactor [ '**' simpleFactor ] — the rhs capture must
    // become an EXPONENTIATION BinaryExpression, matching the Ruby
    // build_factor (lutaml/expressir-rs#9).
    let v = wire("SCHEMA s; CONSTANT c : BOOLEAN := 4 ** 2; END_CONSTANT; END_SCHEMA;\n");
    let expr = &schema(&v)["constants"][0]["expression"];
    assert_eq!(
        expr["_class"],
        "Expressir::Model::Expressions::BinaryExpression"
    );
    assert_eq!(expr["operator"], "EXPONENTIATION");
    assert_eq!(expr["operand1"]["value"], "4");
    assert_eq!(expr["operand2"]["value"], "2");
}

#[test]
fn bare_in_rhs_parses() {
    // 2.4.56 regression (expressir#598): `s IN t` — a bare identifier as
    // the IN right-hand side — failed at the whitespace after IN, while
    // 2.4.48 parsed it as a normal IN relation.
    let src = "SCHEMA explicit_constraint_schema;\n\
ENTITY e;\n\
WHERE\n\
  wr1: s IN t;\n\
END_ENTITY;\n\
END_SCHEMA;\n";
    let v = wire(src);
    let wr = &schema(&v)["entities"][0]["where_rules"][0];
    let expr = &wr["expression"];
    assert_eq!(
        expr["_class"],
        "Expressir::Model::Expressions::BinaryExpression"
    );
    assert_eq!(expr["operator"], "IN");
}

#[test]
fn paren_in_rhs_stays_an_in_relation() {
    // Same regression, parenthesised form: 2.4.56 parsed `s IN (t)` as a
    // call instead of an IN relation.
    let src = "SCHEMA s2;\n\
ENTITY e;\n\
WHERE\n\
  wr1: s IN (t);\n\
END_ENTITY;\n\
END_SCHEMA;\n";
    let v = wire(src);
    let wr = &schema(&v)["entities"][0]["where_rules"][0];
    let expr = &wr["expression"];
    assert_eq!(
        expr["_class"],
        "Expressir::Model::Expressions::BinaryExpression"
    );
    assert_eq!(expr["operator"], "IN");
}
