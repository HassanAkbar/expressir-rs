//! Dump the Rust formatter output head for one file.
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let n: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let source = std::fs::read_to_string(&path).unwrap();
    let tree = expressir_rs::ParsedTree::parse(&source).unwrap();
    let mut wire = tree.to_model_json(&path).unwrap();
    expressir_rs::attach::attach_tagged_remarks(&mut wire, &source, false);
    expressir_rs::attach::attach_untagged_remarks(&mut wire, &source, false);
    let text = expressir_rs::formatter::format(&wire, false);
    for (i, line) in text.lines().take(n).enumerate() {
        println!("{:>4}: {}", i + 1, line);
    }
}
