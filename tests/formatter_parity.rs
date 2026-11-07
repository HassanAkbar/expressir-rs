//! Rust-vs-Ruby formatter parity over the expressir corpus.
//!
//! Both lanes start from the SAME attached wire: the attachment runs in
//! Rust (tagged + untagged), then the Ruby lane hydrates the model and
//! runs the product `Expressir::Express::Formatter` while the Rust lane
//! runs `expressir_rs::formatter::format` over the same wire. The
//! rendered text must be byte-identical. A second pass formats with
//! `no_remarks` on both sides. Skipped without `EXPRESSIR_DIR`.

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
    formatted = Expressir::Express::Formatter.new.format(model)
    formatted_no_remarks = Expressir::Express::Formatter.new(no_remarks: true).format(model)
    out[path] = { "text" => formatted, "no_remarks" => formatted_no_remarks }
  rescue StandardError => e
    errors[path] = "#{e.class}: #{e.message}"
  end
end
puts JSON.generate({ "files" => out, "errors" => errors })
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

fn ruby_formats(
    expressir_dir: &Path,
    wires: &BTreeMap<String, Value>,
) -> Result<(Value, BTreeMap<String, String>), String> {
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
    let errors: BTreeMap<String, String> = parsed["errors"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
        .collect();
    Ok((parsed["files"].clone(), errors))
}

#[test]
fn formatter_matches_ruby_over_the_corpus() {
    let Some(expressir_dir) = expressir_dir() else {
        eprintln!("EXPRESSIR_DIR unset — skipping the formatter parity run");
        return;
    };
    let files = corpus_files(&expressir_dir);
    assert!(!files.is_empty(), "no .exp fixtures found");

    let mut wires: BTreeMap<String, Value> = BTreeMap::new();
    let mut rust: BTreeMap<String, (String, String)> = BTreeMap::new();
    for path in &files {
        let key = path.to_string_lossy().into_owned();
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
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
        let mut attached = wire.clone();
        expressir_rs::attach::attach_tagged_remarks(&mut attached, &source, false);
        expressir_rs::attach::attach_untagged_remarks(&mut attached, &source, false);
        let text = expressir_rs::formatter::format(&attached, false);
        let no_remarks = expressir_rs::formatter::format(&attached, true);
        rust.insert(key.clone(), (text, no_remarks));
        wires.insert(key, attached);
    }

    let (ruby, errors) = ruby_formats(&expressir_dir, &wires).expect("ruby formats");
    let mut mismatches = 0usize;
    let mut compared = 0usize;
    for (key, (rust_text, rust_no_remarks)) in &rust {
        if errors.contains_key(key) {
            eprintln!("skip (ruby lane errored): {key} — {}", errors[key]);
            continue;
        }
        compared += 1;
        let ruby_text = ruby[key]["text"].as_str().unwrap_or_default();
        let ruby_no = ruby[key]["no_remarks"].as_str().unwrap_or_default();
        if ruby_text != rust_text.as_str() {
            mismatches += 1;
            eprintln!("--- {key} (with remarks)");
            report_first_diff(ruby_text, rust_text);
        } else if ruby_no != rust_no_remarks.as_str() {
            mismatches += 1;
            eprintln!("--- {key} (no remarks)");
            report_first_diff(ruby_no, rust_no_remarks);
        }
    }
    eprintln!("formatter parity: {compared} compared, {mismatches} mismatches");
    assert_eq!(mismatches, 0, "formatter output drifted from Ruby");
}

fn report_first_diff(expected: &str, actual: &str) {
    let e_lines: Vec<&str> = expected.lines().collect();
    let a_lines: Vec<&str> = actual.lines().collect();
    for (i, (e, a)) in e_lines.iter().zip(a_lines.iter()).enumerate() {
        if e != a {
            eprintln!("  first diff line {}:", i + 1);
            eprintln!("  ruby: {e:?}");
            eprintln!("  rust: {a:?}");
            return;
        }
    }
    if e_lines.len() != a_lines.len() {
        eprintln!(
            "  line counts differ: ruby {} vs rust {}",
            e_lines.len(),
            a_lines.len()
        );
        let n = e_lines.len().min(a_lines.len());
        eprintln!("  ruby next: {:?}", e_lines.get(n));
        eprintln!("  rust next: {:?}", a_lines.get(n));
    }
}
