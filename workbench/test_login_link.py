"""Easy sign-in: this PC signs in by itself, other devices use a one-time link. Real HTTP, synthetic accounts, temporary storage."""
import http.client
import json
from pathlib import Path
import secrets
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest

from . import qr
from .server import App, Server
from .store import LINK_SECONDS, LINK_SESSION_SECONDS, SESSION_SECONDS, Store

PUBLIC = 'https://preview-fixture.example'
REPO = Path(__file__).resolve().parent.parent


class Running:
    """One workbench on a free port with a public origin, like the tunnel preview."""
    def __init__(self, public=PUBLIC):
        self.tmp = tempfile.TemporaryDirectory(prefix='rustxng-link-')
        self.app = App(Path(self.tmp.name)/'fixture.sqlite', 0, 'http://127.0.0.1:1', None, public)
        self.http = Server(self.app)
        self.port = self.http.server_address[1]
        self.app.port = self.port
        self.app.origin = f'http://localhost:{self.port}'
        self.app.allowed_hosts = {f'localhost:{self.port}', f'127.0.0.1:{self.port}'} | ({public.split('//', 1)[1]} if public else set())
        self.local = {'Host': f'localhost:{self.port}', 'Origin': self.app.origin}
        self.tunnel = {'Host': public.split('//', 1)[1], 'Origin': public, 'Cf-Connecting-Ip': '203.0.113.9', 'Cf-Ray': 'fixture'} if public else None
        self.thread = threading.Thread(target=self.http.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.http.shutdown(); self.http.server_close(); self.thread.join(); self.tmp.cleanup()

    def call(self, method, path, headers, payload=None, session=None):
        headers = dict(headers)
        if session:
            headers.update({'Cookie': 'rxsession='+session['token'], 'X-CSRF-Token': session['csrf']})
        body = None
        if payload is not None:
            headers['Content-Type'] = 'application/json'; body = json.dumps(payload)
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=5)
        conn.request(method, path, body, headers)
        response = conn.getresponse(); data = response.read(); conn.close()
        return response.status, json.loads(data) if data else None, response.getheader('Set-Cookie') or ''


def cookie_token(set_cookie):
    return set_cookie.split(';', 1)[0].split('=', 1)[1]


class ThisPcTests(unittest.TestCase):
    def setUp(self):
        self.w = Running()

    def tearDown(self):
        self.w.close()

    def test_first_run_on_this_pc_sets_up_and_signs_in_the_admin(self):
        code, data, _ = self.w.call('GET', '/api/session', self.w.local)
        self.assertEqual((code, data['user'], data['setup_needed'], data['this_pc'], data['phone_link']), (200, None, True, True, True))
        code, data, cookie = self.w.call('POST', '/api/local-login', self.w.local, {})
        self.assertEqual((code, data['user']['role'], data['user']['name']), (200, 'admin', '自分'))
        self.assertIn('HttpOnly; Secure; SameSite=Strict; Max-Age=%d' % SESSION_SECONDS, cookie)
        session = {'token': cookie_token(cookie), 'csrf': data['csrf']}
        self.assertEqual(self.w.call('GET', '/api/preferences', self.w.local, session=session)[0], 200)
        # Next time: the same first admin, no second account.
        code, again, _ = self.w.call('POST', '/api/local-login', {'Host': f'127.0.0.1:{self.w.port}', 'Origin': f'http://127.0.0.1:{self.w.port}'}, {})
        self.assertEqual((code, again['user']['id']), (200, data['user']['id']))
        with sqlite3.connect(self.w.app.store.path) as db:
            self.assertEqual(db.execute('SELECT COUNT(*) FROM users').fetchone()[0], 1)

    def test_this_pc_uses_the_first_admin_not_a_member(self):
        admin = self.w.app.store.register('Synthetic Admin', 'fixture-password-2026-A')
        self.w.app.store.register('Synthetic Member', 'fixture-password-2026-B', False)
        code, data, _ = self.w.call('POST', '/api/local-login', self.w.local, {})
        self.assertEqual((code, data['user']['id'], data['user']['role']), (200, admin, 'admin'))

    def test_no_automatic_sign_in_through_the_tunnel_or_another_site(self):
        self.w.app.store.register('Synthetic Admin', 'fixture-password-2026-A')
        code, data, _ = self.w.call('GET', '/api/session', self.w.tunnel)
        self.assertEqual((code, data['this_pc'], data['user']), (200, False, None))
        refused = [
            self.w.tunnel,
            {**self.w.tunnel, 'Origin': self.w.app.origin},  # tunnel host with a forged local Origin
            {**self.w.local, 'Cf-Connecting-Ip': '203.0.113.9'},  # tunnel that rewrites Host to loopback still adds its headers
            {**self.w.local, 'Cf-Ray': 'x'}, {**self.w.local, 'X-Forwarded-For': '203.0.113.9'}, {**self.w.local, 'Forwarded': 'for=203.0.113.9'},
            {**self.w.local, 'Origin': PUBLIC},  # a page on the public origin
            {**self.w.local, 'Origin': 'https://attacker.invalid'},
            {**self.w.local, 'Sec-Fetch-Site': 'cross-site'},
            {'Host': 'attacker.invalid', 'Origin': 'http://attacker.invalid'},
        ]
        for headers in refused:
            code, data, cookie = self.w.call('POST', '/api/local-login', headers, {})
            self.assertEqual((code, cookie), (403, ''), headers)
        # A POST without any Origin is refused by the existing boundary.
        self.assertEqual(self.w.call('POST', '/api/local-login', {'Host': f'localhost:{self.w.port}'}, {})[0], 403)

    def test_password_login_still_works_with_csrf(self):
        self.w.app.store.register('Synthetic Admin', 'fixture-password-2026-A')
        code, data, cookie = self.w.call('POST', '/api/login', self.w.tunnel, {'name': 'Synthetic Admin', 'password': 'fixture-password-2026-A'})
        self.assertEqual(code, 200); self.assertIn('SameSite=Strict; Max-Age=%d' % SESSION_SECONDS, cookie)
        session = {'token': cookie_token(cookie), 'csrf': data['csrf']}
        self.assertEqual(self.w.call('POST', '/api/preferences', self.w.tunnel, {'profile': 'general'}, session)[0], 200)
        self.assertEqual(self.w.call('POST', '/api/preferences', self.w.tunnel, {'profile': 'general'}, {**session, 'csrf': 'wrong'})[0], 403)
        self.assertEqual(self.w.call('POST', '/api/login', self.w.tunnel, {'name': 'Synthetic Admin', 'password': 'wrong-password-2026'})[0], 401)


