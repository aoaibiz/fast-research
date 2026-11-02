"""Loopback-only UI/API. All private records are scoped by a server-validated session."""
import argparse
import collections
import ctypes
import hashlib
import hmac
import http.cookies
import json
import logging
import os
from pathlib import Path
import queue
import secrets
import shutil
import signal
import sqlite3
import subprocess
import sys
import threading
import time
import urllib.error
from .ownership import listening_pid, guard_for_state
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit
from .store import Store, LINK_SESSION_SECONDS, LINK_SECONDS
from . import qr
from . import digest, notes
from .core import PROFILES, Research, ResearchError

ROOT=Path(__file__).resolve().parent
MAX_INPUT=16*1024*1024
MESSAGES={
 'invalid_query':'検索語を1〜500文字で入力してください。','invalid_profile':'調査の種類を選び直してください。',
 'invalid_domains':'対象ドメインを確認してください。URLではなくドメイン名を入力します。',
 'possible_private_query':'メールアドレスや鍵のような文字列を検出しました。社内検索を使うか、公開情報だけの検索語に変更してください。',
 'search_unavailable':'検索エンジンに接続できません。設定画面で検索エンジンを再起動してください。',
 'engine_configuration_error':'検索エンジンの設定を確認してください。','invalid_backend_response':'検索エンジンの応答を読み取れません。設定画面を確認してください。',
 'password_length':'パスワードは12〜256文字で入力してください。','invalid_name':'利用者名を1〜64文字で入力してください。',
 'invalid_title':'資料名を1〜200文字で入力してください。','document_length':'資料本文を1〜{document_chars}文字で入力してください。',
 'report_limit':'保存した調査が容量上限に達しました。バックアップを取得し、エージェントに整理を依頼してください。',
 'invalid_backup':'バックアップの形式を確認してください。','document_limit':'資料は1利用者{documents_per_owner}件・合計{document_mb}MBまでです。',
 'setup_finished':'初期設定済みです。ログインしてください。','invalid_credentials':'利用者名またはパスワードを確認してください。',
 'authentication_required':'ログインしてください。','origin_refused':'この画面を開き直してください。','csrf_refused':'画面を更新して操作をやり直してください。',
 'not_found':'対象が見つからないか、参照する権限がありません。','rate_limited':'操作が集中しています。1分後に試してください。',
 'invalid_request':'入力内容を確認してください。','storage_error':'保存できませんでした。画面に出ている処理IDをエージェントへ伝えてください。',
 'unexpected_error':'処理に失敗しました。画面に出ている処理IDで原因を確認できます。','public_consent_required':'公開Webへ送る検索語であることを確認してください。',
 'backend_version_mismatch':'検索エンジンの版が一致しません。改造版を再起動してください。',
 'backend_start_failed':'検索エンジンを起動できません。ポート競合または起動ログの処理をエージェントに確認させてください。',
 'backend_listen_timeout':'検索エンジンの待受を時間内に確認できませんでした。担当AIにポート競合・lsof・起動時間の確認を依頼してください。',
 'private_term_denied':'登録した社内用語が含まれています。公開Webには送れません。社内資料で検索するか、その語を外してください。',
 'denylist_unreadable':'社内用語の登録ファイルを読み取れないため、公開Webへの検索を止めています。ファイルを確認してください。',
 'backend_missing':'検索エンジンの実行ファイルがありません。エージェントにビルドと復旧を依頼してください。',
 'identity_invalid':'名乗りの設定を確認してください。FAST_RESEARCH_IDENTITY は honest か browser です。',
}

SESSION_SECONDS=7*24*3600
PR_SET_PDEATHSIG=1
# Files the screen may fetch by name (everything else under / goes back to the screen). Icons: the same set our other apps ship
# (manifest + apple-touch-icon 180 + 192/512 + maskable + favicon) so "add to home screen" shows 高速リサーチ with its own icon.
STATIC_FILES={
 '/app.js':('app.js','text/javascript; charset=utf-8'),'/boot.js':('boot.js','text/javascript; charset=utf-8'),'/style.css':('style.css','text/css; charset=utf-8'),
 '/manifest.webmanifest':('manifest.webmanifest','application/manifest+json; charset=utf-8'),
 '/icon-192.png':('icons/icon-192.png','image/png'),'/icon-512.png':('icons/icon-512.png','image/png'),'/icon-512-maskable.png':('icons/icon-512-maskable.png','image/png'),
 '/apple-touch-icon.png':('icons/apple-touch-icon.png','image/png'),'/favicon-32.png':('icons/favicon-32.png','image/png'),'/favicon-16.png':('icons/favicon-16.png','image/png'),
 '/favicon.ico':('icons/favicon.ico','image/x-icon'),'/favicon.svg':('icons/favicon.svg','image/svg+xml; charset=utf-8'),
}

try:  # resolved once, in the parent: nothing is imported or loaded between fork and exec
    _PRCTL=ctypes.CDLL(None,use_errno=True).prctl if sys.platform.startswith('linux') else None
except (OSError,AttributeError):_PRCTL=None

def _die_with_parent():
    """Child side of fork: ask the kernel to SIGTERM us when the parent thread goes away (Linux)."""
    if _PRCTL is not None:_PRCTL(PR_SET_PDEATHSIG,int(signal.SIGTERM),0,0,0)

