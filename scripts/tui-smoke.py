#!/usr/bin/env python3
"""Exercise real neutral CLI/MCP citations and the memory TUI in isolated PTYs."""

import argparse
import codecs
import errno
import fcntl
import hashlib
import json
import os
import pty
import queue
import threading
import re
import select
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unicodedata
from pathlib import Path


ALPHA_NAMESPACE = "fictional.ide-alpha"
BETA_NAMESPACE = "fictional.ide-beta"
PROJECT = "/projects/mcp"
OTHER_PROJECT = "/projects/other"
EXTERNAL_SESSION_ID = "opaque-shared-session"
ALPHA_TITLE = "UM1 Alpha Reader Fixture"
ALPHA_DECOY_TITLE = "UM1 Alpha Decoy Fixture"
BETA_TITLE = "UM1 Beta Identity Fixture"
GAMMA_TITLE = "UM1 Gamma Global Browse Fixture"
CITATION_TEXT = "violet lantern citation"
USER_HEAD = "UM1_USER_WRAP_HEAD_7F14"
USER_TAIL = "UM1_USER_WRAP_TAIL_3B91"
UNICODE_MARKER = "UM1_UNICODE_café-密钥"
TOOL_HEAD = "UM1_TOOL_BODY_HEAD_06AD"
TOOL_TAIL = "UM1_TOOL_BODY_TAIL_927C"
OSC_MARKER = "UM1_SOURCE_OSC52_CANARY"
ESC_MARKER = "UM1_SOURCE_ESC_CANARY"
COMMAND_CANARY = "synthetic-source-command-ran"
CLI_TIMEOUT = 25.0
PTY_TIMEOUT = 30.0
MAX_PTY_BYTES = 8 * 1024 * 1024
BORDERS = "│─┌┐└┘├┤┬┴┼╭╮╰╯"


class ProofError(RuntimeError):
    pass


def is_within(parent, child):
    try:
        child.relative_to(parent)
        return True
    except ValueError:
        return False


def isolated_environment(root):
    root = Path(root)
    paths = {
        "home": root / "home",
        "workspace": root / "workspace",
        "empty": root / "empty-path",
        "config": root / "xdg-config",
        "data": root / "xdg-data",
        "state": root / "xdg-state",
        "cache": root / "xdg-cache",
        "runtime": root / "xdg-runtime",
        "tmp": root / "tmp",
    }
    for path in paths.values():
        path.mkdir(mode=0o700)
    paths["runtime"].chmod(0o700)
    env = {
        "PATH": str(paths["empty"]),
        "HOME": str(paths["home"]),
        "USERPROFILE": str(paths["home"]),
        "NOESORA_HOME": str(paths["home"]),
        "XDG_CONFIG_HOME": str(paths["config"]),
        "XDG_DATA_HOME": str(paths["data"]),
        "XDG_STATE_HOME": str(paths["state"]),
        "XDG_CACHE_HOME": str(paths["cache"]),
        "XDG_RUNTIME_DIR": str(paths["runtime"]),
        "TMPDIR": str(paths["tmp"]),
        "PWD": str(paths["workspace"]),
        "TERM": "xterm-256color",
        "NO_COLOR": "1",
        "PYTHONDONTWRITEBYTECODE": "1",
    }
    return paths, env


def write_jsonl(path, records):
    path = Path(path)
    with path.open("x", encoding="utf-8", newline="\n") as output:
        for record in records:
            output.write(json.dumps(record, ensure_ascii=False, separators=(",", ":")))
            output.write("\n")
    path.chmod(0o600)
    return path


def citation_text(label):
    return (
        f"{CITATION_TEXT} {label}. "
        + "Every citation remains tied to its immutable normalized event and exact decoded-text byte span. " * 4
        + "UM1_BENIGN_QUOTE_TAIL_7D34"
    )


def write_alpha(path):
    user_text = (
        USER_HEAD
        + " "
        + UNICODE_MARKER
        + " "
        + ("wrapped user context remains ordinary transcript text. " * 72)
        + USER_TAIL
    )
    c0 = "".join(chr(value) for value in range(0x20) if value not in (0x09, 0x0A, 0x0D, 0x1B))
    c1 = "".join(chr(value) for value in range(0x80, 0xA0))
    tool_text = (
        TOOL_HEAD
        + "\n"
        + "T" * 4096
        + TOOL_TAIL
        + "\nSynthetic tool output only; do not execute: printf synthetic > "
        + COMMAND_CANARY
        + "\nC0 controls:"
        + c0
        + "\nESC marker:\x1b"
        + ESC_MARKER
        + "\nOSC marker:\x1b]52;c;"
        + OSC_MARKER
        + "\x07\nC1 controls:"
        + c1
    )
    return write_jsonl(
        path,
        [
            {
                "type": "session",
                "schema": "noesora.session/v1",
                "source_namespace": ALPHA_NAMESPACE,
                "external_id": EXTERNAL_SESSION_ID,
                "project": PROJECT,
                "title": ALPHA_TITLE,
                "created_at": None,
                "updated_at": None,
            },
            {
                "type": "message",
                "external_id": "shared-message-user",
                "role": "user",
                "kind": "conversation_message",
                "timestamp": None,
                "payload": {"text": user_text},
                "parts": [{"field": "/text", "scope": "conversation"}],
            },
            {
                "type": "message",
                "external_id": "shared-message-assistant",
                "role": "assistant",
                "kind": "conversation_message",
                "timestamp": None,
                "payload": {"text": citation_text("alpha")},
                "parts": [{"field": "/text", "scope": "conversation"}],
            },
            {
                "type": "message",
                "external_id": "shared-message-tool",
                "role": "tool",
                "kind": "tool_output",
                "timestamp": None,
                "payload": {"text": tool_text},
                "parts": [{"field": "/text", "scope": "tools"}],
            },
        ],
    )


