use mcpl_core::capabilities::*;
use mcpl_core::connection::{ConnectionError, IncomingMessage as McplIncoming, McplConnection};
use mcpl_core::methods::*;

use tokio::net::TcpListener;

/// Tool definitions exposed to the MCPL client.
pub fn lobby_tools() -> serde_json::Value {
    serde_json::json!({
        "tools": [
            {
                "name": "lobby_connect",
                "description": "Connect to the Zero-K lobby server",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "host": { "type": "string", "default": "zero-k.info" },
                        "port": { "type": "integer", "default": 8200 }
                    }
                }
            },
            {
                "name": "lobby_login",
                "description": "Authenticate with the Zero-K lobby",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "username": { "type": "string" },
                        "password": { "type": "string" }
                    },
                    "required": ["username", "password"]
                }
            },
            {
                "name": "lobby_register",
                "description": "Register a new account on the Zero-K lobby server",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "username": { "type": "string" },
                        "password": { "type": "string" },
                        "email": { "type": "string" }
                    },
                    "required": ["username", "password", "email"]
                }
            },
            {
                "name": "lobby_disconnect",
                "description": "Disconnect from the Zero-K lobby server",
                "inputSchema": { "type": "object" }
            },
            {
                "name": "lobby_say",
                "description": "Send a chat message to a channel or user",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "Channel name or username" },
                        "text": { "type": "string" },
                        "place": { "type": "integer", "description": "0=Channel, 4=User", "default": 0 }
                    },
                    "required": ["target", "text"]
                }
            },
            {
                "name": "lobby_join_channel",
                "description": "Join a chat channel",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "channel": { "type": "string" }
                    },
                    "required": ["channel"]
                }
            },
            {
                "name": "lobby_leave_channel",
                "description": "Leave a chat channel",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "channel": { "type": "string" }
                    },
                    "required": ["channel"]
                }
            },
            {
                "name": "lobby_list_battles",
                "description": "List open battles in the lobby",
                "inputSchema": { "type": "object" }
            },
            {
                "name": "lobby_list_users",
                "description": "List online users",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "default": 50 }
                    }
                }
            },
            {
                "name": "lobby_join_battle",
                "description": "Join a battle room",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "battle_id": { "type": "integer" },
                        "password": { "type": "string", "default": "" }
                    },
                    "required": ["battle_id"]
                }
            },
            {
                "name": "lobby_leave_battle",
                "description": "Leave the current battle",
                "inputSchema": { "type": "object" }
            },
            {
                "name": "lobby_matchmaker_join",
                "description": "Join matchmaker queues. Available queues are sent on login (e.g. '1v1', 'Sortie', 'Battle', 'Coop'). Can join multiple simultaneously.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "queues": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Queue names to join (e.g. ['1v1', 'Sortie'])"
                        }
                    },
                    "required": ["queues"]
                }
            },
            {
                "name": "lobby_matchmaker_leave",
                "description": "Leave all matchmaker queues",
                "inputSchema": { "type": "object" }
            },
            {
                "name": "lobby_matchmaker_accept",
                "description": "Accept or decline a matchmaker ready-check when a match is found",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ready": { "type": "boolean", "description": "true to accept, false to decline" }
                    },
                    "required": ["ready"]
                }
            },
            {
                "name": "lobby_matchmaker_status",
                "description": "Get current matchmaker status: available queues, joined queues, queue counts",
                "inputSchema": { "type": "object" }
            },
            {
                "name": "lobby_start_game",
                "description": "Start a local scrimmage game (AgentBridge vs opponent AI)",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "map": { "type": "string", "description": "Map name (e.g., 'Comet Catcher Redux')" },
                        "game": { "type": "string", "default": "Zero-K $VERSION", "description": "Game type / archive name" },
                        "opponent": { "type": "string", "default": "1052188CircuitAINovice64", "description": "Opponent AI shortname" },
                        "headless": { "type": "boolean", "default": true, "description": "Run without UI (true) or with UI (false)" },
                        "player_mode": { "type": "boolean", "default": false, "description": "Agent as PLAYER slot (widget hands control via /aicontrol)" }
                    },
                    "required": ["map"]
                }
            },
            {
                "name": "lobby_open_battle",
                "description": "Host a new custom battle room on the lobby server. You must be connected and logged in. After opening, add bots with lobby_add_bot, then start with lobby_start_battle.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Battle room title" },
                        "map": { "type": "string", "description": "Map name" },
                        "max_players": { "type": "integer", "default": 2, "description": "Maximum players" },
                        "password": { "type": "string", "default": "", "description": "Battle password (empty for public)" }
                    },
                    "required": ["title", "map"]
                }
            },
            {
                "name": "lobby_add_bot",
                "description": "Add an AI bot to the current battle room",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ai_lib": { "type": "string", "description": "AI shortname (e.g. 'NullAI', '1052188CircuitAINovice64')" },
                        "name": { "type": "string", "default": "Bot1", "description": "Bot display name" },
                        "ally_number": { "type": "integer", "default": 1, "description": "Team/ally number (0-based)" }
                    },
                    "required": ["ai_lib"]
                }
            },
            {
                "name": "lobby_remove_bot",
                "description": "Remove a bot from the current battle room",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Bot name to remove" }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "lobby_start_battle",
                "description": "Start the game in the current battle room. All participants will receive connection details.",
                "inputSchema": { "type": "object" }
            }
        ]
    })
}

