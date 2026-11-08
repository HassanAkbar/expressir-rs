//! expressir#609: absolute remark-reference tags inside a scope.
//!
//! `(*"s.f.lv"` sitting INSIDE function f must create the item with the
//! tag's leaf id (`lv`). The attacher's S3 used to prepend the containing
//! scope's path to the tag, double-counting an already-absolute tag, and
//! the implicit walk then named the item after the first unresolved
//! segment — the schema.

use serde_json::Value;

#[test]
fn absolute_tag_inside_a_scope_takes_the_leaf_id() {
    let source = "SCHEMA demo_schema;\nFUNCTION f : INTEGER;\n(*\"demo_schema.f.lv\"\n\nthe lv remark\n\n*)\nRETURN(1);\nEND_FUNCTION;\nEND_SCHEMA;\n";
    let wire = expressir_rs::ParsedTree::parse(source)
        .expect("parses")
        .to_model_json("demo_schema.exp")
        .expect("wire");
    let mut attached = wire.clone();
    expressir_rs::attach::attach_tagged_remarks(&mut attached, source, false);
    expressir_rs::attach::attach_untagged_remarks(&mut attached, source, false);

    let item = attached
        .pointer("/schemas/0/functions/0/remark_items/0")
        .expect("item created under the function");
    assert_eq!(item.get("id").and_then(Value::as_str), Some("lv"));
    assert_eq!(
        item.get("remarks").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );
}