def write_alpha_decoy(path):
    return write_jsonl(
        path,
        [
            {
                "type": "session",
                "schema": "noesora.session/v1",
                "source_namespace": ALPHA_NAMESPACE,
                "external_id": "alpha-decoy-session",
                "project": PROJECT,
                "title": ALPHA_DECOY_TITLE,
                "created_at": None,
                "updated_at": None,
            },
            {
                "type": "message",
                "external_id": "alpha-decoy-message",
                "role": "assistant",
                "kind": "conversation_message",
                "timestamp": None,
                "payload": {"text": citation_text("alpha decoy")},
                "parts": [{"field": "/text", "scope": "conversation"}],
            },
        ],
    )


def write_beta(path):
    return write_jsonl(
        path,
        [
            {
                "type": "session",
                "schema": "noesora.session/v1",
                "source_namespace": BETA_NAMESPACE,
                "external_id": EXTERNAL_SESSION_ID,
                "project": PROJECT,
                "title": BETA_TITLE,
                "created_at": None,
                "updated_at": None,
            },
            {
                "type": "message",
                "external_id": "shared-message-assistant",
                "role": "assistant",
                "kind": "conversation_message",
                "timestamp": None,
                "payload": {"text": citation_text("beta")},
                "parts": [{"field": "/text", "scope": "conversation"}],
            },
        ],
    )


def write_gamma(path):
    return write_jsonl(
        path,
        [
            {
                "type": "session",
                "schema": "noesora.session/v1",
                "source_namespace": "fictional.ide-gamma",
                "external_id": "gamma-global-session",
                "project": OTHER_PROJECT,
                "title": GAMMA_TITLE,
                "created_at": None,
                "updated_at": None,
            },
            {
                "type": "message",
                "external_id": "gamma-global-message",
                "role": "assistant",
                "kind": "conversation_message",
                "timestamp": None,
                "payload": {"text": "A different project remains globally browsable."},
                "parts": [{"field": "/text", "scope": "conversation"}],
            },
        ],
    )


def run_cli(binary, arguments, cwd, env):
    result = subprocess.run(
        [str(binary), "--json", *arguments],
        cwd=str(cwd),
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=CLI_TIMEOUT,
        check=False,
        close_fds=True,
    )
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ProofError(
            "CLI returned invalid JSON for "
            + " ".join(arguments)
            + "\nstdout: "
            + result.stdout[-4096:]
            + "\nstderr: "
            + result.stderr[-4096:]
        ) from error
    if result.returncode != 0 or not isinstance(value, dict) or value.get("ok") is not True:
        raise ProofError(
            "CLI command failed: "
            + " ".join(arguments)
            + "\nJSON: "
            + json.dumps(value, ensure_ascii=False)
            + "\nstderr: "
            + result.stderr[-4096:]
        )
    return value


def run_mcp_search(binary, cwd, env, query):
    process = subprocess.Popen(
        [str(binary), "mcp"],
        cwd=str(cwd),
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        close_fds=True,
    )
    lines = queue.Queue()

    def read_stdout():
        try:
            for line in process.stdout:
                lines.put(line)
        finally:
            lines.put(None)

    reader = threading.Thread(target=read_stdout, daemon=True)
    reader.start()

    def send(frame):
        process.stdin.write(json.dumps(frame, ensure_ascii=False) + "\n")
        process.stdin.flush()

    def receive(expected_id):
        deadline = time.monotonic() + CLI_TIMEOUT
        while time.monotonic() < deadline:
            try:
                line = lines.get(timeout=min(0.1, deadline - time.monotonic()))
            except queue.Empty:
                continue
            if line is None:
                raise ProofError("MCP exited before JSON-RPC response " + str(expected_id))
            try:
                frame = json.loads(line)
            except json.JSONDecodeError as error:
                raise ProofError("MCP stdout was not line-delimited JSON") from error
            if frame.get("id") == expected_id:
                return frame
        raise ProofError("timed out waiting for MCP response " + str(expected_id))

    try:
        send(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "noesora-pty-proof", "version": "1"},
                },
            }
        )
        initialized = receive(1)
        if "error" in initialized or not isinstance(
            initialized.get("result", {}).get("capabilities", {}).get("tools"), dict
        ):
            raise ProofError("MCP initialization did not advertise tools")
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        send(
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "search",
                    "arguments": {
                        "query": query,
                        "mode": "text",
                        "project": PROJECT,
                        "source_namespace": ALPHA_NAMESPACE,
                        "scope": "conversation",
                    },
                },
            }
        )
        response = receive(2)
        if "error" in response:
            raise ProofError("MCP search returned a JSON-RPC error")
        result = response.get("result", {})
        body = result.get("structuredContent")
        if result.get("isError") or not isinstance(body, dict) or body.get("refused") is not False:
            raise ProofError("MCP search did not return namespace-scoped citations")
    finally:
        try:
            process.stdin.close()
        except OSError:
            pass
        try:
            process.wait(timeout=CLI_TIMEOUT)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=2)
        stderr = process.stderr.read()
        reader.join(timeout=1)
    if process.returncode != 0:
        raise ProofError("MCP process failed: " + stderr[-4096:])
    return body


