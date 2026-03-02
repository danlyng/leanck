//! Query handlers: diagnostics, goal, sorry, list. Ports the request/response
//! shaping from `leanck-lsp/src/httpServer.ts` so the `leanck-mcp` formatting
//! layer is unchanged. All declaration work goes through the warm worker.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::decls::{extract_decls, END_CHAR};
use crate::error::{Error, Result};
use crate::folding::{fetch_folding_ranges, find_body_end};
use crate::proto::{
    DiagnosticsParams, GoalParams, ListParams, ReferencesParams, RestartFileParams, SorryParams,
};
use crate::resolve::{
    candidate_idents, canonical_uri, document_symbols, file_arg_to_uri,
    resolve_declaration_in_file, symbol_matches,
};
use crate::sorries::{collect_sorries, SorryEntry, SorryKind};
use crate::state::{
    filter_by_range, range_overlaps, wait_for_range, WireDiagnostic, WirePosition, WireRange,
};
use crate::workspace::Workspace;

const DIR_SKIP: &[&str] = &[".git", ".lake", "build", "target", "node_modules"];
const SORRY_WAIT: Duration = Duration::from_secs(5);

// ---- diagnostics ----------------------------------------------------------

pub async fn diagnostics(
    ws: &Workspace,
    p: &DiagnosticsParams,
    timeout: Option<Duration>,
) -> Result<Value> {
    let filter = parse_severity_filter(p.severity.as_deref())
        .ok_or_else(|| Error::BadRequest("invalid severity (expected error|warning|info|hint|all)".into()))?;
    let limit = p.limit.unwrap_or(5);
    let offset = p.offset.unwrap_or(0);

    let abs = file_arg_to_uri(ws.worktree(), &p.file);

    // Resolve a declaration range if an ident was given; else whole-file.
    let (uri, range, symbol) = match p.ident.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(ident) => {
            let resolved = resolve_declaration_in_file(ws, &abs, ident, None)
                .await?
                .ok_or_else(|| Error::Unresolved(ident.to_string()))?;
            (resolved.uri, resolved.range, resolved.symbol)
        }
        None => {
            let uri = ws.sync_from_disk(&abs).await?;
            (uri, None, None)
        }
    };

    let timeout = timeout.unwrap_or_else(|| ws.default_elab_timeout());
    let deadline = tokio::time::Instant::now() + timeout;
    let wait = wait_for_range(ws.state(), &uri, range.as_ref(), timeout).await?;

    // The progress gate alone is not authoritative: Lean can clear fileProgress
    // while proofs still elaborate in background tasks, and the final
    // publishDiagnostics can land after the final progress notification — either
    // way a premature "ok". `textDocument/waitForDiagnostics` resolves only once
    // every diagnostic for this version has been emitted, so confirm, then
    // re-snapshot. Best-effort: if confirmation fails (crashed worker, deadline
    // exhausted), fall back to the gate's snapshot and say so in the notes.
    let confirmed = ws
        .wait_for_diagnostics(
            &uri,
            ws.doc_version(&uri),
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await;

    let snapshot = ws.state().get(&uri);
    let version = snapshot.as_ref().map(|s| s.version).unwrap_or(wait.version);
    let all = snapshot.map(|s| s.diagnostics).unwrap_or_default();
    let source = tokio::fs::read_to_string(&abs).await.unwrap_or_default();
    let sorries = collect_sorries(ws.state(), &uri, &source);
    let mut notes = collect_notes(&all, range.as_ref(), &sorries);
    if let Err(e) = &confirmed {
        tracing::debug!(%uri, error = %e, "waitForDiagnostics did not confirm completion");
        notes.push(
            "elaboration not confirmed complete; diagnostics may be incomplete (retry, or raise timeout_ms)"
                .to_string(),
        );
    }

    let filtered: Vec<WireDiagnostic> = filter_by_range(&all, range.as_ref())
        .into_iter()
        .filter(|d| keep_by_severity(d, &filter) && !is_goals_accomplished(d))
        .collect();
    let total = filtered.len();
    let paged: Vec<WireDiagnostic> = filtered.into_iter().skip(offset).take(limit).collect();
    if total == 0 && confirmed.is_ok() {
        notes.push("ok".to_string());
    }
    let enriched = enrich_errors_with_goals(ws, &uri, paged).await;

    let mut body = json!({
        "uri": uri,
        "range": range,
        "symbol": symbol,
        "version": version,
        "total": total,
        "diagnostics": enriched,
        "notes": notes,
    });
    if offset > 0 {
        body["offset"] = json!(offset);
    }
    if offset + limit < total {
        body["limit"] = json!(limit);
    }
    Ok(body)
}

