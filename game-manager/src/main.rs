mod engine;
mod lobby;
mod mcpl_server;
mod sai_ipc;
mod tool_registry;
mod write_dir;

use engine::EngineManager;
use lobby::*;
use mcpl_core::connection::IncomingMessage as McplIncoming;
use mcpl_core::methods::*;
use mcpl_core::types::*;
use sai_ipc::SaiIpcServer;
use tool_registry::ToolRegistry;
use write_dir::WriteDirConfig;

use std::path::PathBuf;
use tokio::net::TcpListener;

struct GameManager {
    mcpl: Option<mcpl_core::McplConnection>,
    lobby_conn: Option<LobbyConnection>,
    lobby_state: LobbyState,
    engines: EngineManager,
    sai: SaiIpcServer,
    tool_registry: ToolRegistry,
    write_dir: PathBuf,
    spring_home: PathBuf,
    agent_name: String,
}

impl GameManager {
    fn new(write_dir_config: &WriteDirConfig, engine_dir: PathBuf, socket_dir: String) -> Self {
        Self {
            mcpl: None,
            lobby_conn: None,
            lobby_state: LobbyState::new(),
            engines: EngineManager::new(
                engine_dir,
                write_dir_config.write_dir.clone(),
                socket_dir,
            ),
            sai: SaiIpcServer::new(),
            tool_registry: ToolRegistry::new(),
            write_dir: write_dir_config.write_dir.clone(),
            spring_home: write_dir_config.spring_home.clone(),
            agent_name: write_dir_config.agent_name.clone(),
        }
    }

    /// Handle an MCPL tool call from the AF client.
    async fn handle_tool_call(
        &mut self,
        name: &str,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        match name {
            "lobby_connect" => self.tool_lobby_connect(args).await,
            "lobby_login" => self.tool_lobby_login(args).await,
            "lobby_register" => self.tool_lobby_register(args).await,
            "lobby_disconnect" => self.tool_lobby_disconnect().await,
            "lobby_say" => self.tool_lobby_say(args).await,
            "lobby_join_channel" => self.tool_lobby_join_channel(args).await,
            "lobby_leave_channel" => self.tool_lobby_leave_channel(args).await,
            "lobby_list_battles" => self.tool_lobby_list_battles().await,
            "lobby_list_users" => self.tool_lobby_list_users(args).await,
            "lobby_join_battle" => self.tool_lobby_join_battle(args).await,
            "lobby_leave_battle" => self.tool_lobby_leave_battle().await,
            "lobby_matchmaker_join" => self.tool_lobby_matchmaker_join(args).await,
            "lobby_matchmaker_leave" => self.tool_lobby_matchmaker_leave().await,
            "lobby_matchmaker_accept" => self.tool_lobby_matchmaker_accept(args).await,
            "lobby_matchmaker_status" => self.tool_lobby_matchmaker_status().await,
            "lobby_start_game" => self.tool_lobby_start_game(args).await,
            "lobby_open_battle" => self.tool_lobby_open_battle(args).await,
            "lobby_add_bot" => self.tool_lobby_add_bot(args).await,
            "lobby_remove_bot" => self.tool_lobby_remove_bot(args).await,
            "lobby_start_battle" => self.tool_lobby_start_battle().await,
            _ => serde_json::json!({
                "content": [{"type": "text", "text": format!("Unknown tool: {}", name)}],
                "isError": true
            }),
        }
    }

    // ── MCPL channel methods ──

    async fn handle_channels_open(
        &mut self,
        params: &serde_json::Value,
    ) -> serde_json::Value {
        // Guard: don't open a new game channel if one already exists
        if !self.engines.instances.is_empty() {
            let channels: Vec<&str> = self.engines.instances.keys().map(|s| s.as_str()).collect();
            return serde_json::json!({
                "error": { "code": -32000, "message": format!("Game already running (channels: {}). Close existing game first.", channels.join(", ")) }
            });
        }

        let map = params
            .get("address")
            .and_then(|a| a.get("map"))
            .and_then(|v| v.as_str())
            .unwrap_or("Chicken Defence 1.56");
        let game = params
            .get("address")
            .and_then(|a| a.get("game"))
            .and_then(|v| v.as_str())
            .unwrap_or("Zero-K v1.12.1.0");
        let opponent = params
            .get("address")
            .and_then(|a| a.get("opponent"))
            .and_then(|v| v.as_str());
        let player_mode = params
            .get("address")
            .and_then(|a| a.get("player_mode"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let headless = if player_mode {
            false
        } else {
            params
                .get("address")
                .and_then(|a| a.get("headless"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
        };

        match self.engines.start_local_game(map, game, opponent, headless, player_mode, &self.agent_name).await {
            Ok(channel_id) => {
                // Set up SAI IPC listener for this channel
                let socket_path = self
                    .engines
                    .instances
                    .get(&channel_id)
                    .map(|i| i.config.socket_path.clone())
                    .unwrap_or_default();

                if let Err(e) = self.sai.listen_for(&channel_id, &socket_path) {
                    tracing::error!("Failed to set up SAI listener: {}", e);
                }

                // Send channels/changed notification
                self.send_channels_changed(
                    vec![ChannelDescriptor {
                        id: channel_id.clone(),
                        channel_type: "game".into(),
                        label: format!("Game on {}", map),
                        direction: ChannelDirection::Bidirectional,
                        address: None,
                        metadata: Some(serde_json::json!({
                            "map": map,
                            "game": game,
                            "status": "starting",
                        })),
                    }],
                    vec![],
                    vec![],
                )
                .await;

                serde_json::json!({
                    "channel": {
                        "id": channel_id,
                        "type": "game",
                        "label": format!("Game on {}", map),
                        "direction": "bidirectional",
                        "metadata": {
                            "map": map,
                            "game": game,
                            "status": "starting"
                        }
                    }
                })
            }
            Err(e) => serde_json::json!({
                "error": { "code": -32000, "message": e }
            }),
        }
    }

    async fn handle_channels_close(
        &mut self,
        params: &serde_json::Value,
    ) -> serde_json::Value {
        let channel_id = match params.get("channelId").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => {
                return serde_json::json!({
                    "closed": false,
                    "error": "Missing channelId"
                })
            }
        };

        self.sai.close_channel(&channel_id);
        let removed_tools = self.tool_registry.remove_channel(&channel_id);
        if !removed_tools.is_empty() {
            if let Some(mcpl) = &mut self.mcpl {
                let _ = mcpl.send_notification(
                    "notifications/tools/list_changed",
                    None,
                ).await;
            }
        }
        if let Err(e) = self.engines.stop_game(&channel_id).await {
            return serde_json::json!({
                "closed": false,
                "error": e
            });
        }

        // Notify channels/changed
        self.send_channels_changed(vec![], vec![channel_id], vec![])
            .await;

        serde_json::json!({ "closed": true })
    }

    async fn handle_channels_list(&self) -> serde_json::Value {
        let channels: Vec<serde_json::Value> = self
            .engines
            .instances
            .iter()
            .map(|(id, inst)| {
                let connected = self.sai.connections.contains_key(id);
                serde_json::json!({
                    "id": id,
                    "type": "game",
                    "label": format!("Game on {}", inst.config.map),
                    "direction": "bidirectional",
                    "metadata": {
                        "map": inst.config.map,
                        "game": inst.config.game,
                        "status": format!("{:?}", inst.status),
                        "saiConnected": connected,
                    }
                })
            })
            .collect();

        serde_json::json!({ "channels": channels })
    }

    async fn handle_channels_publish(
        &mut self,
        params: &serde_json::Value,
    ) -> serde_json::Value {
        let channel_id = match params.get("channelId").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => {
                return serde_json::json!({
                    "delivered": false,
                    "error": "Missing channelId"
                })
            }
        };

        let content = params
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|block| block.get("text"))
            .map(|v| {
                if let Some(s) = v.as_str() {
                    s.to_string()
                } else {
                    // Agent sent JSON object instead of string — stringify it
                    v.to_string()
                }
            })
            .unwrap_or_default();

        let cmd = match sai_ipc::parse_publish_command(&content) {
            Ok(c) => c,
            Err(e) => {
                return serde_json::json!({
                    "delivered": false,
                    "error": e
                })
            }
        };

        match self.sai.send_to(channel_id, &cmd).await {
            Ok(()) => serde_json::json!({
                "delivered": true,
                "messageId": uuid::Uuid::new_v4().to_string()
            }),
            Err(e) => serde_json::json!({
                "delivered": false,
                "error": e
            }),
        }
    }

    async fn handle_state_rollback(
        &mut self,
        params: &serde_json::Value,
    ) -> serde_json::Value {
        let _feature_set = params
            .get("featureSet")
            .and_then(|v| v.as_str())
            .unwrap_or("game");
        let checkpoint = params
            .get("checkpoint")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // For now, rollback is a placeholder — full implementation
        // requires engine savestate support
        serde_json::json!({
            "success": false,
            "checkpoint": checkpoint,
            "reason": "Rollback not yet implemented — requires engine savestate support"
        })
    }

    // ── Notification helpers ──

    async fn send_channels_changed(
        &mut self,
        added: Vec<ChannelDescriptor>,
        removed: Vec<String>,
        updated: Vec<ChannelDescriptor>,
    ) {
        if let Some(mcpl) = &mut self.mcpl {
            let params = ChannelsChangedParams {
                added: if added.is_empty() {
                    None
                } else {
                    Some(added)
                },
                removed: if removed.is_empty() {
                    None
                } else {
                    Some(removed)
                },
                updated: if updated.is_empty() {
                    None
                } else {
                    Some(updated)
                },
            };
            let _ = mcpl
                .send_notification(
                    method::CHANNELS_CHANGED,
                    Some(serde_json::to_value(&params).unwrap()),
                )
                .await;
        }
    }

    /// Forward a SAI event as channels/incoming to the MCPL client.
    async fn forward_sai_event(
        &mut self,
        channel_id: &str,
        event: &sai_ipc::SaiEvent,
    ) {
        let mcpl = match &mut self.mcpl {
            Some(c) => c,
            None => return,
        };

        let content_text = sai_ipc::event_to_content(event);
        let msg_id = uuid::Uuid::new_v4().to_string();

        let params = ChannelsIncomingParams {
            messages: vec![mcpl_core::methods::IncomingChannelMessage {
                channel_id: channel_id.to_string(),
                message_id: msg_id,
                thread_id: None,
                author: MessageAuthor {
                    id: "engine".into(),
                    name: "Game Engine".into(),
                },
                content: vec![ContentBlock::text(content_text)],
                timestamp: chrono::Utc::now().to_rfc3339(),
                metadata: None,
            }],
        };

        let _ = mcpl
            .send_request(
                method::CHANNELS_INCOMING,
                Some(serde_json::to_value(&params).unwrap()),
            )
            .await;
    }

    // ── Lobby tool implementations (unchanged) ──

    async fn tool_lobby_connect(&mut self, args: &serde_json::Value) -> serde_json::Value {
        // Idempotent: if already connected, return success
        if self.lobby_conn.is_some() {
            let status = if self.lobby_state.logged_in {
                format!("Already connected and logged in as '{}'", self.lobby_state.my_username.as_deref().unwrap_or("?"))
            } else {
                "Already connected (not yet logged in)".to_string()
            };
            return serde_json::json!({
                "content": [{"type": "text", "text": status}]
            });
        }

        let host = args
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or("zero-k.info");
        let port = args.get("port").and_then(|v| v.as_u64()).unwrap_or(8200) as u16;

        match LobbyConnection::connect(host, port).await {
            Ok(conn) => {
                self.lobby_conn = Some(conn);
                serde_json::json!({
                    "content": [{"type": "text", "text": format!("Connected to {}:{}", host, port)}]
                })
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": format!("Connection failed: {}", e)}],
                "isError": true
            }),
        }
    }

