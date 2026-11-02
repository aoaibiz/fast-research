"""高速リサーチ (2026-11-02): rename, home-screen icons, deleting saved research, bigger private-document capacity with an FTS5 index
and title-weighted ranking. Synthetic accounts, temporary storage, no network."""
import http.client
import json
import re
import struct
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from .core import Research
from .server import App, Server, STATIC_FILES
from .store import DEFAULT_LIMITS, Store
from .tokens import fts_query, fts_text, terms

STATIC = Path(__file__).resolve().parent / 'static'


def png_size(data):
    assert data[:8] == b'\x89PNG\r\n\x1a\n', 'not a PNG'
    width, height = struct.unpack('>II', data[16:24])
    return width, height


class HttpCase(unittest.TestCase):
    limits = None

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='fast-research-')
        cls.app = App(Path(cls.tmp.name) / 'fixture.sqlite', 0, 'http://127.0.0.1:1', limits=cls.limits)
        cls.http = Server(cls.app)
        cls.port = cls.http.server_address[1]
        cls.app.port = cls.port
        cls.app.origin = f'http://localhost:{cls.port}'
        cls.app.allowed_hosts = {f'localhost:{cls.port}'}
        cls.thread = threading.Thread(target=cls.http.serve_forever, daemon=True)
        cls.thread.start()
        cls.a = cls.app.store.register('Synthetic A', 'fixture-password-2026-A')
        cls.b = cls.app.store.register('Synthetic B', 'fixture-password-2026-B', False)
        cls.sa = cls.app.store.login('Synthetic A', 'fixture-password-2026-A')
        cls.sb = cls.app.store.login('Synthetic B', 'fixture-password-2026-B')

    @classmethod
    def tearDownClass(cls):
        cls.http.shutdown(); cls.http.server_close(); cls.thread.join()
        cls.tmp.cleanup()

    def request(self, path, payload=None, session=None, overrides=None, raw=False):
        headers = {'Host': f'localhost:{self.port}', 'Origin': self.app.origin}
        if session:
            headers.update({'Cookie': 'rxsession=' + session['token'], 'X-CSRF-Token': session['csrf']})
        if payload is not None:
            headers['Content-Type'] = 'application/json'
        headers.update(overrides or {})
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        conn.request('POST' if payload is not None else 'GET', path, json.dumps(payload) if payload is not None else None, headers)
        response = conn.getresponse(); data = response.read(); status = response.status; head = dict(response.getheaders())
        conn.close()
        if raw:
            return status, head, data
        return status, json.loads(data)

    def save_private_report(self, session, query='Orion'):
        code, data = self.request('/api/search', {'scope': 'private', 'query': query}, session)
        self.assertEqual(code, 200)
        return data['id']


