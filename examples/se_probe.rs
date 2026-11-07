fn main() {
    let path = std::env::args().nth(1).expect("file");
    let source = std::fs::read_to_string(&path).unwrap();
    let tree = expressir_rs::ParsedTree::parse(&source).unwrap();
    let mut wire = tree.to_model_json(&path).unwrap();
    expressir_rs::attach::attach_tagged_remarks(&mut wire, &source);
    fn walk(n: &serde_json::Value, path: &str) -> bool {
        if let Some(o) = n.as_object() {
            if o.contains_key("supertype_expression")
                && o.get("supertype_expression")
                    .map(|v| !v.is_null())
                    .unwrap_or(false)
            {
                let se = o.get("supertype_expression").unwrap();
                println!(
                    "entity {:?} se class={:?}",
                    o.get("id").and_then(|v| v.as_str()),
                    se.get("_class").and_then(|v| v.as_str())
                );
                println!(
                    "  se keys: {:?}",
                    se.as_object()
                        .map(|m| m.keys().cloned().collect::<Vec<_>>())
                );
                println!(
                    "  se json: {}",
                    serde_json::to_string(se).unwrap()[..200].to_string()
                );
                return true;
            }
            for (k, v) in o {
                match v {
                    serde_json::Value::Array(items) => {
                        for (i, it) in items.iter().enumerate() {
                            if walk(it, &format!("{path}/{k}/{i}")) {
                                return true;
                            }
                        }
                    }
                    serde_json::Value::Object(_) if walk(v, &format!("{path}/{k}")) => {
                        return true;
                    }
                    serde_json::Value::Object(_) => {}
                    _ => {}
                }
            }
        }
        false
    }
    walk(&wire, "");
}
