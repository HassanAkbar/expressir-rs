//! Rust core for Expressir: parses EXPRESS schemas through the parsanol
//! engine (grammar embedded as the JSON the Ruby tier serializes) and
//! builds the declaration-level model skeleton.
//!
//! Phase 1 scope (TODO.max-perf/9): the grammar is loaded once, parses
//! run on the packrat engine, and the walk extracts declarations —
//! schema name/version, entities (with explicit attributes and WHERE
//! labels), types, functions, procedures, rules, and constants.
//! Subsequent phases deepen the model and move remark attachment.

pub mod attach;
pub mod batch;
pub mod checker;
pub mod compiled_set;
pub mod extract;
pub mod format_len;
pub mod formatter;
pub mod model;
pub mod model_json;
pub mod p21;
pub mod remark;
pub mod walk;
pub mod wasm;

pub use extract::extract_file;
pub use model::*;
pub use model_json::model_json;

use std::sync::OnceLock;

use parsanol::portable::to_parslet_compatible;
use parsanol::portable::{AstArena, Grammar, PortableParser};

/// The EXPRESS grammar source at edition 2 — the same .parg file the
/// Ruby tier ships (`grammar-parg/express-grammar.parg`, serialized
/// from the live DSL by grammar-parg/atoms_to_parg.rb). The parg
/// frontend compiles the entry to the portable Grammar the engine
/// consumes; the lossy serialize_grammar JSON dump is gone.
///
/// The grammar is SPLIT into logical components composed with
/// top-level `use` declarations (expressir#588): the four component
/// sources below merge into the main document before compilation —
/// the same expansion the Ruby artifact bakes.
const EXPRESS_GRAMMAR_PARG: &str = include_str!("../assets/express-grammar.parg");
const EXPRESS_KEYWORDS_PARG: &str = include_str!("../assets/express_keywords.parg");
const EXPRESS_OPERATORS_PARG: &str = include_str!("../assets/express_operators.parg");
const EXPRESS_REMARKS_PARG: &str = include_str!("../assets/express_remarks.parg");
const EXPRESS_LITERALS_PARG: &str = include_str!("../assets/express_literals.parg");

/// The entry rule the model extraction starts from.
const ENTRY_RULE: &str = "syntax";

/// Crate version, embedded for artifact headers.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// SHA-256 of the embedded grammar source — part of the compiled-set
/// identity (a grammar change invalidates every artifact).
pub fn grammar_digest() -> String {
    compiled_set::sha256_hex(EXPRESS_GRAMMAR_PARG.as_bytes())
}

pub fn grammar() -> &'static Grammar {
    static GRAMMAR: OnceLock<Grammar> = OnceLock::new();
    GRAMMAR.get_or_init(|| {
        use parsanol::parg::frontend;
        let mut parsed = frontend::parser::parse_grammar(EXPRESS_GRAMMAR_PARG)
            .expect("embedded EXPRESS grammar is valid");
        let sources = std::collections::BTreeMap::from([
            (
                "express_keywords".to_string(),
                EXPRESS_KEYWORDS_PARG.to_string(),
            ),
            (
                "express_operators".to_string(),
                EXPRESS_OPERATORS_PARG.to_string(),
            ),
            (
                "express_remarks".to_string(),
                EXPRESS_REMARKS_PARG.to_string(),
            ),
            (
                "express_literals".to_string(),
                EXPRESS_LITERALS_PARG.to_string(),
            ),
        ]);
        frontend::imports::merge_sources(&mut parsed.document, &sources, &mut parsed.atomic_rules)
            .expect("embedded EXPRESS components merge");
        let tables = frontend::compile::Tables::default();
        let mut compiler =
            frontend::compile::compiler_for(&parsed.document, &tables, &parsed.atomic_rules)
                .expect("embedded EXPRESS compiler builds");
        compiler
            .compile_entry_grammar(ENTRY_RULE)
            .expect("embedded EXPRESS grammar compiles")
    })
}

/// Parse `source` and return the parslet-shaped AST root plus the
/// arena it lives in. The raw tagged tree (`:sequence`/`:repetition`
/// arrays) is normalized with `to_parslet_compatible`, so the root
/// matches the tree Expressir's Ruby tier hydrates: single-key
/// camelCase rule hashes, arrays for repetitions, input refs for
/// strings. The arena owns the interned strings and the original
/// input; the tree and any extracted model borrow from it.
pub struct ParsedTree {
    arena: Box<AstArena>,
    root: parsanol::portable::AstNode,
    _grammar: &'static Grammar,
}

impl ParsedTree {
    pub fn parse(source: &str) -> Result<Self, parsanol::portable::ParseError> {
        let grammar = grammar();
        let mut arena = Box::new(AstArena::for_input(source.len()));
        arena.set_input(source.to_string());
        let mut parser = PortableParser::new(grammar, source, &mut arena);
        let raw = parser.parse()?;
        let root = to_parslet_compatible(&raw, arena.as_mut(), source);
        Ok(Self {
            arena,
            root,
            _grammar: grammar,
        })
    }

    pub fn root(&self) -> &parsanol::portable::AstNode {
        &self.root
    }

    pub fn arena(&self) -> &AstArena {
        &self.arena
    }

    /// Extract the declaration-level model from this tree.
    pub fn to_model(&self) -> Repository {
        extract_file(self)
    }

    /// Emit the sparse lutaml-model wire JSON for this tree: the exact
    /// shape `Expressir::Model::ExpFile#to_hash` produces on the Ruby
    /// side, ready for `from_hash` hydration.
    pub fn to_model_json(
        &self,
        path: &str,
    ) -> Result<serde_json::Value, crate::model_json::ModelJsonError> {
        model_json(self.arena(), self.root(), path)
    }
}
