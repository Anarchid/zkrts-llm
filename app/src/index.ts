/**
 * Zero-K Agent — Gameplay App
 *
 * Spawns the GameManager as a subprocess, starts a game,
 * and plays using a think/act/sleep loop.
 *
 * Usage:
 *   npm install
 *   npm start
 *
 * Required environment variables (see .env.example):
 *   ANTHROPIC_API_KEY  - Anthropic API key (for provider=anthropic)
 *   GROQ_API_KEY       - Groq API key (for provider=groq)
 *
 * Optional:
 *   PROVIDER           - "anthropic" (default) or "groq"
 *   GAME_MANAGER_BIN   - Path to game-manager binary
 *   WRITE_DIR         - Agent write directory (default: ~/.spring-loom)
 *   MAP               - Map name (default: Comet Catcher Redux v3.1)
 *   OPPONENT          - Opponent AI (default: NullAI)
 *   STORE_PATH        - Chronicle store path (default: ./data/store)
 *   PLAY_MODE         - "local" or "lobby" (default: local)
 *   ZK_USERNAME       - Zero-K lobby username (required for lobby mode)
 *   ZK_PASSWORD       - Zero-K lobby password (required for lobby mode)
 */

import 'dotenv/config';
import { Membrane, AnthropicAdapter, OpenAICompatibleAdapter, NativeFormatter } from 'membrane';
import type { ProviderAdapter, PrefillFormatter } from 'membrane';
import { AgentFramework, MCPLModule, ApiServer } from '@connectome/agent-framework';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { homedir } from 'node:os';

const __dirname = dirname(fileURLToPath(import.meta.url));
import { WakeModule } from './wake-module.js';

const config = {
  provider: (process.env.PROVIDER || 'anthropic') as 'anthropic' | 'haiku' | 'groq',
  gmBin: process.env.GAME_MANAGER_BIN || resolve(__dirname, '../../game-manager/target/release/game-manager'),
  writeDir: process.env.WRITE_DIR || resolve(homedir(), '.spring-loom'),
  map: process.env.MAP || 'TitanDuel 2.2',
  opponent: process.env.OPPONENT || 'NullAI',
  storePath: process.env.STORE_PATH || './data/store',
  playMode: (process.env.PLAY_MODE || 'local') as 'local' | 'lobby',
  zkUsername: process.env.ZK_USERNAME || '',
  zkPassword: process.env.ZK_PASSWORD || '',
};

const providerConfigs = {
  anthropic: {
    envKey: 'ANTHROPIC_API_KEY',
    model: 'claude-sonnet-4-5-20250929',
    createAdapter: (): ProviderAdapter =>
      new AnthropicAdapter({ apiKey: process.env.ANTHROPIC_API_KEY! }),
    formatter: undefined as PrefillFormatter | undefined,
  },
  haiku: {
    envKey: 'ANTHROPIC_API_KEY',
    model: 'claude-haiku-4-5-20251001',
    createAdapter: (): ProviderAdapter =>
      new AnthropicAdapter({ apiKey: process.env.ANTHROPIC_API_KEY! }),
    formatter: undefined as PrefillFormatter | undefined,
  },
  groq: {
    envKey: 'GROQ_API_KEY',
    model: 'moonshotai/kimi-k2-instruct-0905',
    createAdapter: (): ProviderAdapter =>
      new OpenAICompatibleAdapter({
        baseURL: 'https://api.groq.com/openai/v1',
        apiKey: process.env.GROQ_API_KEY!,
        providerName: 'groq',
      }),
    formatter: new NativeFormatter() as PrefillFormatter,
  },
};

const providerCfg = providerConfigs[config.provider];
const required = [providerCfg.envKey];
if (config.playMode === 'lobby') {
  required.push('ZK_USERNAME', 'ZK_PASSWORD');
}
const missing = required.filter((key) => !process.env[key]);
if (missing.length > 0) {
  console.error('Missing required environment variables:', missing.join(', '));
  console.error('Copy .env.example to .env and fill in the values.');
  process.exit(1);
}

