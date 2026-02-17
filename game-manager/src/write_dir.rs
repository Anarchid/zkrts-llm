//! Agent write-dir initialization.
//!
//! Creates an isolated Spring write directory for the agent, symlinking shared
//! content from the human player's `~/.spring/` to avoid duplication.
//! On first boot this sets up the directory structure, installs the SAI bridge
//! .so, and generates default configs.

use std::path::{Path, PathBuf};

/// Directories to symlink from spring_home into the agent write-dir.
/// Note: `cache` is intentionally excluded — ArchiveCache20.lua stores absolute
/// paths, so sharing it across different write-dirs causes a full rescan anyway,
/// and writing back would clobber the human player's cache.
const SHARED_DIRS: &[&str] = &[
    "pool",
    "packages",
    "maps",
    "games",
    "engine",
    "rapid",
];

/// Initialize the agent write directory.
///
/// - Creates the directory structure
/// - Symlinks shared content from `spring_home`
/// - Installs the SAI bridge .so + metadata
/// - Installs the startup widget
/// - Generates default springsettings.cfg
pub fn init_write_dir(
    base: &Path,
    spring_home: &Path,
    sai_bridge_lib: &Path,
    sai_bridge_data: &Path,
    widget_source: &Path,
    widget_dir: &Path,
    agent_name: &str,
) -> anyhow::Result<()> {
    tracing::info!("Initializing agent write-dir: {}", base.display());

    // 1. Create base dir
    std::fs::create_dir_all(base)?;

    // 2. Create subdirs
    let subdirs = [
        "AI/Skirmish/AgentBridge/0.1",
        "AI/Interfaces",
        "LuaUI/Widgets",
        "LuaUI/Config",
        "demos",
        "temp",
    ];
    for sub in &subdirs {
        let p = base.join(sub);
        if !p.exists() {
            std::fs::create_dir_all(&p)?;
            tracing::info!("  Created {}", sub);
        }
    }

    // 3. Symlink shared content
    for dir_name in SHARED_DIRS {
        let target = spring_home.join(dir_name);
        let link = base.join(dir_name);

        if link.exists() || link.symlink_metadata().is_ok() {
            // Already exists (file, dir, or symlink) — check if correct
            if let Ok(existing_target) = std::fs::read_link(&link) {
                if existing_target == target {
                    continue; // correct symlink
                }
                tracing::warn!(
                    "  Symlink {} points to {} (expected {}), skipping",
                    dir_name,
                    existing_target.display(),
                    target.display()
                );
            }
            continue;
        }

        if target.exists() {
            std::os::unix::fs::symlink(&target, &link)?;
            tracing::info!("  Symlinked {} -> {}", dir_name, target.display());
        } else {
            tracing::warn!("  Spring home dir {} not found, skipping symlink", target.display());
        }
    }

    // Symlink AI/Interfaces from spring_home
    let ai_interfaces_target = spring_home.join("AI/Interfaces");
    let ai_interfaces_link = base.join("AI/Interfaces");
    if ai_interfaces_target.exists() {
        // Remove the empty dir we created above, replace with symlink
        if ai_interfaces_link.is_dir()
            && std::fs::read_dir(&ai_interfaces_link)
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
        {
            std::fs::remove_dir(&ai_interfaces_link)?;
            std::os::unix::fs::symlink(&ai_interfaces_target, &ai_interfaces_link)?;
            tracing::info!(
                "  Symlinked AI/Interfaces -> {}",
                ai_interfaces_target.display()
            );
        }
    }

    // Symlink other Skirmish AIs (CircuitAI variants etc.) from spring_home.
    // We manage AgentBridge ourselves, so skip that.
    let skirmish_home = spring_home.join("AI/Skirmish");
    let skirmish_local = base.join("AI/Skirmish");
    if skirmish_home.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&skirmish_home) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name == "AgentBridge" {
                    continue; // managed locally
                }
                let link = skirmish_local.join(&name);
                if link.exists() || link.symlink_metadata().is_ok() {
                    continue; // already present
                }
                let target = entry.path();
                std::os::unix::fs::symlink(&target, &link)?;
                tracing::info!("  Symlinked AI/Skirmish/{} -> {}", name.to_string_lossy(), target.display());
            }
        }
    }

    // 4. Install SAI bridge
    let ai_dir = base.join("AI/Skirmish/AgentBridge/0.1");
    let lib_dest = ai_dir.join("libSkirmishAI.so");
    if sai_bridge_lib.exists() {
        if should_update(&lib_dest, sai_bridge_lib)? {
            std::fs::copy(sai_bridge_lib, &lib_dest)?;
            tracing::info!("  Installed libSkirmishAI.so");
        }
    } else {
        tracing::warn!(
            "  SAI bridge lib not found at {}, skipping",
            sai_bridge_lib.display()
        );
    }

    // Copy AIInfo.lua and AIOptions.lua
    for name in &["AIInfo.lua", "AIOptions.lua"] {
        let src = sai_bridge_data.join(name);
        let dest = ai_dir.join(name);
        if src.exists() && should_update(&dest, &src)? {
            std::fs::copy(&src, &dest)?;
            tracing::info!("  Installed {}", name);
        }
    }

    // 5. Install startup widget
    let widget_dest = base.join("LuaUI/Widgets/agent_bootstrap.lua");
    if widget_source.exists() && should_update(&widget_dest, widget_source)? {
        std::fs::copy(widget_source, &widget_dest)?;
        tracing::info!("  Installed agent_bootstrap.lua");
    }

    // 5b. Install all agent_*.lua widgets from widget directory
    if widget_dir.is_dir() {
        for entry in std::fs::read_dir(widget_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Install agent_*.lua files (skip agent_bootstrap.lua — handled above)
            if name_str.starts_with("agent_")
                && name_str.ends_with(".lua")
                && name_str != "agent_bootstrap.lua"
            {
                let src = entry.path();
                let dest = base.join("LuaUI/Widgets").join(&name);
                if should_update(&dest, &src)? {
                    std::fs::copy(&src, &dest)?;
                    tracing::info!("  Installed {}", name_str);
                }
            }
        }
    }

    // 6. Generate agent bootstrap config
    let json_path = base.join("LuaUI/Config/agent_bootstrap.json");
    if !json_path.exists() {
        let config = serde_json::json!({
            "players": {
                agent_name: {
                    "ai": "AgentBridge",
                    "version": "0.1"
                }
            }
        });
        std::fs::write(&json_path, serde_json::to_string_pretty(&config)?)?;
        write_bootstrap_lua(base, &config)?;
        tracing::info!("  Generated agent_bootstrap config for '{}'", agent_name);
    }

    // 7. Generate springsettings.cfg if missing
    let settings_path = base.join("springsettings.cfg");
    if !settings_path.exists() {
        std::fs::write(
            &settings_path,
            HEADLESS_SETTINGS,
        )?;
        tracing::info!("  Generated springsettings.cfg");
    }

    tracing::info!("Write-dir initialization complete");
    Ok(())
}