class _Spawner:
    """PR_SET_PDEATHSIG is bound to the *thread* that forked, and request handlers run in
    short-lived threads. Fork every backend from one thread that lives as long as the process,
    so the child dies with the process (even on SIGKILL) and not with the handler thread."""
    def __init__(self):
        self.jobs=queue.Queue();self.thread=None;self.lock=threading.Lock()
    def popen(self,*args,**kwargs):
        with self.lock:
            if self.thread is None or not self.thread.is_alive():
                self.thread=threading.Thread(target=self._loop,name='backend-spawner',daemon=True);self.thread.start()
        done=queue.Queue();self.jobs.put((args,kwargs,done))
        outcome=done.get()
        if isinstance(outcome,BaseException):raise outcome
        return outcome
    def _loop(self):
        while True:
            args,kwargs,done=self.jobs.get()
            try:done.put(subprocess.Popen(*args,**kwargs))
            except BaseException as error:done.put(error)

SPAWNER=_Spawner()

IDENTITIES=('honest','browser')
def identity_env(environ=None):
    """How the search backend introduces itself to search engines and pages.
    FAST_RESEARCH_IDENTITY=browser (default: the upstream rotating Firefox User-Agent) or honest
    (one fixed User-Agent naming this tool). FAST_RESEARCH_USER_AGENT replaces the honest string.
    The earlier names TALMUD_RESEARCH_IDENTITY / TALMUD_RESEARCH_USER_AGENT are still read."""
    environ=os.environ if environ is None else environ
    identity=(environ.get('FAST_RESEARCH_IDENTITY') or environ.get('TALMUD_RESEARCH_IDENTITY') or 'browser').strip().lower()
    if identity not in IDENTITIES:raise ResearchError('identity_invalid','startup',503)
    env={'RUSTXNG_IDENTITY':identity}
    agent=(environ.get('FAST_RESEARCH_USER_AGENT') or environ.get('TALMUD_RESEARCH_USER_AGENT') or '').strip()
    if agent:
        if len(agent)>256 or any(ord(ch)<0x20 or ord(ch)==0x7f for ch in agent):raise ResearchError('identity_invalid','startup',503)
        env['RUSTXNG_USER_AGENT']=agent
    return env

class Backend:
    def __init__(self,binary,port,log_dir,idle_timeout=None):
        self.binary=Path(binary).resolve();self.port=port;self.child=None;self.lock=threading.Lock();self.log_dir=Path(log_dir)
        # Linux: the kernel ends the child with us, so no idle timeout is needed. Elsewhere
        # (no PDEATHSIG) fall back to an idle timeout so an orphan cannot linger.
        self.idle_timeout=(0 if sys.platform.startswith('linux') else 900) if idle_timeout is None else int(idle_timeout)
        self.stopped=False
    def start(self):
        with self.lock:
            if self.child and self.child.poll() is None:return
            if not self.binary.is_file():raise ResearchError('backend_missing','startup',503)
            self.log_dir.mkdir(parents=True,exist_ok=True,mode=0o700)
            self.instance=secrets.token_hex(24);self.stopped=False
            # A fresh random secret for every launch: never the upstream placeholder in settings.toml.
            env={'PATH':os.defpath,'RUST_LOG':'warn','RUSTXNG_IDLE_TIMEOUT':str(self.idle_timeout),'RUSTXNG_INSTANCE_ID':self.instance,'SEARXNG_SECRET':secrets.token_hex(32)}
            env.update(identity_env())
            with open(self.log_dir/'backend.log','ab') as log:
                os.chmod(self.log_dir/'backend.log',0o600)
                self.child=SPAWNER.popen([str(self.binary),'--bind','127.0.0.1','--port',str(self.port)],cwd=self.binary.parent,env=env,stdout=log,stderr=log,preexec_fn=_die_with_parent)
            deadline=time.monotonic()+8
            while time.monotonic()<deadline:
                if self.child.poll() is not None:raise ResearchError(f'backend_exited_{self.child.returncode}','startup',503)
                if listening_pid(self.port)==self.child.pid:
                    state=self.log_dir/'backend-instance.json'
                    state.write_text(json.dumps({'pid':self.child.pid,'port':self.port,'instance':self.instance}));os.chmod(state,0o600)
                    return
                time.sleep(.1)
            self.child.terminate();self.child.wait(timeout=5)
            raise ResearchError('backend_listen_timeout','startup',503)
    def ensure_owned(self):
        # An idle-timeout exit (non-Linux) is not a failure: start a fresh, re-verified child.
        if self.idle_timeout and not self.stopped and self.child and self.child.poll()==0:self.start()
        if not self.child or self.child.poll() is not None or listening_pid(self.port)!=self.child.pid:
            raise ResearchError('search_unavailable','ownership',503)
        return self.instance
    def stop(self):
        with self.lock:
            self.stopped=True
            if self.child and self.child.poll() is None:
                self.child.terminate()
                try:self.child.wait(timeout=5)
                except subprocess.TimeoutExpired:self.child.kill();self.child.wait()
    def restart(self):self.stop();self.start()

def login_link_url(origin,token):return f'{origin}/#link={token}'

def parse_public_origin(value):
    """--public-origin: one https origin (host[:port], nothing else) that may also reach this server, e.g. a tunnel."""
    try:parts=urlsplit(value)
    except ValueError:raise ValueError('public_origin_invalid') from None
    try:port=parts.port
    except ValueError:raise ValueError('public_origin_invalid') from None
    if parts.scheme!='https' or not parts.hostname or parts.username or parts.password or parts.query or parts.fragment or parts.path not in ('','/'):
        raise ValueError('public_origin_invalid')
    host=parts.hostname.lower()
    if host in ('localhost','127.0.0.1','::1') or host.endswith('.localhost'):raise ValueError('public_origin_invalid')
    hostport=host if port in (None,443) else f'{host}:{port}'
    return f'https://{hostport}',hostport

