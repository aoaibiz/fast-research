"""How the search backend introduces itself (User-Agent): browser by default, honest on request."""
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from workbench import server
from workbench.core import ResearchError

ROOT = Path(__file__).resolve().parent.parent


class IdentityEnvTests(unittest.TestCase):
    def test_default_is_browser(self):
        self.assertEqual(server.identity_env({}), {'RUSTXNG_IDENTITY': 'browser'})

    def test_honest_on_request(self):
        self.assertEqual(server.identity_env({'TALMUD_RESEARCH_IDENTITY': ' Honest '}), {'RUSTXNG_IDENTITY': 'honest'})

    def test_custom_honest_string_is_passed(self):
        env = server.identity_env({'TALMUD_RESEARCH_IDENTITY': 'honest', 'TALMUD_RESEARCH_USER_AGENT': 'ExampleBot/1.0 (+https://example.org/bot)'})
        self.assertEqual(env, {'RUSTXNG_IDENTITY': 'honest', 'RUSTXNG_USER_AGENT': 'ExampleBot/1.0 (+https://example.org/bot)'})

    def test_unknown_mode_and_bad_strings_are_refused(self):
        for environ in ({'TALMUD_RESEARCH_IDENTITY': 'stealth'}, {'TALMUD_RESEARCH_USER_AGENT': 'x' * 257},
                        {'TALMUD_RESEARCH_USER_AGENT': 'Bot/1\r\nX-Injected: 1'}):
            with self.assertRaises(ResearchError) as caught:
                server.identity_env(environ)
            self.assertEqual((caught.exception.code, caught.exception.stage), ('identity_invalid', 'startup'))
        self.assertIn('identity_invalid', server.MESSAGES)


class BackendPassesIdentityTests(unittest.TestCase):
    def spawn_env(self, environ):
        seen = {}
        class Stop(Exception):
            pass
        def fake_popen(*args, **kwargs):
            seen.update(kwargs['env'])
            raise Stop()
        with tempfile.TemporaryDirectory() as folder:
            binary = Path(folder) / 'rustxng'
            binary.write_bytes(b'')
            backend = server.Backend(binary, 1, folder)
            with patch.dict(os.environ, environ, clear=False), patch.object(server.SPAWNER, 'popen', fake_popen):
                for name in ('TALMUD_RESEARCH_IDENTITY', 'TALMUD_RESEARCH_USER_AGENT'):
                    if name not in environ:
                        os.environ.pop(name, None)
                with self.assertRaises(Stop):
                    backend.start()
        return seen

    def test_default_spawn_is_browser(self):
        env = self.spawn_env({})
        self.assertEqual(env['RUSTXNG_IDENTITY'], 'browser')
        self.assertNotIn('RUSTXNG_USER_AGENT', env)

    def test_honest_spawn(self):
        self.assertEqual(self.spawn_env({'TALMUD_RESEARCH_IDENTITY': 'honest'})['RUSTXNG_IDENTITY'], 'honest')


class PublishedTreeTests(unittest.TestCase):
    def test_source_zip_lists_only_existing_docs(self):
        text = (ROOT / 'workbench' / 'server.py').read_text(encoding='utf-8')
        for name in ('docs/INSTALL-AI.md', 'docs/AI-USAGE.md', 'docs/WORKBENCH.md', 'docs/LOCAL-EVALUATION.md', 'docs/UPSTREAM-README.md'):
            self.assertIn(name, text)
            self.assertTrue((ROOT / name).is_file(), name)


if __name__ == '__main__':
    unittest.main()
