//! Real-time External CLI Agent Bridge & Live Scene File Watcher for Kine Studio.
//!
//! Replaces the legacy in-app AI chat panel with a zero-overhead, bidirectional
//! TCP bridge (`127.0.0.1:7701` default) and automatic disk hot-reload watcher.
//! External CLI agents run in their own terminal/window and control Kine Studio
//! in real time via `kine agent <subcommand>` or direct file edits.

use crate::agent_tools::{agent_tools_json_schema, dispatch_agent_tool, AgentToolResult};
use crate::state::{StudioError, StudioState};
use kine_script::SceneDef;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

pub const DEFAULT_BRIDGE_HOST: &str = "127.0.0.1";
pub const DEFAULT_BRIDGE_PORT: u16 = 7701;
pub const BRIDGE_MANIFEST_FILENAME: &str = ".kine-studio-bridge.json";

/// Discovery manifest written by a running Kine Studio instance so `kine agent`
/// CLI commands automatically locate the active window without manual port flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeDiscoveryManifest {
    pub pid: u32,
    pub addr: String,
    pub host: String,
    pub port: u16,
    pub workspace_root: Option<PathBuf>,
    pub active_scene: Option<PathBuf>,
    pub protocol: String,
}

/// Real-time event broadcast to connected `kine agent watch` CLI subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEvent {
    pub event: String,
    pub frame_revision: u64,
    pub current_time: f64,
    pub frame: u32,
    pub selected_node: Option<u64>,
    pub summary: String,
    pub payload: Value,
}

struct BridgeRequest {
    tool: String,
    args: Value,
    reply_tx: Sender<AgentToolResult>,
}

/// Live non-blocking TCP bridge server hosted inside `StudioState`.
pub struct StudioBridgeServer {
    addr: String,
    rx: Receiver<BridgeRequest>,
    has_pending: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    subscribers: Arc<Mutex<Vec<TcpStream>>>,
    manifest_paths: Vec<PathBuf>,
    pub commands_handled: u64,
    pub last_command_summary: Option<String>,
}

impl StudioBridgeServer {
    /// Start the background TCP bridge listener on `requested_addr` (or auto-probe `127.0.0.1:7701..=7716`).
    pub fn start(
        requested_addr: Option<&str>,
        workspace_root: Option<&Path>,
        active_scene: Option<&Path>,
    ) -> Result<Self, std::io::Error> {
        let listener = if let Some(explicit) = requested_addr {
            TcpListener::bind(explicit)?
        } else if let Ok(env_addr) = std::env::var("KINE_STUDIO_BRIDGE") {
            TcpListener::bind(env_addr.trim())?
        } else {
            let mut bound = None;
            let mut last_err = None;
            for port in DEFAULT_BRIDGE_PORT..=(DEFAULT_BRIDGE_PORT + 15) {
                let candidate = format!("{DEFAULT_BRIDGE_HOST}:{port}");
                match TcpListener::bind(&candidate) {
                    Ok(l) => {
                        bound = Some(l);
                        break;
                    }
                    Err(e) => last_err = Some(e),
                }
            }
            match bound {
                Some(l) => l,
                None => {
                    return Err(last_err.unwrap_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::AddrInUse,
                            "no bridge ports available",
                        )
                    }))
                }
            }
        };

        let local_addr = listener.local_addr()?;
        let addr_str = local_addr.to_string();
        let host = local_addr.ip().to_string();
        let port = local_addr.port();

        let (tx, rx) = mpsc::channel::<BridgeRequest>();
        let has_pending = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let subscribers: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));

        let manifest = BridgeDiscoveryManifest {
            pid: std::process::id(),
            addr: addr_str.clone(),
            host,
            port,
            workspace_root: workspace_root.map(Path::to_path_buf),
            active_scene: active_scene.map(Path::to_path_buf),
            protocol: "kine-studio-bridge-v1".to_string(),
        };

        let mut manifest_paths = Vec::new();
        if let Ok(json_bytes) = serde_json::to_vec_pretty(&manifest) {
            if let Some(ws) = workspace_root {
                let p = ws.join(BRIDGE_MANIFEST_FILENAME);
                if fs::write(&p, &json_bytes).is_ok() {
                    manifest_paths.push(p);
                }
            }
            let tmp_p = std::env::temp_dir().join(BRIDGE_MANIFEST_FILENAME);
            if fs::write(&tmp_p, &json_bytes).is_ok() {
                manifest_paths.push(tmp_p);
            }
        }

        let has_pending_thread = Arc::clone(&has_pending);
        let shutdown_thread = Arc::clone(&shutdown);
        let subscribers_thread = Arc::clone(&subscribers);

        std::thread::Builder::new()
            .name("kine-studio-bridge".to_string())
            .spawn(move || {
                for incoming in listener.incoming() {
                    if shutdown_thread.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = incoming else {
                        continue;
                    };
                    let tx_conn = tx.clone();
                    let pending_conn = Arc::clone(&has_pending_thread);
                    let subs_conn = Arc::clone(&subscribers_thread);
                    std::thread::spawn(move || {
                        let _ = handle_bridge_client(stream, tx_conn, pending_conn, subs_conn);
                    });
                }
            })?;

        Ok(Self {
            addr: addr_str,
            rx,
            has_pending,
            shutdown,
            subscribers,
            manifest_paths,
            commands_handled: 0,
            last_command_summary: None,
        })
    }

    /// Bound address (`127.0.0.1:7701`) of this live Studio Bridge.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Returns `true` if one or more CLI commands are waiting to be executed on `StudioState`.
    pub fn has_pending(&self) -> bool {
        self.has_pending.load(Ordering::Acquire)
    }

    /// Shared atomic flag used by the UI event loop to wake immediately when a CLI command arrives.
    pub fn pending_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.has_pending)
    }

    /// Number of active `kine agent watch` event stream subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.lock().map_or(0, |s| s.len())
    }

    /// Broadcast a real-time `BridgeEvent` to all connected `kine agent watch` clients.
    pub fn broadcast_event(&self, event: &BridgeEvent) {
        let Ok(mut subs) = self.subscribers.lock() else {
            return;
        };
        if subs.is_empty() {
            return;
        }
        let Ok(line) = serde_json::to_string(event) else {
            return;
        };
        let payload = format!("{line}\n");
        subs.retain_mut(|stream| {
            stream
                .write_all(payload.as_bytes())
                .and_then(|_| stream.flush())
                .is_ok()
        });
    }
}

