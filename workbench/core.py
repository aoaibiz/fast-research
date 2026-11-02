"""Bounded research orchestration. Private terms never enter the public transport."""
import datetime as dt
import hashlib
import json
import math
import os
import re
import time
import unicodedata
from pathlib import Path
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict

PROFILES = {
    'general': {'label':'一般調査','engines':'bing,yahoo japan,duckduckgo,brave','domains':[]},
    'development': {'label':'開発・公式仕様','engines':'bing,yahoo japan,duckduckgo,brave','domains':['docs.python.org','doc.rust-lang.org','tokio.rs','developer.mozilla.org','developers.cloudflare.com','docs.github.com','react.dev','nodejs.org','sqlite.org']},
    'sales': {'label':'営業・企業調査','engines':'bing,yahoo japan,duckduckgo,brave','domains':[]},
    'sns': {'label':'SNS・情報確認','engines':'bing,yahoo japan,duckduckgo,brave','domains':['about.fb.com','about.instagram.com','newsroom.tiktok.com','blog.youtube','blog.google','openai.com','anthropic.com']},
}

class ResearchError(Exception):
    def __init__(self, code, stage='search', status=502):
        self.code, self.stage, self.status = code, stage, status
        super().__init__(code)

def now(): return dt.datetime.now(dt.timezone.utc).isoformat()

from .tokens import terms  # one tokenizer shared with the FTS5 index (workbench/tokens.py)

def excerpt(text, query, limit=600):
    """Up to three fragments that really contain a query term. No match -> empty string
    (the caller flags it; the opening sentences of a page are not evidence)."""
    needle=terms(query)
    if not needle or not text: return ''
    fragments=re.split(r'(?<=[。！？.!?])\s*|\n+',text)
    hits=[(i,len(terms(f) & needle)) for i,f in enumerate(fragments)]
    hits=[(i,n) for i,n in hits if n>0]
    ranked=sorted(hits,key=lambda x:(-x[1],x[0]))[:3]
    return ' '.join(fragments[i] for i in sorted(i for i,_ in ranked))[:limit]

# --- private-term / secret guard (review 中1) -------------------------------------
# Known credential shapes. Case-sensitive on purpose: these prefixes are fixed-case.
SECRET_SHAPES=re.compile(
    r'\bgh[pousr]_[A-Za-z0-9]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}|\bglpat-[A-Za-z0-9_-]{16,}'
    r'|\bxox[abposr]-[A-Za-z0-9-]{10,}|\b(?:AKIA|ASIA)[0-9A-Z]{12,}|\bAIza[0-9A-Za-z_-]{20,}'
    r'|\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{4,}|-----BEGIN [A-Z ]*PRIVATE KEY-----'
    r'|\b[MNO][A-Za-z0-9_-]{23,27}\.[A-Za-z0-9_-]{6,7}\.[A-Za-z0-9_-]{27,}'
    r'|\bnpm_[A-Za-z0-9]{30,}|\bsk_(?:live|test)_[A-Za-z0-9]{16,}|\bAGE-SECRET-KEY-|\bsk-ant-[A-Za-z0-9_-]{8,}')
LEGACY_SHAPES=re.compile(r'\b(sk-[a-zA-Z0-9_-]{12,}|Bearer\s+\S+|[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,})',re.I)
URL_IN_TEXT=re.compile(r'https?://[^\s<>"\']+',re.I)
TOKEN_CHARS=re.compile(r'[A-Za-z0-9_+/=-]{24,}')

def shannon_entropy(token):
    counts={}
    for ch in token: counts[ch]=counts.get(ch,0)+1
    return -sum(n/len(token)*math.log2(n/len(token)) for n in counts.values())

def looks_like_secret_token(token):
    """24+ characters that read as a random credential, not as a word or a slug."""
    if len(token)<24: return False
    has=lambda pattern: re.search(pattern,token) is not None
    if has(r'^[0-9a-fA-F]+$') and len(token)>=32 and has('[0-9]') and has('[a-fA-F]'): return True
    return has('[a-z]') and has('[A-Z]') and has('[0-9]') and shannon_entropy(token)>=3.5

def percent_decoded(text):
    """Decode %XX up to twice (double encoding) so an encoded credential is seen as what it is."""
    once=urllib.parse.unquote(text)
    return urllib.parse.unquote(once)

def high_entropy_tokens(text):
    # Every part of a URL can carry a credential (signed or reset links put it in the path):
    # host, userinfo, each path segment, query and fragment are checked one by one, after percent-decoding.
    from_urls=[]
    def keep_url_pieces(match):
        from_urls.extend(re.split(r'[/?&#;=:@\s\'"<>]+',percent_decoded(match.group(0))))
        return ' '
    rest=URL_IN_TEXT.sub(keep_url_pieces,percent_decoded(text))
    candidates=TOKEN_CHARS.findall(rest)
    for piece in from_urls:
        candidates += TOKEN_CHARS.findall(piece)
    return [t for t in candidates if looks_like_secret_token(t)]

