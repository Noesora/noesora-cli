import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "scripts" / "tui-smoke.py"
SPEC = importlib.util.spec_from_file_location("noesora_tui_smoke", SCRIPT)
TUI_SMOKE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TUI_SMOKE)


class Screen:
    def __init__(self, rows):
        self.rows = rows
        self.cells = [list(row) for row in rows]

    def visible_lines(self):
        return self.rows

    def text(self):
        return "\n".join(self.rows)


class ScriptedSession:
    def __init__(self, frames):
        self.frames = frames
        self.frame_index = -1
        self.screen = Screen(frames[0])
        self.rows = len(frames[0])
        self.scrolls = 0
        self.recording = bytearray(b"selected")

    def wait_for_output_since(self, _offset, _description):
        return None

    def drain(self, _timeout=0.05):
        self.frame_index = min(self.frame_index + 1, len(self.frames) - 1)
        self.screen = Screen(self.frames[self.frame_index])

    def send(self, _data, _description):
        self.scrolls += 1


def evidence_frame(hash_value):
    values = [
        ("Search hit ID:", "mem:session-id"),
        ("Project:", "/projects/alpha"),
        ("Source namespace:", "fictional.ide-alpha"),
        ("Native client:", "not applicable"),
        ("External session ID:", "opaque-external-id"),
        ("Session ID:", "mem:session-id"),
        ("Message ID:", "mem:message-id"),
        ("Path:", "sessions/snapshots/neutral.jsonl"),
        ("Source bytes:", "12..34"),
        ("SHA-256:", hash_value),
        ("JSON pointer:", "/payload/text"),
        ("Decoded-text span:", "0..18"),
        ("", ""),
        ("", "violet lantern citation"),
    ]
    width = 180
    title = "┌ Source evidence"
    rows = [title + "─" * (width - len(title) - 1) + "┐"]
    rows.extend(
        "│" + f"{label} {value}".ljust(width - 2) + "│"
        for label, value in values
    )
    rows.append("└" + "─" * (width - 2) + "┘")
    return rows + [" " * width] * 2


def proof(hash_value):
    return {
        "id": "mem:session-id",
        "project": "/projects/alpha",
        "source_namespace": "fictional.ide-alpha",
        "client": None,
        "external_id": "opaque-external-id",
        "session_id": "mem:session-id",
        "message_id": "mem:message-id",
        "path": "sessions/snapshots/neutral.jsonl",
        "span": [12, 34],
        "hash": hash_value,
        "field": "/payload/text",
        "text_span": [0, 18],
        "text": "violet lantern citation",
    }


class SelectedPaneTransitionTests(unittest.TestCase):
    def test_v46_transient_hash_row_is_rechecked_after_redraw(self):
        expected = "sha256:" + "a" * 64
        transient = "sha256:" + "a" * 16
        session = ScriptedSession([
            evidence_frame(expected),
            evidence_frame(transient),
            evidence_frame(expected),
            evidence_frame(expected),
        ])

        TUI_SMOKE.verify_selected_pane(session, proof(expected), 0, 120)

        self.assertGreater(session.scrolls, 0)
        self.assertEqual(
            "".join(session.screen.cells[10]).strip(),
            "│SHA-256: " + expected + " " * (180 - len("│SHA-256: " + expected) - 1) + "│",
        )

    def test_v46_stable_wrong_hash_still_fails(self):
        expected = "sha256:" + "a" * 64
        wrong = "sha256:" + "b" * 64
        session = ScriptedSession([evidence_frame(wrong)] * 4)

        with self.assertRaisesRegex(TUI_SMOKE.ProofError, "SHA-256"):
            TUI_SMOKE.verify_selected_pane(session, proof(expected), 0, 120)


if __name__ == "__main__":
    unittest.main()
