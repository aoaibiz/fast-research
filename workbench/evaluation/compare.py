"""24 fixed public queries. URL-domain relevance is a proxy, not factual-answer accuracy."""
import argparse,datetime,hashlib,json,statistics,sys,time,urllib.parse,urllib.request
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[2]))
from workbench.core import Research,domain,matches

def measure(rows,expected):
    positions=[i+1 for i,row in enumerate(rows) if matches(domain(row.get('url','')),expected)]
    return {'results':len(rows),'expected_host_hit5':bool(positions and positions[0]<=5),'reciprocal_rank':1/positions[0] if positions else 0,'raw_pages':sum(bool(r.get('raw_content')) for r in rows),'raw_characters':sum(len(r.get('raw_content','')) for r in rows),'quality_ok':sum((r.get('evidence') or {}).get('status')=='ok' for r in rows)}

def main():
    p=argparse.ArgumentParser();p.add_argument('--baseline',default='http://127.0.0.1:18888');p.add_argument('--modified',default='http://127.0.0.1:18889');p.add_argument('--out',required=True);args=p.parse_args()
    questions=Path(__file__).with_name('questions.json');raw=questions.read_bytes();dataset=json.loads(raw);out=Path(args.out);out.mkdir(parents=True,exist_ok=True)
    metadata={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'dataset_sha256':hashlib.sha256(raw).hexdigest(),'queries':len(dataset),'max_runs':1,'max_seconds':1800,'paid_api_budget':0,'criteria':'expected_host_hit5 non-regression; extraction metadata availability; latency reported even when worse','limitations':['expected-host hit is a relevance proxy, not answer correctness','live upstream variation and rate limits','one paired pass; no generalization from a single run','baseline default engines vs modified curated engines and <=2 bounded query attempts']}
    (out/'protocol.json').write_text(json.dumps(metadata,ensure_ascii=False,indent=2));records=[];started=time.monotonic();research=Research(args.modified)
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    for index,item in enumerate(dataset):
        if time.monotonic()-started>1800:break
        for variant in (['baseline','modified'] if index%2==0 else ['modified','baseline']):
            t=time.monotonic();record={'id':item['id'],'variant':variant}
            try:
                if variant=='baseline':
                    params=urllib.parse.urlencode({'q':item['query'],'format':'json','full':'1','max_results':10,'include_raw_content':1,'language':'ja','safesearch':1})
                    with opener.open(args.baseline+'/search?'+params,timeout=25) as r:response=json.loads(r.read(2*1024*1024))
                    rows=response['results'];queries=1
                else:
                    response=research.public(item['query'],item['profile']);rows=response['sources'];queries=len(response['queries'])
                record.update(measure(rows,item['expected_hosts']));record['query_attempts']=queries;record['status']='ok';record['top5_urls']=[r['url'] for r in rows[:5]]
            except Exception as error:record.update(status='failed',error=type(error).__name__,expected_host_hit5=False)
            record['seconds']=round(time.monotonic()-t,4);records.append(record)
            with (out/'measurements.jsonl').open('a') as f:f.write(json.dumps(record,ensure_ascii=False)+'\n')
            print(json.dumps({k:record[k] for k in ('id','variant','status','expected_host_hit5','seconds')}),flush=True)
    summary={}
    for variant in ('baseline','modified'):
        subset=[r for r in records if r['variant']==variant];times=sorted(r['seconds'] for r in subset)
        summary[variant]={'measured':len(subset),'successful_requests':sum(r['status']=='ok' for r in subset),'expected_host_hit5':sum(r['expected_host_hit5'] for r in subset),'p50_seconds':statistics.median(times) if times else None,'p95_seconds':times[max(0,int(len(times)*.95)-1)] if times else None,'raw_characters':sum(r.get('raw_characters',0) for r in subset),'quality_ok_pages':sum(r.get('quality_ok',0) for r in subset)}
    summary['complete']=len(records)==2*len(dataset);summary['adopt_quality_profile']=summary['complete'] and summary['modified']['expected_host_hit5']>=summary['baseline']['expected_host_hit5'];summary['limitations']=metadata['limitations']
    (out/'summary.json').write_text(json.dumps(summary,ensure_ascii=False,indent=2));print(json.dumps(summary,ensure_ascii=False),flush=True)
if __name__=='__main__':main()
