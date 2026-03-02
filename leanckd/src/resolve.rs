//! Identifier → (uri, range) resolution. Port of `leanck-lsp/src/symbols.ts`.
//!
//! Declaration resolution goes through the **warm worker**'s
//! `textDocument/documentSymbol` (flattened with namespace qualification), with a
//! regex + folding-range fallback for syntax-extension decls the server drops —
//! never a cold `lake env lean` subprocess.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::decls::{extract_decls, END_CHAR};
use crate::folding::{fetch_folding_ranges, find_body_end};
use crate::server::lifecycle::path_to_uri;
use crate::state::{WirePosition, WireRange};
use crate::workspace::Workspace;

// LSP SymbolKind values that introduce a namespace prefix for their children.
const KIND_MODULE: i64 = 2;
const KIND_NAMESPACE: i64 = 3;
const KIND_PACKAGE: i64 = 4;
const KIND_CLASS: i64 = 5;
const KIND_STRUCT: i64 = 23;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub full_name: String,
    pub kind: i64,
    pub range: WireRange,
    /// Range of the declaration's *name* token (LSP `selectionRange`). Positions
    /// here land on the identifier itself — what `textDocument/references` and
    /// other navigation requests expect. Falls back to `range` when the server
    /// omits `selectionRange`.
    pub selection_range: WireRange,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub uri: String,
    pub range: Option<WireRange>,
    pub symbol: Option<String>,
}

pub fn looks_like_path(s: &str) -> bool {
    s.contains('/') || s.ends_with(".lean") || s.starts_with('.')
}

pub fn looks_like_module(s: &str) -> bool {
    let Some(first) = s.chars().next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    s.split('.').all(|seg| {
        let mut chars = seg.chars();
        match chars.next() {
            Some(c) if c.is_ascii_uppercase() => {
                seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            }
            _ => false,
        }
    })
}

pub fn symbol_matches(sym: &Symbol, ident: &str) -> bool {
    let target = ident.strip_prefix("_root_.").unwrap_or(ident);
    if sym.full_name == target || sym.name == target {
        return true;
    }
    if let Some(prefix) = sym.full_name.strip_suffix(target) {
        return prefix.is_empty() || prefix.ends_with('.');
    }
    false
}

pub fn candidate_idents(name: &str, namespace: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(ns) = namespace {
        if !ns.is_empty() && !name.contains('.') && !name.starts_with("_root_.") {
            out.push(format!("{ns}.{name}"));
        }
    }
    out.push(name.to_string());
    out
}

/// Flatten an LSP `DocumentSymbol` tree into qualified symbols.
pub fn flatten_lsp(symbols: &[Value], prefix: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    for s in symbols {
        let Some(name) = s.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let kind = s.get("kind").and_then(|k| k.as_i64()).unwrap_or(0);
        let range = s
            .get("range")
            .and_then(|r| serde_json::from_value::<WireRange>(r.clone()).ok());
        let Some(range) = range else { continue };
        let selection_range = s
            .get("selectionRange")
            .and_then(|r| serde_json::from_value::<WireRange>(r.clone()).ok())
            .unwrap_or(range);
        let full_name = qualify(prefix, name);
        let is_container = matches!(
            kind,
            KIND_NAMESPACE | KIND_MODULE | KIND_PACKAGE | KIND_CLASS | KIND_STRUCT
        );
        let child_prefix = if is_container { full_name.clone() } else { prefix.to_string() };
        out.push(Symbol { name: name.to_string(), full_name, kind, range, selection_range });
        if let Some(children) = s.get("children").and_then(|c| c.as_array()) {
            out.extend(flatten_lsp(children, &child_prefix));
        }
    }
    out
}

fn qualify(prefix: &str, name: &str) -> String {
    if let Some(rest) = name.strip_prefix("_root_.") {
        return rest.to_string();
    }
    if prefix.is_empty() || name.starts_with(&format!("{prefix}.")) {
        return name.to_string();
    }
    format!("{prefix}.{name}")
}

/// Fetch + flatten the document symbols for an already-open file.
pub async fn document_symbols(ws: &Workspace, uri: &str, timeout: Duration) -> Vec<Symbol> {
    match ws
        .rpc()
        .request_simple(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
            timeout,
        )
        .await
    {
        Ok(Value::Array(arr)) => flatten_lsp(&arr, ""),
        _ => Vec::new(),
    }
}

