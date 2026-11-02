"""Fail-closed process/socket ownership check for the local Mac launcher."""
import shutil
import subprocess


def listening_pid(port):
    command=shutil.which('lsof') or '/usr/sbin/lsof'
    try:
        result=subprocess.run([command,'-nP',f'-iTCP:{int(port)}','-sTCP:LISTEN','-Fp'],capture_output=True,text=True,timeout=2)
        pids={int(line[1:]) for line in result.stdout.splitlines() if line.startswith('p') and line[1:].isdigit()}
        return pids.pop() if len(pids)==1 else None
    except (OSError,ValueError,subprocess.TimeoutExpired):return None


def guard_for_state(path,port):
    def guard():
        import json
        from pathlib import Path
        from .core import ResearchError
        try:
            state=json.loads(Path(path).read_text())
            if state['port']!=port or listening_pid(port)!=state['pid'] or not isinstance(state['instance'],str):raise ValueError()
            return state['instance']
        except (OSError,ValueError,KeyError):raise ResearchError('search_unavailable','ownership',503) from None
    return guard
