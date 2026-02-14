# Zero-K Agent

An LLM plays a real-time strategy game.

This project connects Claude (or any MCP-compatible LLM) to [Zero-K](https://zero-k.info), a free open-source RTS built on the [Recoil](https://github.com/beyond-all-reason/spring) engine. The agent receives game events (unit positions, combat, economy) and issues commands (move, attack, build) — all through [MCPL](https://github.com/anima-research/mcpl), our extension of the [Model Context Protocol](https://modelcontextprotocol.io) that adds bidirectional channels, push events, and reversible feature sets.

```
┌──────────┐    MCPL      ┌──────────────┐   Unix IPC   ┌────────────┐   C FFI   ┌────────┐
│  Claude   │◄───stdio────►│ GameManager  │◄────JSON────►│ SAI Bridge │◄─────────►│ Engine │
│  (LLM)   │  JSON-RPC    │   (Rust)     │   socket     │  (.so)     │  vtable   │(Recoil)│
└──────────┘              └──────────────┘              └────────────┘           └────────┘
```

The GameManager is an [MCPL](https://github.com/anima-research/mcpl) server — backward-compatible with plain MCP clients, but unlocking bidirectional game channels for those that support it. Claude connects over stdio, calls tools to start games and join lobbies, and receives a live stream of game events through MCPL channels. Commands flow back the same way: Claude publishes JSON to the game channel, GameManager forwards it over IPC, and the SAI bridge translates it into engine C API calls.

### Why MCPL?

Standard MCP is request-response: the client asks, the server answers. That's fine for tools, but an RTS game is a firehose of events — units spawning, enemies appearing, frames ticking. MCPL adds **channels** (persistent bidirectional streams), **push events** (server-initiated messages), and **feature sets** with rollback semantics. The game channel carries events downstream and commands upstream, all within the same protocol session. See the [MCPL spec](https://github.com/anima-research/mcpl) for details.

## Components

### GameManager (`game-manager/`)

Rust binary that acts as the central orchestrator:

- **MCPL server** — stdio or TCP transport, JSON-RPC 2.0 (MCP backward-compatible)
- **Lobby client** — connects to zero-k.info:8200, handles login, chat, matchmaking
- **Engine manager** — spawns headless Spring instances with write-dir isolation
- **SAI IPC server** — Unix socket listener for SAI bridge connections
- **Channel routing** — each game instance becomes a bidirectional MCPL channel

### SAI Bridge (`sai-bridge/`)

Rust cdylib (shared library) loaded by the Recoil engine as a Skirmish AI:

- Implements the `init` / `release` / `handleEvent` C interface
- Connects to GameManager over Unix socket IPC
- Forwards game events (unit_created, enemy_enter_los, update, ...) as JSON
- Polls for commands (move, attack, build, ...) and dispatches them via bindgen-generated callback bindings
- Update events throttled to ~1/sec (every 30th frame)
- **Auto commander selection**: Selects `dyntrainer_strike_base` via Lua rules gadget during `init()`
- **Auto start position**: Picks the best metal spot (scored by centrality + neighbor density), offsets 75 units toward map center, signals readiness

### Agent App (`app/`)

TypeScript application using the Connectome's [Agent Framework](https://github.com/antra-tess/agent-framework) for multi-agent orchestration. Spawns the GameManager as a subprocess, starts a game (local or via the Zero-K lobby), and plays using a real-time think/act/sleep loop with agent-controlled event filtering (WakeModule).

## Tools

The GameManager exposes these MCP tools:

| Tool | Description |
|------|-------------|
| `lobby_connect` | Connect to Zero-K lobby server |
| `lobby_login` | Authenticate with credentials |
| `lobby_register` | Register a new account |
| `lobby_start_game` | Start a local game (map, opponent, headless/headful, player_mode) |
| `lobby_open_battle` | Host a multiplayer battle room (title, map) |
| `lobby_add_bot` | Add an AI opponent to the current battle |
| `lobby_start_battle` | Start the hosted battle |
| `lobby_join_battle` | Join an existing multiplayer battle |
| `lobby_matchmaker_join` | Queue for matchmaking |
| `lobby_say` | Send chat messages |
| `lobby_list_battles` | List open battles |
| `lobby_list_users` | List online users |

All lobby tools are **idempotent** — calling them when already in the desired state returns a success message rather than creating duplicates.

## Game Events

Events flow from the engine through the SAI bridge to the LLM as `channels/incoming` messages:

| Event | Fields | Description |
|-------|--------|-------------|
| `init` | frame, metal_spots, map_width, map_height | Game initialized |
| `update` | frame | Game tick (~1/sec) |
| `unit_created` | unit, unit_name, builder, builder_name, pos | New unit constructed |
| `unit_finished` | unit, unit_name, pos | Unit construction complete |
| `unit_idle` | unit, unit_name | Unit has no orders (one-shot!) |
| `unit_damaged` | unit, unit_name, attacker, attacker_name, damage | Unit took damage |
| `unit_destroyed` | unit, unit_name, attacker, attacker_name | Unit killed |
| `unit_move_failed` | unit, unit_name | Unit pathfinding failed |
| `enemy_enter_los` | enemy, enemy_name, pos | Enemy spotted |
| `enemy_leave_los` | enemy, enemy_name | Enemy left vision |
| `enemy_enter_radar` | enemy, enemy_name | Radar contact |
| `enemy_destroyed` | enemy, enemy_name, attacker, attacker_name | Enemy killed |
| `command_finished` | unit, unit_name, command_id, command_topic | Unit finished an order |
| `command_error` | error, command | A dispatched command failed |
| `message` | player, text | In-game chat |
| `release` | reason | Game over |

## Game Commands

Commands are sent via `channels/publish` as JSON:

```json
{"type": "move", "unit_id": 42, "x": 1024, "y": 0, "z": 2048}
{"type": "attack", "unit_id": 42, "target_id": 99}
{"type": "build", "unit_id": 42, "build_def_name": "staticmex", "x": 512, "y": 0, "z": 512}
{"type": "build", "unit_id": 42, "build_def_name": "cloakraid", "queue": true}
{"type": "patrol", "unit_id": 42, "x": 1500, "y": 0, "z": 1500}
{"type": "fight", "unit_id": 42, "x": 2000, "y": 0, "z": 2000}
{"type": "guard", "unit_id": 42, "guard_id": 43}
{"type": "repair", "unit_id": 42, "repair_id": 43}
{"type": "set_fire_state", "unit_id": 42, "state": 2}
{"type": "set_move_state", "unit_id": 42, "state": 2}
{"type": "send_chat", "text": "glhf"}
{"type": "pause"}
{"type": "unpause"}
```

All movement commands support `"queue": true` for shift-queuing. Build commands accept `build_def_name` (e.g. `"staticmex"`) — the GM resolves it to the numeric def ID. For factory production, omit x/y/z. Build positions are auto-snapped to the nearest valid site.

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Recoil/Spring engine](https://github.com/beyond-all-reason/spring) installed at `~/.spring/engine/`
- Zero-K game files in `~/.spring/` (install via [Zero-K launcher](https://zero-k.info) or Chobby)
- A map (e.g., TitanDuel 2.2) in `~/.spring/maps/`

### Build

```bash
# GameManager
cd game-manager && cargo build --release

# SAI bridge (produces libSkirmishAI.so)
cd sai-bridge && cargo build --release
```

### Run with Claude Code

Add to your `.mcp.json`:

```json
{
  "mcpServers": {
    "zk-game-manager": {
      "command": "cargo",
      "args": ["run", "--manifest-path", "path/to/game-manager/Cargo.toml", "--", "--stdio", "--write-dir", "/path/to/write-dir"]
    }
  }
}
```

Then ask Claude to start a game:

> Start a local game on TitanDuel 2.2 against NullAI

Claude will call `lobby_start_game`, receive game events through the channel, and can issue commands back. Against NullAI (which does nothing), winning is a matter of building an army and marching across the map.

### Run the Agent App

The standalone app spawns the GameManager, starts a game, and plays autonomously:

```bash
cd app
cp .env.example .env
# Edit .env — set ANTHROPIC_API_KEY at minimum

npm install
npm start
```

The agent uses a **think → act → sleep** loop in real-time (no pausing). After acting, it calls `wake:set_conditions` to specify which events should wake it for the next think cycle (e.g. unit finished, enemy spotted, timeout). Agent inference streams to the console in real-time.

For online play against other players or spectated games:

```bash
PLAY_MODE=lobby ZK_USERNAME=loom ZK_PASSWORD=... PROVIDER=haiku npm start
```

Environment variables (all optional except API key):

| Variable | Default | Description |
|----------|---------|-------------|
| `ANTHROPIC_API_KEY` | (required for anthropic/haiku) | Anthropic API key |
| `GROQ_API_KEY` | (required for groq) | Groq API key |
| `PROVIDER` | `anthropic` | Provider: `anthropic`, `haiku`, or `groq` |
| `GAME_MANAGER_BIN` | `../game-manager/target/release/game-manager` | Path to GM binary |
| `WRITE_DIR` | `~/.spring-loom` | Engine write directory |
| `MAP` | `TitanDuel 2.2` | Map to play on |
| `OPPONENT` | `NullAI` | Opponent AI |
| `STORE_PATH` | `./data/store` | Chronicle event store |
| `PLAY_MODE` | `local` | Play mode: `local` or `lobby` |
| `ZK_USERNAME` | | Zero-K username (lobby mode) |
| `ZK_PASSWORD` | | Zero-K password (lobby mode) |

### Integration Tests

```bash
cd game-manager

# Full test suite (tiers 1-3)
python3 tests/integration_test.py --tier 3

# Quick engine launch test only
python3 tests/integration_test.py --tier 1

# Verbose (shows JSON-RPC traffic)
python3 tests/integration_test.py --tier 3 -v

# Fresh write-dir (no cached archives)
python3 tests/integration_test.py --tier 3 --fresh
```

Test tiers:
1. **Engine Launch** — game starts, infolog.txt created
2. **SAI Boot** — Init event received, unit events flowing, SAI connected
3. **Command Round-Trip** — chat command delivered, unit events observed

## Architecture

The project is part of a larger agent framework ([Connectome](../CLAUDE.md)) that includes a branchable event store (Chronicle), LLM abstraction layer (Membrane), and multi-agent orchestration.

**Current status**: Haiku can beat NullAI in both local and online (lobby-hosted) games. The agent builds an economy, produces an army, scouts, and attacks — though it occasionally enters a narration-only mode that needs a safety net fix.

**Planned**:
- **Multi-agent roles** — strategist, tactician, economist operating at different frequencies
- **Hypothesis testing** — rewind game state via savestates, explore alternative strategies
- **Compilation pipeline** — promote successful LLM strategies to Lua scripts, then to compiled Rust
- **Competitive play** — matchmaking on the Zero-K ladder via the lobby protocol

## License

MIT
