//! Rust-vs-Ruby attachment parity over the expressir corpus.
//!
//! For every `.exp` fixture: the Rust lane parses, emits the wire, and
//! runs the full attacher (tagged chain + untagged flows); the Ruby lane
//! parses and attaches through `RemarkAttacher`. Both sides reduce the
//! attached model to a canonical remark summary — per node
//! (class, id, source offset): the plain remarks, the remark items with
//! their texts, the informal propositions, and the untagged_remarks
//! entries with placement/region — and the two summaries must match
//! exactly. Skipped without `EXPRESSIR_DIR`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const RUBY_HARNESS: &str = r##"
require "json"
root = ARGV[0]
$LOAD_PATH.unshift(File.join(root, "lib"))
require "expressir"

summary = lambda do |node, out|
  return unless node.is_a?(Expressir::Model::ModelElement)
  key = [
    node.class.name.to_s.split("::").last,
    node.respond_to?(:id) ? node.id : nil,
    node.source_offset
  ].compact_map { |part| part.nil? ? "" : part.to_s }.join("|")
  entry = out[key] ||= {
    "remarks" => [],
    "remark_items" => [],
    "informal_propositions" => [],
    "untagged_remarks" => []
  }
  (node.respond_to?(:remarks) && node.remarks.is_a?(Array) && node.remarks || []).each do |t|
    entry["remarks"] << t
  end
  if node.respond_to?(:remark_items) && node.remark_items.is_a?(Array)
    node.remark_items.each do |ri|
      entry["remark_items"] << { "id" => ri.id, "remarks" => (ri.remarks || []) }
    end
  end
  if node.respond_to?(:informal_propositions) && node.informal_propositions.is_a?(Array)
    node.informal_propositions.each do |ip|
      entry["informal_propositions"] << {
        "id" => ip.id,
        "remark_items" => (ip.remark_items || []).map { |ri| { "id" => ri.id, "remarks" => (ri.remarks || []) } }
      }
    end
  end
  if node.respond_to?(:untagged_remarks) && node.untagged_remarks.is_a?(Array)
    node.untagged_remarks.each do |u|
      entry["untagged_remarks"] << {
        "text" => u.text,
        "format" => u.format,
        "placement" => u.placement,
        "region" => u.region
      }
    end
  end
  Expressir::Model::ModelElement.collection_registry[node.class]&.each do |attr|
    collection = node.public_send(attr) if node.respond_to?(attr)
    next unless collection.is_a?(Array)
    collection.each { |child| summary.call(child, out) }
  end
end

request = JSON.parse($stdin.read)
result = { "files" => {}, "errors" => {} }
Array(request["files"]).each do |path|
  begin
    source = File.read(path, encoding: "UTF-8")
    repo = Expressir::Express::Parser.from_exp(source)
    out = {}
    summary.call(repo, out)
    result["files"][path] = out
  rescue StandardError => e
    result["errors"][path] = "#{e.class}: #{e.message}"
  end