/// MCPL server capabilities for the GameManager.
pub fn server_capabilities() -> McplCapabilities {
    McplCapabilities {
        version: "0.4".into(),
        push_events: Some(true),
        channels: Some(true),
        rollback: Some(true),
        context_hooks: Some(ContextHooksCap {
            before_inference: false,
            after_inference: None,
        }),
        feature_sets: Some(vec![
            FeatureSetDeclaration {
                name: "lobby".into(),
                description: Some("Lobby operations — non-reversible".into()),
                uses: vec!["connect".into(), "chat".into(), "matchmaking".into()],
                rollback: false,
                host_state: false,
            },
            FeatureSetDeclaration {
                name: "game".into(),
                description: Some("Game operations — reversible via savestates".into()),
                uses: vec!["commands".into(), "observation".into(), "state".into()],
                rollback: true,
                host_state: false,
            },
        ]),
        inference_request: None,
        stream_observer: None,
        scoped_access: None,
        model_info: None,
    }
}

/// Perform the MCPL initialize handshake on an established connection.
async fn mcpl_handshake(conn: &mut McplConnection) -> Result<(), ConnectionError> {
    // Wait for initialize request
    let msg = conn.next_message().await?;
    match msg {
        McplIncoming::Request(req) if req.method == "initialize" => {
            tracing::info!("Received initialize request");

            let result = McplInitializeResult {
                protocol_version: "2024-11-05".into(),
                capabilities: InitializeCapabilities {
                    experimental: Some(ExperimentalCapabilities {
                        mcpl: Some(server_capabilities()),
                    }),
                    other: {
                        let mut m = serde_json::Map::new();
                        m.insert("tools".into(), serde_json::json!({}));
                        m
                    },
                },
                server_info: ImplementationInfo {
                    name: "zk-game-manager".into(),
                    version: "0.1.0".into(),
                },
            };

            conn.send_response(req.id, serde_json::to_value(&result).unwrap())
                .await?;
        }
        _ => {
            tracing::error!("Expected initialize request, got {:?}", msg);
            return Err(ConnectionError::Closed);
        }
    }

    // Wait for initialized notification
    let msg = conn.next_message().await?;
    match msg {
        McplIncoming::Notification(notif) if notif.method == "notifications/initialized" => {
            tracing::info!("Client initialized");
        }
        _ => {
            tracing::warn!("Expected initialized notification, got {:?}", msg);
            // Non-fatal — continue anyway
        }
    }

    Ok(())
}

/// Accept and initialize a single MCPL client connection over TCP.
pub async fn accept_mcpl_client(
    listener: &TcpListener,
) -> Result<McplConnection, ConnectionError> {
    let (stream, addr) = listener.accept().await.map_err(ConnectionError::Io)?;
    tracing::info!("MCPL client connected from {}", addr);

    let mut conn = McplConnection::new(stream);
    mcpl_handshake(&mut conn).await?;
    Ok(conn)
}

/// Create and initialize an MCPL connection over stdin/stdout.
pub async fn accept_mcpl_stdio() -> Result<McplConnection, ConnectionError> {
    tracing::info!("Starting MCPL server on stdio");

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let mut conn = McplConnection::from_parts(Box::new(stdin), Box::new(stdout));
    mcpl_handshake(&mut conn).await?;
    Ok(conn)
}
