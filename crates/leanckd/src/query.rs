//! Workspace-bound queries over document leases and shared declaration discovery.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::declarations::discover;
use crate::documents::Document;
use crate::error::{Error, Result};
use crate::request::RequestContext;
use crate::sorries::{SorryEntry, SorryKind, with_diagnostics};
use crate::state::{WireDiagnostic, WirePosition, WireRange, filter_by_range, range_overlaps};
use crate::workspace::Workspace;
use leanck_core::api::{
    DiagnosticsParams, DiagnosticsResult, GoalParams, GoalResult, ListParams, ListResult,
    ListedSymbol, ReferencesParams, ReferencesResult, RestartFileParams, RestartFileResult,
    Severity, SorryDirectory, SorryFile, SorryParams, SorryResult,
};
use leanck_core::document::DocumentPath;

/// Workspace-bound query dispatch and admission; document leases own query freshness.
pub struct QueryService<'a> {
    workspace: &'a Workspace,
}
impl<'a> QueryService<'a> {
    pub fn new(workspace: &'a Workspace) -> Self {
        Self { workspace }
    }
    pub async fn execute(
        &self,
        command: leanck_core::api::Command,
        ctx: &RequestContext,
    ) -> Result<serde_json::Value> {
        use leanck_core::api::Command;
        let ws = self.workspace;
        let _permit = if matches!(
            &command,
            Command::Diagnostics(_)
                | Command::Goal(_)
                | Command::Sorry(_)
                | Command::List(_)
                | Command::References(_)
        ) {
            Some(ws.acquire_inflight(ctx).await?)
        } else {
            None
        };
        fn encode<T: serde::Serialize>(value: T) -> Result<serde_json::Value> {
            Ok(serde_json::to_value(value)?)
        }
        match command {
            Command::Diagnostics(p) => encode(diagnostics(ws, &p, ctx).await?),
            Command::Goal(p) => encode(goal(ws, &p, ctx).await?),
            Command::Sorry(p) => encode(sorry(ws, &p, ctx).await?),
            Command::List(p) => encode(list(ws, &p, ctx).await?),
            Command::References(p) => encode(references(ws, &p, ctx).await?),
            Command::RestartFile(p) => encode(restart_file(ws, &p, ctx).await?),
            _ => Err(Error::BadRequest(
                "control operation in query dispatcher".into(),
            )),
        }
    }
}

// ---- diagnostics ----------------------------------------------------------

