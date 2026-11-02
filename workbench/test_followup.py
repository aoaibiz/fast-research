"""Follow-up (2026-11-02): delete / replace a private document, a backup restore that does not depend on the JSON size (pieces +
one document at a time), notes from the local AI (MCP private tools, opt-in), the inbox folder and the AI session digest.
Synthetic accounts, temporary storage, no network."""
import http.client
import io
import json
import os
import sqlite3
import tempfile
import threading
import time
import unittest
from collections import namedtuple
from pathlib import Path
from unittest import mock

from . import digest, mcp, notes
from .agent import PrivateNotes, config
from . import server as server_module
from .server import App, MAX_INPUT, Server, Uploads
from .store import Store

SECRET_GH = 'gh' + 'p_' + 'A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8'
SECRET_SK = 's' + 'k-proj-' + 'AbCdEf0123456789XyZ0123'


class Http:
    """A server on an ephemeral port with two synthetic people."""
    def __init__(self, tmp, limits=None):
        self.app = App(Path(tmp) / 'fixture.sqlite', 0, 'http://127.0.0.1:1', limits=limits)
        self.server = Server(self.app); self.port = self.server.server_address[1]
        self.app.port = self.port; self.app.origin = f'http://localhost:{self.port}'; self.app.allowed_hosts = {f'localhost:{self.port}'}
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True); self.thread.start()
        self.a = self.app.store.register('Synthetic A', 'fixture-password-2026-A')
        self.b = self.app.store.register('Synthetic B', 'fixture-password-2026-B', False)
        self.sa = self.app.store.login('Synthetic A', 'fixture-password-2026-A')
        self.sb = self.app.store.login('Synthetic B', 'fixture-password-2026-B')

    def close(self):
        self.server.shutdown(); self.server.server_close(); self.thread.join()

    def raw(self, method, path, body=None, session=None, headers=None, timeout=120):
        h = {'Host': f'localhost:{self.port}', 'Origin': self.app.origin}
        if session:
            h.update({'Cookie': 'rxsession=' + session['token'], 'X-CSRF-Token': session['csrf']})
        if body is not None and 'Content-Type' not in (headers or {}):
            h['Content-Type'] = 'application/json'
        h.update(headers or {})
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=timeout)
        try:
            conn.request(method, path, body, h); r = conn.getresponse(); data = r.read()
            return r.status, (json.loads(data) if data[:1] in (b'{', b'[') else data)
        finally:
            conn.close()

    def call(self, path, payload=None, session=None, headers=None):
        return self.raw('POST' if payload is not None else 'GET', path, json.dumps(payload) if payload is not None else None, session, headers)


def search_titles(store, owner, query):
    return [d['title'] for d in store.search_documents(owner, query)]


class DocumentDeleteReplaceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='fr-followup-'); cls.h = Http(cls.tmp.name)

    @classmethod
    def tearDownClass(cls):
        cls.h.close(); cls.tmp.cleanup()

    def test_delete_removes_the_row_the_index_and_the_listing_and_only_for_the_owner(self):
        h = self.h; store = h.app.store
        doc = store.add_document(h.a, 'Orion delete target', 'Orion zephyrquartz material')
        self.assertIn('Orion delete target', search_titles(store, h.a, 'zephyrquartz'))
        self.assertEqual(h.call(f'/api/documents/{doc}/delete', {}, h.sb)[0], 404, "another person's id looks missing")
        self.assertEqual(h.call(f'/api/documents/{doc}/delete', {})[0], 401)
        for override in [{'X-CSRF-Token': ''}, {'Origin': 'https://attacker.invalid'}, {'Host': 'attacker.invalid'}, {'Sec-Fetch-Site': 'cross-site'}]:
            with self.subTest(override=override):
                self.assertEqual(h.call(f'/api/documents/{doc}/delete', {}, h.sa, override)[0], 403)
        self.assertEqual(h.call(f'/api/documents/{doc}/delete', session=h.sa)[0], 404, 'GET is not a delete')
        self.assertEqual(h.call(f'/api/documents/{doc}', session=h.sa)[0], 200, 'still there after every refused attempt')
        self.assertEqual(h.call(f'/api/documents/{doc}/delete', {}, h.sa), (200, {'ok': True, 'deleted': 1}))
        self.assertNotIn(doc, [d['id'] for d in h.call('/api/documents', session=h.sa)[1]['documents']])
        self.assertEqual(search_titles(store, h.a, 'zephyrquartz'), [])
        with sqlite3.connect(store.path) as db:
            self.assertEqual(db.execute('SELECT count(*) FROM documents WHERE id=?', (doc,)).fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT count(*) FROM documents_fts WHERE doc=?", (doc,)).fetchone()[0], 0)
        self.assertEqual(h.call(f'/api/documents/{doc}/delete', {}, h.sa)[0], 404)

    def test_replace_keeps_the_id_and_the_old_body_is_gone_from_search(self):
        h = self.h; store = h.app.store
        doc = store.add_document(h.a, 'Orion guide', 'the old text says amberfjord twice amberfjord')
        theirs = store.add_document(h.b, 'Orion guide', 'B keeps amberfjord')
        code, data = h.call(f'/api/documents/{doc}/replace', {'body': 'the new text says cobaltmeadow'}, h.sa)
        self.assertEqual((code, data), (200, {'ok': True, 'id': doc, 'replaced': 1}))
        self.assertEqual(h.call(f'/api/documents/{doc}', session=h.sa)[1]['body'], 'the new text says cobaltmeadow')
        self.assertEqual(search_titles(store, h.a, 'cobaltmeadow'), ['Orion guide'])
        self.assertEqual(search_titles(store, h.a, 'amberfjord'), [], 'the old body is not found any more')
        self.assertEqual(search_titles(store, h.b, 'amberfjord'), ['Orion guide'], "B's document is untouched")
        listed = {d['id']: d for d in h.call('/api/documents', session=h.sa)[1]['documents']}
        self.assertEqual(listed[doc]['characters'], len('the new text says cobaltmeadow'))
        self.assertEqual(h.call(f'/api/documents/{doc}/replace', {'body': 'the new text says cobaltmeadow'}, h.sa)[1]['replaced'], 0)
        self.assertEqual(h.call(f'/api/documents/{theirs}/replace', {'body': 'hijack'}, h.sa)[0], 404)
        self.assertEqual(store.document(h.b, theirs)['body'], 'B keeps amberfjord')
        self.assertEqual(h.call(f'/api/documents/{doc}/replace', {'body': 'x'}, h.sa, {'X-CSRF-Token': ''})[0], 403)
        self.assertEqual(h.call(f'/api/documents/{doc}/replace', {'body': ''}, h.sa)[1]['error']['code'], 'document_length')
        self.assertEqual(h.call(f'/api/documents/{doc}/replace', {'body': 'y', 'title': ''}, h.sa)[1]['error']['code'], 'invalid_title')
        other = store.add_document(h.a, 'Orion other', 'same words')
        code, data = h.call(f'/api/documents/{doc}/replace', {'title': 'Orion other', 'body': 'same words'}, h.sa)
        self.assertEqual((code, data['error']['code']), (400, 'invalid_request'), 'would duplicate another document')
        self.assertEqual(store.document(h.a, doc)['body'], 'the new text says cobaltmeadow')
        self.assertTrue(store.document(h.a, other))

    def test_caches_follow_changes_made_by_another_process(self):
        h = self.h
        before = len(h.app.store.documents(h.a))
        other = Store(h.app.store.path)  # e.g. the MCP entry or the inbox import, another process on the same file
        doc = other.add_document(h.a, 'Orion from elsewhere', 'written by another Store')
        self.assertEqual(len(h.call('/api/documents', session=h.sa)[1]['documents']), before + 1)
        other.replace_document(h.a, doc, 'longer body written by another Store')
        listed = {d['id']: d for d in h.app.store.documents(h.a)}
        self.assertEqual(listed[doc]['characters'], len('longer body written by another Store'))
        other.delete_document(h.a, doc)
        self.assertEqual(len(h.app.store.documents(h.a)), before)


class ReplaceCapacityTests(unittest.TestCase):
    def test_replace_counts_the_new_size_against_the_cap_and_leaves_the_document_on_refusal(self):
        with tempfile.TemporaryDirectory() as tmp:
            store = Store(Path(tmp) / 's.sqlite', {'document_bytes_per_owner': 100, 'document_chars': 1000, 'documents_per_owner': 5})
            owner = store.register('Synthetic cap', 'fixture-password-2026-cap')
            doc = store.add_document(owner, 'cap', 'a' * 50)
            store.replace_document(owner, doc, 'b' * 90)  # 3+90 <= 100: the old 53 bytes are not counted twice
            with self.assertRaises(ValueError) as caught:
                store.replace_document(owner, doc, 'c' * 98)
            self.assertEqual(str(caught.exception), 'document_limit')
            self.assertEqual(store.document(owner, doc)['body'], 'b' * 90)
            self.assertIsNone(store.replace_document(owner, 'missing-id', 'z'))
            self.assertFalse(store.delete_document(owner, 'missing-id'))


def escape_heavy_library(store, owner, total_bytes, part_chars):
    """Documents made mostly of backslashes and quotes (each is two bytes in JSON), with one findable word per document."""
    unit = '\\"'  # a backslash and a quote: two bytes each in JSON
    made = 0; i = 0
    while True:
        title = f'Escapes {i}'
        body = (f'quokkastream{i} ' + unit * (part_chars // len(unit) + 1))[:part_chars]
        size = len(title.encode()) + len(body.encode())
        if made + size > total_bytes:
            return i
        store.add_document(owner, title, body); made += size; i += 1


class RestoreTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='fr-restore-')

    def tearDown(self):
        self.tmp.cleanup()

    def library_backup(self, limits, fill):
        source = Store(Path(self.tmp.name) / 'source.sqlite', limits)
        owner = source.register('Synthetic source', 'fixture-password-2026-src')
        made = escape_heavy_library(source, owner, int(limits['document_bytes_per_owner'] * fill), limits['document_chars'] // 4)
        out = io.BytesIO(); source.export_to(owner, out)
        return source, owner, made, out.getvalue()

    def test_export_to_writes_the_same_backup_as_export(self):
        limits = {'document_bytes_per_owner': 200_000, 'document_chars': 20_000, 'documents_per_owner': 100}
        source, owner, made, backup = self.library_backup(limits, 0.5)
        self.assertEqual(json.loads(backup), json.loads(json.dumps(source.export(owner))))

    def test_a_near_cap_escape_heavy_backup_restores_whole_and_in_pieces(self):
        """Review 002 F1, scaled: the decoded library is 99.5% of the cap, its JSON is about twice that (backslashes, quotes), above
        the old request cap (1.25x + 11MB). Both the one-request restore and the pieces restore bring every document back."""
        limits = {'document_bytes_per_owner': 20_000_000, 'document_chars': 2_000_000, 'documents_per_owner': 10_000}
        source, owner, made, backup = self.library_backup(limits, 0.995)
        old_cap = int((limits['document_bytes_per_owner'] + 10_000_000) * 1.25) + 1_000_000
        self.assertGreater(len(backup), old_cap, 'the case is only meaningful above the old cap')
        h = Http(Path(self.tmp.name) / 'target', limits)
        try:
            self.assertGreaterEqual(h.app.restore_max, len(backup))
            self.assertEqual(h.call('/api/session', session=h.sa)[1]['limits']['restore_bytes'], h.app.restore_max)
            code, data = h.raw('POST', '/api/restore', backup, h.sa)
            self.assertEqual((code, data), (200, {'ok': True, 'restored': made}))
            # the same file again, in pieces, for B: start -> pieces -> finish -> state
            code, up = h.call('/api/restore/upload', {'size': len(backup)}, h.sb)
            self.assertEqual(code, 200); self.assertLessEqual(up['chunk_bytes'], MAX_INPUT)
            base = '/api/restore/upload/' + up['upload']
            for offset in range(0, len(backup), up['chunk_bytes']):
                code, r = h.raw('POST', base, backup[offset:offset + up['chunk_bytes']], h.sb, {'Content-Type': 'application/octet-stream', 'X-Upload-Offset': str(offset)})
                self.assertEqual(code, 200); self.assertEqual(r['received'], min(len(backup), offset + up['chunk_bytes']))
            self.assertEqual(h.call(base + '/finish', {}, h.sb), (202, {'ok': True, 'state': 'restoring'}))
            for _ in range(600):
                state = h.call(base, session=h.sb)[1]
                if state['state'] != 'restoring':
                    break
                time.sleep(0.1)
            self.assertEqual((state['state'], state.get('restored')), ('done', made))
            self.assertEqual(list((Path(self.tmp.name) / 'target' / 'restore-uploads').iterdir()), [], 'the uploaded copy is gone')
        finally:
            h.close()
        reopened = Store(Path(self.tmp.name) / 'target' / 'fixture.sqlite', limits)  # "restart"
        for person in (h.a, h.b):
            self.assertEqual(reopened.document_count(person), made)
            self.assertEqual(search_titles(reopened, person, f'quokkastream{made - 1}'), [f'Escapes {made - 1}'])
            self.assertEqual(reopened.document(person, reopened.search_documents(person, 'quokkastream0')[0]['id'])['body'], source.search_documents(owner, 'quokkastream0')[0]['body'])

    def test_upload_pieces_are_checked_and_owned(self):
        h = Http(Path(self.tmp.name) / 'pieces')
        try:
            body = json.dumps({'format': 'rustxng-private-backup-v1', 'documents': [{'title': 'Orion piece', 'body': 'Orion piece body'}]}).encode()
            up = h.call('/api/restore/upload', {'size': len(body)}, h.sa)[1]; base = '/api/restore/upload/' + up['upload']
            octet = {'Content-Type': 'application/octet-stream'}
            self.assertEqual(h.raw('POST', base, body[:10], h.sa, {**octet, 'X-Upload-Offset': '5'})[0], 409, 'pieces only in order')
            self.assertEqual(h.raw('POST', base, body[:10], h.sb, {**octet, 'X-Upload-Offset': '0'})[0], 404, "someone else's upload")
            self.assertEqual(h.raw('POST', base, body[:10], h.sa, {'Content-Type': 'application/json', 'X-Upload-Offset': '0'})[0], 415)
            self.assertEqual(h.raw('POST', base, body[:10], h.sa, {**octet, 'X-Upload-Offset': '0', 'X-CSRF-Token': ''})[0], 403)
            self.assertEqual(h.call(base, session=h.sb)[0], 404)
            self.assertEqual(h.raw('POST', base, body[:10], h.sa, {**octet, 'X-Upload-Offset': '0'})[0], 200)
            self.assertEqual(h.call(base + '/finish', {}, h.sa)[1]['error']['code'], 'invalid_backup', 'not all of it yet')
            self.assertEqual(h.raw('POST', base, body[10:] + b'xx', h.sa, {**octet, 'X-Upload-Offset': '10'})[0], 413, 'more than declared')
            self.assertEqual(h.raw('POST', base, body[10:], h.sa, {**octet, 'X-Upload-Offset': '10'})[1]['received'], len(body))
            self.assertEqual(h.call(base + '/finish', {}, h.sa)[0], 202)
            for _ in range(100):
                state = h.call(base, session=h.sa)[1]
                if state['state'] != 'restoring':
                    break
                time.sleep(0.05)
            self.assertEqual((state['state'], state['restored']), ('done', 1))
            self.assertEqual(h.call('/api/restore/upload', {'size': h.app.restore_max + 1}, h.sa)[0], 413)
            self.assertEqual(h.call('/api/restore/upload', {'size': 0}, h.sa)[0], 400)
            # one request larger than the cap is refused from its Content-Length, before the body is read
            conn = http.client.HTTPConnection('127.0.0.1', h.port, timeout=30)
            conn.putrequest('POST', '/api/restore', skip_host=True)
            for k, v in {'Host': f'localhost:{h.port}', 'Origin': h.app.origin, 'Cookie': 'rxsession=' + h.sa['token'], 'X-CSRF-Token': h.sa['csrf'],
                         'Content-Type': 'application/json', 'Content-Length': str(h.app.restore_max + 1)}.items():
                conn.putheader(k, v)
            conn.endheaders(); self.assertEqual(conn.getresponse().status, 413); conn.close()
        finally:
            h.close()

    def test_a_refused_restore_leaves_the_library_as_it_was(self):
        limits = {'document_bytes_per_owner': 300, 'document_chars': 200, 'documents_per_owner': 10}
        store = Store(Path(self.tmp.name) / 'r.sqlite', limits)
        owner = store.register('Synthetic r', 'fixture-password-2026-r')
        store.add_document(owner, 'kept', 'kept body')
        too_much = {'format': 'rustxng-private-backup-v1', 'documents': [{'title': f't{i}', 'body': 'x' * 100} for i in range(3)]}
        cases = [(too_much, 'document_limit'),
                 ({'format': 'rustxng-private-backup-v1', 'documents': [{'title': 'ok', 'body': 'ok'}, {'title': '', 'body': 'x'}]}, 'invalid_backup'),
                 ({'format': 'other', 'documents': [{'title': 'ok', 'body': 'ok'}]}, 'invalid_backup'),
                 ({'format': 'rustxng-private-backup-v1', 'documents': [{'title': 'ok', 'body': 'ok'}], 'reports': [{'payload': {'query': 'q', 'sources': [{'url': 'javascript:x'}]}}]}, 'invalid_backup')]
        for payload, code in cases:
            for how in ('object', 'stream'):
                with self.subTest(code=code, how=how), self.assertRaises(ValueError) as caught:
                    store.restore(owner, payload) if how == 'object' else store.restore_stream(owner, io.BytesIO(json.dumps(payload).encode()))
                self.assertEqual(str(caught.exception), code)
                self.assertEqual([d['title'] for d in store.documents(owner)], ['kept'])

    def test_the_reader_handles_window_edges_and_refuses_what_is_not_one_backup(self):
        store = Store(Path(self.tmp.name) / 'w.sqlite')
        owner = store.register('Synthetic w', 'fixture-password-2026-w')
        doc = {'id': 'old', 'title': 'Orion window', 'body': 'é\\"' * 40000 + '\u3042' * 30000, 'created': 1730000000.123456789}
        report = {'payload': {'query': 'Orion', 'scope': 'private', 'status': 'ok', 'sources': [{'id': 'old', 'scope': 'private', 'url': ''}], 'markdown': 'm' * 70000}, 'created': 12345678901234567890}
        text = json.dumps({'format': 'rustxng-private-backup-v1', 'documents': [doc], 'preferences': {'profile': 'sales'}, 'reports': [report]}, ensure_ascii=False, indent=3)
        self.assertEqual(store.restore_stream(owner, io.BytesIO(text.encode())), 1)
        self.assertEqual(store.documents(owner, True)[0]['body'], doc['body'])
        restored = store.report(owner, store.reports(owner)[0]['id'])
        self.assertEqual(restored['sources'][0]['id'], store.documents(owner)[0]['id'], 'report ids follow the new document id')
        self.assertEqual(store.preferences(owner), {'profile': 'sales'})
        for bad in [text + 'x', text[:-1], '[]', text.replace('"format"', '"pad": "' + 'x' * 1_000_001 + '", "format"', 1)]:
            with self.subTest(bad=bad[:20] + '...' + bad[-10:]), self.assertRaises(ValueError) as caught:
                store.restore_stream(owner, io.BytesIO(bad.encode()))
            self.assertEqual(str(caught.exception), 'invalid_backup')
        with self.assertRaises(ValueError):
            store.restore_stream(owner, io.BytesIO(b'{"format": "rustxng-private-backup-v1", "documents": [{"title": "\xff\xfe", "body": "x"}]}'))

    @unittest.skipUnless(os.environ.get('FAST_RESEARCH_NEAR_CAP') == '1', 'the full-size run (300MB, a few minutes): FAST_RESEARCH_NEAR_CAP=1')
    def test_full_size_default_cap(self):
        from .store import DEFAULT_LIMITS
        limits = dict(DEFAULT_LIMITS)
        source, owner, made, backup = self.library_backup(limits, 0.995)
        h = Http(Path(self.tmp.name) / 'full', limits)
        try:
            self.assertGreater(len(backup), int((limits['document_bytes_per_owner'] + 10_000_000) * 1.25) + 1_000_000)
            self.assertEqual(h.raw('POST', '/api/restore', backup, h.sa, timeout=1800), (200, {'ok': True, 'restored': made}))
        finally:
            h.close()


Disk = namedtuple('Disk', 'total used free')


def disk_free(nbytes, headroom=1000):
    """The disk as the server sees it: `nbytes` free (a constant, whatever is written), and a small safety margin."""
    return mock.patch.multiple(server_module.shutil, disk_usage=mock.Mock(return_value=Disk(10 ** 12, 10 ** 12 - nbytes, nbytes))), mock.patch.object(Uploads, 'HEADROOM', headroom)


class UploadCapacityTests(unittest.TestCase):
    """Review 001 F2: what several people are sending at the same time is counted together, and what is given up is let go."""
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='fr-cap-')
        self.h = Http(Path(self.tmp.name) / 'cap')
        self.app.store.register('Synthetic C', 'fixture-password-2026-C', False)
        self.sc = self.app.store.login('Synthetic C', 'fixture-password-2026-C')

    app = property(lambda self: self.h.app)

    def tearDown(self):
        self.h.close(); self.tmp.cleanup()

    def start(self, session, size):
        return self.h.call('/api/restore/upload', {'size': size}, session)

    def test_uploads_started_by_different_people_are_counted_together_and_let_go_when_given_up(self):
        a, b = disk_free(10_000)
        with a, b:
            first = self.start(self.h.sa, 4_000)
            self.assertEqual(first[0], 200)
            self.assertEqual(self.start(self.h.sb, 6_000)[1].get('error', {}).get('code'), 'storage_error', '4000 + 6000 + 1000 of margin does not fit in 10000')
            self.assertEqual(self.start(self.h.sb, 5_000)[0], 200, '4000 + 5000 + 1000 fits')
            third = self.start(self.sc, 1_000)
            self.assertEqual((third[0], third[1]['error']['code']), (507, 'storage_error'), '4000 + 5000 + 1000 + 1000 does not')
            # the same person starting again gives up their own earlier upload (nothing is counted twice)
            self.assertEqual(self.start(self.h.sb, 5_000)[0], 200)
            # a person's upload nobody touched for the idle time is let go by the next start, with its file
            old = self.app.uploads.items[first[1]['upload']]
            old['touched'] -= Uploads.IDLE + 1
            self.assertEqual(self.start(self.sc, 1_000)[0], 200, 'the abandoned 4000 no longer counts')
            self.assertEqual(self.h.call('/api/restore/upload/' + first[1]['upload'], session=self.h.sa)[0], 404)
            self.assertFalse(old['path'].exists())
            self.assertEqual(len(list(self.app.uploads.dir.iterdir())), 2, 'only the two live uploads have a file')

    def test_only_what_is_still_to_come_of_an_upload_is_counted(self):
        a, b = disk_free(10_000)
        with a, b:
            up = self.start(self.h.sa, 6_000)[1]
            octet = {'Content-Type': 'application/octet-stream'}
            self.assertEqual(self.start(self.h.sb, 4_000)[0], 507, '6000 + 4000 + 1000 > 10000')
            self.assertEqual(self.h.raw('POST', '/api/restore/upload/' + up['upload'], b'x' * 3_000, self.h.sa, {**octet, 'X-Upload-Offset': '0'})[0], 200)
            self.assertEqual(self.start(self.h.sb, 4_000)[0], 200, '3000 still to come + 4000 + 1000 fits')

    def test_a_restore_sent_whole_and_a_backup_in_the_making_hold_their_room_until_they_end(self):
        a, b = disk_free(1_600, 100)
        with a, b:
            body = json.dumps({'format': 'rustxng-private-backup-v1', 'documents': [{'title': 'Orion whole', 'body': 'w' * 400}]}).encode()
            conn = http.client.HTTPConnection('127.0.0.1', self.h.port, timeout=20)
            conn.putrequest('POST', '/api/restore', skip_host=True)
            for k, v in {'Host': f'localhost:{self.h.port}', 'Origin': self.app.origin, 'Cookie': 'rxsession=' + self.h.sa['token'], 'X-CSRF-Token': self.h.sa['csrf'],
                         'Content-Type': 'application/json', 'Content-Length': str(len(body))}.items():
                conn.putheader(k, v)
            conn.endheaders(); conn.send(body[:100])
            for _ in range(100):
                if any(f.name.startswith('single-') for f in self.app.uploads.dir.iterdir()):
                    break
                time.sleep(0.05)
            self.assertEqual(self.start(self.h.sb, 1_100)[1].get('error', {}).get('code'), 'storage_error', 'the whole-restore body on its way (about 480) + 1100 + 100 > 1600')
            conn.send(body[100:]); response = conn.getresponse(); self.assertEqual(response.status, 200); response.read(); conn.close()
            self.assertEqual(self.start(self.h.sb, 1_100)[0], 200, 'its room is given back when it ends: 1100 + 100 <= 1600')
        a, b = disk_free(1_100, 100)
        with a, b:
            self.assertEqual(self.h.call('/api/backup', session=self.h.sa)[1].get('error', {}).get('code'), 'storage_error', 'a backup that cannot fit is not started')
        a, b = disk_free(10_000_000, 100)
        with a, b:
            self.assertEqual(self.h.raw('GET', '/api/backup', session=self.h.sa)[0], 200)
            self.assertEqual(self.start(self.h.sb, 9_000_000)[0], 200, 'and the export gave its room back')


class FinishCanBeSentAgainTests(unittest.TestCase):
    """Review 001 F1: a lost answer to finish must not make a running or finished restore look like a failure."""
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='fr-fin-')
        self.h = Http(Path(self.tmp.name) / 'fin')

    def tearDown(self):
        self.h.close(); self.tmp.cleanup()

    def upload(self, session, body):
        up = self.h.call('/api/restore/upload', {'size': len(body)}, session)[1]; base = '/api/restore/upload/' + up['upload']
        self.assertEqual(self.h.raw('POST', base, body, session, {'Content-Type': 'application/octet-stream', 'X-Upload-Offset': '0'})[0], 200)
        return base

    def wait(self, base, session, until):
        for _ in range(200):
            state = self.h.call(base, session=session)[1]
            if state['state'] in until:
                return state
            time.sleep(0.05)
        self.fail('the restore did not reach ' + str(until))

    def test_finish_again_while_restoring_and_after_done_restores_once(self):
        body = json.dumps({'format': 'rustxng-private-backup-v1', 'documents': [{'title': 'Orion finish', 'body': 'finish body'}]}).encode()
        base = self.upload(self.h.sa, body)
        release = threading.Event(); real = Store.restore_stream; calls = []

        def slow(store, owner, fileobj):
            calls.append(owner); release.wait(20)
            return real(store, owner, fileobj)
        with mock.patch.object(Store, 'restore_stream', slow):
            self.assertEqual(self.h.call(base + '/finish', {}, self.h.sa), (202, {'ok': True, 'state': 'restoring'}))
            again = self.h.call(base + '/finish', {}, self.h.sa)
            self.assertEqual(again, (202, {'ok': True, 'state': 'restoring'}), 'the same finish sent again while it runs is not an error')
            self.assertEqual(self.h.call(base + '/finish', {}, self.h.sb)[0], 404, "someone else's upload")
            release.set(); done = self.wait(base, self.h.sa, ('done', 'failed'))
        self.assertEqual((done['state'], done['restored']), ('done', 1))
        self.assertEqual(self.h.call(base + '/finish', {}, self.h.sa), (202, {'ok': True, 'state': 'done'}), 'and after it is done')
        self.assertEqual((len(calls), self.h.call(base, session=self.h.sa)[1]['restored']), (1, 1), 'the library was restored once')
        self.assertEqual([d['title'] for d in self.h.app.store.documents(self.h.a)], ['Orion finish'])

    def test_finish_again_after_a_failed_restore_reports_the_failure_and_does_not_run_it_again(self):
        base = self.upload(self.h.sa, b'{"format": "other", "documents": []}')
        real = Store.restore_stream; calls = []

        def counting(store, owner, fileobj):
            calls.append(1)
            return real(store, owner, fileobj)
        with mock.patch.object(Store, 'restore_stream', counting):
            self.assertEqual(self.h.call(base + '/finish', {}, self.h.sa)[0], 202)
            failed = self.wait(base, self.h.sa, ('done', 'failed'))
            self.assertEqual((failed['state'], failed['error']['code']), ('failed', 'invalid_backup'))
            self.assertEqual(self.h.call(base + '/finish', {}, self.h.sa), (202, {'ok': True, 'state': 'failed'}))
        self.assertEqual(len(calls), 1)


class NotesTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='fr-notes-')
        self.data = Path(self.tmp.name)
        self.store = Store(self.data / 'workbench.sqlite')
        self.owner = notes.local_owner(self.store)

    def tearDown(self):
        self.tmp.cleanup()

    def test_redaction_matches_the_notes_ingest_rules(self):
        jwt = '.'.join(['ey' + 'JhbGciOiJIUzI1NiJ9', 'ey' + 'JzdWIiOiIxMjM0NTY3ODkwIn0', 'dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U'])
        pem = notes.D5 + 'BEGIN RSA PRIVATE KEY' + notes.D5 + '\nMIIabc\n' + notes.D5 + 'END RSA PRIVATE KEY' + notes.D5
        for text, needle in [('key ' + SECRET_SK + ' end', 'AbCdEf0123'), ('token: abcdefgh12345678', 'abcdefgh12345678'), ('jwt ' + jwt, jwt[:12]),
                             ('rnd Zx8Qp2Lm9Vb4Nt7Rk1Wy6Hs3Jd5Fg0Ac8Ue2Io4 x', 'Zx8Qp2Lm9'), (pem, 'MIIabc'), (SECRET_GH, SECRET_GH[4:12])]:
            self.assertNotIn(needle, notes.redact(text)[0])
        for keep in ['6ae81636-e8b0-83ee-a73a-c415521aedb3', '手元の資料への保存を確かめる KAMOME7816', '/notes/2026-11-02-save-test.md', 'https://example.org/c/6ae81636-e8b0-83ee-a73a-c415521aedb3']:
            self.assertEqual(notes.redact(keep)[0], keep)

    def test_save_note_strips_secrets_and_keeps_one_copy(self):
        first = notes.save_note(self.store, self.owner, 'Orion の決定', f'要点: 次は Orion を出す。鍵 {SECRET_GH}')
        self.assertTrue(first['saved']); self.assertGreaterEqual(first['redacted'], 1)
        body = self.store.document(self.owner, first['id'])['body']
        self.assertNotIn(SECRET_GH, body); self.assertIn('[REDACTED]', body)
        again = notes.save_note(self.store, self.owner, 'Orion の決定', f'要点: 次は Orion を出す。鍵 {SECRET_GH}')
        self.assertEqual((again['saved'], again['duplicate'], again['id']), (False, True, first['id']))
        self.assertEqual(self.store.document_count(self.owner), 1)

    def test_private_search_and_read_stay_within_4kb_and_read_pages_back_the_whole_body(self):
        long_body = ('Orion の料金は月額です。' + 'あいうえお' * 50 + '\n') * 300
        for i in range(12):
            self.store.add_document(self.owner, f'Orion 料金 {i}', long_body + str(i))
        result = notes.search_private(self.store, self.owner, 'Orion の料金', 10)
        self.assertLessEqual(len(json.dumps(result, ensure_ascii=False).encode()), 4000)
        self.assertTrue(result['results']); self.assertEqual(result['results'][0]['kind'], 'document')
        ident = result['results'][0]['id']; text = ''; offset = 0; pages = 0
        while offset is not None:
            page = notes.read_private(self.store, self.owner, ident, offset)
            self.assertLessEqual(len(json.dumps(page, ensure_ascii=False).encode()), 4000)
            text += page['text']; offset = page['next_offset']; pages += 1
        self.assertEqual(text, self.store.document(self.owner, ident)['body']); self.assertGreater(pages, 10)
        with self.assertRaises(ValueError):
            notes.read_private(self.store, self.owner, 'missing', 0)

    def test_saved_research_is_found_too(self):
        self.store.save_report(self.owner, {'query': 'Orion の競合', 'scope': 'public', 'sources': [], 'markdown': 'Orion の競合は三社。'})
        kinds = [r['kind'] for r in notes.search_private(self.store, self.owner, 'Orion の競合')['results']]
        self.assertIn('research', kinds)

    def test_mcp_offers_private_tools_only_when_opted_in(self):
        init = {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {}}
        listing = {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list', 'params': {}}
        _, ok = mcp.handle(init, None, False)
        names = [t['name'] for t in mcp.handle(listing, None, ok)[0]['result']['tools']]
        self.assertEqual(names, ['search_public'], 'default: public web only')
        refused = mcp.handle({'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call', 'params': {'name': 'save_note', 'arguments': {'title': 't', 'body': 'b'}}}, None, ok)[0]
        self.assertEqual(refused['error']['code'], -32602)
        private = PrivateNotes(self.data)
        names = [t['name'] for t in mcp.handle(listing, None, ok, private)[0]['result']['tools']]
        self.assertEqual(names, ['search_public', 'search_private', 'read_private', 'save_note'])
        def call(name, arguments):
            r = mcp.handle({'jsonrpc': '2.0', 'id': 9, 'method': 'tools/call', 'params': {'name': name, 'arguments': arguments}}, None, ok, private)[0]
            return r.get('error') or (r['result']['isError'], json.loads(r['result']['content'][0]['text']))
        saved = call('save_note', {'title': 'Orion の会議', 'body': '大事な話: Orion は来月。' + SECRET_SK})
        self.assertFalse(saved[0]); self.assertTrue(saved[1]['saved'])
        found = call('search_private', {'query': 'Orion の会議'})
        self.assertEqual(found[1]['results'][0]['id'], saved[1]['id'])
        page = call('read_private', {'id': saved[1]['id']})
        self.assertIn('Orion は来月', page[1]['text']); self.assertNotIn(SECRET_SK, page[1]['text'])
        self.assertEqual(call('save_note', {'title': 'x'})['code'], -32602)
        self.assertEqual(call('read_private', {'id': 'missing'}), (True, {'stage': 'private', 'code': 'not_found'}))
        self.assertEqual(call('search_private', {'query': 'q', 'extra': 1})['code'], -32602)

    def test_registration_is_opt_in(self):
        plain = config('/fixture/rustxng')['mcpServers']['fast-research']['args']
        self.assertNotIn('--private', plain)
        private = config('/fixture/rustxng', True, '/fixture/data')['mcpServers']['fast-research']['args']
        self.assertEqual(private[-3:], ['--private', '--data', str(Path('/fixture/data').resolve())])

    def test_inbox_imports_moves_to_done_dedupes_and_replaces_by_file_name(self):
        inbox = self.data / 'inbox'; inbox.mkdir()
        old = time.time() - 120
        def drop(name, text):
            p = inbox / name; p.write_text(text, encoding='utf-8'); os.utime(p, (old, old)); return p
        drop('2026-11-02-orion.md', f'# 題\nOrion の壁打ち\n\n価格は月額。鍵 {SECRET_GH}\n')
        drop('memo.txt', 'Orion のメモ本文')
        drop('.hidden.md', 'not me'); drop('_draft.md', 'not me'); drop('notes.pdf', 'not me')
        fresh = inbox / 'fresh.md'; fresh.write_text('just written', encoding='utf-8')
        (inbox / 'link.md').symlink_to(inbox / 'memo.txt')
        events = notes.import_inbox(self.store, self.data)
        self.assertEqual(sorted(e['event'] for e in events), ['imported', 'imported'])
        titles = sorted(d['title'] for d in self.store.documents(self.owner))
        self.assertEqual(titles, ['Orion の壁打ち(2026-11-02)', f'memo({time.strftime("%Y-%m-%d", time.localtime(old))})'])
        doc = next(d for d in self.store.documents(self.owner, True) if d['title'].startswith('Orion'))
        self.assertNotIn(SECRET_GH, doc['body'])
        self.assertEqual(sorted(p.name for p in (inbox / 'done').iterdir()), ['2026-11-02-orion.md', 'memo.txt'])
        self.assertTrue(fresh.exists() and (inbox / '.hidden.md').exists() and (inbox / 'notes.pdf').exists())
        drop('copy.md', 'Orion のメモ本文')  # the same content as memo.txt under another name: stored once
        drop('2026-11-02-orion.md', '# 題\nOrion の壁打ち\n\n価格は年額に変えた。\n')  # the same name again: replaces
        events = notes.import_inbox(self.store, self.data)
        self.assertEqual(sorted(e['event'] for e in events), ['duplicate', 'replaced'])
        self.assertEqual(self.store.document_count(self.owner), 2)
        self.assertEqual(self.store.document(self.owner, doc['id'])['body'], '# 題\nOrion の壁打ち\n\n価格は年額に変えた。\n')
        self.assertEqual(sorted(p.name for p in (inbox / 'done').iterdir()), ['2026-11-02-orion-1.md', '2026-11-02-orion.md', 'copy.md', 'memo.txt'])

    def test_digest_keeps_only_what_was_said_and_strips_secrets(self):
        day = '2026-11-01'; t = time.mktime(time.strptime(day + ' 10:00', '%Y-%m-%d %H:%M'))
        iso = lambda s: time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(s)) + 'Z'
        claude = self.data / 'claude-projects' / '-home-someone-orion'; claude.mkdir(parents=True)
        rows = [{'type': 'user', 'timestamp': iso(t), 'message': {'content': 'Orion の価格を決めたい。' + SECRET_SK}},
                {'type': 'assistant', 'timestamp': iso(t + 5), 'message': {'content': [{'type': 'text', 'text': 'Orion は月額 1,000 円で始め、三か月後に利用者の数と解約の理由を見て、値上げか据え置きかを見直すのが良いと思います。'}, {'type': 'tool_use', 'name': 'Bash', 'input': {'command': 'cat secret'}}]}},
                {'type': 'user', 'timestamp': iso(t + 6), 'message': {'content': [{'type': 'tool_result', 'content': 'TOOL OUTPUT MUST NOT APPEAR'}]}},
                {'type': 'user', 'timestamp': iso(t + 7), 'isSidechain': True, 'message': {'content': 'SIDECHAIN MUST NOT APPEAR'}},
                {'type': 'user', 'timestamp': iso(t - 86400), 'message': {'content': 'YESTERDAY MUST NOT APPEAR'}}]
        (claude / 's.jsonl').write_text('\n'.join(json.dumps(r) for r in rows), encoding='utf-8')
        codex = self.data / 'codex'; sessions = codex / 'sessions' / '2026' / '11' / '01'; sessions.mkdir(parents=True)
        crow = [{'type': 'session_meta', 'payload': {'cwd': '/home/someone/orion'}},
                {'type': 'event_msg', 'timestamp': iso(t + 60), 'payload': {'type': 'user_message', 'message': 'Orion の競合を調べて'}},
                {'type': 'event_msg', 'timestamp': iso(t + 90), 'payload': {'type': 'task_complete', 'last_agent_message': '競合は三社でした。'}},
                {'type': 'response_item', 'timestamp': iso(t + 80), 'payload': {'type': 'function_call_output', 'output': 'TOOL OUTPUT MUST NOT APPEAR'}}]
        (sessions / 'rollout.jsonl').write_text('\n'.join(json.dumps(r) for r in crow), encoding='utf-8')
        path = digest.write_digest(self.data, day, str(codex), str(self.data / 'claude-projects'))
        text = path.read_text(encoding='utf-8')
        self.assertEqual(path.name, f'ai-sessions-{day}.md')
        for present in ['Orion の価格を決めたい', '月額 1,000 円', 'Orion の競合を調べて', '競合は三社でした']:
            self.assertIn(present, text)
        for absent in ['MUST NOT APPEAR', SECRET_SK[:10], 'cat secret']:
            self.assertNotIn(absent, text)
        os.utime(path, (time.time() - 120, time.time() - 120))
        notes.import_inbox(self.store, self.data)
        self.assertEqual([d['title'] for d in self.store.documents(self.owner)], ['AI との会話(2026-11-01)'])
        self.assertIsNone(digest.write_digest(self.data, '2026-10-01', str(codex), str(self.data / 'claude-projects')))