const SYSTEM_PROMPT = `You are a Zero-K RTS game agent. Your goal is to win.

## How You Play

The game runs in **real-time**. You play in a **think → act → sleep** loop:

1. When you're woken by game events, analyze the situation.
2. Issue all your commands (move, build, attack, etc.) — use queue:true to append.
3. Set wake conditions, then sleep until the next interesting event.

The world keeps moving while you think and while you sleep. Be decisive — issue commands quickly, then sleep. You'll accumulate events while asleep and process them all on your next wake.

## Safety Rules

- Never use \`lobby_register\` or matchmaker tools unless explicitly instructed.
- Follow the kickoff instructions for which play mode to use (local or lobby).

## Starting the Game

**Local mode**: Call \`zk:lobby_start_game\` with \`headless: false\` to start a local game.

**Lobby mode** (when instructed):
1. \`zk:lobby_connect\` — connect to the lobby server
2. \`zk:lobby_login\` — authenticate with provided credentials
3. \`zk:lobby_open_battle\` — host a custom battle room with a title and map
4. \`zk:lobby_add_bot\` — add an AI opponent (e.g. ai_lib: "NullAI")
5. \`zk:lobby_start_battle\` — start the game

In both modes, a game channel is created. Once the game starts, you'll receive an \`init\` event — that's your cue to begin playing.

## Game Commands (via zk:channel_publish)

Send commands as JSON text to the game channel. Always include the channelId.

### Unit Orders
- \`{"type":"move","unit_id":N,"x":F,"y":F,"z":F,"queue":true}\` — Move unit to position
- \`{"type":"stop","unit_id":N}\` — Cancel all orders
- \`{"type":"attack","unit_id":N,"target_id":N,"queue":true}\` — Attack a unit
- \`{"type":"build","unit_id":N,"build_def_name":"defname","x":F,"y":F,"z":F,"facing":0,"queue":true}\` — Build a unit/structure by def name. Coordinates are auto-snapped to the nearest valid build position, so approximate coordinates (e.g. from metal spot data) are fine. **For factory production** (factory builds a unit), omit x/y/z: \`{"type":"build","unit_id":FACTORY_ID,"build_def_name":"cloakraid","queue":true}\`
- \`{"type":"patrol","unit_id":N,"x":F,"y":F,"z":F,"queue":true}\` — Patrol to position
- \`{"type":"fight","unit_id":N,"x":F,"y":F,"z":F,"queue":true}\` — Attack-move toward position
- \`{"type":"guard","unit_id":N,"guard_id":N,"queue":true}\` — Guard another unit
- \`{"type":"repair","unit_id":N,"repair_id":N,"queue":true}\` — Repair a unit
- \`{"type":"set_fire_state","unit_id":N,"state":N}\` — 0=hold fire, 1=return fire, 2=fire at will
- \`{"type":"set_move_state","unit_id":N,"state":N}\` — 0=hold position, 1=maneuver, 2=roam
- \`{"type":"send_chat","text":"message"}\` — Send in-game chat

## Game Events (you receive these)

Events arrive as channel messages. Key events and what to do:

- **init** — Game started. Your commander spawns moments later as a \`unit_finished\` event — wait for that to learn your commander's unit ID before issuing orders.
- **message** {player, text} — A chat message from a player or spectator. Read and respond if addressed to you!
- **command_error** {error, command} — A command you sent failed. Read the error message carefully — common causes: invalid unit_id (unit doesn't exist or you used a wrong ID), unit belongs to another team, or unknown build def name. **Always use unit IDs from events you received, never guess or fabricate them.**
- **unit_created** {unit, unit_name, builder, builder_name} — A new unit appeared.
- **unit_finished** {unit, unit_name} — Construction complete. The unit is now active.
- **unit_idle** {unit, unit_name} — A unit has no orders. **Always assign idle units work!**
- **unit_damaged** {unit, unit_name, attacker, attacker_name, damage} — You're under attack. Respond.
- **unit_destroyed** {unit, unit_name, attacker, attacker_name} — You lost a unit.
- **enemy_enter_los** {enemy, enemy_name} — Enemy spotted! Assess the threat.
- **enemy_leave_los** {enemy, enemy_name} — Enemy left your vision.
- **enemy_enter_radar** {enemy, enemy_name} — Radar contact.
- **enemy_destroyed** {enemy, enemy_name, attacker, attacker_name} — Kill confirmed.
- **command_finished** {unit, unit_name, command_id} — Unit finished an order.
- **release** — Game over.

Unit IDs are numeric (e.g. 26780). **You MUST track unit IDs from events** — never guess or hardcode them. The \`unit_name\`/\`enemy_name\` fields give the def name (e.g. "cloakraid", "staticmex"). Track units as "cloakraid#26780".

## Zero-K Basics

**Your Commander**: The first unit you receive (via unit_created/unit_finished at game start) is your commander. Its def name will be \`dyntrainer_strike_base\` or similar \`dyntrainer_*\` — this IS your commander, a powerful constructor unit. Note its unit ID from the event and use that ID for all commander orders.

**Economy**: Metal (from extractors on metal spots) + Energy (from solar/wind/fusion).
- Your commander starts as a constructor. Build metal extractors first!
- Constructors can build factories, which produce combat units.
- Keep your economy balanced: don't overspend, don't let resources overflow.

**Key unit roles**:
- **Constructors**: Build structures, reclaim wreckage, repair units
- **Factories**: Produce combat units (each factory type has different units)
- **Raiders**: Fast, cheap units for harassing and scouting
- **Assault**: Frontline combat units
- **Skirmishers**: Long-range units that kite enemies
- **Artillery**: Very long range, slow, area damage

**Key unit def names** (use these in build_def_name):

Economy & infrastructure:
- \`staticmex\` — Metal Extractor (build on metal spots!)
- \`energysolar\` — Solar Collector
- \`energywind\` — Wind Generator
- \`staticradar\` — Radar Tower
- \`staticstorage\` — Storage

Factories:
- \`factorycloak\` — Cloakbot Factory (good starter)
- \`factoryshield\` — Shieldbot Factory
- \`factoryveh\` — Rover Factory
- \`factoryhover\` — Hovercraft Factory
- \`factorygunship\` — Gunship Factory
- \`factoryjump\` — Jumpbot Factory
- \`factoryspider\` — Spider Factory
- \`factorytank\` — Tank Factory

Cloakbot units (from factorycloak):
- \`cloakcon\` — Conjurer (constructor)
- \`cloakraid\` — Glaive (raider — fast, cheap)
- \`cloakskirm\` — Rocko (skirmisher — medium range)
- \`cloakriot\` — Warrior (riot — close range AoE)
- \`cloakassault\` — Knight (assault — tough frontliner)
- \`cloakarty\` — Sling (artillery)
- \`cloaksnipe\` — Phantom (sniper)
- \`cloakaa\` — Angler (anti-air)

Defense:
- \`turretlaser\` — Lotus (light laser turret)
- \`turretmissile\` — Picket (light AA turret)
- \`turretheavylaser\` — Stinger (medium laser turret)

**Opening pattern** (your first factory is FREE and instant via "facplop"):
1. Place \`factorycloak\` near your commander — use the commander's position from its unit_finished event as x/z. Example: \`{"type":"build","unit_id":CMD_ID,"build_def_name":"factorycloak","x":CMD_X,"z":CMD_Z,"queue":false}\`. It's placed instantly at no cost!
2. The new factory sends a \`unit_finished\` event with its unit ID. Use that ID to queue production: \`cloakcon\` then 2-3 \`cloakraid\` (omit x/y/z for factory production).
3. While factory produces, use your commander to build 3 \`staticmex\` on nearby metal spots (use x/z from the metal_spots data in the init event).
4. Build 2-3 \`energysolar\` to balance energy
5. Expand to more metal spots with constructors
6. Scout the enemy with raiders, attack when you have an advantage

Map coordinates: x and z are horizontal (map plane), y is height. [0,0,0] is the north-west corner at water level.

## Sleep/Wake Pattern

After issuing commands, call \`wake:set_conditions\` to sleep until something interesting happens:

\`\`\`
wake:set_conditions({events: ["command_error", "unit_finished", "unit_idle", "enemy_enter_los", "unit_damaged", "message"], timeout_s: 30})
\`\`\`

You'll be woken when a matching event arrives OR the timeout expires. All events that occurred while you slept will be in your context when you wake up.

**Always set wake conditions after acting** — otherwise every single event triggers a new think cycle, which is wasteful. Typical wake events:
- \`command_error\` — a command failed, you need to react
- \`unit_finished\` — a unit you ordered to build is done
- \`unit_idle\` — a unit needs orders
- \`unit_damaged\` — you're under attack
- \`enemy_enter_los\` — new enemy spotted
- \`enemy_destroyed\` — kill confirmed
- \`message\` — someone sent a chat message
- \`release\` — game over

**Always include \`command_error\` and \`message\` in your wake events.**

## Narration

Use \`send_chat\` to narrate your thinking in-game. Spectators and opponents can see it. Keep it brief — one or two sentences per think cycle describing what you see and what you're doing. Example:

\`{"type":"send_chat","text":"Building 3 mexes near base, then a cloakbot factory. Economy first!"}\`

If someone sends you a message, respond with \`send_chat\`. Be sporting.

Note: You can see spectator chat (messages prefixed with \`s:\`). **Do not repeat or quote spectator messages in your own chat** — spectators may be giving private commentary or advice they don't want shared with your opponent.
`;

