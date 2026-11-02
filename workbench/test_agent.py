"""Protocol/lifecycle tests. Real upstream smoke is separate from synthetic fixtures."""
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from .agent import ManagedResearch, config
from .core import ResearchError
from .mcp import handle, serve


class AgentTests(unittest.TestCase):
    def test_initialization_is_lazy_and_does_not_start_ui_or_backend(self):
        research=ManagedResearch('/nonexistent/fixture-binary')
        response,ready=handle({'jsonrpc':'2.0','id':1,'method':'initialize'},research,False)
        self.assertTrue(ready)
        self.assertEqual(response['result']['protocolVersion'],'2025-06-18')
        self.assertIsNone(research.manager)
        response,_=handle({'jsonrpc':'2.0','id':2,'method':'tools/list'},research,True)
        self.assertEqual(response['result']['tools'][0]['name'],'search_public')
        self.assertIsNone(research.manager)

    def test_unconfirmed_or_private_shaped_query_never_starts_backend(self):
        research=ManagedResearch('/nonexistent/fixture-binary')
        response,_=handle({'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'search_public','arguments':{'query':'public fixture'}}},research,True)
        self.assertTrue(response['result']['isError'])
        with self.assertRaises(ResearchError) as error:research.public('fixture@example.invalid')
        self.assertEqual(error.exception.code,'possible_private_query')
        self.assertIsNone(research.manager)

    def test_missing_binary_has_safe_typed_failure(self):
        research=ManagedResearch('/nonexistent/fixture-binary')
        response,_=handle({'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'search_public','arguments':{'query':'public fixture','public_confirmed':True}}},research,True)
        self.assertTrue(response['result']['isError'])
        data=json.loads(response['result']['content'][0]['text'])
        self.assertEqual(data,{'stage':'startup','code':'backend_missing'})
        self.assertIsNone(research.temporary)

    def test_configuration_is_portable_and_does_not_require_a_ui_session(self):
        spec=config('/fixture/rustxng')['mcpServers']['talmud-research']
        self.assertTrue(Path(spec['command']).is_absolute())
        self.assertTrue(Path(spec['args'][0]).is_absolute())
        self.assertNotIn('--owner-state',spec['args'])
        self.assertNotIn('--backend',spec['args'])

    def test_actual_child_exit_preserves_safe_diagnostic_without_stderr(self):
        with tempfile.TemporaryDirectory(prefix='agent-fixture-') as folder:
            binary=Path(folder)/'fixture'
            binary.write_text('#!/bin/sh\necho synthetic-sensitive-stderr >&2\nexit 7\n')
            binary.chmod(0o700)
            research=ManagedResearch(binary)
            response,_=handle({'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'search_public','arguments':{'query':'public fixture','public_confirmed':True}}},research,True)
            self.assertEqual(json.loads(response['result']['content'][0]['text']),{'stage':'startup','code':'backend_exited_7'})
            self.assertNotIn('synthetic-sensitive',json.dumps(response))
            self.assertIsNone(research.temporary)

    def test_actual_invalid_executable_reports_startup_reason(self):
        with tempfile.TemporaryDirectory(prefix='agent-fixture-') as folder:
            binary=Path(folder)/'fixture';binary.write_text('synthetic invalid executable')
            for mode,expected in [(0o600,'backend_not_executable'),(0o700,'backend_wrong_format')]:
                binary.chmod(mode)
                research=ManagedResearch(binary)
                response,_=handle({'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'search_public','arguments':{'query':'public fixture','public_confirmed':True}}},research,True)
                self.assertTrue(response['result']['isError'])
                self.assertEqual(json.loads(response['result']['content'][0]['text']),{'stage':'startup','code':expected})
                self.assertIsNone(research.manager)
                self.assertIsNone(research.temporary)

    def test_oversized_line_rejected_without_search(self):
        class Input:
            buffer=io.BytesIO(b'x'*65537+b'\n')
        output=io.StringIO()
        with patch('sys.stdin',Input()),patch('sys.stdout',output):serve(ManagedResearch('/nonexistent'))
        self.assertEqual(json.loads(output.getvalue())['error']['message'],'Message too large')

    def test_start_failure_stops_only_own_manager_and_cleans_temp_state(self):
        with tempfile.TemporaryDirectory(prefix='agent-fixture-') as folder:
            binary=Path(folder)/'fixture';binary.write_text('not executable')
            with patch('workbench.agent.Backend') as cls:
                cls.return_value.start.side_effect=ResearchError('backend_start_failed','startup',503)
                research=ManagedResearch(binary)
                with self.assertRaises(ResearchError):research.public('public fixture')
                cls.return_value.stop.assert_called_once()
                self.assertIsNone(research.manager)
                self.assertIsNone(research.temporary)

if __name__=='__main__':unittest.main()
