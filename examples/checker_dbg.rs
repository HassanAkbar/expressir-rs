//! Dump a wire pointer or run checker fills.
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let wire = if let Ok(wire_path) = std::env::var("CHECKER_WIRE") {
        serde_json::from_str(&std::fs::read_to_string(&wire_path).unwrap()).unwrap()
    } else {
        let source = std::fs::read_to_string(&path).unwrap();
        let tree = expressir_rs::ParsedTree::parse(&source).unwrap();
        tree.to_model_json(&path).unwrap()
    };
    if let Ok(spec) = std::env::var("CHECKER_DUMP_EXPANSION") {
        let (idx, k) = spec.split_once(':').unwrap();
        let out =
            expressir_rs::checker::debug_expansion(&wire, idx.parse().unwrap(), k.parse().unwrap());
        println!(
            "expansion n={} has_representation={}",
            out.len(),
            out.iter().any(|(n, _)| n == "representation")
        );
        for (n, p) in out.iter().take(8) {
            println!("  {n} -> {p}");
        }
        return;
    }
    if let Ok(spec) = std::env::var("CHECKER_DUMP_CHILDREN") {
        let idx: usize = spec.parse().unwrap();
        let ids = expressir_rs::checker::debug_children(&wire, idx);
        println!(
            "children n={} has_representation={}",
            ids.len(),
            ids.iter().any(|x| x == "representation")
        );
        println!("first ids: {:?}", &ids[..ids.len().min(12)]);
        return;
    }
    if let Ok(ptr) = std::env::var("CHECKER_DUMP_PTR") {
        println!(
            "{}",
            serde_json::to_string_pretty(wire.pointer(&ptr).unwrap()).unwrap()
        );
        return;
    }
    if let Ok(dialect) = std::env::var("CHECKER_DIALECT") {
        let mut w2 = wire.clone();
        let d = if dialect == "none" {
            None
        } else {
            Some(dialect.as_str())
        };
        let notes = expressir_rs::checker::check(&mut w2, d);
        for n in &notes {
            println!("[{}] {}: {}", n.severity, n.id, n.message);
        }
        return;
    }
    let name = std::env::var("CHECKER_DBG_NAME").unwrap_or_else(|_| "remark_entity".into());
    let fills = expressir_rs::checker::debug_fills(&wire, &name);
    println!("--- fills for '{name}': {}", fills.len());
}