pub async fn diagnostics(
    ws: &Workspace,
    p: &DiagnosticsParams,
    ctx: &RequestContext,
) -> Result<DiagnosticsResult> {
    let filter = &p.severity;
    let limit = p.limit.unwrap_or(5);
    let offset = p.offset.unwrap_or(0);
    let page_limit = usize::try_from(limit)
        .map_err(|_| Error::BadRequest("limit exceeds this platform's address space".into()))?;
    let page_offset = usize::try_from(offset)
        .map_err(|_| Error::BadRequest("offset exceeds this platform's address space".into()))?;

    let abs = DocumentPath::resolve(ws.worktree(), &p.file)?.into_path_buf();

    let doc = ws.document(&abs, ctx).await?;
    let mut discovery_notes = Vec::new();
    let (range, symbol) =
        if let Some(ident) = p.ident.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let discovery = discover(&doc, ctx).await?;
            let hit = discovery.resolve(ident, None)?;
            let found = (Some(hit.symbol.range), Some(hit.symbol.full_name.clone()));
            discovery_notes = discovery.notes;
            found
        } else {
            (None, None)
        };
    let uri = doc.uri.clone();
    let timeout = ctx.wait_budget(ws.default_elab_timeout())?;
    let deadline = tokio::time::Instant::now() + timeout;
    doc.wait(range.as_ref(), timeout, ctx).await?;

    // The progress gate alone is not authoritative: Lean can clear fileProgress
    // while proofs still elaborate in background tasks, and the final
    // publishDiagnostics can land after the final progress notification — either
    // way a premature "ok". `textDocument/waitForDiagnostics` resolves only once
    // every diagnostic for this version has been emitted, so confirm, then
    // re-snapshot. Best-effort: if confirmation fails (crashed worker, deadline
    // exhausted), fall back to the gate's snapshot and say so in the notes.
    let confirmed = doc
        .session
        .wait_for_diagnostics(
            &uri,
            doc.version(),
            ctx,
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await;
    ctx.check()?;
    doc.validate()?;
    let snapshot = doc.state()?;
    let version = doc.version();
    let all = snapshot.diagnostics;
    let source = doc.source();
    let sorries = with_diagnostics(&all, source).sorries;
    let mut notes = collect_notes(&all, range.as_ref(), &sorries);
    notes.extend(doc.notes.clone());
    notes.extend(discovery_notes);
    if snapshot.unversioned_diagnostics {
        notes.push("unversioned diagnostics were ignored; diagnostics may be incomplete".into());
    }
    if let Err(e) = &confirmed {
        tracing::debug!(%uri, error = %e, "waitForDiagnostics did not confirm completion");
        notes.push(
            "elaboration not confirmed complete; diagnostics may be incomplete (retry, or raise timeout_ms)"
                .to_string(),
        );
    }

    let filtered: Vec<WireDiagnostic> = filter_by_range(&all, range.as_ref())
        .into_iter()
        .filter(|d| keep_by_severity(d, filter) && !is_goals_accomplished(d))
        .collect();
    let total = u64::try_from(filtered.len())
        .map_err(|_| Error::BadRequest("too many diagnostics".into()))?;
    let paged: Vec<WireDiagnostic> = filtered
        .into_iter()
        .skip(page_offset)
        .take(page_limit)
        .collect();
    if total == 0 && confirmed.is_ok() {
        if notes.is_empty() {
            notes.push("ok".to_string());
        }
    }
    let enriched = enrich_errors_with_goals(&doc, paged, ctx).await?;
    doc.validate()?;

    Ok(DiagnosticsResult {
        uri,
        range,
        symbol,
        version,
        total,
        diagnostics: enriched,
        source: source.to_owned(),
        notes,
        offset: (offset > 0).then_some(offset),
        limit: (offset.saturating_add(limit) < total).then_some(limit),
    })
}

/// For each error-severity diagnostic, attach the conclusions of the open goals
/// at its start position. Best-effort; failures produce no `goals` field.
async fn enrich_errors_with_goals(
    doc: &Document,
    diags: Vec<WireDiagnostic>,
    ctx: &RequestContext,
) -> Result<Vec<WireDiagnostic>> {
    let mut cache: HashMap<(u32, u32), Option<Vec<String>>> = HashMap::new();
    let mut out = Vec::with_capacity(diags.len());
    for mut d in diags {
        ctx.check()?;
        if d.severity_or_error() != Severity::Error {
            out.push(d);
            continue;
        }
        let pos = (d.range.start.line, d.range.start.character);
        let goals = match cache.get(&pos) {
            Some(g) => g.clone(),
            None => {
                let g = fetch_plain_goal_conclusions(doc, pos, ctx).await?;
                cache.insert(pos, g.clone());
                g
            }
        };
        if let Some(g) = goals {
            if !g.is_empty() {
                d.goals = Some(g);
            }
        }
        out.push(d);
    }
    Ok(out)
}

async fn fetch_plain_goal_conclusions(
    doc: &Document,
    pos: (u32, u32),
    ctx: &RequestContext,
) -> Result<Option<Vec<String>>> {
    let result = doc
        .session
        .goal(
            &doc.uri,
            WirePosition {
                line: pos.0,
                character: pos.1,
            },
            ctx,
            doc.lsp_timeout,
        )
        .await;
    doc.validate()?;
    Ok(match ctx.optional(result)? {
        Some(GoalResult::Goals(goals)) => Some(
            goals
                .goals
                .iter()
                .filter_map(|g| extract_conclusion(g))
                .collect(),
        ),
        _ => None,
    })
}

pub async fn goal(ws: &Workspace, p: &GoalParams, ctx: &RequestContext) -> Result<GoalResult> {
    let abs = DocumentPath::resolve(ws.worktree(), &p.file)?.into_path_buf();
    let doc = ws.document(&abs, ctx).await?;
    let result = doc
        .session
        .goal(
            &doc.uri,
            WirePosition {
                line: p.line,
                character: p.character,
            },
            ctx,
            ctx.wait_budget(ws.default_lsp_timeout())?,
        )
        .await?;
    doc.validate()?;
    Ok(result)
}