impl Drop for StudioBridgeServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = TcpStream::connect(&self.addr);
        for p in &self.manifest_paths {
            let _ = fs::remove_file(p);
        }
    }
}

fn handle_bridge_client(
    stream: TcpStream,
    tx: Sender<BridgeRequest>,
    has_pending: Arc<AtomicBool>,
    subscribers: Arc<Mutex<Vec<TcpStream>>>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parsed: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let err_resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "ok": false,
                    "error": { "code": -32700, "message": format!("parse error: {e}") }
                });
                writeln!(writer, "{err_resp}")?;
                writer.flush()?;
                continue;
            }
        };

        let rpc_id = parsed.get("id").cloned();
        let is_jsonrpc = parsed.get("jsonrpc").is_some();
        let method = parsed
            .get("method")
            .or_else(|| parsed.get("tool"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let args = parsed
            .get("params")
            .or_else(|| parsed.get("args"))
            .cloned()
            .unwrap_or_else(|| parsed.clone());

        if method == "ping" {
            let resp = if is_jsonrpc {
                json!({ "jsonrpc": "2.0", "id": rpc_id, "result": "pong", "ok": true })
            } else {
                json!({ "ok": true, "stage": "verified", "tool": "ping", "summary": "pong", "payload": {} })
            };
            writeln!(writer, "{resp}")?;
            writer.flush()?;
            continue;
        }

        if method == "schema" || method == "tools/list" || method == "studio_schema" {
            let schema = agent_tools_json_schema();
            let resp = if is_jsonrpc {
                json!({ "jsonrpc": "2.0", "id": rpc_id, "result": schema, "ok": true })
            } else {
                json!({
                    "ok": true,
                    "stage": "verified",
                    "tool": "studio_schema",
                    "summary": "Returned JSON schema for studio_* tools",
                    "payload": { "tools": schema }
                })
            };
            writeln!(writer, "{resp}")?;
            writer.flush()?;
            continue;
        }

        if method == "subscribe" || method == "watch" {
            let ack = json!({
                "event": "subscribed",
                "ok": true,
                "summary": "Subscribed to live Kine Studio bridge events"
            });
            writeln!(writer, "{ack}")?;
            writer.flush()?;
            if let Ok(mut subs) = subscribers.lock() {
                subs.push(writer);
            }
            return Ok(());
        }

        let tool_name = if method.starts_with("studio_") {
            method
        } else {
            format!("studio_{method}")
        };

        let (reply_tx, reply_rx) = mpsc::channel();
        if tx
            .send(BridgeRequest {
                tool: tool_name.clone(),
                args,
                reply_tx,
            })
            .is_err()
        {
            let err_resp = json!({
                "ok": false,
                "stage": "refused",
                "tool": tool_name,
                "summary": "Studio bridge channel closed",
                "payload": { "error": "Studio bridge channel closed" }
            });
            writeln!(writer, "{err_resp}")?;
            writer.flush()?;
            return Ok(());
        }

        has_pending.store(true, Ordering::Release);

        let result = match reply_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(res) => res,
            Err(_) => AgentToolResult::refused(
                "bridge_timeout",
                "Timed out waiting for Studio UI loop to process bridge command",
            ),
        };

        let out_json = if is_jsonrpc {
            json!({
                "jsonrpc": "2.0",
                "id": rpc_id,
                "ok": result.ok,
                "result": result,
            })
        } else {
            serde_json::to_value(&result).unwrap_or(Value::Null)
        };
        writeln!(writer, "{out_json}")?;
        writer.flush()?;
    }
}

