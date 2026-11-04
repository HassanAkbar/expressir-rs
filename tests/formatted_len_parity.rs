//! Byte-length parity between the Rust formatted-length mirror
//! (`format_len`) and the Ruby `SourceFormatter` over the fixture
//! corpus. The attacher's node spans are derived from Ruby's formatted
//! lengths, so any drift here is attachment drift.
//!
//! Both lanes read the SAME Rust wire: Ruby hydrates it and reports
//! `source.length` per node its collection-registry walk visits (the
//! NodePositionIndex population), the Rust side reports the mirror
//! length for the same nodes. Gated on EXPRESSIR_DIR like the other
//! no-drift tests; skipped when the checkout or its bundle is
//! unavailable.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const RUBY_HARNESS: &str = r##"
require "json"
root = ARGV[0]
$LOAD_PATH.unshift(File.join(root, "lib"))
require "expressir"

strip_end_offsets = lambda do |obj|
  case obj
  when Hash
    obj.delete("source_end_offset")
    obj.each_value { |v| strip_end_offsets.call(v) }
  when Array
    obj.each { |v| strip_end_offsets.call(v) }
  end
  obj
end

out = {}
request = JSON.parse($stdin.read)
request["files"].each do |path, wire|
  begin
    strip_end_offsets.call(wire)
    model = Expressir::Model::ExpFile.from_hash(wire)
    model.wire_parents
    lengths = []
    walk = lambda do |node, ptr|
      if node.is_a?(Expressir::Model::ModelElement) && node.source_offset && node.source
        lengths << [node.class.name.split("::").last, node.source_offset, node.source.length, node.source.count("\n")]
      end
      Expressir::Model::ModelElement.collection_registry[node.class]&.each do |attr|
        collection = node.respond_to?(attr) ? node.public_send(attr) : nil
        next unless collection.is_a?(Array)

        collection.each_with_index { |child, i| walk.call(child, "#{ptr}/#{attr}/#{i}") }
      end
    end
    walk.call(model, "")
    out[path] = lengths
  rescue StandardError => e
    out[path] = { "error" => "#{e.class}: #{e.message}" }
  end
end
puts JSON.generate(out)
"##;

fn expressir_dir() -> Option<PathBuf> {
    std::env::var_os("EXPRESSIR_DIR").map(PathBuf::from)
}

fn collect_exp_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_exp_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "exp") {
            out.push(path);
        }
    }
}

fn corpus_files(expressir_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_exp_files(expressir_dir, &mut files);
    files.sort();
    files.dedup();
    files
}

fn ruby_lengths(expressir_dir: &Path, wires: &std::collections::BTreeMap<String, Value>) -> Result<Value, String> {
    let request = json!({ "files": wires });
    let mut child = Command::new("bundle")
        .arg("exec")
        .arg("ruby")
        .arg("-e")
        .arg(RUBY_HARNESS)
        .arg(expressir_dir)
        .current_dir(expressir_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("bundle/ruby unavailable: {e}"))?;
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(request.to_string().as_bytes())
            .map_err(|e| format!("write: {e}"))?;
    }
    let output = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ruby harness failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("json: {e}"))
}

/// The registry-walk mirror: collection children only, exactly the
/// population the Ruby index records — including Case's VIRTUAL
/// collections (the action statements and the otherwise body, never the
/// position-less CaseAction wrapper or its labels).
fn rust_lengths(wire: &Value) -> Vec<(String, u64, usize, usize)> {
    let map = expressir_rs::format_len::lengths_by_ptr(wire);
    let mut out = Vec::new();
    fn walk(
        node: &Value,
        ptr: &str,
        map: &std::collections::HashMap<String, expressir_rs::format_len::Piece>,
        out: &mut Vec<(String, u64, usize, usize)>,
    ) {
        let Some(obj) = node.as_object() else {
            return;
        };
        let class = obj.get("_class").and_then(Value::as_str);
        if let (Some(class), Some(offset)) = (
            class,
            obj.get("source_offset").and_then(Value::as_u64),
        ) {
            let suffix = class.rsplit("::").next().unwrap_or(class);
            let len = map.get(ptr).map(|p| p.len).unwrap_or(usize::MAX);
            let nl = map.get(ptr).map(|p| p.nl).unwrap_or(usize::MAX);
            out.push((suffix.to_string(), offset, len, nl));
        }
        let is_case = class == Some("Expressir::Model::Statements::Case");
        if is_case {
            if let Some(Value::Array(items)) = obj.get("actions") {
                for (i, item) in items.iter().enumerate() {
                    if let Some(stmt) = item.get("statement").filter(|v| !v.is_null()) {
                        walk(
                            stmt,
                            &format!("{ptr}/actions/{i}/statement"),
                            map,
                            out,
                        );
                    }
                }
            }
            if let Some(v) = obj.get("otherwise_statement").filter(|v| !v.is_null()) {
                walk(v, &format!("{ptr}/otherwise_statement"), map, out);
            }
            return;
        }
        let suffix_full = class.unwrap_or("");
        let suffix = suffix_full
            .strip_prefix("Expressir::Model::")
            .unwrap_or(suffix_full);
        for key in registry_keys(suffix) {
            if let Some(Value::Array(items)) = obj.get(*key) {
                for (i, item) in items.iter().enumerate() {
                    walk(item, &format!("{ptr}/{key}/{i}"), map, out);
                }
            }
        }
    }
    walk(wire, "", &map, &mut out);
    out
}