    /// Read lobby messages until we see `response_command`, or timeout.
    /// Handles pings and updates lobby state for other messages while waiting.
    /// Takes the connection out of self to satisfy the borrow checker.
    async fn await_lobby_response(
        &mut self,
        response_command: &str,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, String> {
        let mut conn = match self.lobby_conn.take() {
            Some(c) => c,
            None => return Err("Not connected".into()),
        };

        let deadline = tokio::time::Instant::now()
            + tokio::time::Duration::from_secs(timeout_secs);

        let result = loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break Err(format!("Timed out waiting for {}", response_command));
            }

            match tokio::time::timeout(remaining, conn.recv()).await {
                Ok(Ok(msg)) => {
                    if msg.command == response_command {
                        break Ok(msg.data);
                    }
                    // Handle keepalive
                    if msg.command == "Ping" {
                        let pong = LobbyMessage::new("Ping", serde_json::json!({}));
                        let _ = conn.send(&pong).await;
                        continue;
                    }
                    // Process other messages for state updates
                    self.lobby_state.handle_message(&msg);
                }
                Ok(Err(e)) => {
                    self.lobby_conn = Some(conn);
                    return Err(format!("Connection error: {}", e));
                }
                Err(_) => {
                    break Err(format!("Timed out waiting for {}", response_command));
                }
            }
        };