/// Ensure a player name is whitelisted in the bootstrap config.
/// For multiplayer, the lobby username may differ from the default agent_name
/// that was written at write-dir init time.
pub fn ensure_player_whitelisted(write_dir: &Path, player_name: &str) -> anyhow::Result<()> {
    let json_path = write_dir.join("LuaUI/Config/agent_bootstrap.json");
    let mut config: serde_json::Value = if json_path.exists() {
        let contents = std::fs::read_to_string(&json_path)?;
        serde_json::from_str(&contents)?
    } else {
        serde_json::json!({"players": {}})
    };

    if let Some(players) = config.get_mut("players").and_then(|p| p.as_object_mut()) {
        if !players.contains_key(player_name) {
            players.insert(
                player_name.to_string(),
                serde_json::json!({"ai": "AgentBridge", "version": "0.1"}),
            );
            std::fs::write(&json_path, serde_json::to_string_pretty(&config)?)?;
            write_bootstrap_lua(write_dir, &config)?;
            tracing::info!("Added '{}' to bootstrap config", player_name);
        }
    }
    Ok(())
}

/// Generate the Lua config file that the widget reads via VFS.Include.
/// The JSON file is Rust's source of truth; this is the generated output.
fn write_bootstrap_lua(write_dir: &Path, config: &serde_json::Value) -> anyhow::Result<()> {
    let lua_path = write_dir.join("LuaUI/Config/agent_bootstrap_config.lua");
    let lua = format!("return {}\n", json_to_lua(config, 0));
    std::fs::write(&lua_path, lua)?;
    Ok(())
}

/// Convert a serde_json::Value to a Lua table literal string.
fn json_to_lua(value: &serde_json::Value, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    let inner = "  ".repeat(indent + 1);
    match value {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                return "{}".to_string();
            }
            let entries: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}[\"{}\"] = {}", inner, k, json_to_lua(v, indent + 1)))
                .collect();
            format!("{{\n{},\n{}}}", entries.join(",\n"), pad)
        }
        serde_json::Value::String(s) => {
            format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
        }
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => "nil".to_string(),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(|v| json_to_lua(v, indent + 1)).collect();
            format!("{{ {} }}", items.join(", "))
        }
    }
}

/// Configure ZK_order.lua to disable all widgets except Agent Bootstrap.
/// Called before headless player-mode engine launches to prevent LuaUI OOM.
/// Widget names that should be enabled in headless mode.
const AGENT_WIDGETS: &[&str] = &[
    "Agent Bootstrap",
    "Agent Manager",
    "Agent Threatmap",
    "Agent Economy",
    "Agent Roster",
    "Agent Intel",
    "Agent Squads",
];

/// Check if a widget name is one of our agent widgets.
fn is_agent_widget(name: &str) -> bool {
    AGENT_WIDGETS.iter().any(|w| name.contains(w))
}

