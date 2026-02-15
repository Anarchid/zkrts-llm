//! SAI Bridge — Recoil/Spring SkirmishAI shared library.
//!
//! Exports init(), release(), handleEvent() as C functions.
//! Routes engine events to GameManager via Unix socket IPC,
//! receives commands back, and dispatches them to the engine.

pub mod callbacks;
pub mod commands;
pub mod events;
pub mod ipc;
pub mod plugin;

use callbacks::{EngineCallbacks, SSkirmishAICallback};
use events::{enrich_event, parse_event, GameEvent, EVENT_INIT, EVENT_UPDATE};
use ipc::IpcClient;
use std::ffi::{c_int, c_void};
use std::sync::Mutex;

/// Per-AI instance state.
struct AiInstance {
    callbacks: EngineCallbacks,
    ipc: Option<IpcClient>,
    plugin_host: Option<plugin::PluginHost>,
    frame_counter: u32,
}

/// Select commander type and pick a start position near the best metal spot.
/// Matches the algorithm used by both zkgbai (Java) and cpp-zkgbai.
fn select_commander_and_start_pos(cb: &EngineCallbacks) {
    // 1. Select commander via Lua rules gadget
    let commander = "dyntrainer_strike_base";
    cb.call_lua_rules(&format!("ai_commander:{}", commander));
    cb.log(&format!("[SAI Bridge] Selected commander: {}", commander));

    // 2. Query metal spots
    let all_spots = cb.get_metal_spots();
    if all_spots.is_empty() {
        cb.log("[SAI Bridge] No metal spots found, skipping start position");
        return;
    }

    // 3. Filter to spots within our startbox (ai_is_valid_startpos gadget check).
    //    If no startbox is defined (e.g. local games), all spots pass.
    let valid_spots: Vec<(f32, f32, f32, f32)> = all_spots
        .iter()
        .copied()
        .filter(|&(x, _, z, _)| {
            let query = format!("ai_is_valid_startpos:{}/{}", x, z);
            cb.call_lua_rules(&query).as_deref() == Some("1")
        })
        .collect();

    let spots = if valid_spots.is_empty() {
        cb.log("[SAI Bridge] No spots in startbox (or no startbox), using all spots");
        &all_spots
    } else {
        cb.log(&format!(
            "[SAI Bridge] {} of {} metal spots in startbox",
            valid_spots.len(),
            all_spots.len()
        ));
        &valid_spots
    };

    // 4. Map center: Map_getWidth/Height returns heightmap squares,
    //    world coords = squares * 8, center = squares * 4.
    let center_x = cb.map_width() as f32 * 4.0;
    let center_z = cb.map_height() as f32 * 4.0;

    // 5. Score each spot: dist_to_center + min_dist_to_nearest_neighbor.
    //    Lowest score = good central position near other mexes.
    let mut best_idx = 0;
    let mut best_score = f32::MAX;
    for (i, &(x, _, z, _)) in spots.iter().enumerate() {
        let dist_center = ((x - center_x).powi(2) + (z - center_z).powi(2)).sqrt();
        let mut min_neighbor = f32::MAX;
        for (j, &(ox, _, oz, _)) in spots.iter().enumerate() {
            if i == j {
                continue;
            }
            let d = ((x - ox).powi(2) + (z - oz).powi(2)).sqrt();
            if d < min_neighbor {
                min_neighbor = d;
            }
        }
        let score = dist_center + if min_neighbor < f32::MAX { min_neighbor } else { 0.0 };
        if score < best_score {
            best_score = score;
            best_idx = i;
        }
    }

    let (sx, _, sz, _) = spots[best_idx];

    // 5. Offset 75 units toward map center (so commander isn't on the mex)
    let dx = center_x - sx;
    let dz = center_z - sz;
    let dist = (dx * dx + dz * dz).sqrt();
    let (ox, oz) = if dist > 0.0 {
        (sx + (dx / dist) * 75.0, sz + (dz / dist) * 75.0)
    } else {
        (sx, sz)
    };

    // 6. Send start position
    let mut pos = [ox, 0.0, oz];
    cb.send_start_position(true, &mut pos);
    cb.log(&format!(
        "[SAI Bridge] Start position: ({:.0}, {:.0}) near metal spot ({:.0}, {:.0})",
        ox, oz, sx, sz
    ));
}

/// Global AI instance storage. Recoil supports up to 255 AIs,
/// but we typically only have one.
static INSTANCES: Mutex<Vec<Option<AiInstance>>> = Mutex::new(Vec::new());

/// How often to send UPDATE events over IPC (not every frame).
/// At 30 fps, every 30 frames = ~1 second.
const UPDATE_INTERVAL: u32 = 30;

