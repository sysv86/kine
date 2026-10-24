//! Built-in AI Agent Chat & Ask-Agent workflow for Kine Studio.
//!
//! Combines Hyperframes Studio's `AskAgentModal.tsx` (structured element prompt
//! generation + context attachment) with an interactive in-process Agent Chat
//! loop capable of executing natural-language requests, slash commands, and
//! structured `studio_*` tool calls directly against [`StudioState`].

use crate::agent_tools::{
    build_element_agent_prompt, dispatch_agent_tool, studio_add_animation, studio_inspect,
    studio_look, studio_seek, studio_select, studio_set_style, studio_set_text, studio_transform,
    AgentToolResult, TransformPatch,
};
use crate::state::StudioState;
use kine_core::NodeId;
use kine_script::AnimationDef;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentChatRole {
    System,
    User,
    Assistant,
    ToolReceipt,
}

impl AgentChatRole {
    pub const fn badge(self) -> &'static str {
        match self {
            Self::System => "SYS",
            Self::User => "YOU",
            Self::Assistant => "AI",
            Self::ToolReceipt => "TOOL",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatMessage {
    pub role: AgentChatRole,
    pub content: String,
    pub node_context: Option<NodeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioChatSession {
    pub messages: Vec<AgentChatMessage>,
    pub input_draft: String,
    pub attach_selected_node: bool,
    pub provider_endpoint: String,
    pub provider_model: String,
    pub api_key_configured: bool,
}

impl Default for StudioChatSession {
    fn default() -> Self {
        Self::new()
    }
}

impl StudioChatSession {
    pub fn new() -> Self {
        let endpoint = std::env::var("KINE_AI_ENDPOINT")
            .unwrap_or_else(|_| "https://api.openai.com/v1/chat/completions".to_string());
        let model =
            std::env::var("KINE_AI_MODEL").unwrap_or_else(|_| "gpt-4.1-mini / local-agent".to_string());
        let api_key_configured = std::env::var("KINE_AI_API_KEY")
            .ok()
            .is_some_and(|k| !k.trim().is_empty());

        Self {
            messages: vec![AgentChatMessage {
                role: AgentChatRole::System,
                content: "Kine Studio AI Agent ready (in-process Engine + 12 studio_* tools). \
                          Select any node and type natural-language edits (e.g. 'set text to KINE 2.0', \
                          'make it crimson #f43f5e', 'center element', 'move right 40', \
                          'fade in 0 to 1.2s', 'add film grain') or tool commands (/look, /inspect, /seek 1.0)."
                    .to_string(),
                node_context: None,
            }],
            input_draft: String::new(),
            attach_selected_node: true,
            provider_endpoint: endpoint,
            provider_model: model,
            api_key_configured,
        }
    }
}

impl StudioState {
    /// Generate and store the structured "Kine Studio element edit request v1" prompt
    /// for the currently selected node (matching Hyperframes' `AskAgentModal`).
    pub fn generate_selected_element_prompt(&mut self, custom_request: &str) -> String {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let req = if custom_request.trim().is_empty() {
            self.ask_agent_draft.as_str()
        } else {
            custom_request
        };
        let prompt = build_element_agent_prompt(self, node_id, req);
        self.last_copied_agent_prompt = Some(prompt.clone());
        self.status_banner = format!("Built element agent prompt v1 for {node_id}");
        prompt
    }

    /// Submit the current `self.chat.input_draft` to the in-process AI agent loop.
    pub fn submit_current_chat_draft(&mut self) -> Vec<AgentToolResult> {
        let draft = self.chat.input_draft.trim().to_string();
        if draft.is_empty() {
            return Vec::new();
        }
        self.chat.input_draft.clear();
        self.submit_chat_prompt(&draft)
    }

    /// Process a user chat prompt or tool invocation against the in-process studio state.
    pub fn submit_chat_prompt(&mut self, prompt: &str) -> Vec<AgentToolResult> {
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let target_node = if self.chat.attach_selected_node {
            self.selected_node.or(Some(NodeId(0)))
        } else {
            Some(NodeId(0))
        };

        self.chat.messages.push(AgentChatMessage {
            role: AgentChatRole::User,
            content: trimmed.to_string(),
            node_context: target_node,
        });

        let mut receipts = Vec::new();

        // 1. Direct JSON tool call: `{"tool": "studio_...", "args": {...}}`
        if trimmed.starts_with('{') {
            if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                if let Some(tool_name) = val.get("tool").and_then(Value::as_str) {
                    let args = val.get("args").cloned().unwrap_or_else(|| val.clone());
                    let res = dispatch_agent_tool(self, tool_name, &args);
                    self.record_tool_receipt(&res, target_node);
                    receipts.push(res);
                    return receipts;
                }
            }
        }

        // 2. Slash commands (`/look`, `/inspect`, `/seek 1.5`, `/select 2`, `/prompt`)
        if let Some(cmd_body) = trimmed.strip_prefix('/') {
            let mut parts = cmd_body.split_whitespace();
            let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
            let arg_rest: String = parts.collect::<Vec<_>>().join(" ");
            match cmd.as_str() {
                "look" => {
                    let res = studio_look(self);
                    self.record_tool_receipt(&res, target_node);
                    receipts.push(res);
                    return receipts;
                }
                "inspect" => {
                    let id = arg_rest.parse::<u64>().ok();
                    let res = studio_inspect(self, id);
                    self.record_tool_receipt(&res, target_node);
                    receipts.push(res);
                    return receipts;
                }
                "seek" => {
                    let t = arg_rest
                        .trim_end_matches('s')
                        .parse::<f64>()
                        .unwrap_or(0.0);
                    let res = studio_seek(self, t);
                    self.record_tool_receipt(&res, target_node);
                    receipts.push(res);
                    return receipts;
                }
                "select" => {
                    let id = arg_rest.trim_start_matches('#').parse::<u64>().ok();
                    let res = studio_select(self, id);
                    self.record_tool_receipt(&res, target_node);
                    receipts.push(res);
                    return receipts;
                }
                "prompt" => {
                    let p = self.generate_selected_element_prompt(&arg_rest);
                    self.chat.messages.push(AgentChatMessage {
                        role: AgentChatRole::Assistant,
                        content: p,
                        node_context: target_node,
                    });
                    return receipts;
                }
                _ => {}
            }
        }

        // 3. Natural-language intent synthesis -> verified `studio_*` tool calls in-process
        let node_u64 = target_node.unwrap_or(NodeId(0)).0;
        let lower = trimmed.to_ascii_lowercase();

        // Undo / Redo
        if lower == "undo" || lower.contains("undo last") {
            let msg = match self.undo() {
                Some(lbl) => format!("Undid last edit: {lbl}"),
                None => "Undo stack is empty.".to_string(),
            };
            self.chat.messages.push(AgentChatMessage {
                role: AgentChatRole::Assistant,
                content: msg,
                node_context: target_node,
            });
            return receipts;
        }
        if lower == "redo" {
            let msg = match self.redo() {
                Some(lbl) => format!("Redid edit: {lbl}"),
                None => "Redo stack is empty.".to_string(),
            };
            self.chat.messages.push(AgentChatMessage {
                role: AgentChatRole::Assistant,
                content: msg,
                node_context: target_node,
            });
            return receipts;
        }

        // Add catalog presets via natural language ("add film grain", "add vignette", "add headline")
        if lower.contains("add film grain") || lower.contains("film grain") {
            if let Ok(new_id) = self.insert_catalog_item("shader-film-grain") {
                let res = AgentToolResult::verified(
                    "studio_insert_shader",
                    format!("Inserted WGSL Film Grain shader layer ({new_id})"),
                    json!({ "nodeId": new_id.0, "preset": "filmGrain" }),
                );
                self.record_tool_receipt(&res, Some(new_id));
                receipts.push(res);
                return receipts;
            }
        }
        if lower.contains("add vignette") || lower.contains("vignette") {
            if let Ok(new_id) = self.insert_catalog_item("shader-vignette") {
                let res = AgentToolResult::verified(
                    "studio_insert_shader",
                    format!("Inserted WGSL Vignette shader layer ({new_id})"),
                    json!({ "nodeId": new_id.0, "preset": "vignette" }),
                );
                self.record_tool_receipt(&res, Some(new_id));
                receipts.push(res);
                return receipts;
            }
        }
        if lower.contains("add scanlines") || lower.contains("scanline") {
            if let Ok(new_id) = self.insert_catalog_item("shader-scanlines") {
                let res = AgentToolResult::verified(
                    "studio_insert_shader",
                    format!("Inserted WGSL CRT Scanlines shader layer ({new_id})"),
                    json!({ "nodeId": new_id.0, "preset": "scanlines" }),
                );
                self.record_tool_receipt(&res, Some(new_id));
                receipts.push(res);
                return receipts;
            }
        }

        // Text change: e.g. `set text to ...` or `change text to ...`
        if let Some(idx) = lower.find("text to ") {
            let new_text = trimmed[idx + "text to ".len()..]
                .trim()
                .trim_matches('"')
                .trim_matches('\'');
            let res = studio_set_text(
                self,
                node_u64,
                new_text,
                Some("AI Agent: update text content"),
            );
            self.record_tool_receipt(&res, target_node);
            receipts.push(res);
        }

        // Color / Fill change: hex `#...` or named colors
        if let Some(hex) = extract_color_from_prompt(trimmed, &lower) {
            let mut changes = Map::new();
            let is_text = self
                .node_def(NodeId(node_u64))
                .and_then(|d| d.kind.as_deref())
                == Some("text");
            if is_text {
                changes.insert("color".to_string(), json!(hex));
            } else {
                changes.insert("fill".to_string(), json!(hex));
            }
            let res = studio_set_style(
                self,
                node_u64,
                &changes,
                Some("AI Agent: update node color"),
            );
            self.record_tool_receipt(&res, target_node);
            receipts.push(res);
        }

        // Opacity change: e.g. `opacity 0.5` or `set opacity to 0.4`
        if lower.contains("opacity") && !lower.contains("animate") && !lower.contains("fade") {
            if let Some(val) = extract_first_number_after(&lower, "opacity") {
                let normalized = if val > 1.0 && val <= 100.0 {
                    val / 100.0
                } else {
                    val
                };
                let mut changes = Map::new();
                changes.insert("opacity".to_string(), json!(normalized.clamp(0.0, 1.0)));
                let res = studio_set_style(
                    self,
                    node_u64,
                    &changes,
                    Some("AI Agent: set opacity"),
                );
                self.record_tool_receipt(&res, target_node);
                receipts.push(res);
            }
        }

        // Center alignment
        if lower.contains("center") {
            let _ = self.inspector_align_selected(Some("center"), Some("middle"));
            let rect = self.node_abs_rect(NodeId(node_u64));
            let res = AgentToolResult::verified(
                "studio_transform",
                format!("Centered #{node_u64} in canvas -> {rect:?}"),
                json!({ "nodeId": node_u64, "afterBounds": rect }),
            );
            self.record_tool_receipt(&res, target_node);
            receipts.push(res);
        }

        // Directional move ("move right 40", "move left 20", "move up 15", "move down 30")
        if lower.contains("move ") || lower.contains("nudge ") {
            let amount = extract_last_number(&lower).unwrap_or(24.0) as f32;
            let cur = self
                .node_abs_rect(NodeId(node_u64))
                .unwrap_or_else(|| kine_core::Rect::new(0.0, 0.0, 100.0, 100.0));
            let mut patch = TransformPatch::default();
            if lower.contains("right") {
                patch.x = Some(cur.x + amount);
            } else if lower.contains("left") {
                patch.x = Some(cur.x - amount);
            }
            if lower.contains("down") {
                patch.y = Some(cur.y + amount);
            } else if lower.contains("up") {
                patch.y = Some(cur.y - amount);
            }
            if patch.x.is_some() || patch.y.is_some() {
                let res = studio_transform(
                    self,
                    node_u64,
                    patch,
                    Some("AI Agent: move node"),
                );
                self.record_tool_receipt(&res, target_node);
                receipts.push(res);
            }
        }

        // Rotation / Scale ("rotate 45", "scale 1.5")
        if lower.contains("rotate") && !lower.contains("animate") {
            let deg = extract_first_number_after(&lower, "rotate").unwrap_or(15.0) as f32;
            let res = studio_transform(
                self,
                node_u64,
                TransformPatch {
                    rotation: Some(deg),
                    ..Default::default()
                },
                Some("AI Agent: rotate node"),
            );
            self.record_tool_receipt(&res, target_node);
            receipts.push(res);
        }
        if lower.contains("scale") && !lower.contains("animate") {
            if let Some(s) = extract_first_number_after(&lower, "scale") {
                let res = studio_transform(
                    self,
                    node_u64,
                    TransformPatch {
                        scale: Some(s as f32),
                        ..Default::default()
                    },
                    Some("AI Agent: scale node"),
                );
                self.record_tool_receipt(&res, target_node);
                receipts.push(res);
            }
        }

        // Fade in / Animate requests
        if lower.contains("fade in") {
            let spec = AnimationDef {
                from: 0.0,
                to: 1.0,
                start: 0.0,
                end: 1.2,
                keyframes: None,
                easing: Some("easeOut".to_string()),
            };
            let res = studio_add_animation(
                self,
                node_u64,
                "opacity",
                spec,
                Some("AI Agent: add fade-in animation"),
            );
            self.record_tool_receipt(&res, target_node);
            receipts.push(res);
        } else if lower.contains("animate") && lower.contains("rotat") {
            let spec = AnimationDef {
                from: 0.0,
                to: 360.0,
                start: 0.0,
                end: 2.0,
                keyframes: None,
                easing: Some("easeInOutCubic".to_string()),
            };
            let res = studio_add_animation(
                self,
                node_u64,
                "rotation",
                spec,
                Some("AI Agent: animate rotation 0..360"),
            );
            self.record_tool_receipt(&res, target_node);
            receipts.push(res);
        }

        // Fallback: if no mutating intent matched, inspect the target node and provide an actionable summary.
        if receipts.is_empty() {
            let res = studio_inspect(self, Some(node_u64));
            let prompt_v1 = build_element_agent_prompt(self, NodeId(node_u64), trimmed);
            self.last_copied_agent_prompt = Some(prompt_v1);
            self.chat.messages.push(AgentChatMessage {
                role: AgentChatRole::Assistant,
                content: format!(
                    "Inspected #{node_u64} and prepared structured 'Kine Studio element edit request v1' \
                     prompt. Try commands like: 'set text to KINE 2.0', 'color #38bdf8', 'center', \
                     'move right 30', 'fade in', or 'add film grain'."
                ),
                node_context: target_node,
            });
            receipts.push(res);
        }

        receipts
    }

    fn record_tool_receipt(&mut self, res: &AgentToolResult, node_context: Option<NodeId>) {
        self.chat.messages.push(AgentChatMessage {
            role: AgentChatRole::ToolReceipt,
            content: format!("[{}] {}: {}", res.stage, res.tool, res.summary),
            node_context,
        });
        self.status_banner = format!("AI Tool [{}]: {}", res.tool, res.summary);
    }
}

fn extract_color_from_prompt(raw: &str, lower: &str) -> Option<String> {
    if let Some(hash_idx) = raw.find('#') {
        let hex_part: String = raw[hash_idx + 1..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if matches!(hex_part.len(), 3 | 4 | 6 | 8) {
            return Some(format!("#{hex_part}"));
        }
    }
    let named = [
        ("crimson", "#f43f5e"),
        ("red", "#ef4444"),
        ("cyan", "#22d3ee"),
        ("sky", "#38bdf8"),
        ("blue", "#3b82f6"),
        ("emerald", "#10b981"),
        ("green", "#22c55e"),
        ("amber", "#f59e0b"),
        ("yellow", "#eab308"),
        ("purple", "#a855f7"),
        ("violet", "#8b5cf6"),
        ("white", "#f8fafc"),
        ("slate", "#334155"),
    ];
    for (name, hex) in named {
        if lower.contains(name) {
            return Some(hex.to_string());
        }
    }
    None
}

fn extract_first_number_after(lower: &str, keyword: &str) -> Option<f64> {
    let idx = lower.find(keyword)?;
    let tail = &lower[idx + keyword.len()..];
    for token in tail.split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-') {
        if !token.is_empty() && token != "-" && token != "." {
            if let Ok(n) = token.parse::<f64>() {
                return Some(n);
            }
        }
    }
    None
}

fn extract_last_number(lower: &str) -> Option<f64> {
    lower
        .split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .filter(|t| !t.is_empty() && *t != "-" && *t != ".")
        .filter_map(|t| t.parse::<f64>().ok())
        .next_back()
}