fn registry_keys(class: &str) -> &'static [&'static str] {
    match class {
        "ExpFile" => &["schemas"],
        "Declarations::Schema" => &[
            "interfaces",
            "constants",
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "rules",
            "procedures",
            "remark_items",
        ],
        "Declarations::Entity" => &[
            "attributes",
            "derived_attributes",
            "inverse_attributes",
            "unique_rules",
            "where_rules",
            "informal_propositions",
            "remark_items",
        ],
        "Declarations::Type" => &["where_rules", "informal_propositions", "remark_items"],
        "Declarations::Function" | "Declarations::Procedure" => &[
            "parameters",
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "procedures",
            "constants",
            "variables",
            "statements",
            "remark_items",
        ],
        "Declarations::Rule" => &[
            "applies_to",
            "types",
            "entities",
            "subtype_constraints",
            "functions",
            "procedures",
            "constants",
            "variables",
            "statements",
            "where_rules",
            "informal_propositions",
            "remark_items",
        ],
        "Declarations::Interface" => &["items"],
        "Statements::If" => &["statements", "else_statements"],
        "Statements::Repeat" | "Statements::Alias" | "Statements::Compound" => &["statements"],
        _ => &[],
    }
}

#[test]
fn formatted_lengths_match_ruby_over_the_corpus() {
    let Some(expressir_dir) = expressir_dir() else {
        eprintln!("EXPRESSIR_DIR not set; skipping");
        return;
    };
    let files = corpus_files(&expressir_dir);
    if files.is_empty() {
        eprintln!("no corpus files under EXPRESSIR_DIR; skipping");
        return;
    }

    let mut wires: std::collections::BTreeMap<String, Value> = Default::default();
    let mut parsed: Vec<(String, Value)> = Vec::new();
    for path in &files {
        let key = path.to_string_lossy().into_owned();
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tree = match expressir_rs::ParsedTree::parse(&source) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let wire = match tree.to_model_json("parity.exp") {
            Ok(w) => w,
            Err(_) => continue,
        };
        wires.insert(key.clone(), wire.clone());
        parsed.push((key, wire));
    }

    let ruby = ruby_lengths(&expressir_dir, &wires).expect("ruby lengths");

    let mut failures: Vec<String> = Vec::new();
    for (key, wire) in &parsed {
        let Some(entry) = ruby.get(key) else {
            continue;
        };
        if entry.get("error").is_some() {
            continue;
        }
        let mut expected: Vec<(String, u64, usize, usize)> = entry
            .as_array()
            .expect("length rows")
            .iter()
            .map(|row| {
                (
                    row[0].as_str().expect("class").to_string(),
                    row[1].as_u64().expect("offset"),
                    row[2].as_u64().expect("len") as usize,
                    row[3].as_u64().expect("nl") as usize,
                )
            })
            .collect();
        expected.sort();
        expected.dedup();

        let mut actual = rust_lengths(wire);
        actual.sort();
        actual.dedup();

        if expected != actual {
            // Merge-join on (class, offset): len mismatches and
            // one-sided nodes report distinctly.
            let mut i = 0usize;
            let mut j = 0usize;
            while i < expected.len() && j < actual.len() {
                let ek = (expected[i].0.as_str(), expected[i].1);
                let ak = (actual[j].0.as_str(), actual[j].1);
                match ek.cmp(&ak) {
                    std::cmp::Ordering::Less => {
                        failures.push(format!("{key}: ruby-only {:?} {}", expected[i], expected[i].2));
                        i += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        failures.push(format!("{key}: rust-only {:?} {}", actual[j], actual[j].2));
                        j += 1;
                    }
                    std::cmp::Ordering::Equal => {
                        if expected[i].2 != actual[j].2 {
                            failures.push(format!(
                                "{key}: {}|{} len ruby={} rust={} nl ruby={} rust={}",
                                expected[i].0, expected[i].1, expected[i].2, actual[j].2,
                                expected[i].3, actual[j].3
                            ));
                        }
                        i += 1;
                        j += 1;
                    }
                }
            }
            for row in &expected[i..] {
                failures.push(format!("{key}: ruby-only {row:?}"));
            }
            for row in &actual[j..] {
                failures.push(format!("{key}: rust-only {row:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "formatted-length drift over {} files:\n{}",
        parsed.len(),
        failures.iter().take(40).cloned().collect::<Vec<_>>().join("\n")
    );
}
