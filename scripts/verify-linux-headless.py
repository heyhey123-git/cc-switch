"""No-display Linux smoke test. Only loopback mock providers and dummy keys."""
from pathlib import Path
import copy
import http.client
import http.server
import json
import os
import queue
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

exe = Path(sys.argv[1]).resolve()
home = Path(tempfile.mkdtemp(prefix='cc-switch-headless-'))
env = dict(os.environ)
for name in ('DISPLAY', 'WAYLAND_DISPLAY', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY',
             'http_proxy', 'https_proxy', 'all_proxy', 'OPENAI_API_KEY', 'ANTHROPIC_API_KEY'):
    env.pop(name, None)
env.update(HOME=str(home), CC_SWITCH_TEST_HOME=str(home), NO_PROXY='127.0.0.1,localhost,::1')
subprocess.run([str(exe), '--headless', '--init-only'], env=env, check=True, timeout=30)

captures = []
class Mock(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *_):
        pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert self.headers.get('x-api-key') == 'LOCAL-DUMMY' or self.headers.get('Authorization') == 'Bearer LOCAL-DUMMY'
        captures.append(body)
        if self.path == '/v1/messages':
            response = {'id':'msg_headless', 'type':'message', 'role':'assistant', 'model':body['model'],
                        'content':[{'type':'text','text':'LOCAL_HEADLESS_OK'}], 'stop_reason':'end_turn',
                        'usage':{'input_tokens':2,'output_tokens':3,'cache_read_input_tokens':1200,'cache_creation_input_tokens':20}}
        else:
            assert self.path == '/v1/responses'
            response = {'id':'resp_native','object':'response','status':'completed','model':body['model'],
                        'output':[{'id':'msg_native','type':'message','role':'assistant','status':'completed',
                                   'content':[{'type':'output_text','text':'LOCAL_NATIVE_OK','annotations':[]}]}],
                        'usage':{'input_tokens':2,'output_tokens':3,'total_tokens':5}}
        raw = json.dumps(response).encode()
        self.send_response(200)
        self.send_header('Content-Type','application/json')
        self.send_header('Content-Length',str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

mock = http.server.ThreadingHTTPServer(('127.0.0.1',0), Mock)
threading.Thread(target=mock.serve_forever,daemon=True).start()
cc = home/'.cc-switch'
conn = sqlite3.connect(cc/'cc-switch.db')
for identifier, api in [('fixture-claude','anthropic'), ('fixture-native','openai_responses')]:
    config = f'model_provider="fixture"\n[model_providers.fixture]\nbase_url="http://127.0.0.1:{mock.server_port}/v1"\nwire_api="responses"\n'
    settings = {'auth':{'OPENAI_API_KEY':'LOCAL-DUMMY'}, 'config':config}
    conn.execute('insert into providers(id,app_type,name,settings_config,meta,is_current) values(?,?,?,?,?,0)',
                 (identifier,'codex','Local headless fixture',json.dumps(settings),json.dumps({'apiFormat':api,'impersonateClaudeCode':False})))
conn.execute("insert or replace into settings(key,value) values('optimizer_config',?)", (json.dumps({'enabled':True,'thinkingOptimizer':True,'cacheInjection':True}),))
conn.execute("update proxy_config set listen_address='127.0.0.1',listen_port=0")
conn.commit();conn.close()
token = 'LOCAL-HEADLESS-CONTRACT'
state = {'version':1,'apps':{'codex':{'mode':'proxy','attached':True,'proxy_route':'fixture-claude',
         'contract':{'version':1,'key':token},'stack':{'enabled':True,'members':['fixture-claude','fixture-native'],
         'keys':{'fixture':'fixture-claude','native':'fixture-native'}}}}}
(cc/'live-state.json').write_text(json.dumps(state), 'utf-8')
stderr = (home/'service.log').open('w')
process = subprocess.Popen([str(exe),'--headless'],env=env,stdout=subprocess.PIPE,stderr=stderr,text=True,bufsize=1)
lines = queue.Queue()
def read_stdout():
    for line in process.stdout:
        lines.put(line.strip())
threading.Thread(target=read_stdout,daemon=True).start()
checks = []
def check(name, condition):
    checks.append({'name':name,'passed':bool(condition)})
    assert condition, name
def request(body):
    client = http.client.HTTPConnection('127.0.0.1',port,timeout=15)
    try:
        client.request('POST','/v1/responses',json.dumps(body),{'Content-Type':'application/json','Authorization':'Bearer '+token,'session_id':'headless-local-cache-fixture'})
        response = client.getresponse()
        raw = response.read()
        check('http_success',response.status==200)
        return json.loads(raw)
    finally:
        client.close()
try:
    deadline = time.monotonic()+30
    while time.monotonic()<deadline:
        if process.poll() is not None:
            raise RuntimeError('No-display service exited: '+(home/'service.log').read_text())
        try:
            line = lines.get(timeout=.2)
        except queue.Empty:
            continue
        if line.startswith('CC_SWITCH_HEADLESS_READY '):
            port = int(line.rsplit(':',1)[1])
            break
    else:
        raise TimeoutError('No-display service did not become ready')
    check('no_display_startup',True)
    resize = '<image_resize_notice>\nImage 1 of 1 in the preceding tool output was resized from 2368x3776 to 1247x1990 pixels.\n</image_resize_notice>'
    history = [{'role':'developer','content':'Stable developer instructions.'},
               {'role':'user','content':'Synthetic long repository context. '*16000},
               {'type':'function_call','call_id':'view_1','name':'view_image','arguments':'{}'},
               {'type':'function_call_output','call_id':'view_1','output':[{'type':'input_image','image_url':'data:image/png;base64,aGVsbG8='}]}]
    base = {'model':'ccs-fixture/claude-opus-5-5','instructions':'Stable system.','input':history,'stream':False,
            'tools':[{'type':'function','name':'view_image','parameters':{'type':'object'}}]}
    request(base)
    before = copy.deepcopy(captures[-1])
    changed = copy.deepcopy(base)
    changed['input'].append({'role':'developer','content':[{'type':'input_text','text':resize}]})
    result = request(changed)
    after = captures[-1]
    check('system_stable_after_image_notice',before.get('system')==after.get('system'))
    check('notice_preserved_inside_tool_result',any(part.get('text')==resize for m in after['messages'] for block in m['content'] if block.get('type')=='tool_result' for part in block.get('content',[]) if isinstance(part,dict)))
    check('ordinary_developer_precedence','Stable developer instructions.' in json.dumps(after.get('system')))
    check('cache_usage_conversion',result['usage']['input_tokens_details']['cached_tokens']==1200)
    upload_notice = '<image_resize_notice>\nImage 1 of 1 in the preceding user message was resized from 2944x1096 to 2048x762 pixels.\n</image_resize_notice>'
    uploaded = copy.deepcopy(changed)
    uploaded['input'].extend([
        {'role':'assistant','content':'Previous image inspected.'},
        {'role':'user','content':[{'type':'input_text','text':'Inspect another upload.'},{'type':'input_image','image_url':'data:image/png;base64,aGVsbG8='}]},
        {'role':'developer','content':[{'type':'input_text','text':upload_notice}]}])
    request(uploaded)
    upload_capture = copy.deepcopy(captures[-1])
    check('uploaded_user_image_system_stable',upload_capture.get('system')==after.get('system'))
    check('uploaded_user_image_notice_preserved',any(block.get('text')==upload_notice for m in upload_capture['messages'] for block in m['content']))
    grown = copy.deepcopy(uploaded)
    grown['input'].append({'role':'user','content':[{'type':'input_text','text':'Appended block '+str(n)} for n in range(45)]})
    request(grown)
    grown_capture = captures[-1]
    check('previous_request_anchor_retained_beyond_20_blocks',any(block.get('text')==upload_notice and 'cache_control' in block for m in grown_capture['messages'] for block in m['content']))
    check('four_breakpoint_limit',sum('cache_control' in block for m in grown_capture['messages'] for block in m['content'])+sum('cache_control' in t for t in grown_capture.get('tools',[]))+sum('cache_control' in s for s in grown_capture.get('system',[]))==4)
    native = {'model':'ccs-native/gpt-local','input':[{'role':'user','content':'LOCAL_NATIVE'}], 'stream':False,'prompt_cache_key':'stable-key'}
    request(native)
    check('native_responses_unchanged',captures[-1]['input']==native['input'] and captures[-1]['prompt_cache_key']=='stable-key' and 'messages' not in captures[-1])
    process.send_signal(signal.SIGTERM)
    check('graceful_sigterm',process.wait(timeout=15)==0)
    report = {'passed':True,'paid_model_requests':0,'no_display':True,'checks':checks,'upstream_requests':len(captures)}
    print(json.dumps(report))
finally:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill(); process.wait()
    stderr.close();mock.shutdown();mock.server_close()