class RenameAndIconTests(HttpCase):
    def test_user_visible_name_is_fast_research(self):
        html = (STATIC / 'index.html').read_text(encoding='utf-8')
        self.assertIn('<title>高速リサーチ</title>', html)
        self.assertNotIn('Talmud Research', html)
        self.assertIn('content="高速リサーチ"', html)  # apple-mobile-web-app-title
        self.assertIn('<link rel="manifest" href="/manifest.webmanifest">', html)
        self.assertIn('<link rel="apple-touch-icon" href="/apple-touch-icon.png" sizes="180x180">', html)
        self.assertIn('<meta name="theme-color" content="#17225e">', html)
        self.assertNotIn('Talmud Research', (STATIC / 'app.js').read_text(encoding='utf-8'))

    def test_manifest_is_served_and_names_the_app_with_its_icons(self):
        status, head, body = self.request('/manifest.webmanifest', raw=True)
        self.assertEqual(status, 200)
        self.assertTrue(head['Content-Type'].startswith('application/manifest+json'), head['Content-Type'])
        manifest = json.loads(body)
        self.assertEqual((manifest['name'], manifest['short_name'], manifest['display'], manifest['start_url']), ('高速リサーチ', '高速リサーチ', 'standalone', '/'))
        self.assertTrue(re.fullmatch(r'#[0-9a-f]{6}', manifest['theme_color']) and re.fullmatch(r'#[0-9a-f]{6}', manifest['background_color']))
        sizes = {(i['src'], i['sizes'], i.get('purpose', 'any')) for i in manifest['icons']}
        self.assertEqual(sizes, {('/icon-192.png', '192x192', 'any'), ('/icon-512.png', '512x512', 'any'), ('/icon-512-maskable.png', '512x512', 'maskable')})

    def test_every_icon_is_served_as_png_with_the_declared_size(self):
        for path, size in (('/icon-192.png', 192), ('/icon-512.png', 512), ('/icon-512-maskable.png', 512), ('/apple-touch-icon.png', 180), ('/favicon-32.png', 32), ('/favicon-16.png', 16)):
            with self.subTest(path=path):
                status, head, body = self.request(path, raw=True)
                self.assertEqual(status, 200)
                self.assertEqual(head['Content-Type'], 'image/png')
                self.assertEqual(png_size(body), (size, size))
                self.assertIn("img-src 'self' data:", head['Content-Security-Policy'])
        status, head, body = self.request('/favicon.ico', raw=True)
        self.assertEqual((status, head['Content-Type'], body[:4]), (200, 'image/x-icon', b'\x00\x00\x01\x00'))
        status, head, body = self.request('/favicon.svg', raw=True)
        self.assertEqual(status, 200); self.assertTrue(head['Content-Type'].startswith('image/svg+xml')); self.assertIn(b'<svg', body)

    def test_icons_do_not_need_a_session_but_unknown_names_still_go_back_to_the_screen(self):
        self.assertEqual(self.request('/icon-192.png', raw=True)[0], 200)
        for path in ('/icons/icon-192.png', '/static/icons/icon-192.png', '/icon-256.png'):
            with self.subTest(path=path):
                self.assertEqual(self.request(path, raw=True)[0], 302)
        for path, (name, _) in STATIC_FILES.items():
            self.assertTrue((STATIC / name).is_file(), f'{path} -> {name} missing')


class DeleteSavedResearchTests(HttpCase):
    def test_delete_one_removes_it_from_the_list_and_the_database(self):
        self.app.store.add_document(self.a, 'Synthetic Orion', 'Orion fixture-only material')
        first = self.save_private_report(self.sa); second = self.save_private_report(self.sa)
        self.assertEqual({r['id'] for r in self.request('/api/reports', session=self.sa)[1]['reports']} >= {first, second}, True)
        code, data = self.request(f'/api/reports/{first}/delete', {}, self.sa)
        self.assertEqual((code, data), (200, {'ok': True, 'deleted': 1}))
        ids = {r['id'] for r in self.request('/api/reports', session=self.sa)[1]['reports']}
        self.assertNotIn(first, ids); self.assertIn(second, ids)
        self.assertEqual(self.request('/api/reports/' + first, session=self.sa)[0], 404)
        self.assertIsNone(Store(self.app.store.path).report(self.a, first), 'row must be gone from SQLite, not just from the response')
        # deleting again: nothing there, nothing changes
        self.assertEqual(self.request(f'/api/reports/{first}/delete', {}, self.sa)[0], 404)

    def test_delete_is_scoped_to_the_owner_and_guarded_like_every_post(self):
        self.app.store.add_document(self.a, 'Synthetic Orion two', 'Orion fixture-only material two')
        mine = self.save_private_report(self.sa)
        self.assertEqual(self.request(f'/api/reports/{mine}/delete', {}, self.sb)[0], 404, "another person's id looks like a missing id")
        self.assertEqual(self.request(f'/api/reports/{mine}/delete', {})[0], 401)
        for override in [{'X-CSRF-Token': ''}, {'Origin': 'https://attacker.invalid'}, {'Host': 'attacker.invalid'}, {'Sec-Fetch-Site': 'cross-site'}]:
            with self.subTest(override=override):
                self.assertEqual(self.request(f'/api/reports/{mine}/delete', {}, self.sa, override)[0], 403)
        self.assertEqual(self.request('/api/reports/delete-all', {}, self.sa, {'X-CSRF-Token': ''})[0], 403)
        self.assertEqual(self.request('/api/reports/delete-all', {})[0], 401)
        self.assertEqual(self.request('/api/reports/' + mine, session=self.sa)[0], 200, 'still there after every refused attempt')
        self.assertEqual(self.request(f'/api/reports/{mine}/delete', session=self.sa)[0], 404, 'GET is not a delete')
        self.assertEqual(self.request('/api/reports/' + mine, session=self.sa)[0], 200)

    def test_delete_all_removes_only_this_owners_rows(self):
        self.app.store.add_document(self.a, 'Synthetic Orion three', 'Orion fixture-only material three')
        self.app.store.add_document(self.b, 'Synthetic Orion B', 'Orion fixture-only material of B')
        for _ in range(3):
            self.save_private_report(self.sa)
        theirs = self.save_private_report(self.sb)
        mine_before = len(self.request('/api/reports', session=self.sa)[1]['reports'])
        self.assertGreaterEqual(mine_before, 3)
        code, data = self.request('/api/reports/delete-all', {}, self.sa)
        self.assertEqual((code, data['ok'], data['deleted']), (200, True, mine_before))
        self.assertEqual(self.request('/api/reports', session=self.sa)[1]['reports'], [])
        self.assertEqual(self.request('/api/reports/' + theirs, session=self.sb)[0], 200, "B's research survives A's delete-all")
        self.assertEqual(self.request('/api/reports/delete-all', {}, self.sa)[1]['deleted'], 0)

    def test_a_backup_restores_what_was_deleted(self):
        self.app.store.add_document(self.a, 'Synthetic Orion four', 'Orion fixture-only material four')
        rid = self.save_private_report(self.sa, 'Orion four')
        backup = self.request('/api/backup', session=self.sa)[1]
        self.assertTrue(any(r['payload']['query'] == 'Orion four' for r in backup['reports']))
        self.assertEqual(self.request(f'/api/reports/{rid}/delete', {}, self.sa)[0], 200)
        self.assertFalse(any(r['query'] == 'Orion four' for r in self.request('/api/reports', session=self.sa)[1]['reports']))
        self.assertEqual(self.request('/api/restore', backup, self.sa)[0], 200)
        self.assertTrue(any(r['query'] == 'Orion four' for r in self.request('/api/reports', session=self.sa)[1]['reports']))


