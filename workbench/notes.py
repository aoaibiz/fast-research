"""Notes into the private documents of this PC: secret stripping, the inbox folder, and the private tools of the MCP entry.
Local only: nothing here opens a network connection or calls an AI."""
import hashlib
import json
import math
import os
import re
import secrets
import time
from collections import Counter
from pathlib import Path

from .core import excerpt

# ---------------------------------------------------------------- secret-shaped strings (same rules as the box's notes ingest)
D5 = '-' * 5  # the PEM fence, not written out
SECRET_PATTERNS = [
    re.compile(D5 + r'BEGIN [A-Z ]*PRIVATE KEY' + D5 + r'.*?' + D5 + r'END [A-Z ]*PRIVATE KEY' + D5, re.S),
    re.compile(r'\b(?:sk|pk|rk)-(?:proj-|live-|test-|ant-)?[A-Za-z0-9_\-]{16,}'),
    re.compile(r'\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}'),
    re.compile(r'\bgithub_pat_[A-Za-z0-9_]{20,}'),
    re.compile(r'\bglpat-[A-Za-z0-9_\-]{16,}'),
    re.compile(r'\bxox[abposr]-[A-Za-z0-9\-]{10,}'),
    re.compile(r'\bAKIA[0-9A-Z]{16}\b'),
    re.compile(r'\bAIza[0-9A-Za-z_\-]{30,}'),
    re.compile(r'\bEAA[A-Za-z0-9]{30,}'),
    re.compile(r'\bey' + r'J[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}'),
    re.compile(r'(?i)\bBearer\s+[A-Za-z0-9._\-+/=]{16,}'),
    re.compile(r'https?://[a-z0-9\-]+\.trycloudflare\.com\S*'),
]
NAMED_VALUE = re.compile(r'(?i)\b((?:api[_\-]?key|secret|token|password|passwd|client[_\-]?secret|access[_\-]?key|private[_\-]?key)["\']?\s*[:=]\s*)(["\']?)([^\s"\']{6,})')
LONG_TOKEN = re.compile(r'[A-Za-z0-9_\-+=]{32,}')  # no "/" so paths and URLs stay
UUID = re.compile(r'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$')


def _entropy(s):
    c = Counter(s)
    return -sum(v / len(s) * math.log2(v / len(s)) for v in c.values())


def redact(text):
    """Replaces key prefixes (sk-, ghp_ ...), JWTs, PEM private keys, values named password/token/..., and long random strings
    with [REDACTED]. Returns (text, how many were replaced)."""
    n = 0
    for p in SECRET_PATTERNS:
        text, k = p.subn('[REDACTED]', text)
        n += k
    text, k = NAMED_VALUE.subn(lambda m: m.group(1) + m.group(2) + '[REDACTED]', text)
    n += k

    def long_tok(m):
        nonlocal n
        s = m.group(0)
        if UUID.match(s) or s.startswith('[REDACTED') or ('-' in s and max(len(x) for x in s.split('-')) < 16):
            return s  # UUIDs and names made of short words joined by - (file names) stay
        if re.search(r'[A-Za-z]', s) and re.search(r'[0-9]', s) and _entropy(s) >= 3.5:
            n += 1
            return '[REDACTED]'
        return s
    return LONG_TOKEN.sub(long_tok, text), n


# ---------------------------------------------------------------- titles of dropped files
def _section(text, names):
    """The value of a 「# 題」 heading (its next line) or of a 「題: ...」 line."""
    lines = text.splitlines()
    for i, line in enumerate(lines):
        s = line.strip().lstrip('#').strip().lstrip('-*').strip()
        for name in names:
            m = re.match(rf'^\**{name}\**\s*[:：]\s*(.+)$', s, re.I)
            if m:
                return m.group(1).strip().strip('*').strip()
            if re.match(rf'^\**{name}\**$', s, re.I) and line.lstrip().startswith('#'):
                for nxt in lines[i + 1:]:
                    if nxt.strip():
                        return None if nxt.lstrip().startswith('#') else nxt.strip().lstrip('-*').strip()
    return None


