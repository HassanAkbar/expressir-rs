//! Rust-vs-Ruby scanner parity over the expressir corpus.
//!
//! Runs the Ruby `Expressir::Express::RemarkScanner` (from an
//! `EXPRESSIR_DIR` checkout) and the Rust port over every `.exp` fixture
//! the checkout carries, plus a set of inline edge cases, and requires
//! identical remark lists: position, end_position, line, text, tag, and
//! format. Skipped when `EXPRESSIR_DIR` is unset (the rs CI checkout
//! job sets it).
//!
//! Also honours `REMARK_SCANNER_EXTRA_CORPUS` — a colon-separated list
//! of extra directories (or files) to scan, for the iso-10303 samples.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const RUBY_HARNESS: &str = r##"
require "json"
root = ARGV[0]
$LOAD_PATH.unshift(File.join(root, "lib"))
require "expressir/model/remark_format"
require "expressir/express/line_map"
require "expressir/express/remark_scanner"
request = JSON.parse($stdin.read)
serialize = lambda do |remark|
  {
    "position" => remark.position,
    "end_position" => remark.end_position,
    "line" => remark.line,
    "text" => remark.text.to_s.scrub,
    "tag" => remark.tag&.scrub,
    "format" => remark.format
  }
end
result = { "files" => {}, "inline" => [] }
Array(request["files"]).each do |path|
  begin
    source = File.read(path, encoding: "UTF-8")
    result["files"][path] = Expressir::Express::RemarkScanner.new(source).scan.map(&serialize)
  rescue StandardError => e
    # Non-UTF-8 sources raise in the Ruby scanner (String#strip on an
    # invalid encoding); parity covers the sources the Ruby lane can
    # actually scan.
    result["files"][path] = { "error" => "#{e.class}: #{e.message}" }
  end
end
Array(request["inline"]).each do |source|
  result["inline"] << Expressir::Express::RemarkScanner.new(source).scan.map(&serialize)
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

/// The inline sources: every behavioural shape the Ruby scanner spec
/// covers, plus the CRLF and multibyte cases.
fn inline_sources() -> Vec<String> {
    let mut sources = vec![
        // issue #308: inline `--` inside an embedded remark.
        "SCHEMA mini_schema;\n(*\nThis schema provides representations for items a) -- d). that are mandatory in any application.\n\n[NOTE]\n--\nScenarios of the expected use are described in the introduction.\n--\n*)\nEND_SCHEMA;\n".to_string(),
        // nesting
        "(* outer (* inner *) still outer *)".to_string(),
        "(* a (* b (* c *) b *) a *)".to_string(),
        "(* The '(*' symbol starts a remark, and the '*)' symbol ends it *)".to_string(),
        // tails
        "SCHEMA x;\n-- hello world\nEND_SCHEMA;\n".to_string(),
        "--\"schema_x.attr\" tagged tail remark\n".to_string(),
        "-- IP1: an informal proposition\n".to_string(),
        "-- tail with trailing spaces   \n".to_string(),
        "CODE; -- end of file remark".to_string(),
        "SCHEMA test;\n(* block *) -- tail after block\nEND_SCHEMA;\n".to_string(),
        "a : INTEGER; -- see (* note\nb : INTEGER; -- other\n(* real *)\n".to_string(),
        // embedded
        "(* hello *)".to_string(),
        "(*\n  first line\n  second line\n*)\n".to_string(),
        "(*\"schema.entity\" remark body *)".to_string(),
        "(*\"schema.entity\" body -- with dash *)".to_string(),
        "(* first *) CODE; (* second *)\n".to_string(),
        "SCHEMA test;\n(* prose -- more prose *)\nEND_SCHEMA;\n".to_string(),
        // strings
        "CONSTANT greeting : STRING := 'hello -- world'; END_CONSTANT;\n".to_string(),
        "CONSTANT note : STRING := 'see (* not a block *) here'; END_CONSTANT;\n".to_string(),
        "x : STRING := 'it''s -- ok';\n".to_string(),
        "x : STRING := 'no -- dash'; -- real tail\n".to_string(),
        "x := 'never closed -- not a tail".to_string(),
        // edge cases
        String::new(),
        "SCHEMA x;\nEND_SCHEMA;\n".to_string(),
        "SCHEMA test;\n(* never closed\n-- not a tail\n".to_string(),
        "(* never closed".to_string(),
        "(* never closed but has -- inside".to_string(),
        "CODE; -- tail remark\r\nnext line\n".to_string(),
        "(*\r\nfirst\r\nsecond\r\n*)\r\n".to_string(),
        // `(*)` overlap (issue #126)
        "(*)".to_string(),
        "(* test (*) here *)".to_string(),
        "(* before (*) after (*) end *)".to_string(),
        "(*\"tag\" content (*) content *)".to_string(),
        "(* *)".to_string(),
        "(**)".to_string(),
        "(* a (**) c *)".to_string(),
        // order/positions/lines
        "-- one\n(* two *)\n-- three\n".to_string(),
        "SCHEMA test;\n-- a tail remark\nEND_SCHEMA;\n".to_string(),
        "SCHEMA test;\n\n(* doc\nspans lines *)\n-- tail\n".to_string(),
        // multibyte
        "name : STRING; -- Name in Japanese: 名前\n".to_string(),
        // tag edges
        "--\"solo\"\n".to_string(),
        "(*\"solo\"*)".to_string(),
        "-- IP1 no colon here\n".to_string(),
        "--IP12: twelve\n".to_string(),
        "(*\"\" empty tag *)".to_string(),
        "--\"\" empty tag\n".to_string(),
    ];
    sources.dedup();
    sources
}

