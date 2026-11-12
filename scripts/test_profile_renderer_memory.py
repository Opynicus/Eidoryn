"""Accounting checks without a display or GPU."""
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from profile_renderer_memory import (PHASE, backend_states, backend_transitions,
                                     dgop_sample, drm_clients, mappings, phase_identity)


class RendererAccountingTests(unittest.TestCase):
    def test_repeated_mappings_keep_residency_separate_from_size(self):
        data = ("1000-2000 r--p 0 00:00 1 /driver.so\nSize: 100 kB\nRss: 4 kB\nPss: 2 kB\n"
                "2000-3000 r-xp 0 00:00 1 /driver.so\nSize: 200 kB\nRss: 8 kB\nPss: 4 kB\n"
                "3000-4000 rw-p 0 00:00 0\nSize: 4 kB\nRss: 4 kB\nPrivate_Dirty: 4 kB\n")
        result = mappings(data)
        self.assertEqual(result["/driver.so"]["Size"], 300)
        self.assertEqual(result["/driver.so"]["Rss"], 12)
        self.assertEqual(result["/driver.so"]["Pss"], 6)
        self.assertEqual(result["[anonymous]"]["Private_Dirty"], 4)

    def test_duplicate_fds_count_once_but_different_devices_do_not(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            record = "drm-driver: example\ndrm-client-id: 7\ndrm-pdev: GPU0\ndrm-resident-vram: 4 MiB\n"
            (folder / "1").write_text(record)
            (folder / "2").write_text(record)
            (folder / "3").write_text(record.replace("GPU0", "GPU1"))
            (folder / "4").write_text("pos: 0\nflags: 02\n")
            self.assertEqual(len(drm_clients(folder)), 2)

    def test_failed_or_paused_renderer_is_not_a_successful_sample(self):
        record = "renderer-probe revision=3 renderer=Vulkan running=true instructions=99 frames=5 error=None"
        self.assertIsNotNone(PHASE.fullmatch(record))
        self.assertIsNone(PHASE.fullmatch(record.replace("true", "false")))
        self.assertIsNone(PHASE.fullmatch(record.replace("None", 'Some("failure")')))

    def test_cpu_transitions_are_observed_without_a_gpu_revision_change(self):
        line = ("backend-probe seconds=1.000 cpu=Interpreter renderer=Serial revision=0 "
                "bios=false disc=false running=true halted=false instructions=99 frames=1 "
                "blocks=0 native=0 error=None\n")
        jit = line.replace("cpu=Interpreter", "cpu=Jit").replace("blocks=0", "blocks=7")
        states = backend_states(line + jit + jit + line + "backend-probe seconds=")
        self.assertEqual(backend_transitions(states),
                         [("Interpreter", "Serial"), ("Jit", "Serial"), ("Interpreter", "Serial")])
        self.assertEqual(states[1]["blocks"], 7)
        self.assertTrue(all(state["healthy"] for state in states))
        self.assertFalse(backend_states(line.replace("None", 'Some("failure")'))[0]["healthy"])
        self.assertNotEqual(phase_identity(line, "jit-first"), phase_identity(line + jit, "jit-first"))
        # A periodic update with the same acknowledged mode is still the same phase.
        self.assertEqual(phase_identity(line + jit, "jit-first"), phase_identity(line + jit + jit, "jit-first"))

    def test_monitor_display_value_is_distinct_from_rss_and_other_processes_are_omitted(self):
        result = SimpleNamespace(stdout='{"processes": ['
                                 '{"pid": 1, "fullCommand": "unrelated"},'
                                 '{"pid": 2, "memoryKB": 40000, "memoryCalculation": "pss_dirty",'
                                 '"rssKB": 103000, "pssKB": 0, "fullCommand": "not recorded"}]}')
        with patch("profile_renderer_memory.subprocess.run", return_value=result):
            self.assertEqual(dgop_sample(2), {"memoryKB": 40000, "memoryCalculation": "pss_dirty",
                                              "rssKB": 103000, "pssKB": 0})
            self.assertIsNone(dgop_sample(3))


if __name__ == "__main__":
    unittest.main()