/// For each error-severity diagnostic, attach the conclusions of the open goals
/// at its start position. Best-effort; failures produce no `goals` field.
async fn enrich_errors_with_goals(
    ws: &Workspace,
    uri: &str,
    diags: Vec<WireDiagnostic>,
) -> Vec<WireDiagnostic> {
    let mut cache: HashMap<(u32, u32), Option<Vec<String>>> = HashMap::new();
    let mut out = Vec::with_capacity(diags.len());
    for mut d in diags {
        if d.severity.unwrap_or(1) != 1 {
            out.push(d);
            continue;
        }
        let pos = (d.range.start.line, d.range.start.character);
        let goals = match cache.get(&pos) {
            Some(g) => g.clone(),
            None => {
                let g = fetch_plain_goal_conclusions(ws, uri, pos).await;
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
    out
}

async fn fetch_plain_goal_conclusions(
    ws: &Workspace,
    uri: &str,
    pos: (u32, u32),
) -> Option<Vec<String>> {
    let resp = ws
        .rpc()
        .request_simple(
            "$/lean/plainGoal",
            json!({"textDocument": {"uri": uri}, "position": {"line": pos.0, "character": pos.1}}),
            ws.default_lsp_timeout(),
        )
        .await
        .ok()?;
    let goals = resp.get("goals")?.as_array()?;
    let conclusions: Vec<String> = goals
        .iter()
        .filter_map(|g| g.as_str())
        .filter_map(extract_conclusion)
        .collect();
    Some(conclusions)
}

// ---- goal -----------------------------------------------------------------

pub async fn goal(ws: &Workspace, p: &GoalParams, timeout: Option<Duration>) -> Result<Value> {
    let abs = file_arg_to_uri(ws.worktree(), &p.file);
    let uri = ws.sync_from_disk(&abs).await?;
    let result = ws
        .rpc()
        .request_simple(
            "$/lean/plainGoal",
            json!({"textDocument": {"uri": uri}, "position": {"line": p.line, "character": p.character}}),
            timeout.unwrap_or_else(|| ws.default_lsp_timeout()),
        )
        .await?;
    if result.is_null() {
        return Ok(Value::Null);
    }
    // Drop `rendered` (a markdown duplicate of `goals`).
    let mut obj = result;
    if let Some(map) = obj.as_object_mut() {
        map.remove("rendered");
    }
    Ok(obj)
}

// ---- sorry ----------------------------------------------------------------

pub async fn sorry(ws: &Workspace, p: &SorryParams, timeout: Option<Duration>) -> Result<Value> {
    let abs = file_arg_to_uri(ws.worktree(), &p.path);
    let meta = tokio::fs::metadata(&abs)
        .await
        .map_err(|_| Error::InvalidPath(format!("not found: {}", abs.display())))?;

    if meta.is_dir() {
        let mut files = Vec::new();
        let mut total = 0usize;
        for leaf in walk_lean_files(&abs) {
            let uri = canonical_uri(&leaf);
            let source = tokio::fs::read_to_string(&leaf).await.unwrap_or_default();
            ws.state().touch(&uri);
            let entries = collect_sorries(ws.state(), &uri, &source);
            if entries.is_empty() {
                continue;
            }
            total += entries.len();
            files.push(json!({ "uri": uri, "sorries": entries }));
        }
        return Ok(json!({ "uri": canonical_uri(&abs), "total": total, "files": files }));
    }

    let uri = ws.sync_from_disk(&abs).await?;
    // Wait briefly for elaboration so hasSorry diagnostics appear; fall back to
    // a text-only scan on timeout. An explicit per-request timeout extends the wait.
    let _ = wait_for_range(ws.state(), &uri, None, timeout.unwrap_or(SORRY_WAIT)).await;
    let source = tokio::fs::read_to_string(&abs).await.unwrap_or_default();
    let entries = collect_sorries(ws.state(), &uri, &source);
    Ok(json!({ "uri": uri, "sorries": entries }))
}

// ---- restart_file ---------------------------------------------------------

pub async fn restart_file(ws: &Workspace, p: &RestartFileParams) -> Result<Value> {
    let abs = file_arg_to_uri(ws.worktree(), &p.file);
    let uri = ws.restart_file(&abs).await?;
    Ok(json!({ "uri": uri, "restarted": true }))
}

// ---- references -----------------------------------------------------------

/// Find all references to a declaration via the Lean server's reference index
/// (backed by `.ilean` files, so it includes uses inside proof bodies). Returns
/// the raw LSP `Location[]`; the caller maps each location to its enclosing
/// declaration. Requires the project to have been built so the index exists.
pub async fn references(
    ws: &Workspace,
    p: &ReferencesParams,
    timeout: Option<Duration>,
) -> Result<Value> {
    let abs = file_arg_to_uri(ws.worktree(), &p.file);
    let uri = ws.sync_from_disk(&abs).await?;
    // The server resolves a reference query against a position once the defining
    // file has elaborated; gate on that first (like `list`).
    wait_for_range(
        ws.state(),
        &uri,
        None,
        timeout.unwrap_or_else(|| ws.default_elab_timeout()),
    )
    .await?;

    let lsp_timeout = ws.default_lsp_timeout();

    // Resolve the declaration's *name token* position (selectionRange), which is
    // where `textDocument/references` must be anchored.
    let candidates = candidate_idents(&p.ident, p.namespace.as_deref());
    let flat = document_symbols(ws, &uri, lsp_timeout).await;
    let (name_pos, symbol) = match candidates
        .iter()
        .find_map(|c| flat.iter().find(|s| symbol_matches(s, c)))
    {
        Some(sym) => (sym.selection_range.start, sym.full_name.clone()),
        None => {
            // Syntax-extension decls documentSymbol drops: fall back to the
            // regex/folding resolver and anchor at the declaration start.
            let resolved = resolve_declaration_in_file(ws, &abs, &p.ident, p.namespace.as_deref())
                .await?
                .ok_or_else(|| Error::Unresolved(p.ident.clone()))?;
            let pos = resolved
                .range
                .map(|r| r.start)
                .unwrap_or(WirePosition { line: 0, character: 0 });
            (pos, resolved.symbol.unwrap_or_else(|| p.ident.clone()))
        }
    };

    let result = ws
        .rpc()
        .request_simple(
            "textDocument/references",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": name_pos.line, "character": name_pos.character},
                "context": {"includeDeclaration": false},
            }),
            lsp_timeout,
        )
        .await?;

    let locations = match result {
        Value::Array(arr) => arr,
        _ => Vec::new(),
    };
    Ok(json!({ "symbol": symbol, "uri": uri, "locations": locations }))
}

// ---- list -----------------------------------------------------------------

#[derive(Debug, Clone)]
struct ListedSymbol {
    name: String,
    full_name: String,
    kind: String,
    range: WireRange,
}

pub async fn list(ws: &Workspace, p: &ListParams, timeout: Option<Duration>) -> Result<Value> {
    let abs = file_arg_to_uri(ws.worktree(), &p.file);
    let uri = ws.sync_from_disk(&abs).await?;
    // documentSymbol blocks server-side until elaboration settles; gate first.
    wait_for_range(
        ws.state(),
        &uri,
        None,
        timeout.unwrap_or_else(|| ws.default_elab_timeout()),
    )
    .await?;

    let source = tokio::fs::read_to_string(&abs).await.unwrap_or_default();
    let listed = build_listed_symbols(ws, &uri, &source).await;

    let filtered = match &p.kind {
        Some(kinds) if !kinds.is_empty() => {
            let allowed = normalize_kind_filter(kinds);
            listed.into_iter().filter(|s| allowed.contains(&s.kind)).collect()
        }
        _ => listed,
    };

    let symbols: Vec<Value> = filtered
        .into_iter()
        .map(|s| json!({"name": s.name, "fullName": s.full_name, "kind": s.kind, "range": s.range}))
        .collect();
    Ok(json!({ "uri": uri, "symbols": symbols }))
}

async fn build_listed_symbols(ws: &Workspace, uri: &str, source: &str) -> Vec<ListedSymbol> {
    let timeout = ws.default_lsp_timeout();
    let lsp_flat = document_symbols(ws, uri, timeout).await;
    let lsp_names: HashSet<&str> = lsp_flat.iter().map(|s| s.name.as_str()).collect();
    let lsp_full: HashSet<&str> = lsp_flat.iter().map(|s| s.full_name.as_str()).collect();

    let parsed = extract_decls(source);
    let new_regex: Vec<_> = parsed
        .iter()
        .filter(|p| {
            let tail = p.name.rsplit('.').next().unwrap_or(&p.name);
            !lsp_full.contains(p.name.as_str()) && !lsp_names.contains(tail)
        })
        .collect();

    let folds = if new_regex.is_empty() {
        Vec::new()
    } else {
        fetch_folding_ranges(ws.rpc().as_ref(), uri, timeout).await
    };

    // Merge: start from LSP symbols, then regex-only hits with folding-refined ends.
    let mut merged: Vec<(String, String, i64, WireRange)> = lsp_flat
        .iter()
        .map(|s| (s.name.clone(), s.full_name.clone(), s.kind, s.range))
        .collect();
    for p in new_regex {
        let tail = p.name.rsplit('.').next().unwrap_or(&p.name).to_string();
        let start_line = p.range.start.line;
        let end_line = find_body_end(&folds, start_line).unwrap_or(start_line);
        merged.push((
            tail,
            p.name.clone(),
            6, // method; reclassified below
            WireRange {
                start: WirePosition { line: start_line, character: 0 },
                end: WirePosition { line: end_line, character: END_CHAR },
            },
        ));
    }

    let source_lines: Vec<&str> = source.split('\n').collect();
    let mut listed: Vec<ListedSymbol> = merged
        .into_iter()
        .map(|(name, full_name, kind, range)| {
            let kind_str = symbol_kind_name(kind);
            let resolved = if kind_str == "method" {
                classify_decl_kind(&source_lines, range.start.line as usize)
            } else {
                kind_str.to_string()
            };
            ListedSymbol { name, full_name, kind: resolved, range }
        })
        .collect();

    listed.sort_by(|a, b| {
        a.range
            .start
            .line
            .cmp(&b.range.start.line)
            .then(a.range.start.character.cmp(&b.range.start.character))
    });
    listed
}

// ---- helpers (ported from httpServer.ts) ----------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeverityFilter {
    All,
    Only(HashSet<i32>),
}

