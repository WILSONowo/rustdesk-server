"""Add verified beta1 artifacts without replacing assets or changing release settings."""
import hashlib
import json
import os
import sys
from pathlib import Path
import subprocess
import tarfile
import zipfile


def gh(*args):
    return subprocess.check_output(['gh', *args])


def api(endpoint):
    return json.loads(gh('api', endpoint))


def digest(path):
    return 'sha256:' + hashlib.sha256(path.read_bytes()).hexdigest()


repo = os.environ['GITHUB_REPOSITORY']
tag = os.environ['RELEASE_TAG']
source = os.environ['SOURCE_COMMIT']
build_commit = os.environ['BUILD_COMMIT']
run_id = os.environ['BUILD_RUN']
kind = os.environ['PACKAGE_KIND']
count = {'server': 17, 'api': 7, 'web': 1}[kind]
prefix = f'repos/{repo}'
run = api(f'{prefix}/actions/runs/{run_id}')
assert run['conclusion'] == 'success', 'Source build has not succeeded'
assert run['head_sha'] == build_commit
assert run['path'] == '.github/workflows/backfill-beta1.yml'
ref = api(f'{prefix}/git/ref/tags/{tag}')['object']
while ref['type'] == 'tag':
    ref = api(f"{prefix}/git/tags/{ref['sha']}")['object']
assert ref['type'] == 'commit' and ref['sha'] == source, 'Release tag changed'
release = api(f'{prefix}/releases/tags/{tag}')
assert release['tag_name'] == tag
pattern = {'server': 'server-release-*', 'api': 'api-release-*', 'web': 'web-release-assets'}[kind]
gh('run', 'download', run_id, '--repo', repo, '--pattern', pattern, '--dir', 'download')
files = [p for p in Path('download').rglob('*') if p.is_file()]
assert len(files) == count and len({p.name for p in files}) == count
assets = Path('supplement-assets')
assets.mkdir()
for file in files:
    assert file.stat().st_size > 0
    if file.suffix == '.zip':
        with zipfile.ZipFile(file) as z:
            assert z.testzip() is None
            if kind == 'web':
                assert 'index.html' in z.namelist()
            elif kind == 'api':
                assert z.read('release/resources/version').decode().strip() == tag
                assert 'release/resources/admin/index.html' in z.namelist()
    elif file.name.endswith('.tar.gz'):
        with tarfile.open(file) as t:
            assert t.extractfile('release/resources/version').read().decode().strip() == tag
            assert t.getmember('release/resources/admin/index.html').size > 0
    elif file.suffix == '.deb':
        version = subprocess.check_output(['dpkg-deb', '-f', str(file), 'Version'], text=True).strip()
        assert version == tag.lstrip('v').replace('-beta', '~beta')
    # GitHub normalizes '~' in uploaded names; keep the Debian version inside the package intact.
    (assets / file.name.replace('~', '.')).write_bytes(file.read_bytes())
metadata = {
    'repository': repo, 'tag': tag, 'source_commit': source,
    'workflow_commit': build_commit, 'workflow_run': run['html_url'],
    'purpose': 'Additional platform packages for the existing beta1 release',
    'build_only_inputs': ({'windows_ui': 'Packaging branch npm lockfile and NSIS version label'} if kind == 'server' else
                         {'go_sum': 'Verified checksum for already-pinned mousetrap v1.1.0',
                          'web_commit': 'fea7cb30d71cbf8c687ce7d22765614908764d87'} if kind == 'api' else {}),
}
(assets / 'build-info-beta1-supplement.json').write_text(json.dumps(metadata, indent=2) + '\n')
(assets / 'SHA256SUMS-beta1-supplement.txt').write_text(''.join(
    f'{digest(p)[7:]}  {p.name}\n' for p in sorted(assets.iterdir())))
old = {a['name']: a for a in release['assets']}
expected = {p.name: digest(p) for p in assets.iterdir()}
# Validate all conflicts before uploading anything. Never use --clobber.
for name, checksum in expected.items():
    if name in old:
        assert old[name]['digest'] == checksum, f'Existing asset differs: {name}'
if '--verify-only' in sys.argv:
    print(f'Validated {count} packages; no release changes made.')
    sys.exit(0)
for name in sorted(expected):
    if name not in old:
        subprocess.run(['gh', 'release', 'upload', tag, '--repo', repo, str(assets / name)], check=True)
after = api(f'{prefix}/releases/tags/{tag}')
for field in ['id', 'name', 'body', 'draft', 'prerelease', 'tag_name', 'target_commitish']:
    assert after[field] == release[field], f'Release setting changed: {field}'
received = {a['name']: a for a in after['assets']}
for name, checksum in expected.items():
    assert received[name]['digest'] == checksum, name
for name, asset in old.items():
    assert received[name]['id'] == asset['id'] and received[name]['digest'] == asset['digest']
print(f'Verified {count} packages and two metadata files; existing release contents preserved.')