def title_for(name, text, mtime):
    """「題名(YYYY-MM-DD)」 from the note: a 題/タイトル/件名/title line, else the first heading, else the file name; the date
    from a 日時/日付/date line, else a date at the start of the file name, else the file's time."""
    stem = Path(name).stem
    topic = _section(text, ['題', 'タイトル', '件名', 'title'])
    if not topic:
        m = re.search(r'^#\s+(.+)$', text, re.M)
        topic = m.group(1).strip() if m and m.group(1).strip() not in ('日時', '日付') else None
    if not topic:
        topic = re.sub(r'^\d{4}-\d{2}-\d{2}-?', '', stem) or stem
    when = _section(text, ['日時', '日付', 'date'])
    m = re.search(r'(\d{4})[-/年](\d{1,2})[-/月](\d{1,2})', when or '') or re.match(r'(\d{4})-(\d{2})-(\d{2})', name)
    date = f'{int(m.group(1)):04d}-{int(m.group(2)):02d}-{int(m.group(3)):02d}' if m else time.strftime('%Y-%m-%d', time.localtime(mtime))
    topic = re.sub(r'\s+', ' ', topic)[:150]
    return f'{topic}({date})'


# ---------------------------------------------------------------- the local person and saving
def local_owner(store, create=True):
    """The first admin = the person who uses this PC (the same account the screen signs in to on this PC)."""
    admin = store.first_admin()
    if admin is None and create:
        try:
            store.register('自分', secrets.token_urlsafe(24))
        except ValueError as error:
            if str(error) != 'setup_finished':
                raise
        admin = store.first_admin()
    return admin['id'] if admin else None


def save_note(store, owner, title, body):
    """Stores a note as a private document after stripping secret-shaped strings. The same title and body are kept once."""
    if not isinstance(title, str) or not isinstance(body, str):
        raise ValueError('invalid_request')
    clean_title, n_title = redact(title)
    clean_body, n_body = redact(body)
    clean_title = re.sub(r'\s+', ' ', clean_title).strip()[:200]
    clean_body = clean_body[:store.limits['document_chars']]
    existing = store.document_id_for(owner, clean_title, clean_body)
    if existing:
        return {'saved': False, 'duplicate': True, 'id': existing, 'redacted': n_title + n_body}
    identifier = store.add_document(owner, clean_title, clean_body)
    return {'saved': True, 'duplicate': False, 'id': identifier, 'redacted': n_title + n_body}


def _fit(payload, budget):
    return len(json.dumps(payload, ensure_ascii=False).encode()) <= budget


