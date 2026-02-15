"""
Reusable MCPL stdio client for testing game-manager.

Speaks newline-delimited JSON-RPC 2.0 over subprocess stdin/stdout.
Handles bidirectional communication: sends requests, receives responses,
and auto-responds to server-initiated requests (push/event, channels/incoming).
"""

import json
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any, Optional


class McplStdioClient:
    """Mock MCPL client that communicates with game-manager over stdio."""

    def __init__(self, cmd: list[str], cwd: Optional[str] = None, verbose: bool = False):
        self.verbose = verbose
        self.proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            cwd=cwd,
            preexec_fn=os.setsid,  # new process group so we can kill children
        )
        self._next_id = 1
        self._lock = threading.Lock()

        # Pending request tracking: id -> {event, result}
        self._pending: dict[int, dict] = {}

        # Incoming events from server
        self.push_events: list[dict] = []
        self.sai_events: list[dict] = []
        self.notifications: list[dict] = []

        # Start reader threads
        self._alive = True
        self._reader_thread = threading.Thread(target=self._read_loop, daemon=True)
        self._reader_thread.start()
        self._stderr_thread = threading.Thread(target=self._stderr_loop, daemon=True)
        self._stderr_thread.start()

        # Stderr capture
        self.stderr_lines: list[str] = []

    def _log(self, direction: str, msg: dict):
        if self.verbose:
            text = json.dumps(msg, separators=(",", ":"))
            if len(text) > 200:
                text = text[:200] + "..."
            print(f"  [{direction}] {text}", file=sys.stderr)

    def _next_request_id(self) -> int:
        with self._lock:
            rid = self._next_id
            self._next_id += 1
            return rid

    def _send(self, msg: dict):
        self._log(">>>", msg)
        line = json.dumps(msg) + "\n"
        self.proc.stdin.write(line.encode())
        self.proc.stdin.flush()

    def _send_response(self, msg_id: int, result: Any):
        self._send({"jsonrpc": "2.0", "id": msg_id, "result": result})

    def _read_loop(self):
        """Background thread: read lines from stdout and dispatch."""
        while self._alive:
            try:
                line = self.proc.stdout.readline()
                if not line:
                    break
                line = line.decode().strip()
                if not line:
                    continue
                msg = json.loads(line)
                self._log("<<<", msg)
                self._dispatch(msg)
            except (json.JSONDecodeError, ValueError) as e:
                if self.verbose:
                    print(f"  [ERR] Parse error: {e}", file=sys.stderr)
            except Exception:
                break

    def _stderr_loop(self):
        """Background thread: capture stderr."""
        while self._alive:
            try:
                line = self.proc.stderr.readline()
                if not line:
                    break
                text = line.decode().rstrip()
                self.stderr_lines.append(text)
                if self.verbose:
                    print(f"  [gm] {text}", file=sys.stderr)
            except Exception:
                break

    def _dispatch(self, msg: dict):
        has_id = "id" in msg
        has_method = "method" in msg
        has_result = "result" in msg
        has_error = "error" in msg

        if has_id and (has_result or has_error):
            # Response to our request
            msg_id = msg["id"]
            if msg_id in self._pending:
                self._pending[msg_id]["result"] = msg
                self._pending[msg_id]["event"].set()

        elif has_id and has_method:
            # Server-initiated request — must respond
            method = msg["method"]
            params = msg.get("params", {})

            if method == "push/event":
                self.push_events.append(params)
                self._send_response(msg["id"], {"accepted": True})
            elif method == "channels/incoming":
                self.sai_events.append(params)
                self._send_response(msg["id"], {})
            elif method == "channels/changed":
                self.notifications.append({"method": method, "params": params})
                self._send_response(msg["id"], {})
            else:
                # Unknown server request — respond with empty result
                self._send_response(msg["id"], {})

        elif has_method and not has_id:
            # Notification from server
            self.notifications.append(msg)

    def _request(self, method: str, params: Optional[dict] = None, timeout: float = 30) -> dict:
        """Send a JSON-RPC request and wait for the response."""
        msg_id = self._next_request_id()
        msg: dict = {"jsonrpc": "2.0", "id": msg_id, "method": method}
        if params is not None:
            msg["params"] = params

        entry = {"event": threading.Event(), "result": None}
        self._pending[msg_id] = entry

        self._send(msg)

        if not entry["event"].wait(timeout=timeout):
            del self._pending[msg_id]
            raise TimeoutError(f"Timed out waiting for response to {method} (id={msg_id})")

        result_msg = entry["result"]
        del self._pending[msg_id]

        if "error" in result_msg:
            raise RuntimeError(f"RPC error: {result_msg['error']}")
        return result_msg.get("result", {})

    def handshake(self) -> dict:
        """Perform MCP initialize handshake."""
        result = self._request("initialize", {
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "experimental": {
                    "mcpl": {
                        "version": "0.4",
                        "pushEvents": True,
                        "channels": True,
                        "rollback": True,
                    }
                }
            },
            "clientInfo": {
                "name": "integration-test",
                "version": "0.1.0",
            },
        })

        # Send initialized notification
        self._send({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        })

        return result

    def call_tool(self, name: str, args: Optional[dict] = None) -> dict:
        """Call an MCP tool and return the result."""
        params = {"name": name}
        if args:
            params["arguments"] = args
        return self._request("tools/call", params)

    def list_tools(self) -> dict:
        """List available tools."""
        return self._request("tools/list")

    def list_channels(self) -> dict:
        """List active game channels."""
        return self._request("channels/list")

    def publish(self, channel_id: str, command: dict) -> dict:
        """Send a SAI command via channels/publish."""
        return self._request("channels/publish", {
            "channelId": channel_id,
            "content": [{"type": "text", "text": json.dumps(command)}],
        })

    def collect_events(self, timeout: float = 5.0) -> tuple[list[dict], list[dict]]:
        """Wait for `timeout` seconds, then return (push_events, sai_events) accumulated."""
        start_push = len(self.push_events)
        start_sai = len(self.sai_events)
        time.sleep(timeout)
        return (
            self.push_events[start_push:],
            self.sai_events[start_sai:],
        )

    def wait_for_sai_event(self, event_type: str, timeout: float = 30.0) -> Optional[dict]:
        """Wait until a SAI event of the given type appears."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for evt in self.sai_events:
                messages = evt.get("messages", [])
                for m in messages:
                    for block in m.get("content", []):
                        text = block.get("text", "")
                        try:
                            parsed = json.loads(text)
                            if parsed.get("type") == event_type:
                                return parsed
                        except (json.JSONDecodeError, TypeError):
                            if f'"type":"{event_type}"' in text or f'"type": "{event_type}"' in text:
                                return {"_raw": text}
            time.sleep(0.5)
        return None

    def wait_for_notification(self, method: str, timeout: float = 30.0) -> Optional[dict]:
        """Wait until a notification with the given method name appears."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for notif in self.notifications:
                notif_method = notif.get("method", "")
                if notif_method == method:
                    return notif
            time.sleep(0.5)
        return None

    def close(self):
        """Stop the subprocess and all its children.

        Sends SIGTERM to the process group first (game-manager + engine),
        then SIGKILL as fallback. Using process groups ensures the engine
        child is killed even if game-manager doesn't get to run destructors.
        """
        self._alive = False
        pgid = None
        try:
            pgid = os.getpgid(self.proc.pid)
        except (ProcessLookupError, OSError):
            pass

        try:
            if pgid:
                os.killpg(pgid, signal.SIGTERM)
            else:
                self.proc.terminate()
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                if pgid:
                    os.killpg(pgid, signal.SIGKILL)
                else:
                    self.proc.kill()
                self.proc.wait(timeout=5)
            except Exception:
                pass
        except Exception:
            pass