end
puts JSON.generate(result)
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
    if let Some(extra) = std::env::var_os("REMARK_ATTACHMENT_EXTRA_CORPUS") {
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

fn ruby_summary(expressir_dir: &Path, files: &[PathBuf]) -> Result<Value, String> {
    let request = json!({
        "files": files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
    });
    let mut child = Command::new("ruby")
        .arg("-e")
        .arg(RUBY_HARNESS)
        .arg(expressir_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ruby unavailable: {e}"))?;
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

/// The canonical per-node remark summary of the attached wire.
fn rust_summary(wire: &Value) -> Value {
    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    fn walk(node: &Value, out: &mut BTreeMap<String, Value>) {
        let Some(map) = node.as_object() else {
            return;
        };
        let class = map
            .get("_class")
            .and_then(Value::as_str)
            .map(|c| c.rsplit("::").next().unwrap_or(c).to_string());
        if let Some(class) = class {
            let id = map.get("id").and_then(Value::as_str);
            let offset = map.get("source_offset").and_then(Value::as_u64);
            let has_remark_data = map.contains_key("remarks")
                || map.contains_key("remark_items")
                || map.contains_key("informal_propositions")
                || map.contains_key("untagged_remarks");
            if has_remark_data {
                let key = format!(
                    "{}|{}|{}",
                    class,
                    id.unwrap_or(""),
                    offset.map(|o| o.to_string()).unwrap_or_default()
                );
                let entry = out.entry(key).or_insert_with(|| {
                    json!({
                        "remarks": [],
                        "remark_items": [],
                        "informal_propositions": [],
                        "untagged_remarks": [],
                    })
                });
                if let Some(remarks) = map.get("remarks").and_then(Value::as_array) {
                    for text in remarks {
                        entry["remarks"].as_array_mut().unwrap().push(text.clone());
                    }
                }
                if let Some(items) = map.get("remark_items").and_then(Value::as_array) {
                    for item in items {
                        entry["remark_items"].as_array_mut().unwrap().push(json!({
                            "id": item.get("id"),
                            "remarks": item.get("remarks").cloned().unwrap_or(json!([])),
                        }));
                    }
                }
                if let Some(ips) = map.get("informal_propositions").and_then(Value::as_array) {
                    for ip in ips {
                        let items: Vec<Value> = ip
                            .get("remark_items")
                            .and_then(Value::as_array)
                            .map(|list| {
                                list.iter()
                                    .map(|ri| {
                                        json!({
                                            "id": ri.get("id"),
                                            "remarks": ri.get("remarks").cloned().unwrap_or(json!([])),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        entry["informal_propositions"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!({
                                "id": ip.get("id"),
                                "remark_items": items,
                            }));
                    }
                }
                if let Some(un) = map.get("untagged_remarks").and_then(Value::as_array) {
                    for u in un {
                        entry["untagged_remarks"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!({
                                "text": u.get("text"),
                                "format": u.get("format"),
                                "placement": u.get("placement"),
                                "region": u.get("region"),
                            }));
                    }
                }
            }
        }
        for (_key, value) in map {
            match value {
                Value::Array(items) => {
                    for item in items {
                        walk(item, out);
                    }
                }
                Value::Object(_) => walk(value, out),
                _ => {}
            }
        }
    }
    walk(wire, &mut out);
    Value::Object(out.into_iter().collect::<serde_json::Map<String, Value>>())
}

// one-file attachment parity dump for /tmp/one.exp
#[test]
fn attachment_matches_ruby_over_the_corpus() {
    let Some(expressir_dir) = expressir_dir() else {
        eprintln!("EXPRESSIR_DIR unset — skipping the attachment parity run");
        return;
    };
    let files = corpus_files(&expressir_dir);
    assert!(!files.is_empty(), "no .exp fixtures found");

    let ruby = ruby_summary(&expressir_dir, &files).expect("ruby summary");
    let errors = ruby["errors"].as_object().cloned().unwrap_or_default();
    let mut mismatches: BTreeMap<String, String> = BTreeMap::new();
    let mut compared = 0usize;

    for path in &files {
        let key = path.to_string_lossy().into_owned();
        if errors.contains_key(&key) {
            eprintln!("skip (ruby lane errored): {key}");
            continue;
        }
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
        let mut wire = match tree.to_model_json(&key) {
            Ok(w) => w,
            Err(_) => continue,
        };
        expressir_rs::attach::attach_tagged_remarks(&mut wire, &source);
        expressir_rs::attach::attach_untagged_remarks(&mut wire, &source);
        let rust = rust_summary(&wire);
        let ruby_remarks = &ruby["files"][&key];
        compared += 1;
        if rust != *ruby_remarks {
            mismatches.insert(key.clone(), format!("rust: {rust}\nruby: {ruby_remarks}"));
        }
    }

    eprintln!(
        "attachment parity: {compared} compared, {} mismatches",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "{} file(s) with attachment drift:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .map(|(k, v)| format!("--- {k}\n{v}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
