"""Check the distributable archive and checksum for both desktop platforms."""
import hashlib
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile


ROOT = pathlib.Path(__file__).resolve().parent.parent


class ReleasePackageTest(unittest.TestCase):
    def test_desktop_archives_include_only_runtime_files(self):
        binary_bytes = b'Eidoryn packaging fixture\x00\xff'
        theme = 'plugins/themes/ember.json'
        notices = ('licenses/gpui/LICENSE-APACHE', 'licenses/gpui/SOURCE')
        with tempfile.TemporaryDirectory() as temporary:
            work = pathlib.Path(temporary)
            # Adjacent build inputs and local media must not enter the package.
            (work / 'bios.bin').write_bytes(b'not distributable')
            (work / 'card1.mcd').write_bytes(b'private save')
            for binary_name, extension in [('eidoryn', '.tar.gz'), ('eidoryn.exe', '.zip')]:
                with self.subTest(binary=binary_name):
                    binary = work / binary_name
                    binary.write_bytes(binary_bytes)
                    binary.chmod(0o755)
                    label = 'test-windows-x86_64' if extension == '.zip' else 'test-linux-x86_64'
                    package = 'Eidoryn-' + label
                    output = work / 'dist'
                    subprocess.run([
                        sys.executable, str(ROOT / 'scripts/package_release.py'),
                        '--label', label, '--binary', str(binary), '--output', str(output),
                    ], check=True, capture_output=True, text=True)
                    archive = output / (package + extension)
                    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
                    self.assertEqual(
                        archive.with_name(archive.name + '.sha256').read_text().splitlines(),
                        [f'{digest}  {archive.name}'],
                    )
                    if extension == '.zip':
                        with zipfile.ZipFile(archive) as bundle:
                            contents = {
                                name: bundle.read(name)
                                for name in bundle.namelist()
                                if not name.endswith('/')
                            }
                    else:
                        with tarfile.open(archive, 'r:gz') as bundle:
                            contents = {
                                entry.name: bundle.extractfile(entry).read()
                                for entry in bundle.getmembers()
                                if entry.isfile()
                            }
                            if sys.platform != 'win32':
                                self.assertTrue(
                                    bundle.getmember(f'{package}/{binary_name}').mode & 0o111,
                                )
                    expected = {binary_name, theme, *notices}
                    self.assertEqual(set(contents), {f'{package}/{name}' for name in expected})
                    self.assertEqual(contents[f'{package}/{binary_name}'], binary_bytes)
                    self.assertEqual(contents[f'{package}/{theme}'], (ROOT / theme).read_bytes())
                    for filename in notices:
                        self.assertEqual(contents[f'{package}/{filename}'], (ROOT / filename).read_bytes())


if __name__ == '__main__':
    unittest.main()
