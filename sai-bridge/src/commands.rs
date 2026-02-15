//! Command dispatch: receives JSON commands from GameManager,
//! converts them to C structs, and calls Engine_handleCommand.

use crate::callbacks::*;
use serde::Deserialize;
use std::ffi::{c_float, c_int, c_void, CString};

/// Commands received from GameManager over IPC.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum GameCommand {
    #[serde(rename = "move")]
    Move {
        unit_id: i32,
        x: f32,
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
        y: f32,
        z: f32,
        #[serde(default)]
        queue: bool,
    },

    #[serde(rename = "fight")]
    Fight {
        unit_id: i32,
        x: f32,
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

/// Translate engine return codes to human-readable errors.
fn describe_error(code: c_int) -> &'static str {
    match code {
        -1 => "invalid unit ID or null command",
        -2 => "orders not allowed (AI may lack authority)",
        -3 => "unit does not exist",
        -4 => "unknown error (-4)",
        -5 => "unit does not belong to this AI's team",
        _ => "unknown error",
    }
}

/// Validate that a unit_id refers to an existing unit owned by the AI.
/// Returns a descriptive error if not.
fn validate_unit(cb: &EngineCallbacks, unit_id: i32) -> Result<(), String> {
    let def_id = cb.unit_get_def(unit_id);
    if def_id < 0 {
        return Err(format!(
            "unit {} does not exist (unit_get_def returned {})",
            unit_id, def_id
        ));
    }
    Ok(())
}

