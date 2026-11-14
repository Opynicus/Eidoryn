"""Package only runtime files; never include local media, saves or captures."""
import argparse
import hashlib
import pathlib
import shutil
import tarfile
import tempfile
import zipfile


RUNTIME_FILES = ('plugins/themes/ember.json',)


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
    for filename in RUNTIME_FILES:
        destination = stage / filename
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(root / filename, destination)
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
