//! LeanSession is the only query-facing LSP API. Standard methods use lsp-types;
//! the few Lean extensions have explicit request/notification models.
use super::{extensions as lean, io::SessionIo};
use crate::{
    error::{Error, Result},
    request::RequestContext,
    state::ElaborationStore,
};
use leanck_core::api::{GoalResult, Location, Position};
use lsp_server::{ErrorCode, Response};
use lsp_types::{
    self as lsp,
    notification::{self as note, Notification},
    request::{self as req, Request},
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

struct PeerState {
    generation: u64,
    state: Arc<ElaborationStore>,
    folders: Mutex<Vec<lsp::WorkspaceFolder>>,
}
pub struct LeanSession {
    io: Arc<SessionIo>,
    peer: Arc<PeerState>,
}
impl LeanSession {
    pub fn spawn<R, W>(reader: R, writer: W, state: Arc<ElaborationStore>) -> Arc<Self>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let peer = Arc::new(PeerState {
            generation: state.generation(),
            state,
            folders: Mutex::new(Vec::new()),
        });
        let notifications = peer.clone();
        let requests = peer.clone();
        let io = SessionIo::spawn(
            reader,
            writer,
            Arc::new(move |notification| notifications.notify(notification)),
            Arc::new(move |request| requests.request(request)),
        );
        Arc::new(Self { io, peer })
    }
    async fn query<R: Request>(
        &self,
        params: R::Params,
        ctx: &RequestContext,
        cap: Duration,
    ) -> Result<R::Result> {
        let budget = ctx.remaining()?.min(cap);
        ctx.run(async {
            tokio::time::timeout(budget, self.io.request::<R>(params))
                .await
                .map_err(|_| Error::Timeout)?
        })
        .await
    }
    pub async fn initialize(
        &self,
        worktree: &Path,
        ctx: &RequestContext,
    ) -> Result<lsp::InitializeResult> {
        let uri = uri(&leanck_core::document::path_to_uri(worktree)?)?;
        let folder = lsp::WorkspaceFolder {
            uri: uri.clone(),
            name: worktree
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        *self.peer.folders.lock().unwrap() = vec![folder.clone()];
        #[allow(deprecated)]
        let params = lsp::InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(uri),
            root_path: Some(worktree.to_string_lossy().into_owned()),
            capabilities: capabilities(),
            workspace_folders: Some(vec![folder]),
            initialization_options: Some(serde_json::json!({"hasWidgets":true})),
            ..Default::default()
        };
        let result = self
            .query::<req::Initialize>(params, ctx, Duration::from_secs(60))
            .await?;
        self.io
            .notify::<note::Initialized>(lsp::InitializedParams {})?;
        Ok(result)
    }
    pub fn open(
        &self,
        document: &str,
        text: &str,
        version: i64,
        mode: Option<lean::DependencyBuildMode>,
    ) -> Result<()> {
        self.io.notify::<lean::OpenDocument>(lean::OpenParams {
            text_document: lsp::TextDocumentItem {
                uri: uri(document)?,
                language_id: "lean".into(),
                version: version_number(version)?,
                text: text.into(),
            },
            dependency_build_mode: mode,
        })
    }
    pub fn change(&self, document: &str, text: &str, version: i64) -> Result<()> {
        self.io
            .notify::<note::DidChangeTextDocument>(lsp::DidChangeTextDocumentParams {
                text_document: lsp::VersionedTextDocumentIdentifier {
                    uri: uri(document)?,
                    version: version_number(version)?,
                },
                content_changes: vec![lsp::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: text.into(),
                }],
            })
    }
    pub fn close(&self, document: &str) -> Result<()> {
        self.io
            .notify::<note::DidCloseTextDocument>(lsp::DidCloseTextDocumentParams {
                text_document: document_id(document)?,
            })
    }
    pub async fn goal(
        &self,
        document: &str,
        position: Position,
        ctx: &RequestContext,
        cap: Duration,
    ) -> Result<GoalResult> {
        let mut goal = self
            .query::<lean::PlainGoal>(
                lsp::TextDocumentPositionParams {
                    text_document: document_id(document)?,
                    position: lsp::Position::new(position.line, position.character),
                },
                ctx,
                cap,
            )
            .await?;
        if let GoalResult::Goals(goals) = &mut goal {
            goals.extensions.remove("rendered");
        }
        Ok(goal)
    }
    pub async fn wait_for_diagnostics(
        &self,
        document: &str,
        version: i64,
        ctx: &RequestContext,
        cap: Duration,
    ) -> Result<()> {
        self.query::<lean::WaitForDiagnostics>(
            lean::DiagnosticsVersion {
                uri: uri(document)?,
                version,
            },
            ctx,
            cap,
        )
        .await
        .map(|_| ())
    }
    pub async fn document_symbols(
        &self,
        document: &str,
        ctx: &RequestContext,
        cap: Duration,
    ) -> Result<Option<lean::SymbolResponse>> {
        self.query::<lean::DocumentSymbols>(
            lsp::DocumentSymbolParams {
                text_document: document_id(document)?,
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            },
            ctx,
            cap,
        )
        .await
    }
    pub async fn folding_ranges(
        &self,
        document: &str,
        ctx: &RequestContext,
        cap: Duration,
    ) -> Result<Vec<lsp::FoldingRange>> {
        self.query::<req::FoldingRangeRequest>(
            lsp::FoldingRangeParams {
                text_document: document_id(document)?,
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            },
            ctx,
            cap,
        )
        .await
        .map(Option::unwrap_or_default)
    }
    pub async fn references(
        &self,
        document: &str,
        position: Position,
        ctx: &RequestContext,
        cap: Duration,
    ) -> Result<Vec<Location>> {
        let result = self
            .query::<req::References>(
                lsp::ReferenceParams {
                    text_document_position: lsp::TextDocumentPositionParams {
                        text_document: document_id(document)?,
                        position: lsp::Position::new(position.line, position.character),
                    },
                    context: lsp::ReferenceContext {
                        include_declaration: false,
                    },
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                },
                ctx,
                cap,
            )
            .await?;
        Ok(result
            .unwrap_or_default()
            .into_iter()
            .map(|location| Location {
                uri: location.uri.into(),
                range: range(location.range),
            })
            .collect())
    }
    pub fn dead_token(&self) -> CancellationToken {
        self.io.dead_token()
    }
    pub fn is_dead(&self) -> bool {
        self.io.is_dead()
    }
    pub async fn cooperative_shutdown(&self, ctx: &RequestContext) -> Result<()> {
        let result = self
            .query::<req::Shutdown>((), ctx, Duration::from_secs(2))
            .await;
        let _ = self.io.notify::<note::Exit>(());
        if result.is_ok() {
            ctx.run(self.io.flush()).await?;
        }
        result
    }
    pub async fn shutdown(&self) {
        self.io.shutdown().await;
    }
}
fn version_number(version: i64) -> Result<i32> {
    i32::try_from(version)
        .map_err(|_| Error::BadRequest("document version exceeds LSP's signed 32-bit range".into()))
}
fn uri(value: &str) -> Result<lsp::Url> {
    lsp::Url::parse(value).map_err(|error| Error::InvalidPath(error.to_string()))
}
fn document_id(value: &str) -> Result<lsp::TextDocumentIdentifier> {
    Ok(lsp::TextDocumentIdentifier { uri: uri(value)? })
}
pub(crate) fn range(range: lsp::Range) -> leanck_core::api::Range {
    leanck_core::api::Range {
        start: Position {
            line: range.start.line,
            character: range.start.character,
        },
        end: Position {
            line: range.end.line,
            character: range.end.character,
        },
    }
}
fn capabilities() -> lsp::ClientCapabilities {
    lsp::ClientCapabilities {
        text_document: Some(lsp::TextDocumentClientCapabilities {
            synchronization: Some(lsp::TextDocumentSyncClientCapabilities {
                did_save: Some(true),
                dynamic_registration: Some(false),
                ..Default::default()
            }),
            publish_diagnostics: Some(lsp::PublishDiagnosticsClientCapabilities {
                related_information: Some(true),
                ..Default::default()
            }),
            document_symbol: Some(lsp::DocumentSymbolClientCapabilities {
                hierarchical_document_symbol_support: Some(true),
                ..Default::default()
            }),
            folding_range: Some(lsp::FoldingRangeClientCapabilities {
                line_folding_only: Some(false),
                ..Default::default()
            }),
            hover: Some(lsp::HoverClientCapabilities {
                content_format: Some(vec![lsp::MarkupKind::PlainText]),
                ..Default::default()
            }),
            ..Default::default()
        }),
        window: Some(lsp::WindowClientCapabilities {
            work_done_progress: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    }
}
impl PeerState {
    fn notify(&self, notification: lsp_server::Notification) -> Result<()> {
        match notification.method.as_str() {
            note::PublishDiagnostics::METHOD => {
                let params: lean::Diagnostics = serde_json::from_value(notification.params)?;
                let params = params.normalize();
                self.state.diagnostics_for(
                    self.generation,
                    params.uri.as_str(),
                    params.version,
                    params.diagnostics,
                );
            }
            lean::ProgressNotification::METHOD => {
                let params: lean::FileProgress = serde_json::from_value(notification.params)?;
                self.state.progress_for(
                    self.generation,
                    params.text_document.uri.as_str(),
                    params.text_document.version,
                    params.processing.into_iter().map(|p| p.range).collect(),
                );
            }
            // Progress/log notifications do not change elaboration state.
            _ => {}
        }
        Ok(())
    }
    fn request(&self, request: lsp_server::Request) -> Response {
        match request.method.as_str() {
            req::WorkDoneProgressCreate::METHOD => {
                answer::<req::WorkDoneProgressCreate>(request, |_| ())
            }
            req::WorkspaceConfiguration::METHOD => {
                answer::<req::WorkspaceConfiguration>(request, |p| {
                    vec![serde_json::Value::Null; p.items.len()]
                })
            }
            req::WorkspaceFoldersRequest::METHOD => {
                answer::<req::WorkspaceFoldersRequest>(request, |_| {
                    Some(self.folders.lock().unwrap().clone())
                })
            }
            // Dynamic registration and editor mutations are not advertised or implemented.
            _ => Response::new_err(
                request.id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported Lean server request: {}", request.method),
            ),
        }
    }
}
fn answer<R: Request>(
    request: lsp_server::Request,
    handle: impl FnOnce(R::Params) -> R::Result,
) -> Response {
    match serde_json::from_value::<R::Params>(request.params) {
        Ok(params) => Response::new_ok(request.id, handle(params)),
        Err(error) => Response::new_err(
            request.id,
            ErrorCode::InvalidParams as i32,
            error.to_string(),
        ),
    }
}
#[cfg(test)]
mod tests;