// ---- sorry ----------------------------------------------------------------

pub async fn sorry(ws: &Workspace, p: &SorryParams, ctx: &RequestContext) -> Result<SorryResult> {
    let abs = DocumentPath::resolve(ws.worktree(), &p.path)?.into_path_buf();
    let meta = tokio::fs::metadata(&abs)
        .await
        .map_err(|_| Error::InvalidPath(format!("not found: {}", abs.display())))?;

    if meta.is_dir() {
        let context = ctx.clone();
        let root = abs.clone();
        let paths = ctx
            .run(async {
                tokio::task::spawn_blocking(move || {
                    leanck_core::sorry::lean_files(
                        &root,
                        &mut || context.check(),
                        &Error::InvalidPath,
                    )
                })
                .await
                .map_err(|e| Error::ServerDown(e.to_string()))?
            })
            .await?;
        let mut files = Vec::new();
        let mut total = 0u64;
        // Check every file, including those without sorry tokens. Release each
        // document lease before opening the next; retention stays bounded and
        // all waits share the directory request's deadline and cancellation.
        for path in paths {
            ctx.check()?;
            let file = sorry_file(ws, &path, ctx).await?;
            total += u64::try_from(file.sorries.len())
                .map_err(|_| Error::BadRequest("too many sorries".into()))?;
            if !file.sorries.is_empty() || !file.notes.is_empty() {
                files.push(file);
            }
        }
        return Ok(SorryResult::Directory(SorryDirectory {
            uri: leanck_core::document::path_to_uri(&abs)?,
            total,
            files,
            notes: Vec::new(),
        }));
    }

    Ok(SorryResult::File(sorry_file(ws, &abs, ctx).await?))
}