class CapacityTests(HttpCase):
    limits = {'document_chars': 50, 'documents_per_owner': 3, 'document_bytes_per_owner': 400}

    def test_limits_come_from_configuration_and_the_messages_say_the_real_numbers(self):
        self.assertEqual(DEFAULT_LIMITS, {'document_chars': 2_000_000, 'documents_per_owner': 10_000, 'document_bytes_per_owner': 300_000_000})
        self.assertEqual(self.app.store.limits, self.limits)
        session = self.request('/api/session', session=self.sa)[1]
        self.assertEqual(({k: session['limits'][k] for k in self.limits}, session['document_count']), (self.limits, 0))
        self.assertEqual(session['limits']['restore_bytes'], 6 * (400 + 10_000_000) + 256 * 3 + 1_000_000)  # follow-up: the worst-case JSON of a full library
        code, data = self.request('/api/documents', {'title': 'long', 'body': 'x' * 51}, self.sa)
        self.assertEqual((code, data['error']['code'], data['error']['message']), (400, 'document_length', '資料本文を1〜50文字で入力してください。'))
        for i in range(3):
            self.assertEqual(self.request('/api/documents', {'title': f'doc {i}', 'body': f'body {i}'}, self.sa)[0], 201)
        code, data = self.request('/api/documents', {'title': 'doc 4', 'body': 'body 4'}, self.sa)
        self.assertEqual((code, data['error']['code']), (400, 'document_limit'))
        self.assertEqual(data['error']['message'], '資料は1利用者3件・合計0MBまでです。')
        self.assertEqual(self.request('/api/session', session=self.sa)[1]['document_count'], 3)
        self.assertEqual(self.request('/api/session', session=self.sb)[1]['document_count'], 0, 'the count is per person')

    def test_a_full_size_backup_restores_in_one_request_and_survives_a_restart(self):
        """F1 of review 001: three 2,000,000-character documents make a 17MB backup; it must come back (the default request cap is 16MiB)."""
        with tempfile.TemporaryDirectory(prefix='fast-research-restore-') as tmp:
            source = Store(Path(tmp) / 'source.sqlite')
            owner = source.register('Synthetic source', 'fixture-password-2026-src')
            for i in range(3):
                source.add_document(owner, f'大きな資料 {i}', (f'資料{i}の本文。ハルモニアの料金は月額です。' * 200000)[:2_000_000])
            backup = json.dumps(source.export(owner), ensure_ascii=False).encode()
            self.assertGreater(len(backup), 17_000_000)  # above the general 16MiB request cap
            app = App(Path(tmp) / 'target.sqlite', 0, 'http://127.0.0.1:1')
            server = Server(app); port = server.server_address[1]; app.port = port; app.origin = f'http://localhost:{port}'; app.allowed_hosts = {f'localhost:{port}'}
            thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
            try:
                target_owner = app.store.register('Synthetic target', 'fixture-password-2026-tgt')
                session = app.store.login('Synthetic target', 'fixture-password-2026-tgt')
                headers = {'Host': f'localhost:{port}', 'Origin': app.origin, 'Cookie': 'rxsession=' + session['token'], 'X-CSRF-Token': session['csrf'], 'Content-Type': 'application/json'}
                conn = http.client.HTTPConnection('127.0.0.1', port, timeout=120)
                conn.request('POST', '/api/restore', backup, headers); response = conn.getresponse(); data = json.loads(response.read()); conn.close()
                self.assertEqual((response.status, data), (200, {'ok': True, 'restored': 3}))
                self.assertGreater(len(backup), 16 * 1024 * 1024, 'the case is only meaningful above the general request cap')
                # the general cap still guards every other request, and a restore larger than the library itself is refused too:
                # the server answers 413 from the Content-Length before reading the body (so the client may see the connection close instead)
                def refused(path, payload):
                    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=120)
                    try:
                        conn.request('POST', path, payload, headers); status = conn.getresponse().status
                    except (BrokenPipeError, ConnectionResetError):
                        status = 'closed before the body was read'
                    finally:
                        conn.close()
                    return status
                self.assertIn(refused('/api/documents', backup), (413, 'closed before the body was read'))
                self.assertGreater(app.restore_max, len(backup))
                # follow-up: the cap is now 6x a full library (about 1.9GB by default); a request above it is refused from its
                # Content-Length alone, so no body of that size has to be built here
                conn = http.client.HTTPConnection('127.0.0.1', port, timeout=120)
                conn.putrequest('POST', '/api/restore', skip_host=True)
                for k, v in {**headers, 'Content-Length': str(app.restore_max + 1)}.items():
                    conn.putheader(k, v)
                conn.endheaders(); self.assertEqual(conn.getresponse().status, 413); conn.close()
                self.assertEqual(app.store.document_count(target_owner), 3, 'nothing was added by the refused requests')
            finally:
                server.shutdown(); server.server_close(); thread.join()
            # "restart": a fresh Store on the same file sees the three documents and finds them through the index
            reopened = Store(Path(tmp) / 'target.sqlite')
            self.assertEqual(reopened.document_count(target_owner), 3)
            self.assertEqual(len(reopened.search_documents(target_owner, 'ハルモニアの料金')), 3)

    def test_default_limits_accept_a_two_million_character_document(self):
        with tempfile.TemporaryDirectory(prefix='fast-research-big-') as tmp:
            store = Store(Path(tmp) / 'big.sqlite')
            owner = store.register('Synthetic big', 'fixture-password-2026-big')
            body = ('ハルモニアの料金は月額で、' * 200000)[:2_000_000]
            identifier = store.add_document(owner, 'big one', body)
            self.assertEqual(store.documents(owner)[0]['characters'], 2_000_000)
            self.assertEqual([d['id'] for d in store.search_documents(owner, 'ハルモニア')], [identifier])
            with self.assertRaises(ValueError):
                store.add_document(owner, 'too big', body + 'x')