class Uploads:
    """A backup restore in pieces: start (declared size) -> pieces in order (each <= MAX_INPUT, so a tunnel's request cap does not
    matter) -> finish, which restores from the file on disk in a background thread (the screen asks for the state; no request has
    to stay open for the minutes a full library takes). Files live in <data>/restore-uploads (0700/0600) and go away when done.
    Disk: what every upload still has to write (declared size - received), what a restore sent whole is writing and what a backup in
    the making may write are counted together, with HEADROOM, against the free space before anything new starts (a full disk stops the
    whole box). An upload nobody touched for IDLE seconds is let go, with its file, by whatever asks next."""
    CHUNK=8*1024*1024;IDLE=3600;HEADROOM=64*1024*1024
    def __init__(self,directory,store):
        self.dir=Path(directory);self.dir.mkdir(mode=0o700,exist_ok=True);os.chmod(self.dir,0o700);self.store=store
        for leftover in self.dir.iterdir():  # pieces / exports of a previous run of the server
            if leftover.is_file() and not leftover.is_symlink():leftover.unlink()
        self.items={};self.holds={};self.lock=threading.Lock();self.running=threading.Lock()
    def new_file(self,prefix):
        path=self.dir/f'{prefix}-{secrets.token_hex(16)}';os.close(os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600));return path
    def _drop(self,token):
        item=self.items.pop(token,None)
        if item:item['path'].unlink(missing_ok=True)
    def _expire(self,now):  # call with self.lock held
        for token,item in list(self.items.items()):
            if item['state']!='restoring' and now-item['touched']>self.IDLE:self._drop(token)
    def _reserved(self):  # call with self.lock held: what is still to be written by uploads and by whole restores / backups in flight
        return sum(max(i['size']-i['received'],0) for i in self.items.values() if i['state']=='uploading')+sum(self.holds.values())
    def _check(self,size):  # call with self.lock held
        if shutil.disk_usage(self.dir).free<self._reserved()+size+self.HEADROOM:raise ResearchError('storage_error','storage',507)
    def reserve(self,size):
        """Room for a file of up to `size` bytes that a request is about to write (a restore sent whole, a backup); release() ends it."""
        with self.lock:
            self._expire(time.monotonic());self._check(size)
            token=secrets.token_hex(16);self.holds[token]=size
        return token
    def release(self,token):
        with self.lock:self.holds.pop(token,None)
    def start(self,owner,size):
        with self.lock:
            now=time.monotonic()
            for token,item in list(self.items.items()):
                if item['owner']==owner and item['state']=='uploading':self._drop(token)  # the person's earlier upload is given up
            self._expire(now);self._check(size)
            token=secrets.token_hex(16)
            self.items[token]={'owner':owner,'path':self.new_file('restore'),'size':size,'received':0,'state':'uploading','touched':now,'lock':threading.Lock()}
        return {'ok':True,'upload':token,'size':size,'chunk_bytes':self.CHUNK}
    def get(self,owner,token):
        with self.lock:
            self._expire(time.monotonic())
            item=self.items.get(token)
            return item if item and item['owner']==owner else None
    def append(self,owner,token,offset,data):
        item=self.get(owner,token)
        if not item:raise ResearchError('not_found','authorization',404)
        with item['lock']:
            if item['state']!='uploading' or offset!=item['received']:raise ResearchError('invalid_request','input',409)
            if item['received']+len(data)>item['size']:raise ResearchError('invalid_request','input',413)
            with open(item['path'],'ab') as f:f.write(data)
            item['received']+=len(data);item['touched']=time.monotonic()
            return item['received']
    def finish(self,owner,token):
        item=self.get(owner,token)
        if not item:raise ResearchError('not_found','authorization',404)
        with item['lock']:
            if item['state']!='uploading':return item['state']  # sent again (the answer to the first was lost): it is not started twice
            if item['received']!=item['size']:raise ValueError('invalid_backup')
            if not self.running.acquire(blocking=False):raise ResearchError('rate_limited','input',429)
            item['state']='restoring'
        threading.Thread(target=self._run,args=(item,),name='restore',daemon=True).start()
        return 'restoring'
    def _run(self,item):
        try:
            with open(item['path'],'rb') as f:item['restored']=self.store.restore_stream(item['owner'],f)
            item['state']='done'
        except ValueError as error:item['state']='failed';item['code']=str(error) if str(error) in MESSAGES else 'invalid_backup'
        except sqlite3.Error:item['state']='failed';item['code']='storage_error'
        except Exception:item['state']='failed';item['code']='unexpected_error'
        finally:
            item['path'].unlink(missing_ok=True);item['touched']=time.monotonic();self.running.release()
    def restore_now(self,owner,path):
        """The one-request restore (a backup sent whole): the same reader, while the caller waits."""
        if not self.running.acquire(blocking=False):raise ResearchError('rate_limited','input',429)
        try:
            with open(path,'rb') as f:return self.store.restore_stream(owner,f)
        finally:self.running.release()

def inbox_loop(store,data,stop,every=30):
    """Imports what was dropped in <data>/inbox every `every` seconds (moved to inbox/done/ once stored)."""
    while not stop.wait(every):
        try:
            for event in notes.import_inbox(store,data):logging.info(json.dumps({'stage':'inbox',**event},ensure_ascii=False))
        except Exception as error:logging.info(json.dumps({'stage':'inbox','event':'error','why':type(error).__name__}))

