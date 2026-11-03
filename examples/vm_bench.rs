//! Bytecode VM vs packrat on the EXPRESS grammar: compile-once,
//! parse-many, with to_parslet_compatible structural comparison.
//!
//! Usage: `cargo run --release --example vm_bench -- <file.exp>...`

use std::time::Instant;

use parsanol::portable::compile_bytecode;
use parsanol::portable::parse_with_vm;
use parsanol::portable::{to_parslet_compatible, AstArena, AstNode, PortableParser};

/// Content-based deep equality across two arenas (pool indexes differ
/// by interning order, so every ref is resolved to its content).
fn deep_equal(a: &AstArena, x: &AstNode, b: &AstArena, y: &AstNode) -> bool {
    match (x, y) {
        (AstNode::Nil, AstNode::Nil) => true,
        (AstNode::Bool(p), AstNode::Bool(q)) => p == q,
        (AstNode::Int(p), AstNode::Int(q)) => p == q,
        (AstNode::Float(p), AstNode::Float(q)) => p == q,
        (AstNode::StringRef { pool_index: p }, AstNode::StringRef { pool_index: q }) => {
            a.get_string(*p as usize) == b.get_string(*q as usize)
        }
        (
            AstNode::InputRef {
                offset: po,
                length: pl,
            },
            AstNode::InputRef {
                offset: qo,
                length: ql,
            },
        ) => {
            pl == ql
                && a.get_input()[*po as usize..(*po + *pl) as usize]
                    == b.get_input()[*qo as usize..(*qo + *ql) as usize]
        }
        (
            AstNode::Array {
                pool_index: p,
                length: pl,
            },
            AstNode::Array {
                pool_index: q,
                length: ql,
            },
        ) => {
            pl == ql
                && a.get_array(*p as usize, *pl as usize)
                    .iter()
                    .zip(b.get_array(*q as usize, *ql as usize).iter())
                    .all(|(xi, yi)| deep_equal(a, xi, b, yi))
        }
        (
            AstNode::Hash {
                pool_index: p,
                length: pl,
            },
            AstNode::Hash {
                pool_index: q,
                length: ql,
            },
        ) => {
            pl == ql && {
                let ah = a.get_hash_items(*p as usize, *pl as usize);
                let bh = b.get_hash_items(*q as usize, *ql as usize);
                ah.iter()
                    .all(|(k, v)| bh.iter().any(|(k2, v2)| k == k2 && deep_equal(a, v, b, v2)))
            }
        }
        _ => false,
    }
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    assert!(!paths.is_empty(), "usage: vm_bench <file.exp>...");

    let grammar = expressir_rs::grammar().clone();

    let t0 = Instant::now();
    let program = compile_bytecode(expressir_rs::grammar().clone()).expect("bytecode compile");
    println!("program compile: {:.2}s", t0.elapsed().as_secs_f64());

    let mut packrat_s = 0.0f64;
    let mut vm_s = 0.0f64;
    let mut bytes = 0usize;
    let mut struct_equal = 0usize;
    let mut pk_mem = 0usize;
    let mut vm_mem = 0usize;
    let mut vm_failures = 0usize;

    for path in &paths {
        let source = std::fs::read_to_string(path).expect("read");
        bytes += source.len();

        let mut arena = AstArena::for_input(source.len());
        arena.set_input(source.clone());
        let t1 = Instant::now();
        let mut parser = PortableParser::new(&grammar, &source, &mut arena);
        let pk = match parser.parse() {
            Ok(r) => r,
            Err(e) => {
                println!("{}: packrat FAILED {:?}", path, e);
                continue;
            }
        };
        let pk_t = t1.elapsed().as_secs_f64();
        packrat_s += pk_t;
        pk_mem += arena.memory_usage();
        let pk_norm = to_parslet_compatible(&pk, &mut arena, &source);

        let mut vm_arena = AstArena::for_input(source.len());
        vm_arena.set_input(source.clone());
        let t2 = Instant::now();
        let vm = match parse_with_vm(&program, &source, &mut vm_arena) {
            Ok(r) => r.value,
            Err(e) => {
                println!("{}: vm FAILED {:?}", path, e);
                vm_failures += 1;
                continue;
            }
        };
        let vm_t = t2.elapsed().as_secs_f64();
        vm_s += vm_t;
        vm_mem += vm_arena.memory_usage();
        let vm_norm = to_parslet_compatible(&vm, &mut vm_arena, &source);
        let equal = deep_equal(&arena, &pk_norm, &vm_arena, &vm_norm);
        struct_equal += equal as usize;
        println!(
            "{}: packrat={:.3}s vm={:.3}s ({:.2}x) struct_equal={}",
            path,
            pk_t,
            vm_t,
            pk_t / vm_t.max(1e-9),
            equal
        );
    }
    println!(
        "files={} bytes={} packrat={:.2}s vm={:.2}s vm_speedup={:.2}x struct_equal={}/{} vm_failures={} pk_arena={}MB vm_arena={}MB",
        paths.len(),
        bytes,
        packrat_s,
        vm_s,
        packrat_s / vm_s.max(1e-9),
        struct_equal,
        paths.len(),
        vm_failures,
        pk_mem / 1048576,
        vm_mem / 1048576
    );
}
