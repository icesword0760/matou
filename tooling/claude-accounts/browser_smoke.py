"""Manual installed-browser smoke test: synthetic localhost cookies only."""
import json
import queue
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit
from isolated_browser import find_browser, start_browser, stop_process


def main():
    events = queue.Queue()
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_): pass
        def do_GET(self):
            parsed=urlsplit(self.path)
            run=parse_qs(parsed.query).get('run',[''])[0]
            if parsed.path=='/seed':
                events.put((run,'seed',self.headers.get('Cookie','')))
                self.send_response(302)
                self.send_header('Set-Cookie','matou_fixture='+run+'; SameSite=Lax; Path=/')
                self.send_header('Location','/verify?run='+run)
                self.end_headers()
            else:
                if parsed.path=='/verify': events.put((run,'verify',self.headers.get('Cookie','')))
                self.send_response(200);self.end_headers()
                self.wfile.write(b'<title>Matou isolated browser test</title>Temporary test window. No accounts used.')
    server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    children=[]
    try:
        with tempfile.TemporaryDirectory(prefix='matou-browser-isolation-') as tmp:
            executable=find_browser()
            observed=[]
            for run in ('a','b'):
                profile=Path(tmp)/run;profile.mkdir(mode=0o700)
                children.append(start_browser(executable,profile,f'http://127.0.0.1:{server.server_port}/seed?run={run}'))
                seed=events.get(timeout=20);verify=events.get(timeout=20)
                assert seed==(run,'seed',''),seed
                assert verify==(run,'verify','matou_fixture='+run),verify
                observed.append({'run':run,'freshCookieJar':True,'ownCookieRetained':True})
            for child in children: stop_process(child)
            print(json.dumps({'browser':'installed Chromium browser','isolatedConcurrentWindows':observed,'ownedProcessesClosed':all(c.poll() is not None for c in children)}))
    finally:
        for child in children: stop_process(child)
        server.shutdown()

if __name__=='__main__':main()