fn get_socket_path(cb: &EngineCallbacks) -> String {
    // 1. connection.json in AI data dir (written by GM before each launch).
    //    Checked first because AIOptions.lua declares a default for socket_path,
    //    so get_option_value always returns *something* — even for dynamically
    //    created AIs via /aicontrol that have no startscript [Options] block.
    if let Some(data_dir) = cb.get_info_value("dataDir") {
        let config_path = format!("{}/connection.json", data_dir.trim_end_matches('/'));
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(path) = config.get("socket_path").and_then(|v| v.as_str()) {
                    cb.log(&format!("[SAI Bridge] Socket path from {}", config_path));
                    return path.to_string();
                }
            }
        }
    }

    // 2. AI option (startscript [Options] — AI-slot mode fallback)
    if let Some(path) = cb.get_option_value("socket_path") {
        cb.log("[SAI Bridge] Socket path from AI option");
        return path;
    }

    // 3. Environment variable
    if let Ok(path) = std::env::var("SAI_SOCKET_PATH") {
        cb.log("[SAI Bridge] Socket path from SAI_SOCKET_PATH env");
        return path;
    }

    // 4. Default
    cb.log("[SAI Bridge] Using default socket path");
    "/tmp/game-manager.sock".to_string()
}

/// Called by the engine when this AI is instantiated.
///
/// # Safety
/// Called by the Recoil engine with valid parameters.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn init(
    skirmish_ai_id: c_int,
    callback: *const SSkirmishAICallback,
) -> c_int {
    let cb = unsafe { EngineCallbacks::new(skirmish_ai_id, callback) };
    cb.log("[SAI Bridge] Initializing... (v3 — auto commander + start pos)");

    // Select commander and start position before game starts
    select_commander_and_start_pos(&cb);

    // Connect to GameManager
    let socket_path = get_socket_path(&cb);
    let ipc = match IpcClient::connect(&socket_path) {
        Ok(client) => {
            cb.log(&format!(
                "[SAI Bridge] Connected to GameManager at {}",
                socket_path
            ));
            // Don't send init here — wait for handleEvent(EVENT_INIT) which has game data
            Some(client)
        }
        Err(e) => {
            cb.log(&format!(
                "[SAI Bridge] Failed to connect to GameManager at {}: {}",
                socket_path, e
            ));
            None
        }
    };

    // Load plugins from <data_dir>/plugins/ if available
    let plugin_host = cb.get_info_value("dataDir").map(|data_dir| {
        let plugin_dir = std::path::Path::new(&data_dir).join("plugins");
        let host = plugin::PluginHost::load_from(&plugin_dir);
        if host.has_plugins() {
            cb.log(&format!(
                "[SAI Bridge] Loaded {} plugin tool(s)",
                host.all_tools().len()
            ));
        }
        host
    });

    let instance = AiInstance {
        callbacks: cb,
        ipc,
        plugin_host,
        frame_counter: 0,
    };

    // Store instance
    let mut instances = INSTANCES.lock().unwrap();
    let id = skirmish_ai_id as usize;
    while instances.len() <= id {
        instances.push(None);
    }
    instances[id] = Some(instance);

    0 // success
}

/// Called by the engine when this AI is removed.
///
/// # Safety
/// Called by the Recoil engine with valid parameters.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn release(skirmish_ai_id: c_int) -> c_int {
    let mut instances = INSTANCES.lock().unwrap();
    let id = skirmish_ai_id as usize;
    if let Some(Some(instance)) = instances.get_mut(id) {
        instance.callbacks.log("[SAI Bridge] Releasing...");

        // Send release event
        if let Some(ref mut ipc) = instance.ipc {
            let _ = ipc.send_event(&GameEvent::Release { reason: 0 });
        }

        instances[id] = None;
    }
    0
}