def digest_loop(data,stop,every=600,hour=3):
    """--digest-ai-sessions: after `hour` o'clock (local), yesterday's AI session logs once into the inbox."""
    state=Path(data)/'digest-state.json'
    while True:
        try:
            now=time.localtime();yesterday=time.strftime('%Y-%m-%d',time.localtime(time.time()-86400))
            done=json.loads(state.read_text()).get('last_day') if state.exists() else None
            if now.tm_hour>=hour and done!=yesterday:
                codex,claude=digest.default_sources();digest.write_digest(data,yesterday,codex,claude)
                notes._write_json(state,{'last_day':yesterday})
        except Exception as error:logging.info(json.dumps({'stage':'digest','event':'error','why':type(error).__name__}))
        if stop.wait(every):return

class App:
    def __init__(self,path,port,backend_url,backend=None,public_origin=None,limits=None):
        self.store=Store(path,limits);self.port=port;self.backend=backend
        self.limit_text={**self.store.limits,'document_mb':self.store.limits['document_bytes_per_owner']//1000000}
        # The largest backup file a full library can make: JSON escapes write one byte as up to six (\u0001), so 6x the
        # documents + reports (10MB), plus the fields of every document. The size that counts is checked on the decoded
        # documents while restoring; this only bounds what may be uploaded.
        self.restore_max=6*(self.store.limits['document_bytes_per_owner']+10_000_000)+256*self.store.limits['documents_per_owner']+1_000_000
        self.uploads=Uploads(Path(path).parent/'restore-uploads',self.store)
        # Optional private-term deny-list next to the database (one term per line).
        self.research=Research(backend_url,denylist=Path(path).parent/'denylist.txt',denylist_optional=True,official_files=[Path(path).parent/'official-domains.txt'])
        self.research.guard=backend.ensure_owned if backend else guard_for_state(Path(path).parent/'backend-instance.json',urlsplit(backend_url).port)
        # Both loopback names of this port; any other Host/Origin (DNS rebinding, other sites) is refused.
        self.origin=f'http://localhost:{port}';self.allowed_hosts={f'localhost:{port}',f'127.0.0.1:{port}'}
        # Optional single https origin (a tunnel). Default none: behaviour is exactly as before.
        self.public_origin=None
        if public_origin is not None:
            self.public_origin,public_host=parse_public_origin(public_origin);self.allowed_hosts.add(public_host)
        self.instance=secrets.token_hex(24)
        self.events=collections.deque(maxlen=100);self.limits={};self.lock=threading.Lock();self.search_slots=threading.BoundedSemaphore(3)
    @property
    def origins(self):return {self.origin,f'http://127.0.0.1:{self.port}'}|({self.public_origin} if self.public_origin else set())
    def event(self,request_id,stage,code,elapsed):
        event={'request_id':request_id,'stage':stage,'code':code,'elapsed_ms':round(elapsed*1000),'at':time.time(),'version':'local-research-1'}
        with self.lock:self.events.append(event)
        logging.info(json.dumps(event))
    def limit(self,key,maximum):
        with self.lock:
            now=time.monotonic();items=[t for t in self.limits.get(key,[]) if now-t<60]
            if len(items)>=maximum:return False
            items.append(now);self.limits[key]=items
            if len(self.limits)>2000:self.limits={k:v for k,v in self.limits.items() if v and now-v[-1]<60}
            return True

class Handler(BaseHTTPRequestHandler):
    server_version='RustXNGWorkbench'
    def log_message(self,*args):pass
    @property
    def app(self):return self.server.app
    def send(self,status,payload,content_type='application/json; charset=utf-8',cookie=None,attachment=None,location=None):
        data=json.dumps(payload,ensure_ascii=False).encode() if isinstance(payload,(dict,list)) else payload
        self.send_response(status)
        for k,v in {'Content-Type':content_type,'Content-Length':str(len(data)),'Cache-Control':'no-store','X-Content-Type-Options':'nosniff','Referrer-Policy':'no-referrer','Cross-Origin-Resource-Policy':'same-origin','Content-Security-Policy':"default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"}.items():self.send_header(k,v)
        self.send_header('X-Request-ID',self.request_id)
        if cookie:self.send_header('Set-Cookie',cookie)
        if attachment:self.send_header('Content-Disposition',f'attachment; filename="{attachment}"')
        if location:self.send_header('Location',location)
        self.end_headers();self.wfile.write(data)
    def message(self,code):
        message=MESSAGES.get(code,MESSAGES['unexpected_error'])
        if '{' in message:message=message.format(**{k:(f'{v:,}' if isinstance(v,int) else v) for k,v in self.app.limit_text.items()})
        return message
    def fail(self,status,code,stage='input'):
        self.app.event(self.request_id,stage,code,time.monotonic()-self.started)
        self.send(status,{'ok':False,'error':{'code':code,'stage':stage,'message':self.message(code),'request_id':self.request_id}})
    def send_file(self,path,content_type,attachment):
        """A file on disk as the response body (the backup), copied in pieces."""
        size=path.stat().st_size
        self.send_response(200)
        for k,v in {'Content-Type':content_type,'Content-Length':str(size),'Cache-Control':'no-store','X-Content-Type-Options':'nosniff','Referrer-Policy':'no-referrer','Cross-Origin-Resource-Policy':'same-origin','Content-Security-Policy':"default-src 'self'; frame-ancestors 'none'"}.items():self.send_header(k,v)
        self.send_header('X-Request-ID',self.request_id);self.send_header('Content-Disposition',f'attachment; filename="{attachment}"');self.end_headers()
        with open(path,'rb') as f:shutil.copyfileobj(f,self.wfile,1<<20)
    def length(self,content_type,maximum):
        if self.headers.get('Content-Type','').split(';')[0]!=content_type:raise ResearchError('invalid_request','input',415)
        if self.headers.get('Transfer-Encoding'):raise ResearchError('invalid_request','input',400)
        try:length=int(self.headers.get('Content-Length','0'))
        except ValueError:raise ResearchError('invalid_request','input',400)
        if not 0<length<=maximum:raise ResearchError('invalid_request','input',413)
        self.connection.settimeout(10)
        return length
    def read_into(self,f,length):
        try:
            while length:
                piece=self.rfile.read(min(length,1<<20))
                if not piece:raise ResearchError('invalid_request','input',400)
                f.write(piece);length-=len(piece)
        except TimeoutError:raise ResearchError('invalid_request','input',400) from None
    def token(self):
        try:
            cookie=http.cookies.SimpleCookie();cookie.load(self.headers.get('Cookie',''))
            return cookie['rxsession'].value if 'rxsession' in cookie else ''
        except http.cookies.CookieError:return ''
    # Headers a proxy or tunnel (cloudflared, Cloudflare) adds; a request carrying any of them did not come from this PC directly.
    PROXY_HEADERS=('Cf-Connecting-Ip','Cf-Ray','Cf-Visitor','Cdn-Loop','X-Forwarded-For','X-Forwarded-Host','X-Forwarded-Proto','Forwarded','X-Real-Ip')
    def from_this_pc(self):
        if self.headers.get('Host','') not in (f'localhost:{self.app.port}',f'127.0.0.1:{self.app.port}'):return False
        if any(self.headers.get(name) is not None for name in self.PROXY_HEADERS):return False
        origin=self.headers.get('Origin')
        return origin is None or origin in (self.app.origin,f'http://127.0.0.1:{self.app.port}')
    def session_cookie(self,token,seconds):return f"rxsession={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={seconds}"
    def body(self,maximum=MAX_INPUT):
        if self.headers.get('Content-Type','').split(';')[0]!='application/json':raise ResearchError('invalid_request','input',415)
        if self.headers.get('Transfer-Encoding'):raise ResearchError('invalid_request','input',400)
        try:length=int(self.headers.get('Content-Length','0'))
        except ValueError:raise ResearchError('invalid_request','input',400)
        if not 0<length<=maximum:raise ResearchError('invalid_request','input',413)
        self.connection.settimeout(10)
        try:
            data=json.loads(self.rfile.read(length))
        except (ValueError,UnicodeDecodeError,TimeoutError):raise ResearchError('invalid_request','input',400)
        if not isinstance(data,dict):raise ResearchError('invalid_request','input',400)
        return data
    def do_GET(self):self.dispatch('GET')
    def do_POST(self):self.dispatch('POST')
    def dispatch(self,method):
        self.request_id=secrets.token_hex(12);self.started=time.monotonic()
        try:
            if self.headers.get('Host','') not in self.app.allowed_hosts:return self.fail(403,'origin_refused','boundary')
            origin=self.headers.get('Origin')
            if origin and origin not in self.app.origins:return self.fail(403,'origin_refused','boundary')
            if self.headers.get('Sec-Fetch-Site')=='cross-site' and (method!='GET' or urlsplit(self.path).path.startswith('/api/')):return self.fail(403,'origin_refused','boundary')
            if method=='POST' and origin not in self.app.origins:return self.fail(403,'origin_refused','boundary')
            path=urlsplit(self.path).path
            if path=='/healthz' and method=='GET':return self.send(200,{'product':'rustxng-local-workbench','version':'local-research-1','instance':self.app.instance})
            if path=='/' and method=='GET':return self.send(200,(ROOT/'static/index.html').read_bytes(),'text/html; charset=utf-8')
            if path in STATIC_FILES and method=='GET':
                name,content_type=STATIC_FILES[path];return self.send(200,(ROOT/'static'/name).read_bytes(),content_type)
            # A page address the screen does not know (e.g. chat apps gluing the next words onto a link) goes back to the screen, not to an API error body.
            if method=='GET' and not path.startswith('/api/'):return self.send(302,b'','text/plain; charset=utf-8',location='/')
            if path=='/api/session' and method=='GET':
                s=self.app.store.session(self.token())
                return self.send(200,{'setup_needed':not self.app.store.has_users(),'user':{'name':s['name'],'id':s['owner'],'role':s['role']} if s else None,'csrf':s['csrf'] if s else None,'profiles':PROFILES,'platform':sys.platform,'this_pc':self.from_this_pc(),'phone_link':bool(self.app.public_origin),
                                      'document_count':self.app.store.document_count(s['owner']) if s else 0,'limits':{**self.app.store.limits,'restore_bytes':self.app.restore_max}})
            if path in ('/api/setup','/api/login') and method=='POST':
                if not self.app.limit(('login',self.client_address[0]),10):return self.fail(429,'rate_limited','authentication')
                data=self.body()
                if path.endswith('setup'):self.app.store.register(data.get('name'),data.get('password'))
                s=self.app.store.login(data.get('name'),data.get('password'))
                if not s:return self.fail(401,'invalid_credentials','authentication')
                return self.send(200,{'ok':True,'user':s['user'],'csrf':s['csrf']},cookie=self.session_cookie(s['token'],SESSION_SECONDS))
            if path=='/api/local-login' and method=='POST':
                # This PC only: loopback Host and Origin, no proxy/tunnel headers. Signs in as the first admin (first run: sets it up).
                if not self.from_this_pc():return self.fail(403,'origin_refused','boundary')
                if not self.app.limit(('login',self.client_address[0]),10):return self.fail(429,'rate_limited','authentication')
                s=self.app.store.local_session()
                if not s:return self.fail(401,'authentication_required','authentication')
                return self.send(200,{'ok':True,'user':s['user'],'csrf':s['csrf']},cookie=self.session_cookie(s['token'],SESSION_SECONDS))
            if path=='/api/link-login' and method=='POST':
                # The page posts the one-time token from its own address (#link=...), so the Strict cookie is set by a same-site request.
                if not self.app.limit(('login',self.client_address[0]),10):return self.fail(429,'rate_limited','authentication')
                s=self.app.store.use_login_link(self.body().get('token'))
                if not s:return self.fail(401,'authentication_required','authentication')
                return self.send(200,{'ok':True,'user':s['user'],'csrf':s['csrf']},cookie=self.session_cookie(s['token'],LINK_SESSION_SECONDS))
            session=self.app.store.session(self.token())
            if not session:return self.fail(401,'authentication_required','authentication')
            owner=session['owner']
            if method=='POST' and not hmac.compare_digest(self.headers.get('X-CSRF-Token',''),session['csrf']):return self.fail(403,'csrf_refused','authorization')
            if path in ('/api/accounts','/api/restart','/api/stop','/api/phone-link') and session['role']!='admin':return self.fail(403,'not_found','authorization')
            # Bodies that are not one JSON object: a restore sent whole (spooled to disk, then read one document at a time) and the pieces of a restore upload.
            if path=='/api/restore' and method=='POST':
                length=self.length('application/json',self.app.restore_max);hold=self.app.uploads.reserve(length)
                try:
                    spool=self.app.uploads.new_file('single')
                    try:
                        with open(spool,'wb') as f:self.read_into(f,length)
                        return self.send(200,{'ok':True,'restored':self.app.uploads.restore_now(owner,spool)})
                    finally:spool.unlink(missing_ok=True)
                finally:self.app.uploads.release(hold)
            upload=path[len('/api/restore/upload/'):].split('/') if path.startswith('/api/restore/upload/') else None
            if upload and len(upload)==1 and method=='POST':
                try:offset=int(self.headers.get('X-Upload-Offset',''))
                except ValueError:raise ResearchError('invalid_request','input',400)
                piece=self.rfile.read(self.length('application/octet-stream',MAX_INPUT))
                if len(piece)!=int(self.headers['Content-Length']):raise ResearchError('invalid_request','input',400)
                return self.send(200,{'ok':True,'received':self.app.uploads.append(owner,upload[0],offset,piece)})
            data=self.body() if method=='POST' else {}
            if path=='/api/logout' and method=='POST':
                self.app.store.logout(self.token());return self.send(200,{'ok':True},cookie=self.session_cookie('',0))
            if path=='/api/phone-link' and method=='POST':
                if not self.app.public_origin:return self.fail(404,'not_found')
                if not self.app.limit(('phone-link',owner),5):return self.fail(429,'rate_limited')
                url=login_link_url(self.app.public_origin,self.app.store.issue_login_link(owner))
                return self.send(200,{'ok':True,'url':url,'expires_in':LINK_SECONDS,'qr_svg':qr.svg(url)})
            if path=='/api/accounts' and method=='POST':
                if not self.app.limit(('account',owner),5):return self.fail(429,'rate_limited')
                self.app.store.register(data.get('name'),data.get('password'),False);return self.send(201,{'ok':True})
            if path=='/api/documents':
                if method=='GET':return self.send(200,{'documents':self.app.store.documents(owner)})
                identifier=self.app.store.add_document(owner,data.get('title'),data.get('body'));return self.send(201,{'ok':True,'id':identifier})
            # Deleting / replacing a document: this owner's rows only (owner=? in the store); someone else's id is 404 and nothing changes.
            if path.startswith('/api/documents/') and path.endswith('/delete') and method=='POST':
                identifier=path[len('/api/documents/'):-len('/delete')]
                return self.send(200,{'ok':True,'deleted':1}) if self.app.store.delete_document(owner,identifier) else self.fail(404,'not_found','authorization')
            if path.startswith('/api/documents/') and path.endswith('/replace') and method=='POST':
                result=self.app.store.replace_document(owner,path[len('/api/documents/'):-len('/replace')],data.get('body'),data.get('title'))
                return self.send(200,{'ok':True,**result}) if result else self.fail(404,'not_found','authorization')
            if path.startswith('/api/documents/') and method=='GET':
                doc=self.app.store.document(owner,path.rsplit('/',1)[1])
                return self.send(200,doc) if doc else self.fail(404,'not_found','authorization')
            if path=='/api/preferences':return self.send(200,self.app.store.preferences(owner,data if method=='POST' else None))
            if path=='/api/search' and method=='POST':
                if not self.app.limit(('search',owner),20):return self.fail(429,'rate_limited','search')
                if not self.app.search_slots.acquire(blocking=False):return self.fail(429,'rate_limited','search')
                try:
                    scope=data.get('scope')
                    if scope=='private':report=self.app.research.private(data.get('query'),self.app.store,owner)
                    elif scope=='public':
                        if data.get('public_confirmed') is not True:raise ResearchError('public_consent_required','input',400)
                        report=self.app.research.public(data.get('query'),data.get('profile','general'),data.get('domains',[]))
                    elif scope=='combined':
                        if data.get('public_confirmed') is not True:raise ResearchError('public_consent_required','input',400)
                        private=self.app.research.private(data.get('query'),self.app.store,owner)
                        # Deliberately separate argument; never derive public_query from private query.
                        try:
                            public=self.app.research.public(data.get('public_query'),data.get('profile','general'),data.get('domains',[]))
                        except ResearchError as error:
                            if error.stage=='input':raise
                            public={'status':'failed','sources':[],'queries':[],'created_at':private['created_at'],'markdown':'公開検索を完了できませんでした。','external_query_sent':None,'errors':[{'stage':error.stage,'code':error.code}]}
                            self.app.event(self.request_id,error.stage,error.code,time.monotonic()-self.started)
                        report={'query':data['query'],'scope':'combined','status':'partial' if public['status']!='ok' else 'ok','sources':private['sources']+public['sources'],'private':private,'public':public,'external_query_sent':public.get('external_query_sent',True),'queries':public['queries'],'created_at':public['created_at'],'verification':'社内検索は成功。公開検索は失敗（送信結果は未確認）。' if public['status']=='failed' else '社内検索語と外部送信語を別々に処理。','next_action':'社内資料と公開情報の根拠を確認してください。','markdown':private['markdown']+'\n\n'+public['markdown']}
                    else:raise ResearchError('invalid_request','input',400)
                    report['request_id']=self.request_id
                    report['id']=self.app.store.save_report(owner,report)
                    self.app.event(self.request_id,'search',report['status'],time.monotonic()-self.started)
                    return self.send(200,report)
                finally:self.app.search_slots.release()
            if path=='/api/reports' and method=='GET':return self.send(200,{'reports':self.app.store.reports(owner)})
            # Deleting saved research: this owner's rows only (the store's DELETE carries owner=?); someone else's id is 404 and nothing changes.
            if path=='/api/reports/delete-all' and method=='POST':return self.send(200,{'ok':True,'deleted':self.app.store.delete_reports(owner)})
            if path.startswith('/api/reports/') and path.endswith('/delete') and method=='POST':
                identifier=path[len('/api/reports/'):-len('/delete')]
                return self.send(200,{'ok':True,'deleted':1}) if self.app.store.delete_report(owner,identifier) else self.fail(404,'not_found','authorization')
            if path.startswith('/api/reports/') and method=='GET':
                report=self.app.store.report(owner,path.rsplit('/',1)[1]);return self.send(200,report) if report else self.fail(404,'not_found','authorization')
            if path=='/api/backup' and method=='GET':
                hold=self.app.uploads.reserve(self.app.store.backup_size_bound(owner))
                try:
                    spool=self.app.uploads.new_file('export')
                    try:
                        with open(spool,'wb') as f:self.app.store.export_to(owner,f)
                        return self.send_file(spool,'application/json; charset=utf-8','rustxng-private-backup.json')
                    finally:spool.unlink(missing_ok=True)
                finally:self.app.uploads.release(hold)
            if path=='/api/restore/upload' and method=='POST':
                size=data.get('size')
                if not isinstance(size,int) or isinstance(size,bool) or size<=0:raise ResearchError('invalid_request','input',400)
                if size>self.app.restore_max:raise ResearchError('invalid_request','input',413)
                return self.send(200,self.app.uploads.start(owner,size))
            if upload and len(upload)==2 and upload[1]=='finish' and method=='POST':
                return self.send(202,{'ok':True,'state':self.app.uploads.finish(owner,upload[0])})
            if upload and len(upload)==1 and method=='GET':
                item=self.app.uploads.get(owner,upload[0])
                if not item:return self.fail(404,'not_found','authorization')
                state={'ok':True,'state':item['state'],'size':item['size'],'received':item['received']}
                if item['state']=='done':state['restored']=item['restored']
                if item['state']=='failed':state['error']={'code':item['code'],'message':self.message(item['code'])}
                return self.send(200,state)
            if path=='/api/operations' and method=='GET':
                healthy=False
                try:
                    expected=self.app.research.guard()
                    with self.app.research.opener.open(self.app.research.backend+'/healthz',timeout=2) as r:healthy=r.status==200 and r.headers.get('x-rustxng-safety')=='public-web-v1' and r.headers.get('x-rustxng-instance')==expected
                except (ResearchError,OSError,urllib.error.URLError):pass
                return self.send(200,{'backend_healthy':healthy,'platform':sys.platform,'version':'local-research-1','events':list(self.app.events)[-30:] if session['role']=='admin' else [],'private_storage':'local SQLite / account-scoped','source_license':'AGPL-3.0-or-later','source_available':'/api/source','capabilities':['public_search','private_search','combined_search','profiles','evidence','backup_restore','mcp_public'],'unverified':['external account integrations','production hosting','semantic contradiction checking']})
            if path=='/api/stop' and method=='POST':
                self.send(200,{'ok':True});threading.Thread(target=self.server.shutdown,daemon=True).start();return
            if path=='/api/restart' and method=='POST':
                if not self.app.backend:raise ResearchError('backend_missing','startup',503)
                self.app.backend.restart();return self.send(200,{'ok':True,'status':'starting'})
            if path=='/api/mcp-config' and method=='GET':
                return self.send(200,{'mcpServers':{'fast-research':{'command':sys.executable,'args':[str(ROOT/'agent.py')]+(['--binary',str(self.app.backend.binary.resolve())] if self.app.backend else [])}}},attachment='fast-research-mcp.json')
            if path=='/api/source' and method=='GET':
                import io,zipfile
                buf=io.BytesIO()
                # Only tracked source + this implementation's allowlisted source trees; never DB/log/env/toolchain.
                repo=ROOT.parent
                with zipfile.ZipFile(buf,'w',zipfile.ZIP_DEFLATED) as archive:
                    paths=['Cargo.toml','Cargo.lock','LICENSE','NOTICE','README.md','settings.toml',
                           'src/public_web.rs','src/evidence.rs','docs/INSTALL-AI.md','docs/AI-USAGE.md','docs/WORKBENCH.md','docs/LOCAL-EVALUATION.md','docs/UPSTREAM-README.md']
                    for directory,suffixes in [('src',{'.rs','.html','.css','.js','.json','.svg','.png','.ico','.txt','.tsv','.yml','.yaml'}),('tests',{'.rs'}),('templates',{'.html'}),('static',{'.css','.js','.svg','.png','.ico','.woff','.woff2'}),('workbench',{'.py','.html','.js','.css','.md','.txt','.webmanifest','.svg','.png','.ico'})]:
                        paths += [str(p.relative_to(repo)) for p in (repo/directory).rglob('*') if p.is_file() and p.suffix in suffixes and '__pycache__' not in p.parts and 'test-output' not in p.parts]
                    paths += ['workbench/evaluation/questions.json','workbench/distribution/settings-default.toml','RustXNG.app/Contents/Info.plist','RustXNG.app/Contents/MacOS/launch']
                    for relative in sorted(set(paths)):
                        p=repo/relative
                        if not relative or p.is_symlink() or not p.is_file() or any(part.startswith('.') and part not in ('.github',) for part in Path(relative).parts):continue
                        if p.suffix in ('.sqlite','.db','.log') or 'evidence' in p.parts or 'fixtures' in p.parts:continue
                        if relative=='settings.toml':
                            # Ship only the immutable upstream defaults, never a runtime configuration.
                            defaults=(ROOT/'distribution/settings-default.toml').read_bytes()
                            archive.writestr('RustXNG/settings.toml',defaults)
                        else:archive.write(p,'RustXNG/'+relative)
                return self.send(200,buf.getvalue(),'application/zip',attachment='rustxng-source.zip')
            return self.fail(404,'not_found')
        except ResearchError as error:return self.fail(error.status,error.code,error.stage)
        except ValueError as error:return self.fail(400,str(error) if str(error) in MESSAGES else 'invalid_request')
        except sqlite3.IntegrityError:return self.fail(400,'invalid_request','storage')
        except sqlite3.Error:return self.fail(500,'storage_error','storage')
        except (BrokenPipeError,ConnectionResetError):pass
        except Exception:return self.fail(500,'unexpected_error','handler')

class Server(ThreadingHTTPServer):
    daemon_threads=True
    def __init__(self,app):super().__init__(('127.0.0.1',app.port),Handler);self.app=app

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--port',type=int,default=18890);parser.add_argument('--backend-port',type=int,default=18889);parser.add_argument('--data',default=str(ROOT.parent/'local-data'));parser.add_argument('--binary',default=str(ROOT.parent/'target/debug/rustxng'));parser.add_argument('--no-backend',action='store_true');parser.add_argument('--public-origin',default=None,help='one https origin that may also reach this server (for example a tunnel); default: none')
    parser.add_argument('--issue-login-link',action='store_true',help='print a one-time login link (10 minutes, first admin) for --public-origin and exit')
    parser.add_argument('--doc-chars',type=int,default=None,help='characters per private document (default 2,000,000)')
    parser.add_argument('--doc-count',type=int,default=None,help='private documents per person (default 10,000)')
    parser.add_argument('--doc-bytes',type=int,default=None,help='bytes of private documents per person (default 300,000,000)')
    parser.add_argument('--digest-ai-sessions',action='store_true',help="opt-in: once a day, yesterday's Codex / Claude Code session logs of this PC into <data>/inbox (local only)")
    args=parser.parse_args();data=Path(args.data);data.mkdir(parents=True,exist_ok=True,mode=0o700);os.chmod(data,0o700)
    limits={'document_chars':args.doc_chars,'documents_per_owner':args.doc_count,'document_bytes_per_owner':args.doc_bytes}
    if args.issue_login_link:
        if not args.public_origin:parser.error('--issue-login-link needs --public-origin')
        try:origin=parse_public_origin(args.public_origin)[0]
        except ValueError as error:parser.error(f'--public-origin must be a bare https origin such as https://example.org ({error})')
        store=Store(data/'workbench.sqlite');admin=store.first_admin()
        if not admin:parser.error('no admin account yet')
        print(login_link_url(origin,store.issue_login_link(admin['id'])),flush=True)
        return
    logging.basicConfig(filename=data/'diagnostics.jsonl',level=logging.INFO,format='%(message)s');os.chmod(data/'diagnostics.jsonl',0o600)
    backend=None if args.no_backend else Backend(args.binary,args.backend_port,data)
    try:app=App(data/'workbench.sqlite',args.port,f'http://127.0.0.1:{args.backend_port}',backend,args.public_origin,limits)
    except ValueError as error:
        if backend:backend.stop()
        parser.error(f'--public-origin must be a bare https origin such as https://example.org ({error})')
    server=Server(app)
    stop=threading.Event()
    (data/'inbox').mkdir(mode=0o700,exist_ok=True)
    threading.Thread(target=inbox_loop,args=(app.store,data,stop),name='inbox',daemon=True).start()
    if args.digest_ai_sessions:threading.Thread(target=digest_loop,args=(data,stop),name='digest',daemon=True).start()
    instance_path=data/'instance.json'
    instance_path.write_text(json.dumps({'pid':os.getpid(),'instance':app.instance}))
    os.chmod(instance_path,0o600)
    if backend:
        try:backend.start()
        except ResearchError as error:app.event('startup','startup',error.code,0)
    print(f'RustXNG workbench: {app.origin}',flush=True)
    try:server.serve_forever()
    except KeyboardInterrupt:pass
    finally:
        stop.set();server.server_close()
        if backend:backend.stop()

if __name__=='__main__':main()
