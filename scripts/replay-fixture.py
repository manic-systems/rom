#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Replay a ROM JSONL fixture through the real binary."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import termios
import time
import tty


ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "rom" / "tests" / "fixtures"


def fixture_path(value: str) -> Path:
    candidate = Path(value)
    if not candidate.exists():
        candidate = FIXTURES / value
    events = candidate / "events.jsonl" if candidate.is_dir() else candidate
    if not events.is_file():
        raise argparse.ArgumentTypeError(f"fixture not found: {value}")
    return events


def events(path: Path) -> list[dict[str, object]]:
    result: list[dict[str, object]] = []
    previous = 0
    for line_number, line in enumerate(path.read_text().splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError as error:
            raise SystemExit(f"{path}:{line_number}: {error}") from error
        timestamp = event.get("at_ms")
        if not isinstance(timestamp, int) or timestamp < previous:
            raise SystemExit(f"{path}:{line_number}: timestamps must be monotonic integers")
        previous = timestamp
        result.append(event)
    return result


def wait_for_key() -> None:
    with open("/dev/tty", "rb", buffering=0) as terminal:
        descriptor = terminal.fileno()
        previous = termios.tcgetattr(descriptor)
        try:
            tty.setcbreak(descriptor)
            terminal.read(1)
        finally:
            termios.tcsetattr(descriptor, termios.TCSADRAIN, previous)


def payload(event: dict[str, object]) -> bytes:
    text = event.get("text")
    raw = event.get("bytes")
    if isinstance(text, str) and raw is None:
        return text.encode()
    if text is None and isinstance(raw, list) and all(isinstance(byte, int) for byte in raw):
        return bytes(raw)
    raise SystemExit("write event needs exactly one of text or bytes")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=fixture_path)
    parser.add_argument("--rom", type=Path, default=ROOT / "target" / "debug" / "rom")
    parser.add_argument("--speed", type=float, default=1.0)
    parser.add_argument("--instant", action="store_true")
    parser.add_argument("--step", action="store_true")
    raw_arguments = sys.argv[1:]
    if "--" in raw_arguments:
        separator = raw_arguments.index("--")
        rom_args = raw_arguments[separator + 1 :]
        raw_arguments = raw_arguments[:separator]
    else:
        rom_args = []
    arguments = parser.parse_args(raw_arguments)

    if arguments.speed <= 0:
        parser.error("--speed must be greater than zero")
    if not arguments.rom.is_file():
        raise SystemExit(
            f"ROM binary not found at {arguments.rom}\n"
            "Build it first with: cargo build -p rom"
        )

    fixture_events = events(arguments.fixture)
    if arguments.step:
        checkpoints = [str(event["name"]) for event in fixture_events if event["type"] == "checkpoint"]
        print(
            "Step mode: press any key at each checkpoint: " + ", ".join(checkpoints),
            file=sys.stderr,
        )

    process = subprocess.Popen(
        [os.fspath(arguments.rom), *rom_args],
        stdin=subprocess.PIPE,
        stdout=None,
        stderr=None,
    )
    assert process.stdin is not None
    started = time.monotonic()
    closed = False
    for event in fixture_events:
        target = int(event["at_ms"]) / 1000 / arguments.speed
        if not arguments.instant:
            time.sleep(max(0.0, target - (time.monotonic() - started)))
        kind = event["type"]
        if kind == "write":
            if closed:
                raise SystemExit("write event follows eof")
            process.stdin.write(payload(event))
            process.stdin.flush()
        elif kind == "checkpoint" and arguments.step:
            wait_for_key()
        elif kind == "eof" and not closed:
            process.stdin.close()
            closed = True
    if not closed:
        process.stdin.close()
    return process.wait()


if __name__ == "__main__":
    raise SystemExit(main())