class LoginLinkTests(unittest.TestCase):
    def setUp(self):
        self.w = Running()
        store = self.w.app.store
        store.register('Synthetic Admin', 'fixture-password-2026-A')
        store.register('Synthetic Member', 'fixture-password-2026-B', False)
        self.admin = store.login('Synthetic Admin', 'fixture-password-2026-A')
        self.member = store.login('Synthetic Member', 'fixture-password-2026-B')

    def tearDown(self):
        self.w.close()

    def issue(self, session=None):
        return self.w.call('POST', '/api/phone-link', self.w.local, {}, session or self.admin)

    def use(self, token, headers=None):
        return self.w.call('POST', '/api/link-login', headers or self.w.tunnel, {'token': token})

    def test_link_signs_a_device_in_once_for_thirty_days(self):
        code, data, _ = self.issue()
        self.assertEqual((code, data['expires_in']), (200, LINK_SECONDS))
        self.assertTrue(data['url'].startswith(PUBLIC + '/#link='))
        token = data['url'].split('#link=', 1)[1]
        self.assertEqual(len(token), 43)
        self.assertTrue(data['qr_svg'].startswith('<svg'))
        with sqlite3.connect(self.w.app.store.path) as db:  # only a digest is stored
            self.assertIsNone(db.execute('SELECT 1 FROM login_links WHERE digest=?', (token,)).fetchone())
        code, body, cookie = self.use(token)
        self.assertEqual((code, body['user']['role']), (200, 'admin'))
        self.assertIn('HttpOnly; Secure; SameSite=Strict; Max-Age=%d' % LINK_SESSION_SECONDS, cookie)
        self.assertEqual(LINK_SESSION_SECONDS, 30*24*3600)
        session = {'token': cookie_token(cookie), 'csrf': body['csrf']}
        self.assertEqual(self.w.call('GET', '/api/preferences', self.w.tunnel, session=session)[0], 200)
        with sqlite3.connect(self.w.app.store.path) as db:
            expires = max(row[0] for row in db.execute('SELECT expires FROM sessions'))
        self.assertAlmostEqual(expires - time.time(), LINK_SESSION_SECONDS, delta=60)
        # Used once: the same link no longer works.
        self.assertEqual(self.use(token)[0:1], (401,))
        self.assertEqual(self.use(token)[1]['error']['code'], 'authentication_required')

    def test_expired_tampered_and_malformed_links_are_refused(self):
        token = self.issue()[1]['url'].split('#link=', 1)[1]
        tampered = token[:-1] + ('A' if token[-1] != 'A' else 'B')
        for bad in [tampered, token[:-1], token + 'x', '', None, 42, '見本' * 22, secrets.token_urlsafe(32)]:
            code, _, cookie = self.use(bad)
            self.assertEqual((code, cookie), (401, ''), bad)
        self.assertEqual(self.use(token)[0], 200)  # the tampered tries did not consume the real one
        expired = self.issue()[1]['url'].split('#link=', 1)[1]
        with sqlite3.connect(self.w.app.store.path) as db:
            db.execute('UPDATE login_links SET expires=?', (time.time() - 1,))
        self.assertEqual(self.use(expired)[0], 401)
        with sqlite3.connect(self.w.app.store.path) as db:
            self.assertEqual(db.execute('SELECT COUNT(*) FROM login_links').fetchone()[0], 0)

    def test_only_an_admin_with_csrf_can_issue_and_issuing_is_rate_limited(self):
        self.assertEqual(self.issue(self.member)[0], 403)
        self.assertEqual(self.w.call('POST', '/api/phone-link', self.w.local, {})[0], 401)
        self.assertEqual(self.issue({**self.admin, 'csrf': 'wrong'})[0], 403)
        self.assertEqual(self.w.call('POST', '/api/phone-link', {**self.w.local, 'Origin': 'https://attacker.invalid'}, {}, self.admin)[0], 403)
        self.w.app.limits = {}
        self.assertEqual([self.issue()[0] for _ in range(6)], [200]*5 + [429])
        with self.assertRaises(PermissionError):
            self.w.app.store.issue_login_link(self.member['user']['id'])

    def test_no_link_button_target_without_a_public_origin(self):
        w = Running(public=None)
        try:
            w.app.store.register('Synthetic Admin', 'fixture-password-2026-A')
            admin = w.app.store.login('Synthetic Admin', 'fixture-password-2026-A')
            self.assertFalse(w.call('GET', '/api/session', w.local)[1]['phone_link'])
            self.assertEqual(w.call('POST', '/api/phone-link', w.local, {}, admin)[0], 404)
        finally:
            w.close()

    def test_command_line_prints_a_link_for_the_first_admin(self):
        with tempfile.TemporaryDirectory(prefix='rustxng-link-cli-') as tmp:
            args = [sys.executable, '-m', 'workbench.server', '--data', tmp, '--no-backend', '--issue-login-link', '--public-origin', PUBLIC]
            empty = subprocess.run(args, cwd=REPO, capture_output=True, text=True, timeout=60)
            self.assertNotEqual(empty.returncode, 0)
            store = Store(Path(tmp)/'workbench.sqlite')
            admin = store.register('Synthetic Admin', 'fixture-password-2026-A')
            done = subprocess.run(args, cwd=REPO, capture_output=True, text=True, timeout=60)
            self.assertEqual(done.returncode, 0, done.stderr)
            url = done.stdout.strip()
            self.assertTrue(url.startswith(PUBLIC + '/#link='), url)
            session = store.use_login_link(url.split('#link=', 1)[1])
            self.assertEqual(session['user']['id'], admin)
            self.assertIsNone(store.use_login_link(url.split('#link=', 1)[1]))
            no_origin = subprocess.run(args[:-2], cwd=REPO, capture_output=True, text=True, timeout=60)
            self.assertNotEqual(no_origin.returncode, 0)


