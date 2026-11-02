"""Local account-scoped storage. No external connections or request-content logging."""
import codecs
import hashlib
import collections
import copy
from contextlib import contextmanager
import hmac
import re
import json
import os
import secrets
import sqlite3
import time
from pathlib import Path
from .tokens import fts_text, fts_query, fts_query_all, terms

# Private-document capacity (one person's tool: one set of limits, configurable from the server command line).
DEFAULT_LIMITS = {'document_chars': 2_000_000, 'documents_per_owner': 10_000, 'document_bytes_per_owner': 300_000_000}
# Imported bundles are split into "題名 2/5" or "題名(続き2)"; results keep one part per bundle first.
BUNDLE_SUFFIX = re.compile(r'(\s+\d+/\d+|\(続き\d+\))$')
FTS_SCHEMA = "CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(owner UNINDEXED, doc UNINDEXED, title, body, tokenize='unicode61')"

SCHEMA = '''
CREATE TABLE IF NOT EXISTS users(id TEXT PRIMARY KEY, name TEXT UNIQUE NOT NULL, salt BLOB NOT NULL, digest BLOB NOT NULL, role TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS sessions(digest TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES users(id), csrf TEXT NOT NULL, expires REAL NOT NULL);
CREATE TABLE IF NOT EXISTS documents(id TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES users(id), title TEXT NOT NULL, body TEXT NOT NULL, fingerprint TEXT NOT NULL, created REAL NOT NULL, UNIQUE(owner,fingerprint));
CREATE TABLE IF NOT EXISTS reports(id TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES users(id), payload TEXT NOT NULL, created REAL NOT NULL);
CREATE TABLE IF NOT EXISTS preferences(owner TEXT PRIMARY KEY REFERENCES users(id), payload TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS login_links(digest TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES users(id), expires REAL NOT NULL);
CREATE TABLE IF NOT EXISTS document_versions(owner TEXT PRIMARY KEY, version INTEGER NOT NULL);
'''

SESSION_SECONDS = 7 * 24 * 3600
LINK_SECONDS = 10 * 60  # a one-time login link is usable for 10 minutes
LINK_SESSION_SECONDS = 30 * 24 * 3600  # a device that opened the link stays signed in for 30 days
LINK_TOKEN = re.compile(r'[A-Za-z0-9_-]{43}')

