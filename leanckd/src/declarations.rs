//! One declaration catalog serves listing, lookup, and reference positions.
use crate::{
    decls::{END_CHAR, extract_decls},
    documents::Document,
    error::{Error, Result},
    folding::find_body_end,
    request::RequestContext,
    resolve::{Symbol, candidate_idents, flatten_lsp, qualify, symbol_matches},
    server::{extensions::SymbolResponse, session::range},
    state::{WirePosition, WireRange},
};
use leanck_core::api::{DeclarationKind, LspSymbolKind};
use lsp_types::SymbolKind;

#[derive(Clone, Debug)]
pub struct Declaration {
    pub symbol: Symbol,
    pub kind: DeclarationKind,
}
pub struct Discovery {
    pub declarations: Vec<Declaration>,
    pub notes: Vec<String>,
    pub authoritative: bool,
}
impl Discovery {
    pub fn resolve(&self, ident: &str, namespace: Option<&str>) -> Result<&Declaration> {
        for candidate in candidate_idents(ident, namespace) {
            if let Some(hit) = self
                .declarations
                .iter()
                .find(|d| d.symbol.full_name == candidate.trim_start_matches("_root_."))
            {
                return Ok(hit);
            }
            let hits: Vec<_> = self
                .declarations
                .iter()
                .filter(|d| symbol_matches(&d.symbol, &candidate))
                .collect();
            if hits.len() == 1 {
                return Ok(hits[0]);
            }
            if hits.len() > 1 {
                return Err(Error::BadRequest(format!(
                    "ambiguous declaration {ident:?}; qualify its namespace"
                )));
            }
        }
        if self.authoritative {
            Err(Error::Unresolved(ident.into()))
        } else {
            Err(Error::ServerDown(format!(
                "declaration {ident:?} was not found in a heuristic fallback; retry"
            )))
        }
    }
}
pub async fn discover(doc: &Document, ctx: &RequestContext) -> Result<Discovery> {
    let result = doc
        .session
        .document_symbols(&doc.uri, ctx, doc.lsp_timeout)
        .await;
    let authoritative = result.is_ok();
    let mut notes = Vec::new();
    if !authoritative {
        notes.push("Lean symbols unavailable; declaration discovery is a text heuristic".into());
    }
    let flat = match ctx.optional(result)?.flatten() {
        Some(SymbolResponse::Nested(symbols)) => flatten_lsp(&symbols, ""),
        Some(SymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .map(|s| Symbol {
                full_name: qualify(s.container_name.as_deref().unwrap_or(""), &s.name),
                name: s.name,
                kind: s.kind,
                range: range(s.location.range),
                selection_range: range(s.location.range),
            })
            .collect(),
        None => Vec::new(),
    };
    let source = doc.source().to_owned();
    let parsed = ctx
        .run(async {
            tokio::task::spawn_blocking(move || extract_decls(&source))
                .await
                .map_err(|e| Error::ServerDown(e.to_string()))
        })
        .await?;
    let mut declarations: Vec<_> = flat
        .into_iter()
        .map(|symbol| {
            let parsed = parsed.iter().find(|p| {
                p.range.start.line == symbol.range.start.line
                    && (p.name == symbol.full_name
                        || p.name.rsplit('.').next() == Some(symbol.name.as_str()))
            });
            let kind = parsed
                .map(|p| DeclarationKind::Lean(p.kind))
                .unwrap_or_else(|| DeclarationKind::Lsp(shared_symbol_kind(symbol.kind)));
            Declaration { symbol, kind }
        })
        .collect();
    let extra: Vec<_> = parsed
        .into_iter()
        .filter(|p| !declarations.iter().any(|d| d.symbol.full_name == p.name))
        .collect();
    let folds = if extra.is_empty() {
        Vec::new()
    } else {
        let result = doc
            .session
            .folding_ranges(&doc.uri, ctx, doc.lsp_timeout)
            .await;
        if result.is_err() {
            notes.push(
                "folding ranges unavailable; textual declaration ranges are approximate".into(),
            );
        }
        ctx.optional(result)?
            .unwrap_or_default()
            .into_iter()
            .map(|f| crate::folding::FoldingRange {
                start_line: f.start_line,
                end_line: f.end_line,
                kind: f.kind.map(|k| {
                    match k {
                        lsp_types::FoldingRangeKind::Region => "region",
                        lsp_types::FoldingRangeKind::Imports => "imports",
                        lsp_types::FoldingRangeKind::Comment => "comment",
                    }
                    .into()
                }),
            })
            .collect()
    };
    if !extra.is_empty() && authoritative {
        notes.push("some declarations were discovered textually; ranges are heuristic".into());
    }
    for parsed in extra {
        let end = find_body_end(&folds, parsed.range.start.line)
            .map(|line| WirePosition {
                line,
                character: END_CHAR,
            })
            .unwrap_or(parsed.range.end);
        declarations.push(Declaration {
            kind: DeclarationKind::Lean(parsed.kind),
            symbol: Symbol {
                name: parsed.name.rsplit('.').next().unwrap().into(),
                full_name: parsed.name,
                kind: SymbolKind::METHOD,
                range: WireRange {
                    start: parsed.range.start,
                    end,
                },
                selection_range: parsed.selection_range,
            },
        });
    }
    declarations.sort_by_key(|d| (d.symbol.range.start.line, d.symbol.range.start.character));
    doc.validate()?;
    Ok(Discovery {
        declarations,
        notes,
        authoritative,
    })
}
pub(crate) fn shared_symbol_kind(kind: SymbolKind) -> LspSymbolKind {
    match kind {
        SymbolKind::FILE => LspSymbolKind::File,
        SymbolKind::MODULE => LspSymbolKind::Module,
        SymbolKind::NAMESPACE => LspSymbolKind::Namespace,
        SymbolKind::PACKAGE => LspSymbolKind::Package,
        SymbolKind::CLASS => LspSymbolKind::Class,
        SymbolKind::METHOD => LspSymbolKind::Method,
        SymbolKind::PROPERTY => LspSymbolKind::Property,
        SymbolKind::FIELD => LspSymbolKind::Field,
        SymbolKind::CONSTRUCTOR => LspSymbolKind::Constructor,
        SymbolKind::ENUM => LspSymbolKind::Enum,
        SymbolKind::INTERFACE => LspSymbolKind::Interface,
        SymbolKind::FUNCTION => LspSymbolKind::Function,
        SymbolKind::VARIABLE => LspSymbolKind::Variable,
        SymbolKind::CONSTANT => LspSymbolKind::Constant,
        SymbolKind::STRING => LspSymbolKind::String,
        SymbolKind::NUMBER => LspSymbolKind::Number,
        SymbolKind::BOOLEAN => LspSymbolKind::Boolean,
        SymbolKind::ARRAY => LspSymbolKind::Array,
        SymbolKind::OBJECT => LspSymbolKind::Object,
        SymbolKind::KEY => LspSymbolKind::Key,
        SymbolKind::NULL => LspSymbolKind::Null,
        SymbolKind::ENUM_MEMBER => LspSymbolKind::EnumMember,
        SymbolKind::STRUCT => LspSymbolKind::Struct,
        SymbolKind::EVENT => LspSymbolKind::Event,
        SymbolKind::OPERATOR => LspSymbolKind::Operator,
        SymbolKind::TYPE_PARAMETER => LspSymbolKind::TypeParameter,
        _ => LspSymbolKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        documents::{DocumentRegistry, SessionView},
        server::test_peer,
        state::ElaborationStore,
    };
    use serde_json::{Value, json};
    use std::{
        sync::{
            Arc, RwLock,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };
    async fn catalog(source: &str, symbols: Option<Value>, folds: bool) -> Discovery {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "leanck-catalog-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let path = root.join("A.lean");
        std::fs::write(&path, source).unwrap();
        let state = Arc::new(ElaborationStore::new());
        let (session, mut peer) = test_peer::pair(state.clone());
        let registry = DocumentRegistry::new(
            Arc::new(RwLock::new(SessionView {
                generation: 0,
                session: session.clone(),
            })),
            state,
            leanck_lake::source::SourceRoots {
                paths: vec![root.clone()],
                resolved: true,
            },
        );
        let ctx = RequestContext::new(Some(2000), Default::default()).unwrap();
        let doc = registry.pin(&path, false, &ctx).await.unwrap();
        peer.notify("textDocument/didOpen").await;
        let task = tokio::spawn(async move { discover(&doc, &ctx).await });
        let request = peer.request("textDocument/documentSymbol").await;
        let response = match symbols {
            Some(value) => lsp_server::Response::new_ok(request.id, value),
            None => lsp_server::Response::new_err(request.id, -32601, "symbols unavailable".into()),
        };
        peer.send(response).await;
        if folds {
            let request = peer.request("textDocument/foldingRange").await;
            peer.send(lsp_server::Response::new_ok(request.id, json!([])))
                .await;
        }
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        session.shutdown().await;
        peer.finish().await;
        std::fs::remove_dir_all(root).unwrap();
        result
    }
    #[tokio::test]
    async fn one_catalog_merges_symbols_and_namespace_qualified_lexical_kinds() {
        let source = "namespace N\ntheorem α : True := trivial\nlemma β : True := trivial\nend N\n";
        let discovery=catalog(source,Some(json!([{"name":"N","kind":3,"range":{"start":{"line":0,"character":0},"end":{"line":3,"character":5}},"children":[{"name":"α","kind":12,"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":30}},"selectionRange":{"start":{"line":1,"character":8},"end":{"line":1,"character":9}}}]}])),true).await;
        let a = discovery.resolve("α", Some("N")).unwrap();
        assert_eq!(
            a.kind,
            DeclarationKind::Lean(leanck_core::api::LeanDeclarationKind::Theorem)
        );
        assert_eq!(a.symbol.full_name, "N.α");
        let b = discovery.resolve("N.β", None).unwrap();
        assert_eq!(
            b.kind,
            DeclarationKind::Lean(leanck_core::api::LeanDeclarationKind::Lemma)
        );
        assert_eq!(b.symbol.selection_range.start.character, 6);
        assert!(discovery.notes.iter().any(|n| n.contains("heuristic")));
    }
    #[tokio::test]
    async fn lsp_fallback_kinds_and_lexical_precedence_are_typed() {
        use leanck_core::api::{KindSelector, LeanDeclarationKind};
        let discovery = catalog("theorem precise : True := trivial\n", Some(json!([
            {"name":"precise","kind":6,"location":{"uri":"file:///tmp/A.lean","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":32}}}},
            {"name":"member","kind":22,"location":{"uri":"file:///tmp/A.lean","range":{"start":{"line":1,"character":0},"end":{"line":1,"character":1}}}},
            {"name":"parameter","kind":26,"location":{"uri":"file:///tmp/A.lean","range":{"start":{"line":2,"character":0},"end":{"line":2,"character":1}}}},
            {"name":"future","kind":99,"location":{"uri":"file:///tmp/A.lean","range":{"start":{"line":3,"character":0},"end":{"line":3,"character":1}}}}
        ])), false).await;
        let precise = discovery.resolve("precise", None).unwrap();
        assert_eq!(
            precise.kind,
            DeclarationKind::Lean(LeanDeclarationKind::Theorem)
        );
        assert_eq!(precise.symbol.kind, SymbolKind::METHOD);
        assert!(!KindSelector::Method.matches(precise.kind));
        for (name, kind, selector) in [
            (
                "member",
                LspSymbolKind::EnumMember,
                KindSelector::EnumMember,
            ),
            (
                "parameter",
                LspSymbolKind::TypeParameter,
                KindSelector::TypeParameter,
            ),
            ("future", LspSymbolKind::Unknown, KindSelector::Unknown),
        ] {
            let declaration = discovery.resolve(name, None).unwrap();
            assert_eq!(declaration.kind, DeclarationKind::Lsp(kind));
            assert!(selector.matches(declaration.kind));
        }
    }

    #[tokio::test]
    async fn a_failed_symbol_request_is_never_an_authoritative_empty_list() {
        let discovery = catalog("-- empty file\n", None, false).await;
        assert!(!discovery.authoritative);
        assert!(discovery.declarations.is_empty());
        assert!(!discovery.notes.is_empty());
        assert!(matches!(
            discovery.resolve("absent", None),
            Err(Error::ServerDown(_))
        ));
    }
    #[tokio::test]
    async fn heuristic_lookup_rejects_ambiguity_and_honors_namespace() {
        let discovery = catalog(
            "namespace A\ndef x := 1\nend A\nnamespace B\ndef x := 2\nend B\n",
            None,
            true,
        )
        .await;
        assert!(matches!(
            discovery.resolve("x", None),
            Err(Error::BadRequest(_))
        ));
        assert_eq!(
            discovery.resolve("x", Some("B")).unwrap().symbol.full_name,
            "B.x"
        );
        assert_eq!(
            discovery
                .resolve("_root_.A.x", None)
                .unwrap()
                .symbol
                .full_name,
            "A.x"
        );
    }
}