def load_denylist(path, optional=False):
    if not path: return []
    from pathlib import Path
    p=Path(path)
    try:
        if not p.is_file():
            if optional: return []
            raise OSError('missing')
        if p.stat().st_size>256*1024: raise OSError('too large')
        lines=p.read_text(encoding='utf-8').splitlines()
    except (OSError,UnicodeDecodeError): raise ResearchError('denylist_unreadable','input',503) from None
    words=[]
    for line in lines[:2000]:
        line=line.strip()
        if line and not line.startswith('#') and len(line)>=2: words.append(' '.join(unicodedata.normalize('NFKC',line).casefold().split()))
    return words

def check_public_query(q, denylist=None, denylist_optional=False):
    """Refuse a query that looks like a credential or contains a registered private term."""
    decoded=percent_decoded(q)
    if LEGACY_SHAPES.search(q) or SECRET_SHAPES.search(q) or LEGACY_SHAPES.search(decoded) or SECRET_SHAPES.search(decoded) or high_entropy_tokens(q):
        raise ResearchError('possible_private_query','input',400)
    words=load_denylist(denylist,denylist_optional)
    if words:
        # The listed words are matched against what the engines will really receive: the query as typed,
        # after percent-decoding (once or twice, '+' as a space too), with width/case folded, runs of spaces
        # collapsed and invisible format characters (zero-width joiners and the like) removed.
        variants={q,percent_decoded(q),urllib.parse.unquote_plus(q),urllib.parse.unquote_plus(urllib.parse.unquote_plus(q))}
        for variant in variants:
            folded=unicodedata.normalize('NFKC',variant).casefold()
            folded=' '.join(''.join(ch for ch in folded if unicodedata.category(ch)!='Cf').split())
            if any(w in folded for w in words): raise ResearchError('private_term_denied','input',400)

# --- official-domain table (small, file-driven; nothing is guessed to be official) ---------
OFFICIAL_FILE=Path(__file__).resolve().parent/'official-domains.txt'
_DOMAIN_ENTRY=re.compile(r'(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}')

def load_official_domains(paths):
    """Domains (suffix match) that are marked 公式. One per line, '#' starts a comment, malformed lines are
    ignored. A missing or unreadable file contributes nothing: unknown stays 'not confirmed'."""
    found=set()
    for path in paths:
        if not path: continue
        try:
            p=Path(path)
            if not p.is_file() or p.stat().st_size>256*1024: continue
            lines=p.read_text(encoding='utf-8').splitlines()
        except (OSError,UnicodeDecodeError): continue
        for line in lines[:5000]:
            entry=line.split('#',1)[0].strip().lower()
            if entry.startswith('*.'): entry=entry[2:]
            if _DOMAIN_ENTRY.fullmatch(entry): found.add(entry)
    return found

def host_key(url):
    host=domain(url)
    return host[4:] if host.startswith('www.') else host

def redirected_offsite(url, final_url):
    """True when the page that was actually read lives on another host than the search hit."""
    return isinstance(final_url,str) and bool(final_url) and host_key(final_url)!=host_key(url)

def domain(url):
    try: return (urllib.parse.urlsplit(url).hostname or '').lower()
    except ValueError: return ''

def matches(host, patterns): return any(host == d or host.endswith('.'+d) for d in patterns)

def validate_query(q):
    if not isinstance(q,str) or not q.strip() or len(q)>500:
        raise ResearchError('invalid_query','input',400)
    return q.strip()

def validate_domains(values):
    if not isinstance(values,list) or len(values)>20:
        raise ResearchError('invalid_domains','input',400)
    cleaned=[]
    for value in values:
        if not isinstance(value,str) or not re.fullmatch(r'(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)+[a-zA-Z]{2,63}',value):
            raise ResearchError('invalid_domains','input',400)
        cleaned.append(value.lower())
    return cleaned

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self,req,fp,code,msg,headers,newurl):return None

