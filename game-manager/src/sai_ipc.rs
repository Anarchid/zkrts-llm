//! Unix socket IPC server for SAI bridge connections.
//!
//! Listens for incoming connections from game engine processes
//! running the SAI bridge. Routes events to MCPL channels and
//! commands from MCPL to the appropriate engine.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// A tool definition registered by a widget or plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetalSpot {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub metal: f32,
}

/// An event received from a SAI bridge instance.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SaiEvent {
    #[serde(rename = "init")]
    Init {
        frame: i32,
        saved_game: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metal_spots: Option<Vec<MetalSpot>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        map_width: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        map_height: Option<i32>,
    },
    #[serde(rename = "release")]
    Release { reason: i32 },
    #[serde(rename = "update")]
    Update { frame: i32 },
    #[serde(rename = "message")]
    Message { player: i32, text: String },
    #[serde(rename = "unit_created")]
    UnitCreated {
        unit: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_name: Option<String>,
        builder: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        builder_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pos: Option<[f32; 3]>,
    },
    #[serde(rename = "unit_finished")]
    UnitFinished {
        unit: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pos: Option<[f32; 3]>,
    },
    #[serde(rename = "unit_idle")]
    UnitIdle {
        unit: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_name: Option<String>,
    },
    #[serde(rename = "unit_move_failed")]
    UnitMoveFailed {
        unit: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_name: Option<String>,
    },
    #[serde(rename = "unit_damaged")]
    UnitDamaged {
        unit: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_name: Option<String>,
        attacker: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attacker_name: Option<String>,
        damage: f32,
        weapon_def_id: i32,
        paralyzer: bool,
    },
    #[serde(rename = "unit_destroyed")]
    UnitDestroyed {
        unit: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_name: Option<String>,
        attacker: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attacker_name: Option<String>,
        weapon_def_id: i32,
    },
    #[serde(rename = "unit_given")]
    UnitGiven {
        unit: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_name: Option<String>,
        old_team: i32,
        new_team: i32,
    },
    #[serde(rename = "unit_captured")]
    UnitCaptured {
        unit: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_name: Option<String>,
        old_team: i32,
        new_team: i32,
    },
    #[serde(rename = "enemy_enter_los")]
    EnemyEnterLos {
        enemy: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enemy_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pos: Option<[f32; 3]>,
    },
    #[serde(rename = "enemy_leave_los")]
    EnemyLeaveLos {
        enemy: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enemy_name: Option<String>,
    },
    #[serde(rename = "enemy_enter_radar")]
    EnemyEnterRadar {
        enemy: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enemy_name: Option<String>,
    },
    #[serde(rename = "enemy_leave_radar")]
    EnemyLeaveRadar {
        enemy: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enemy_name: Option<String>,
    },
    #[serde(rename = "enemy_damaged")]
    EnemyDamaged {
        enemy: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enemy_name: Option<String>,
        attacker: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attacker_name: Option<String>,
        damage: f32,
        weapon_def_id: i32,
        paralyzer: bool,
    },
    #[serde(rename = "enemy_destroyed")]
    EnemyDestroyed {
        enemy: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enemy_name: Option<String>,
        attacker: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attacker_name: Option<String>,
    },
    #[serde(rename = "enemy_created")]
    EnemyCreated {
        enemy: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enemy_name: Option<String>,
    },
    #[serde(rename = "enemy_finished")]
    EnemyFinished {
        enemy: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enemy_name: Option<String>,
    },
    #[serde(rename = "weapon_fired")]
    WeaponFired {
        unit: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_name: Option<String>,
        weapon_def_id: i32,
    },
    #[serde(rename = "command_finished")]
    CommandFinished {
        unit: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_name: Option<String>,
        command_id: i32,
        command_topic: i32,
    },
    #[serde(rename = "lua_message")]
    LuaMessage { data: String },
    #[serde(rename = "command_error")]
    CommandError { error: String, command: String },
    #[serde(rename = "tools_registered")]
    ToolsRegistered {
        tools: Vec<ToolDefinition>,
        source: String,
    },
    #[serde(rename = "tools_unregistered")]
    ToolsUnregistered {
        tool_names: Vec<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        call_id: String,
        content: Vec<serde_json::Value>,
        #[serde(default)]
        is_error: bool,
    },
}

/// A command to send to a SAI bridge instance.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SaiCommand {
    #[serde(rename = "move")]
    Move {
        unit_id: i32,
        x: f32,
        #[serde(default)]
        y: f32,
        z: f32,
        #[serde(default)]
        queue: bool,
    },
    #[serde(rename = "stop")]
    Stop { unit_id: i32 },
    #[serde(rename = "attack")]
    Attack {
        unit_id: i32,
        target_id: i32,
        #[serde(default)]
        queue: bool,
    },
    #[serde(rename = "build")]
    Build {
        unit_id: i32,
        #[serde(default)]
        build_def_id: i32,
        #[serde(default)]
        build_def_name: Option<String>,
        #[serde(default)]
        x: f32,
        #[serde(default)]
        y: f32,
        #[serde(default)]
        z: f32,
        #[serde(default)]
        facing: i32,
        #[serde(default)]
        queue: bool,
    },
    #[serde(rename = "patrol")]
    Patrol {
        unit_id: i32,
        x: f32,
        #[serde(default)]
        y: f32,
        z: f32,
        #[serde(default)]
        queue: bool,
    },
    #[serde(rename = "fight")]
    Fight {
        unit_id: i32,
        x: f32,
        #[serde(default)]
        y: f32,
        z: f32,
        #[serde(default)]
        queue: bool,
    },
    #[serde(rename = "guard")]
    Guard {
        unit_id: i32,
        guard_id: i32,
        #[serde(default)]
        queue: bool,
    },
    #[serde(rename = "repair")]
    Repair {
        unit_id: i32,
        repair_id: i32,
        #[serde(default)]
        queue: bool,
    },
    #[serde(rename = "set_fire_state")]
    SetFireState { unit_id: i32, state: i32 },
    #[serde(rename = "set_move_state")]
    SetMoveState { unit_id: i32, state: i32 },
    #[serde(rename = "send_chat")]
    SendChat { text: String },
    #[serde(rename = "pause")]
    Pause,
    #[serde(rename = "unpause")]
    Unpause,
    #[serde(rename = "set_speed")]
    SetSpeed { speed: f32 },
    #[serde(rename = "tool_call")]
    ToolCall {
        call_id: String,
        tool: String,
        args: serde_json::Value,
    },
}

