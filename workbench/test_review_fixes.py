"""Regression tests for the 2026-11-02 security review (中1-中4, 低1-低4) and the workbench UI contract.

Synthetic credentials are assembled at run time so that no secret-shaped literal sits in the source.
"""
import http.client
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import textwrap
import threading
import time
import unittest
import urllib.parse
from unittest.mock import patch

from .core import OFFICIAL_FILE, Research, ResearchError, check_public_query, excerpt, load_official_domains, redirected_offsite, terms
from .server import App, Backend, Server, parse_public_origin
from .store import Store

ROOT = Path(__file__).resolve().parent.parent


def make(*parts):
    return ''.join(parts)


FAKE_BACKEND = '''#!{python}
import os, socket, sys, time
port = int(sys.argv[sys.argv.index('--port') + 1])
sock = socket.socket()
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(('127.0.0.1', port))
sock.listen(5)
print('SECRET=' + os.environ.get('SEARXNG_SECRET', ''), flush=True)
print('IDLE=' + os.environ.get('RUSTXNG_IDLE_TIMEOUT', ''), flush=True)
time.sleep(600)
'''


def free_port():
    import socket
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def alive(pid):
    try:
        state = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0]
    except (OSError, IndexError):
        return False
    return state not in ('Z', 'X')


def fake_binary(folder):
    binary = Path(folder) / 'fake-rustxng'
    binary.write_text(FAKE_BACKEND.format(python=sys.executable))
    binary.chmod(0o700)
    return binary


def row(url, title='', content='', raw='', evidence=None):
    return {'url': url, 'title': title, 'content': content, 'raw_content': raw, 'evidence': evidence or {}}


def search_with(rows, q='Python pathlib 公式', domains=None, profile='general'):
    research = Research('http://127.0.0.1:1')
    with patch.object(research, 'fetch', return_value={'results': rows}):
        return research.public(q, profile, domains or [], max_queries=1)


BODY = 'Python pathlib は標準ライブラリです。公式ドキュメントに pathlib の使い方があります。' * 6
OK_META = {'status': 'ok', 'code': 'extracted', 'final_url': None}