def search_private(store, owner, query, limit=8, budget=4000):
    """Titles, short excerpts and ids of the person's documents (FTS5 + BM25, the screen's ranking) and saved research, at most
    `budget` bytes of JSON."""
    if not isinstance(query, str) or not query.strip() or len(query) > 500:
        raise ValueError('invalid_query')
    limit = max(1, min(int(limit or 8), 10))
    docs = [{'kind': 'document', 'id': d['id'], 'title': d['title'], 'created': round(d['created']), 'excerpt': excerpt(d['body'], query, 300) or d['body'][:160]}
            for d in store.search_documents(owner, query, limit=limit)]
    research = [{'kind': 'research', 'id': r['id'], 'title': r['query'], 'created': round(r['created']), 'excerpt': excerpt(r['text'], query, 300) or r['text'][:160]}
                for r in store.search_reports(owner, query, limit=max(1, limit // 2))]
    result = {'query': query, 'results': docs + research, 'truncated': False, 'read_with': 'read_private'}
    for size in (300, 200, 120, 60):
        for item in result['results']:
            item['excerpt'] = item['excerpt'][:size]
        if _fit(result, budget):
            return result
    while result['results'] and not _fit(result, budget):
        result['results'].pop(); result['truncated'] = True
    return result


def read_private(store, owner, identifier, offset=0, budget=4000):
    """One page (at most `budget` bytes of JSON) of a document or of a saved research's text, from character `offset`."""
    if not isinstance(identifier, str) or not identifier or not isinstance(offset, int) or isinstance(offset, bool) or offset < 0:
        raise ValueError('invalid_request')
    doc = store.document(owner, identifier)
    if doc:
        kind, title, text = 'document', doc['title'], doc['body']
    else:
        report = store.report(owner, identifier)
        if not report:
            raise ValueError('not_found')
        kind, title, text = 'research', report.get('query', ''), report.get('markdown', '')
    page = {'kind': kind, 'id': identifier, 'title': title, 'offset': offset, 'total_chars': len(text), 'next_offset': None, 'text': ''}
    low, high = 0, max(0, min(len(text) - offset, budget))
    while low < high:  # the longest slice that fits
        mid = (low + high + 1) // 2
        page['text'] = text[offset:offset + mid]
        if _fit(page, budget - 24):
            low = mid
        else:
            high = mid - 1
    page['text'] = text[offset:offset + low]
    page['next_offset'] = offset + low if offset + low < len(text) else None
    return page


# ---------------------------------------------------------------- the inbox folder
INBOX_SUFFIXES = ('.md', '.txt')


def _write_json(path, value):
    tmp = path.with_name(path.name + '.tmp')
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w', encoding='utf-8') as f:
        json.dump(value, f, ensure_ascii=False)
    os.replace(tmp, path)


def import_inbox(store, data_dir, settle=30, now=None):
    """Imports every settled .md / .txt dropped in <data>/inbox as a private document of the local person, then moves the file to
    inbox/done/. Same content twice: stored once. The same file name dropped again with new content replaces the document it made.
    A file that cannot be stored stays where it is and is not tried again until its content changes. Returns the events."""
    inbox = Path(data_dir) / 'inbox'
    inbox.mkdir(mode=0o700, exist_ok=True)
    done = inbox / 'done'
    done.mkdir(mode=0o700, exist_ok=True)
    owner = local_owner(store, create=False)
    if owner is None:
        return []
    state_path = inbox / '.state.json'
    try:
        state = json.loads(state_path.read_text(encoding='utf-8'))
    except (OSError, ValueError):
        state = {}
    state.setdefault('contents', {}); state.setdefault('names', {}); state.setdefault('failed', {})
    now = time.time() if now is None else now
    events = []
    for p in sorted(inbox.iterdir()):
        if p.name.startswith(('.', '_')) or p.suffix.lower() not in INBOX_SUFFIXES or p.is_symlink() or not p.is_file():
            continue
        st = p.stat()
        if now - st.st_mtime < settle or st.st_size > 4 * store.limits['document_chars']:
            if st.st_size > 4 * store.limits['document_chars'] and state['failed'].get(p.name) != f'size:{st.st_size}:{st.st_mtime}':
                state['failed'][p.name] = f'size:{st.st_size}:{st.st_mtime}'; events.append({'event': 'too_large', 'file': p.name})
            continue
        raw = p.read_bytes(); digest = hashlib.sha256(raw).hexdigest()
        if state['failed'].get(p.name) == digest:
            continue
        if digest in state['contents'] and store.document(owner, state['contents'][digest]):
            identifier, how = state['contents'][digest], 'duplicate'
            events.append({'event': how, 'file': p.name, 'redacted': 0})
        else:
            text = raw.decode('utf-8', errors='replace')
            if not text.strip():
                state['failed'][p.name] = digest; events.append({'event': 'empty', 'file': p.name}); continue
            title, n_title = redact(title_for(p.name, text, st.st_mtime))
            body, n_body = redact(text)
            title = re.sub(r'\s+', ' ', title).strip()[:200]; body = body[:store.limits['document_chars']]
            earlier = state['names'].get(p.name)
            try:
                if earlier and store.document(owner, earlier):
                    changed = store.replace_document(owner, earlier, body, title)
                    identifier, how = earlier, 'replaced' if changed and changed['replaced'] else 'duplicate'
                else:
                    known = store.document_id_for(owner, title, body)
                    identifier = known or store.add_document(owner, title, body)
                    how = 'duplicate' if known else 'imported'
            except ValueError as error:
                state['failed'][p.name] = digest; events.append({'event': 'refused', 'file': p.name, 'code': str(error)}); continue
            events.append({'event': how, 'file': p.name, 'redacted': n_title + n_body})
        state['contents'][digest] = identifier; state['names'][p.name] = identifier; state['failed'].pop(p.name, None)
        target = done / p.name
        k = 1
        while target.exists():
            target = done / f'{p.stem}-{k}{p.suffix}'; k += 1
        p.rename(target)
    _write_json(state_path, state)
    return events
