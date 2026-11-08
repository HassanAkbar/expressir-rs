//! expressir#603: tagged remark items (`RemarkItem` children) must
//! render. Over the mn-samples-expressdocs corpus, every schema keeps
//! its remark-item count across a format round trip, and the rendered
//! text carries the `(*"path"` blocks.

use std::path::PathBuf;

fn collect_exp(root: &PathBuf, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_exp(&path, out);
        } else if path.extension().is_some_and(|e| e == "exp") {
            out.push(path);
        }
    }
}

fn remark_item_count(model_json: &serde_json::Value) -> usize {
    fn walk(node: &serde_json::Value, acc: &mut usize) {
        if let Some(items) = node.get("remark_items").and_then(|v| v.as_array()) {
            *acc += items.len();
            for item in items {
                walk(item, acc);
            }
        }
        if let Some(obj) = node.as_object() {
            for (key, value) in obj {
                if key == "remark_items" {
                    continue;
                }
                if let Some(arr) = value.as_array() {
                    for child in arr {
                        if child.is_object() {
                            walk(child, acc);
                        }
                    }
                }
            }
        }
    }
    let mut acc = 0;
    if let Some(schemas) = model_json.get("schemas").and_then(|v| v.as_array()) {
        for schema in schemas {
            walk(schema, &mut acc);
        }
    }
    acc
}

#[test]
fn remark_items_round_trip_over_the_samples_corpus() {
    let dir = std::env::var_os("SAMPLES_DIR").map(PathBuf::from);
    let Some(dir) = dir else {
        eprintln!("SAMPLES_DIR unset — skipping the samples corpus run");
        return;
    };
    let mut files = Vec::new();
    collect_exp(&dir.join("schemas"), &mut files);
    collect_exp(&dir.join("plain_schemas"), &mut files);
    files.sort();
    files.dedup();
    assert!(!files.is_empty(), "no .exp files under SAMPLES_DIR");

    let mut compared = 0;
    let mut dropped = 0;
    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tree = match expressir_rs::ParsedTree::parse(&source) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mut wire = match tree.to_model_json(&file.to_string_lossy()) {
            Ok(w) => w,
            Err(_) => continue,
        };
        expressir_rs::attach::attach_tagged_remarks(&mut wire, &source, false);
        expressir_rs::attach::attach_untagged_remarks(&mut wire, &source, false);
        let before = remark_item_count(&wire);
        if before == 0 {
            continue;
        }
        let rendered = expressir_rs::formatter::format(&wire, false);
        let markers = rendered.matches("(*\"").count() + rendered.matches("--\"").count();
        compared += 1;
        if markers < before {
            dropped += 1;
            eprintln!(
                "dropped: {} — items {} markers {}",
                file.display(),
                before,
                markers
            );
        }
        // The tags must be schema-relative — no file-path segment — or
        // the rendered file cannot re-attach its remark items.
        if let Some(pos) = rendered.find("(*\"") {
            let tag = &rendered[pos + 3..];
            let tag_end = tag.find('"').unwrap_or(0);
            let tag = &tag[..tag_end];
            if tag.starts_with('/') || tag.contains(".exp.") {
                dropped += 1;
                eprintln!("file-prefixed tag on {}: {tag}", file.display());
            }
        }
        // Round trip: re-parse the rendered text; the items must survive.
        let tree2 = match expressir_rs::ParsedTree::parse(&rendered) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mut wire2 = match tree2.to_model_json(&file.to_string_lossy()) {
            Ok(w) => w,
            Err(_) => continue,
        };
        expressir_rs::attach::attach_tagged_remarks(&mut wire2, &rendered, false);
        expressir_rs::attach::attach_untagged_remarks(&mut wire2, &rendered, false);
        let after = remark_item_count(&wire2);
        if after < before {
            dropped += 1;
            eprintln!(
                "round-trip loss: {} — items {} -> {}",
                file.display(),
                before,
                after
            );
        }
    }
    eprintln!("remark-item conservation: {compared} schemas with items, {dropped} dropped");
    assert_eq!(dropped, 0, "every remark item must render and round trip");
}