        self.lobby_conn = Some(conn);
        result
    }

    async fn tool_lobby_login(&mut self, args: &serde_json::Value) -> serde_json::Value {
        // Idempotent: if already logged in, return success
        if self.lobby_state.logged_in {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Already logged in as '{}'", self.lobby_state.my_username.as_deref().unwrap_or("?"))}]
            });
        }

        let username = match args.get("username").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing username"}],
                    "isError": true
                })
            }
        };
        let password = match args.get("password").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing password"}],
                    "isError": true
                })
            }
        };

        if self.lobby_conn.is_none() {
            return serde_json::json!({
                "content": [{"type": "text", "text": "Not connected to lobby. Call lobby_connect first."}],
                "isError": true
            });
        }

        let cmd = LoginCommand {
            name: username.clone(),
            password_hash: hash_password(password),
            user_id: 0,
            install_id: 0,
            lobby_version: 0,
            steam_auth_token: String::new(),
            dlc: String::new(),
        };

        if let Some(conn) = &mut self.lobby_conn {
            if let Err(e) = conn.send_command("Login", &cmd).await {
                return serde_json::json!({
                    "content": [{"type": "text", "text": format!("Failed to send login: {}", e)}],
                    "isError": true
                });
            }
        }

        // Wait for the correlated response
        match self.await_lobby_response("LoginResponse", 10).await {
            Ok(data) => {
                if let Ok(resp) = serde_json::from_value::<LoginResponseData>(data) {
                    if resp.result_code == LOGIN_OK {
                        self.lobby_state.logged_in = true;
                        self.lobby_state.my_username = Some(resp.name.clone());
                        serde_json::json!({
                            "content": [{"type": "text", "text": format!("Logged in as '{}'", resp.name)}]
                        })
                    } else {
                        serde_json::json!({
                            "content": [{"type": "text", "text": format!("Login failed (code {}): {}", resp.result_code, resp.message)}],
                            "isError": true
                        })
                    }
                } else {
                    serde_json::json!({
                        "content": [{"type": "text", "text": "Login response unparseable"}],
                        "isError": true
                    })
                }
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": e}],
                "isError": true
            }),
        }
    }

    async fn tool_lobby_register(&mut self, args: &serde_json::Value) -> serde_json::Value {
        let username = match args.get("username").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing username"}],
                    "isError": true
                })
            }
        };
        let password = match args.get("password").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing password"}],
                    "isError": true
                })
            }
        };
        let email = match args.get("email").and_then(|v| v.as_str()) {
            Some(e) => e,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing email"}],
                    "isError": true
                })
            }
        };

        if self.lobby_conn.is_none() {
            return serde_json::json!({
                "content": [{"type": "text", "text": "Not connected to lobby. Call lobby_connect first."}],
                "isError": true
            });
        }

        let cmd = RegisterCommand {
            name: username.clone(),
            password_hash: hash_password(password),
            email: email.to_string(),
            user_id: 0,
            install_id: String::new(),
            steam_auth_token: String::new(),
            dlc: String::new(),
        };

        if let Some(conn) = &mut self.lobby_conn {
            if let Err(e) = conn.send_command("Register", &cmd).await {
                return serde_json::json!({
                    "content": [{"type": "text", "text": format!("Failed to send register: {}", e)}],
                    "isError": true
                });
            }
        }

        // Wait for the correlated response
        match self.await_lobby_response("RegisterResponse", 10).await {
            Ok(data) => {
                if let Ok(resp) = serde_json::from_value::<RegisterResponseData>(data) {
                    if resp.result_code == REGISTER_OK {
                        serde_json::json!({
                            "content": [{"type": "text", "text": format!("Account '{}' registered successfully", username)}]
                        })
                    } else {
                        let reason = resp.ban_reason.unwrap_or_default();
                        serde_json::json!({
                            "content": [{"type": "text", "text": format!("Registration failed (code {}): {}", resp.result_code, reason)}],
                            "isError": true
                        })
                    }
                } else {
                    serde_json::json!({
                        "content": [{"type": "text", "text": "Register response unparseable"}],
                        "isError": true
                    })
                }
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": e}],
                "isError": true
            }),
        }
    }

    async fn tool_lobby_disconnect(&mut self) -> serde_json::Value {
        self.lobby_conn = None;
        self.lobby_state = LobbyState::new();
        serde_json::json!({
            "content": [{"type": "text", "text": "Disconnected from lobby"}]
        })
    }

    async fn tool_lobby_say(&mut self, args: &serde_json::Value) -> serde_json::Value {
        let target = match args.get("target").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing target"}],
                    "isError": true
                })
            }
        };
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing text"}],
                    "isError": true
                })
            }
        };
        let place = args
            .get("place")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;

        let conn = match &mut self.lobby_conn {
            Some(c) => c,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Not connected"}],
                    "isError": true
                })
            }
        };

        let cmd = SayCommand {
            place,
            target: target.to_string(),
            text: text.to_string(),
            is_emote: false,
        };

        match conn.send_command("Say", &cmd).await {
            Ok(()) => serde_json::json!({
                "content": [{"type": "text", "text": format!("Sent to {}: {}", target, text)}]
            }),
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": format!("Send failed: {}", e)}],
                "isError": true
            }),
        }
    }

    async fn tool_lobby_join_channel(
        &mut self,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let channel = match args.get("channel").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing channel"}],
                    "isError": true
                })
            }
        };

        if self.lobby_conn.is_none() {
            return serde_json::json!({
                "content": [{"type": "text", "text": "Not connected"}],
                "isError": true
            });
        }

        let cmd = JoinChannelCommand {
            channel_name: channel.clone(),
            password: String::new(),
        };

        if let Some(conn) = &mut self.lobby_conn {
            if let Err(e) = conn.send_command("JoinChannel", &cmd).await {
                return serde_json::json!({
                    "content": [{"type": "text", "text": format!("Failed: {}", e)}],
                    "isError": true
                });
            }
        }

        match self.await_lobby_response("JoinChannelResponse", 10).await {
            Ok(data) => {
                if let Ok(resp) = serde_json::from_value::<JoinChannelResponseData>(data) {
                    if resp.success {
                        let user_count = resp.channel.as_ref()
                            .map(|c| c.users.len())
                            .unwrap_or(0);
                        let topic = resp.channel.as_ref()
                            .and_then(|c| c.topic.as_ref())
                            .map(|t| t.text.clone())
                            .unwrap_or_default();
                        // State update is handled by await_lobby_response via handle_message
                        serde_json::json!({
                            "content": [{"type": "text", "text": format!("Joined #{} ({} users). Topic: {}", channel, user_count, if topic.is_empty() { "(none)".into() } else { topic })}]
                        })
                    } else {
                        serde_json::json!({
                            "content": [{"type": "text", "text": format!("Failed to join #{}: rejected by server", channel)}],
                            "isError": true
                        })
                    }
                } else {
                    serde_json::json!({
                        "content": [{"type": "text", "text": "Join response unparseable"}],
                        "isError": true
                    })
                }
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": e}],
                "isError": true
            }),
        }
    }

    async fn tool_lobby_leave_channel(
        &mut self,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let channel = match args.get("channel").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing channel"}],
                    "isError": true
                })
            }
        };

        let conn = match &mut self.lobby_conn {
            Some(c) => c,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Not connected"}],
                    "isError": true
                })
            }
        };

        let cmd = LeaveChannelCommand {
            channel_name: channel.to_string(),
        };

        match conn.send_command("LeaveChannel", &cmd).await {
            Ok(()) => {
                self.lobby_state.channels.remove(channel);
                serde_json::json!({
                    "content": [{"type": "text", "text": format!("Left #{}", channel)}]
                })
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": format!("Failed: {}", e)}],
                "isError": true
            }),
        }
    }

    async fn tool_lobby_list_battles(&mut self) -> serde_json::Value {
        let battles: Vec<serde_json::Value> = self
            .lobby_state
            .battles
            .values()
            .map(|b| {
                serde_json::json!({
                    "id": b.battle_id,
                    "title": b.title,
                    "founder": b.founder,
                    "map": b.map,
                    "players": b.player_count,
                    "maxPlayers": b.max_players,
                    "spectators": b.spectator_count,
                    "running": b.is_running,
                    "passwordProtected": b.is_password_protected,
                    "mode": b.mode,
                })
            })
            .collect();

        serde_json::json!({
            "content": [{"type": "text", "text": serde_json::to_string_pretty(&battles).unwrap()}]
        })
    }

    async fn tool_lobby_list_users(
        &mut self,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;

        let users: Vec<serde_json::Value> = self
            .lobby_state
            .users
            .values()
            .take(limit)
            .map(|u| {
                serde_json::json!({
                    "name": u.name,
                    "level": u.level,
                    "elo": u.elo,
                    "clan": u.clan,
                    "country": u.country,
                    "isBot": u.is_bot,
                    "isAdmin": u.is_admin,
                    "battleId": u.battle_id,
                })
            })
            .collect();

        serde_json::json!({
            "content": [{"type": "text", "text": format!("{} users (showing {})\n{}", self.lobby_state.users.len(), users.len(), serde_json::to_string_pretty(&users).unwrap())}]
        })
    }

    async fn tool_lobby_join_battle(
        &mut self,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let battle_id = match args.get("battle_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing battle_id"}],
                    "isError": true
                })
            }
        };
        let password = args
            .get("password")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if self.lobby_conn.is_none() {
            return serde_json::json!({
                "content": [{"type": "text", "text": "Not connected"}],
                "isError": true
            });
        }

        let cmd = JoinBattleCommand {
            battle_id,
            password: password.to_string(),
        };

        if let Some(conn) = &mut self.lobby_conn {
            if let Err(e) = conn.send_command("JoinBattle", &cmd).await {
                return serde_json::json!({
                    "content": [{"type": "text", "text": format!("Failed: {}", e)}],
                    "isError": true
                });
            }
        }

        // ZKLS has no explicit failure message for JoinBattle — only JoinBattleSuccess on success.
        // Timeout means rejection (wrong password, battle full, etc.)
        match self.await_lobby_response("JoinBattleSuccess", 10).await {
            Ok(data) => {
                if let Ok(resp) = serde_json::from_value::<JoinBattleSuccessData>(data) {
                    self.lobby_state.my_battle = Some(resp.battle_id);
                    let player_count = resp.players.len();
                    let bot_count = resp.bots.len();

                    // Report sync status
                    self.send_battle_sync().await;

                    serde_json::json!({
                        "content": [{"type": "text", "text": format!("Joined battle {} ({} players, {} bots)", resp.battle_id, player_count, bot_count)}]
                    })
                } else {
                    self.lobby_state.my_battle = Some(battle_id);
                    self.send_battle_sync().await;
                    serde_json::json!({
                        "content": [{"type": "text", "text": format!("Joined battle {}", battle_id)}]
                    })
                }
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": format!("Failed to join battle {}: {}", battle_id, e)}],
                "isError": true
            }),
        }
    }

    async fn tool_lobby_leave_battle(&mut self) -> serde_json::Value {
        let conn = match &mut self.lobby_conn {
            Some(c) => c,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Not connected"}],
                    "isError": true
                })
            }
        };

        let cmd = LeaveBattleCommand { battle_id: None };

        match conn.send_command("LeaveBattle", &cmd).await {
            Ok(()) => {
                self.lobby_state.my_battle = None;
                serde_json::json!({
                    "content": [{"type": "text", "text": "Left battle"}]
                })
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": format!("Failed: {}", e)}],
                "isError": true
            }),
        }
    }

    // ── Matchmaker tool implementations ──

    async fn tool_lobby_matchmaker_join(
        &mut self,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let queues: Vec<String> = match args.get("queues").and_then(|v| v.as_array()) {
            Some(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing queues array"}],
                    "isError": true
                })
            }
        };

        if queues.is_empty() {
            return serde_json::json!({
                "content": [{"type": "text", "text": "Queues array is empty. Use lobby_matchmaker_leave to leave all queues."}],
                "isError": true
            });
        }

        if self.lobby_conn.is_none() {
            return serde_json::json!({
                "content": [{"type": "text", "text": "Not connected"}],
                "isError": true
            });
        }

        let cmd = MatchMakerQueueRequestCommand {
            queues: queues.clone(),
        };

        if let Some(conn) = &mut self.lobby_conn {
            if let Err(e) = conn.send_command("MatchMakerQueueRequest", &cmd).await {
                return serde_json::json!({
                    "content": [{"type": "text", "text": format!("Failed: {}", e)}],
                    "isError": true
                });
            }
        }

        // Wait for MatchMakerStatus response confirming our queue state
        match self.await_lobby_response("MatchMakerStatus", 10).await {
            Ok(data) => {
                if let Ok(status) = serde_json::from_value::<MatchMakerStatusData>(data) {
                    self.lobby_state.matchmaker_joined = status.joined_queues.clone();
                    self.lobby_state.matchmaker_queue_counts = status.queue_counts.clone();
                    let joined = status.joined_queues.join(", ");
                    let counts: Vec<String> = status
                        .queue_counts
                        .iter()
                        .filter(|(name, _)| status.joined_queues.contains(name))
                        .map(|(name, count)| format!("{}: {} queued", name, count))
                        .collect();
                    if status.joined_queues.is_empty() {
                        serde_json::json!({
                            "content": [{"type": "text", "text": format!("Failed to join queues (may be banned for {}s)", status.banned_seconds.unwrap_or(0))}],
                            "isError": true
                        })
                    } else {
                        serde_json::json!({
                            "content": [{"type": "text", "text": format!("Joined matchmaker queues: [{}]. {}", joined, counts.join(", "))}]
                        })
                    }
                } else {
                    serde_json::json!({
                        "content": [{"type": "text", "text": "MatchMakerStatus unparseable"}],
                        "isError": true
                    })
                }
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": e}],
                "isError": true
            }),
        }
    }

    async fn tool_lobby_matchmaker_leave(&mut self) -> serde_json::Value {
        if self.lobby_conn.is_none() {
            return serde_json::json!({
                "content": [{"type": "text", "text": "Not connected"}],
                "isError": true
            });
        }

        let cmd = MatchMakerQueueRequestCommand {
            queues: vec![],
        };

        if let Some(conn) = &mut self.lobby_conn {
            if let Err(e) = conn.send_command("MatchMakerQueueRequest", &cmd).await {
                return serde_json::json!({
                    "content": [{"type": "text", "text": format!("Failed: {}", e)}],
                    "isError": true
                });
            }
        }

        match self.await_lobby_response("MatchMakerStatus", 10).await {
            Ok(data) => {
                if let Ok(status) = serde_json::from_value::<MatchMakerStatusData>(data) {
                    self.lobby_state.matchmaker_joined = status.joined_queues.clone();
                    self.lobby_state.matchmaker_queue_counts = status.queue_counts.clone();
                    serde_json::json!({
                        "content": [{"type": "text", "text": "Left all matchmaker queues"}]
                    })
                } else {
                    self.lobby_state.matchmaker_joined.clear();
                    serde_json::json!({
                        "content": [{"type": "text", "text": "Left matchmaker queues"}]
                    })
                }
            }
            Err(e) => {
                // Even on timeout, we likely left — the server might not send status
                // if we weren't in any queue
                self.lobby_state.matchmaker_joined.clear();
                tracing::debug!("MatchMakerStatus await after leave: {}", e);
                serde_json::json!({
                    "content": [{"type": "text", "text": "Left matchmaker queues"}]
                })
            }
        }
    }

    async fn tool_lobby_matchmaker_accept(
        &mut self,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let ready = args
            .get("ready")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let conn = match &mut self.lobby_conn {
            Some(c) => c,
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Not connected"}],
                    "isError": true
                })
            }
        };

        if !self.lobby_state.matchmaker_ready_pending {
            return serde_json::json!({
                "content": [{"type": "text", "text": "No ready-check pending"}],
                "isError": true
            });
        }

        let cmd = AreYouReadyResponseCommand { ready };

        match conn.send_command("AreYouReadyResponse", &cmd).await {
            Ok(()) => {
                self.lobby_state.matchmaker_ready_pending = false;
                let action = if ready { "Accepted" } else { "Declined" };
                serde_json::json!({
                    "content": [{"type": "text", "text": format!("{} matchmaker ready-check", action)}]
                })
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": format!("Failed: {}", e)}],
                "isError": true
            }),
        }
    }

    /// Tell the lobby server we have the map/game/engine files.
    async fn send_battle_sync(&mut self) {
        let username = self.lobby_state.my_username.clone().unwrap_or_default();
        let cmd = UpdateUserBattleStatusCommand {
            name: username,
            is_spectator: Some(false),
            sync: Some("Synced".into()),
            ally_number: Some(0),
        };
        if let Some(conn) = &mut self.lobby_conn {
            if let Err(e) = conn.send_command("UpdateUserBattleStatus", &cmd).await {
                tracing::warn!("Failed to send sync status: {}", e);
            }
        }
    }

    // ── Battle hosting tool implementations ──

    async fn tool_lobby_open_battle(
        &mut self,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        // Idempotent: if already in a battle, return info about it
        if let Some(battle_id) = self.lobby_state.my_battle {
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Already in battle {}. Use lobby_leave_battle first to leave, or lobby_start_battle to start it.", battle_id)}]
            });
        }

        let title = match args.get("title").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing title"}],
                    "isError": true
                })
            }
        };
        let map = match args.get("map").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(),
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing map"}],
                    "isError": true
                })
            }
        };
        let max_players = args
            .get("max_players")
            .and_then(|v| v.as_i64())
            .unwrap_or(2) as i32;
        let password = args
            .get("password")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if !self.lobby_state.logged_in {
            return serde_json::json!({
                "content": [{"type": "text", "text": "Not logged in"}],
                "isError": true
            });
        }

        let cmd = OpenBattleCommand {
            header: BattleHeader {
                battle_id: 0,
                title: title.clone(),
                founder: self.lobby_state.my_username.clone().unwrap_or_default(),
                map: map.clone(),
                game: self.lobby_state.server_game.clone(),
                engine: self.lobby_state.server_engine.clone(),
                max_players,
                player_count: 0,
                spectator_count: 0,
                is_running: false,
                is_password_protected: !password.is_empty(),
                mode: None,
            },
        };

        if let Some(conn) = &mut self.lobby_conn {
            if let Err(e) = conn.send_command("OpenBattle", &cmd).await {
                return serde_json::json!({
                    "content": [{"type": "text", "text": format!("Failed to send OpenBattle: {}", e)}],
                    "isError": true
                });
            }
        }

        // The server responds with JoinBattleSuccess (founder auto-joins)
        match self.await_lobby_response("JoinBattleSuccess", 10).await {
            Ok(data) => {
                if let Ok(resp) = serde_json::from_value::<JoinBattleSuccessData>(data) {
                    self.lobby_state.my_battle = Some(resp.battle_id);

                    // Report sync status — tell the server we have the map/game/engine
                    self.send_battle_sync().await;

                    serde_json::json!({
                        "content": [{"type": "text", "text": format!(
                            "Opened battle '{}' on {} (battle_id: {}). Add bots with lobby_add_bot, then start with lobby_start_battle.",
                            title, map, resp.battle_id
                        )}]
                    })
                } else {
                    serde_json::json!({
                        "content": [{"type": "text", "text": "Opened battle but could not parse response"}]
                    })
                }
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": format!("Failed to open battle: {}", e)}],
                "isError": true
            }),
        }
    }

    async fn tool_lobby_add_bot(
        &mut self,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let ai_lib = match args.get("ai_lib").and_then(|v| v.as_str()) {
            Some(a) => a.to_string(),
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing ai_lib"}],
                    "isError": true
                })
            }
        };
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Bot1")
            .to_string();
        let ally_number = args
            .get("ally_number")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        if self.lobby_state.my_battle.is_none() {
            return serde_json::json!({
                "content": [{"type": "text", "text": "Not in a battle"}],
                "isError": true
            });
        }

        let cmd = UpdateBotStatusCommand {
            name: name.clone(),
            ai_lib: ai_lib.clone(),
            ally_number,
            owner: self.lobby_state.my_username.clone().unwrap_or_default(),
        };

        if let Some(conn) = &mut self.lobby_conn {
            match conn.send_command("UpdateBotStatus", &cmd).await {
                Ok(()) => serde_json::json!({
                    "content": [{"type": "text", "text": format!("Added bot '{}' (AI: {}, ally: {})", name, ai_lib, ally_number)}]
                }),
                Err(e) => serde_json::json!({
                    "content": [{"type": "text", "text": format!("Failed: {}", e)}],
                    "isError": true
                }),
            }
        } else {
            serde_json::json!({
                "content": [{"type": "text", "text": "Not connected"}],
                "isError": true
            })
        }
    }

    async fn tool_lobby_remove_bot(
        &mut self,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing bot name"}],
                    "isError": true
                })
            }
        };

        let cmd = RemoveBotCommand { name: name.clone() };

        if let Some(conn) = &mut self.lobby_conn {
            match conn.send_command("RemoveBot", &cmd).await {
                Ok(()) => serde_json::json!({
                    "content": [{"type": "text", "text": format!("Removed bot '{}'", name)}]
                }),
                Err(e) => serde_json::json!({
                    "content": [{"type": "text", "text": format!("Failed: {}", e)}],
                    "isError": true
                }),
            }
        } else {
            serde_json::json!({
                "content": [{"type": "text", "text": "Not connected"}],
                "isError": true
            })
        }
    }

    async fn tool_lobby_start_battle(&mut self) -> serde_json::Value {
        if self.lobby_state.my_battle.is_none() {
            return serde_json::json!({
                "content": [{"type": "text", "text": "Not in a battle"}],
                "isError": true
            });
        }

        // Guard: don't send !start if a game engine is already running
        if !self.engines.instances.is_empty() {
            let channels: Vec<&str> = self.engines.instances.keys().map(|s| s.as_str()).collect();
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("Game already running (channels: {}). The battle has already started.", channels.join(", "))}]
            });
        }

        // ZK custom battles are started by sending !start in battle chat.
        // The ZKLS autohost processes it and spins up a dedicated game server.
        let cmd = SayCommand {
            place: PLACE_BATTLE,
            target: String::new(),
            text: "!start".into(),
            is_emote: false,
        };

        if let Some(conn) = &mut self.lobby_conn {
            if let Err(e) = conn.send_command("Say", &cmd).await {
                return serde_json::json!({
                    "content": [{"type": "text", "text": format!("Failed to send !start: {}", e)}],
                    "isError": true
                });
            }
        }

        // Return immediately — the background event loop will receive ConnectSpring
        // and call handle_connect_spring() to launch the engine. The agent will get
        // a lobby.connect_spring push event when that happens.
        tracing::info!("Sent !start to battle chat, waiting for ConnectSpring from background loop");
        serde_json::json!({
            "content": [{"type": "text", "text": "Sent !start — waiting for game server to launch. You'll receive a lobby.connect_spring event when the engine connects."}]
        })
    }

    // ── Game tool implementations ──

    async fn tool_lobby_start_game(
        &mut self,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        // Guard: don't start a new game if one is already running
        if !self.engines.instances.is_empty() {
            let channels: Vec<&str> = self.engines.instances.keys().map(|s| s.as_str()).collect();
            return serde_json::json!({
                "content": [{"type": "text", "text": format!("A game is already running (channels: {}). Close it first with channels/close before starting a new one.", channels.join(", "))}],
                "isError": true
            });
        }

        let map = match args.get("map").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(),
            None => {
                return serde_json::json!({
                    "content": [{"type": "text", "text": "Missing map name"}],
                    "isError": true
                })
            }
        };
        let opponent = args
            .get("opponent")
            .and_then(|v| v.as_str())
            .unwrap_or("CircuitAINovice");
        let game = args
            .get("game")
            .and_then(|v| v.as_str())
            .unwrap_or("Zero-K $VERSION");
        let player_mode = args
            .get("player_mode")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let headless = if player_mode {
            false // player mode needs LuaUI for bootstrap widget
        } else {
            args.get("headless").and_then(|v| v.as_bool()).unwrap_or(true)
        };

        match self
            .engines
            .start_local_game(&map, game, Some(opponent), headless, player_mode, &self.agent_name)
            .await
        {
            Ok(channel_id) => {
                // Set up SAI IPC listener
                let socket_path = self
                    .engines
                    .instances
                    .get(&channel_id)
                    .map(|i| i.config.socket_path.clone())
                    .unwrap_or_default();

                if let Err(e) = self.sai.listen_for(&channel_id, &socket_path) {
                    tracing::error!("Failed to set up SAI listener: {}", e);
                }

                // Notify channels/changed
                self.send_channels_changed(
                    vec![ChannelDescriptor {
                        id: channel_id.clone(),
                        channel_type: "game".into(),
                        label: format!("Local game on {}", map),
                        direction: ChannelDirection::Bidirectional,
                        address: None,
                        metadata: Some(serde_json::json!({
                            "map": map,
                            "opponent": opponent,
                            "headless": headless,
                            "status": "starting",
                        })),
                    }],
                    vec![],
                    vec![],
                )
                .await;

                serde_json::json!({
                    "content": [{"type": "text", "text": format!(
                        "Started local game: AgentBridge vs {} on {} (channel: {}, headless: {})",
                        opponent, map, channel_id, headless
                    )}]
                })
            }
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": format!("Failed to start game: {}", e)}],
                "isError": true
            }),
        }
    }

    /// Handle ConnectSpring lobby event — launch engine in client mode for multiplayer.
    async fn handle_connect_spring(&mut self, data: &ConnectSpringData) {
        tracing::info!(
            "ConnectSpring received: {}:{} map={} mode={}",
            data.ip, data.port, data.map, data.mode
        );

        let player_name = self
            .lobby_state
            .my_username
            .clone()
            .unwrap_or_else(|| self.agent_name.clone());

        // Ensure the lobby username is whitelisted for /aicontrol
        if let Err(e) = crate::write_dir::ensure_player_whitelisted(
            &self.write_dir,
            &player_name,
        ) {
            tracing::warn!("Failed to whitelist player '{}': {}", player_name, e);
        }

        // Warm archive cache for the server's engine version if needed
        if !data.engine.is_empty() {
            if let Ok(mp_engine_dir) = engine::find_engine_dir(&self.spring_home, Some(&data.engine)) {
                let _ = chmod_executable(&mp_engine_dir);
                if mp_engine_dir != self.engines.engine_dir {
                    warm_archive_cache(&self.write_dir, &mp_engine_dir).await;
                }
            }
        }

        match self
            .engines
            .start_multiplayer_game(data, &player_name, &self.spring_home)
            .await
        {
            Ok(channel_id) => {
                // Set up SAI IPC listener
                let socket_path = self
                    .engines
                    .instances
                    .get(&channel_id)
                    .map(|i| i.config.socket_path.clone())
                    .unwrap_or_default();

                if let Err(e) = self.sai.listen_for(&channel_id, &socket_path) {
                    tracing::error!("Failed to set up SAI listener for MP game: {}", e);
                }

                self.send_channels_changed(
                    vec![ChannelDescriptor {
                        id: channel_id.clone(),
                        channel_type: "game".into(),
                        label: format!("MP game on {}", data.map),
                        direction: ChannelDirection::Bidirectional,
                        address: None,
                        metadata: Some(serde_json::json!({
                            "map": data.map,
                            "mode": data.mode,
                            "title": data.title,
                            "status": "connecting",
                            "multiplayer": true,
                        })),
                    }],
                    vec![],
                    vec![],
                )
                .await;

                tracing::info!("Launched multiplayer engine for channel {}", channel_id);
            }
            Err(e) => {
                tracing::error!("Failed to launch engine for ConnectSpring: {}", e);
            }
        }
    }

    async fn tool_lobby_matchmaker_status(&mut self) -> serde_json::Value {
        let available: Vec<serde_json::Value> = self
            .lobby_state
            .matchmaker_queues
            .iter()
            .map(|q| {
                let count = self
                    .lobby_state
                    .matchmaker_queue_counts
                    .get(&q.name)
                    .copied()
                    .unwrap_or(0);
                serde_json::json!({
                    "name": q.name,
                    "description": q.description,
                    "maxPartySize": q.max_party_size,
                    "queued": count,
                })
            })
            .collect();

        let joined = &self.lobby_state.matchmaker_joined;
        let ready_pending = self.lobby_state.matchmaker_ready_pending;

        serde_json::json!({
            "content": [{"type": "text", "text": format!(
                "Joined queues: [{}]\nReady-check pending: {}\nAvailable queues:\n{}",
                joined.join(", "),
                ready_pending,
                serde_json::to_string_pretty(&available).unwrap()
            )}]
        })
    }

    /// Convert a lobby event to an MCPL push event and send it.
    async fn push_lobby_event(
        &mut self,
        event: &LobbyEvent,
    ) -> Result<(), mcpl_core::connection::ConnectionError> {
        let mcpl = match &mut self.mcpl {
            Some(c) => c,
            None => return Ok(()),
        };

        let (event_id, content_text) = match event {
            LobbyEvent::Connected { engine, game } => (
                "lobby.connected".to_string(),
                format!("Connected to lobby. Engine: {}, Game: {}", engine, game),
            ),
            LobbyEvent::Disconnected { reason } => (
                "lobby.disconnected".to_string(),
                format!("Disconnected: {}", reason),
            ),
            LobbyEvent::LoggedIn { username } => (
                "lobby.logged_in".to_string(),
                format!("Logged in as {}", username),
            ),
            LobbyEvent::LoginFailed { code, message } => (
                "lobby.login_failed".to_string(),
                format!("Login failed (code {}): {}", code, message),
            ),
            LobbyEvent::RegisterSuccess => (
                "lobby.register_success".to_string(),
                "Account registration successful".to_string(),
            ),
            LobbyEvent::RegisterFailed { code, reason } => (
                "lobby.register_failed".to_string(),
                format!("Registration failed (code {}): {}", code, reason),
            ),
            LobbyEvent::ChatMessage {
                user,
                text,
                target,
                place,
                ..
            } => {
                // Only forward battle chat and DMs — skip channel chatter
                match *place {
                    1 => (
                        "lobby.chat".to_string(),
                        format!("[battle] {}: {}", user, text),
                    ),
                    4 => (
                        "lobby.chat".to_string(),
                        format!("[dm] {}: {}", user, text),
                    ),
                    _ => return Ok(()), // skip channel chat, server messages, etc.
                }
            }
            LobbyEvent::BattleJoined { battle_id, player_count, bot_count } => (
                "lobby.battle_joined".to_string(),
                format!("Joined battle {} ({} players, {} bots)", battle_id, player_count, bot_count),
            ),
            LobbyEvent::ChannelJoined {
                channel,
                users,
                topic,
            } => (
                "lobby.channel_joined".to_string(),
                format!(
                    "Joined #{} ({} users). Topic: {}",
                    channel,
                    users.len(),
                    topic.as_deref().unwrap_or("(none)")
                ),
            ),
            LobbyEvent::MatchMakerReady {
                seconds_remaining,
                quick_play,
            } => (
                "lobby.matchmaker_ready".to_string(),
                format!(
                    "MATCH FOUND! Accept within {}s (quickplay: {}). Use lobby_matchmaker_accept to respond.",
                    seconds_remaining, quick_play
                ),
            ),
            LobbyEvent::MatchMakerResult {
                is_battle_starting,
                are_you_banned,
            } => (
                "lobby.matchmaker_result".to_string(),
                if *is_battle_starting {
                    "Match starting! ConnectSpring will follow.".to_string()
                } else if *are_you_banned {
                    "Match cancelled. You have been temporarily banned for not accepting.".to_string()
                } else {
                    "Match cancelled. Not enough players accepted.".to_string()
                },
            ),
            LobbyEvent::ConnectSpring(_) => (
                "lobby.connect_spring".to_string(),
                "Game starting — engine launch initiated".to_string(),
            ),
            // Skip high-frequency events that would flood the agent's context
            LobbyEvent::UserJoined(_)
            | LobbyEvent::UserLeft { .. }
            | LobbyEvent::BattleUpdated(_)
            | LobbyEvent::BattleOpened(_)
            | LobbyEvent::BattleClosed { .. }
            | LobbyEvent::ChannelUserJoined { .. }
            | LobbyEvent::ChannelUserLeft { .. }
            | LobbyEvent::MatchMakerStatus(_)
            | LobbyEvent::MatchMakerSetup { .. }
            | LobbyEvent::MatchMakerReadyUpdate(_) => {
                return Ok(());
            }
        };

        let params = PushEventParams {
            feature_set: "lobby".into(),
            event_id: format!("{}_{}", event_id, uuid::Uuid::new_v4()),
            timestamp: chrono::Utc::now().to_rfc3339(),
            origin: Some(serde_json::json!({"source": "zk-lobby"})),
            payload: PushEventPayload {
                content: vec![ContentBlock::text(content_text)],
            },
        };

        mcpl.send_request(
            method::PUSH_EVENT,
            Some(serde_json::to_value(&params).unwrap()),
        )
        .await?;

        Ok(())
    }
}