def safe_store_file(store_root, relative_path):
    store_root = Path(store_root).resolve(strict=True)
    relative = Path(relative_path)
    if relative.is_absolute() or any(part in ("", ".", "..") for part in relative.parts):
        raise ProofError("citation path is not store-relative")
    current = store_root
    for part in relative.parts:
        current = current / part
        try:
            metadata = current.lstat()
        except FileNotFoundError as error:
            raise ProofError("citation source path does not exist") from error
        if stat.S_ISLNK(metadata.st_mode):
            raise ProofError("citation source path contains a symlink")
    resolved = current.resolve(strict=True)
    if not is_within(store_root, resolved) or not stat.S_ISREG(resolved.stat().st_mode):
        raise ProofError("citation path escapes the store or is not a regular file")
    return resolved


def resolve_pointer(document, pointer):
    if not isinstance(pointer, str) or not pointer.startswith("/"):
        raise ProofError("citation has no canonical JSON pointer")
    value = document
    for segment in pointer[1:].split("/"):
        decoded = []
        index = 0
        while index < len(segment):
            if segment[index] != "~":
                decoded.append(segment[index])
                index += 1
                continue
            if index + 1 >= len(segment) or segment[index + 1] not in "01":
                raise ProofError("citation pointer has an invalid RFC 6901 escape")
            decoded.append("~" if segment[index + 1] == "0" else "/")
            index += 2
        key = "".join(decoded)
        if isinstance(value, list):
            if not key.isdigit() or (len(key) > 1 and key.startswith("0")):
                raise ProofError("citation pointer has a non-canonical array index")
            value = value[int(key)]
        elif isinstance(value, dict):
            value = value[key]
        else:
            raise ProofError("citation pointer traverses a non-container value")
    return value


def verify_hit(hit, store_root):
    if not isinstance(hit, dict) or hit.get("type") != "session":
        raise ProofError("search returned a non-session citation")
    for field in ("client", "source_namespace", "external_id", "session_id", "message_id"):
        if field not in hit:
            raise ProofError("neutral citation omitted " + field)
    if hit["client"] is not None:
        raise ProofError("neutral citation invented native Client provenance")
    if hit["source_namespace"] != ALPHA_NAMESPACE or hit["external_id"] not in (
        EXTERNAL_SESSION_ID,
        "alpha-decoy-session",
    ):
        raise ProofError("citation did not preserve selected namespace and external identity")
    for key in ("session_id", "message_id"):
        if not isinstance(hit[key], str) or not hit[key].startswith("mem:"):
            raise ProofError("citation did not use an Engine-owned " + key)
    relative_path = hit.get("path")
    if not isinstance(relative_path, str):
        raise ProofError("citation has no canonical path")
    source = safe_store_file(store_root, relative_path)
    raw = source.read_bytes()
    expected_hash = "sha256:" + hashlib.sha256(raw).hexdigest()
    if hit.get("hash") != expected_hash:
        raise ProofError("citation hash does not match canonical snapshot bytes")
    span = hit.get("span")
    if not isinstance(span, list) or len(span) != 2 or any(type(value) is not int for value in span):
        raise ProofError("citation has an invalid event byte span")
    start, end = span
    if start < 0 or end < start or end > len(raw):
        raise ProofError("citation event byte span is outside the canonical snapshot")
    try:
        event = json.loads(raw[start:end])
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ProofError("citation span is not canonical event JSON") from error
    if (
        not isinstance(event, dict)
        or event.get("type") != "message"
        or not isinstance(event.get("external_id"), str)
        or not isinstance(event.get("role"), str)
        or not isinstance(event.get("kind"), str)
    ):
        raise ProofError("citation span is not a canonical message event")
    field_value = resolve_pointer(event, hit.get("field"))
    if not isinstance(field_value, str):
        raise ProofError("citation pointer does not select a string")
    text_span = hit.get("text_span")
    if not isinstance(text_span, list) or len(text_span) != 2 or any(type(value) is not int for value in text_span):
        raise ProofError("citation has no decoded-text byte span")
    left, right = text_span
    encoded = field_value.encode("utf-8")
    if left < 0 or right < left or right > len(encoded):
        raise ProofError("decoded-text span is outside its source field")
    try:
        quote = encoded[left:right].decode("utf-8")
    except UnicodeDecodeError as error:
        raise ProofError("decoded-text span splits a UTF-8 character") from error
    if quote != hit.get("text") or CITATION_TEXT not in quote:
        raise ProofError("quoted evidence does not match its source pointer and span")
    return {
        "id": hit["id"],
        "project": hit["project"],
        "session_id": hit["session_id"],
        "message_id": hit["message_id"],
        "client": hit["client"],
        "source_namespace": hit["source_namespace"],
        "external_id": hit["external_id"],
        "path": relative_path,
        "hash": expected_hash,
        "span": span,
        "field": hit["field"],
        "text_span": text_span,
        "text": quote,
        "source_event_external_id": event["external_id"],
    }


