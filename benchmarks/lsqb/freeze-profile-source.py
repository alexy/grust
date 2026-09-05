#!/usr/bin/env python3
"""Freeze allowlisted tracked and untracked inputs for a diagnostic Linux build.

This is not publication evidence or a worktree lock. A fresh output directory
contains a read-only source copy and a manifest written only after source
revalidation. Failed outputs are incomplete and must never be reused. Runtime
datasets and upstream query/oracle roots must be supplied separately.
"""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import sys


SCHEMA = 'grust-lsqb-profile-source-v1'
EXACT = frozenset({
    'Cargo.toml', 'Cargo.lock', '.cargo/config', '.cargo/config.toml',
    'benchmarks/lsqb/Cargo.toml', 'benchmarks/lsqb/Cargo.lock',
    'benchmarks/lsqb/evidence-manifest-v2.json',
})
PREFIXES = ('crates/', 'examples/beads/', 'benchmarks/lsqb/src/',
            'benchmarks/lsqb/examples/', 'benchmarks/lsqb/attacks/')
REQUIRED = frozenset({
    'Cargo.toml', 'Cargo.lock', 'examples/beads/Cargo.toml',
    'benchmarks/lsqb/Cargo.toml', 'benchmarks/lsqb/Cargo.lock',
    'benchmarks/lsqb/evidence-manifest-v2.json',
    'benchmarks/lsqb/examples/profile_memory.rs',
})
EXCLUDED_PARTS = frozenset({
    '.git', 'target', 'upstream', 'datasets', 'node_modules', '__pycache__',
    '.DS_Store', '.netrc', '.npmrc', '.pypirc', '.git-credentials',
    'credentials', 'credentials.toml', 'id_rsa', 'id_ed25519', 'id_ecdsa',
})
READ_FLAGS = os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW
DIRECTORY_FLAGS = READ_FLAGS | os.O_DIRECTORY
CHUNK_BYTES = 1024 * 1024


def canonical(value):
    return (json.dumps(value, sort_keys=True, ensure_ascii=True,
                       separators=(',', ':'), allow_nan=False) + '\n').encode('utf-8')


def relative_path(path):
    parts = path.split('/')
    if not path or '\\' in path or any(part in ('', '.', '..') for part in parts):
        raise ValueError(f'unsafe Git input path: {path!r}')
    if PurePosixPath(path).is_absolute():
        raise ValueError(f'absolute Git input path: {path!r}')
    return parts


def allowed(path):
    parts = relative_path(path)
    if any(part in EXCLUDED_PARTS or part == '.env' or part.startswith('.env.') for part in parts):
        return False
    return path in EXACT or path.startswith(PREFIXES)