fn ruby_scan(expressir_dir: &Path, files: &[PathBuf], inline: &[String]) -> Value {
    let request = json!({
        "files": files
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        "inline": inline,
    });
    let mut child = Command::new("ruby")
        .arg("-e")
        .arg(RUBY_HARNESS)
        .arg(expressir_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("ruby is available");
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(request.to_string().as_bytes())
            .expect("write request");
    }
    let output = child.wait_with_output().expect("ruby runs");
    assert!(
        output.status.success(),
        "ruby harness failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("harness emits JSON")
}

fn rust_scan(source: &str) -> Vec<Value> {
    expressir_rs::remark::scan(source)
        .into_iter()
        .map(|remark| {
            json!({
                "position": remark.position,
                "end_position": remark.end_position,
                "line": remark.line,
                "text": remark.text,
                "tag": remark.tag,
                "format": remark.format,
            })
        })
        .collect()
}

fn corpus_files(expressir_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_exp_files(expressir_dir, &mut files);
    if let Some(extra) = std::env::var_os("REMARK_SCANNER_EXTRA_CORPUS") {
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

#[test]
fn scanner_matches_ruby_over_the_corpus() {
    let Some(expressir_dir) = expressir_dir() else {
        eprintln!("EXPRESSIR_DIR unset — skipping the Ruby parity run");
        return;
    };
    let files = corpus_files(&expressir_dir);
    let inline = inline_sources();
    assert!(!files.is_empty(), "no .exp fixtures found");
    eprintln!(
        "parity: {} files + {} inline sources",
        files.len(),
        inline.len()
    );

    let ruby = ruby_scan(&expressir_dir, &files, &inline);

    let mut mismatches: BTreeMap<String, String> = BTreeMap::new();
    let mut skipped = 0usize;
    for path in &files {
        let key = path.to_string_lossy().into_owned();
        if ruby["files"][&key].get("error").is_some() {
            skipped += 1;
            continue;
        }
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {key}: {e}"));
        let source = String::from_utf8_lossy(&bytes);
        let rust = Value::Array(rust_scan(&source));
        let ruby_remarks = ruby["files"][&key].clone();
        if rust != ruby_remarks {
            mismatches.insert(key, format!("rust: {rust}\nruby: {ruby_remarks}"));
        }
    }
    eprintln!("parity: {skipped} source(s) skipped (unscannable by the Ruby lane)");

    let ruby_inline = ruby["inline"].as_array().expect("inline array");
    assert_eq!(ruby_inline.len(), inline.len());
    for (i, source) in inline.iter().enumerate() {
        let rust = Value::Array(rust_scan(source));
        if rust != ruby_inline[i] {
            mismatches.insert(
                format!("inline[{i}]: {source:?}"),
                format!("rust: {rust}\nruby: {}", ruby_inline[i]),
            );
        }
    }

    assert!(
        mismatches.is_empty(),
        "{} mismatching source(s):\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .map(|(k, v)| format!("--- {k}\n{v}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
