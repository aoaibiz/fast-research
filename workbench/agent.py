"""AI-first stdio entry: private child backend, no UI, account, DB or daemon required.
Opt-in --private: also search / read / save this PC's own private documents (the screen's database in --data; local only)."""
import argparse
import errno
import json
from pathlib import Path
import signal
import socket
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from workbench.core import Research, ResearchError
from workbench.mcp import serve
from workbench.server import Backend
from workbench import notes
from workbench.store import Store

DATA = ROOT / 'local-data'  # the screen's default data folder (workbench/launch.py and workbench.server use the same)


def binary_path(explicit=None):
    if explicit:
        return Path(explicit).expanduser().resolve()
    for mode in ('release', 'debug'):
        candidate = ROOT / 'target' / mode / 'rustxng'
        if candidate.is_file():
            return candidate
    return ROOT / 'target' / 'release' / 'rustxng'


class ManagedResearch(Research):
    """One MCP process owns one backend. Close only that child on EOF or termination."""
    def __init__(self, binary=None):
        super().__init__('http://127.0.0.1:1')
        self.binary = binary_path(binary)
        self.manager = None
        self.temporary = None

    def fetch(self, query, profile, domains, *, raw=True):
        if self.manager is None:
            if not self.binary.is_file():
                raise ResearchError('backend_missing', 'startup', 503)
            self.temporary = tempfile.TemporaryDirectory(prefix='talmud-research-')
            with socket.socket() as listener:
                listener.bind(('127.0.0.1', 0))
                port = listener.getsockname()[1]
            # Dies with this process (PDEATHSIG on Linux); the idle timeout covers hosts without it.
            self.manager = Backend(self.binary, port, self.temporary.name, idle_timeout=900)
            try:
                self.manager.start()
            except OSError as error:
                self.close()
                code = {errno.EACCES: 'backend_not_executable', errno.ENOEXEC: 'backend_wrong_format',
                        errno.ENOENT: 'backend_missing'}.get(error.errno, 'backend_launch_failed')
                raise ResearchError(code, 'startup', 503) from None
            except BaseException:
                self.close()
                raise
            self.backend = f'http://127.0.0.1:{port}'
            self.guard = self.manager.ensure_owned
        return super().fetch(query, profile, domains, raw=raw)

    def close(self):
        try:
            if self.manager is not None:
                self.manager.stop()
        finally:
            self.manager = None
            if self.temporary is not None:
                self.temporary.cleanup()
                self.temporary = None


class PrivateNotes:
    """The private tools: the local person's documents and saved research in the screen's SQLite (opened on first use)."""
    def __init__(self, data):
        self.data = Path(data).expanduser().resolve()
        self.store = None
        self.owner = None

    def _open(self):
        if self.store is None:
            self.data.mkdir(parents=True, exist_ok=True, mode=0o700)
            self.store = Store(self.data / 'workbench.sqlite')
            self.owner = notes.local_owner(self.store)
        return self.store, self.owner

    def search(self, query, limit=8):
        store, owner = self._open()
        return notes.search_private(store, owner, query, limit)

    def read(self, identifier, offset=0):
        store, owner = self._open()
        return notes.read_private(store, owner, identifier, offset)

    def save(self, title, body):
        store, owner = self._open()
        return notes.save_note(store, owner, title, body)


def config(binary=None, private=False, data=None):
    args = [str(Path(__file__).resolve()), '--binary', str(binary_path(binary))]
    if private:
        args += ['--private', '--data', str(Path(data or DATA).expanduser().resolve())]
    return {'mcpServers': {'fast-research': {'command': sys.executable, 'args': args}}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', help='Absolute path to this modified rustxng binary')
    parser.add_argument('--print-config', action='store_true', help='Print a machine-specific MCP configuration; do not edit existing host settings')
    parser.add_argument('--private', action='store_true', help="opt-in: also search, read and save this PC's own private documents (local only)")
    parser.add_argument('--data', default=str(DATA), help="the screen's data folder (default: local-data next to workbench)")
    args = parser.parse_args()
    if args.print_config:
        print(json.dumps(config(args.binary, args.private, args.data), ensure_ascii=False, indent=2))
        return
    research = ManagedResearch(args.binary)
    private = PrivateNotes(args.data) if args.private else None
    previous = signal.getsignal(signal.SIGTERM)
    def terminate(signum, frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, terminate)
    try:
        serve(research, private)
    except (KeyboardInterrupt, BrokenPipeError):
        pass
    finally:
        research.close()
        signal.signal(signal.SIGTERM, previous)


if __name__ == '__main__':
    main()
