//! Dynamic tool registry for widget/plugin-provided tools.
//!
//! Tracks tool definitions registered by game-side components (Lua widgets
//! or Rust plugins) and maps them back to their source channel for routing.

use crate::sai_ipc::ToolDefinition;
use mcpl_core::types::JsonRpcId;
use std::collections::HashMap;
use std::time::Instant;

/// Tracks a pending tool call awaiting a result from the game side.
pub struct PendingCall {
    /// The MCPL request ID to respond to when the result arrives.
    pub mcpl_request_id: JsonRpcId,
    /// When this call was initiated (for timeout).
    pub created_at: Instant,
}

/// Registry of dynamically registered tools from widgets/plugins.
pub struct ToolRegistry {
    /// tool_name → (channel_id, definition)
    tools: HashMap<String, (String, ToolDefinition)>,
    /// call_id → pending call info
    pending_calls: HashMap<String, PendingCall>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            pending_calls: HashMap::new(),
        }
    }

    /// Register tools from a specific channel (game instance).
    /// Returns the names of newly registered tools.
    pub fn register(
        &mut self,
        channel_id: &str,
        tools: Vec<ToolDefinition>,
    ) -> Vec<String> {
        let mut registered = Vec::new();
        for tool in tools {
            let name = tool.name.clone();
            self.tools
                .insert(name.clone(), (channel_id.to_string(), tool));
            registered.push(name);
        }
        registered
    }

    /// Unregister specific tools by name.
    /// Returns the names actually removed.
    pub fn unregister(&mut self, tool_names: &[String]) -> Vec<String> {
        let mut removed = Vec::new();
        for name in tool_names {
            if self.tools.remove(name).is_some() {
                removed.push(name.clone());
            }
        }
        removed
    }

    /// Remove all tools belonging to a channel (e.g. when a game ends).
    /// Returns the names of removed tools.
    pub fn remove_channel(&mut self, channel_id: &str) -> Vec<String> {
        let to_remove: Vec<String> = self
            .tools
            .iter()
            .filter(|(_, (ch, _))| ch == channel_id)
            .map(|(name, _)| name.clone())
            .collect();

        for name in &to_remove {
            self.tools.remove(name);
        }

        // Orphaned pending calls for this channel will be caught by the timeout mechanism.

        to_remove
    }

    /// Get the list of all registered tools as MCP-compatible JSON.
    pub fn list_tools(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|(_, def)| {
                serde_json::json!({
                    "name": def.name,
                    "description": def.description,
                    "inputSchema": def.input_schema,
                })
            })
            .collect()
    }

    /// Resolve a tool name to its source channel ID.
    pub fn resolve_tool(&self, tool_name: &str) -> Option<&str> {
        self.tools
            .get(tool_name)
            .map(|(ch, _)| ch.as_str())
    }

    /// Track a pending tool call.
    pub fn track_call(
        &mut self,
        call_id: String,
        mcpl_request_id: JsonRpcId,
    ) {
        self.pending_calls.insert(
            call_id,
            PendingCall {
                mcpl_request_id,
                created_at: Instant::now(),
            },
        );
    }

    /// Complete a pending call, returning the MCPL request ID to respond to.
    pub fn complete_call(&mut self, call_id: &str) -> Option<JsonRpcId> {
        self.pending_calls
            .remove(call_id)
            .map(|pc| pc.mcpl_request_id)
    }

    /// Collect calls that have exceeded the timeout duration.
    /// Returns (call_id, mcpl_request_id) pairs for timed-out calls.
    pub fn collect_timed_out(
        &mut self,
        timeout: std::time::Duration,
    ) -> Vec<(String, JsonRpcId)> {
        let now = Instant::now();
        let timed_out: Vec<String> = self
            .pending_calls
            .iter()
            .filter(|(_, pc)| now.duration_since(pc.created_at) > timeout)
            .map(|(id, _)| id.clone())
            .collect();

        timed_out
            .into_iter()
            .filter_map(|id| {
                self.pending_calls
                    .remove(&id)
                    .map(|pc| (id, pc.mcpl_request_id))
            })
            .collect()
    }

    /// Check if there are any registered tools.
    pub fn has_tools(&self) -> bool {
        !self.tools.is_empty()
    }
}