class Store:
    def __init__(self, path, limits=None):
        self.path = Path(path)
        # Per-owner caches, each kept with the owner's document version (document_versions: +1 in the same transaction as every
        # add / replace / delete / restore, by this process or another one such as the MCP entry or the inbox import).
        self._used = {}  # owner -> (version, bytes of documents); a full scan per insert made 10,000 inserts quadratic
        self._listing = {}  # owner -> (version, document list); length(body) over 300MB costs a second
        self.limits = {**DEFAULT_LIMITS, **{k: int(v) for k, v in (limits or {}).items() if k in DEFAULT_LIMITS and v}}
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        if self.path.is_symlink():
            raise ValueError('database_symlink_refused')
        with self.connect() as db:
            db.executescript(SCHEMA)
            self.fts = self._ensure_index(db)
        os.chmod(self.path, 0o600)

    def _ensure_index(self, db):
        """FTS5 index for the private search; built once for rows that predate it. False when this SQLite lacks FTS5 (scan fallback)."""
        try:
            db.execute(FTS_SCHEMA)
        except sqlite3.OperationalError:
            return False
        # Index rows share the document's rowid, so a search can read match lists by rowid alone (never an FTS content row).
        # An index from an older layout (its own rowids) is rebuilt once.
        aligned = db.execute('SELECT count(*) FROM documents_fts f JOIN documents d ON d.rowid=f.rowid AND d.id=f.doc').fetchone()[0]
        if aligned != db.execute('SELECT count(*) FROM documents_fts').fetchone()[0]:
            db.execute('DELETE FROM documents_fts')
        missing = db.execute('SELECT rowid,id,owner,title,body FROM documents WHERE rowid NOT IN (SELECT rowid FROM documents_fts)')
        for row in missing.fetchall():
            db.execute('INSERT INTO documents_fts(rowid,owner,doc,title,body) VALUES(?,?,?,?,?)', (row['rowid'], row['owner'], row['id'], fts_text(row['title']), fts_text(row['body'])))
        return True

    def _index_document(self, db, owner, identifier, title, body):
        if self.fts:
            db.execute('INSERT INTO documents_fts(rowid,owner,doc,title,body) SELECT rowid,?,?,?,? FROM documents WHERE id=?', (owner, identifier, fts_text(title), fts_text(body), identifier))

    @staticmethod
    def _version(db, owner):
        row = db.execute('SELECT version FROM document_versions WHERE owner=?', (owner,)).fetchone()
        return row[0] if row else 0

    def _bump(self, db, owner):
        """A document change: the version moves on in this transaction, so every process's caches for this owner go stale."""
        db.execute('INSERT INTO document_versions VALUES(?,1) ON CONFLICT(owner) DO UPDATE SET version=version+1', (owner,))
        self._used.pop(owner, None); self._listing.pop(owner, None)
        return self._version(db, owner)

    @contextmanager
    def connect(self):
        db = sqlite3.connect(self.path, timeout=10)
        db.row_factory = sqlite3.Row
        db.execute('PRAGMA foreign_keys=ON')
        try:
            with db:
                yield db
        finally:
            db.close()

    def has_users(self):
        with self.connect() as db:
            return bool(db.execute('SELECT 1 FROM users LIMIT 1').fetchone())

    def register(self, name, password, initial_only=True):
        if not isinstance(name, str) or not name.strip() or len(name) > 64:
            raise ValueError('invalid_name')
        if not isinstance(password, str) or not 12 <= len(password) <= 256:
            raise ValueError('password_length')
        salt = secrets.token_bytes(16)
        digest = hashlib.pbkdf2_hmac('sha256', password.encode(), salt, 600000)
        user = secrets.token_hex(16)
        with self.connect() as db:
            db.execute('BEGIN IMMEDIATE')
            if initial_only and db.execute('SELECT 1 FROM users LIMIT 1').fetchone():
                raise ValueError('setup_finished')
            role='member' if db.execute('SELECT 1 FROM users LIMIT 1').fetchone() else 'admin'
            db.execute('INSERT INTO users VALUES(?,?,?,?,?)', (user, name.strip(), salt, digest,role))
        return user

    def login(self, name, password):
        if not isinstance(name, str) or not isinstance(password, str) or len(password) > 256:
            return None
        with self.connect() as db:
            row = db.execute('SELECT * FROM users WHERE name=?', (name,)).fetchone()
            salt = bytes(row['salt']) if row else b'local-dummy-salt!'
            actual = hashlib.pbkdf2_hmac('sha256', password.encode(), salt, 600000)
            expected = bytes(row['digest']) if row else bytes(32)
            if not hmac.compare_digest(actual, expected) or row is None:
                return None
            return self._new_session(db, row, SESSION_SECONDS)

    @staticmethod
    def _new_session(db, row, seconds):
        token, csrf = secrets.token_urlsafe(32), secrets.token_urlsafe(24)
        db.execute('INSERT INTO sessions VALUES(?,?,?,?)', (hashlib.sha256(token.encode()).hexdigest(), row['id'], csrf, time.time()+seconds))
        return {'token': token, 'csrf': csrf, 'user': {'id': row['id'], 'name': row['name'],'role':row['role']}}

    def first_admin(self):
        with self.connect() as db:
            row = db.execute("SELECT id,name,role FROM users WHERE role='admin' ORDER BY rowid LIMIT 1").fetchone()
            return dict(row) if row else None

    def local_session(self):
        """Session for the first admin (this PC only; the caller checks that). Finishes first-run setup when needed."""
        if not self.has_users():
            try:self.register('自分', secrets.token_urlsafe(24))
            except ValueError as error:
                if str(error) != 'setup_finished':raise
        with self.connect() as db:
            row = db.execute("SELECT id,name,role FROM users WHERE role='admin' ORDER BY rowid LIMIT 1").fetchone()
            return self._new_session(db, row, SESSION_SECONDS) if row else None

    def issue_login_link(self, owner):
        """One-time login link token for an admin. Only its digest is stored."""
        token = secrets.token_urlsafe(32)
        with self.connect() as db:
            row = db.execute('SELECT role FROM users WHERE id=?', (owner,)).fetchone()
            if not row or row['role'] != 'admin':
                raise PermissionError('not_admin')
            db.execute('DELETE FROM login_links WHERE expires<=?', (time.time(),))
            db.execute('INSERT INTO login_links VALUES(?,?,?)', (hashlib.sha256(token.encode()).hexdigest(), owner, time.time()+LINK_SECONDS))
        return token

    def use_login_link(self, token):
        """Consume a one-time link: it is deleted whether or not it is still valid."""
        if not isinstance(token, str) or not LINK_TOKEN.fullmatch(token):
            return None
        with self.connect() as db:
            db.execute('BEGIN IMMEDIATE')
            digest = hashlib.sha256(token.encode()).hexdigest()
            link = db.execute('SELECT owner,expires FROM login_links WHERE digest=?', (digest,)).fetchone()
            if not link:
                return None
            db.execute('DELETE FROM login_links WHERE digest=?', (digest,))
            if link['expires'] <= time.time():
                return None
            row = db.execute('SELECT id,name,role FROM users WHERE id=?', (link['owner'],)).fetchone()
            return self._new_session(db, row, LINK_SESSION_SECONDS) if row else None

    def session(self, token):
        if not token or len(token) > 128:
            return None
        with self.connect() as db:
            row = db.execute('SELECT s.owner,s.csrf,u.name,u.role FROM sessions s JOIN users u ON u.id=s.owner WHERE s.digest=? AND s.expires>?', (hashlib.sha256(token.encode()).hexdigest(), time.time())).fetchone()
            return dict(row) if row else None

    def logout(self, token):
        with self.connect() as db:
            db.execute('UPDATE sessions SET expires=0 WHERE digest=?', (hashlib.sha256(token.encode()).hexdigest(),))

    def add_document(self, owner, title, body):
        if not isinstance(title, str) or not title.strip() or len(title) > 200:
            raise ValueError('invalid_title')
        if not isinstance(body, str) or not body.strip() or len(body) > self.limits['document_chars']:
            raise ValueError('document_length')
        title=title.strip()
        fingerprint = hashlib.sha256((title+'\0'+body).encode()).hexdigest()
        with self.connect() as db:
            db.execute('BEGIN IMMEDIATE')
            row = db.execute('SELECT id FROM documents WHERE owner=? AND fingerprint=?', (owner,fingerprint)).fetchone()
            if row:
                return row['id']
            if db.execute('SELECT count(*) FROM documents WHERE owner=?', (owner,)).fetchone()[0] >= self.limits['documents_per_owner']:
                raise ValueError('document_limit')
            used=self._bytes_used(db, owner); added=len(title.encode())+len(body.encode())
            if used+added>self.limits['document_bytes_per_owner']:raise ValueError('document_limit')
            identifier = secrets.token_hex(16)
            db.execute('INSERT INTO documents VALUES(?,?,?,?,?,?)', (identifier,owner,title.strip(),body,fingerprint,time.time()))
            self._index_document(db, owner, identifier, title, body)
            cache = (self._bump(db, owner), used+added)
        self._used[owner] = cache  # only after the commit
        return identifier

    def document_id_for(self, owner, title, body):
        """The id of this owner's document with exactly this title and body (the app's fingerprint), or None."""
        if not isinstance(title, str) or not isinstance(body, str):
            return None
        fingerprint = hashlib.sha256((title.strip()+'\0'+body).encode()).hexdigest()
        with self.connect() as db:
            row = db.execute('SELECT id FROM documents WHERE owner=? AND fingerprint=?', (owner, fingerprint)).fetchone()
            return row['id'] if row else None

    def delete_document(self, owner, identifier):
        """Deletes one document of this owner only (row and index row). True when a row went away."""
        if not isinstance(identifier, str) or not identifier:
            return False
        with self.connect() as db:
            db.execute('BEGIN IMMEDIATE')
            row = db.execute('SELECT rowid FROM documents WHERE owner=? AND id=?', (owner, identifier)).fetchone()
            if not row:
                return False
            if self.fts:
                db.execute('DELETE FROM documents_fts WHERE rowid=?', (row[0],))
            db.execute('DELETE FROM documents WHERE rowid=?', (row[0],))
            self._bump(db, owner)
            return True

    def replace_document(self, owner, identifier, body, title=None):
        """Replaces the body (and, when given, the title) of one document of this owner, in place: same id, the old body and its
        index row are gone in the same transaction. None when the id is not this owner's. replaced=0 when nothing changed."""
        if title is not None and (not isinstance(title, str) or not title.strip() or len(title) > 200):
            raise ValueError('invalid_title')
        if not isinstance(body, str) or not body.strip() or len(body) > self.limits['document_chars']:
            raise ValueError('document_length')
        if not isinstance(identifier, str) or not identifier:
            return None
        with self.connect() as db:
            db.execute('BEGIN IMMEDIATE')
            row = db.execute('SELECT rowid,title,length(CAST(title AS BLOB))+length(CAST(body AS BLOB)) AS size FROM documents WHERE owner=? AND id=?', (owner, identifier)).fetchone()
            if not row:
                return None
            title = row['title'] if title is None else title.strip()
            fingerprint = hashlib.sha256((title+'\0'+body).encode()).hexdigest()
            same = db.execute('SELECT id FROM documents WHERE owner=? AND fingerprint=?', (owner, fingerprint)).fetchone()
            if same and same['id'] == identifier:
                return {'id': identifier, 'replaced': 0}
            if same:
                raise ValueError('invalid_request')  # another document already has exactly this title and body
            used = self._bytes_used(db, owner); size = len(title.encode())+len(body.encode())
            if used-row['size']+size > self.limits['document_bytes_per_owner']:
                raise ValueError('document_limit')
            db.execute('UPDATE documents SET title=?,body=?,fingerprint=?,created=? WHERE rowid=?', (title, body, fingerprint, time.time(), row['rowid']))
            if self.fts:
                db.execute('DELETE FROM documents_fts WHERE rowid=?', (row['rowid'],))
                db.execute('INSERT INTO documents_fts(rowid,owner,doc,title,body) VALUES(?,?,?,?,?)', (row['rowid'], owner, identifier, fts_text(title), fts_text(body)))
            cache = (self._bump(db, owner), used-row['size']+size)
        self._used[owner] = cache  # only after the commit
        return {'id': identifier, 'replaced': 1}

    def _bytes_used(self, db, owner):
        version = self._version(db, owner); cached = self._used.get(owner)
        if cached and cached[0] == version:
            return cached[1]
        used = db.execute('SELECT coalesce(sum(length(CAST(title AS BLOB))+length(CAST(body AS BLOB))),0) FROM documents WHERE owner=?',(owner,)).fetchone()[0]
        self._used[owner] = (version, used)
        return used

    def documents(self, owner, full=False):
        with self.connect() as db:
            if full:
                return [dict(r) for r in db.execute('SELECT id,title,body,created FROM documents WHERE owner=? ORDER BY created DESC', (owner,))]
            # listing never loads bodies, and is computed once per change (10,000 documents / 300MB must list in milliseconds)
            version = self._version(db, owner); cached = self._listing.get(owner)
            if not cached or cached[0] != version:
                cached = self._listing[owner] = (version, [dict(r) for r in db.execute('SELECT id,title,length(body) AS characters,created FROM documents WHERE owner=? ORDER BY created DESC', (owner,))])
            return [dict(r) for r in cached[1]]

    def document_count(self, owner):
        with self.connect() as db:
            return db.execute('SELECT count(*) FROM documents WHERE owner=?', (owner,)).fetchone()[0]

    def search_documents(self, owner, query, limit=10, candidates=30):
        """Owner's documents that match the query, best first.
        A document is a hit only when more than half of the query's units (words / bigrams) occur in it: one shared
        common bigram is not a match, and a query nobody wrote about returns nothing.
        FTS5: the hit set is exact and uncapped - one posting-list scan per unit counts, per document, how many units it has
        (no body is read for that). The hits are then taken in rank order: first the documents that contain every run of the
        query (phrase = substring), then any-token matches, each ordered by BM25 with the title weighted 8x the body
        (length-normalised, so a short exact page beats a long bundle); only the chosen `candidates` bodies are loaded.
        Finally one part per imported bundle before second parts of the same bundle. Without FTS5: the historical scan with
        the same hit rule."""
        wanted = terms(query)
        if not wanted:
            return []
        needed = len(wanted) // 2 + 1
        with self.connect() as db:
            if self.fts:
                # whose rows: the owner's document rowids (the first columns of a documents row; bodies are not read)
                mine = {r[0] for r in db.execute('SELECT rowid FROM documents WHERE owner=?', (owner,))}
                units = collections.Counter()
                for unit in wanted:
                    for (rowid,) in db.execute('SELECT rowid FROM documents_fts WHERE documents_fts MATCH ?', ('"' + unit + '"',)):
                        if rowid in mine:
                            units[rowid] += 1
                hits = {rowid for rowid, n in units.items() if n >= needed}
                ranked, seen = [], set()
                order = 'SELECT rowid FROM documents_fts WHERE documents_fts MATCH ? ORDER BY bm25(documents_fts,0,0,8.0,1.0)'
                for expression in ((fts_query_all(query), fts_query(query)) if hits else ()):
                    for (rowid,) in db.execute(order, (expression,)):
                        if len(ranked) >= candidates:
                            break
                        if rowid in hits and rowid not in seen:
                            seen.add(rowid)
                            row = db.execute('SELECT id,title,body,created FROM documents WHERE owner=? AND rowid=?', (owner, rowid)).fetchone()
                            if row:
                                ranked.append(dict(row))
                    if len(ranked) >= candidates:
                        break
            else:
                scored = []
                for r in db.execute('SELECT id,title,body,created FROM documents WHERE owner=?', (owner,)):
                    score = len(wanted & terms(r['title'] + ' ' + r['body']))
                    if score >= needed:
                        scored.append((score, r['created'], dict(r)))
                scored.sort(key=lambda x: (-x[0], -x[1]))
                ranked = [x[2] for x in scored[:candidates]]
        first, rest, seen = [], [], set()
        for doc in ranked:
            key = BUNDLE_SUFFIX.sub('', doc['title']).strip()
            (rest if key in seen else first).append(doc)
            seen.add(key)
        return (first + rest)[:limit]

    def document(self, owner, identifier):
        with self.connect() as db:
            row = db.execute('SELECT id,title,body,created FROM documents WHERE owner=? AND id=?', (owner,identifier)).fetchone()
            return dict(row) if row else None

    def save_report(self, owner, payload):
        identifier = secrets.token_hex(16)
        encoded=json.dumps(payload,ensure_ascii=False)
        with self.connect() as db:
            db.execute('BEGIN IMMEDIATE')
            used=db.execute('SELECT coalesce(sum(length(CAST(payload AS BLOB))),0) FROM reports WHERE owner=?',(owner,)).fetchone()[0]
            if used+len(encoded.encode())>10000000:raise ValueError('report_limit')
            db.execute('INSERT INTO reports VALUES(?,?,?,?)', (identifier,owner,encoded,time.time()))
        return identifier

    def reports(self, owner):
        rows=[]
        with self.connect() as db:
            for r in db.execute('SELECT * FROM reports WHERE owner=? ORDER BY created DESC LIMIT 100', (owner,)):
                p=json.loads(r['payload']); sources=p.get('sources') or []
                if p.get('scope')=='private': usable=len(sources)
                elif 'usable_sources' in p: usable=p['usable_sources']
                else: usable=(p.get('public') or {}).get('usable_sources',0) + len((p.get('private') or {}).get('sources') or [])
                rows.append({'id':r['id'],'created':r['created'],'query':p.get('query',''),'scope':p.get('scope',''),'status':p.get('status',''),'usable':usable,'total':len(sources)})
        return rows

    def report(self, owner, identifier):
        with self.connect() as db:
            row = db.execute('SELECT payload FROM reports WHERE owner=? AND id=?', (owner,identifier)).fetchone()
            return json.loads(row['payload']) if row else None

    def delete_report(self, owner, identifier):
        """Deletes one saved research of this owner only. True when a row went away."""
        if not isinstance(identifier, str) or not identifier:
            return False
        with self.connect() as db:
            return db.execute('DELETE FROM reports WHERE owner=? AND id=?', (owner, identifier)).rowcount == 1

    def delete_reports(self, owner):
        """Deletes every saved research of this owner. Returns how many rows went away."""
        with self.connect() as db:
            return db.execute('DELETE FROM reports WHERE owner=?', (owner,)).rowcount

    def preferences(self, owner, value=None):
        with self.connect() as db:
            if value is not None:
                if value.get('profile') not in ('general','development','sales','sns'):
                    raise ValueError('invalid_profile')
                db.execute('INSERT INTO preferences VALUES(?,?) ON CONFLICT(owner) DO UPDATE SET payload=excluded.payload', (owner,json.dumps({'profile':value['profile']})))
            row = db.execute('SELECT payload FROM preferences WHERE owner=?', (owner,)).fetchone()
            return json.loads(row[0]) if row else {'profile':'general'}

    def export(self, owner):
        with self.connect() as db:
            reports=[{'payload':json.loads(r['payload']),'created':r['created']} for r in db.execute('SELECT payload,created FROM reports WHERE owner=?',(owner,))]
        return {'format':'rustxng-private-backup-v1','documents':self.documents(owner,True),'preferences':self.preferences(owner),'reports':reports}

    def export_to(self, owner, out):
        """The same backup as export(), written to a binary file one document at a time (a 300MB library is never one string)."""
        preferences = self.preferences(owner)
        with self.connect() as db:
            reports=[{'payload':json.loads(r['payload']),'created':r['created']} for r in db.execute('SELECT payload,created FROM reports WHERE owner=?',(owner,))]
            out.write(b'{"format": "rustxng-private-backup-v1", "documents": [')
            first = True
            for r in db.execute('SELECT id,title,body,created FROM documents WHERE owner=? ORDER BY created DESC', (owner,)):
                out.write((b'' if first else b', ') + json.dumps(dict(r), ensure_ascii=False).encode()); first = False
            out.write(b'], "preferences": ' + json.dumps(preferences).encode() + b', "reports": ' + json.dumps(reports, ensure_ascii=False).encode() + b'}')

    def restore(self, owner, payload):
        """Restores a backup given as a parsed object."""
        if not isinstance(payload,dict):
            raise ValueError('invalid_backup')
        members = ((k, iter(copy.deepcopy(v) if k == 'reports' else v) if k in ('documents','reports') and isinstance(v,list) else v) for k, v in payload.items())
        return self._restore(owner, members)

    def restore_stream(self, owner, fileobj):
        """Restores a backup read from a binary file, one document at a time: memory stays about one document whatever the size
        of the file, and the limits are checked on the decoded documents (not on the JSON, where escapes such as \\ double the size)."""
        reader = _BackupReader(fileobj, element_cap=12*self.limits['document_chars']+4096, report_cap=6*10_000_000+4096)
        try:
            return self._restore(owner, reader.members())
        except UnicodeDecodeError:
            raise ValueError('invalid_backup') from None

    def _restore(self, owner, members):
        """One transaction: every document and report is validated before the commit; any refusal leaves the library as it was.
        Never trusts exported owner/id. Duplicates (same title and body) are not added twice."""
        def invalid():
            return ValueError('invalid_backup')
        with self.connect() as db:
            # A full library is hundreds of MB in one transaction. Spilling dirty pages mid-transaction would take the exclusive
            # lock for all of it (every other request's session read waits, then fails); kept in memory, the lock is held at commit only.
            db.execute('PRAGMA cache_spill=OFF')
            db.execute('BEGIN IMMEDIATE')
            self._bump(db, owner)  # caches of every process are dropped; counted again from the rows
            existing = {r[0]: r[1] for r in db.execute('SELECT fingerprint,id FROM documents WHERE owner=?',(owner,))}
            count = len(existing)
            used = db.execute('SELECT coalesce(sum(length(CAST(title AS BLOB))+length(CAST(body AS BLOB))),0) FROM documents WHERE owner=?',(owner,)).fetchone()[0]
            fmt = pref = None; reports = []; report_bytes = 0; id_map = {}; added = 0; seen = set()
            for key, value in members:
                if key in seen:
                    raise invalid()
                seen.add(key)
                if key == 'format':
                    fmt = value
                elif key == 'preferences':
                    pref = value
                elif key == 'documents':
                    if isinstance(value, (dict, str, int, float, bool)) or value is None:
                        raise invalid()
                    n = 0
                    for doc in value:
                        n += 1
                        if n > self.limits['documents_per_owner']:
                            raise invalid()
                        if not isinstance(doc,dict) or not isinstance(doc.get('title'),str) or not 0<len(doc['title'])<=200 or not isinstance(doc.get('body'),str) or not 0<len(doc['body'])<=self.limits['document_chars']:
                            raise invalid()
                        title = doc['title'].strip(); body = doc['body']
                        if not title or not body.strip():
                            raise invalid()
                        fp = hashlib.sha256((title+'\0'+body).encode()).hexdigest()
                        target = existing.get(fp)
                        if target is None:
                            size = len(title.encode())+len(body.encode())
                            if count+1 > self.limits['documents_per_owner'] or used+size > self.limits['document_bytes_per_owner']:
                                raise ValueError('document_limit')
                            target = secrets.token_hex(16)
                            db.execute('INSERT INTO documents VALUES(?,?,?,?,?,?)',(target,owner,title,body,fp,time.time()))
                            self._index_document(db, owner, target, title, body)
                            existing[fp] = target; count += 1; used += size; added += 1
                        if isinstance(doc.get('id'),str):
                            id_map[doc['id']] = target
                elif key == 'reports':
                    if isinstance(value, (dict, str, int, float, bool)) or value is None:
                        raise invalid()
                    for report in value:
                        if len(reports) >= 10000 or not isinstance(report,dict) or not isinstance(report.get('payload'),dict):
                            raise invalid()
                        item = report['payload']
                        if not isinstance(item.get('query'),str) or len(item['query'])>500 or not isinstance(item.get('sources'),list):
                            raise invalid()
                        for source in item['sources']:
                            if not isinstance(source,dict):
                                raise invalid()
                            url = source.get('url','')
                            if url and (not isinstance(url,str) or not url.startswith(('https://','http://'))):
                                raise invalid()
                        report_bytes += len(json.dumps(item, ensure_ascii=False).encode())
                        if report_bytes > 10000000:
                            raise ValueError('report_limit')
                        reports.append(report)
            if fmt != 'rustxng-private-backup-v1' or 'documents' not in seen:
                raise invalid()
            pref = {'profile':'general'} if pref is None else pref
            if not isinstance(pref,dict) or pref.get('profile') not in ('general','development','sales','sns'):
                raise invalid()
            def remap(value):
                if isinstance(value,dict):
                    if value.get('scope')=='private' and value.get('id') in id_map:value['id']=id_map[value['id']]
                    for child in value.values():remap(child)
                elif isinstance(value,list):
                    for child in value:remap(child)
            remap(reports)
            old_reports={r[0] for r in db.execute('SELECT payload FROM reports WHERE owner=?',(owner,))}
            additions_reports={json.dumps(r['payload'],ensure_ascii=False) for r in reports}-old_reports
            if sum(len(x.encode()) for x in old_reports|additions_reports)>10000000:raise ValueError('report_limit')
            for text in additions_reports:
                db.execute('INSERT INTO reports VALUES(?,?,?,?)',(secrets.token_hex(16),owner,text,time.time()))
            db.execute('INSERT INTO preferences VALUES(?,?) ON CONFLICT(owner) DO UPDATE SET payload=excluded.payload',(owner,json.dumps({'profile':pref['profile']})))
        return added

    def search_reports(self, owner, query, limit=5):
        """Saved research whose question or text has more than half of the query's units (the same hit rule as documents),
        best first (matched units, then newest). Reads at most 10MB (the per-person cap on saved research)."""
        wanted = terms(query)
        if not wanted:
            return []
        needed = len(wanted) // 2 + 1; scored = []
        with self.connect() as db:
            for r in db.execute('SELECT id,payload,created FROM reports WHERE owner=?', (owner,)):
                try:
                    p = json.loads(r['payload'])
                except ValueError:
                    continue
                text = (p.get('query') or '') + '\n' + (p.get('markdown') or '')
                score = len(wanted & terms(text))
                if score >= needed:
                    scored.append((score, r['created'], {'id': r['id'], 'query': p.get('query', ''), 'created': r['created'], 'text': text}))
        scored.sort(key=lambda x: (-x[0], -x[1]))
        return [x[2] for x in scored[:limit]]


