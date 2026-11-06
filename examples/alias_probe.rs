fn main() {
    let path = std::env::args().nth(1).expect("file");
    let source = std::fs::read_to_string(&path).unwrap();
    let tree = expressir_rs::ParsedTree::parse(&source).unwrap();
    let mut wire = tree.to_model_json(&path).unwrap();
    expressir_rs::attach::attach_tagged_remarks(&mut wire, &source);
    expressir_rs::attach::attach_untagged_remarks(&mut wire, &source);
    fn walk(n: &serde_json::Value, path: &str) {
        if let Some(o) = n.as_object() {
            if o.get("_class")
                .and_then(|c| c.as_str())
                .map(|c| c.ends_with("Statements::Alias"))
                .unwrap_or(false)
            {
                println!("alias at {path}");
                println!("  remarks: {:?}", o.get("remarks"));
                println!(
                    "  untagged n={}",
                    o.get("untagged_remarks")
                        .and_then(|r| r.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0)
                );
            }
            for (k, v) in o {
                match v {
                    serde_json::Value::Array(items) => {
                        for (i, it) in items.iter().enumerate() {
                            walk(it, &format!("{path}/{k}/{i}"));
                        }
                    }
                    serde_json::Value::Object(_) => walk(v, &format!("{path}/{k}")),
                    _ => {}
                }
            }
        }
    }
    walk(&wire, "");
    let text = expressir_rs::formatter::format(&wire, false);
    let lines: Vec<&str> = text.lines().collect();
    println!("formatted lines 36-42:");
    for (i, l) in lines.iter().enumerate().skip(35).take(7) {
        println!("{:>4}: {}", i + 1, l);
    }
}