class PrivateSearchRankingTests(unittest.TestCase):
    """Titles shaped like imported bundles, bodies synthetic. The two checks:
    「Orion Suiteについて」 -> the Orion Suite explanation first; 「ハルモニアの料金」 -> the ハルモニア LP/price page first."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='fast-research-rank-')
        self.store = Store(Path(self.tmp.name) / 'rank.sqlite')
        self.owner = self.store.register('Synthetic rank', 'fixture-password-2026-rank')
        self.other = self.store.register('Synthetic other', 'fixture-password-2026-other', False)
        filler = 'Orion Suite の各メイトは予約した時刻に投稿します。ハルモニアの会員は料金のプランを選びます。' * 60
        docs = [
            ('Orionスイート: 写真メイトの機能', '写真の投稿を作る。Orion Suite の一部。' + filler * 3),
            ('Orionスイート: 連絡メイトの機能', '連絡の配信を作る。Orion Suite の一部。' + filler * 2),
            ('Orionスイート: 全メイト共通の決まり', 'スケジュールしたら投稿する。' + filler),
            ('Orionスイートについて(Orion Suite プラグインの説明)', 'Orion Suite は Codex のプラグインとして各 SNS の投稿を作る。' + filler[:200]),
            ('会社の規則: 会社の憲章(CHARTER)', '会社の決まり。' + filler),
            ('サービス: ハル(ハルモニアの AI)にできる事', 'ハルはハルモニアの会員の AI。' + filler * 2),
            ('公開の文: ハルモニアの公開ページ(LP・料金・法定ページ)', 'ハルモニアの料金は月額のプランです。' + filler),
            ('wiki: 運用の決まり・アプリ(ops-app)', '運用の決まり。' + filler * 8),
            ('公開の文: 会社HP(株式会社の例)', '会社の公開の文。' + filler * 6),
        ]
        for title, body in docs:
            self.store.add_document(self.owner, title, body)
        self.store.add_document(self.other, 'Orionスイートについて(Orion Suite プラグインの説明)', 'another person has the same title; never visible to the first')

    def tearDown(self):
        self.tmp.cleanup()

    def test_fts5_index_is_used(self):
        self.assertTrue(self.store.fts, 'this SQLite must have FTS5 for the ranking below')
        self.assertEqual(fts_text('Orion Suiteについて'), 'orion suite につ つい いて')
        self.assertEqual(fts_query('ハルモニアの料金'), '"ハル ルモ モニ ニア アの の料 料金" OR "ハル" OR "ルモ" OR "モニ" OR "ニア" OR "アの" OR "の料" OR "料金"')
        self.assertEqual(terms('Orion Suiteについて'), {'orion', 'suite', 'につ', 'つい', 'いて'})

    def test_suite_question_puts_the_explanation_first(self):
        titles = [d['title'] for d in self.store.search_documents(self.owner, 'Orion Suiteについて')]
        self.assertEqual(titles[0], 'Orionスイートについて(Orion Suite プラグインの説明)', titles)
        self.assertNotIn('another person', ' '.join(d['body'] for d in self.store.search_documents(self.owner, 'Orion Suiteについて')))

    def test_product_price_puts_the_price_page_first(self):
        titles = [d['title'] for d in self.store.search_documents(self.owner, 'ハルモニアの料金')]
        self.assertEqual(titles[0], '公開の文: ハルモニアの公開ページ(LP・料金・法定ページ)', titles)

    def test_report_keeps_the_rank_order_and_the_highlight_terms(self):
        research = Research('http://127.0.0.1:1', denylist=None, denylist_optional=True)
        with patch.object(research, 'fetch', side_effect=AssertionError('private network leak')):
            report = research.private('ハルモニアの料金', self.store, self.owner)
        self.assertEqual(report['status'], 'ok')
        self.assertEqual(report['sources'][0]['title'], '公開の文: ハルモニアの公開ページ(LP・料金・法定ページ)')
        self.assertEqual(report['sources'][0]['citation'], 'L1')
        self.assertIn('料金', report['sources'][0]['match_terms'])
        self.assertFalse(report['external_query_sent'])

    def test_a_phrase_nobody_wrote_about_returns_nothing(self):
        # every bigram of this query exists somewhere in the corpus, but no document has more than half of them
        self.assertEqual(self.store.search_documents(self.owner, '存在しない語句ゼロ件'), [])
        self.assertEqual(self.store.search_documents(self.owner, 'xyzzy 未登録の話題'), [])
        research = Research('http://127.0.0.1:1', denylist=None, denylist_optional=True)
        report = research.private('存在しない語句ゼロ件', self.store, self.owner)
        self.assertEqual((report['status'], report['sources']), ('empty', []))
        # the same corpus still answers the real question first
        self.assertEqual(self.store.search_documents(self.owner, 'Orion Suiteについて')[0]['title'], 'Orionスイートについて(Orion Suite プラグインの説明)')

    def test_a_majority_hit_behind_many_partial_matches_is_still_found(self):
        """F2 of review 001: 40 short documents carry 2 of the 5 query words with a high frequency (they outrank everything in
        BM25) while the only real hit carries 3 of 5 once each in a long body. The hit must still come back, alone."""
        for i in range(40):
            self.store.add_document(self.owner, f'妨害 {i}', 'kappa lambda ' * 200 + str(i))
        self.store.add_document(self.owner, '本命 sigma omega theta', ('長い本文。' * 2000) + ' sigma が一度、 omega が一度、 theta が一度。')
        titles = [d['title'] for d in self.store.search_documents(self.owner, 'kappa lambda sigma omega theta')]
        self.assertEqual(titles, ['本命 sigma omega theta'])
        self.store.fts = False
        try:
            self.assertEqual([d['title'] for d in self.store.search_documents(self.owner, 'kappa lambda sigma omega theta')], ['本命 sigma omega theta'])
        finally:
            self.store.fts = True

    def test_one_part_per_bundle_comes_before_second_parts(self):
        for i in range(1, 6):
            self.store.add_document(self.owner, f'決定の記録(DECISIONS) 2026-10 {i}/5', 'ハルモニアの料金の決定 ' * 40)
        titles = [d['title'] for d in self.store.search_documents(self.owner, 'ハルモニアの料金', limit=4)]
        self.assertEqual(sum(t.startswith('決定の記録') for t in titles), 1, titles)

    def test_listing_does_not_depend_on_body_size_and_counts_characters(self):
        rows = self.store.documents(self.owner)
        self.assertNotIn('body', rows[0])
        self.assertTrue(all(isinstance(r['characters'], int) and r['characters'] > 0 for r in rows))
        self.assertEqual(self.store.document_count(self.owner), 9)

    def test_rows_written_before_the_index_existed_are_indexed_on_open(self):
        with self.store.connect() as db:
            db.execute('DROP TABLE documents_fts')
        reopened = Store(self.store.path)
        self.assertTrue(reopened.fts)
        self.assertEqual(reopened.search_documents(self.owner, 'Orion Suiteについて')[0]['title'], 'Orionスイートについて(Orion Suite プラグインの説明)')
        with reopened.connect() as db:
            self.assertEqual(db.execute('SELECT count(*) FROM documents_fts').fetchone()[0], db.execute('SELECT count(*) FROM documents').fetchone()[0])

    def test_scan_fallback_without_fts_gives_the_same_set(self):
        self.store.fts = False
        titles = {d['title'] for d in self.store.search_documents(self.owner, 'ハルモニアの料金')}
        self.store.fts = True
        self.assertEqual(titles, {d['title'] for d in self.store.search_documents(self.owner, 'ハルモニアの料金')})

    def test_restored_documents_are_indexed_too(self):
        payload = {'format': 'rustxng-private-backup-v1', 'documents': [{'title': 'Orionスイート: 短文メイトの機能', 'body': '短文の投稿を Draftly に下書きする。Orion Suite。'}], 'preferences': {'profile': 'general'}, 'reports': []}
        self.assertEqual(self.store.restore(self.owner, payload), 1)
        self.assertIn('Orionスイート: 短文メイトの機能', [d['title'] for d in self.store.search_documents(self.owner, 'Draftly')])

    def test_search_is_fast_on_a_few_hundred_documents(self):
        for i in range(300):
            self.store.add_document(self.owner, f'埋め草 {i}', ('会員の投稿と料金のプランと予約の時刻。' * 200) + str(i))
        started = time.monotonic()
        self.store.search_documents(self.owner, 'ハルモニアの料金')
        self.assertLess(time.monotonic() - started, 1.0)


if __name__ == '__main__':
    unittest.main()