/// Dispatch a GameCommand to the engine via callbacks.
/// Returns Ok(()) on success, Err with description on failure.
pub fn dispatch(cb: &EngineCallbacks, cmd: &GameCommand) -> Result<(), String> {
    let result = match cmd {
        GameCommand::Move {
            unit_id,
            x,
            y,
            z,
            queue,
        } => {
            validate_unit(cb, *unit_id)?;
            let mut pos: [c_float; 3] = [*x, *y, *z];
            let mut data = SMoveUnitCommand {
                unit_id: *unit_id as c_int,
                group_id: -1,
                options: if *queue { UNIT_COMMAND_OPTION_SHIFT_KEY } else { 0 },
                time_out: i32::MAX,
                to_pos: &mut pos as *mut [c_float; 3],
            };
            cb.handle_command(COMMAND_UNIT_MOVE, &mut data as *mut _ as *mut c_void)
        }

        GameCommand::Stop { unit_id } => {
            validate_unit(cb, *unit_id)?;
            let mut data = SStopUnitCommand {
                unit_id: *unit_id as c_int,
                group_id: -1,
                options: 0,
                time_out: i32::MAX,
            };
            cb.handle_command(COMMAND_UNIT_STOP, &mut data as *mut _ as *mut c_void)
        }

        GameCommand::Attack {
            unit_id,
            target_id,
            queue,
        } => {
            validate_unit(cb, *unit_id)?;
            let mut data = SAttackUnitCommand {
                unit_id: *unit_id as c_int,
                group_id: -1,
                options: if *queue { UNIT_COMMAND_OPTION_SHIFT_KEY } else { 0 },
                time_out: i32::MAX,
                to_attack_unit_id: *target_id as c_int,
            };
            cb.handle_command(COMMAND_UNIT_ATTACK, &mut data as *mut _ as *mut c_void)
        }

        GameCommand::Build {
            unit_id,
            build_def_id,
            build_def_name,
            x,
            y,
            z,
            facing,
            queue,
        } => {
            validate_unit(cb, *unit_id)?;
            // Resolve def name to ID if provided, otherwise use numeric ID
            let def_id = if let Some(name) = build_def_name {
                cb.get_unit_def_by_name(name)
                    .ok_or_else(|| format!("Unknown unit def name: {}", name))?
            } else {
                *build_def_id
            };
            // For factory production (no coords), pass [0,0,0] directly.
            // For construction (has coords), snap to closest valid build position.
            let mut pos: [c_float; 3] = if *x == 0.0 && *y == 0.0 && *z == 0.0 {
                [0.0, 0.0, 0.0]
            } else {
                let requested_pos: [c_float; 3] = [*x, *y, *z];
                cb.map_find_closest_build_site(def_id, &requested_pos, 200.0, 0, *facing)
                    .ok_or_else(|| {
                        format!(
                            "No valid build position found near ({}, {}, {}) for def {}",
                            x, y, z,
                            build_def_name.as_deref().unwrap_or("?")
                        )
                    })?
            };
            let mut data = SBuildUnitCommand {
                unit_id: *unit_id as c_int,
                group_id: -1,
                options: if *queue { UNIT_COMMAND_OPTION_SHIFT_KEY } else { 0 },
                time_out: i32::MAX,
                to_build_unit_def_id: def_id as c_int,
                build_pos: &mut pos as *mut [c_float; 3],
                facing: *facing as c_int,
            };
            cb.handle_command(COMMAND_UNIT_BUILD, &mut data as *mut _ as *mut c_void)
        }

        GameCommand::Patrol {
            unit_id,
            x,
            y,
            z,
            queue,
        } => {
            validate_unit(cb, *unit_id)?;
            let mut pos: [c_float; 3] = [*x, *y, *z];
            let mut data = SPatrolUnitCommand {
                unit_id: *unit_id as c_int,
                group_id: -1,
                options: if *queue { UNIT_COMMAND_OPTION_SHIFT_KEY } else { 0 },
                time_out: i32::MAX,
                to_pos: &mut pos as *mut [c_float; 3],
            };
            cb.handle_command(COMMAND_UNIT_PATROL, &mut data as *mut _ as *mut c_void)
        }

        GameCommand::Fight {
            unit_id,
            x,
            y,
            z,
            queue,
        } => {
            validate_unit(cb, *unit_id)?;
            let mut pos: [c_float; 3] = [*x, *y, *z];
            let mut data = SFightUnitCommand {
                unit_id: *unit_id as c_int,
                group_id: -1,
                options: if *queue { UNIT_COMMAND_OPTION_SHIFT_KEY } else { 0 },
                time_out: i32::MAX,
                to_pos: &mut pos as *mut [c_float; 3],
            };
            cb.handle_command(COMMAND_UNIT_FIGHT, &mut data as *mut _ as *mut c_void)
        }

        GameCommand::Guard {
            unit_id,
            guard_id,
            queue,
        } => {
            validate_unit(cb, *unit_id)?;
            let mut data = SGuardUnitCommand {
                unit_id: *unit_id as c_int,
                group_id: -1,
                options: if *queue { UNIT_COMMAND_OPTION_SHIFT_KEY } else { 0 },
                time_out: i32::MAX,
                to_guard_unit_id: *guard_id as c_int,
            };
            cb.handle_command(COMMAND_UNIT_GUARD, &mut data as *mut _ as *mut c_void)
        }

        GameCommand::Repair {
            unit_id,
            repair_id,
            queue,
        } => {
            validate_unit(cb, *unit_id)?;
            let mut data = SRepairUnitCommand {
                unit_id: *unit_id as c_int,
                group_id: -1,
                options: if *queue { UNIT_COMMAND_OPTION_SHIFT_KEY } else { 0 },
                time_out: i32::MAX,
                to_repair_unit_id: *repair_id as c_int,
            };
            cb.handle_command(COMMAND_UNIT_REPAIR, &mut data as *mut _ as *mut c_void)
        }

        GameCommand::SetFireState { unit_id, state } => {
            validate_unit(cb, *unit_id)?;
            let mut data = SSetFireStateUnitCommand {
                unit_id: *unit_id as c_int,
                group_id: -1,
                options: 0,
                time_out: i32::MAX,
                fire_state: *state as c_int,
            };
            cb.handle_command(
                COMMAND_UNIT_SET_FIRE_STATE,
                &mut data as *mut _ as *mut c_void,
            )
        }

        GameCommand::SetMoveState { unit_id, state } => {
            validate_unit(cb, *unit_id)?;
            let mut data = SSetMoveStateUnitCommand {
                unit_id: *unit_id as c_int,
                group_id: -1,
                options: 0,
                time_out: i32::MAX,
                move_state: *state as c_int,
            };
            cb.handle_command(
                COMMAND_UNIT_SET_MOVE_STATE,
                &mut data as *mut _ as *mut c_void,
            )
        }

        GameCommand::SendChat { text } => {
            // SendTextMsg only handles /commands — plain text is ignored.
            // Prepend /say to send actual network chat visible to all players.
            let say_text = format!("/say {}", text);
            let c_text = CString::new(say_text.as_str()).map_err(|e| e.to_string())?;
            let mut data = SSendTextMessageCommand {
                text: c_text.as_ptr(),
                zone: 0,
            };
            cb.handle_command(
                COMMAND_SEND_TEXT_MESSAGE,
                &mut data as *mut _ as *mut c_void,
            )
        }

        GameCommand::Pause | GameCommand::Unpause => {
            // No-op: pausing the engine deadlocks the AI (UPDATE events stop,
            // so the bridge can never poll the unpause command).
            // The agent plays in real-time; wake/sleep handles pacing.
            return Ok(());
        }

        GameCommand::SetSpeed { .. } => {
            return Err("set_speed is not supported by the engine AI interface".into());
        }

        GameCommand::ToolCall { .. } => {
            // Handled in lib.rs handleEvent(EVENT_UPDATE), not via dispatch()
            return Ok(());
        }
    };

    // Engine returns 0 for unit commands, 1 for engine-level commands (pause, etc.)
    if result >= 0 {
        Ok(())
    } else {
        Err(format!(
            "Engine rejected command (code {}): {}",
            result,
            describe_error(result)
        ))
    }
}
