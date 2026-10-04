use arc_swap::ArcSwap;
use crossbeam_channel::unbounded;
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, HoverParams,
    HoverProviderCapability, InitializeResult, PublishDiagnosticsParams, ServerCapabilities,
    ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use crate::catalog::Catalog;
use crate::lsp::diagnostics::{clear_document_diagnostics, validate_document};
use crate::lsp::document::Document;
use crate::lsp::hover::resolve_document_hover;

#[derive(Debug, Clone)]
struct AnalysisJob {
    uri: Uri,
    version: i32,
    job_id: u64,
}

fn cast_req<P>(req: Request) -> Result<(RequestId, P), Box<dyn std::error::Error>>
where
    P: serde::de::DeserializeOwned,
{
    let p = serde_json::from_value(req.params)?;
    Ok((req.id, p))
}

fn cast_notif<P>(notif: Notification) -> Result<P, Box<dyn std::error::Error>>
where
    P: serde::de::DeserializeOwned,
{
    let p = serde_json::from_value(notif.params)?;
    Ok(p)
}

/// Runs the modular LSP server over stdio with incremental sync.
pub fn run_lsp_server<P: AsRef<Path>>(migrations_dir: P) -> Result<(), Box<dyn std::error::Error>> {
    let migrations_path = migrations_dir.as_ref().to_path_buf();

    let (connection, io_threads) = Connection::stdio();

    let init_result = InitializeResult {
        capabilities: ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                TextDocumentSyncKind::INCREMENTAL,
            )),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            ..Default::default()
        },
        server_info: Some(ServerInfo {
            name: "sqltype-lsp".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };

    let _init_params = connection.initialize(serde_json::to_value(init_result)?)?;

    // In-memory VFS mapping uri_string -> Document backed by ropey::Rope
    let vfs: Arc<RwLock<HashMap<String, Document>>> = Arc::new(RwLock::new(HashMap::new()));

    // Schema catalog snapshot with lock-free atomic swap
    let initial_catalog = Catalog::load_from_dir(&migrations_path).unwrap_or_default();
    let catalog = Arc::new(ArcSwap::from_pointee(initial_catalog));

    // Channels and job counter for background analysis worker
    let (job_sender, job_receiver) = unbounded::<AnalysisJob>();
    let latest_job_counter = Arc::new(AtomicU64::new(0));
    let uri_latest_jobs: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));

    // Spawn analysis worker thread
    {
        let catalog = Arc::clone(&catalog);
        let uri_latest_jobs = Arc::clone(&uri_latest_jobs);
        let vfs = Arc::clone(&vfs);
        let msg_sender = connection.sender.clone();

        std::thread::spawn(move || {
            while let Ok(job) = job_receiver.recv() {
                // Superseded job cancellation check
                {
                    let guard = uri_latest_jobs.lock().unwrap();
                    if let Some(&latest) = guard.get(job.uri.as_str())
                        && job.job_id < latest
                    {
                        continue;
                    }
                }

                let doc_opt = {
                    let vfs_guard = vfs.read().unwrap();
                    vfs_guard.get(job.uri.as_str()).cloned()
                };

                if let Some(doc) = doc_opt {
                    let catalog_snapshot = catalog.load();
                    let diagnostics = validate_document(&doc, &catalog_snapshot);

                    let notif = Notification {
                        method: "textDocument/publishDiagnostics".to_string(),
                        params: serde_json::to_value(PublishDiagnosticsParams {
                            uri: job.uri,
                            diagnostics,
                            version: Some(job.version),
                        })
                        .unwrap(),
                    };
                    let _ = msg_sender.send(Message::Notification(notif));
                }
            }
        });
    }

    // Spawn migration file watcher thread
    {
        let catalog = Arc::clone(&catalog);
        let vfs = Arc::clone(&vfs);
        let job_sender = job_sender.clone();
        let latest_job_counter = Arc::clone(&latest_job_counter);
        let uri_latest_jobs = Arc::clone(&uri_latest_jobs);
        let migrations_path_clone = migrations_path.clone();

        std::thread::spawn(move || {
            if !migrations_path_clone.exists() {
                return;
            }

            let (tx, rx) = std::sync::mpsc::channel();
            let mut watcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(_) => return,
            };

            if watcher
                .watch(&migrations_path_clone, RecursiveMode::Recursive)
                .is_err()
            {
                return;
            }

            while let Ok(res) = rx.recv() {
                if let Ok(event) = res {
                    let has_sql = event.paths.iter().any(|p| {
                        p.extension()
                            .map(|e| e.eq_ignore_ascii_case("sql"))
                            .unwrap_or(false)
                    });

                    if has_sql && let Ok(new_cat) = Catalog::load_from_dir(&migrations_path_clone) {
                        catalog.store(Arc::new(new_cat));

                        // Re-trigger analysis on all active open buffers
                        let vfs_docs: Vec<(String, i32)> = {
                            let vfs_guard = vfs.read().unwrap();
                            vfs_guard
                                .values()
                                .map(|d| (d.uri.clone(), d.version))
                                .collect()
                        };

                        for (uri_str, version) in vfs_docs {
                            if let Ok(uri) = uri_str.parse::<Uri>() {
                                let job_id = latest_job_counter.fetch_add(1, Ordering::SeqCst) + 1;
                                uri_latest_jobs.lock().unwrap().insert(uri_str, job_id);
                                let _ = job_sender.send(AnalysisJob {
                                    uri,
                                    version,
                                    job_id,
                                });
                            }
                        }
                    }
                }
            }
        });
    }

    // Main LSP server event loop
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }

                if req.method == "textDocument/hover" {
                    let (id, params) = cast_req::<HoverParams>(req)?;
                    let uri_str = params
                        .text_document_position_params
                        .text_document
                        .uri
                        .to_string();

                    let vfs_guard = vfs.read().unwrap();
                    let hover_res = if let Some(doc) = vfs_guard.get(&uri_str) {
                        let cat_snapshot = catalog.load();
                        resolve_document_hover(
                            doc,
                            params.text_document_position_params.position,
                            &cat_snapshot,
                        )
                    } else {
                        None
                    };

                    let resp = Response {
                        id,
                        result: Some(serde_json::to_value(hover_res)?),
                        error: None,
                    };
                    connection.sender.send(Message::Response(resp))?;
                }
            }
            Message::Notification(notif) => match notif.method.as_str() {
                "textDocument/didOpen" => {
                    if let Ok(params) = cast_notif::<DidOpenTextDocumentParams>(notif) {
                        let uri = params.text_document.uri;
                        let uri_str = uri.to_string();
                        let version = params.text_document.version;
                        let text = params.text_document.text;

                        let doc = Document::new(uri_str.clone(), version, &text);
                        vfs.write().unwrap().insert(uri_str.clone(), doc);

                        let job_id = latest_job_counter.fetch_add(1, Ordering::SeqCst) + 1;
                        uri_latest_jobs.lock().unwrap().insert(uri_str, job_id);
                        let _ = job_sender.send(AnalysisJob {
                            uri,
                            version,
                            job_id,
                        });
                    }
                }
                "textDocument/didChange" => {
                    if let Ok(params) = cast_notif::<DidChangeTextDocumentParams>(notif) {
                        let uri = params.text_document.uri;
                        let uri_str = uri.to_string();
                        let version = params.text_document.version;

                        let mut vfs_guard = vfs.write().unwrap();
                        if let Some(doc) = vfs_guard.get_mut(&uri_str) {
                            let has_sql_change = doc.apply_content_changes(params.content_changes);
                            doc.version = version;

                            if has_sql_change {
                                let job_id = latest_job_counter.fetch_add(1, Ordering::SeqCst) + 1;
                                uri_latest_jobs.lock().unwrap().insert(uri_str, job_id);
                                let _ = job_sender.send(AnalysisJob {
                                    uri,
                                    version,
                                    job_id,
                                });
                            }
                        }
                    }
                }
                "textDocument/didClose" => {
                    if let Ok(params) = cast_notif::<DidCloseTextDocumentParams>(notif) {
                        let uri = params.text_document.uri;
                        let uri_str = uri.to_string();
                        vfs.write().unwrap().remove(&uri_str);
                        uri_latest_jobs.lock().unwrap().remove(&uri_str);

                        // Clear diagnostics for closed document
                        let clear_params = clear_document_diagnostics(&uri);
                        let clear_notif = Notification {
                            method: "textDocument/publishDiagnostics".to_string(),
                            params: serde_json::to_value(clear_params).unwrap(),
                        };
                        let _ = connection.sender.send(Message::Notification(clear_notif));
                    }
                }
                _ => {}
            },
            Message::Response(_) => {}
        }
    }

    io_threads.join()?;
    Ok(())
}
