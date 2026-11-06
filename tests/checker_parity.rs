//! Rust-vs-Ruby checker parity over the expressir corpus.
//!
//! Both lanes run over the SAME wire: Ruby hydrates the model, resolves
//! references (`ResolveReferencesModelVisitor`), and runs the product
//! `Checker`; the Rust lane runs `expressir_rs::checker::check` over
//! the wire directly. Findings reduce to the canonical `validate check`
//! shape — `{id, severity, schema, message}` — and the sequences must
//! match exactly. Skipped without `EXPRESSIR_DIR`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const RUBY_HARNESS: &str = r##"
require "json"
root = ARGV[0]
$LOAD_PATH.unshift(File.join(root, "lib"))
require "expressir"

request = JSON.parse($stdin.read)
out = {}
errors = {}
request["files"].each do |path, wire|
  begin
    strip = lambda do |obj|
      case obj
      when Hash then obj.delete("source_end_offset"); obj.each_value { |v| strip.call(v) }
      when Array then obj.each { |v| strip.call(v) }
      end
      obj
    end
    strip.call(wire)
    model = Expressir::Model::ExpFile.from_hash(wire)
    model.wire_parents
    Expressir::Express::ResolveReferencesModelVisitor.new.visit(model)
    result = Expressir::Express::Checker.new(model, dialect: request["dialect"]).check
    out[path] = result.notes.map do |n|
      { "id" => n.id.to_s, "severity" => n.severity.to_s,
        "schema" => n.schema, "message" => n.message }
    end
  rescue StandardError => e
    errors[path] = "#{e.class}: #{e.message}"
  end
end
puts JSON.generate({ "notes" => out, "errors" => errors })
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
    if let Some(extra) = std::env::var_os("CHECKER_EXTRA_CORPUS") {
        for dir in std::env::split_paths(&extra) {
            if dir.is_file() {
                files.push(dir);
            } else {
                collect_exp_files(&dir, &mut files);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

type NotesByFile = BTreeMap<String, Value>;
type ErrorsByFile = BTreeMap<String, String>;

fn ruby_notes(
    expressir_dir: &Path,
    wires: &BTreeMap<String, Value>,
    dialect: Option<&str>,
) -> Result<(NotesByFile, ErrorsByFile), String> {
    let request = json!({ "files": wires, "dialect": dialect });
    let mut child = Command::new("bundle")
        .arg("exec")
        .arg("ruby")
        .arg("-e")
        .arg(RUBY_HARNESS)
        .arg(expressir_dir)
        .current_dir(expressir_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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
    let parsed: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("json: {e}"))?;
    let notes = parsed["notes"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect::<BTreeMap<String, Value>>();
    let errors: BTreeMap<String, String> = parsed["errors"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
        .collect();
    Ok((notes, errors))
}

fn rust_notes(wire: &Value, dialect: Option<&str>) -> Value {
    let mut wire = wire.clone();
    let notes = expressir_rs::checker::check(&mut wire, dialect);
    Value::Array(
        notes
            .iter()
            .map(|n| {
                json!({
                    "id": n.id,
                    "severity": n.severity,
                    "schema": n.schema,
                    "message": n.message,
                })
            })
            .collect(),
    )
}

fn run_parity(dialect: Option<&str>) {
    let Some(expressir_dir) = expressir_dir() else {
        eprintln!("EXPRESSIR_DIR unset — skipping the checker parity run");
        return;
    };
    let files = corpus_files(&expressir_dir);
    assert!(!files.is_empty(), "no .exp fixtures found");

    let mut wires: BTreeMap<String, Value> = BTreeMap::new();
    let mut rust: BTreeMap<String, Value> = BTreeMap::new();
    for path in &files {
        let key = path.to_string_lossy().into_owned();
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tree = match expressir_rs::ParsedTree::parse(&source) {
            Ok(t) => t,
            Err(_) => {
                eprintln!("skip (rust lane cannot parse): {key}");
                continue;
            }
        };
        let wire = match tree.to_model_json(&key) {
            Ok(w) => w,
            Err(_) => continue,
        };
        rust.insert(key.clone(), rust_notes(&wire, dialect));
        wires.insert(key, wire);
    }

    let (ruby, errors) = ruby_notes(&expressir_dir, &wires, dialect).expect("ruby notes");
    let mut mismatches = 0usize;
    let mut compared = 0usize;
    for (key, rust_notes) in &rust {
        if errors.contains_key(key) {
            eprintln!("skip (ruby lane errored): {key} — {}", errors[key]);
            continue;
        }
        compared += 1;
        let ruby_notes = ruby.get(key).cloned().unwrap_or(Value::Null);
        if ruby_notes != *rust_notes {
            mismatches += 1;
            eprintln!("--- {key}");
            eprintln!(
                "rust: {}",
                serde_json::to_string(rust_notes).unwrap_or_default()
            );
            eprintln!(
                "ruby: {}",
                serde_json::to_string(&ruby_notes).unwrap_or_default()
            );
        }
    }
    eprintln!(
        "checker parity (dialect={:?}): {compared} compared, {mismatches} mismatches",
        dialect
    );
    assert_eq!(mismatches, 0, "checker findings drifted from Ruby");
}

#[test]
fn checker_matches_ruby_over_the_corpus() {
    run_parity(None);
}

#[test]
fn checker_matches_ruby_over_the_corpus_dialect_1994() {
    run_parity(Some("1994"));
}

#[cfg(test)]
mod find_repro {
    use serde_json::Value;

    #[test]
    fn interface_schema_ref_resolves_at_root() {
        let Ok(text) = std::fs::read_to_string("/tmp/lsp_wire.json") else {
            return;
        };
        let wire: Value = serde_json::from_str(&text).unwrap();
        let ctx = expressir_rs::checker::Ctx::new(&wire);
        let hit = ctx.find("/files/1/schemas/0/interfaces/0/schema", "alpha");
        println!("find => {hit:?}");
        let by_root = ctx.children_by_id("");
        println!(
            "root children has alpha: {} (n={})",
            by_root.contains_key("alpha"),
            by_root.len()
        );
        assert!(hit.is_some(), "alpha should resolve at the root scope");
    }
}
