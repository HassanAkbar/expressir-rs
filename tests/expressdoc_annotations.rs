//! EXPRESSdoc annotations (metanorma/annotated-express#94,
//! expressir#91): with annotations mode on, `---` and `(**` units scan
//! as annotations and render with their own markers. With the mode off
//! (the default), the same sources scan as plain remarks and nothing
//! changes for existing corpora.

use expressir_rs::attach::{attach_tagged_remarks, attach_untagged_remarks};
use expressir_rs::formatter::format;
use expressir_rs::ParsedTree;

#[test]
fn annotations_scan_and_render_with_their_own_markers() {
    let source = concat!(
        "SCHEMA demo;\n",
        "  ENTITY thing;\n",
        "    (** an inline annotation *)\n",
        "    name : STRING;\n",
        "  END_ENTITY;\n",
        "  --- a schema annotation\n",
        "END_SCHEMA;\n",
    );
    let units = expressir_rs::remark::scan_with(source, true);
    let formats: Vec<&str> = units.iter().map(|u| u.format).collect();
    assert!(
        formats.contains(&"embedded_annotation"),
        "formats: {formats:?}"
    );
    assert!(formats.contains(&"tail_annotation"), "formats: {formats:?}");

    let tree = ParsedTree::parse(source).expect("parses");
    let mut wire = tree.to_model_json("demo.exp").expect("wire");
    attach_tagged_remarks(&mut wire, source, true);
    attach_untagged_remarks(&mut wire, source, true);

    // The annotation format reaches the wire's RemarkInfo entries.
    let rendered = serde_json::to_string(&wire).expect("wire json");
    assert!(rendered.contains("embedded_annotation"), "wire: {rendered}");

    // The formatter emits the annotation marker for the inline form.
    let out = format(&wire, false);
    assert!(out.contains("(** an inline annotation *)"), "out: {out}");
    assert!(!out.contains("(* an inline annotation *)"), "out: {out}");
}

#[test]
fn annotations_mode_off_keeps_plain_remark_reading() {
    // The corpus-safety property: with the mode off, a three-dash line
    // still scans as a tail remark and renders as one.
    let source = "SCHEMA demo;\n  ENTITY thing;\n    name : STRING;\n  END_ENTITY;\nEND_SCHEMA;\n--- a plain remark\n";
    let units = expressir_rs::remark::scan(source);
    assert!(units
        .iter()
        .all(|u| u.format == "tail" || u.format == "embedded"));

    let tree = ParsedTree::parse(source).expect("parses");
    let mut wire = tree.to_model_json("demo.exp").expect("wire");
    attach_tagged_remarks(&mut wire, source, false);
    attach_untagged_remarks(&mut wire, source, false);
    let rendered = serde_json::to_string(&wire).expect("wire json");
    assert!(rendered.contains("\"format\":\"tail\""), "wire: {rendered}");
    assert!(!rendered.contains("annotation"));
}
