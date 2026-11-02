"""Opt-in: one Markdown file per day from this PC's own AI session logs (Codex: ~/.codex/sessions, Claude Code: ~/.claude/projects),
written into <data>/inbox so it becomes a private document. Only the text of what you asked and what the AI answered is kept
(no tool output); secret-shaped strings are replaced. Local only: no network, no AI call.

  python3 -m workbench.digest --data local-data [--day YYYY-MM-DD] [--codex-home DIR] [--claude-projects DIR]
The screen's server does it once a day by itself only when started with --digest-ai-sessions."""
import argparse
import datetime as dt
import glob
import json
import os
from pathlib import Path

from .notes import redact

USER_SKIP = ('<', 'Caveat:', '[Request interrupted')
TOTAL_CHARS = 1_000_000


def _ts(value):
    if isinstance(value, (int, float)):
        return float(value) / (1000 if value > 1e12 else 1)
    if isinstance(value, str) and value:
        try:
            return dt.datetime.fromisoformat(value.replace('Z', '+00:00')).timestamp()
        except ValueError:
            return None
    return None


def _clip(text, n):
    text = (text or '').strip()
    return text if len(text) <= n else text[:n].rstrip() + '…'


def _lines(path):
    try:
        with open(path, encoding='utf-8', errors='replace') as f:
            for line in f:
                if line.strip() and len(line) < 3_000_000:
                    try:
                        yield json.loads(line)
                    except ValueError:
                        continue
    except OSError:
        return


def _text_blocks(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return '\n'.join(b.get('text', '') for b in content if isinstance(b, dict) and b.get('type') == 'text' and b.get('text'))
    return ''


def claude_records(projects_root, start, end):
    """Claude Code: what you typed and the AI's text replies (not tool calls or results, not side chains)."""
    out = []
    root = Path(projects_root)
    if not root.is_dir():
        return out
    for path in sorted(root.glob('*/*.jsonl')):
        try:
            if path.stat().st_mtime < start:
                continue
        except OSError:
            continue
        project = path.parent.name.strip('-').rsplit('-', 1)[-1] or path.parent.name
        for o in _lines(path):
            if o.get('isSidechain') or o.get('isMeta') or o.get('type') not in ('user', 'assistant'):
                continue
            t = _ts(o.get('timestamp'))
            if t is None or not start <= t < end:
                continue
            text = _text_blocks((o.get('message') or {}).get('content')).strip()
            if o['type'] == 'user':
                if text and not text.startswith(USER_SKIP):
                    out.append((t, 'Claude Code', project, 'あなた', _clip(text, 2000)))
            elif len(text) >= 40:
                out.append((t, 'Claude Code', project, 'AI', _clip(text, 800)))
    return out


def codex_records(codex_home, start, end):
    """Codex: your requests and the final answer of each task."""
    out = []
    days = {(dt.datetime.fromtimestamp(start) + dt.timedelta(days=k)).strftime('%Y/%m/%d') for k in (-1, 0, 1)}
    for day in sorted(days):
        for f in sorted(glob.glob(os.path.join(codex_home, 'sessions', day, '*.jsonl'))):
            project = ''
            for o in _lines(f):
                p = o.get('payload') or {}
                if o.get('type') == 'session_meta':
                    project = os.path.basename((p.get('cwd') or '').rstrip('/'))
                if o.get('type') != 'event_msg':
                    continue
                t = _ts(o.get('timestamp'))
                if t is None or not start <= t < end:
                    continue
                if p.get('type') == 'user_message' and isinstance(p.get('message'), str) and p['message'].strip():
                    out.append((t, 'Codex', project, 'あなた', _clip(p['message'], 2000)))
                elif p.get('type') == 'task_complete' and isinstance(p.get('last_agent_message'), str) and p['last_agent_message'].strip():
                    out.append((t, 'Codex', project, 'AI', _clip(p['last_agent_message'], 1200)))
    return out


def render(day, records):
    lines = ['# 題', 'AI との会話', '', '# 日時', day, '']
    used = 0; last = None
    for t, tool, project, who, text in sorted(records):
        head = (tool, project)
        if head != last:
            lines += ['', f'## {tool}・{project or "(場所なし)"}', '']; last = head
        line = f'- {dt.datetime.fromtimestamp(t).strftime("%H:%M")} {who}: ' + text.replace('\n', '\n  ')
        if used + len(line) > TOTAL_CHARS:
            lines.append('- (この日の残りは長さの上限で省いた)'); break
        lines.append(line); used += len(line)
    return redact('\n'.join(lines) + '\n')[0]


def write_digest(data_dir, day, codex_home, claude_projects):
    """Writes <data>/inbox/ai-sessions-<day>.md (atomically). None when that day has no records."""
    start = dt.datetime.fromisoformat(day).timestamp()
    end = (dt.datetime.fromisoformat(day) + dt.timedelta(days=1)).timestamp()
    records = (codex_records(codex_home, start, end) if codex_home else []) + (claude_records(claude_projects, start, end) if claude_projects else [])
    if not records:
        return None
    inbox = Path(data_dir) / 'inbox'
    inbox.mkdir(mode=0o700, parents=True, exist_ok=True)
    target = inbox / f'ai-sessions-{day}.md'
    tmp = inbox / f'.ai-sessions-{day}.md.tmp'
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w', encoding='utf-8') as f:
        f.write(render(day, records))
    os.replace(tmp, target)
    return target


def default_sources(environ=None):
    environ = os.environ if environ is None else environ
    home = Path(environ.get('HOME') or Path.home())
    codex = environ.get('CODEX_HOME') or str(home / '.codex')
    claude = str(Path(environ['CLAUDE_CONFIG_DIR']) / 'projects') if environ.get('CLAUDE_CONFIG_DIR') else str(home / '.claude' / 'projects')
    return codex, claude


def main(argv=None):
    codex, claude = default_sources()
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--data', required=True, help="the screen's data folder (the inbox is <data>/inbox)")
    parser.add_argument('--day', default=(dt.date.today() - dt.timedelta(days=1)).isoformat(), help='local day, default yesterday')
    parser.add_argument('--codex-home', default=codex)
    parser.add_argument('--claude-projects', default=claude)
    args = parser.parse_args(argv)
    path = write_digest(args.data, args.day, args.codex_home, args.claude_projects)
    print(json.dumps({'day': args.day, 'written': str(path) if path else None}, ensure_ascii=False))


if __name__ == '__main__':
    main()