/// Parse a comma-separated severity list. `None`/empty → error-only. `"all"` →
/// no filter. Returns `None` for an unknown token (caller → BadRequest).
pub fn parse_severity_filter(raw: Option<&str>) -> Option<SeverityFilter> {
    let raw = match raw {
        None => return Some(SeverityFilter::Only(HashSet::from([1]))),
        Some(s) if s.trim().is_empty() => return Some(SeverityFilter::Only(HashSet::from([1]))),
        Some(s) => s,
    };
    let tokens: Vec<String> = raw
        .split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Some(SeverityFilter::Only(HashSet::from([1])));
    }
    if tokens.iter().any(|t| t == "all") {
        return Some(SeverityFilter::All);
    }
    let mut out = HashSet::new();
    for t in tokens {
        let sev = match t.as_str() {
            "error" => 1,
            "warning" | "warn" => 2,
            "info" | "information" => 3,
            "hint" => 4,
            _ => return None,
        };
        out.insert(sev);
    }
    Some(SeverityFilter::Only(out))
}

pub fn keep_by_severity(d: &WireDiagnostic, filter: &SeverityFilter) -> bool {
    match filter {
        SeverityFilter::All => true,
        SeverityFilter::Only(set) => set.contains(&d.severity.unwrap_or(1)),
    }
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
        let scope = if target_range.is_some() { "declaration" } else { "file" };
        let label = if lines.len() == 1 { "line" } else { "lines" };
        let joined = lines.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ");
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
        scoped.into_iter().filter(|s| s.kind == SorryKind::Tactic).collect()
    } else {
        scoped
    };
    let mut lines: Vec<u32> = preferred.iter().map(|s| s.range.start.line + 1).collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

