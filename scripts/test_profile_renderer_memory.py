"""Accounting checks without a display or GPU."""
import tempfile
import unittest
from pathlib import Path

from profile_renderer_memory import PHASE, drm_clients, mappings


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


if __name__ == "__main__":
    unittest.main()