async fn sorry_file(ws: &Workspace, path: &Path, ctx: &RequestContext) -> Result<SorryFile> {
    let doc = ws.document(path, ctx).await?;
    let budget = ctx.wait_budget(Duration::from_millis(ws.settings.sorry_timeout_ms))?;
    let deadline = tokio::time::Instant::now() + budget;
    let settled = doc.wait(None, budget, ctx).await;
    let confirmed = if settled.is_ok() {
        doc.session
            .wait_for_diagnostics(
                &doc.uri,
                doc.version(),
                ctx,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
    } else {
        Err(Error::Timeout)
    };
    ctx.check()?;
    doc.validate()?;
    let (diagnostics, mut notes) = if settled.is_ok() {
        let snapshot = doc.state()?;
        let mut notes = doc.notes.clone();
        if snapshot.unversioned_diagnostics {
            notes.push(
                "unversioned diagnostics ignored; declaration sorry report may be incomplete"
                    .into(),
            );
        }
        if confirmed.is_err() {
            notes.push(
                "elaboration not confirmed complete; declaration sorry report may be incomplete"
                    .into(),
            );
        }
        (snapshot.diagnostics, notes)
    } else {
        (
            Vec::new(),
            vec!["elaboration unavailable; sorry report uses a text-only heuristic".into()],
        )
    };
    let scan = with_diagnostics(&diagnostics, doc.source());
    doc.validate()?;
    notes.extend(scan.notes);
    notes.dedup();

    Ok(SorryFile {
        uri: doc.uri.clone(),
        sorries: scan.sorries,
        notes,
    })
}

// ---- restart_file ---------------------------------------------------------

pub async fn restart_file(
    ws: &Workspace,
    p: &RestartFileParams,
    ctx: &RequestContext,
) -> Result<RestartFileResult> {
    let abs = DocumentPath::resolve(ws.worktree(), &p.file)?.into_path_buf();
    let uri = ctx.run(ws.restart_file(&abs, ctx)).await?;
    Ok(RestartFileResult {
        uri,
        restarted: true,
    })
}

// ---- references -----------------------------------------------------------

/// Find all references to a declaration via the Lean server's reference index
/// (backed by `.ilean` files, so it includes uses inside proof bodies). Returns
/// the raw LSP `Location[]`; the caller maps each location to its enclosing
/// declaration. Requires the project to have been built so the index exists.
pub async fn references(
    ws: &Workspace,
    p: &ReferencesParams,
    ctx: &RequestContext,
) -> Result<ReferencesResult> {
    let abs = DocumentPath::resolve(ws.worktree(), &p.file)?.into_path_buf();
    let doc = ws.document(&abs, ctx).await?;
    doc.wait(None, ctx.wait_budget(ws.default_elab_timeout())?, ctx)
        .await?;
    let discovery = discover(&doc, ctx).await?;
    let hit = discovery.resolve(&p.ident, p.namespace.as_deref())?;
    let locations = doc
        .session
        .references(
            &doc.uri,
            hit.symbol.selection_range.start,
            ctx,
            ws.default_lsp_timeout(),
        )
        .await?;
    doc.validate()?;
    Ok(ReferencesResult {
        symbol: hit.symbol.full_name.clone(),
        uri: doc.uri.clone(),
        locations,
        notes: {
            let mut notes = doc.notes.clone();
            notes.extend(discovery.notes);
            notes
        },
    })
}

// ---- list -----------------------------------------------------------------

pub async fn list(ws: &Workspace, p: &ListParams, ctx: &RequestContext) -> Result<ListResult> {
    let abs = DocumentPath::resolve(ws.worktree(), &p.file)?.into_path_buf();
    let doc = ws.document(&abs, ctx).await?;
    doc.wait(None, ctx.wait_budget(ws.default_elab_timeout())?, ctx)
        .await?;
    let discovery = discover(&doc, ctx).await?;

    let symbols = discovery
        .declarations
        .into_iter()
        .filter(|d| p.includes_kind(d.kind))
        .map(|d| ListedSymbol {
            name: d.symbol.name,
            full_name: d.symbol.full_name,
            kind: d.kind,
            range: d.symbol.range,
        })
        .collect();
    doc.validate()?;
    let mut notes = doc.notes.clone();
    notes.extend(discovery.notes);
    Ok(ListResult {
        uri: doc.uri.clone(),
        symbols,
        notes,
    })
}

// ---- helpers (ported from httpServer.ts) ----------------------------------

pub use leanck_core::api::SeverityFilter;

pub fn keep_by_severity(d: &WireDiagnostic, filter: &SeverityFilter) -> bool {
    filter.includes(d.severity_or_error())
}

pub fn is_goals_accomplished(d: &WireDiagnostic) -> bool {
    d.message == "Goals accomplished!"
}

/// Pull the conclusion (text after `⊢`) out of one goal's pretty-printed form.
pub fn extract_conclusion(goal_text: &str) -> Option<String> {
    let lines: Vec<&str> = goal_text.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let Some(head) = trimmed.strip_prefix('⊢') else {
            continue;
        };
        let head = head.trim();
        let mut parts: Vec<String> = vec![head.to_string()];
        for l in &lines[i + 1..] {
            parts.push(l.trim_end().to_string());
        }
        return Some(parts.join("\n").trim().to_string());
    }
    None
}

/// Build structured notes from the whole-file diagnostics + sorries.
pub fn collect_notes(
    all: &[WireDiagnostic],
    target_range: Option<&WireRange>,
    sorries: &[SorryEntry],
) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(first) = all.first() {
        if first.message.starts_with("Imports are out of date") {
            notes.push(
                "imports are out of date: Use `restart_file` on THIS file to rebuild its imports"
                    .to_string(),
            );
        }
    }
    let lines = sorry_note_lines(sorries, target_range);
    if !lines.is_empty() {
        let scope = if target_range.is_some() {
            "declaration"
        } else {
            "file"
        };
        let label = if lines.len() == 1 { "line" } else { "lines" };
        let joined = lines
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        notes.push(format!("{scope} contains sorry at {label} {joined}"));
    }
    notes
}