def git(root, *arguments):
    environment = os.environ.copy()
    for key in ('GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_COMMON_DIR',
                'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES'):
        environment.pop(key, None)
    environment['GIT_OPTIONAL_LOCKS'] = '0'
    try:
        result = subprocess.run(
            ['git', '-C', str(root), '-c', 'core.fsmonitor=false', *arguments],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True,
            env=environment, timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise ValueError(f'cannot inspect source Git state: {error}') from error
    return result.stdout


def source_state(root):
    head = git(root, 'rev-parse', '--verify', 'HEAD').decode('ascii').strip()
    if re.fullmatch(r'(?:[0-9a-f]{40}|[0-9a-f]{64})', head) is None:
        raise ValueError('source HEAD is not a full Git object ID')
    status = git(root, 'status', '--porcelain=v1', '-z', '--untracked-files=all')
    raw = git(root, 'ls-files', '-z', '--cached', '--others', '--exclude-standard')
    if raw and not raw.endswith(b'\0'):
        raise ValueError('Git input inventory is not NUL-terminated')
    try:
        paths = sorted({path for path in raw.decode('utf-8').split('\0') if path and allowed(path)})
    except UnicodeDecodeError as error:
        raise ValueError('Git input paths must be UTF-8') from error
    missing = REQUIRED.difference(paths)
    if missing:
        raise ValueError(f'missing required build inputs: {sorted(missing)}')
    return head, status, paths


@contextmanager
def directory(root_fd, parts=(), create=False):
    descriptor = os.dup(root_fd)
    try:
        for part in parts:
            if create:
                try:
                    os.mkdir(part, mode=0o700, dir_fd=descriptor)
                except FileExistsError:
                    pass
            child = os.open(part, DIRECTORY_FLAGS, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        yield descriptor
    finally:
        os.close(descriptor)


def signature(metadata):
    return (metadata.st_dev, metadata.st_ino, metadata.st_mode, metadata.st_size,
            metadata.st_mtime_ns, metadata.st_ctime_ns)


def file_stat(root_fd, path):
    parts = relative_path(path)
    with directory(root_fd, parts[:-1]) as parent:
        metadata = os.stat(parts[-1], dir_fd=parent, follow_symlinks=False)
    if not stat.S_ISREG(metadata.st_mode):
        raise ValueError(f'input must be a regular non-symlink file: {path}')
    return metadata


def read_file(root_fd, path, expected, destination=None):
    """Stream exactly the captured length; detect replacement or concurrent writes."""
    parts = relative_path(path)
    digest = hashlib.sha256()
    with directory(root_fd, parts[:-1]) as parent:
        descriptor = os.open(parts[-1], READ_FLAGS | os.O_NONBLOCK, dir_fd=parent)
        with os.fdopen(descriptor, 'rb') as source:
            if signature(os.fstat(source.fileno())) != signature(expected):
                raise ValueError(f'source changed before reading: {path}')
            remaining = expected.st_size
            while remaining:
                chunk = source.read(min(remaining, CHUNK_BYTES))
                if not chunk:
                    raise ValueError(f'source truncated while reading: {path}')
                remaining -= len(chunk)
                digest.update(chunk)
                if destination is not None:
                    destination.write(chunk)
            if source.read(1) or signature(os.fstat(source.fileno())) != signature(expected):
                raise ValueError(f'source changed while reading: {path}')
        current = os.stat(parts[-1], dir_fd=parent, follow_symlinks=False)
        if signature(current) != signature(expected):
            raise ValueError(f'source path changed while reading: {path}')
    return digest.hexdigest()


def copy_file(source_fd, output_fd, path, metadata):
    parts = relative_path(path)
    mode = stat.S_IMODE(metadata.st_mode) & 0o555
    with directory(output_fd, parts[:-1], create=True) as parent:
        descriptor = os.open(parts[-1], os.O_WRONLY | os.O_CREAT | os.O_EXCL |
                             os.O_CLOEXEC | os.O_NOFOLLOW, 0o600, dir_fd=parent)
        with os.fdopen(descriptor, 'wb') as destination:
            digest = read_file(source_fd, path, metadata, destination)
            destination.flush()
            os.fchmod(destination.fileno(), mode)
            os.fsync(destination.fileno())
    return dict(path=path, size=metadata.st_size, mode=f'{mode:04o}',
                source_mode=f'{stat.S_IMODE(metadata.st_mode):04o}', sha256=digest)


def readonly_directories(root_fd, paths):
    directories = {()}
    for path in paths:
        parts = relative_path(path)
        directories.update(tuple(parts[:length]) for length in range(1, len(parts)))
    for parts in sorted(directories, key=lambda value: (-len(value), value)):
        with directory(root_fd, parts) as descriptor:
            os.fchmod(descriptor, 0o555)
            os.fsync(descriptor)


def freeze(root, output):
    root, output = Path(root), Path(output)
    if root.is_symlink() or not root.is_dir():
        raise ValueError('source root must be a regular non-symlink directory')
    root = root.resolve(strict=True)
    if output.name in ('', '.', '..'):
        raise ValueError('output must name a fresh directory')
    output = output.parent.resolve(strict=True) / output.name
    if output == root or root in output.parents:
        raise ValueError('output must be outside the source worktree')
    if output.exists() or output.is_symlink():
        raise ValueError('output already exists; refusing to reuse it')
    top = git(root, 'rev-parse', '--show-toplevel').decode('utf-8').strip()
    if Path(top).resolve(strict=True) != root:
        raise ValueError('source root must be the Git worktree root')
    initial = source_state(root)
    head, status, paths = initial
    source_fd = os.open(root, DIRECTORY_FLAGS)
    output_fd = copy_fd = None
    try:
        root_identity = os.fstat(source_fd)
        metadata = {path: file_stat(source_fd, path) for path in paths}
        output.mkdir(mode=0o700)
        output_fd = os.open(output, DIRECTORY_FLAGS)
        os.mkdir('source', mode=0o700, dir_fd=output_fd)
        copy_fd = os.open('source', DIRECTORY_FLAGS, dir_fd=output_fd)
        entries = [copy_file(source_fd, copy_fd, path, metadata[path]) for path in paths]
        # Independently hash the frozen bytes too. No hard links or live-source
        # mounts are used, so subsequent worktree edits cannot alter this copy.
        for entry in entries:
            path = entry['path']
            copied = file_stat(copy_fd, path)
            if (copied.st_size != entry['size'] or
                    stat.S_IMODE(copied.st_mode) != int(entry['mode'], 8) or
                    read_file(copy_fd, path, copied) != entry['sha256']):
                raise ValueError(f'frozen copy failed verification: {path}')
            if read_file(source_fd, path, metadata[path]) != entry['sha256']:
                raise ValueError(f'source bytes changed after copying: {path}')
        # Recheck every captured file after the content pass, then the Git
        # inventory/HEAD/dirty status. Metadata detects same-byte replacements.
        for path in paths:
            if signature(file_stat(source_fd, path)) != signature(metadata[path]):
                raise ValueError(f'source metadata changed after copying: {path}')
        if source_state(root) != initial:
            raise ValueError('source Git state or allowed path list changed while freezing')
        current_root = os.stat(root, follow_symlinks=False)
        if (current_root.st_dev, current_root.st_ino, current_root.st_mode) != (
                root_identity.st_dev, root_identity.st_ino, root_identity.st_mode):
            raise ValueError('source root changed while freezing')
        readonly_directories(copy_fd, paths)
        manifest = dict(
            schema=SCHEMA, publication_eligible=False,
            source_head=head, source_dirty=bool(status),
            git_status_sha256=hashlib.sha256(status).hexdigest(),
            aggregate_sha256=hashlib.sha256(canonical(entries)).hexdigest(),
            file_count=len(entries), total_bytes=sum(entry['size'] for entry in entries),
            files=entries, source_subdirectory='source',
            input_worktree_locked=False,
            verification='source bytes, metadata, Git state and allowlist revalidated after copy',
            limitation='Diagnostic build inputs only; not publication or host-isolation evidence.',
        )
        descriptor = os.open('manifest.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL |
                             os.O_CLOEXEC | os.O_NOFOLLOW, 0o600, dir_fd=output_fd)
        with os.fdopen(descriptor, 'wb') as destination:
            destination.write(canonical(manifest))
            destination.flush()
            os.fchmod(destination.fileno(), 0o444)
            os.fsync(destination.fileno())
        os.fsync(output_fd)
        return manifest
    finally:
        for descriptor in (copy_fd, output_fd, source_fd):
            if descriptor is not None:
                os.close(descriptor)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        manifest = freeze(args.root, args.output)
    except (ValueError, OSError) as error:
        print(f'freeze-profile-source: {error}; incomplete outputs must not be reused', file=sys.stderr)
        return 1
    print(json.dumps({key: manifest[key] for key in
                      ('schema', 'publication_eligible', 'source_head', 'source_dirty',
                       'file_count', 'total_bytes', 'aggregate_sha256')}, sort_keys=True), flush=True)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