_WS = re.compile(r'[ \t\r\n]*')


class _BackupReader:
    """Reads a backup (one JSON object) from a binary file in windows: the members one by one, and the "documents" / "reports"
    arrays one element at a time. A value is decoded by the json module's own scanner on a window that doubles until the value
    fits (so a cut value costs at most twice its length); no value may be longer than its cap."""
    def __init__(self, fileobj, element_cap, report_cap, other_cap=1_000_000):
        self.file = fileobj; self.decoder = codecs.getincrementaldecoder('utf-8')('strict'); self.json = json.JSONDecoder()
        self.buf = ''; self.pos = 0; self.eof = False
        self.caps = {'documents': element_cap, 'reports': report_cap}; self.other_cap = other_cap

    def _fill(self, need):
        while not self.eof and len(self.buf) - self.pos < need:
            if self.pos > (1 << 20) and self.pos * 2 > len(self.buf):
                self.buf = self.buf[self.pos:]; self.pos = 0
            chunk = self.file.read(max(need - (len(self.buf) - self.pos), 1 << 20))
            if chunk:
                self.buf += self.decoder.decode(chunk)
            else:
                self.buf += self.decoder.decode(b'', final=True); self.eof = True

    def _peek(self):
        while True:
            self._fill(1)
            self.pos = _WS.match(self.buf, self.pos).end()
            if self.pos < len(self.buf):
                return self.buf[self.pos]
            if self.eof:
                return ''

    def _expect(self, char):
        if self._peek() != char:
            raise ValueError('invalid_backup')
        self.pos += 1

    def _value(self, cap):
        self._peek(); window = 1 << 16
        while True:
            self._fill(window)
            try:
                value, end = self.json.raw_decode(self.buf, self.pos)
            except json.JSONDecodeError:
                end = None
            available = len(self.buf) - self.pos
            # not decodable yet, or a number that may go on past the window ("12" of "123"): read more, up to the cap
            if end is None or (end == len(self.buf) and not self.eof):
                if self.eof or available >= cap:
                    raise ValueError('invalid_backup')
                window = min(cap, max(window * 2, available * 2)); continue
            if end - self.pos > cap:
                raise ValueError('invalid_backup')
            self.pos = end
            return value

    def _array(self, cap):
        self._expect('[')
        if self._peek() == ']':
            self.pos += 1; return
        while True:
            yield self._value(cap)
            char = self._peek()
            if char == ']':
                self.pos += 1; return
            if char != ',':
                raise ValueError('invalid_backup')
            self.pos += 1

    def members(self):
        self._expect('{')
        if self._peek() == '}':
            self.pos += 1
        else:
            while True:
                if self._peek() != '"':
                    raise ValueError('invalid_backup')
                key = self._value(4096)
                self._expect(':')
                if key in self.caps:
                    elements = self._array(self.caps[key])
                    yield key, elements
                    for _ in elements:  # the consumer reads them all; anything left is still checked
                        pass
                else:
                    yield key, self._value(self.other_cap)
                char = self._peek()
                if char == '}':
                    self.pos += 1; break
                if char != ',':
                    raise ValueError('invalid_backup')
                self.pos += 1
        if self._peek() != '':
            raise ValueError('invalid_backup')
