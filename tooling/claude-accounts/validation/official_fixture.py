"""Synthetic-account hot-switch tests of the unmodified official CLI.
No real credentials, no upstream forwarding, no cswap process.
"""
import os, sys, json, time, pathlib, hashlib, ssl, socketserver, http.server
import threading, subprocess, pty, select, signal, re, fcntl, termios, struct

ROOT = pathlib.Path(__file__).resolve().parent
CLI = os.environ.get('MATOU_TEST_CLAUDE', 'claude')
START = time.monotonic()
EVENTS = []
STATE = {'mode': 'success', 'phase': 'initial', 'tool': False, 'tool_issued': False}
IDS = {'A': '11111111-1111-4111-8111-111111111111', 'B': '33333333-3333-4333-8333-333333333333'}
ORGS = {'A': '22222222-2222-4222-8222-222222222222', 'B': '44444444-4444-4444-8444-444444444444'}

def event(kind, **values):
    row = {'t': round(time.monotonic()-START, 3), 'kind': kind, 'phase': STATE['phase'], **values}
    EVENTS.append(row)
    # Synthetic event values are retained only in the fixture result.
    return row

def credentials(account, expired=False):
    return {'claudeAiOauth': {'accessToken': f'fixture-{account}-'+('expired' if expired else 'valid'),
        'refreshToken': f'fixture-{account}-refresh', 'expiresAt': int((time.time()+(-3600 if expired else 7200))*1000),
        'scopes': ['user:inference','user:profile'], 'subscriptionType':'max', 'rateLimitTier':'default_claude_max_5x'}}

def identity(account):
    return {'accountUuid': IDS[account], 'emailAddress': f'fixture-{account.lower()}@example.invalid', 'organizationUuid': ORGS[account]}

def write_json(path, value):
    tmp=path.with_name(path.name+'.tmp');tmp.write_text(json.dumps(value));tmp.chmod(0o600);tmp.replace(path)

def token_label(value):
    v=value.removeprefix('Bearer ')
    return v if v.startswith('fixture-') else ('absent' if not v else 'UNEXPECTED_NON_FIXTURE')

