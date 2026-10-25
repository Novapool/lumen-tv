"""Tests for the pure state machine in session/lumen-airplay (no GTK needed)."""
import importlib.machinery
import importlib.util
import os
import unittest

PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "lumen-airplay")
loader = importlib.machinery.SourceFileLoader("lumen_airplay", PATH)
spec = importlib.util.spec_from_loader("lumen_airplay", loader)
mod = importlib.util.module_from_spec(spec)
loader.exec_module(mod)

START = "raop_rtp_mirror starting mirroring\n"
STOP = "raop_rtp_mirror->running is no longer true\n"


class WatchTests(unittest.TestCase):
    def test_idle_timeout(self):
        w = mod.Watch(0)
        self.assertIsNone(w.tick(mod.IDLE_TIMEOUT))
        self.assertEqual(w.tick(mod.IDLE_TIMEOUT + 1), "idle")

    def test_any_line_resets_idle_clock(self):
        w = mod.Watch(0)
        w.feed("some unrelated line\n", 100)
        self.assertIsNone(w.tick(100 + mod.IDLE_TIMEOUT))
        self.assertEqual(w.tick(100 + mod.IDLE_TIMEOUT + 1), "idle")

    def test_mirroring_never_idles(self):
        w = mod.Watch(0)
        w.feed(START, 1)
        self.assertIsNone(w.tick(10_000))
        self.assertIsNone(w.seconds_left(10_000))

    def test_grace_exit(self):
        w = mod.Watch(0)
        w.feed(START, 1)
        w.feed(STOP, 50)
        self.assertIsNone(w.tick(50 + mod.GRACE))
        self.assertEqual(w.tick(50 + mod.GRACE + 1), "stopped")

    def test_reconnect_within_grace_cancels(self):
        w = mod.Watch(0)
        w.feed(START, 1)
        w.feed(STOP, 50)
        w.feed(START, 53)
        self.assertIsNone(w.tick(60))
        self.assertTrue(w.mirroring)

    def test_no_idle_exit_during_grace(self):
        # long-quiet session: grace decides, not the idle clock
        w = mod.Watch(0)
        w.feed(START, 1)
        w.feed(STOP, 500)
        self.assertIsNone(w.tick(503))

    def test_start_clears_idle_path(self):
        w = mod.Watch(0)
        w.feed(START, 100)
        self.assertIsNone(w.tick(100 + mod.IDLE_TIMEOUT * 3))

    def test_seconds_left(self):
        w = mod.Watch(0)
        self.assertEqual(w.seconds_left(0), mod.IDLE_TIMEOUT)
        self.assertEqual(w.seconds_left(30.5), mod.IDLE_TIMEOUT - 30)
        self.assertEqual(w.seconds_left(1000), 0)

    def test_split_lines_two_in_one_chunk_and_partial_carry(self):
        lines, rest = mod.split_lines(b"", b"a\nb\nc")
        self.assertEqual(lines, ["a\n", "b\n"])
        self.assertEqual(rest, b"c")
        lines, rest = mod.split_lines(rest, b"d\n")
        self.assertEqual(lines, ["cd\n"])
        self.assertEqual(rest, b"")

    def test_split_lines_bad_utf8(self):
        lines, _ = mod.split_lines(b"", b"x\xff\n")
        self.assertEqual(len(lines), 1)

    def test_command_shape(self):
        self.assertIn("waylandsink fullscreen=true", mod.CMD)
        self.assertIn("pulsesink", mod.CMD)


if __name__ == "__main__":
    unittest.main()