/// Lightweight file watcher that detects external edits to the active scene file
/// (`scene.ts` / `scene.json`) and project `Modules/` directory.
#[derive(Debug, Clone)]
pub struct SceneFileWatcher {
    pub watched_path: Option<PathBuf>,
    last_fingerprint: u64,
    last_check: Instant,
    pub reload_count: u64,
}

impl Default for SceneFileWatcher {
    fn default() -> Self {
        Self {
            watched_path: None,
            last_fingerprint: 0,
            last_check: Instant::now(),
            reload_count: 0,
        }
    }
}

impl SceneFileWatcher {
    pub fn watch(&mut self, path: Option<PathBuf>, module_dir: Option<&Path>) {
        self.watched_path = path;
        self.last_fingerprint = compute_watch_fingerprint(self.watched_path.as_deref(), module_dir);
        self.last_check = Instant::now();
    }

    pub fn record_self_write(&mut self, module_dir: Option<&Path>) {
        self.last_fingerprint = compute_watch_fingerprint(self.watched_path.as_deref(), module_dir);
        self.last_check = Instant::now();
    }

    /// Check at most every 180ms whether the watched scene file or `Modules/` changed on disk.
    pub fn check_changed(&mut self, module_dir: Option<&Path>, force: bool) -> Option<PathBuf> {
        let path = self.watched_path.as_ref()?.clone();
        if !force && self.last_check.elapsed() < Duration::from_millis(180) {
            return None;
        }
        self.last_check = Instant::now();
        let current_fp = compute_watch_fingerprint(Some(&path), module_dir);
        if current_fp != 0 && current_fp != self.last_fingerprint {
            self.last_fingerprint = current_fp;
            self.reload_count += 1;
            return Some(path);
        }
        None
    }
}