class Inner(http.server.BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def reply(self,status,value):
        data=json.dumps(value).encode();self.send_response(status)
        self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(data)))
        self.end_headers();self.wfile.write(data)
    def do_GET(self):self.do_POST()
    def do_POST(self):
        raw=self.rfile.read(int(self.headers.get('Content-Length',0)))
        try:body=json.loads(raw or b'{}')
        except ValueError:body={}
        label=token_label(self.headers.get('authorization',''))
        messages=json.dumps(body.get('messages',[]))
        has_tool_result=any(isinstance(m.get('content'),list) and any(isinstance(c,dict) and c.get('type')=='tool_result' and 'TOOL_COMPLETED_MARKER' in json.dumps(c.get('content')) for c in m['content']) for m in body.get('messages',[]))
        record=event('request', method=self.command,path=self.path,credential=label,
            refresh=token_label(body.get('refresh_token','')),user_agent=self.headers.get('user-agent',''),
            history_a='HISTORY_A_MARKER' in messages,turn_b='TURN_B_MARKER' in messages,turn_c='AFTER_RENEWAL_MARKER' in messages,
            tool_result=has_tool_result,metadata=body.get('metadata',{}))
        if label=='UNEXPECTED_NON_FIXTURE' or record['refresh']=='UNEXPECTED_NON_FIXTURE':
            self.reply(403,{'error':'unexpected credential'});return
        if '/oauth/token' in self.path:
            account='B' if body.get('refresh_token')=='fixture-B-refresh' else 'A'
            if STATE['mode']=='invalid_grant' or body.get('refresh_token')!=f'fixture-{account}-refresh':
                self.reply(400,{'error':'invalid_grant','error_description':'Synthetic refresh revoked'});return
            if STATE.get('refresh_delay'): time.sleep(STATE['refresh_delay'])
            self.reply(200,{'access_token':f'fixture-{account}-renewed','refresh_token':f'fixture-{account}-refresh-new',
                'expires_in':3600,'token_type':'Bearer','scope':'user:inference user:profile',
                'account':{'uuid':IDS[account],'email_address':f'fixture-{account.lower()}@example.invalid'},
                'organization':{'uuid':ORGS[account]}});return
        if '/oauth/profile' in self.path:
            a='B' if label.startswith('fixture-B') else 'A'
            self.reply(200,{'account':{'uuid':IDS[a],'email':f'fixture-{a.lower()}@example.invalid'},
                'organization':{'uuid':ORGS[a],'organization_type':'claude_max','rate_limit_tier':'default_claude_max_5x'}});return
        if '/count_tokens' in self.path:
            self.reply(200,{'input_tokens':30});return
        if '/v1/messages' not in self.path:
            self.reply(404,{'error':'unhandled synthetic route'});return
        if label not in ('fixture-A-valid','fixture-B-valid','fixture-B-renewed','fixture-A-renewed'):
            self.reply(401,{'type':'error','error':{'type':'authentication_error','message':'Synthetic expired credential rejected'}});return
        self.send_response(200);self.send_header('Content-Type','text/event-stream')
        for name,value in {'status':'allowed','5h-utilization':'.24' if 'A-' in label else '.61',
            '7d-utilization':'.08' if 'A-' in label else '.32','5h-reset':str(int(time.time()+3600)),
            '7d-reset':str(int(time.time()+86400))}.items():
            self.send_header('anthropic-ratelimit-unified-'+name,value)
        self.end_headers()
        use_tool=STATE['tool'] and not STATE['tool_issued'] and 'BEGIN_TOOL_MARKER' in messages and 'HISTORY_A_MARKER' in messages
        if use_tool:STATE['tool_issued']=True
        msg={'id':'msg_fixture_'+str(len(EVENTS)),'type':'message','role':'assistant','model':body.get('model'),
            'content':[],'stop_reason':None,'stop_sequence':None,'usage':{'input_tokens':30,'output_tokens':5}}
        events=[('message_start',{'type':'message_start','message':msg})]
        if use_tool:
            delay=38 if 'keychain' in STATE['run_dir'] else 8
            command=f"/bin/echo started > {STATE['run_dir']}/tool-started; /bin/sleep {delay}; /bin/echo TOOL_COMPLETED_MARKER > {STATE['run_dir']}/tool-completed; /bin/cat {STATE['run_dir']}/tool-completed"
            events += [('content_block_start',{'type':'content_block_start','index':0,'content_block':{'type':'tool_use','id':'toolu_fixture','name':'Bash','input':{}}}),
                ('content_block_delta',{'type':'content_block_delta','index':0,'delta':{'type':'input_json_delta','partial_json':json.dumps({'command':command,'description':'Synthetic local completion marker','timeout':60000})}})]
        else:
            answer='RESPONSE_'+label+('_AFTER_TOOL' if 'TOOL_COMPLETED_MARKER' in messages else '')
            events += [('content_block_start',{'type':'content_block_start','index':0,'content_block':{'type':'text','text':''}}),
                ('content_block_delta',{'type':'content_block_delta','index':0,'delta':{'type':'text_delta','text':answer}})]
        events += [('content_block_stop',{'type':'content_block_stop','index':0}),
            ('message_delta',{'type':'message_delta','delta':{'stop_reason':'tool_use' if use_tool else 'end_turn','stop_sequence':None},'usage':{'output_tokens':5}}),
            ('message_stop',{'type':'message_stop'})]
        for name,value in events:self.wfile.write(('event: '+name+'\ndata: '+json.dumps(value)+'\n\n').encode())
        self.wfile.flush()

CTX=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
class Proxy(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(20);buf=b''
        while b'\r\n\r\n' not in buf:
            part=self.request.recv(1)
            if not part:return
            buf+=part
            if len(buf)>16384:return
        line=buf.split(b'\r\n',1)[0].decode(errors='replace');event('proxy',line=line)
        if not line.startswith('CONNECT '):return
        self.request.sendall(b'HTTP/1.1 200 Connection Established\r\n\r\n')
        try:
            with CTX.wrap_socket(self.request,server_side=True) as sock:Inner(sock,self.client_address,self.server)
        except Exception as e:event('proxy_error',error=str(e))
class Server(socketserver.ThreadingTCPServer):allow_reuse_address=True;daemon_threads=True

def snapshots(path):
    if not path.exists():return []
    text=path.read_text();out=[];decoder=json.JSONDecoder()
    while text.strip():
        text=text.lstrip()
        try:d,n=decoder.raw_decode(text)
        except ValueError:break
        out.append(d);text=text[n:]
    return out

