//! Symbol qualification and matching for declaration discovery.

use crate::server::{extensions::DocumentSymbol, session::range as wire_range};

use crate::state::WireRange;

use lsp_types::SymbolKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub full_name: String,
    pub kind: SymbolKind,
    pub range: WireRange,
    /// Range of the declaration's *name* token (LSP `selectionRange`). Positions
    /// here land on the identifier itself — what `textDocument/references` and
    /// other navigation requests expect. Falls back to `range` when the server
    /// omits `selectionRange`.
    pub selection_range: WireRange,
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
pub fn flatten_lsp(symbols: &[DocumentSymbol], prefix: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    for symbol in symbols {
        let full_name = qualify(prefix, &symbol.name);
        let is_container = matches!(
            symbol.kind,
            SymbolKind::NAMESPACE
                | SymbolKind::MODULE
                | SymbolKind::PACKAGE
                | SymbolKind::CLASS
                | SymbolKind::STRUCT
        );
        let child_prefix = if is_container {
            full_name.clone()
        } else {
            prefix.to_string()
        };
        out.push(Symbol {
            name: symbol.name.clone(),
            full_name,
            kind: symbol.kind,
            range: wire_range(symbol.range),
            selection_range: wire_range(symbol.selection_range.unwrap_or(symbol.range)),
        });
        if let Some(children) = &symbol.children {
            out.extend(flatten_lsp(children, &child_prefix));
        }
    }
    out
}

pub(crate) fn qualify(prefix: &str, name: &str) -> String {
    if let Some(rest) = name.strip_prefix("_root_.") {
        return rest.to_string();
    }
    if prefix.is_empty() || name.starts_with(&format!("{prefix}.")) {
        return name.to_string();
    }
    format!("{prefix}.{name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::WirePosition;
    use serde_json::json;

    fn sym(name: &str, full: &str, kind: SymbolKind) -> Symbol {
        let range = WireRange {
            start: WirePosition {
                line: 0,
                character: 0,
            },
            end: WirePosition {
                line: 0,
                character: 0,
            },
        };
        Symbol {
            name: name.into(),
            full_name: full.into(),
            kind,
            range,
            selection_range: range,
        }
    }

    #[test]
    fn symbol_matches_full_tail_and_root() {
        let s = sym("add", "Nat.add", SymbolKind::METHOD);
        assert!(symbol_matches(&s, "Nat.add"));
        assert!(symbol_matches(&s, "add"));
        assert!(symbol_matches(&s, "_root_.Nat.add"));
        assert!(!symbol_matches(&s, "t.add")); // prefix doesn't end on a dot boundary
    }

    #[test]
    fn candidate_idents_prepends_namespace() {
        assert_eq!(
            candidate_idents("foo", Some("My.NS")),
            vec!["My.NS.foo", "foo"]
        );
        assert_eq!(candidate_idents("A.foo", Some("My.NS")), vec!["A.foo"]);
        assert_eq!(candidate_idents("foo", None), vec!["foo"]);
    }

    #[test]
    fn flatten_qualifies_nested_namespaces() {
        let tree = json!([
            {
                "name": "Foo", "kind": SymbolKind::NAMESPACE,
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}},
                "children": [
                    {"name": "bar", "kind": 6,
                     "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 5}}}
                ]
            }
        ]);
        let flat = flatten_lsp(
            &serde_json::from_value::<Vec<DocumentSymbol>>(tree).unwrap(),
            "",
        );
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
        let flat = flatten_lsp(
            &serde_json::from_value::<Vec<DocumentSymbol>>(tree).unwrap(),
            "",
        );
        // selectionRange present → name token position
        assert_eq!(
            flat[0].selection_range.start,
            WirePosition {
                line: 3,
                character: 8
            }
        );
        // selectionRange absent → falls back to range
        assert_eq!(
            flat[1].selection_range.start,
            WirePosition {
                line: 7,
                character: 0
            }
        );
    }

    #[test]
    fn flatten_resets_on_root_prefix() {
        let tree = json!([
            {
                "name": "NS", "kind": SymbolKind::NAMESPACE,
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}},
                "children": [
                    {"name": "_root_.Top", "kind": 6,
                     "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 5}}}
                ]
            }
        ]);
        let flat = flatten_lsp(
            &serde_json::from_value::<Vec<DocumentSymbol>>(tree).unwrap(),
            "",
        );
        assert_eq!(flat[1].full_name, "Top");
    }
}