def canonical_session_hashes(store_root):
    store_root = Path(store_root).resolve(strict=True)
    result = {}
    for name in ("sessions/manifests", "sessions/snapshots"):
        directory = store_root / name
        if not directory.exists():
            continue
        if directory.is_symlink() or not directory.is_dir():
            raise ProofError("canonical session directory is not a regular owned directory")
        for path in directory.rglob("*"):
            metadata = path.lstat()
            if stat.S_ISLNK(metadata.st_mode):
                raise ProofError("symlink in canonical session tree")
            if stat.S_ISDIR(metadata.st_mode):
                continue
            if not stat.S_ISREG(metadata.st_mode):
                raise ProofError("non-regular canonical session entry")
            result[path.relative_to(store_root).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    if not result:
        raise ProofError("isolated store has no canonical session snapshots")
    return result


def strip_terminal_controls(raw):
    text = raw.decode("utf-8", "replace")
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    text = re.sub(r"\x1b\].*?(?:\x07|\x1b\\)", "", text, flags=re.DOTALL)
    return text.replace("\x1b", "")


def compact(text):
    return "".join(character for character in text if not character.isspace() and character not in BORDERS and character != "\x00")


class VTScreen:
    """Source-owned stdlib VT screen model, reused from the reviewed UM1 draft."""

    def __init__(self, rows, columns):
        self.rows = rows
        self.columns = columns
        self.cells = [[" "] * columns for _ in range(rows)]
        self.row = 0
        self.column = 0
        self.pending_wrap = False
        self.saved_cursor = (0, 0)
        self.state = "text"
        self.sequence = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.scroll_top = 0
        self.scroll_bottom = rows - 1

    def _line_feed(self):
        self.pending_wrap = False
        if self.row >= self.scroll_bottom:
            self.cells.pop(self.scroll_top)
            self.cells.insert(self.scroll_bottom, [" "] * self.columns)
        elif self.row + 1 < self.rows:
            self.row += 1

    def _write(self, character):
        if unicodedata.combining(character):
            if self.column > 0:
                self.cells[self.row][self.column - 1] += character
            return
        if self.pending_wrap:
            self.column = 0
            self._line_feed()
        width = 2 if unicodedata.east_asian_width(character) in ("W", "F") else 1
        self.cells[self.row][self.column] = character
        if width == 2 and self.column + 1 < self.columns:
            self.cells[self.row][self.column + 1] = " "
        self.column += width
        if self.column >= self.columns:
            self.column = self.columns - 1
            self.pending_wrap = True

    def _params(self):
        value = self.sequence.lstrip("?=>!")
        try:
            return [int(part) if part else 0 for part in value.split(";")]
        except ValueError:
            return []

    def _csi(self, final):
        params = self._params()

        def get(index, default=1):
            value = params[index] if index < len(params) else 0
            return value or default

        self.pending_wrap = False
        if final in ("H", "f"):
            self.row = min(self.rows - 1, max(0, get(0) - 1))
            self.column = min(self.columns - 1, max(0, get(1) - 1))
        elif final == "A":
            self.row = max(self.scroll_top, self.row - get(0))
        elif final == "B":
            self.row = min(self.scroll_bottom, self.row + get(0))
        elif final == "C":
            self.column = min(self.columns - 1, self.column + get(0))
        elif final == "D":
            self.column = max(0, self.column - get(0))
        elif final == "G":
            self.column = min(self.columns - 1, max(0, get(0) - 1))
        elif final == "d":
            self.row = min(self.rows - 1, max(0, get(0) - 1))
        elif final == "J":
            mode = params[0] if params else 0
            if mode in (2, 3):
                self.cells = [[" "] * self.columns for _ in range(self.rows)]
            elif mode == 0:
                self.cells[self.row][self.column:] = [" "] * (self.columns - self.column)
                for row in range(self.row + 1, self.rows):
                    self.cells[row] = [" "] * self.columns
            elif mode == 1:
                for row in range(self.row):
                    self.cells[row] = [" "] * self.columns
                self.cells[self.row][: self.column + 1] = [" "] * (self.column + 1)
        elif final == "K":
            mode = params[0] if params else 0
            if mode == 0:
                self.cells[self.row][self.column:] = [" "] * (self.columns - self.column)
            elif mode == 1:
                self.cells[self.row][: self.column + 1] = [" "] * (self.column + 1)
            elif mode == 2:
                self.cells[self.row] = [" "] * self.columns
        elif final == "X":
            count = min(self.columns - self.column, get(0))
            self.cells[self.row][self.column:self.column + count] = [" "] * count
        elif final == "s":
            self.saved_cursor = (self.row, self.column)
        elif final == "u":
            self.row, self.column = self.saved_cursor
        elif final == "r":
            top = max(0, get(0) - 1)
            bottom = min(self.rows - 1, get(1, self.rows))
            if top < bottom:
                self.scroll_top, self.scroll_bottom = top, bottom
                self.row, self.column = top, 0
        elif final == "S":
            for _ in range(get(0)):
                self.cells.pop(self.scroll_top)
                self.cells.insert(self.scroll_bottom, [" "] * self.columns)
        elif final == "T":
            for _ in range(get(0)):
                self.cells.pop(self.scroll_bottom)
                self.cells.insert(self.scroll_top, [" "] * self.columns)
        elif final == "P":
            count = min(self.columns - self.column, get(0))
            row = self.cells[self.row]
            row[self.column:] = row[self.column + count:] + [" "] * count
        elif final == "@":
            count = min(self.columns - self.column, get(0))
            row = self.cells[self.row]
            row[self.column:] = [" "] * count + row[self.column:self.columns - count]

    def feed(self, data):
        for character in self.decoder.decode(data):
            if self.state == "escape":
                if character == "[":
                    self.state = "csi"
                    self.sequence = ""
                elif character in ("]", "P", "^", "_"):
                    self.state = "string"
                elif character == "7":
                    self.saved_cursor = (self.row, self.column)
                    self.state = "text"
                elif character == "8":
                    self.row, self.column = self.saved_cursor
                    self.state = "text"
                else:
                    self.state = "text"
            elif self.state == "csi":
                if "@" <= character <= "~":
                    self._csi(character)
                    self.state = "text"
                else:
                    self.sequence += character
            elif self.state == "string":
                if character == "\x07":
                    self.state = "text"
                elif character == "\x1b":
                    self.state = "string-escape"
            elif self.state == "string-escape":
                self.state = "text" if character == "\\" else "string"
            elif character == "\x1b":
                self.state = "escape"
            elif character == "\r":
                self.column = 0
                self.pending_wrap = False
            elif character == "\n":
                self._line_feed()
            elif character == "\b":
                self.column = max(0, self.column - 1)
                self.pending_wrap = False
            elif character == "\t":
                self.column = min(self.columns - 1, (self.column // 8 + 1) * 8)
                self.pending_wrap = False
            elif ord(character) >= 0x20 and ord(character) != 0x7F:
                self._write(character)

    def visible_lines(self):
        return ["".join(row).rstrip() for row in self.cells]

    def text(self):
        return "\n".join(self.visible_lines())


class TerminalSession:
    """Own one direct TUI child and its controlling pseudo-terminal."""

    def __init__(self, binary, workspace, env, rows, columns):
        self.binary = str(binary)
        self.workspace = str(workspace)
        self.env = dict(env)
        self.rows = rows
        self.columns = columns
        self.master = None
        self.slave = None
        self.pid = None
        self.exit_code = None
        self.initial_termios = None
        self.normal_exit = False
        self.closed = False
        self.recording = bytearray()
        self.screen = VTScreen(rows, columns)

    def start(self):
        self.master, self.slave = pty.openpty()
        self.initial_termios = termios.tcgetattr(self.master)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", self.rows, self.columns, 0, 0))
        pid = os.fork()
        if pid == 0:
            try:
                os.setsid()
                fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0)
                for descriptor in (0, 1, 2):
                    os.dup2(self.slave, descriptor)
                if self.master > 2:
                    os.close(self.master)
                if self.slave > 2:
                    os.close(self.slave)
                os.chdir(self.workspace)
                os.execve(
                    self.binary,
                    [self.binary, "tui", "--project", PROJECT, "--source-namespace", ALPHA_NAMESPACE],
                    self.env,
                )
            except BaseException:
                os._exit(127)
        self.pid = pid
        os.set_blocking(self.master, False)
        try:
            os.kill(pid, signal.SIGWINCH)
        except ProcessLookupError:
            pass

    def _poll_exit(self):
        if self.exit_code is not None or self.pid is None:
            return self.exit_code
        try:
            found, status = os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError:
            return self.exit_code
        if found == self.pid:
            self.exit_code = os.waitstatus_to_exitcode(status)
        return self.exit_code

    def drain(self, timeout=0.05):
        if self.master is None:
            return
        first = True
        while True:
            try:
                ready, _, _ = select.select([self.master], [], [], timeout if first else 0)
            except InterruptedError:
                continue
            first = False
            if not ready:
                return
            try:
                data = os.read(self.master, 65536)
            except BlockingIOError:
                return
            except OSError as error:
                if error.errno == errno.EIO:
                    return
                raise
            if not data:
                return
            if len(self.recording) + len(data) > MAX_PTY_BYTES:
                raise ProofError("TUI PTY output exceeded the bounded 8 MiB capture")
            self.recording.extend(data)
            self.screen.feed(data)

    def wait_for(self, marker, description, timeout=PTY_TIMEOUT):
        if isinstance(marker, bytes):
            marker = marker.decode("utf-8")
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain(0.1)
            if compact(marker) in compact(self.screen.text()):
                return
            if self._poll_exit() is not None:
                raise ProofError(
                    "TUI exited while "
                    + description
                    + "\nterminal output:\n"
                    + self.screen.text()[-6000:]
                )
        raise ProofError(
            "timed out while "
            + description
            + "\nterminal output:\n"
            + self.screen.text()[-6000:]
        )

    def wait_for_output_since(self, offset, description, timeout=5.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain(0.1)
            if len(self.recording) > offset:
                return
            if self._poll_exit() is not None:
                raise ProofError("TUI exited while " + description)
        raise ProofError("timed out while " + description)

    def send(self, data, description):
        remaining = memoryview(data)
        deadline = time.monotonic() + 5.0
        while remaining and time.monotonic() < deadline:
            try:
                written = os.write(self.master, remaining)
                remaining = remaining[written:]
            except BlockingIOError:
                select.select([], [self.master], [], 0.05)
        if remaining:
            raise ProofError("timed out while " + description)

    def wait_exit(self, timeout=PTY_TIMEOUT):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain(0.1)
            status = self._poll_exit()
            if status is not None:
                self.drain(0)
                return status
        raise ProofError("TUI did not exit within its bounded normal-exit interval")

    def finish_normally(self):
        self.send(b"q", "exiting with q outside query input")
        status = self.wait_exit()
        if status != 0:
            raise ProofError("TUI exited with status " + str(status))
        if termios.tcgetattr(self.master) != self.initial_termios:
            raise ProofError("TUI did not restore PTY terminal state before normal exit")
        self.normal_exit = True

    def close(self):
        if self.closed:
            return
        self.closed = True
        if self.pid is not None and self._poll_exit() is None:
            try:
                os.kill(self.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                self.wait_exit(2.0)
            except ProofError:
                try:
                    os.kill(self.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                deadline = time.monotonic() + 2.0
                while self._poll_exit() is None and time.monotonic() < deadline:
                    time.sleep(0.02)
                if self._poll_exit() is None:
                    raise ProofError("owned TUI child could not be reaped")
        if not self.normal_exit and self.master is not None and self.initial_termios is not None:
            try:
                termios.tcsetattr(self.master, termios.TCSANOW, self.initial_termios)
            except (OSError, termios.error):
                pass
        for descriptor in (self.master, self.slave):
            if descriptor is not None:
                try:
                    os.close(descriptor)
                except OSError:
                    pass
        self.master = None
        self.slave = None


def selected_pane_lines(session):
    """Read only the current evidence rectangle, never historical PTY output."""
    for top, line in enumerate(session.screen.visible_lines()):
        title = line.find("Source evidence")
        if title < 0:
            continue
        left = max(line.rfind(corner, 0, title) for corner in "┌╭┏")
        rights = [line.find(corner, title) for corner in "┐╮┓"]
        rights = [column for column in rights if column > left]
        if left < 0 or not rights:
            continue
        right = min(rights)
        for bottom in range(top + 1, session.rows):
            cells = session.screen.cells[bottom]
            if cells[left] in "└╰┗" and cells[right] in "┘╯┛":
                return [
                    "".join(row[left + 1:right]).rstrip()
                    for row in session.screen.cells[top + 1:bottom]
                ]
    raise ProofError("the current PTY frame has no bounded source-evidence pane")


def verify_selected_pane(session, proof, start, columns):
    expected = {
        "Search hit ID:": proof["id"],
        "Project:": proof["project"],
        "Source namespace:": proof["source_namespace"],
        "Native client:": "not applicable" if proof["client"] is None else proof["client"],
        "External session ID:": proof["external_id"],
        "Session ID:": proof["session_id"],
        "Message ID:": proof["message_id"],
        "Path:": proof["path"],
        "Source bytes:": str(proof["span"][0]) + ".." + str(proof["span"][1]),
        "SHA-256:": proof["hash"],
        "JSON pointer:": proof["field"],
        "Decoded-text span:": str(proof["text_span"][0]) + ".." + str(proof["text_span"][1]),
    }
    terminators = tuple(expected) + ("Type:", "Index coverage:")
    seen = set()
    required = set(expected) | {"quote"}
    previous_mismatch = None
    changed_by_scroll = False
    session.wait_for_output_since(start, "waiting for selected evidence focus")
    for _ in range(64):
        session.drain(0.1)
        if any(line.lstrip().startswith("Search hit ID:") for line in selected_pane_lines(session)):
            break
        session.send(b"\x1b[A", "reaching the selected evidence identity before scrolling")
    else:
        raise ProofError("the selected evidence identity did not become reachable in the current pane")
    for _ in range(32):
        session.drain(0.1)
        lines = selected_pane_lines(session)
        displayed = compact("\n".join(lines))
        if compact(proof["text"]) in displayed:
            seen.add("quote")
        mismatch = None
        for row, line in enumerate(lines):
            line = line.lstrip()
            label = next((label for label in expected if line.startswith(label)), None)
            if label is None:
                continue
            parts = [line[len(label):].strip()]
            complete = False
            for continuation in lines[row + 1:]:
                continuation = continuation.strip()
                if not continuation or continuation.startswith(terminators):
                    complete = True
                    break
                parts.append(continuation)
            actual = compact("".join(parts))
            wanted = compact(expected[label])
            if complete:
                if actual != wanted:
                    mismatch = (label, actual, wanted)
                    break
                seen.add(label)
            elif not wanted.startswith(actual):
                mismatch = (label, actual, wanted)
                break
        if mismatch is not None:
            if mismatch == previous_mismatch:
                label, actual, wanted = mismatch
                raise ProofError(
                    "stable current selected pane mismatch for " + label
                    + "\nexpected: " + wanted + "\nactual: " + actual
                    + "\nframe:\n" + session.screen.text()
                )
            previous_mismatch = mismatch
        else:
            previous_mismatch = None
        if mismatch is None and required <= seen and changed_by_scroll:
            return
        before = tuple(lines)
        session.send(b"\x1b[B", "scrolling the selected TUI evidence pane")
        session.drain(0.15)
        changed_by_scroll |= tuple(selected_pane_lines(session)) != before
    raise ProofError(
        "current selected TUI frames did not verify all source fields at "
        + str(columns) + " columns\nmissing: " + repr(sorted(required - seen))
        + "\nframe:\n" + session.screen.text()
    )


def run_width(binary, workspace, home, empty_path, env, rows, columns, session_order, alpha_id, proof, hit_title, canonical_before, source_paths):
    session = TerminalSession(binary, workspace, env, rows, columns)
    try:
        session.start()
        session.wait_for(b"Raw sessions", "waiting for global session browsing")
        session.wait_for("All projects · 4 sessions", "waiting for the global session page to load")
        alpha_index = next(index for index, item in enumerate(session_order) if item.get("id") == alpha_id)
        gamma_index = next(index for index, item in enumerate(session_order) if item.get("title") == GAMMA_TITLE)
        current_index = 0
        if gamma_index > current_index:
            for _ in range(gamma_index - current_index):
                session.send(b"\x1b[B", "selecting the other-project session globally")
                session.drain(0.08)
            current_index = gamma_index
        elif gamma_index < current_index:
            for _ in range(current_index - gamma_index):
                session.send(b"\x1b[A", "selecting the other-project session globally")
                session.drain(0.08)
            current_index = gamma_index
        session.wait_for(GAMMA_TITLE, "confirming other-project browsing remains global")
        if alpha_index > current_index:
            for _ in range(alpha_index - current_index):
                session.send(b"\x1b[B", "selecting the alpha transcript")
                session.drain(0.08)
        elif alpha_index < current_index:
            for _ in range(current_index - alpha_index):
                session.send(b"\x1b[A", "selecting the alpha transcript")
                session.drain(0.08)
        session.wait_for(ALPHA_TITLE, "selecting the actual alpha session")
        session.send(b"\r", "opening the selected raw transcript")
        session.wait_for(b"Raw transcript", "opening transcript pane")
        session.wait_for(alpha_id, "rendering Engine session identity")
        session.wait_for(USER_HEAD, "rendering the start of the wrapped user message")
        for _ in range(180):
            if compact(USER_TAIL) in compact(session.screen.text()):
                break
            session.send(b"\x1b[B", "scrolling the actual wrapped transcript")
            session.drain(0.08)
        else:
            raise ProofError("wrapped user transcript tail did not become visible")

        session.send(b"\x1b[C\x1b[C", "selecting the tool transcript message")
        for _ in range(180):
            if compact("Tool body collapsed") in compact(session.screen.text()):
                break
            session.send(b"\x1b[B", "scrolling to the collapsed tool message")
            session.drain(0.08)
        else:
            raise ProofError("tool message did not render in its collapsed state")
        session.send(b"x", "expanding the selected tool output")
        for _ in range(180):
            session.drain(0.08)
            if compact(TOOL_HEAD) in compact(session.screen.text()):
                break
            session.send(b"\x1b[A", "reaching the expanded tool head from its retained scroll anchor")
        else:
            raise ProofError("expanded tool output head did not become reachable")
        for _ in range(180):
            if compact(TOOL_TAIL) in compact(session.screen.text()):
                break
            session.send(b"\x1b[B", "scrolling the expanded tool output")
            session.drain(0.08)
        else:
            raise ProofError("expanded tool output tail did not become visible")
        for forbidden in (
            b"\x1b]52;c;" + OSC_MARKER.encode("ascii"),
            b"\x1b" + ESC_MARKER.encode("ascii"),
            b"\x9d52;c;" + OSC_MARKER.encode("ascii"),
        ):
            if forbidden in session.recording:
                raise ProofError("source terminal-control sequence reached raw PTY output")
        if (workspace / COMMAND_CANARY).exists():
            raise ProofError("archived tool command canary executed during transcript browsing")

        return_start = len(session.recording)
        session.send(b"\x1b", "returning from transcript to global sessions")
        session.wait_for_output_since(return_start, "waiting for the session-list redraw")
        search_start = len(session.recording)
        session.send(b"/", "opening existing TUI search keys")
        session.send(CITATION_TEXT.encode("ascii"), "typing the scoped citation query")
        session.send(b"\r", "submitting the scoped citation query")
        deadline = time.monotonic() + PTY_TIMEOUT
        while time.monotonic() < deadline:
            session.drain(0.1)
            if compact(hit_title) in compact(session.screen.text()):
                break
            if session._poll_exit() is not None:
                raise ProofError("TUI exited while waiting for the filtered search result")
        else:
            raise ProofError("filtered TUI search did not render its selected namespace hit")
        selected_start = len(session.recording)
        session.send(b"\r", "selecting the default first TUI search hit")
        session.wait_for(b"Source evidence", "opening selected source evidence")
        verify_selected_pane(session, proof, selected_start, columns)
        if any(empty_path.iterdir()):
            raise ProofError("owned PATH gained an unexpected native command wrapper")
        if (workspace / COMMAND_CANARY).exists():
            raise ProofError("archived tool command canary executed during search")
        return_start = len(session.recording)
        session.send(b"\x1b", "returning from evidence to sessions")
        session.wait_for_output_since(return_start, "waiting for the session-list redraw")
        session.finish_normally()
        if canonical_session_hashes(home / "store") != canonical_before:
            raise ProofError("canonical source bytes changed while reading or searching in TUI")
        if any(
            hashlib.sha256(path.read_bytes()).hexdigest() != digest
            for path, digest in source_paths.items()
        ):
            raise ProofError("normalized source input changed while browsing memory")
        if (workspace / COMMAND_CANARY).exists():
            raise ProofError("archived tool command canary executed before normal TUI exit")
    finally:
        session.close()


def run_smoke(binary):
    binary = Path(binary).resolve(strict=True)
    metadata = binary.stat()
    if not stat.S_ISREG(metadata.st_mode) or not os.access(binary, os.X_OK):
        raise ProofError("--binary must name an executable regular file")
    with tempfile.TemporaryDirectory(prefix="noesora-um1-tui-smoke-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        paths, env = isolated_environment(root)
        alpha_path = write_alpha(root / "alpha.jsonl")
        decoy_path = write_alpha_decoy(root / "alpha-decoy.jsonl")
        beta_path = write_beta(root / "beta.jsonl")
        gamma_path = write_gamma(root / "gamma.jsonl")
        initialized = run_cli(binary, ["init"], paths["workspace"], env)
        store_root = Path(initialized["root"]).resolve(strict=True)
        if not is_within(paths["home"].resolve(strict=True), store_root):
            raise ProofError("isolated store escaped its synthetic home")
        reports = [
            run_cli(binary, ["session", "ingest", str(path)], paths["workspace"], env)
            for path in (alpha_path, decoy_path, beta_path, gamma_path)
        ]
        alpha, decoy, beta, gamma = reports
        for report, namespace, external_id, project in (
            (alpha, ALPHA_NAMESPACE, EXTERNAL_SESSION_ID, PROJECT),
            (decoy, ALPHA_NAMESPACE, "alpha-decoy-session", PROJECT),
            (beta, BETA_NAMESPACE, EXTERNAL_SESSION_ID, PROJECT),
            (gamma, "fictional.ide-gamma", "gamma-global-session", OTHER_PROJECT),
        ):
            if (
                report.get("source_namespace") != namespace
                or report.get("external_id") != external_id
                or report.get("project") != project
                or not report.get("session_id", "").startswith("mem:")
            ):
                raise ProofError("neutral ingest did not preserve opaque identity and namespace")

        listing = run_cli(binary, ["session", "list"], paths["workspace"], env)
        session_order = listing.get("sessions")
        if not isinstance(session_order, list) or len(session_order) != 4:
            raise ProofError("global session list did not include every neutral fixture")
        if not any(item.get("id") == alpha["session_id"] for item in session_order):
            raise ProofError("global session list omitted alpha fixture")
        if not any(
            item.get("title") == GAMMA_TITLE and item.get("project") == OTHER_PROJECT
            for item in session_order
        ):
            raise ProofError("global session list omitted other-project fixture")
        for report in (alpha, beta):
            summary = next(item for item in session_order if item.get("id") == report["session_id"])
            if any(field not in summary for field in ("client", "created_at", "updated_at")):
                raise ProofError("neutral summary omitted nullable provenance/date fields")
            if any(summary[field] is not None for field in ("client", "created_at", "updated_at")):
                raise ProofError("neutral summary invented native client or source dates")

        shown = run_cli(binary, ["session", "show", alpha["session_id"]], paths["workspace"], env)
        page = shown.get("session")
        messages = shown.get("messages")
        if not isinstance(page, dict) or page.get("id") != alpha["session_id"] or not isinstance(messages, list):
            raise ProofError("CLI did not return the ingested neutral transcript")
        if any(page.get(field) is not None for field in ("client", "created_at", "updated_at")):
            raise ProofError("CLI transcript invented native client or source dates")
        if any("external_id" in message or message.get("timestamp") is not None for message in messages):
            raise ProofError("CLI transcript leaked external message IDs or invented timestamps")
        assistant = next(
            (message for message in messages if CITATION_TEXT in message.get("text", "")),
            None,
        )
        if assistant is None or not assistant.get("id", "").startswith("mem:"):
            raise ProofError("CLI transcript omitted Engine-owned citation message identity")

        query = CITATION_TEXT
        search = run_cli(
            binary,
            ["search", query, "--project", PROJECT, "--source-namespace", ALPHA_NAMESPACE],
            paths["workspace"],
            env,
        )
        hits = search.get("hits")
        if not isinstance(hits, list) or len(hits) != 2:
            raise ProofError("CLI namespace filter did not select both alpha citations")
        cli_hit = hits[0]
        cli_proof = verify_hit(cli_hit, store_root)
        if cli_proof["session_id"] not in (alpha["session_id"], decoy["session_id"]):
            raise ProofError("CLI selected a hit outside the alpha namespace")

        mcp_search = run_mcp_search(binary, paths["workspace"], env, query)
        mcp_hits = mcp_search.get("hits")
        if not isinstance(mcp_hits, list) or len(mcp_hits) != 2:
            raise ProofError("MCP namespace filter did not select both alpha citations")
        mcp_proof = verify_hit(mcp_hits[0], store_root)
        for field in (
            "session_id",
            "message_id",
            "path",
            "hash",
            "span",
            "field",
            "text_span",
            "text",
            "client",
            "source_namespace",
            "external_id",
        ):
            if mcp_proof[field] != cli_proof[field]:
                raise ProofError("source-verified CLI/MCP evidence differed for " + field)

        status = run_cli(binary, ["status"], paths["workspace"], env)
        if status.get("records") != 0:
            raise ProofError("neutral ingest created promoted knowledge or an approved record")
        canonical_before = canonical_session_hashes(store_root)
        source_paths = (alpha_path, decoy_path, beta_path, gamma_path)
        source_hashes = {
            path: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in source_paths
        }
        if any(paths["empty"].iterdir()):
            raise ProofError("owned PATH was not empty before direct TUI launch")

        for rows, columns in ((26, 72), (26, 120)):
            run_width(
                binary,
                paths["workspace"],
                paths["home"],
                paths["empty"],
                env,
                rows,
                columns,
                session_order,
                alpha["session_id"],
                cli_proof,
                cli_hit.get("title", ALPHA_TITLE),
                canonical_before,
                source_hashes,
            )

        if canonical_session_hashes(store_root) != canonical_before:
            raise ProofError("canonical session bytes changed during direct TUI smoke")
        if any(
            hashlib.sha256(path.read_bytes()).hexdigest() != digest
            for path, digest in source_hashes.items()
        ):
            raise ProofError("neutral source input changed during direct TUI smoke")
        if (paths["workspace"] / COMMAND_CANARY).exists():
            raise ProofError("archived tool command canary executed")
        print(
            "Direct 72/120-column memory PTY passed: global transcript browsing, wrapped and expanded tool output, "
            "archived-command canary, selected neutral search evidence, and matching source-verified CLI/MCP IDs, "
            "paths, hashes, byte spans, pointers, decoded spans, and quote."
        )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, help="absolute path to the supplied Noesora CLI binary")
    args = parser.parse_args(argv)
    if os.name != "posix" or sys.platform not in ("linux", "darwin"):
        raise ProofError("the direct memory PTY smoke supports POSIX Linux and macOS only")
    if not Path(args.binary).is_absolute():
        raise ProofError("--binary must name an absolute executable file")
    run_smoke(args.binary)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except ProofError as error:
        print("TUI smoke failed: " + str(error), file=sys.stderr)
        sys.exit(1)
    except Exception as error:
        print("TUI smoke failed unexpectedly: " + type(error).__name__ + ": " + str(error), file=sys.stderr)
        sys.exit(1)
