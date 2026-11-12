"""Sanity checks for attribution and accounting, without launching a window."""
import tempfile
import unittest
from pathlib import Path

from profile_ui_memory import category, read_profile, smaps_summary


class HeapAccountingTests(unittest.TestCase):
    def test_live_and_cumulative_bytes_are_distinct(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "sample.heap"
            path.write_text("heap profile: 1: 40 [3: 120] @ heapprofile\n"
                            " 1: 40 [1: 40] @ 0x1001\n"
                            " 0: 0 [2: 80] @ 0x2001\nMAPPED_LIBRARIES:\n")
            entries, _ = read_profile(path)
            self.assertEqual(entries, [((1, 40, 1, 40), (0x1000,)),
                                       ((0, 0, 2, 80), (0x2000,))])
            path.write_text(path.read_text().replace("1: 40 [3: 120]", "1: 40 [3: 121]"))
            with self.assertRaises(ValueError):
                read_profile(path)

    def test_categories_do_not_charge_shared_atlas_to_svg(self):
        atlas = "gpui::platform::blade::blade_atlas::BladeAtlas"
        self.assertEqual(category([atlas, "paint_svg"]), "shared_rendering")
        self.assertEqual(category(["gpui::svg_renderer::render_pixmap", atlas]), "svg")
        self.assertEqual(category(["cosmic_text::FontSystem", atlas]), "fonts_text")
        self.assertEqual(category(["gpui::text_system::layout", "taffy::layout"]), "fonts_text")
        self.assertEqual(category(["taffy::layout"]), "layout_elements")
        self.assertEqual(category(["<unresolved>"]), "other")

    def test_font_mapping_size_is_not_resident_memory(self):
        text = ("1000-2000 r--p 0 00:00 1 /fonts/example.ttc\n"
                "Size: 20000 kB\nRss: 4000 kB\nPss: 1000 kB\n"
                "2000-3000 rw-p 0 00:00 0 [heap]\nRss: 5000 kB\n")
        self.assertEqual(smaps_summary(text), {"Size_KiB": 20000, "Rss_KiB": 4000, "Pss_KiB": 1000})


if __name__ == "__main__":
    unittest.main()
