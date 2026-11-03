//! Navigation helpers over the parsanol arena AST.
//!
//! The Ruby-tier grammar serializes named rules as single-key hashes
//! (camelCase rule name → child value) and repetitions as arrays. These
//! helpers mirror the shapes the EXPRESS grammar produces without
//! forcing the extractor to pattern-match on `AstNode` variants.

use parsanol::portable::{AstArena, AstNode};

/// Value stored under `key` in a hash node.
pub fn hash_get(arena: &AstArena, node: &AstNode, key: &str) -> Option<AstNode> {
    match node {
        AstNode::Hash { pool_index, length } => arena
            .get_hash_items(*pool_index as usize, *length as usize)
            .into_iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v),
        _ => None,
    }
}

/// Keys present in a hash node.
pub fn hash_keys(arena: &AstArena, node: &AstNode) -> Vec<String> {
    hash_pairs(arena, node)
        .into_iter()
        .map(|(k, _)| k)
        .collect()
}

/// All key-value pairs of a hash node, in insertion order.
pub fn hash_pairs(arena: &AstArena, node: &AstNode) -> Vec<(String, AstNode)> {
    match node {
        AstNode::Hash { pool_index, length } => {
            arena.get_hash_items(*pool_index as usize, *length as usize)
        }
        _ => Vec::new(),
    }
}

/// Elements of an array node (empty for anything else).
pub fn array_items(arena: &AstArena, node: &AstNode) -> Vec<AstNode> {
    match node {
        AstNode::Array { pool_index, length } => {
            arena.get_array(*pool_index as usize, *length as usize)
        }
        _ => Vec::new(),
    }
}

/// Treat a node as a list. Arrays yield their elements; a present
/// non-array node yields itself (the grammar's maybe/one-or-more
/// wrapping collapses single occurrences); nil yields nothing.
pub fn as_list(arena: &AstArena, node: &AstNode) -> Vec<AstNode> {
    match node {
        AstNode::Array { .. } => array_items(arena, node),
        AstNode::Nil => Vec::new(),
        other => vec![other.clone()],
    }
}

/// Resolve a node to its source text: input refs slice the original
/// input, string refs read the interned pool.
pub fn text(arena: &AstArena, node: &AstNode) -> Option<String> {
    match node {
        AstNode::StringRef { pool_index } => {
            Some(arena.get_string(*pool_index as usize).to_string())
        }
        AstNode::InputRef { offset, length } => {
            let input = arena.get_input();
            input
                .get(*offset as usize..(*offset + *length) as usize)
                .map(|s| s.to_string())
        }
        _ => None,
    }
}

/// Depth-first first `str` found under `node` (hash values and array
/// elements in insertion order) — mirrors the builders'
/// extract_nested_text, minus the Parsanol::Slice special case.
pub fn nested_text(arena: &AstArena, node: &AstNode) -> Option<String> {
    match node {
        AstNode::Hash { .. } => {
            if let Some(s) = hash_get(arena, node, "str").and_then(|s| text(arena, &s)) {
                return Some(s);
            }
            hash_pairs(arena, node)
                .into_iter()
                .find_map(|(_, v)| nested_text(arena, &v))
        }
        AstNode::Array { .. } => as_list(arena, node)
            .into_iter()
            .find_map(|v| nested_text(arena, &v)),
        _ => text(arena, node),
    }
}

/// Mirror of Builder#find_slice: the offset of the first input slice
/// under `node`, depth-first. A hash's `str` wins over its `spaces`;
/// `spaces` keys are otherwise skipped. Depth-capped like the Ruby.
pub fn first_slice_offset(arena: &AstArena, node: &AstNode, depth: usize) -> Option<u32> {
    if depth > 10 {
        return None;
    }
    match node {
        AstNode::InputRef { offset, .. } => Some(*offset),
        AstNode::Hash { .. } => {
            if let Some(AstNode::InputRef { offset, .. }) = hash_get(arena, node, "str").as_ref() {
                return Some(*offset);
            }
            for (key, value) in hash_pairs(arena, node) {
                if key == "spaces" {
                    continue;
                }
                if let AstNode::InputRef { offset, .. } = value {
                    return Some(offset);
                }
                if let Some(found) = first_slice_offset(arena, &value, depth + 1) {
                    return Some(found);
                }
            }
            None
        }
        AstNode::Array { .. } => as_list(arena, node)
            .into_iter()
            .find_map(|v| first_slice_offset(arena, &v, depth + 1)),
        _ => None,
    }
}

/// Recursively yield every hash node reachable from `node`, including
/// `node` itself when it is a hash.
pub fn self_and_descendant_hashes(
    arena: &AstArena,
    node: &AstNode,
    out: &mut Vec<(String, AstNode)>,
) {
    match node {
        AstNode::Hash { pool_index, length } => {
            for (key, value) in arena.get_hash_items(*pool_index as usize, *length as usize) {
                out.push((key, value.clone()));
                self_and_descendant_hashes(arena, &value, out);
            }
        }
        AstNode::Array { .. } => {
            for item in array_items(arena, node) {
                self_and_descendant_hashes(arena, &item, out);
            }
        }
        _ => {}
    }
}

/// The end offset (offset + length) of the LAST input slice under
/// `node`, depth-first from the last child — the complement of
/// `first_slice_offset`; together the two bound a subtree's byte span.
/// `spaces` keys are skipped, mirroring `first_slice_offset`.
pub fn last_slice_end(arena: &AstArena, node: &AstNode, depth: usize) -> Option<u32> {
    if depth > 10 {
        return None;
    }
    match node {
        AstNode::InputRef { offset, length } => Some(offset + length),
        AstNode::Hash { .. } => {
            let mut end = None;
            for (key, value) in hash_pairs(arena, node) {
                if key == "spaces" {
                    continue;
                }
                if let AstNode::InputRef { offset, length } = value {
                    end = Some(offset + length);
                } else if let Some(found) = last_slice_end(arena, &value, depth + 1) {
                    end = Some(found);
                }
            }
            end
        }
        AstNode::Array { .. } => {
            let mut end = None;
            for v in as_list(arena, node) {
                if let Some(found) = last_slice_end(arena, &v, depth + 1) {
                    end = Some(found);
                }
            }
            end
        }
        _ => None,
    }
}