fn fnv1a_64(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = if seed == 0 { 0xcbf29ce484222325 } else { seed };
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn compute_watch_fingerprint(scene_path: Option<&Path>, module_dir: Option<&Path>) -> u64 {
    let Some(path) = scene_path else {
        return 0;
    };
    let Ok(bytes) = fs::read(path) else {
        return 0;
    };
    let mut hash = fnv1a_64(&bytes, 0);

    if let Some(base) = module_dir {
        let mod_root = base.join(kine_script::MODULES_DIR);
        if let Ok(entries) = fs::read_dir(mod_root) {
            let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            dirs.sort();
            for d in dirs {
                for file_name in ["module.json", "scene.ts"] {
                    let fp = d.join(file_name);
                    if let Ok(b) = fs::read(&fp) {
                        hash = fnv1a_64(fp.to_string_lossy().as_bytes(), hash);
                        hash = fnv1a_64(&b, hash);
                    }
                }
            }
        }
    }
    hash
}

impl StudioState {
    /// Start the live TCP Bridge Server for external CLI agents on `addr` (or default `127.0.0.1:7701`).
    pub fn start_bridge_server(&mut self, addr: Option<&str>) -> Result<String, std::io::Error> {
        let scene_path = self.watched_scene_path();
        let server =
            StudioBridgeServer::start(addr, self.workspace_root.as_deref(), scene_path.as_deref())?;
        let bound = server.addr().to_string();
        self.bridge = Some(server);
        self.notify_status(format!("CLI Agent Bridge active on {bound}"));
        Ok(bound)
    }

    /// Stop the live TCP Bridge Server if running.
    pub fn stop_bridge_server(&mut self) {
        self.bridge = None;
        self.notify_status("CLI Agent Bridge stopped");
    }

    /// Toggle the live TCP Bridge Server on or off from the Studio header bar.
    pub fn toggle_bridge_server(&mut self) {
        if self.bridge.is_some() {
            self.stop_bridge_server();
        } else if let Err(e) = self.start_bridge_server(None) {
            self.notify_status(format!("Bridge start failed: {e}"));
        }
    }

    /// Bound address of the live TCP Bridge Server, if active.
    pub fn bridge_listen_addr(&self) -> Option<String> {
        self.bridge.as_ref().map(|b| b.addr().to_string())
    }

    /// Currently watched scene file path on disk, if any.
    pub fn watched_scene_path(&self) -> Option<PathBuf> {
        self.file_watcher.watched_path.clone().or_else(|| {
            self.compositions
                .get(self.active_composition)
                .and_then(|c| c.source_path.clone())
        })
    }

    /// Poll pending external CLI bridge commands and disk file changes, executing them
    /// in-process and returning the number of updates applied.
    pub fn poll_bridge_and_watcher(&mut self) -> usize {
        let mut updates = 0usize;

        // 1. Drain all queued commands from external CLI agents
        let mut pending_reqs = Vec::new();
        if let Some(bridge) = &self.bridge {
            while let Ok(req) = bridge.rx.try_recv() {
                pending_reqs.push(req);
            }
            bridge.has_pending.store(false, Ordering::Release);
        }

        for req in pending_reqs {
            let prev_rev = self.frame_revision;
            let prev_sel = self.selected_node;
            let prev_time = self.current_time;

            let result = dispatch_agent_tool(self, &req.tool, &req.args);

            if self.settings.auto_save_scene && result.ok && self.frame_revision != prev_rev {
                if let Some(path) = self.watched_scene_path() {
                    if path.extension().and_then(|s| s.to_str()) == Some("json") {
                        let _ = self.save_scene_def_to_disk(Some(&path));
                    }
                }
            }

            let event_revision = self.frame_revision;
            let event_time = self.current_time;
            let event_frame = self.current_frame_index();
            let event_selected = self.selected_node.map(|n| n.0);
            if let Some(bridge) = &mut self.bridge {
                bridge.commands_handled += 1;
                bridge.last_command_summary = Some(format!("{}: {}", result.tool, result.summary));

                let event_kind = if event_revision != prev_rev {
                    "scene_mutated"
                } else if event_selected != prev_sel.map(|n| n.0) {
                    "selection_changed"
                } else if (event_time - prev_time).abs() > 1e-6 {
                    "playhead_changed"
                } else {
                    "tool_executed"
                };

                bridge.broadcast_event(&BridgeEvent {
                    event: event_kind.to_string(),
                    frame_revision: event_revision,
                    current_time: event_time,
                    frame: event_frame,
                    selected_node: event_selected,
                    summary: result.summary.clone(),
                    payload: json!({
                        "tool": result.tool,
                        "ok": result.ok,
                        "stage": result.stage,
                    }),
                });
            }

            let _ = req.reply_tx.send(result);
            updates += 1;
        }

        // 2. Check if an external agent modified the active scene file on disk
        let mod_base = self.workspace_root.clone();
        if self
            .file_watcher
            .check_changed(mod_base.as_deref(), false)
            .is_some()
        {
            if self.reload_watched_scene_from_disk(false).is_ok() {
                updates += 1;
            }
        }

        updates
    }

    /// Reload the active composition's scene file from disk while preserving playhead and selection.
    pub fn reload_watched_scene_from_disk(&mut self, force: bool) -> Result<PathBuf, StudioError> {
        let Some(path) = self.watched_scene_path() else {
            return Err(StudioError::Message(
                "active composition has no associated file on disk".to_string(),
            ));
        };
        if force {
            self.file_watcher
                .record_self_write(self.workspace_root.as_deref());
        }

        let saved_time = self.current_time;
        let saved_sel = self.selected_node;
        let was_playing = self.is_playing;

        let content = fs::read_to_string(&path)
            .map_err(|e| StudioError::Message(format!("read {}: {e}", path.display())))?;
        let base = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(|| self.workspace_root.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        self.reload_modules(&base)?;

        let def: SceneDef = if content.trim_start().starts_with('{') {
            serde_json::from_str(&content)
                .map_err(|e| StudioError::Message(format!("parse JSON {}: {e}", path.display())))?
        } else {
            self.module_registry
                .eval_scene_ts(&content)
                .map_err(|e| StudioError::Message(format!("eval TS {}: {e}", path.display())))?
        };

        self.scene_def = def;
        self.selected_node = saved_sel;
        self.current_time = saved_time;
        self.is_playing = was_playing;
        self.sync_scene_to_engine()?;
        self.anim.smooth_playhead = self.current_time;
        self.file_watcher.record_self_write(Some(&base));

        let msg = format!("Live Reload: synced external edits from {}", path.display());
        self.notify_status(&msg);

        if let Some(bridge) = &self.bridge {
            bridge.broadcast_event(&BridgeEvent {
                event: "file_reloaded".to_string(),
                frame_revision: self.frame_revision,
                current_time: self.current_time,
                frame: self.current_frame_index(),
                selected_node: self.selected_node.map(|n| n.0),
                summary: msg,
                payload: json!({
                    "path": path,
                    "nodeCount": self.node_paths().len(),
                }),
            });
        }

        Ok(path)
    }

    /// Save `self.scene_def` as pretty-printed JSON to `explicit_path` (or the active scene file).
    pub fn save_scene_def_to_disk(
        &mut self,
        explicit_path: Option<&Path>,
    ) -> Result<PathBuf, StudioError> {
        let target_path = explicit_path
            .map(Path::to_path_buf)
            .or_else(|| self.watched_scene_path())
            .unwrap_or_else(|| {
                let base = self
                    .workspace_root
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("."));
                base.join("scene.json")
            });

        let json = serde_json::to_string_pretty(&self.scene_def)
            .map_err(|e| StudioError::Message(format!("serialize scene: {e}")))?;
        fs::write(&target_path, json)
            .map_err(|e| StudioError::Message(format!("write {}: {e}", target_path.display())))?;

        self.file_watcher
            .watch(Some(target_path.clone()), self.workspace_root.as_deref());
        self.notify_status(format!("Saved scene JSON to {}", target_path.display()));
        Ok(target_path)
    }
}

/// Discover the address (`host:port`) of a running Kine Studio instance.
pub fn discover_bridge_addr(workspace_hint: Option<&Path>) -> String {
    if let Ok(env_addr) = std::env::var("KINE_STUDIO_BRIDGE") {
        let trimmed = env_addr.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let mut search_dirs = Vec::new();
    if let Some(ws) = workspace_hint {
        search_dirs.push(ws.to_path_buf());
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut cur: Option<&Path> = Some(&cwd);
        while let Some(d) = cur {
            search_dirs.push(d.to_path_buf());
            cur = d.parent();
        }
    }
    search_dirs.push(std::env::temp_dir());

    for dir in search_dirs {
        let manifest_path = dir.join(BRIDGE_MANIFEST_FILENAME);
        if let Ok(text) = fs::read_to_string(&manifest_path) {
            if let Ok(manifest) = serde_json::from_str::<BridgeDiscoveryManifest>(&text) {
                if !manifest.addr.is_empty() {
                    return manifest.addr;
                }
            }
        }
    }

    format!("{DEFAULT_BRIDGE_HOST}:{DEFAULT_BRIDGE_PORT}")
}

/// Execute a single `studio_*` tool call against a running Kine Studio instance over TCP.
pub fn bridge_call(addr: &str, tool: &str, args: Value) -> Result<AgentToolResult, String> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let mut stream = TcpStream::connect(addr).map_err(|e| {
        format!(
            "failed to connect to Kine Studio bridge at {addr}: {e} \
             (start `kine studio` in another window or pass `--scene <file>` for offline mode)"
        )
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let req = json!({
        "jsonrpc": "2.0",
        "id": NEXT_ID.fetch_add(1, Ordering::Relaxed),
        "method": tool,
        "params": args,
    });
    let line = format!("{req}\n");
    stream
        .write_all(line.as_bytes())
        .and_then(|_| stream.flush())
        .map_err(|e| format!("bridge write error: {e}"))?;

    let mut reader = BufReader::new(stream);
    let mut resp_line = String::new();
    reader
        .read_line(&mut resp_line)
        .map_err(|e| format!("bridge read error: {e}"))?;

    let v: Value =
        serde_json::from_str(resp_line.trim()).map_err(|e| format!("bridge JSON error: {e}"))?;

    if let Some(err_obj) = v.get("error") {
        let code = err_obj.get("code").and_then(Value::as_i64).unwrap_or(-32000);
        let msg = err_obj
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("JSON-RPC bridge error");
        return Err(format!("bridge RPC error ({code}): {msg}"));
    }
    if let Some(res_obj) = v.get("result") {
        if let Ok(tool_res) = serde_json::from_value::<AgentToolResult>(res_obj.clone()) {
            return Ok(tool_res);
        }
    }
    serde_json::from_value::<AgentToolResult>(v)
        .map_err(|e| format!("invalid bridge response: {e}"))
}

/// Subscribe to real-time events (`scene_mutated`, `selection_changed`, `playhead_changed`,
/// `file_reloaded`) from a running Kine Studio instance.
pub fn bridge_watch(addr: &str, mut on_event: impl FnMut(Value) -> bool) -> Result<(), String> {
    let mut stream = TcpStream::connect(addr)
        .map_err(|e| format!("failed to connect to Kine Studio bridge at {addr}: {e}"))?;
    let req = json!({ "method": "subscribe" });
    writeln!(stream, "{req}").map_err(|e| format!("bridge subscribe write error: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("bridge subscribe flush error: {e}"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("bridge stream read error: {e}"))?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
            if !on_event(val) {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kine_render::RenderBackend;
    use std::sync::mpsc;
    use std::thread;

    fn temp_workspace(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kine_bridge_test_{}_{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp workspace");
        dir
    }

    #[test]
    fn bridge_server_manifest_discovery_and_cleanup() {
        let ws = temp_workspace("manifest");
        let server =
            StudioBridgeServer::start(Some("127.0.0.1:0"), Some(&ws), None).expect("bind bridge");
        let bound_addr = server.addr().to_string();
        assert!(bound_addr.starts_with("127.0.0.1:"));

        let manifest_file = ws.join(BRIDGE_MANIFEST_FILENAME);
        assert!(manifest_file.exists(), "manifest must be written on start");

        let discovered = discover_bridge_addr(Some(&ws));
        assert_eq!(discovered, bound_addr);

        drop(server);
        assert!(
            !manifest_file.exists(),
            "manifest must be removed when StudioBridgeServer drops"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn bridge_wire_ping_schema_and_parse_error() {
        let ws = temp_workspace("wire");
        let server =
            StudioBridgeServer::start(Some("127.0.0.1:0"), Some(&ws), None).expect("bind bridge");
        let addr = server.addr().to_string();

        // 1. JSON-RPC ping
        let mut stream = TcpStream::connect(&addr).expect("connect to bridge");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        writeln!(
            stream,
            r#"{{"jsonrpc":"2.0","id":7,"method":"ping","params":{{}}}}"#
        )
        .unwrap();
        stream.flush().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["result"], "pong");
        assert_eq!(v["ok"], true);

        // 2. JSON-RPC schema
        line.clear();
        writeln!(
            stream,
            r#"{{"jsonrpc":"2.0","id":8,"method":"schema","params":{{}}}}"#
        )
        .unwrap();
        stream.flush().unwrap();
        reader.read_line(&mut line).unwrap();
        let schema_resp: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(schema_resp["id"], 8);
        assert!(schema_resp["result"].as_array().unwrap().len() >= 25);

        // 3. Malformed JSON -> -32700 parse error
        line.clear();
        writeln!(stream, "{{not valid json").unwrap();
        stream.flush().unwrap();
        reader.read_line(&mut line).unwrap();
        let err_resp: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(err_resp["ok"], false);
        assert_eq!(err_resp["error"]["code"], -32700);

        drop(server);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn bridge_call_and_watch_roundtrip_against_studio_state() {
        let ws = temp_workspace("roundtrip");
        let mut state = StudioState::new(Some(ws.clone()));
        state.engine.set_backend(RenderBackend::Cpu);
        let _ = state.render_current_frame();

        let addr = state
            .start_bridge_server(Some("127.0.0.1:0"))
            .expect("start bridge server");
        let pending_flag = state.bridge.as_ref().unwrap().pending_flag();

        // Subscribe a watcher client before mutating state
        let addr_watch = addr.clone();
        let (ev_tx, ev_rx) = mpsc::channel::<Value>();
        let watcher_handle = thread::spawn(move || {
            let _ = bridge_watch(&addr_watch, |ev| {
                let kind = ev.get("event").and_then(Value::as_str).unwrap_or("");
                let _ = ev_tx.send(ev.clone());
                kind != "playhead_changed"
            });
        });

        // Wait for initial "subscribed" ack event
        let sub_ack = ev_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("receive subscribed ack");
        assert_eq!(sub_ack["event"], "subscribed");

        // Spawn a client thread executing studio_status, studio_add_node, and studio_seek
        let addr_client = addr.clone();
        let client_handle = thread::spawn(move || {
            let status =
                bridge_call(&addr_client, "studio_status", json!({})).expect("status call");
            assert!(status.ok);
            assert_eq!(status.tool, "studio_status");

            let add = bridge_call(
                &addr_client,
                "studio_add_node",
                json!({
                    "kind": "rect",
                    "x": 40.0,
                    "y": 50.0,
                    "width": 200.0,
                    "height": 100.0,
                    "fill": "#33aaee"
                }),
            )
            .expect("add_node call");
            assert!(add.ok);
            assert_eq!(add.tool, "studio_add_node");

            let seek = bridge_call(&addr_client, "studio_seek", json!({ "time": 1.25 }))
                .expect("seek call");
            assert!(seek.ok);
            assert_eq!(seek.tool, "studio_seek");
        });

        // Pump StudioState bridge queue until client thread finishes
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if pending_flag.load(Ordering::Acquire) {
                state.poll_bridge_and_watcher();
            }
            if client_handle.is_finished() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        state.poll_bridge_and_watcher();
        client_handle.join().expect("client thread succeeded");

        assert!(
            (state.current_time - 1.25).abs() < 1e-6,
            "playhead must be updated to 1.25s"
        );
        assert_eq!(
            state.bridge.as_ref().unwrap().commands_handled,
            3,
            "bridge server must record 3 handled commands"
        );

        // Verify broadcast events received by bridge_watch subscriber
        let ev1 = ev_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("event 1 (tool_executed)");
        assert_eq!(ev1["event"], "tool_executed");
        let ev2 = ev_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("event 2 (scene_mutated)");
        assert_eq!(ev2["event"], "scene_mutated");
        let ev3 = ev_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("event 3 (playhead_changed)");
        assert_eq!(ev3["event"], "playhead_changed");

        watcher_handle.join().expect("watcher thread exited cleanly");
        state.stop_bridge_server();
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn scene_file_watcher_detects_external_disk_edits() {
        let ws = temp_workspace("watcher");
        let scene_path = ws.join("scene.json");

        let mut state = StudioState::new(Some(ws.clone()));
        state.engine.set_backend(RenderBackend::Cpu);
        state
            .save_scene_def_to_disk(Some(&scene_path))
            .expect("save initial scene");
        assert!(state.file_watcher.check_changed(Some(&ws), true).is_none());

        // Modify the scene on disk externally (change canvas size to 1920x1080)
        let mut modified = state.scene_def.clone();
        modified.width = 1920;
        modified.height = 1080;
        fs::write(&scene_path, serde_json::to_string_pretty(&modified).unwrap()).unwrap();

        let changed = state.file_watcher.check_changed(Some(&ws), true);
        assert_eq!(changed.as_deref(), Some(scene_path.as_path()));
        state
            .reload_watched_scene_from_disk(false)
            .expect("reload modified scene");
        assert_eq!(state.scene_def.width, 1920);
        assert_eq!(state.scene_def.height, 1080);

        let _ = fs::remove_dir_all(&ws);
    }
}
