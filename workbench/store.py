"""Local account-scoped storage. No external connections or request-content logging."""
import hashlib
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

SCHEMA = '''
CREATE TABLE IF NOT EXISTS users(id TEXT PRIMARY KEY, name TEXT UNIQUE NOT NULL, salt BLOB NOT NULL, digest BLOB NOT NULL, role TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS sessions(digest TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES users(id), csrf TEXT NOT NULL, expires REAL NOT NULL);
CREATE TABLE IF NOT EXISTS documents(id TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES users(id), title TEXT NOT NULL, body TEXT NOT NULL, fingerprint TEXT NOT NULL, created REAL NOT NULL, UNIQUE(owner,fingerprint));
CREATE TABLE IF NOT EXISTS reports(id TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES users(id), payload TEXT NOT NULL, created REAL NOT NULL);
CREATE TABLE IF NOT EXISTS preferences(owner TEXT PRIMARY KEY REFERENCES users(id), payload TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS login_links(digest TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES users(id), expires REAL NOT NULL);
'''

SESSION_SECONDS = 7 * 24 * 3600
LINK_SECONDS = 10 * 60  # a one-time login link is usable for 10 minutes
LINK_SESSION_SECONDS = 30 * 24 * 3600  # a device that opened the link stays signed in for 30 days
LINK_TOKEN = re.compile(r'[A-Za-z0-9_-]{43}')

class Store:
    def __init__(self, path):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        if self.path.is_symlink():
            raise ValueError('database_symlink_refused')
        with self.connect() as db:
            db.executescript(SCHEMA)
        os.chmod(self.path, 0o600)

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
        if not isinstance(body, str) or not body.strip() or len(body) > 100000:
            raise ValueError('document_length')
        title=title.strip()
        fingerprint = hashlib.sha256((title+'\0'+body).encode()).hexdigest()
        with self.connect() as db:
            db.execute('BEGIN IMMEDIATE')
            row = db.execute('SELECT id FROM documents WHERE owner=? AND fingerprint=?', (owner,fingerprint)).fetchone()
            if row:
                return row['id']
            if db.execute('SELECT count(*) FROM documents WHERE owner=?', (owner,)).fetchone()[0] >= 500:
                raise ValueError('document_limit')
            used=db.execute('SELECT coalesce(sum(length(CAST(title AS BLOB))+length(CAST(body AS BLOB))),0) FROM documents WHERE owner=?',(owner,)).fetchone()[0]
            if used+len(title.encode())+len(body.encode())>1500000:raise ValueError('document_limit')
            identifier = secrets.token_hex(16)
            db.execute('INSERT INTO documents VALUES(?,?,?,?,?,?)', (identifier,owner,title.strip(),body,fingerprint,time.time()))
            return identifier

    def documents(self, owner, full=False):
        with self.connect() as db:
            rows = db.execute('SELECT id,title,body,created FROM documents WHERE owner=? ORDER BY created DESC', (owner,)).fetchall()
            return [dict(r) if full else {'id':r['id'],'title':r['title'],'characters':len(r['body']),'created':r['created']} for r in rows]

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

    def restore(self, owner, payload):
        if not isinstance(payload,dict) or payload.get('format') != 'rustxng-private-backup-v1':
            raise ValueError('invalid_backup')
        docs = payload.get('documents')
        if not isinstance(docs,list) or len(docs)>500:
            raise ValueError('invalid_backup')
        for doc in docs:
            if not isinstance(doc,dict) or not isinstance(doc.get('title'),str) or not 0<len(doc['title'])<=200 or not isinstance(doc.get('body'),str) or not 0<len(doc['body'])<=100000:
                raise ValueError('invalid_backup')
        pref=payload.get('preferences',{'profile':'general'})
        if not isinstance(pref,dict) or pref.get('profile') not in ('general','development','sales','sns'):raise ValueError('invalid_backup')
        reports=payload.get('reports',[])
        if not isinstance(reports,list) or len(reports)>10000:raise ValueError('invalid_backup')
        for report in reports:
            if not isinstance(report,dict) or not isinstance(report.get('payload'),dict):raise ValueError('invalid_backup')
            item=report['payload']
            if not isinstance(item.get('query'),str) or len(item['query'])>500 or not isinstance(item.get('sources'),list):raise ValueError('invalid_backup')
            for source in item['sources']:
                if not isinstance(source,dict):raise ValueError('invalid_backup')
                url=source.get('url','')
                if url and (not isinstance(url,str) or not url.startswith(('https://','http://'))):raise ValueError('invalid_backup')
        # Validate and insert the whole bundle in one transaction. Never trust exported owner/id.
        with self.connect() as db:
            db.execute('BEGIN IMMEDIATE')
            existing = {r[0] for r in db.execute('SELECT fingerprint FROM documents WHERE owner=?',(owner,))}
            additions = {}
            for doc in docs:
                doc={**doc,'title':doc['title'].strip()}
                if not doc['title'] or not doc['body'].strip():raise ValueError('invalid_backup')
                fp=hashlib.sha256((doc['title']+'\0'+doc['body']).encode()).hexdigest()
                if fp not in existing: additions[fp]=doc
            if len(existing)+len(additions)>500: raise ValueError('document_limit')
            used=db.execute('SELECT coalesce(sum(length(CAST(title AS BLOB))+length(CAST(body AS BLOB))),0) FROM documents WHERE owner=?',(owner,)).fetchone()[0]
            if used+sum(len(d['title'].encode())+len(d['body'].encode()) for d in additions.values())>1500000:raise ValueError('document_limit')
            for fp,doc in additions.items():
                db.execute('INSERT INTO documents VALUES(?,?,?,?,?,?)',(secrets.token_hex(16),owner,doc['title'],doc['body'],fp,time.time()))
            id_map={}
            for doc in docs:
                fp=hashlib.sha256((doc['title'].strip()+'\0'+doc['body']).encode()).hexdigest()
                row=db.execute('SELECT id FROM documents WHERE owner=? AND fingerprint=?',(owner,fp)).fetchone()
                if row and isinstance(doc.get('id'),str):id_map[doc['id']]=row[0]
            reports=copy.deepcopy(reports)
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
        return len(additions)