/// 1-based, sorted, unique start lines of in-scope sorries; prefers tactic hits.
pub fn sorry_note_lines(sorries: &[SorryEntry], target_range: Option<&WireRange>) -> Vec<u32> {
    let scoped: Vec<&SorryEntry> = sorries
        .iter()
        .filter(|s| target_range.is_none_or(|t| range_overlaps(&s.range, t)))
        .collect();
    let has_tactic = scoped.iter().any(|s| s.kind == SorryKind::Tactic);
    let preferred: Vec<&SorryEntry> = if has_tactic {
        scoped
            .into_iter()
            .filter(|s| s.kind == SorryKind::Tactic)
            .collect()
    } else {
        scoped
    };
    let mut lines: Vec<u32> = preferred.iter().map(|s| s.range.start.line + 1).collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(severity: Option<i32>, message: &str) -> WireDiagnostic {
        WireDiagnostic {
            range: WireRange {
                start: WirePosition {
                    line: 0,
                    character: 0,
                },
                end: WirePosition {
                    line: 0,
                    character: 1,
                },
            },
            severity: severity.map(Into::into),
            code: None,
            message: message.to_string(),
            tags: None,
            related_information: None,
            data: None,
            goals: None,
        }
    }

    fn sorry(line: u32, kind: SorryKind) -> SorryEntry {
        SorryEntry {
            range: WireRange {
                start: WirePosition { line, character: 0 },
                end: WirePosition { line, character: 5 },
            },
            kind,
        }
    }

    #[test]
    fn keep_by_severity_rules() {
        use leanck_core::api::DiagnosticSeverity;
        let warnings = SeverityFilter::Only {
            severities: vec![DiagnosticSeverity::Warning],
        };
        assert!(keep_by_severity(&diag(Some(3), ""), &SeverityFilter::All));
        assert!(keep_by_severity(&diag(Some(99), ""), &SeverityFilter::All));
        assert!(keep_by_severity(
            &diag(None, ""),
            &SeverityFilter::default()
        ));
        assert!(!keep_by_severity(&diag(None, ""), &warnings));
        assert!(keep_by_severity(&diag(Some(2), ""), &warnings));
        assert!(!keep_by_severity(&diag(Some(99), ""), &warnings));
        assert!(!keep_by_severity(
            &diag(Some(1), ""),
            &SeverityFilter::Only { severities: vec![] }
        ));
    }

    #[test]
    fn goals_accomplished_exact_match() {
        assert!(is_goals_accomplished(&diag(None, "Goals accomplished!")));
        assert!(!is_goals_accomplished(&diag(None, "goals accomplished")));
    }

    #[test]
    fn extract_conclusion_after_turnstile() {
        assert_eq!(
            extract_conclusion("n : Nat\nh : n > 0\n⊢ n + 0 = n").as_deref(),
            Some("n + 0 = n")
        );
        assert_eq!(
            extract_conclusion("⊢ a =\n  b").as_deref(),
            Some("a =\n  b")
        );
        assert_eq!(extract_conclusion("no turnstile here"), None);
    }

    #[test]
    fn collect_notes_flags_imports_and_sorries() {
        let notes = collect_notes(
            &[diag(None, "Imports are out of date and must be rebuilt")],
            None,
            &[],
        );
        assert!(notes.iter().any(|n| n.contains("restart_file")));

        let notes = collect_notes(&[], None, &[sorry(4, SorryKind::Tactic)]);
        assert!(notes.contains(&"file contains sorry at line 5".to_string()));

        assert_eq!(collect_notes(&[], None, &[]), Vec::<String>::new());
    }

    #[test]
    fn sorry_note_lines_prefers_tactic_sorted_unique() {
        let lines = sorry_note_lines(
            &[
                sorry(8, SorryKind::Tactic),
                sorry(2, SorryKind::Tactic),
                sorry(2, SorryKind::Declaration),
            ],
            None,
        );
        assert_eq!(lines, vec![3, 9]);
    }

    #[test]
    fn sorry_note_lines_honors_target_range() {
        let target = WireRange {
            start: WirePosition {
                line: 0,
                character: 0,
            },
            end: WirePosition {
                line: 3,
                character: 0,
            },
        };
        let lines = sorry_note_lines(
            &[sorry(1, SorryKind::Tactic), sorry(50, SorryKind::Tactic)],
            Some(&target),
        );
        assert_eq!(lines, vec![2]);
    }
}