/// Resolve `ident` to a declaration range within `abs_path`. Syncs the file,
/// queries document symbols (warm worker), and falls back to a regex + folding
/// scan for syntax-extension decls. Returns `None` if not found.
pub async fn resolve_declaration_in_file(
    ws: &Workspace,
    abs_path: &Path,
    ident: &str,
    namespace: Option<&str>,
) -> crate::error::Result<Option<Resolved>> {
    let uri = ws.sync_from_disk(abs_path).await?;
    let candidates = candidate_idents(ident, namespace);
    let timeout = ws.default_lsp_timeout();

    let flat = document_symbols(ws, &uri, timeout).await;
    for c in &candidates {
        if let Some(hit) = flat.iter().find(|s| symbol_matches(s, c)) {
            return Ok(Some(Resolved {
                uri,
                range: Some(hit.range),
                symbol: Some(hit.full_name.clone()),
            }));
        }
    }

    // Fallback: regex scan for syntax-extension commands documentSymbol misses,
    // refined with folding ranges from the same warm worker.
    let source = tokio::fs::read_to_string(abs_path).await.unwrap_or_default();
    let parsed = extract_decls(&source);
    for c in &candidates {
        let target = c.strip_prefix("_root_.").unwrap_or(c);
        let tail = target.rsplit('.').next().unwrap_or(target);
        if let Some(hit) = parsed.iter().find(|p| p.name == target || p.name == tail) {
            let folds = fetch_folding_ranges(ws.rpc().as_ref(), &uri, timeout).await;
            let start_line = hit.range.start.line;
            let end_line = find_body_end(&folds, start_line).unwrap_or(start_line);
            return Ok(Some(Resolved {
                uri,
                range: Some(WireRange {
                    start: WirePosition { line: start_line, character: 0 },
                    end: WirePosition { line: end_line, character: END_CHAR },
                }),
                symbol: Some(hit.name.clone()),
            }));
        }
    }
    Ok(None)
}

/// Resolve a file argument to a `file://` URI (path or module-style or literal).
pub fn file_arg_to_uri(worktree: &Path, file: &str) -> std::path::PathBuf {
    if let Some(rest) = file.strip_prefix("file://") {
        return std::path::PathBuf::from(rest);
    }
    let p = Path::new(file);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        worktree.join(file)
    }
}

/// Best-effort canonical `file://` URI for a path.
pub fn canonical_uri(path: &Path) -> String {
    let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path_to_uri(&real)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str, full: &str, kind: i64) -> Symbol {
        let range = WireRange {
            start: WirePosition { line: 0, character: 0 },
            end: WirePosition { line: 0, character: 0 },
        };
        Symbol { name: name.into(), full_name: full.into(), kind, range, selection_range: range }
    }

    #[test]
    fn looks_like_path_detects_paths() {
        assert!(looks_like_path("a/b.lean"));
        assert!(looks_like_path("./x"));
        assert!(looks_like_path("Foo.lean"));
        assert!(!looks_like_path("Nat.add"));
    }

    #[test]
    fn looks_like_module_requires_uppercase_segments() {
        assert!(looks_like_module("Mathlib.Tactic"));
        assert!(!looks_like_module("Nat.add")); // `add` lowercase
        assert!(!looks_like_module("foo"));
    }

    #[test]
    fn symbol_matches_full_tail_and_root() {
        let s = sym("add", "Nat.add", 6);
        assert!(symbol_matches(&s, "Nat.add"));
        assert!(symbol_matches(&s, "add"));
        assert!(symbol_matches(&s, "_root_.Nat.add"));
        assert!(!symbol_matches(&s, "t.add")); // prefix doesn't end on a dot boundary
    }

    #[test]
    fn candidate_idents_prepends_namespace() {
        assert_eq!(candidate_idents("foo", Some("My.NS")), vec!["My.NS.foo", "foo"]);
        assert_eq!(candidate_idents("A.foo", Some("My.NS")), vec!["A.foo"]);
        assert_eq!(candidate_idents("foo", None), vec!["foo"]);
    }

    #[test]
    fn flatten_qualifies_nested_namespaces() {
        let tree = json!([
            {
                "name": "Foo", "kind": KIND_NAMESPACE,
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}},
                "children": [
                    {"name": "bar", "kind": 6,
                     "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 5}}}
                ]
            }
        ]);
        let flat = flatten_lsp(tree.as_array().unwrap(), "");
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].full_name, "Foo");
        assert_eq!(flat[1].full_name, "Foo.bar");
        assert_eq!(flat[1].name, "bar");
    }

    #[test]
    fn flatten_captures_selection_range_else_range() {
        let tree = json!([
            {
                "name": "foo", "kind": 6,
                "range": {"start": {"line": 3, "character": 0}, "end": {"line": 5, "character": 2}},
                "selectionRange": {"start": {"line": 3, "character": 8}, "end": {"line": 3, "character": 11}}
            },
            {
                "name": "bar", "kind": 6,
                "range": {"start": {"line": 7, "character": 0}, "end": {"line": 7, "character": 9}}
            }
        ]);
        let flat = flatten_lsp(tree.as_array().unwrap(), "");
        // selectionRange present → name token position
        assert_eq!(flat[0].selection_range.start, WirePosition { line: 3, character: 8 });
        // selectionRange absent → falls back to range
        assert_eq!(flat[1].selection_range.start, WirePosition { line: 7, character: 0 });
    }

    #[test]
    fn flatten_resets_on_root_prefix() {
        let tree = json!([
            {
                "name": "NS", "kind": KIND_NAMESPACE,
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}},
                "children": [
                    {"name": "_root_.Top", "kind": 6,
                     "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 5}}}
                ]
            }
        ]);
        let flat = flatten_lsp(tree.as_array().unwrap(), "");
        assert_eq!(flat[1].full_name, "Top");
    }
}
