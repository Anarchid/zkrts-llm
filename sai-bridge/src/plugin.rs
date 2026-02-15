//! Rust plugin infrastructure for SAI bridge.
//!
//! Plugins are compiled `.so` files placed in `<data_dir>/plugins/`.
//! They expose tools via the `Plugin` trait and receive a safe, read-only
//! view of the game state via `GameState`.

use crate::callbacks::EngineCallbacks;
use crate::events::ToolDef;
use std::path::Path;

/// Read-only view of game state, passed to plugins during tool calls.
/// Wraps EngineCallbacks with a safe interface — plugins cannot issue commands.
pub struct GameState<'a> {
    cb: &'a EngineCallbacks,
}

impl<'a> GameState<'a> {
    pub fn new(cb: &'a EngineCallbacks) -> Self {
        Self { cb }
    }

    pub fn current_frame(&self) -> i32 {
        self.cb.get_current_frame()
    }

    pub fn my_team(&self) -> i32 {
        self.cb.get_my_team()
    }

    pub fn my_ally_team(&self) -> i32 {
        self.cb.get_my_ally_team()
    }

    pub fn economy_current(&self, resource_id: i32) -> f32 {
        self.cb.economy_current(resource_id)
    }

    pub fn economy_income(&self, resource_id: i32) -> f32 {
        self.cb.economy_income(resource_id)
    }

    pub fn economy_usage(&self, resource_id: i32) -> f32 {
        self.cb.economy_usage(resource_id)
    }

    pub fn economy_storage(&self, resource_id: i32) -> f32 {
        self.cb.economy_storage(resource_id)
    }

    pub fn unit_get_def(&self, unit_id: i32) -> i32 {
        self.cb.unit_get_def(unit_id)
    }

    pub fn unit_get_pos(&self, unit_id: i32) -> [f32; 3] {
        self.cb.unit_get_pos(unit_id)
    }

    pub fn unit_def_get_name(&self, unit_def_id: i32) -> Option<String> {
        self.cb.unit_def_get_name(unit_def_id)
    }

    pub fn unit_def_get_human_name(&self, unit_def_id: i32) -> Option<String> {
        self.cb.unit_def_get_human_name(unit_def_id)
    }

    pub fn map_width(&self) -> i32 {
        self.cb.map_width()
    }

    pub fn map_height(&self) -> i32 {
        self.cb.map_height()
    }
}

/// Result of a plugin tool call.
pub struct PluginToolResult {
    pub content: Vec<serde_json::Value>,
    pub is_error: bool,
}

impl PluginToolResult {
    pub fn text(s: String) -> Self {
        Self {
            content: vec![serde_json::json!({"type": "text", "text": s})],
            is_error: false,
        }
    }

    pub fn error(s: String) -> Self {
        Self {
            content: vec![serde_json::json!({"type": "text", "text": s})],
            is_error: true,
        }
    }
}

/// Trait that plugins must implement.
pub trait Plugin: Send {
    /// Plugin name (used for logging and tool namespacing).
    fn name(&self) -> &str;

    /// Return tool definitions this plugin provides.
    fn tools(&self) -> Vec<ToolDef>;

    /// Handle a tool call. Called synchronously from the engine thread.
    fn call(
        &mut self,
        tool: &str,
        args: &serde_json::Value,
        state: &GameState,
    ) -> PluginToolResult;
}

/// Hosts loaded plugins and routes tool calls to them.
pub struct PluginHost {
    plugins: Vec<Box<dyn Plugin>>,
    /// Keep libraries alive so symbols remain valid.
    _libraries: Vec<libloading::Library>,
}

/// Type signature for the plugin entry point.
/// Plugins must export: `extern "C" fn create_plugin() -> Box<dyn Plugin>`
type CreatePluginFn = unsafe extern "C" fn() -> Box<dyn Plugin>;

impl PluginHost {
    /// Scan a directory for `.so` plugin files and load them.
    pub fn load_from(plugin_dir: &Path) -> Self {
        let mut plugins: Vec<Box<dyn Plugin>> = Vec::new();
        let mut libraries: Vec<libloading::Library> = Vec::new();

        if !plugin_dir.is_dir() {
            return Self {
                plugins,
                _libraries: libraries,
            };
        }

        let entries = match std::fs::read_dir(plugin_dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[SAI Plugin] Failed to read plugin dir: {}", e);
                return Self {
                    plugins,
                    _libraries: libraries,
                };
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("so") {
                continue;
            }

            match Self::load_plugin(&path) {
                Ok((plugin, lib)) => {
                    eprintln!("[SAI Plugin] Loaded plugin '{}' from {}", plugin.name(), path.display());
                    plugins.push(plugin);
                    libraries.push(lib);
                }
                Err(e) => {
                    eprintln!("[SAI Plugin] Failed to load {}: {}", path.display(), e);
                }
            }
        }

        Self {
            plugins,
            _libraries: libraries,
        }
    }

    fn load_plugin(
        path: &Path,
    ) -> Result<(Box<dyn Plugin>, libloading::Library), String> {
        let lib = unsafe {
            libloading::Library::new(path)
                .map_err(|e| format!("dlopen failed: {}", e))?
        };

        let create_fn: libloading::Symbol<CreatePluginFn> = unsafe {
            lib.get(b"create_plugin")
                .map_err(|e| format!("Missing create_plugin symbol: {}", e))?
        };

        let plugin = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            create_fn()
        }))
        .map_err(|_| "create_plugin panicked".to_string())?;

        Ok((plugin, lib))
    }

    /// Get all tool definitions from all loaded plugins.
    pub fn all_tools(&self) -> Vec<ToolDef> {
        self.plugins.iter().flat_map(|p| p.tools()).collect()
    }

    /// Try to call a tool on any loaded plugin. Returns None if no plugin handles it.
    pub fn call(
        &mut self,
        tool: &str,
        args: &serde_json::Value,
        state: &GameState,
    ) -> Option<PluginToolResult> {
        for plugin in &mut self.plugins {
            let tool_names: Vec<String> = plugin.tools().iter().map(|t| t.name.clone()).collect();
            if tool_names.iter().any(|n| n == tool) {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    plugin.call(tool, args, state)
                }));
                return Some(match result {
                    Ok(r) => r,
                    Err(_) => PluginToolResult::error(format!(
                        "Plugin '{}' panicked while handling '{}'",
                        plugin.name(),
                        tool
                    )),
                });
            }
        }
        None
    }

    /// Check if any plugins are loaded.
    pub fn has_plugins(&self) -> bool {
        !self.plugins.is_empty()
    }
}
