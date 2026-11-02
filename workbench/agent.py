"""AI-first stdio entry: private child backend, no UI, account, DB or daemon required."""
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


def config(binary=None):
    return {'mcpServers': {'talmud-research': {
        'command': sys.executable,
        'args': [str(Path(__file__).resolve()), '--binary', str(binary_path(binary))],
    }}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', help='Absolute path to this modified rustxng binary')
    parser.add_argument('--print-config', action='store_true', help='Print a machine-specific MCP configuration; do not edit existing host settings')
    args = parser.parse_args()
    if args.print_config:
        print(json.dumps(config(args.binary), ensure_ascii=False, indent=2))
        return
    research = ManagedResearch(args.binary)
    previous = signal.getsignal(signal.SIGTERM)
    def terminate(signum, frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, terminate)
    try:
        serve(research)
    except (KeyboardInterrupt, BrokenPipeError):
        pass
    finally:
        research.close()
        signal.signal(signal.SIGTERM, previous)


if __name__ == '__main__':
    main()