/// Main event handler — called by the engine for every game event.
///
/// # Safety
/// Called by the Recoil engine. `data` points to the event-specific struct.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn handleEvent(
    skirmish_ai_id: c_int,
    topic: c_int,
    data: *const c_void,
) -> c_int {
    let mut instances = INSTANCES.lock().unwrap();
    let id = skirmish_ai_id as usize;

    let instance = match instances.get_mut(id).and_then(|i| i.as_mut()) {
        Some(i) => i,
        None => return -1,
    };

    // Handle EVENT_INIT specially — it also carries the callback pointer
    if topic == EVENT_INIT {
        let init_data = unsafe { &*(data as *const events::SInitEvent) };
        instance.callbacks =
            unsafe { EngineCallbacks::new(skirmish_ai_id, init_data.callback) };

        // Query map data and metal spots from GameRulesParams
        let map_width = instance.callbacks.map_width();
        let map_height = instance.callbacks.map_height();
        instance.callbacks.log(&format!(
            "[SAI Bridge] EVENT_INIT: map {}x{}", map_width, map_height
        ));

        let mex_count = instance.callbacks.game_rules_param_float("mex_count", -1.0);
        instance.callbacks.log(&format!(
            "[SAI Bridge] mex_count from GameRulesParams = {}", mex_count
        ));

        let raw_spots = instance.callbacks.get_metal_spots();
        let metal_spots = if raw_spots.is_empty() {
            instance.callbacks.log("[SAI Bridge] No metal spots found");
            None
        } else {
            instance.callbacks.log(&format!(
                "[SAI Bridge] Found {} metal spots from GameRulesParams",
                raw_spots.len()
            ));
            Some(
                raw_spots
                    .into_iter()
                    .map(|(x, y, z, metal)| events::MetalSpot { x, y, z, metal })
                    .collect(),
            )
        };

        if let Some(ref mut ipc) = instance.ipc {
            let event = GameEvent::Init {
                frame: 0,
                saved_game: init_data.saved_game,
                metal_spots,
                map_width: Some(map_width),
                map_height: Some(map_height),
            };
            let _ = ipc.send_event(&event);

            // Register plugin tools with GameManager
            if let Some(ref host) = instance.plugin_host {
                let tools = host.all_tools();
                if !tools.is_empty() {
                    let reg_event = GameEvent::ToolsRegistered {
                        tools,
                        source: "plugin".to_string(),
                    };
                    let _ = ipc.send_event(&reg_event);
                }
            }
        }
        return 0;
    }

    // For UPDATE events, throttle and poll for incoming commands
    if topic == EVENT_UPDATE {
        instance.frame_counter += 1;

        // Poll for commands from GameManager every frame
        if let Some(ref mut ipc) = instance.ipc {
            let cmds = ipc.poll_commands();
            for cmd in &cmds {
                // Intercept tool calls — route to plugin or LuaUI widget
                if let commands::GameCommand::ToolCall { call_id, tool, args } = cmd {
                    instance.callbacks.log(&format!(
                        "[SAI Bridge] Tool call: {} (call_id={})", tool, call_id
                    ));

                    // Try plugin host first
                    if let Some(ref mut host) = instance.plugin_host {
                        let state = plugin::GameState::new(&instance.callbacks);
                        if let Some(result) = host.call(tool, args, &state) {
                            let result_event = GameEvent::ToolResult {
                                call_id: call_id.clone(),
                                content: result.content,
                                is_error: result.is_error,
                            };
                            let _ = ipc.send_event(&result_event);
                            continue;
                        }
                    }

                    // Fall through to LuaUI widget
                    let call_json = serde_json::json!({
                        "op": "call",
                        "call_id": call_id,
                        "tool": tool,
                        "args": args,
                    }).to_string();

                    let (content, is_error) = match instance.callbacks.call_lua_ui(&call_json) {
                        Some(response) => {
                            // Try to parse as widget protocol result
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&response) {
                                if parsed.get("op").and_then(|v| v.as_str()) == Some("result") {
                                    let content = parsed.get("content")
                                        .and_then(|v| v.as_array())
                                        .cloned()
                                        .unwrap_or_else(|| vec![serde_json::json!({"type": "text", "text": response})]);
                                    let is_err = parsed.get("is_error")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false);
                                    (content, is_err)
                                } else {
                                    (vec![serde_json::json!({"type": "text", "text": response})], false)
                                }
                            } else {
                                (vec![serde_json::json!({"type": "text", "text": response})], false)
                            }
                        }
                        None => {
                            (vec![serde_json::json!({"type": "text", "text": format!("No handler for tool '{}'", tool)})], true)
                        }
                    };

                    let result_event = GameEvent::ToolResult {
                        call_id: call_id.clone(),
                        content,
                        is_error,
                    };
                    let _ = ipc.send_event(&result_event);
                    continue;
                }

                instance.callbacks.log(&format!("[SAI Bridge] Dispatching: {:?}", cmd));
                if let Err(e) = commands::dispatch(&instance.callbacks, cmd) {
                    instance
                        .callbacks
                        .log(&format!("[SAI Bridge] Command error: {}", e));
                    let error_event = GameEvent::CommandError {
                        error: e,
                        command: format!("{:?}", cmd),
                    };
                    let _ = ipc.send_event(&error_event);
                }
            }
        }

        // Only send update events at throttled rate
        if instance.frame_counter % UPDATE_INTERVAL != 0 {
            return 0;
        }
    }

    // Parse, enrich with unit names, and forward the event
    if let Some(mut event) = unsafe { parse_event(topic, data) } {
        enrich_event(&mut event, &instance.callbacks);
        if let Some(ref mut ipc) = instance.ipc {
            if let Err(e) = ipc.send_event(&event) {
                instance
                    .callbacks
                    .log(&format!("[SAI Bridge] IPC send error: {}", e));
                // Connection lost — clear it
                instance.ipc = None;
            }
        }
    }

    0
}