const DECL_KEYWORDS: &[&str] = &[
    "theorem",
    "lemma",
    "def",
    "definition",
    "abbrev",
    "instance",
    "structure",
    "class",
    "inductive",
    "opaque",
    "axiom",
];

fn classify_decl_kind(source_lines: &[&str], start_line: usize) -> String {
    let limit = 5.min(source_lines.len().saturating_sub(start_line));
    for off in 0..limit {
        let line = source_lines[start_line + off];
        for kw in DECL_KEYWORDS {
            if let Some(idx) = find_word(line, kw) {
                let _ = idx;
                return kw.to_string();
            }
        }
    }
    "method".to_string()
}

/// Find `word` as a whole word in `line` (word boundaries on both sides).
fn find_word(line: &str, word: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let wb = word.as_bytes();
    let mut i = 0;
    while i + wb.len() <= bytes.len() {
        if &bytes[i..i + wb.len()] == wb {
            let before_ok = i == 0 || !is_word_byte(bytes[i - 1]);
            let after = i + wb.len();
            let after_ok = after >= bytes.len() || !is_word_byte(bytes[after]);
            if before_ok && after_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn symbol_kind_name(kind: i64) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enumMember",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "typeParameter",
        _ => "unknown",
    }
}

fn normalize_kind_filter(kinds: &[String]) -> HashSet<String> {
    let mut out = HashSet::new();
    for k in kinds {
        let lower = k.trim().to_lowercase();
        if lower.is_empty() {
            continue;
        }
        let aliases: &[&str] = match lower.as_str() {
            "theorem" => &["theorem", "method"],
            "lemma" => &["lemma", "method"],
            "def" | "definition" => &["def", "definition", "method", "function"],
            "instance" => &["instance", "method"],
            "structure" => &["structure", "class", "struct"],
            "class" => &["class"],
            "inductive" => &["inductive", "enum"],
            "abbrev" => &["abbrev", "method", "function"],
            "namespace" | "section" => &["namespace"],
            "variable" => &["variable"],
            "constant" => &["constant"],
            "axiom" => &["axiom", "constant"],
            "opaque" => &["opaque", "constant"],
            _ => &[],
        };
        if aliases.is_empty() {
            out.insert(lower);
        } else {
            for a in aliases {
                out.insert((*a).to_string());
            }
        }
    }
    out
}

fn walk_lean_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                let name = entry.file_name();
                if DIR_SKIP.iter().any(|d| std::ffi::OsStr::new(d) == name) {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file() && path.extension().is_some_and(|e| e == "lean") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(severity: Option<i32>, message: &str) -> WireDiagnostic {
        WireDiagnostic {
            range: WireRange {
                start: WirePosition { line: 0, character: 0 },
                end: WirePosition { line: 0, character: 1 },
            },
            severity,
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
    fn severity_defaults_to_error_only() {
        assert_eq!(parse_severity_filter(None), Some(SeverityFilter::Only(HashSet::from([1]))));
        assert_eq!(parse_severity_filter(Some("  ")), Some(SeverityFilter::Only(HashSet::from([1]))));
    }

    #[test]
    fn severity_maps_names_and_all_and_invalid() {
        assert_eq!(
            parse_severity_filter(Some("error,warn,info")),
            Some(SeverityFilter::Only(HashSet::from([1, 2, 3])))
        );
        assert_eq!(parse_severity_filter(Some("hint")), Some(SeverityFilter::Only(HashSet::from([4]))));
        assert_eq!(parse_severity_filter(Some("all")), Some(SeverityFilter::All));
        assert_eq!(parse_severity_filter(Some("error,all")), Some(SeverityFilter::All));
        assert_eq!(parse_severity_filter(Some("bogus")), None);
    }

    #[test]
    fn keep_by_severity_rules() {
        assert!(keep_by_severity(&diag(Some(3), ""), &SeverityFilter::All));
        assert!(keep_by_severity(&diag(None, ""), &SeverityFilter::Only(HashSet::from([1]))));
        assert!(!keep_by_severity(&diag(None, ""), &SeverityFilter::Only(HashSet::from([2]))));
        assert!(keep_by_severity(&diag(Some(2), ""), &SeverityFilter::Only(HashSet::from([2]))));
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
        assert_eq!(extract_conclusion("⊢ a =\n  b").as_deref(), Some("a =\n  b"));
        assert_eq!(extract_conclusion("no turnstile here"), None);
    }

    #[test]
    fn collect_notes_flags_imports_and_sorries() {
        let notes = collect_notes(&[diag(None, "Imports are out of date and must be rebuilt")], None, &[]);
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
            start: WirePosition { line: 0, character: 0 },
            end: WirePosition { line: 3, character: 0 },
        };
        let lines = sorry_note_lines(
            &[sorry(1, SorryKind::Tactic), sorry(50, SorryKind::Tactic)],
            Some(&target),
        );
        assert_eq!(lines, vec![2]);
    }

    #[test]
    fn classify_reads_keyword_from_source() {
        assert_eq!(classify_decl_kind(&["theorem foo : True"], 0), "theorem");
        assert_eq!(classify_decl_kind(&["lemma bar"], 0), "lemma");
        assert_eq!(classify_decl_kind(&["#check x"], 0), "method");
    }
}