class QueryGuardTests(unittest.TestCase):  # review 中1
    def refused(self, q, **kw):
        with self.assertRaises(ResearchError) as error:
            check_public_query(q, **kw)
        return error.exception.code

    def test_known_credential_shapes_are_refused(self):
        shapes = [
            make('gh', 'p_', 'A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8'),
            make('github', '_pat_', '11ABCDEFG0abcdefghij_klmnopqrstuvwxyz0123456789ABCDEFGH'),
            make('xo', 'xb-', '1234567890-abcdefghijklmnop'),
            make('AK', 'IA', 'IOSFODNN7EXAMPLE'),
            make('AI', 'za', 'SyA1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6'),
            make('ey', 'Jhbgcioijiuzi1nij9', '.', 'eyJzdWIiOiIxMjM0'),
            make('M', 'TA0ODg4MDE5Nzg5MDEyMzQ1Ng', '.', 'GabcDe', '.', 'abcdefghijklmnopqrstuvwxyz0'),
            make('gl', 'pat-', 'abcdefghij0123456789'),
            make('-----BEGIN ', 'RSA PRIVATE KEY', '-----'),
        ]
        for shape in shapes:
            self.assertEqual(self.refused('料金を調べて ' + shape), 'possible_private_query', shape[:8])

    def test_long_random_looking_token_is_refused_but_words_are_not(self):
        token = 'q7ZxT2mV9bLk4HcR8wYpN3sD5fGj'  # 28 chars, upper+lower+digit
        self.assertEqual(self.refused('設定 ' + token), 'possible_private_query')
        self.assertEqual(self.refused('key ' + 'a3f9c1e7b2d84f60a1c3e5f7b9d2a4c6'), 'possible_private_query')  # 32 hex
        for ok in ['Python pathlib 公式', 'AbstractSingletonProxyFactoryBean usage',
                   'my-very-long-project-name-about-search-engines', 'configuration_management_best_practices',
                   'https://docs.aws.amazon.com/AmazonS3/latest/userguide/Welcome-Guide2.html の意味',
                   'Pneumonoultramicroscopicsilicovolcanoconiosis とは']:
            check_public_query(ok)  # must not raise

    def test_credentials_in_any_part_of_a_url_are_refused(self):
        token = 'q7ZxT2mV9bLk4HcR8wYpN3sD5fGj'
        for url in ['https://example.com/reset/' + token, 'https://example.com/a/b/' + token + '/confirm', 'https://example.com/x#' + token,
                    'https://example.com/s/%71%37ZxT2mV9bLk4HcR8wYpN3sD5fGj', 'https://example.com/s/%2571%2537ZxT2mV9bLk4HcR8wYpN3sD5fGj',
                    'https://example.com/p?next=' + token, 'https://' + 'user:' + token + '@example.com/']:
            self.assertEqual(self.refused('参考 ' + url), 'possible_private_query', url)
        # ordinary documentation URLs, including long paths and mixed-case segments, are still allowed
        for url in ['https://docs.aws.amazon.com/AmazonS3/latest/userguide/Welcome-Guide2.html', 'https://developer.mozilla.org/en-US/docs/Web/API/Fetch_API/Using_Fetch',
                    'https://docs.python.org/3/library/pathlib.html#pathlib.Path.read_text', 'https://example.com/items/550e8400-e29b-41d4-a716-446655440000']:
            check_public_query('参考 ' + url)

    def test_url_credential_never_reaches_transport(self):
        research = Research('http://127.0.0.1:1')
        with patch.object(research, 'fetch', side_effect=AssertionError('leaked to transport')):
            with self.assertRaises(ResearchError) as error:
                research.public('この署名つきURLを調べて https://example.com/download/q7ZxT2mV9bLk4HcR8wYpN3sD5fGj')
        self.assertEqual(error.exception.code, 'possible_private_query')

    def test_secret_in_url_query_string_is_still_refused(self):
        url = 'https://example.com/a?token=' + 'q7ZxT2mV9bLk4HcR8wYpN3sD5fGj'
        self.assertEqual(self.refused(url), 'possible_private_query')

    def test_denylist_file_blocks_registered_private_terms(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'deny.txt'
            path.write_text('# 社内用語\nProject Orion\n株式会社ひみつ\n', encoding='utf-8')
            self.assertEqual(self.refused('project   orion'.replace('   ', ' ') + ' の予算', denylist=path), 'private_term_denied')
            self.assertEqual(self.refused('ＰＲＯＪＥＣＴ　ＯＲＩＯＮ', denylist=path), 'private_term_denied')  # full width
            self.assertEqual(self.refused('株式会社ひみつ 評判', denylist=path), 'private_term_denied')
            check_public_query('Python pathlib 公式', denylist=path)

    def test_denylist_sees_through_percent_encoding_and_other_spellings(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'deny.txt'
            path.write_text('Project Orion\n株式会社ひみつ\n', encoding='utf-8')
            once = urllib.parse.quote('Project Orion')
            twice = urllib.parse.quote(once)
            spellings = ['調べて ' + once, '調べて ' + twice, '調べて Project+Orion', '調べて project%20orion', 'Project%2BOrion'.replace('%2B', '+'),
                         urllib.parse.quote('株式会社ひみつ'), urllib.parse.quote(urllib.parse.quote('株式会社ひみつ')),
                         'https://example.com/search?q=' + once, 'Project   Orion', 'Pro\u200bject Or\u200dion', '株式会社\u200bひみつ']
            for q in spellings:
                self.assertEqual(self.refused(q, denylist=path), 'private_term_denied', repr(q))
            check_public_query('Python pathlib 公式 Project Gutenberg', denylist=path)

    def test_encoded_denylist_word_never_reaches_transport(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'deny.txt'
            path.write_text('Project Orion\n', encoding='utf-8')
            research = Research('http://127.0.0.1:1', denylist=path)
            with patch.object(research, 'fetch', side_effect=AssertionError('leaked to transport')):
                for q in [urllib.parse.quote('Project Orion の予算'), urllib.parse.quote(urllib.parse.quote('Project Orion'))]:
                    with self.assertRaises(ResearchError) as error:
                        research.public(q)
                    self.assertEqual(error.exception.code, 'private_term_denied')

    def test_denylist_missing_file_is_fail_closed_unless_optional(self):
        missing = Path(tempfile.gettempdir()) / 'definitely-missing-denylist-fixture.txt'
        self.assertEqual(self.refused('public words', denylist=missing), 'denylist_unreadable')
        check_public_query('public words', denylist=missing, denylist_optional=True)

    def test_research_public_applies_guard_before_any_transport(self):
        research = Research('http://127.0.0.1:1')
        with patch.object(research, 'fetch', side_effect=AssertionError('leaked to transport')):
            with self.assertRaises(ResearchError) as error:
                research.public('調べて ' + make('gh', 'p_', 'A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8'))
        self.assertEqual(error.exception.code, 'possible_private_query')

    def test_denylist_from_environment_is_used_by_research(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'deny.txt'
            path.write_text('ひみつの案件名\n', encoding='utf-8')
            with patch.dict(os.environ, {'TALMUD_RESEARCH_DENYLIST': str(path)}):
                research = Research('http://127.0.0.1:1')
            with patch.object(research, 'fetch', side_effect=AssertionError('leaked to transport')):
                with self.assertRaises(ResearchError) as error:
                    research.public('ひみつの案件名 とは')
            self.assertEqual(error.exception.code, 'private_term_denied')


class EvidenceTests(unittest.TestCase):
    def test_usable_evidence_needs_the_term_in_the_fetched_body(self):  # review 中3
        wall = 'このサイトはCookieを使用します。同意して続行してください。' * 12
        rows = [row(f'https://example.com/{i}', title='Python pathlib 公式ガイド', content='Python pathlib の公式説明',
                    raw=wall, evidence={'status': 'ok', 'code': 'extracted'}) for i in range(3)]
        report = search_with(rows)
        self.assertEqual(report['usable_sources'], 0)
        self.assertTrue(all(s['term_matches'] == 0 for s in report['sources']))
        self.assertEqual(report['status'], 'partial')

    def test_body_with_the_terms_is_usable(self):
        rows = [row(f'https://example.com/{i}', raw=BODY, evidence={'status': 'ok', 'code': 'extracted'}) for i in range(3)]
        report = search_with(rows)
        self.assertEqual(report['usable_sources'], 3)
        self.assertEqual(report['status'], 'ok')

    def test_offsite_redirect_is_not_the_requested_source(self):  # review 中4
        meta = {'status': 'ok', 'code': 'extracted', 'final_url': 'https://tracker.example.net/landing?x=1'}
        rows = [row('https://docs.python.org/3/library/pathlib.html', raw=BODY, evidence=meta)]
        report = search_with(rows, domains=['docs.python.org'])
        source = report['sources'][0]
        self.assertEqual(source['quality'], 'redirected_offsite')
        self.assertFalse(source['preferred_domain'])
        self.assertEqual(source['cite_url'], 'https://tracker.example.net/landing?x=1')
        self.assertEqual(report['usable_sources'], 0)
        self.assertIn('tracker.example.net', report['markdown'])

    def test_same_site_and_www_redirects_stay_trusted(self):
        self.assertFalse(redirected_offsite('https://example.com/a', 'https://example.com/b/'))
        self.assertFalse(redirected_offsite('https://example.com/a', 'https://www.example.com/a'))
        self.assertTrue(redirected_offsite('https://docs.python.org/a', 'https://www.python.org/a'))
        self.assertTrue(redirected_offsite('https://example.com/a', 'https://example.org/a'))
        meta = {'status': 'ok', 'code': 'extracted', 'final_url': 'https://docs.python.org/3/library/pathlib.html'}
        report = search_with([row('https://docs.python.org/3/library/pathlib.html', raw=BODY, evidence=meta)], domains=['docs.python.org'])
        self.assertEqual(report['sources'][0]['quality'], 'ok')
        self.assertTrue(report['sources'][0]['preferred_domain'])

    def test_zero_term_match_gives_no_excerpt_and_a_flag(self):  # review 低4
        text = 'これは全く別の話題です。天気の話をしています。三文目も無関係です。'
        self.assertEqual(excerpt(text, 'Python pathlib'), '')
        rows = [row('https://example.com/x', raw=text * 8, evidence={'status': 'ok', 'code': 'extracted'})]
        source = search_with(rows)['sources'][0]
        self.assertEqual(source['excerpt'], '')
        self.assertFalse(source['excerpt_matched'])
        report = search_with(rows)
        self.assertIn('根拠箇所なし', report['markdown'])

    def test_excerpt_only_contains_fragments_with_a_match(self):
        text = '無関係な最初の文です。Python pathlib の説明です。もう一つ無関係な文です。'
        self.assertEqual(excerpt(text, 'Python pathlib'), 'Python pathlib の説明です。')
        source = search_with([row('https://example.com/x', raw=text * 10, evidence={'status': 'ok', 'code': 'extracted'})])['sources'][0]
        self.assertTrue(source['excerpt_matched'])
        self.assertIn('pathlib', source['excerpt'])
        self.assertIn('pathlib', source['match_terms'])


class OfficialDomainTests(unittest.TestCase):
    """The 公式 label comes only from a small file-driven table (or a domain the user/profile named)."""
    def official(self, url, **kw):
        rows = [row(url, raw=BODY, evidence={'status': 'ok', 'code': 'extracted', **kw.get('meta', {})})]
        source = search_with(rows, domains=kw.get('domains'), profile=kw.get('profile', 'general'))['sources'][0]
        return source

    def test_general_profile_marks_docs_python_org_official(self):
        source = self.official('https://docs.python.org/3/library/pathlib.html')
        self.assertTrue(source['official'])
        self.assertFalse(source['preferred_domain'])  # not a profile/user-specified domain
        self.assertEqual(source['source_basis'], '公式ドメイン表に載っているサイト')
        self.assertTrue(self.official('https://www.digital.go.jp/policies/')['official'])
        self.assertTrue(self.official('https://www.city.example.lg.jp/page')['official'])

    def test_unknown_lookalike_and_user_hosted_sites_are_not_official(self):
        for url in ['https://docs.python.org.evil.example/x', 'https://evilpython.org/x', 'https://someone.github.io/pathlib',
                    'https://qiita.com/user/items/abc', 'https://note.nkmk.me/python-pathlib-usage/', 'https://blog.csdn.net/a/b']:
            source = self.official(url)
            self.assertFalse(source['official'], url)
            self.assertEqual(source['source_basis'], '一次情報か未確認')

    def test_offsite_redirect_from_an_official_host_is_not_official(self):
        source = self.official('https://docs.python.org/3/library/pathlib.html', meta={'final_url': 'https://tracker.example.net/x'})
        self.assertFalse(source['official'])
        self.assertEqual(source['quality'], 'redirected_offsite')

    def test_a_domain_the_user_typed_is_preferred_but_not_official(self):
        source = self.official('https://docs.example.test/a', domains=['docs.example.test'])
        self.assertFalse(source['official'])  # 公式 only comes from the table
        self.assertTrue(source['preferred_domain'])
        self.assertEqual(source['source_basis'], '指定・プロファイルの優先ドメイン')
        listed = self.official('https://docs.python.org/3/library/pathlib.html', domains=['docs.python.org'])
        self.assertTrue(listed['official'] and listed['preferred_domain'])

    def test_table_is_a_plain_file_that_can_be_extended(self):
        self.assertTrue(OFFICIAL_FILE.is_file())
        table = load_official_domains([OFFICIAL_FILE])
        self.assertIn('docs.python.org', table)
        self.assertNotIn('github.io', table)
        self.assertNotIn('qiita.com', table)
        with tempfile.TemporaryDirectory() as folder:
            extra = Path(folder) / 'mine.txt'
            extra.write_text('# 自分で足す\nHelp.Example.Test  # 大文字・コメント\n*.wild.example.test\nnot a domain\n\n', encoding='utf-8')
            added = load_official_domains([OFFICIAL_FILE, extra, Path(folder) / 'missing.txt', None])
            self.assertIn('help.example.test', added)
            self.assertIn('wild.example.test', added)
            self.assertNotIn('not a domain', added)
            research = Research('http://127.0.0.1:1', official_files=[extra])
            with patch.object(research, 'fetch', return_value={'results': [row('https://help.example.test/x', raw=BODY, evidence={'status': 'ok', 'code': 'extracted'})]}):
                self.assertTrue(research.public('Python pathlib 公式', max_queries=1)['sources'][0]['official'])

    def test_official_sources_sort_ahead_of_unknown_ones(self):
        rows = [row('https://blog.example.net/a', raw=BODY, evidence={'status': 'ok', 'code': 'extracted'}),
                row('https://docs.python.org/3/library/pathlib.html', raw=BODY, evidence={'status': 'ok', 'code': 'extracted'})]
        sources = search_with(rows)['sources']
        self.assertEqual([s['official'] for s in sources], [True, False])

    def test_source_download_ships_the_table_file(self):
        import io, zipfile
        # the AGPL source zip must contain the table the app uses
        tmp = tempfile.TemporaryDirectory()
        app = App(Path(tmp.name) / 's.sqlite', free_port(), 'http://127.0.0.1:1')
        server = Server(app)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            app.store.register('Synthetic Z', 'fixture-password-zip-2026')
            login = app.store.login('Synthetic Z', 'fixture-password-zip-2026')
            conn = http.client.HTTPConnection('127.0.0.1', app.port, timeout=20)
            conn.request('GET', '/api/source', headers={'Host': f'localhost:{app.port}', 'Cookie': 'rxsession=' + login['token']})
            response = conn.getresponse(); data = response.read(); conn.close()
            self.assertEqual(response.status, 200)
            self.assertIn('RustXNG/workbench/official-domains.txt', zipfile.ZipFile(io.BytesIO(data)).namelist())
        finally:
            server.shutdown(); server.server_close(); tmp.cleanup()


class PublicOriginTests(unittest.TestCase):
    """--public-origin lets exactly one https origin (a tunnel) reach the app; every other check stays."""
    TUNNEL = 'demo-abc.trycloudflare.com'

    def make(self, public_origin=None):
        tmp = tempfile.TemporaryDirectory(prefix='review-origin-')
        self.addCleanup(tmp.cleanup)
        app = App(Path(tmp.name) / 'o.sqlite', free_port(), 'http://127.0.0.1:1', public_origin=public_origin)
        server = Server(app)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(lambda: (server.shutdown(), server.server_close()))
        app.store.register('Synthetic O', 'fixture-password-origin-2026')
        return app, app.store.login('Synthetic O', 'fixture-password-origin-2026')

    def call(self, app, session, path, payload=None, host=None, origin=None, extra=None):
        headers = {'Host': host or f'localhost:{app.port}'}
        if origin is not None:
            headers['Origin'] = origin
        if session:
            headers.update({'Cookie': 'rxsession=' + session['token'], 'X-CSRF-Token': session['csrf']})
        if payload is not None:
            headers['Content-Type'] = 'application/json'
        headers.update(extra or {})
        conn = http.client.HTTPConnection('127.0.0.1', app.port, timeout=5)
        conn.request('POST' if payload is not None else 'GET', path, json.dumps(payload) if payload is not None else None, headers)
        response = conn.getresponse(); response.read(); conn.close()
        return response.status

    def test_without_the_option_a_tunnel_host_is_refused_as_before(self):
        app, session = self.make()
        self.assertEqual(self.call(app, session, '/api/session', host=self.TUNNEL), 403)
        self.assertEqual(self.call(app, session, '/api/documents', {'title': 'x', 'body': 'y'}, host=self.TUNNEL, origin='https://' + self.TUNNEL), 403)
        self.assertEqual(self.call(app, session, '/api/session', host=f'localhost:{app.port}'), 200)

    def test_with_the_option_only_that_one_origin_is_added(self):
        app, session = self.make('https://' + self.TUNNEL)
        tunnel = 'https://' + self.TUNNEL
        self.assertEqual(self.call(app, session, '/api/session', host=self.TUNNEL), 200)
        self.assertEqual(self.call(app, session, '/api/documents', {'title': 'x', 'body': 'y'}, host=self.TUNNEL, origin=tunnel), 201)
        # still refused: another host, the http form of the same host, another https origin, a foreign Origin on the right Host
        self.assertEqual(self.call(app, session, '/api/session', host='other.trycloudflare.com'), 403)
        self.assertEqual(self.call(app, session, '/api/documents', {'title': 'x2', 'body': 'y2'}, host=self.TUNNEL, origin='http://' + self.TUNNEL), 403)
        self.assertEqual(self.call(app, session, '/api/documents', {'title': 'x3', 'body': 'y3'}, host=self.TUNNEL, origin='https://other.trycloudflare.com'), 403)
        self.assertEqual(self.call(app, session, '/api/documents', {'title': 'x4', 'body': 'y4'}, host=self.TUNNEL, origin='https://attacker.invalid'), 403)
        # the other protections are untouched: cross-site fetches and a missing CSRF token
        self.assertEqual(self.call(app, session, '/api/documents', {'title': 'x5', 'body': 'y5'}, host=self.TUNNEL, origin=tunnel, extra={'Sec-Fetch-Site': 'cross-site'}), 403)
        self.assertEqual(self.call(app, session, '/api/documents', {'title': 'x6', 'body': 'y6'}, host=self.TUNNEL, origin=tunnel, extra={'X-CSRF-Token': ''}), 403)
        self.assertEqual(self.call(app, None, '/api/documents', host=self.TUNNEL), 401)

    def test_only_a_bare_https_origin_is_accepted(self):
        self.assertEqual(parse_public_origin('https://Demo-Abc.trycloudflare.com/'), ('https://demo-abc.trycloudflare.com', 'demo-abc.trycloudflare.com'))
        self.assertEqual(parse_public_origin('https://x.example:8443')[1], 'x.example:8443')
        for bad in ['http://x.example', 'https://x.example/path', 'https://user' + '@' + 'x.example', 'https://x.example?a=1', 'https://x.example#f',
                    'https://localhost', 'https://127.0.0.1', 'ftp://x.example', 'x.example', '', 'https://']:
            with self.assertRaises(ValueError, msg=bad):
                parse_public_origin(bad)
            with self.assertRaises(ValueError, msg=bad):
                with tempfile.TemporaryDirectory() as folder:
                    App(Path(folder) / 'b.sqlite', 1, 'http://127.0.0.1:1', public_origin=bad)

    def test_command_line_rejects_a_non_https_value(self):
        with tempfile.TemporaryDirectory() as folder:
            done = subprocess.run([sys.executable, '-m', 'workbench.server', '--data', folder, '--no-backend', '--port', str(free_port()), '--public-origin', 'http://x.example'],
                                  cwd=ROOT, capture_output=True, text=True, timeout=30)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('--public-origin', done.stderr)


class BackendLifecycleTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='review-fixture-')
        self.binary = fake_binary(self.tmp.name)
        self.pids = []

    def tearDown(self):
        for pid in self.pids:
            try:
                os.kill(pid, signal.SIGKILL)
            except OSError:
                pass
        self.tmp.cleanup()

    def wait_gone(self, pid, seconds=6):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if not alive(pid):
                return True
            time.sleep(.1)
        return False

    @unittest.skipUnless(sys.platform.startswith('linux'), 'PR_SET_PDEATHSIG is Linux only')
    def test_child_dies_when_the_parent_is_killed(self):  # review 低1
        port = free_port()
        script = textwrap.dedent(f'''
            import sys, time
            sys.path.insert(0, {str(ROOT)!r})
            from workbench.server import Backend
            backend = Backend({str(self.binary)!r}, {port}, {self.tmp.name!r})
            backend.start()
            print(backend.child.pid, flush=True)
            time.sleep(120)
        ''')
        parent = subprocess.Popen([sys.executable, '-c', script], stdout=subprocess.PIPE, text=True)
        try:
            child = int(parent.stdout.readline())
            self.pids.append(child)
            self.assertTrue(alive(child))
            parent.send_signal(signal.SIGKILL)
            parent.wait(timeout=5)
            self.assertTrue(self.wait_gone(child), 'backend child outlived a SIGKILLed parent')
        finally:
            parent.kill()
            parent.wait()
            parent.stdout.close()

    @unittest.skipUnless(sys.platform.startswith('linux'), 'PR_SET_PDEATHSIG is Linux only')
    def test_child_started_from_a_short_lived_thread_survives_while_the_parent_lives(self):
        backend = Backend(self.binary, free_port(), self.tmp.name)
        errors = []
        def restart_like_a_request_handler():
            try:
                backend.start()
            except Exception as error:  # pragma: no cover
                errors.append(error)
        worker = threading.Thread(target=restart_like_a_request_handler)
        worker.start(); worker.join()
        self.assertEqual(errors, [])
        self.pids.append(backend.child.pid)
        time.sleep(1.0)
        self.assertTrue(alive(backend.child.pid), 'handler thread exit must not kill the backend')
        backend.stop()
        self.assertFalse(alive(backend.child.pid))

    def test_each_launch_gets_its_own_random_secret(self):  # review 低3
        secrets_seen = []
        for _ in range(2):
            backend = Backend(self.binary, free_port(), self.tmp.name)
            backend.start()
            self.pids.append(backend.child.pid)
            time.sleep(.3)
            log = (Path(self.tmp.name) / 'backend.log').read_text()
            secrets_seen.append([l for l in log.splitlines() if l.startswith('SECRET=')][-1][7:])
            backend.stop()
        for value in secrets_seen:
            self.assertGreaterEqual(len(value), 32)
            self.assertNotEqual(value, 'ultrasecretkey')
        self.assertNotEqual(secrets_seen[0], secrets_seen[1])

    def test_idle_timeout_is_configurable_and_zero_on_linux_by_default(self):
        backend = Backend(self.binary, free_port(), self.tmp.name, idle_timeout=900)
        backend.start(); self.pids.append(backend.child.pid); time.sleep(.3)
        self.assertIn('IDLE=900', (Path(self.tmp.name) / 'backend.log').read_text())
        backend.stop()
        self.assertEqual(Backend(self.binary, 1, self.tmp.name).idle_timeout, 0 if sys.platform.startswith('linux') else 900)


REAL_BINARY = ROOT / 'target' / 'debug' / 'rustxng'


class RealBackendHostTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not REAL_BINARY.is_file():
            raise unittest.SkipTest('build the rustxng binary first')

    def test_rust_http_refuses_foreign_host_headers(self):  # review 低2, against the real binary
        with tempfile.TemporaryDirectory(prefix='review-real-') as folder:
            port = free_port()
            backend = Backend(REAL_BINARY, port, folder)
            backend.start()
            try:
                def get(host):
                    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=5)
                    conn.request('GET', '/healthz', headers={'Host': host})
                    response = conn.getresponse(); response.read(); conn.close()
                    return response.status
                self.assertEqual(get(f'127.0.0.1:{port}'), 200)
                self.assertEqual(get(f'localhost:{port}'), 200)
                self.assertEqual(get('attacker.invalid'), 403)
                self.assertEqual(get(f'attacker.invalid:{port}'), 403)
            finally:
                backend.stop()


class WorkbenchContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='review-ui-')
        port = free_port()
        cls.app = App(Path(cls.tmp.name) / 'ui.sqlite', port, 'http://127.0.0.1:1')
        cls.http = Server(cls.app)
        cls.port = port
        threading.Thread(target=cls.http.serve_forever, daemon=True).start()
        cls.app.store.register('Synthetic UI', 'fixture-password-ui-2026')
        cls.session = cls.app.store.login('Synthetic UI', 'fixture-password-ui-2026')

    @classmethod
    def tearDownClass(cls):
        cls.http.shutdown(); cls.http.server_close(); cls.tmp.cleanup()

    def call(self, path, payload=None, host=None, origin=None, session=True):
        host = host or f'localhost:{self.port}'
        headers = {'Host': host, 'Origin': origin or f'http://{host}'}
        if session:
            headers.update({'Cookie': 'rxsession=' + self.session['token'], 'X-CSRF-Token': self.session['csrf']})
        if payload is not None:
            headers['Content-Type'] = 'application/json'
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=5)
        conn.request('POST' if payload is not None else 'GET', path, json.dumps(payload) if payload is not None else None, headers)
        response = conn.getresponse(); body = response.read(); conn.close()
        return response.status, json.loads(body), response

    def test_both_loopback_names_work_and_foreign_hosts_do_not(self):
        self.assertEqual(self.call('/api/session', host=f'127.0.0.1:{self.port}')[0], 200)
        self.assertEqual(self.call('/api/session', host=f'localhost:{self.port}')[0], 200)
        self.assertEqual(self.call('/api/session', host='attacker.invalid')[0], 403)
        self.assertEqual(self.call('/api/session', host=f'127.0.0.1:{self.port}', origin='http://attacker.invalid')[0], 403)
        # a loopback Origin on a different port is not ours either
        self.assertEqual(self.call('/api/documents', {'title': 'x', 'body': 'y'}, origin='http://127.0.0.1:1')[0], 403)
        self.assertEqual(self.call('/api/documents', {'title': 'x', 'body': 'y'}, origin=f'http://127.0.0.1:{self.port}')[0], 201)

    def test_session_reports_platform_for_platform_specific_wording(self):
        code, data, _ = self.call('/api/session', session=False)
        self.assertEqual(code, 200)
        self.assertEqual(data['platform'], sys.platform)

    def test_saved_report_list_shows_scope_and_usable_count(self):
        owner = self.app.store.register('Synthetic lister', 'fixture-password-lister', False)
        report = {'query': '公開の語', 'scope': 'public', 'status': 'partial', 'usable_sources': 2, 'sources': [{'citation': 'W1'}, {'citation': 'W2'}, {'citation': 'W3'}]}
        self.app.store.save_report(owner, report)
        self.app.store.save_report(owner, {'query': '社内', 'scope': 'private', 'status': 'ok', 'sources': [{'citation': 'L1'}]})
        rows = {r['query']: r for r in self.app.store.reports(owner)}
        self.assertEqual((rows['公開の語']['scope'], rows['公開の語']['usable'], rows['公開の語']['total']), ('public', 2, 3))
        self.assertEqual((rows['社内']['scope'], rows['社内']['usable'], rows['社内']['total']), ('private', 1, 1))
        self.assertIn('created', rows['社内'])

    def test_private_term_guard_reaches_the_ui_search_path(self):
        (Path(self.tmp.name) / 'denylist.txt').write_text('ひみつ案件\n', encoding='utf-8')
        code, data, _ = self.call('/api/search', {'scope': 'public', 'query': 'ひみつ案件の相場', 'public_confirmed': True})
        self.assertEqual(code, 400)
        self.assertEqual(data['error']['code'], 'private_term_denied')
        self.assertIn('社内', data['error']['message'])


if __name__ == '__main__':
    unittest.main()
