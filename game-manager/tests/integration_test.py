#!/usr/bin/env python3
"""
Integration test for the game-manager + spring-headless + SAI bridge pipeline.

Tiers:
  1. Engine launch — verify infolog.txt creation
  2. SAI boot — verify Init and Update events from SAI bridge
  3. Command round-trip — send a command, observe unit events
  4. Widget tools — verify dynamic tool registration and invocation

Usage:
  python3 tests/integration_test.py [--tier 1|2|3|4] [--map MAP] [--game GAME] [--verbose]
"""

import argparse
import os
import re
import shutil
import sys
import tempfile
import time
from pathlib import Path

# Add tests/ to path for mcpl_client import
sys.path.insert(0, str(Path(__file__).parent))
from mcpl_client import McplStdioClient


def wait_for_file(path: Path, timeout: float = 30.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists() and path.stat().st_size > 0:
            return True
        time.sleep(0.5)
    return False


def extract_channel_id(result: dict) -> str:
    """Extract channel_id from lobby_start_game result text."""
    content = result.get("content", [])
    for block in content:
        text = block.get("text", "")
        match = re.search(r"channel: (game:[a-zA-Z0-9_-]+)", text)
        if match:
            return match.group(1)
    raise ValueError(f"Could not extract channel_id from: {result}")


PERSISTENT_WRITE_DIR = Path.home() / ".spring-test-agent"


class IntegrationTest:
    def __init__(self, args):
        self.tier = args.tier
        self.map_name = args.map
        self.game_name = args.game
        self.opponent = args.opponent
        self.verbose = args.verbose
        self.timeout = args.timeout
        self.fresh = args.fresh
        self.player_mode = args.player_mode
        if self.fresh:
            self.write_dir = Path(tempfile.mkdtemp(prefix="gm-integration-"))
        else:
            self.write_dir = PERSISTENT_WRITE_DIR
        self.gm_dir = Path(__file__).parent.parent
        self.client = None
        self.channel_id = None
        self.passed = 0
        self.failed = 0
        self.warnings = 0

    def log(self, status: str, msg: str):
        symbol = {"PASS": "\033[32m+\033[0m", "FAIL": "\033[31m!\033[0m", "WARN": "\033[33m?\033[0m", "INFO": " "}
        print(f"  {symbol.get(status, ' ')} {status}: {msg}")

    def check(self, condition: bool, msg: str) -> bool:
        if condition:
            self.passed += 1
            self.log("PASS", msg)
            return True
        else:
            self.failed += 1
            self.log("FAIL", msg)
            return False

    def warn(self, msg: str):
        self.warnings += 1
        self.log("WARN", msg)

    def run(self):
        mode = "player" if self.player_mode else "AI"
        print(f"\n=== Integration Test (Tier {self.tier}, {mode} mode) ===")
        print(f"  Write-dir: {self.write_dir}")
        print(f"  Map: {self.map_name}, Game: {self.game_name}, Opponent: {self.opponent}")
        print()

        try:
            self._start_client()

            if self.tier >= 1:
                self._tier1_engine_launch()
            if self.tier >= 2:
                self._tier2_sai_boot()
            if self.tier >= 3:
                self._tier3_command_roundtrip()
            if self.tier >= 4:
                self._tier4_widget_tools()

        except Exception as e:
            self.failed += 1
            self.log("FAIL", f"Unhandled exception: {e}")
            if self.verbose:
                import traceback
                traceback.print_exc()
        finally:
            self._cleanup()

        print()
        print(f"=== Results: {self.passed} passed, {self.failed} failed, {self.warnings} warnings ===")
        return self.failed == 0

    def _start_client(self):
        print("--- Starting game-manager ---")
        cmd = [
            "cargo", "run", "--",
            "--stdio",
            "--write-dir", str(self.write_dir),
        ]
        self.client = McplStdioClient(cmd, cwd=str(self.gm_dir), verbose=self.verbose)

        # Wait a moment for cargo build + startup
        time.sleep(0.5)

        result = self.client.handshake()
        server_name = result.get("serverInfo", {}).get("name", "unknown")
        self.check(server_name == "zk-game-manager", f"Handshake complete (server: {server_name})")

    def _tier1_engine_launch(self):
        print("\n--- Tier 1: Engine Launch ---")

        tool_args = {
            "map": self.map_name,
            "game": self.game_name,
            "opponent": self.opponent,
            "headless": not self.player_mode,
        }
        if self.player_mode:
            tool_args["player_mode"] = True
        result = self.client.call_tool("lobby_start_game", tool_args)

        # Check for error
        is_error = result.get("isError", False)
        if is_error:
            content_text = result.get("content", [{}])[0].get("text", "")
            self.check(False, f"lobby_start_game failed: {content_text}")
            return

        try:
            self.channel_id = extract_channel_id(result)
            self.check(True, f"lobby_start_game succeeded (channel: {self.channel_id})")
        except ValueError:
            self.check(False, f"Could not extract channel_id: {result}")
            return

        # Wait for infolog.txt
        infolog = self.write_dir / "infolog.txt"
        start = time.monotonic()
        found = wait_for_file(infolog, timeout=self.timeout)
        elapsed = time.monotonic() - start
        self.check(found, f"infolog.txt created (took {elapsed:.1f}s)")

        if found and self.verbose:
            # Print first few lines of infolog
            lines = infolog.read_text().splitlines()[:10]
            for line in lines:
                print(f"    | {line}")

    def _tier2_sai_boot(self):
        print("\n--- Tier 2: SAI Boot Verification ---")

        if not self.channel_id:
            self.check(False, "No channel_id from tier 1, skipping")
            return

        # Wait for SAI init event
        init_event = self.client.wait_for_sai_event("init", timeout=self.timeout)
        self.check(init_event is not None, "SAI Init event received")

        if init_event is None:
            self.warn("SAI bridge did not connect — check infolog.txt for errors")
            # Print recent stderr for debugging
            if self.client.stderr_lines:
                recent = self.client.stderr_lines[-10:]
                for line in recent:
                    print(f"    | {line}")
            return

        # Wait for unit events (game is ticking — units spawn shortly after Init)
        unit_event = self.client.wait_for_sai_event("unit_created", timeout=self.timeout)
        self.check(unit_event is not None, "Game is ticking (unit_created events)")

        # Verify channels/list shows SAI connected
        try:
            channels = self.client.list_channels()
            game_channels = [
                c for c in channels.get("channels", [])
                if c.get("id") == self.channel_id
            ]
            if game_channels:
                connected = game_channels[0].get("metadata", {}).get("saiConnected", False)
                self.check(connected, "channels/list shows saiConnected=true")
            else:
                self.check(False, f"Channel {self.channel_id} not in channels/list")
        except Exception as e:
            self.check(False, f"channels/list failed: {e}")

    def _tier3_command_roundtrip(self):
        print("\n--- Tier 3: Command Round-Trip ---")

        if not self.channel_id:
            self.check(False, "No channel_id, skipping")
            return

        # Send a chat command (simplest — no unit needed)
        try:
            result = self.client.publish(
                self.channel_id,
                {"type": "send_chat", "text": "Hello from integration test"},
            )
            delivered = result.get("delivered", False)
            self.check(delivered, "SendChat command delivered")
        except Exception as e:
            self.check(False, f"publish failed: {e}")

        # Collect events for a few seconds to observe more activity
        push_events, sai_events = self.client.collect_events(timeout=10)

        # Count unit events across ALL accumulated SAI events (including from tier 2)
        unit_event_count = 0
        for evt in self.client.sai_events:
            messages = evt.get("messages", [])
            for m in messages:
                for block in m.get("content", []):
                    text = block.get("text", "")
                    if any(t in text for t in ["unit_created", "unit_finished", "unit_idle"]):
                        unit_event_count += 1

        if unit_event_count > 0:
            self.check(True, f"Received {unit_event_count} unit events from SAI")
        else:
            self.warn(f"No unit events in {len(self.client.sai_events)} total SAI messages (game may still be loading)")

        # Report total event counts
        self.log("INFO", f"Total: {len(self.client.sai_events)} SAI events ({len(sai_events)} new in tier 3)")

    def _tier4_widget_tools(self):
        print("\n--- Tier 4: Widget Tool Registration & Invocation ---")

        if not self.channel_id:
            self.check(False, "No channel_id, skipping")
            return

        # 1. Wait for tools/list_changed notification (widgets registering tools)
        notif = self.client.wait_for_notification(
            "notifications/tools/list_changed", timeout=self.timeout
        )
        self.check(notif is not None, "Received tools/list_changed notification")

        # 2. List tools and verify dynamic widget tools appear
        try:
            tools_result = self.client.list_tools()
            tool_names = [t.get("name", "") for t in tools_result.get("tools", [])]

            # Check for expected widget tools
            expected_tools = [
                "economy:snapshot",
                "roster:own",
                "intel:known_enemies",
                "threat:sectors",
                "squad:list",
            ]
            found_tools = [t for t in expected_tools if t in tool_names]
            missing_tools = [t for t in expected_tools if t not in tool_names]

            if found_tools:
                self.check(True, f"Dynamic tools registered: {', '.join(found_tools)}")
            else:
                self.check(False, f"No dynamic tools found in tools/list ({len(tool_names)} total tools)")

            if missing_tools:
                self.warn(f"Missing expected tools: {', '.join(missing_tools)}")

            if self.verbose:
                for name in tool_names:
                    prefix = "  *" if name in expected_tools else "   "
                    print(f"    |{prefix} {name}")
        except Exception as e:
            self.check(False, f"tools/list failed: {e}")
            return

        # 3. Call economy:snapshot — should return real game data
        try:
            result = self.client.call_tool("economy:snapshot", {})
            is_error = result.get("isError", False)
            content = result.get("content", [])
            text = content[0].get("text", "") if content else ""

            if is_error:
                self.check(False, f"economy:snapshot returned error: {text}")
            else:
                # Should contain metal/energy data
                import json as json_mod
                try:
                    data = json_mod.loads(text)
                    has_metal = "metal" in data
                    has_energy = "energy" in data
                    self.check(
                        has_metal and has_energy,
                        f"economy:snapshot returned valid data (metal income: {data.get('metal', {}).get('income', '?')})"
                    )
                except (json_mod.JSONDecodeError, TypeError):
                    self.check(False, f"economy:snapshot returned non-JSON: {text[:100]}")
        except TimeoutError:
            self.check(False, "economy:snapshot timed out (5s tool timeout?)")
        except Exception as e:
            self.check(False, f"economy:snapshot call failed: {e}")

        # 4. Call roster:own — should return unit groups
        try:
            result = self.client.call_tool("roster:own", {})
            is_error = result.get("isError", False)
            content = result.get("content", [])
            text = content[0].get("text", "") if content else ""

            if is_error:
                self.check(False, f"roster:own returned error: {text}")
            else:
                import json as json_mod
                try:
                    data = json_mod.loads(text)
                    total = data.get("total", 0)
                    self.check(total > 0, f"roster:own shows {total} own units")
                except (json_mod.JSONDecodeError, TypeError):
                    self.check(False, f"roster:own returned non-JSON: {text[:100]}")
        except TimeoutError:
            self.check(False, "roster:own timed out")
        except Exception as e:
            self.check(False, f"roster:own call failed: {e}")

        # 5. Call squad:list — should return empty squads (none created yet)
        try:
            result = self.client.call_tool("squad:list", {})
            is_error = result.get("isError", False)
            content = result.get("content", [])
            text = content[0].get("text", "") if content else ""

            if not is_error and "squads" in text:
                self.check(True, "squad:list returned valid response")
            else:
                self.check(False, f"squad:list unexpected response: {text[:100]}")
        except Exception as e:
            self.check(False, f"squad:list call failed: {e}")

    def _cleanup(self):
        print("\n--- Cleanup ---")
        if self.client:
            self.client.close()
            print(f"  Game-manager stopped")

        if self.write_dir.exists():
            # Check infolog for errors before removing
            infolog = self.write_dir / "infolog.txt"
            if infolog.exists():
                content = infolog.read_text()
                error_lines = [l for l in content.splitlines() if "[Error]" in l or "[Fatal]" in l]
                if error_lines:
                    print(f"  Engine errors found in infolog.txt:")
                    for line in error_lines[:5]:
                        print(f"    | {line}")

            if self.fresh:
                shutil.rmtree(self.write_dir, ignore_errors=True)
                print(f"  Removed {self.write_dir}")
            else:
                # Persistent mode: clean up per-run artifacts but keep cache
                for f in (self.write_dir / "temp").glob("gm_script_*.txt"):
                    f.unlink(missing_ok=True)
                if self.failed > 0 and infolog.exists():
                    print(f"  Preserving infolog.txt for debugging ({infolog})")
                elif infolog.exists():
                    infolog.unlink()
                print(f"  Cleaned per-run files in {self.write_dir} (cache preserved)")


def main():
    parser = argparse.ArgumentParser(description="Game-manager integration test")
    parser.add_argument("--tier", type=int, default=3, choices=[1, 2, 3, 4],
                        help="Test tier (1=launch, 2=SAI boot, 3=commands, 4=widget tools)")
    parser.add_argument("--map", default="SimpleChess",
                        help="Map name for the test game")
    parser.add_argument("--game", default="Zero-K $VERSION",
                        help="Game type / archive name")
    parser.add_argument("--opponent", default="NullAI",
                        help="Opponent AI shortname")
    parser.add_argument("--timeout", type=float, default=120,
                        help="Timeout in seconds for engine operations")
    parser.add_argument("--fresh", action="store_true",
                        help="Use a fresh tempdir (slow: full archive rescan). Default: persistent ~/.spring-test-agent")
    parser.add_argument("--player-mode", action="store_true",
                        help="Use player mode (agent as PLAYER slot, widget fires /aicontrol)")
    parser.add_argument("--verbose", "-v", action="store_true",
                        help="Verbose output (show JSON-RPC traffic)")
    args = parser.parse_args()

    test = IntegrationTest(args)
    success = test.run()
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