class Research:
    def __init__(self, backend='http://127.0.0.1:18889', denylist=None, denylist_optional=False, official_files=None):
        parsed=urllib.parse.urlsplit(backend)
        if parsed.scheme!='http' or parsed.hostname!='127.0.0.1' or parsed.path or parsed.query or parsed.username:
            raise ValueError('backend_must_be_loopback')
        self.guard=None
        self.backend=backend
        # packaged table + optional per-install additions (env var, data folder)
        self.official_files=[OFFICIAL_FILE,os.environ.get('FAST_RESEARCH_OFFICIAL_DOMAINS') or os.environ.get('TALMUD_RESEARCH_OFFICIAL_DOMAINS')]+list(official_files or [])
        self.denylist=denylist or os.environ.get('FAST_RESEARCH_DENYLIST') or os.environ.get('TALMUD_RESEARCH_DENYLIST') or None
        self.denylist_optional=denylist_optional and not (os.environ.get('FAST_RESEARCH_DENYLIST') or os.environ.get('TALMUD_RESEARCH_DENYLIST'))
        self.opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),NoRedirect())

    def fetch(self, query, profile, domains, *, raw=True):
        expected_instance=self.guard() if self.guard else None
        params={'q':query,'format':'json','full':'1','max_results':'10','language':'ja','safesearch':'1','engines':PROFILES[profile]['engines']}
        if raw: params['include_raw_content']='1'
        if domains: params['include_domains']=','.join(domains)
        url=self.backend+'/search?'+urllib.parse.urlencode(params)
        try:
            with self.opener.open(self.backend+'/healthz',timeout=3) as health:
                if expected_instance and health.headers.get('x-rustxng-instance')!=expected_instance:raise ResearchError('backend_version_mismatch','ownership',503)
                if health.headers.get('x-rustxng-safety')!='public-web-v1':raise ResearchError('backend_version_mismatch','startup',503)
            with self.opener.open(url,timeout=20) as response:
                payload=response.read(2*1024*1024+1)
                if len(payload)>2*1024*1024: raise ResearchError('backend_response_too_large')
                data=json.loads(payload)
        except (urllib.error.URLError,TimeoutError): raise ResearchError('search_unavailable') from None
        except (json.JSONDecodeError,UnicodeDecodeError): raise ResearchError('invalid_backend_response') from None
        if not isinstance(data,dict) or not isinstance(data.get('results'),list): raise ResearchError('invalid_backend_response')
        if data.get('error'): raise ResearchError('engine_configuration_error')
        return data

    def public(self, q, profile='general', domains=None, max_queries=2):
        q=validate_query(q)
        if profile not in PROFILES: raise ResearchError('invalid_profile','input',400)
        domains=validate_domains(domains or [])
        check_public_query(q,self.denylist,self.denylist_optional)
        start=time.monotonic(); queries=[]; sources=[]; failures=[]; seen=set(); errors=[]
        official_table=load_official_domains(self.official_files)
        max_queries=max(1,min(int(max_queries),2))
        for step in range(max_queries):
            query=q if step==0 else q+(' 公式 ドキュメント' if profile=='development' else ' 公式')
            queries.append(query)
            try: data=self.fetch(query,profile,domains)
            except ResearchError as e:
                if not sources: raise
                errors.append({'stage':e.stage,'code':e.code}); break
            failures.extend(data.get('unresponsive_engines') or [])
            for row in data['results']:
                url=row.get('url','')
                try: parsed=urllib.parse.urlsplit(url)
                except ValueError: continue
                if parsed.scheme not in ('http','https') or not parsed.hostname or url in seen: continue
                if domains and not matches(domain(url),domains): continue
                seen.add(url)
                raw=row.get('raw_content') or ''; meta=row.get('evidence') or {}
                # Relevance is judged on the fetched body only. A title or search snippet can
                # mention the words while the page itself is a consent wall or an empty shell.
                overlap=len(terms(q) & terms(raw))
                quality=meta.get('status','not_fetched')
                # Never make an old/unmodified backend look verified.
                if not meta and raw: quality='unverified'
                final_url=meta.get('final_url') if isinstance(meta.get('final_url'),str) else ''
                offsite=redirected_offsite(url,final_url)
                if offsite: quality='redirected_offsite'
                # A page that redirected to another host is not the source that was asked for.
                preferred=False if offsite else matches(domain(url),domains or PROFILES[profile]['domains'])
                # 公式 comes only from the official-domain table (never from a domain the user typed in, never for an
                # off-site redirect). A user-named or profile domain is shown as 指定のドメイン by the screen.
                official=False if offsite else matches(domain(url),official_table)
                published=row.get('publishedDate')
                date_state='unknown'
                if isinstance(published,str):
                    try:
                        parsed_date=dt.datetime.fromisoformat(published.replace('Z','+00:00'))
                        if not parsed_date.tzinfo: parsed_date=parsed_date.replace(tzinfo=dt.timezone.utc)
                        date_state='older_than_one_year' if (dt.datetime.now(dt.timezone.utc)-parsed_date).days>365 else 'within_one_year'
                    except ValueError: pass
                shown=excerpt(raw,q)
                sources.append({'url':url,'final_url':final_url,'cite_url':final_url if offsite else url,'title':str(row.get('title',''))[:500], 'snippet':str(row.get('content',''))[:1500],
                    'excerpt':shown,'excerpt_matched':bool(shown),'match_terms':sorted(terms(q))[:80],'raw_content':raw,'evidence':meta,'quality':quality,'term_matches':overlap,
                    'source_basis':'公式ドメイン表に載っているサイト' if official else ('指定・プロファイルの優先ドメイン' if preferred else '一次情報か未確認'),
                    'official':official,
                    'preferred_domain':preferred,'published_at':published,'freshness':date_state,'engines':row.get('engines',[]), 'scope':'public'})
            usable=[s for s in sources if s['quality']=='ok' and s['term_matches']>0]
            if len(usable)>=3: break
        # Keep search relevance order within each preferred-domain group. A failed
        # fetch must remain visible, but must not bury a relevant official source.
        sources.sort(key=lambda s:s['official'] or s['preferred_domain'],reverse=True)
        sources=sources[:10]
        for i,s in enumerate(sources,1): s['citation']=f'W{i}'
        usable=sum(s['quality']=='ok' and s['term_matches']>0 for s in sources)
        conflicts=conflict_candidates(sources)
        report={'query':q,'scope':'public','profile':profile,'status':'ok' if usable>=3 else 'partial',
            'created_at':now(),'queries':queries,'sources':sources,'usable_sources':usable,
            'insufficient':usable<3,'next_action':'根拠箇所を確認してください。' if usable>=3 else '必要な根拠が不足しています。検索語や対象ドメインを絞って再検索してください。',
            'conflict_candidates':conflicts,'errors':errors,'engine_failures':list({tuple(map(str,f)) for f in failures if isinstance(f,(list,tuple))}),
            'elapsed_seconds':round(time.monotonic()-start,4),'external_query_sent':True,
            'verification':'関連性・件数の機械判定。内容の真偽・質問への完全な回答は未検証。'}
        report['markdown']=render_markdown(report)
        return report

    def private(self, q, store, owner):
        q=validate_query(q); start=time.monotonic(); wanted=terms(q); sources=[]
        # The store ranks (FTS5 BM25, title-weighted, one part per bundle first); the order is kept, score = matched units.
        for doc in store.search_documents(owner,q,limit=10):
            score=len(wanted & terms(doc['title']+' '+doc['body']))
            if score==0: continue
            shown=excerpt(doc['body'],q)
            sources.append({'id':doc['id'],'title':doc['title'],'url':'','scope':'private','score':score,'excerpt':shown,'excerpt_matched':bool(shown),'match_terms':sorted(wanted)[:80],'quality':'local_document','created_at':doc['created'],'source_basis':'本人が登録した資料。内容の正確性は未確認。'})
        for i,s in enumerate(sources,1):s['citation']=f'L{i}'
        report={'query':q,'scope':'private','status':'ok' if sources else 'empty','sources':sources,'created_at':now(),'external_query_sent':False,'elapsed_seconds':round(time.monotonic()-start,4),'queries':[], 'verification':'登録資料の語句一致検索。外部照合は行っていません。','next_action':'該当箇所を確認してください。' if sources else '一致する資料がありません。資料を登録するか検索語を変えてください。'}
        report['markdown']=render_markdown(report);return report