async function main() {
  console.log('Starting Zero-K gameplay agent...\n');

  const adapter = providerCfg.createAdapter();
  const membrane = new Membrane(adapter, {
    ...(providerCfg.formatter ? { formatter: providerCfg.formatter } : {}),
  });

  const wake = new WakeModule();

  const zkModule = new MCPLModule({
    name: 'zk',
    command: config.gmBin,
    args: ['--stdio', '--write-dir', config.writeDir],
    reconnect: false,
    shouldTriggerInference: wake.shouldTrigger,
  });

  const framework = await AgentFramework.create({
    storePath: config.storePath,
    membrane,
    agents: [
      {
        name: 'commander',
        model: providerCfg.model,
        systemPrompt: SYSTEM_PROMPT,
      },
    ],
    modules: [zkModule, wake],
  });

  framework.onTrace((event) => {
    switch (event.type) {
      case 'inference:started':
        console.log('\n[INFERENCE] Starting...');
        break;
      case 'inference:tokens': {
        const content = (event as { content?: string }).content;
        if (content) process.stdout.write(content);
        break;
      }
      case 'inference:completed':
        process.stdout.write('\n');
        console.log('[INFERENCE] Complete');
        break;
      case 'inference:failed': {
        const err = event as { error?: string; stack?: string };
        console.error('[ERROR]', err.error);
        if (err.stack) console.error(err.stack);
        break;
      }
      case 'inference:tool_calls_yielded': {
        const calls = (event as { calls: Array<{ name: string }> }).calls;
        console.log(`\n[TOOLS] ${calls.map((c) => c.name).join(', ')}`);
        break;
      }
      case 'tool:started':
        console.log('[TOOL]', (event as { tool?: string }).tool);
        break;
    }
  });

  const apiServer = new ApiServer(framework);
  await apiServer.start();

  framework.start();
  console.log('Framework started (API on :8765)');

  // Kick off the game
  const kickoff =
    config.playMode === 'lobby'
      ? `Play an online game via the Zero-K lobby:

1. Connect: call zk:lobby_connect
2. Login: call zk:lobby_login with username "${config.zkUsername}" and password "${config.zkPassword}"
3. Host: call zk:lobby_open_battle with title "Agent Practice" and map "${config.map}"
4. Add opponent: call zk:lobby_add_bot with ai_lib "${config.opponent}"
5. Start: call zk:lobby_start_battle

Then set wake conditions for the "unit_finished" event — this is when your commander spawns and you learn its unit ID and position. When you wake, begin your opening: place factorycloak at your commander's x/z position (include coordinates!), then queue production on the new factory (omit coords for that), then build 3 staticmex on metal spots with your commander, then 2-3 energysolar.`
      : `Start a local game: call zk:lobby_start_game with map "${config.map}", opponent "${config.opponent}", and player_mode: true. Then set wake conditions for the "unit_finished" event — this is when your commander spawns and you learn its unit ID and position. When you wake, begin your opening: place factorycloak at your commander's x/z position (include coordinates!), then queue production on the new factory (omit coords for that), then build 3 staticmex on metal spots with your commander, then 2-3 energysolar.`;

  framework.pushEvent({
    type: 'external-message',
    source: 'system',
    content: kickoff,
    metadata: { initial: true },
    triggerInference: true,
  });

  process.on('SIGINT', async () => {
    console.log('\nShutting down...');
    await apiServer.stop();
    await framework.stop();
    process.exit(0);
  });

  console.log('\n' + '='.repeat(50));
  console.log('Zero-K gameplay agent running');
  console.log('='.repeat(50));
  console.log(`Provider:    ${config.provider} (${providerCfg.model})`);
  console.log(`Play mode:   ${config.playMode}`);
  console.log(`GameManager: ${config.gmBin}`);
  console.log(`Write dir:   ${config.writeDir}`);
  console.log(`Map:         ${config.map}`);
  console.log(`Opponent:    ${config.opponent}`);
  if (config.playMode === 'lobby') {
    console.log(`ZK user:     ${config.zkUsername}`);
  }
  console.log('Press Ctrl+C to stop.\n');
}

main().catch((err) => {
  console.error('Fatal error:', err);
  process.exit(1);
});
