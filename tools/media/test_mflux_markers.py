"""Synthetic checks for the in-process marker adapter."""
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("mflux_markers", Path(__file__).with_name("mflux_markers.py"))
adapter = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(adapter)


class AdapterTests(unittest.TestCase):
    def check_method_binding(self, kind):
        class Engine:
            @staticmethod
            def static(value):
                return value

            @classmethod
            def class_method(cls, value):
                return cls, value

            def plain(self, value):
                return self, value

        class Child(Engine):
            pass

        module = types.ModuleType("synthetic_hook")
        module.Engine = Engine
        module.Child = Child
        events = []
        with patch.dict(sys.modules, synthetic_hook=module):
            for event, when in [("generation_started", "before"), ("generation_finished", "after")]:
                adapter.install_hook(f"{event}={when}:synthetic_hook:Engine.{kind}", events.append)
            for owner in [Engine, Child]:
                instance = owner()
                if kind == "static":
                    self.assertEqual(getattr(owner, kind)("class"), "class")
                    self.assertEqual(getattr(instance, kind)("instance"), "instance")
                    self.assertIsInstance(Engine.__dict__[kind], staticmethod)
                elif kind == "class_method":
                    self.assertEqual(getattr(owner, kind)("class"), (owner, "class"))
                    self.assertEqual(getattr(instance, kind)("instance"), (owner, "instance"))
                    self.assertIsInstance(Engine.__dict__[kind], classmethod)
                else:
                    self.assertEqual(getattr(owner, kind)(instance, "class"), (instance, "class"))
                    self.assertEqual(getattr(instance, kind)("instance"), (instance, "instance"))
        self.assertEqual(events, ["generation_started", "generation_finished"])

    def test_staticmethod_binding(self):
        self.check_method_binding("static")

    def test_classmethod_subclass_binding(self):
        self.check_method_binding("class_method")

    def test_plain_method_binding(self):
        self.check_method_binding("plain")

    def test_reentrant_hooks_stamp_once(self):
        module = types.ModuleType("synthetic_hook")
        events = []

        def call(depth):
            if depth:
                module.call(depth - 1)

        module.call = call
        with patch.dict(sys.modules, synthetic_hook=module):
            adapter.install_hook("generation_started=before:synthetic_hook:call", events.append)
            adapter.install_hook("generation_finished=after:synthetic_hook:call", events.append)
            module.call(2)
            module.call(0)
        self.assertEqual(events, ["generation_started", "generation_finished"])

    def test_failed_call_does_not_stamp_finished(self):
        module = types.ModuleType("synthetic_hook")
        events = []

        def call():
            raise RuntimeError("fixture")

        module.call = call
        with patch.dict(sys.modules, synthetic_hook=module):
            adapter.install_hook("generation_finished=after:synthetic_hook:call", events.append)
            with self.assertRaises(RuntimeError):
                module.call()
        self.assertEqual(events, [])

    def invoke_error(self, args):
        with patch.object(sys, "argv", ["adapter", *args]), contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                adapter.main()
        self.assertEqual(error.exception.code, 2)

    def test_output_flag_must_be_unique_and_have_value(self):
        for args in [[], ["--output"], ["--output", "a", "--output", "b"]]:
            self.invoke_error(["--grill-entry", "fixture", "--grill-output-flag=--output", "--", *args])

    def test_existing_marker_is_not_modified(self):
        with tempfile.TemporaryDirectory() as root:
            path = os.path.join(root, "markers.ndjson")
            Path(path).write_bytes(b"retained")
            self.invoke_error(["--grill-entry", "fixture", "--grill-marker-file", path])
            self.assertEqual(Path(path).read_bytes(), b"retained")

    def test_unknown_entry_exits_as_usage_error(self):
        with patch.object(adapter.importlib.metadata, "entry_points", return_value=[]):
            self.invoke_error(["--grill-entry", "fixture"])


if __name__ == "__main__":
    unittest.main()
