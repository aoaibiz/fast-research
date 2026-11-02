"""Click entry with instance and socket ownership checks; no user terminal work."""
import json,os,subprocess,sys,time,urllib.request,webbrowser
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parent.parent))
from workbench.ownership import listening_pid
ROOT=Path(__file__).resolve().parent.parent
PORT=18890
URL=f'http://localhost:{PORT}'
DATA=ROOT/'local-data'

def running():
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        state=json.loads((DATA/'instance.json').read_text())
        if listening_pid(PORT)!=state['pid']:return False
        with opener.open(URL+'/healthz',timeout=1) as response:
            result=json.load(response)
            return result.get('product')=='rustxng-local-workbench' and result.get('instance')==state['instance']
    except (OSError,ValueError,KeyError):return False

def error_page():
    error=DATA/'startup-error.html'
    error.write_text('<!doctype html><meta charset="utf-8"><title>RustXNG 起動の確認</title><h1>RustXNGを起動できませんでした</h1><p>ポート競合または起動処理の確認が必要です。エージェントに「RustXNGの起動を復旧して」と依頼してください。既存の資料は保持されています。</p>')
    webbrowser.open(error.as_uri())

def main():
    DATA.mkdir(exist_ok=True,mode=0o700)
    if running():webbrowser.open(URL);return
    if listening_pid(PORT) is not None:error_page();return
    with (DATA/'launcher.log').open('ab') as log:
        os.chmod(DATA/'launcher.log',0o600)
        child=subprocess.Popen([sys.executable,'-m','workbench.server','--data',str(DATA),'--port',str(PORT)],cwd=ROOT,stdout=log,stderr=log)
    try:
        deadline=time.monotonic()+20
        while time.monotonic()<deadline:
            if child.poll() is not None:break
            if running() and listening_pid(PORT)==child.pid:
                webbrowser.open(URL);child.wait();return
            time.sleep(.25)
        error_page()
    finally:
        if child.poll() is None:child.terminate();child.wait(timeout=10)

if __name__=='__main__':main()
