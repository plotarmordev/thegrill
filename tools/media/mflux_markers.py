#!/usr/bin/env python3
"""Forward a console script in-process with operator-selected phase hooks."""
import argparse
import functools
import importlib
import importlib.metadata
import json
import os
import sys
import time


def install_hook(spec, stamp):
    event, target = spec.split("=", 1)
    when, module, qualname = target.split(":", 2)
    if event not in {"model_loaded", "generation_started", "generation_finished"}:
        raise ValueError("unsupported hook event")
    if when not in {"before", "after"}:
        raise ValueError("hook timing must be before or after")
    owner = importlib.import_module(module)
    parts = qualname.split(".")
    for part in parts[:-1]:
        owner = getattr(owner, part)
    original = getattr(owner, parts[-1])
    if not callable(original):
        raise ValueError("hook target is not callable")
    called = False

    @functools.wraps(original)
    def wrapped(*args, **kwargs):
        nonlocal called
        first = not called
        called = True  # Reserve before invocation so recursion cannot stamp twice.
        if first and when == "before":
            stamp(event)
        result = original(*args, **kwargs)
        if first and when == "after":
            stamp(event)
        return result

    setattr(owner, parts[-1], wrapped)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--grill-marker-file")
    parser.add_argument("--grill-entry", required=True)
    parser.add_argument("--grill-output-flag")
    parser.add_argument("--grill-hook", action="append", default=[])
    argv = sys.argv[1:]
    split = argv.index("--") if "--" in argv else len(argv)
    options = parser.parse_args(argv[:split])
    rest = argv[split + 1:]
    output = None
    if options.grill_output_flag:
        positions = [i for i, arg in enumerate(rest) if arg == options.grill_output_flag]
        if len(positions) != 1 or positions[0] + 1 == len(rest):
            parser.error("output flag must occur exactly once with a following value")
        output = rest[positions[0] + 1]
    events = [hook.split("=", 1)[0] for hook in options.grill_hook]
    if len(set(events)) != len(events):
        parser.error("duplicate hook event")
    fd = None
    try:
        if options.grill_marker_file:
            try:
                fd = os.open(options.grill_marker_file,
                             os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_APPEND, 0o644)
            except OSError as error:
                parser.error(str(error))

        def stamp(event):
            if fd is not None:
                ns = time.monotonic_ns()
                line = json.dumps({"grill_media": 1, "event": event, "monotonic_ns": ns}) + "\n"
                data = line.encode("utf-8")
                if os.write(fd, data) != len(data):
                    raise OSError("short marker write")

        stamp("runner_started")
        entries = list(importlib.metadata.entry_points(
            group="console_scripts", name=options.grill_entry))
        if len(entries) != 1:
            parser.error("console entry must resolve uniquely")
        entry = entries[0].load()
        try:
            for hook in options.grill_hook:
                install_hook(hook, stamp)
        except (ValueError, AttributeError, ImportError) as error:
            parser.error(str(error))
        sys.argv = [options.grill_entry, *rest]
        try:
            code = entry()
        except SystemExit as error:
            code = error.code
        if output is not None:
            try:
                os.stat(output)
            except OSError:
                pass
            else:
                stamp("output_written")
        return code
    finally:
        if fd is not None:
            os.close(fd)


if __name__ == "__main__":
    sys.exit(main())
