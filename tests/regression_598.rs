//! Issue expressir#598: an identifier named `s` immediately before the
//! `IN` operator broke the core parser — trivia atoms injected between
//! the letters of byte-range keywords let `tSIN` match `"s IN"` across
//! the whitespace, so the identifier was consumed as the keyword SIN.
//! `s IN t` then failed outright and `s IN (t)` became a function
//! call. The parsanol lane (`from_exp`) never regressed; only the
//! embedded-grammar lane did.

fn wire_for(expr: &str) -> serde_json::Value {
    let src = format!(
        "SCHEMA y_1;\nFUNCTION f(s : INTEGER; sv : INTEGER; a : INTEGER; c : REAL; t : SET OF INTEGER) : GENERIC;\n  RETURN ({});\nEND_FUNCTION;\nEND_SCHEMA;\n",
        expr
    );
    let tree = expressir_rs::ParsedTree::parse(&src).expect("must parse");
    tree.to_model_json("y_1.exp").expect("wire")
}

#[test]
fn identifier_s_before_in_is_an_in_relation() {
    for expr in ["s IN t", "s IN (t)", "(s) IN t", "c + s IN t", "sv IN t"] {
        let v = wire_for(expr);
        let ret = &v["schemas"][0]["functions"][0]["statements"][0]["expression"];
        assert_eq!(
            ret["_class"], "Expressir::Model::Expressions::BinaryExpression",
            "{expr} must be a BinaryExpression"
        );
        assert_eq!(ret["operator"], "IN", "{expr} must keep the IN operator");
    }
}

#[test]
fn keyword_spellings_stay_contiguous() {
    for expr in ["SIN(c)", "a IN t", "1 IN t"] {
        let v = wire_for(expr);
        let ret = &v["schemas"][0]["functions"][0]["statements"][0]["expression"];
        assert!(
            !ret.to_string().is_empty(),
            "{expr} must still extract a return expression"
        );
    }
}

#[test]
fn identifier_s_in_the_parameter_list_parses() {
    let v = wire_for("s");
    let params = &v["schemas"][0]["functions"][0]["parameters"];
    assert_eq!(params.as_array().map(Vec::len), Some(5));
}

#[test]
fn identifier_s_before_in_with_structured_rhs() {
    // The issue's "other test schemas" column: the bug was in the
    // LEFT operand (`s` eaten as the keyword SIN), so every RHS shape
    // must come back as an IN relation too.
    for expr in [
        "s IN q.items",
        "s IN [1, 2]",
        "s IN TYPEOF(r)",
        "s IN r\\grp.items",
    ] {
        let src = format!(
            "SCHEMA y_1;\nENTITY q;\n  items : LIST OF INTEGER;\nEND_ENTITY;\nENTITY grp;\n  items : LIST OF INTEGER;\nEND_ENTITY;\nENTITY e;\n  r : grp;\nWHERE\n  wr1: {};\nEND_ENTITY;\nEND_SCHEMA;\n",
            expr
        );
        let tree = expressir_rs::ParsedTree::parse(&src).expect("must parse");
        let v = tree.to_model_json("y_1.exp").expect("wire");
        let ret = &v["schemas"][0]["entities"][2]["where_rules"][0]["expression"];
        assert_eq!(
            ret["_class"], "Expressir::Model::Expressions::BinaryExpression",
            "{expr} must be a BinaryExpression"
        );
        assert_eq!(ret["operator"], "IN", "{expr} must keep the IN operator");
    }
}
