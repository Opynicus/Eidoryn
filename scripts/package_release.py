"""Package only distributable files; never include local media, saves or captures."""
import argparse
import hashlib
import pathlib
import shutil
import tarfile
import tempfile
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument('--label', required=True)
parser.add_argument('--binary', required=True, type=pathlib.Path)
parser.add_argument('--output', default='dist', type=pathlib.Path)
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent.parent
args.output.mkdir(parents=True, exist_ok=True)
name = 'Eidoryn-' + args.label
with tempfile.TemporaryDirectory() as temporary:
    stage = pathlib.Path(temporary) / name
    stage.mkdir()
    shutil.copy2(args.binary, stage / args.binary.name)
    for filename in ['README.md', 'README.zh-CN.md', 'LICENSE', 'LICENSE-MIT', 'THIRD_PARTY.md']:
        shutil.copy2(root / filename, stage / filename)
    for directory in ['assets', 'plugins', 'packaging', 'docs']:
        shutil.copytree(root / directory, stage / directory)
    (stage / 'BUILD.txt').write_text(
        f'Eidoryn {args.label}\n'
        'Experimental project developed with extensive AI assistance. '
        'Hardware accuracy and game compatibility are still under active development.\n'
        'No BIOS, games or saves included. CLI packages have no desktop UI.\n'
        'Desktop packages are experimental and unsigned. JIT supports Linux x86-64 only.\n'
        'Launch from this directory to discover bundled themes.\n', encoding='utf-8')
    if args.binary.suffix == '.exe':
        archive = args.output / (name + '.zip')
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as output:
            for file in sorted(stage.rglob('*')):
                if file.is_file():
                    output.write(file, file.relative_to(stage.parent))
    else:
        archive = args.output / (name + '.tar.gz')
        with tarfile.open(archive, 'w:gz') as output:
            output.add(stage, arcname=name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name + '.sha256').write_text(f'{digest}  {archive.name}\n')
    print(archive)