class QrTests(unittest.TestCase):
    def test_qr_svg_is_well_formed_and_grows_with_the_text(self):
        short, long = qr.matrix('x'), qr.matrix(PUBLIC + '/#link=' + 'A'*43)
        self.assertEqual(len(short), 21)
        self.assertGreater(len(long), 21)
        self.assertTrue(qr.svg('x').startswith('<svg xmlns="http://www.w3.org/2000/svg"'))
        with self.assertRaises(ValueError):
            qr.matrix('x' * 400)

    def test_qr_decodes_back_to_the_link(self):
        try:
            import cv2, numpy
        except ImportError:
            self.skipTest('OpenCV not installed')
        # OpenCV's default detector misses a few percent of valid codes at a given scale (the same matrix reads at another
        # scale or with the ArUco-based detector), so a code passes when some reader returns exactly the text and none returns other text.
        detectors = [cv2.QRCodeDetector()] + ([cv2.QRCodeDetectorAruco()] if hasattr(cv2, 'QRCodeDetectorAruco') else [])
        def image(grid, scale):
            out = numpy.full(((len(grid)+8)*scale,)*2, 255, numpy.uint8)
            for y, row in enumerate(grid):
                for x, dark in enumerate(row):
                    if dark: out[(y+4)*scale:(y+5)*scale, (x+4)*scale:(x+5)*scale] = 0
            return out
        texts = ['https://demo-words-for-test.trycloudflare.com/#link=' + 'A' * 43, 'x', PUBLIC + '/' + 'y' * 150]
        texts += ['https://demo-words-for-test.trycloudflare.com/#link=' + secrets.token_urlsafe(32) for _ in range(20)]
        for text in texts:
            grid = qr.matrix(text)
            read = [d.detectAndDecode(image(grid, scale))[0] for d in detectors for scale in (5, 8)]
            self.assertIn(text, read)
            self.assertEqual([r for r in read if r and r != text], [], text)


if __name__ == '__main__':
    unittest.main()
