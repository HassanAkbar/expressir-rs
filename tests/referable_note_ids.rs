//! annotated-express#103: a referable `__note:id` tag keeps its id
//! through attach, and a where-rule note formats with a tag that
//! reattaches to the same rule.

use serde_json::Value;

fn attached(source: &str) -> Value {
    let mut wire = expressir_rs::ParsedTree::parse(source)
        .expect("parses")
        .to_model_json("s.exp")
        .expect("wire");
    expressir_rs::attach::attach_tagged_remarks(&mut wire, source, false);
    expressir_rs::attach::attach_untagged_remarks(&mut wire, source, false);
    wire
}

fn item_ids(wire: &Value, pointer: &str) -> Vec<String> {
    wire.pointer(pointer)
        .and_then(|n| n.get("remark_items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.get("id").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

const WHERE_NOTE: &str = "SCHEMA s;\nENTITY w;\n  size : INTEGER;\nWHERE\n  WR1: size > 0;\nEND_ENTITY;\nEND_SCHEMA;\n(*\"s.w.wr:WR1.__note:n1\"\nrule note\n*)\n";

#[test]
fn where_rule_note_formats_with_a_tag_that_reattaches_to_the_rule() {
    let wire = attached(WHERE_NOTE);
    let formatted = expressir_rs::formatter::format(&wire, false);
    assert!(
        formatted.contains("--\"s.w.WR1.__note:n1\" rule note"),
        "{formatted}"
    );

    let reparsed = attached(&formatted);
    assert_eq!(
        item_ids(&reparsed, "/schemas/0/entities/0/where_rules/0"),
        ["__note:n1"]
    );
}
