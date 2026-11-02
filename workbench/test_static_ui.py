"""The workspace screen is served under a strict CSP: every script is a same-origin file, no inline script/style/handlers.
These checks pin the parts of the redesigned screen that other code and the E2E rely on."""
import http.client
import re
import tempfile
import threading
import unittest
from pathlib import Path
from .server import App, Server

STATIC = Path(__file__).resolve().parent/'static'
# Every id app.js reaches for (the redesign kept all of them) plus the ones it added.
REQUIRED_IDS = ['view-gate', 'auth-form', 'auth-password', 'pw-toggle', 'auth-submit', 'gate-title', 'view-search', 'search-form', 'query',
                'search-submit', 'options', 'public-query-row', 'result', 'view-reports', 'report-list', 'report-detail', 'report-back', 'saved-result',
                'view-documents', 'document-form', 'document-list', 'document-detail', 'view-settings', 'health', 'refresh-health', 'restart',
                'restore-form', 'restore-file', 'account-form', 'account-controls', 'diagnostics', 'stop', 'view-ai', 'mcp-download', 'notice',
                'identity', 'logout', 'gate-ai-link', 'nav', 'app', 'scope-chip', 'recent', 'side-foot', 'health-dot', 'profile']


class StaticScreenTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='rustxng-static-')
        cls.app = App(Path(cls.tmp.name)/'fixture.sqlite', 0, 'http://127.0.0.1:1')
        cls.http = Server(cls.app)
        cls.port = cls.http.server_address[1]
        cls.app.port = cls.port
        cls.app.origin = f'http://localhost:{cls.port}'
        cls.app.allowed_hosts = {f'localhost:{cls.port}'}
        cls.thread = threading.Thread(target=cls.http.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.http.shutdown(); cls.http.server_close(); cls.thread.join()
        cls.tmp.cleanup()

    def get(self, path):
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=5)
        conn.request('GET', path, None, {'Host': f'localhost:{self.port}'})
        response = conn.getresponse(); body = response.read(); headers = dict(response.getheaders()); status = response.status
        conn.close()
        return status, headers, body

    def test_every_page_file_is_served_with_its_type_under_the_strict_csp(self):
        for path, kind in (('/', 'text/html'), ('/app.js', 'text/javascript'), ('/boot.js', 'text/javascript'), ('/style.css', 'text/css')):
            with self.subTest(path=path):
                status, headers, body = self.get(path)
                self.assertEqual(status, 200)
                self.assertTrue(headers['Content-Type'].startswith(kind), headers['Content-Type'])
                self.assertIn("script-src 'self'", headers['Content-Security-Policy'])
                self.assertIn("style-src 'self'", headers['Content-Security-Policy'])
                self.assertGreater(len(body), 100)

    def test_other_static_names_are_not_served(self):
        for path in ('/index.html', '/static/app.js', '/boot.js.map', '/../server.py'):
            with self.subTest(path=path):
                self.assertNotEqual(self.get(path)[0], 200)

    def test_index_has_no_inline_code_and_keeps_every_id(self):
        html = (STATIC/'index.html').read_text(encoding='utf-8')
        scripts = re.findall(r'<script\b[^>]*>', html)
        self.assertEqual(len(scripts), 2)
        self.assertTrue(all(re.search(r'\bsrc="/(boot|app)\.js"', s) for s in scripts), scripts)
        self.assertNotRegex(html, r'<script\b[^>]*>\s*[^<\s]')  # nothing between <script ...> and </script>
        self.assertNotRegex(html, r'<style\b')
        self.assertNotRegex(html, r'\sstyle="')
        self.assertNotRegex(html, r'\son[a-z]+="')
        self.assertLess(html.index('/boot.js'), html.index('/style.css'), 'the theme is applied before the stylesheet paints')
        ids = re.findall(r'\bid="([^"]+)"', html)
        self.assertEqual(len(ids), len(set(ids)), 'duplicate id')
        for name in REQUIRED_IDS:
            self.assertIn(name, ids)

    def test_scope_labels_replace_the_send_sentence(self):
        html = (STATIC/'index.html').read_text(encoding='utf-8')
        self.assertNotIn('scope-note', html)
        labels = re.findall(r'<input type="radio" name="scope" value="(\w+)"[^>]*><span><b>.*?</b><small>([^<]+)</small>', html)
        self.assertEqual(labels, [('public', '外部へ送信'), ('private', '外部へ送らない'), ('combined', '公開分を送信')])

    def test_page_script_builds_the_dom_without_html_strings(self):
        js = (STATIC/'app.js').read_text(encoding='utf-8')
        code = '\n'.join(line for line in js.splitlines() if not line.lstrip().startswith('//'))
        for banned in ('innerHTML', 'outerHTML', 'insertAdjacentHTML', 'document.write', "setAttribute('style'", 'eval('):
            self.assertFalse(banned in code, banned)
        # the public-send flag follows the chosen scope only: true for public/combined, false for private
        self.assertIn("const publicScope = scope !== 'private';", js)
        self.assertIn('public_confirmed: publicScope,', js)

    def test_boot_script_reads_the_theme_safely(self):
        js = (STATIC/'boot.js').read_text(encoding='utf-8')
        self.assertIn("localStorage.getItem('talmudo_theme')", js)
        self.assertIn('try', js)
        self.assertIn('catch', js)


if __name__ == '__main__':
    unittest.main()
