//! Corpus benchmark: parse vs wire-JSON emission vs serialization,
//! summed over the files given on the command line (best of 3).
//!
//! Usage: `cargo run --release --example corpus_bench -- <file.exp>...`

use std::time::Instant;

use expressir_rs::ParsedTree;
use parsanol::portable::{AstArena, PortableParser};

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    assert!(!paths.is_empty(), "usage: corpus_bench <file.exp>...");
    let sources: Vec<String> = paths
        .iter()
        .map(|p| std::fs::read_to_string(p).expect("read"))
        .collect();
    let kb: usize = sources.iter().map(|s| s.len()).sum::<usize>() / 1024;
    println!("{} files, {} KB", paths.len(), kb);

    let grammar = expressir_rs::grammar().clone();

    let runs = 3;
    let mut parse_only = f64::MAX;
    let mut parse_json_value = f64::MAX;
    let mut parse_json_string = f64::MAX;

    for _ in 0..runs {
        let t0 = Instant::now();
        for source in &sources {
            let mut arena = AstArena::for_input(source.len());
            arena.set_input(source.clone());
            let mut parser = PortableParser::new(&grammar, source, &mut arena);
            let _ = parser.parse().expect("parse");
        }
        parse_only = parse_only.min(t0.elapsed().as_secs_f64());
    }

    for _ in 0..runs {
        let t0 = Instant::now();
        for source in &sources {
            let tree = ParsedTree::parse(source).expect("parse");
            let _ = tree.to_model_json("x.exp").expect("model json");
        }
        parse_json_value = parse_json_value.min(t0.elapsed().as_secs_f64());
    }

    for _ in 0..runs {
        let t0 = Instant::now();
        for source in &sources {
            let tree = ParsedTree::parse(source).expect("parse");
            let v = tree.to_model_json("x.exp").expect("model json");
            let _ = serde_json::to_string(&v).expect("json string");
        }
        parse_json_string = parse_json_string.min(t0.elapsed().as_secs_f64());
    }

    println!("  parse only            : {:6.2} s", parse_only);
    println!("  parse + model_json    : {:6.2} s", parse_json_value);
    println!("  parse + json + string : {:6.2} s", parse_json_string);
}