/// A connected SAI bridge instance.
pub struct SaiConnection {
    pub channel_id: String,
    writer: tokio::io::WriteHalf<UnixStream>,
    reader: BufReader<tokio::io::ReadHalf<UnixStream>>,
    read_buf: String,
}

impl SaiConnection {
    pub fn new(channel_id: String, stream: UnixStream) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self {
            channel_id,
            writer,
            reader: BufReader::new(reader),
            read_buf: String::new(),
        }
    }

    /// Read the next event from this SAI connection.
    /// Returns None on EOF.
    pub async fn next_event(&mut self) -> Option<SaiEvent> {
        loop {
            self.read_buf.clear();
            match self.reader.read_line(&mut self.read_buf).await {
                Ok(0) => return None, // EOF
                Ok(_) => {
                    let trimmed = self.read_buf.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str(trimmed) {
                        Ok(event) => return Some(event),
                        Err(e) => {
                            tracing::warn!("Failed to parse SAI event: {} — {:?}", e, trimmed);
                            continue;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("SAI read error: {}", e);
                    return None;
                }
            }
        }
    }

    /// Send a command to this SAI connection.
    pub async fn send_command(&mut self, cmd: &SaiCommand) -> Result<(), std::io::Error> {
        let json = serde_json::to_string(cmd).unwrap();
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }
}

/// Manages SAI IPC connections.
pub struct SaiIpcServer {
    pub listeners: HashMap<String, std::os::unix::net::UnixListener>,
    pub connections: HashMap<String, SaiConnection>,
}

impl SaiIpcServer {
    pub fn new() -> Self {
        Self {
            listeners: HashMap::new(),
            connections: HashMap::new(),
        }
    }

    /// Start listening for a specific channel's SAI connection.
    pub fn listen_for(&mut self, channel_id: &str, socket_path: &str) -> Result<(), String> {
        // Remove existing socket file if present
        let _ = std::fs::remove_file(socket_path);

        let listener = std::os::unix::net::UnixListener::bind(socket_path)
            .map_err(|e| format!("Failed to bind {}: {}", socket_path, e))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to set nonblocking: {}", e))?;

        self.listeners
            .insert(channel_id.to_string(), listener);
        Ok(())
    }

    /// Stop listening for a channel and close any active connection.
    pub fn close_channel(&mut self, channel_id: &str) {
        self.listeners.remove(channel_id);
        self.connections.remove(channel_id);
    }

    /// Accept any pending connections from SAI bridges (non-blocking).
    /// Returns channel IDs of newly connected SAIs.
    pub fn accept_pending(&mut self) -> Vec<String> {
        let mut connected = Vec::new();
        let channel_ids: Vec<String> = self.listeners.keys().cloned().collect();

        for channel_id in channel_ids {
            if self.connections.contains_key(&channel_id) {
                continue; // Already connected
            }
            if let Some(listener) = self.listeners.get(&channel_id) {
                match listener.accept() {
                    Ok((std_stream, _addr)) => {
                        tracing::info!("SAI connected for channel {}", channel_id);
                        // Convert std stream to tokio
                        std_stream.set_nonblocking(true).ok();
                        match UnixStream::from_std(std_stream) {
                            Ok(stream) => {
                                let conn = SaiConnection::new(channel_id.clone(), stream);
                                self.connections.insert(channel_id.clone(), conn);
                                connected.push(channel_id);
                            }
                            Err(e) => {
                                tracing::error!("Failed to convert stream for {}: {}", channel_id, e);
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // No connection yet
                    }
                    Err(e) => {
                        tracing::error!("SAI accept error for {}: {}", channel_id, e);
                    }
                }
            }
        }

        connected
    }

    /// Send a command to a specific channel's SAI.
    pub async fn send_to(
        &mut self,
        channel_id: &str,
        cmd: &SaiCommand,
    ) -> Result<(), String> {
        let conn = self
            .connections
            .get_mut(channel_id)
            .ok_or_else(|| format!("No SAI connection for channel {}", channel_id))?;
        conn.send_command(cmd)
            .await
            .map_err(|e| format!("Failed to send to SAI: {}", e))
    }
}

/// Convert a channels/publish content text into a SaiCommand.
pub fn parse_publish_command(text: &str) -> Result<SaiCommand, String> {
    serde_json::from_str(text).map_err(|e| format!("Invalid command JSON: {}", e))
}

/// Convert a SaiEvent into MCPL channels/incoming content.
pub fn event_to_content(event: &SaiEvent) -> String {
    serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string())
}
