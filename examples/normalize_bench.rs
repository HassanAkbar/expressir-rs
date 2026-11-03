//! Share of `to_parslet_compatible` normalization vs the raw packrat
//! parse, summed over the files given on the command line.
//!
//! Usage: `cargo run --release --example normalize_bench -- <file.exp>...`

use parsanol::portable::{to_parslet_compatible, AstArena, PortableParser};

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    assert!(!paths.is_empty(), "usage: normalize_bench <file.exp>...");

    let grammar = expressir_rs::grammar().clone();
    let mut parse_raw = 0.0f64;
    let mut normalize = 0.0f64;
    let mut bytes = 0usize;
    let mut parsed = 0usize;

    for path in &paths {
        let source = std::fs::read_to_string(path).expect("read");
        bytes += source.len();
        let mut arena = AstArena::for_input(source.len());
        arena.set_input(source.clone());
        let t0 = std::time::Instant::now();
        let mut parser = PortableParser::new(&grammar, &source, &mut arena);
        let raw = match parser.parse() {
            Ok(r) => r,
            Err(_) => continue,
        };
        parse_raw += t0.elapsed().as_secs_f64();
        let t1 = std::time::Instant::now();
        let root = to_parslet_compatible(&raw, &mut arena, &source);
        normalize += t1.elapsed().as_secs_f64();
        std::hint::black_box(&root);
        parsed += 1;
    }
    println!(
        "files={}/{} bytes={} parse_raw={:.2}s normalize={:.2}s (normalize {:.0}% of pipeline)",
        parsed,
        paths.len(),
        bytes,
        parse_raw,
        normalize,
        normalize / (parse_raw + normalize) * 100.0
    );
}