pub fn configure_headless_widgets(write_dir: &Path) -> anyhow::Result<()> {
    let order_path = write_dir.join("LuaUI/Config/ZK_order.lua");

    if order_path.exists() {
        // Read existing order file and set everything to 0 except our widgets
        let content = std::fs::read_to_string(&order_path)?;
        let mut new_lines = Vec::new();
        for line in content.lines() {
            if is_agent_widget(line) {
                // Keep our widgets enabled
                new_lines.push(line.to_string());
            } else if line.contains("] =") && !line.starts_with("--") {
                // Disable other widgets: replace the order number with 0
                if let Some(eq_pos) = line.rfind("= ") {
                    let mut disabled = line[..eq_pos + 2].to_string();
                    disabled.push_str("0,");
                    new_lines.push(disabled);
                } else {
                    new_lines.push(line.to_string());
                }
            } else {
                new_lines.push(line.to_string());
            }
        }
        std::fs::write(&order_path, new_lines.join("\n"))?;
    } else {
        // No prior run — write minimal order file.
        let mut content = String::from("-- Widget Order List  (0 disables a widget)\nreturn {\n");
        for (i, name) in AGENT_WIDGETS.iter().enumerate() {
            content.push_str(&format!("\t[\"{}\"] = {},\n", name, i + 1));
        }
        content.push_str("\tversion = 8,\n}\n");
        std::fs::write(&order_path, content)?;
    }

    // Ensure LuaAutoModWidgets=0 so unknown widgets from archives don't auto-enable
    let settings_path = write_dir.join("springsettings.cfg");
    if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)?;
        if !content.contains("LuaAutoModWidgets") {
            let mut new_content = content;
            new_content.push_str("LuaAutoModWidgets=0\n");
            std::fs::write(&settings_path, new_content)?;
        }
    }

    tracing::info!("Configured headless widget order ({} agent widgets enabled)", AGENT_WIDGETS.len());
    Ok(())
}

/// Check if dest file needs updating (missing or different from src).
/// Compares file sizes first (recompilation almost always changes binary size),
/// then falls back to mtime comparison.
fn should_update(dest: &Path, src: &Path) -> anyhow::Result<bool> {
    if !dest.exists() {
        return Ok(true);
    }
    let src_meta = std::fs::metadata(src)?;
    let dest_meta = std::fs::metadata(dest)?;
    if src_meta.len() != dest_meta.len() {
        return Ok(true);
    }
    Ok(src_meta.modified()? > dest_meta.modified()?)
}

/// Resolve paths for SAI bridge components.
pub struct WriteDirConfig {
    pub write_dir: PathBuf,
    pub spring_home: PathBuf,
    pub sai_bridge_lib: PathBuf,
    pub sai_bridge_data: PathBuf,
    pub widget_source: PathBuf,
    pub widget_dir: PathBuf,
    pub agent_name: String,
}

impl WriteDirConfig {
    /// Build config from CLI args / env vars / defaults.
    pub fn from_env(
        write_dir: Option<&str>,
        spring_home: Option<&str>,
        agent_name: Option<&str>,
    ) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());

        let write_dir = write_dir
            .map(PathBuf::from)
            .or_else(|| std::env::var("AGENT_WRITE_DIR").ok().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(format!("{}/.spring-loom", home)));

        let spring_home = spring_home
            .map(PathBuf::from)
            .or_else(|| std::env::var("SPRING_HOME").ok().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(format!("{}/.spring", home)));

        let agent_name = agent_name
            .map(String::from)
            .or_else(|| std::env::var("AGENT_NAME").ok())
            .unwrap_or_else(|| "loom".into());

        // SAI bridge lib: check env, then relative to game-manager binary
        let sai_bridge_lib = std::env::var("SAI_BRIDGE_LIB")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                // Try path relative to workspace
                let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                workspace
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("sai-bridge/target/release/libSkirmishAI.so")
            });

        let sai_bridge_data = std::env::var("SAI_BRIDGE_DATA")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                workspace
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("sai-bridge/data")
            });

        let widget_source = std::env::var("WIDGET_SOURCE")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                workspace.join("data/widgets/agent_bootstrap.lua")
            });

        let widget_dir = std::env::var("WIDGET_DIR")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                workspace.join("data/widgets")
            });

        Self {
            write_dir,
            spring_home,
            sai_bridge_lib,
            sai_bridge_data,
            widget_source,
            widget_dir,
            agent_name,
        }
    }

    pub fn init(&self) -> anyhow::Result<()> {
        init_write_dir(
            &self.write_dir,
            &self.spring_home,
            &self.sai_bridge_lib,
            &self.sai_bridge_data,
            &self.widget_source,
            &self.widget_dir,
            &self.agent_name,
        )
    }
}

const HEADLESS_SETTINGS: &str = "\
XResolution=1280
YResolution=720
WindowState=0
Fullscreen=0
VSync=0
ROAM=0
SmoothLines=0
SmoothPoints=0
FSAA=0
FSAALevel=0
AdvSky=0
DynamicSky=0
3DTrees=0
HighResInfoTexture=0
GroundDetail=1
UnitLodDist=0
GrassDetail=0
MaxParticles=0
GroundDecals=0
UnitIconDist=0
MaxSounds=0
snd_volmaster=0
";
