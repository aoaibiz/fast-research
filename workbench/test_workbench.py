"""Real HTTP boundary tests with synthetic accounts and isolated temporary storage."""
import http.client
import json
import tempfile
import threading
import urllib.parse
import unittest
from pathlib import Path
from unittest.mock import patch
from .server import App, Server
from .store import Store
from .core import Research, ResearchError


class BoundaryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='rustxng-fixture-')
        cls.app = App(Path(cls.tmp.name)/'fixture.sqlite', 0, 'http://127.0.0.1:1')
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

    def request(self, path, payload=None, session=None, overrides=None):
        headers = {'Host':f'localhost:{self.port}', 'Origin':self.app.origin}
        if session:
            headers.update({'Cookie':'rxsession='+session['token'], 'X-CSRF-Token':session['csrf']})
        if payload is not None: headers['Content-Type']='application/json'
        headers.update(overrides or {})
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=5)
        conn.request('POST' if payload is not None else 'GET', path, json.dumps(payload) if payload is not None else None, headers)
        response=conn.getresponse(); data=response.read(); status=response.status
        conn.close()
        return status,json.loads(data)

    def test_owner_boundaries_and_persistence(self):
        code,data=self.request('/api/documents', {'title':'Synthetic project Orion','body':'Project Orion budget is fictional 42 yen.'}, self.sa)
        self.assertEqual(code,201); identifier=data['id']
        self.assertEqual(self.request('/api/documents/'+identifier,session=self.sb)[0],404)
        self.assertEqual(self.request('/api/documents/'+identifier)[0],401)
        self.assertEqual(self.request('/api/documents',session=self.sb)[1]['documents'],[])
        reopened=Store(self.app.store.path)
        self.assertIn('42 yen',reopened.document(self.a,identifier)['body'])

    def test_private_search_never_uses_network_and_report_is_scoped(self):
        self.app.store.add_document(self.a,'Synthetic confidential Orion','Orion fixture-only private material')
        with patch.object(self.app.research,'fetch',side_effect=AssertionError('private network leak')), patch.object(self.app.research.opener,'open',side_effect=AssertionError('private network leak')):
            code,data=self.request('/api/search',{'scope':'private','query':'Orion'},self.sa)
        self.assertEqual(code,200); self.assertFalse(data['external_query_sent'])
        self.assertTrue(data['sources']); self.assertEqual(data['queries'],[])
        self.assertEqual(self.request('/api/reports/'+data['id'],session=self.sb)[0],404)
        self.assertEqual(self.request('/api/reports/'+data['id'],session=self.sa)[0],200)

    def test_csrf_origin_host_and_role(self):
        for override in [{'X-CSRF-Token':''},{'Origin':'https://attacker.invalid'},{'Host':'attacker.invalid'},{'Sec-Fetch-Site':'cross-site'}]:
            self.assertEqual(self.request('/api/documents',{'title':'bad','body':'bad'},self.sa,override)[0],403)
        for path in ['/api/accounts','/api/restart','/api/stop']:
            self.assertEqual(self.request(path,{},self.sb)[0],403)
        self.assertEqual(self.request('/api/setup',{'name':'Synthetic C','password':'fixture-password-C'})[0],400)

    def test_public_consent_and_separate_combined_query(self):
        self.assertEqual(self.request('/api/search',{'scope':'public','query':'public'},self.sa)[0],400)
        def public(q,*args):
            self.assertEqual(q,'Python pathlib public')
            return {'status':'partial','sources':[],'queries':[q],'created_at':'fixture','markdown':'fixture'}
        with patch.object(self.app.research,'public',side_effect=public):
            code,data=self.request('/api/search',{'scope':'combined','query':'PRIVATE-ORION','public_query':'Python pathlib public','public_confirmed':True},self.sa)
        self.assertEqual(code,200); self.assertNotIn('PRIVATE',json.dumps(data['public']))

    def test_backup_idempotency_owner_and_atomicity(self):
        owner=self.app.store.register('Synthetic backup','fixture-password-backup',False)
        first=self.app.store.add_document(owner,'Synthetic backup doc','fixture only')
        self.assertEqual(first,self.app.store.add_document(owner,'Synthetic backup doc','fixture only'))
        backup=self.app.store.export(owner)
        self.assertEqual(self.app.store.restore(owner,backup),0)
        backup['documents'].append({'title':'valid but rolled back','body':'fixture'})
        backup['documents'].append({'title':'invalid','body':None})
        with self.assertRaises(ValueError):self.app.store.restore(owner,backup)
        self.assertEqual(len(self.app.store.documents(owner)),1)
        self.assertNotIn('digest',json.dumps(self.app.store.export(owner)))

    def test_restore_reports_preferences_and_remap_document_ids(self):
        original=self.app.store.register('Synthetic original','fixture-password-original',False)
        dest=self.app.store.register('Synthetic destination','fixture-password-destination',False)
        docid=self.app.store.add_document(original,'Restore Orion','Orion fixture body')
        report=self.app.research.private('Orion',self.app.store,original)
        self.app.store.save_report(original,report)
        self.app.store.preferences(original,{'profile':'sns'})
        exported=self.app.store.export(original)
        self.assertEqual(self.app.store.restore(dest,exported),1)
        self.assertEqual(self.app.store.preferences(dest),{'profile':'sns'})
        restored=self.app.store.report(dest,self.app.store.reports(dest)[0]['id'])
        new_id=restored['sources'][0]['id']
        self.assertNotEqual(new_id,docid)
        self.assertIsNotNone(self.app.store.document(dest,new_id))
        self.assertEqual(self.app.store.restore(dest,exported),0)
        self.assertEqual(len(self.app.store.reports(dest)),1)

    def test_combined_failure_preserves_local_evidence(self):
        self.app.store.add_document(self.a,'Orion retained','Orion fixture must remain available')
        with patch.object(self.app.research,'public',side_effect=ResearchError('search_unavailable')):
            code,data=self.request('/api/search',{'scope':'combined','query':'Orion','public_query':'public Python','public_confirmed':True},self.sa)
        self.assertEqual(code,200);self.assertEqual(data['status'],'partial')
        self.assertTrue(data['sources']);self.assertIsNone(data['external_query_sent'])
        self.assertEqual(data['public']['errors'][0]['code'],'search_unavailable')

    def test_ownership_guard_blocks_before_query_transport(self):
        from .ownership import guard_for_state
        research=Research('http://127.0.0.1:1')
        research.guard=guard_for_state(Path(self.tmp.name)/'nonexistent-instance.json',1)
        with patch.object(research.opener,'open',side_effect=AssertionError('query leaked')):
            with self.assertRaises(ResearchError) as error:research.public('public fixture')
        self.assertEqual(error.exception.stage,'ownership')

    def test_failure_has_safe_stage_and_no_content_in_diagnostics(self):
        with patch.object(self.app.research,'public',side_effect=ResearchError('search_unavailable')):
            code,data=self.request('/api/search',{'scope':'public','query':'Synthetic-private-marker','public_confirmed':True},self.sa)
        self.assertEqual(code,502); self.assertEqual(data['error']['stage'],'search')
        self.assertTrue(data['error']['request_id'])
        self.assertNotIn('Synthetic-private-marker',json.dumps(list(self.app.events)))
        code,data=self.request('/api/operations',session=self.sa)
        self.assertEqual(code,200); self.assertFalse(data['backend_healthy'])

    def test_logout_revokes_session(self):
        session=self.app.store.login('Synthetic A','fixture-password-2026-A')
        self.assertEqual(self.request('/api/logout',{},session)[0],200)
        self.assertEqual(self.request('/api/documents',session=session)[0],401)

    def raw_get(self, path, headers):
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=5)
        conn.request('GET', path, headers=headers)
        response=conn.getresponse(); body=response.read(); conn.close()
        return response.status,response.getheader('Location'),body

    def test_unknown_page_address_goes_back_to_the_screen(self):
        # A chat app glued the following Japanese words onto the shared link: the browser opened that path.
        glued='/'+urllib.parse.quote('(利用者名「見本」・パスワードは前と同じ)')
        host={'Host':f'localhost:{self.port}'}
        for path in [glued, glued+'?x=1', '/foo', '/foo/bar', '/'+urllib.parse.quote('見本')]:
            for headers in [host, {**host,'Cookie':'rxsession='+self.sa['token']}, {**host,'Sec-Fetch-Site':'cross-site','Sec-Fetch-Mode':'navigate'}]:
                status,location,body=self.raw_get(path,headers)
                self.assertEqual((status,location,body),(302,'/',b''),(path,headers))
        # The screen itself and the API keep their answers.
        self.assertEqual(self.raw_get('/',host)[0],200)
        code,data=self.request('/api/xxx')
        self.assertEqual((code,data['error']['code']),(401,'authentication_required'))
        self.assertEqual(self.request('/api/xxx',session=self.sa)[0],404)
        # The boundary checks still run first.
        self.assertEqual(self.raw_get('/foo',{'Host':'attacker.invalid'})[0],403)
        self.assertEqual(self.raw_get('/foo',{**host,'Origin':'https://attacker.invalid'})[0],403)
        self.assertEqual(self.raw_get('/api/documents',{**host,'Sec-Fetch-Site':'cross-site'})[0],403)


if __name__=='__main__':unittest.main()