def conflict_candidates(sources):
    # Conservative, inspectable candidates; never present differences as proven contradictions.
    amounts=defaultdict(list)
    for s in sources:
        for match in re.finditer(r'(?:月額|年額|料金|価格)[^。\n]{0,25}?([0-9][0-9,]*(?:\.[0-9]+)?)\s*円',s.get('raw_content','')):
            label=match.group(0)[:2]; amounts[label].append((s['citation'],match.group(1),match.group(0)))
    output=[]
    for label,rows in amounts.items():
        if len({r[0] for r in rows})>1 and len({r[1] for r in rows})>1:
            output.append({'kind':'amount_difference','label':label,'observations':[{'citation':a,'amount':b,'text':c} for a,b,c in rows[:8]],'message':'異なる金額の記載があります。商品・プラン・時点が同一か未確認です。'})
    return output

def render_markdown(report):
    lines=['# 調査の根拠', '',report.get('verification',''), '',f"対象: {report['scope']} / 状態: {report['status']}",'']
    for source in report['sources']:
        if source.get('excerpt'): shown=source['excerpt']
        elif source.get('quality') in ('ok','partial','unverified','local_document'): shown='本文に検索語と一致する箇所がありませんでした(根拠箇所なし)。'
        else: shown='本文を取得できていません。'
        lines += [f"## [{source['citation']}] {source['title']}",source.get('cite_url') or source.get('url',''),shown,f"取得品質: {source.get('quality','unknown')}",'']
    lines += [report.get('next_action','')]
    return '\n'.join(lines)