/// Parse a named CLI argument: --flag value
fn cli_arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// Ensure engine binaries in a directory are executable.
fn chmod_executable(engine_dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for name in &["spring", "spring-headless"] {
        let bin = engine_dir.join(name);
        if bin.exists() {
            let meta = std::fs::metadata(&bin)?;
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o111);
            std::fs::set_permissions(&bin, perms)?;
        }
    }
    Ok(())
}

/// Run the engine briefly to populate the archive cache in the write-dir.
async fn warm_archive_cache(write_dir: &std::path::Path, engine_dir: &std::path::Path) {
    let headless = engine::resolve_engine_binary(engine_dir, true);
    if !headless.exists() {
        tracing::warn!("Cannot warm cache: {} not found", headless.display());
        return;
    }

    tracing::info!("Warming archive cache with {}...", engine_dir.display());
    let dummy_script = write_dir.join("temp/cache_warm.txt");
    let _ = tokio::fs::write(&dummy_script, "[GAME]\n{\n}\n").await;

    let result = tokio::process::Command::new(&headless)
        .arg("--write-dir")
        .arg(write_dir)
        .arg(&dummy_script)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    let _ = tokio::fs::remove_file(&dummy_script).await;

    match result {
        Ok(status) => tracing::info!("Cache warm-up done (exit: {})", status),
        Err(e) => tracing::warn!("Cache warm-up failed: {}", e),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Tracing always goes to stderr — safe for stdio mode
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "game_manager=info,mcpl_core=debug".parse().unwrap()),
        )
        .init();

    let use_stdio = std::env::args().any(|a| a == "--stdio");

    // Write-dir configuration: CLI args > env vars > defaults
    let wdc = WriteDirConfig::from_env(
        cli_arg("--write-dir").as_deref(),
        cli_arg("--spring-home").as_deref(),
        cli_arg("--agent-name").as_deref(),
    );

    // Initialize write directory (creates dirs, symlinks, installs SAI bridge)
    wdc.init()?;

    // Discover engine binary
    let engine_version = cli_arg("--engine-version");
    let engine_dir = engine::find_engine_dir(&wdc.spring_home, engine_version.as_deref())?;
    tracing::info!("Using engine at {}", engine_dir.display());

    // Warm archive cache: --warm-engine <version> to target a specific engine, then exit
    if let Some(warm_ver) = cli_arg("--warm-engine") {
        let warm_dir = engine::find_engine_dir(&wdc.spring_home, Some(&warm_ver))?;
        // Make sure the binary is executable
        let _ = chmod_executable(&warm_dir);
        warm_archive_cache(&wdc.write_dir, &warm_dir).await;
        tracing::info!("Cache warmed for engine {}. Exiting.", warm_ver);
        return Ok(());
    }

    // Cache warm on startup (opt-in with --cache-warm)
    if std::env::args().any(|a| a == "--cache-warm") {
        warm_archive_cache(&wdc.write_dir, &engine_dir).await;
    }
    // Note: multiplayer games may use a different engine — handle_connect_spring
    // warms the cache for that engine before launching.

    let socket_dir = std::env::var("SOCKET_DIR").unwrap_or_else(|_| "/tmp".into());

    let mcpl_conn = if use_stdio {
        mcpl_server::accept_mcpl_stdio().await?
    } else {
        let mcpl_port: u16 = std::env::var("MCPL_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(9800);

        let listener = TcpListener::bind(format!("127.0.0.1:{}", mcpl_port)).await?;
        tracing::info!("GameManager MCPL server listening on port {}", mcpl_port);

        mcpl_server::accept_mcpl_client(&listener).await?
    };
    tracing::info!("MCPL client connected and initialized");

    let mut gm = GameManager::new(&wdc, engine_dir, socket_dir);
    gm.mcpl = Some(mcpl_conn);

    // Engine check interval
    let mut engine_check = tokio::time::interval(tokio::time::Duration::from_millis(100));

    // Main event loop
    loop {
        let lobby_msg = async {
            if let Some(conn) = &mut gm.lobby_conn {
                conn.recv().await
            } else {
                std::future::pending().await
            }
        };

        let mcpl_msg = async {
            if let Some(conn) = &mut gm.mcpl {
                conn.next_message().await
            } else {
                std::future::pending().await
            }
        };

        tokio::select! {
            result = lobby_msg => {
                match result {
                    Ok(msg) => {
                        if msg.command == "Ping" {
                            if let Some(conn) = &mut gm.lobby_conn {
                                let pong = LobbyMessage::new("Ping", serde_json::json!({}));
                                if let Err(e) = conn.send(&pong).await {
                                    tracing::error!("Failed to send ping response: {}", e);
                                }
                            }
                            continue;
                        }

                        tracing::info!("Lobby msg: {} {}", msg.command, msg.data);
                        let events = gm.lobby_state.handle_message(&msg);
                        for event in &events {
                            // Handle ConnectSpring by launching the engine
                            if let LobbyEvent::ConnectSpring(data) = event {
                                tracing::info!("Background loop received ConnectSpring — launching engine");
                                gm.handle_connect_spring(data).await;
                            }
                            if let Err(e) = gm.push_lobby_event(event).await {
                                tracing::error!("Failed to push lobby event: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Lobby connection error: {}", e);
                        gm.lobby_conn = None;
                        gm.lobby_state.connected = false;
                        gm.lobby_state.logged_in = false;
                        let event = LobbyEvent::Disconnected { reason: e.to_string() };
                        let _ = gm.push_lobby_event(&event).await;
                    }
                }
            }

            result = mcpl_msg => {
                match result {
                    Ok(msg) => {
                        match msg {
                            McplIncoming::Request(req) => {
                                let result = match req.method.as_str() {
                                    "tools/list" => {
                                        let mut result = mcpl_server::lobby_tools();
                                        // Merge dynamic tools from widget/plugin registry
                                        if gm.tool_registry.has_tools() {
                                            if let Some(tools_arr) = result.get_mut("tools").and_then(|v| v.as_array_mut()) {
                                                tools_arr.extend(gm.tool_registry.list_tools());
                                            }
                                        }
                                        result
                                    }
                                    "tools/call" => {
                                        let params = req.params.unwrap_or_default();
                                        let tool_name = params.get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let tool_args = params.get("arguments")
                                            .cloned()
                                            .unwrap_or(serde_json::json!({}));

                                        // Check if this is a dynamic tool (widget/plugin)
                                        if let Some(channel_id) = gm.tool_registry.resolve_tool(&tool_name).map(String::from) {
                                            let call_id = uuid::Uuid::new_v4().to_string();
                                            let cmd = sai_ipc::SaiCommand::ToolCall {
                                                call_id: call_id.clone(),
                                                tool: tool_name.clone(),
                                                args: tool_args,
                                            };
                                            match gm.sai.send_to(&channel_id, &cmd).await {
                                                Ok(()) => {
                                                    // Defer the response — track the pending call
                                                    gm.tool_registry.track_call(call_id.clone(), req.id.clone());
                                                    tracing::debug!("Deferred tool call {} -> channel {}", call_id, channel_id);
                                                    continue; // Skip sending response now
                                                }
                                                Err(e) => {
                                                    serde_json::json!({
                                                        "content": [{"type": "text", "text": format!("Failed to route tool call: {}", e)}],
                                                        "isError": true
                                                    })
                                                }
                                            }
                                        } else {
                                            // Built-in lobby tool
                                            gm.handle_tool_call(&tool_name, &tool_args).await
                                        }
                                    }
                                    "channels/open" => {
                                        let params = req.params.unwrap_or_default();
                                        gm.handle_channels_open(&params).await
                                    }
                                    "channels/close" => {
                                        let params = req.params.unwrap_or_default();
                                        gm.handle_channels_close(&params).await
                                    }
                                    "channels/list" => {
                                        gm.handle_channels_list().await
                                    }
                                    "channels/publish" => {
                                        let params = req.params.unwrap_or_default();
                                        gm.handle_channels_publish(&params).await
                                    }
                                    "state/rollback" => {
                                        let params = req.params.unwrap_or_default();
                                        gm.handle_state_rollback(&params).await
                                    }
                                    _ => {
                                        tracing::warn!("Unknown MCPL method: {}", req.method);
                                        serde_json::json!({
                                            "error": { "code": -32601, "message": format!("Method not found: {}", req.method) }
                                        })
                                    }
                                };

                                if let Some(mcpl) = &mut gm.mcpl {
                                    if let Err(e) = mcpl.send_response(req.id, result).await {
                                        tracing::error!("Failed to send response: {}", e);
                                    }
                                }
                            }
                            McplIncoming::Notification(notif) => {
                                match notif.method.as_str() {
                                    "featureSets/update" => {
                                        tracing::info!("Feature sets update: {:?}", notif.params);
                                    }
                                    _ => {
                                        tracing::trace!("Unhandled notification: {}", notif.method);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("MCPL client disconnected: {}", e);
                        break;
                    }
                }
            }

            _ = engine_check.tick() => {
                // Check for SAI connections
                let newly_connected = gm.sai.accept_pending();
                for channel_id in &newly_connected {
                    tracing::info!("SAI connected for channel {}", channel_id);
                    if let Some(inst) = gm.engines.instances.get_mut(channel_id) {
                        inst.status = engine::GameStatus::Running;
                    }
                    gm.send_channels_changed(
                        vec![],
                        vec![],
                        vec![ChannelDescriptor {
                            id: channel_id.clone(),
                            channel_type: "game".into(),
                            label: "Game".into(),
                            direction: ChannelDirection::Bidirectional,
                            address: None,
                            metadata: Some(serde_json::json!({"status": "running", "saiConnected": true})),
                        }],
                    ).await;
                }

                // Check for engine crashes
                let changed = gm.engines.check_all().await;
                for (channel_id, status) in &changed {
                    tracing::warn!("Engine {} status changed: {:?}", channel_id, status);
                    gm.sai.close_channel(channel_id);
                    let removed_tools = gm.tool_registry.remove_channel(channel_id);
                    if !removed_tools.is_empty() {
                        tracing::info!("Removed {} tools from closed channel {}", removed_tools.len(), channel_id);
                        if let Some(mcpl) = &mut gm.mcpl {
                            let _ = mcpl.send_notification(
                                "notifications/tools/list_changed",
                                None,
                            ).await;
                        }
                    }
                    gm.send_channels_changed(
                        vec![],
                        vec![channel_id.clone()],
                        vec![],
                    ).await;
                }

                // Timeout pending tool calls (5 second timeout)
                let timed_out = gm.tool_registry.collect_timed_out(
                    std::time::Duration::from_secs(5)
                );
                for (call_id, mcpl_req_id) in timed_out {
                    tracing::warn!("Tool call {} timed out", call_id);
                    let result = serde_json::json!({
                        "content": [{"type": "text", "text": "Tool call timed out (5s)"}],
                        "isError": true,
                    });
                    if let Some(mcpl) = &mut gm.mcpl {
                        let _ = mcpl.send_response(mcpl_req_id, result).await;
                    }
                }

                // Read events from connected SAIs
                let channel_ids: Vec<String> = gm.sai.connections.keys().cloned().collect();
                for channel_id in channel_ids {
                    // Collect events from this SAI
                    let mut events = Vec::new();
                    if let Some(conn) = gm.sai.connections.get_mut(&channel_id) {
                        // Non-blocking poll: try to read available events
                        // (next_event is async but the socket is set up for line-buffered reads)
                        loop {
                            // Use tokio::time::timeout for a quick check
                            match tokio::time::timeout(
                                tokio::time::Duration::from_millis(1),
                                conn.next_event(),
                            ).await {
                                Ok(Some(event)) => events.push(event),
                                Ok(None) => {
                                    // EOF — SAI disconnected
                                    tracing::warn!("SAI disconnected for {}", channel_id);
                                    break;
                                }
                                Err(_) => break, // timeout — no more events
                            }
                        }
                    }

                    // Process events
                    for event in &events {
                        // Skip Update ticks — noise for the LLM
                        if matches!(event, sai_ipc::SaiEvent::Update { .. }) {
                            continue;
                        }

                        // Handle dynamic tool registration
                        match event {
                            sai_ipc::SaiEvent::ToolsRegistered { tools, source } => {
                                let tool_defs: Vec<sai_ipc::ToolDefinition> = tools.iter().map(|t| {
                                    sai_ipc::ToolDefinition {
                                        name: t.name.clone(),
                                        description: t.description.clone(),
                                        input_schema: t.input_schema.clone(),
                                    }
                                }).collect();
                                let registered = gm.tool_registry.register(&channel_id, tool_defs);
                                tracing::info!(
                                    "Registered {} tools from {} (source: {}): {:?}",
                                    registered.len(), channel_id, source, registered
                                );
                                // Send tools/list_changed notification
                                if let Some(mcpl) = &mut gm.mcpl {
                                    let _ = mcpl.send_notification(
                                        "notifications/tools/list_changed",
                                        None,
                                    ).await;
                                }
                                continue; // Don't forward registration events to the agent
                            }
                            sai_ipc::SaiEvent::ToolsUnregistered { tool_names } => {
                                let removed = gm.tool_registry.unregister(tool_names);
                                if !removed.is_empty() {
                                    tracing::info!("Unregistered tools: {:?}", removed);
                                    if let Some(mcpl) = &mut gm.mcpl {
                                        let _ = mcpl.send_notification(
                                            "notifications/tools/list_changed",
                                            None,
                                        ).await;
                                    }
                                }
                                continue;
                            }
                            sai_ipc::SaiEvent::ToolResult { call_id, content, is_error } => {
                                if let Some(mcpl_req_id) = gm.tool_registry.complete_call(call_id) {
                                    let result = serde_json::json!({
                                        "content": content,
                                        "isError": is_error,
                                    });
                                    if let Some(mcpl) = &mut gm.mcpl {
                                        if let Err(e) = mcpl.send_response(mcpl_req_id, result).await {
                                            tracing::error!("Failed to send deferred tool response: {}", e);
                                        }
                                    }
                                } else {
                                    tracing::warn!("Tool result for unknown call_id: {}", call_id);
                                }
                                continue;
                            }
                            _ => {}
                        }

                        gm.forward_sai_event(&channel_id, event).await;
                    }
                }
            }
        }
    }

    tracing::info!("GameManager shutting down");
    Ok(())
}
