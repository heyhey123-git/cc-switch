"""Configure an initialized server-local CCS for the same user's Codex SSH worker.

Reads secrets from a private JSON file or stdin; never sends model requests.
"""
from pathlib import Path
import argparse
import copy
import datetime
import json
import os
import re
import secrets
import shutil
import sqlite3
import sys
import tomllib
from urllib.parse import urlsplit

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--input', required=True, help='private JSON file, or - for stdin')
parser.add_argument('--home', type=Path, default=Path.home())
parser.add_argument('--port', type=int, default=15721)
parser.add_argument('--dry-run', action='store_true')
args = parser.parse_args()
os.umask(0o077)
data = json.load(sys.stdin) if args.input == '-' else json.loads(Path(args.input).read_text('utf-8'))
home = args.home.resolve()
cc = home/'.cc-switch'
codex = home/'.codex'
config_path = codex/'config.toml'
original = config_path.read_text('utf-8')
config = tomllib.loads(original)
active = config['model_provider']
if active == 'cc_switch_remote':
    raise SystemExit('Server CCS is already configured; keep the existing credentials and edit its provider settings.')
upstream = config['model_providers'][active]
assert upstream.get('wire_api','responses') == 'responses', 'Existing native provider must use Responses'
auth = json.loads((codex/'auth.json').read_text('utf-8'))
native_key = auth.get('OPENAI_API_KEY') or upstream.get('experimental_bearer_token')
if not native_key and upstream.get('env_key'):
    native_key = os.environ.get(upstream['env_key'])
assert native_key, 'Existing provider API key could not be resolved; no files changed'
assert 1 <= args.port <= 65535
anthropic = data['anthropic']
base = anthropic['base_url'].rstrip('/')
parsed = urlsplit(base)
assert parsed.scheme == 'https' and parsed.hostname and not parsed.username and not parsed.password
assert anthropic['api_key'], 'Anthropic API key is required'
namespace = data.get('namespace','claude')
assert re.fullmatch(r'[a-z0-9][a-z0-9-]*',namespace), 'Invalid model namespace'
catalog_path = Path(config['model_catalog_json']).expanduser()
if not catalog_path.is_absolute():
    catalog_path = codex/catalog_path
catalog = json.loads(catalog_path.read_text('utf-8'))
existing_slugs = {x['slug'] for x in catalog['models']}
models = data.get('claude_models')
if models is None:
    template = next(x for x in catalog['models'] if x['slug']==config['model'])
    models = []
    for name in ['claude-fable-5-2','claude-opus-5-5','claude-sonnet-5-5','claude-haiku-5-5']:
        row = copy.deepcopy(template)
        row.update(slug=f'ccs-{namespace}/{name}',display_name=name,description='Claude through the server-local CC Switch proxy',
                   context_window=1000000,max_context_window=1000000,auto_compact_token_limit=800000,
                   effective_context_window_percent=95,input_modalities=['text','image'],default_reasoning_level='medium',
                   supported_reasoning_levels=[{'effort':effort,'description':effort} for effort in ['low','medium','high','xhigh','max']])
        models.append(row)
assert len(models)==4 and len({x['slug'] for x in models})==4
assert all(x['slug'].startswith('ccs-'+namespace+'/') for x in models)
assert not any(x['slug'] in existing_slugs for x in models), 'Claude aliases already exist; no files changed'
catalog_after = copy.deepcopy(catalog)
catalog_after['models'].extend(models)
state_path = cc/'live-state.json'
state = json.loads(state_path.read_text('utf-8')) if state_path.exists() else {'version':1,'apps':{}}
current = state.get('apps',{}).get('codex',{})
assert not current.get('attached'), 'An attached CCS Codex route already exists; no files changed'
token = secrets.token_urlsafe(32)
state.setdefault('apps',{})['codex'] = {
    'mode':'proxy','attached':True,'proxy_route':'server-native','contract':{'version':1,'key':token},
    'stack':{'enabled':True,'members':['server-native','server-claude'],
             'keys':{namespace:'server-claude','native':'server-native'}}}
new_config,count = re.subn(r'(?m)^model_provider\s*=\s*[^\r\n]+', 'model_provider = "cc_switch_remote"', original, count=1)
assert count == 1 and 'cc_switch_remote' not in config.get('model_providers',{})
new_config += f'''
[model_providers.cc_switch_remote]
name = "Server-local CC Switch"
base_url = "http://127.0.0.1:{args.port}/v1"
wire_api = "responses"
requires_openai_auth = false
experimental_bearer_token = {json.dumps(token)}
supports_websockets = false
request_max_retries = 3
stream_max_retries = 0
stream_idle_timeout_ms = 300000
'''
expected = copy.deepcopy(config)
expected['model_provider'] = 'cc_switch_remote'
expected.setdefault('model_providers',{})['cc_switch_remote'] = tomllib.loads(new_config)['model_providers']['cc_switch_remote']
assert tomllib.loads(new_config)==expected, 'Unrelated Codex configuration would change'
native_settings = {'auth':{'OPENAI_API_KEY':native_key},'config':original}
claude_config = f'model_provider="custom"\n[model_providers.custom]\nname="Claude relay"\nbase_url={json.dumps(base)}\nwire_api="responses"\n'
claude_settings = {'auth':{'OPENAI_API_KEY':anthropic['api_key']},'config':claude_config}
pin_path = cc/'tls-pins.json'
files = {config_path:new_config.encode(), catalog_path:json.dumps(catalog_after,ensure_ascii=False,indent=2).encode(),
         state_path:json.dumps(state,indent=2).encode()}
if data.get('tls_pins') is not None:
    assert not pin_path.exists(), 'A certificate pin policy already exists; merge it manually'
    files[pin_path] = json.dumps(data['tls_pins'],indent=2).encode()
db_path = cc/'cc-switch.db'
assert db_path.exists(), 'Run cc-switch --headless --init-only first'
conn = sqlite3.connect(db_path,timeout=15)
assert conn.execute("select count(*) from providers where app_type='codex' and id in ('server-native','server-claude')").fetchone()[0]==0
report = {'dry_run':args.dry_run,'original_provider':active,'default_model':config['model'],
          'listen':'127.0.0.1:'+str(args.port),'claude_aliases':[x['slug'] for x in models],
          'request_max_retries':3,'stream_max_retries':0,'auth_json_unchanged':True,
          'other_codex_settings_preserved':True,'paid_model_requests':0}
if args.dry_run:
    conn.close();print(json.dumps(report));sys.exit(0)
backup = cc/'backups'/('before-linux-proxy-'+datetime.datetime.now().strftime('%Y%m%d-%H%M%S'))
backup.mkdir(parents=True,mode=0o700)
cc.chmod(0o700)
db_path.chmod(0o600)
old_files = {p:p.read_bytes() if p.exists() else None for p in files}
for index,(path,raw) in enumerate(old_files.items()):
    if raw is not None:
        (backup/(str(index)+'-'+path.name)).write_bytes(raw)
db_backup = sqlite3.connect(backup/'cc-switch.db')
conn.backup(db_backup);db_backup.close()
try:
    conn.execute('begin immediate')
    for identifier,name,settings,meta in [
        ('server-native','Existing native Responses',native_settings,{'apiFormat':'openai_responses','commonConfigEnabled':False}),
        ('server-claude','Claude relay',claude_settings,{'apiFormat':'anthropic','commonConfigEnabled':False,'impersonateClaudeCode':False,'maxOutputTokens':16384}),
    ]:
        conn.execute('insert into providers(id,app_type,name,settings_config,meta,is_current) values(?,?,?,?,?,?)',
                     (identifier,'codex',name,json.dumps(settings),json.dumps(meta),int(identifier=='server-native')))
    conn.execute('update proxy_config set listen_address=?,listen_port=?',('127.0.0.1',args.port))
    conn.execute("update proxy_config set enabled=1,proxy_enabled=1,auto_failover_enabled=0 where app_type='codex'")
    conn.execute("insert or replace into settings(key,value) values('optimizer_config',?)",
                 (json.dumps({'enabled':True,'thinkingOptimizer':True,'cacheInjection':True}),))
    for path,raw in files.items():
        tmp = path.with_name(path.name+'.ccs-setup.tmp')
        tmp.write_bytes(raw);tmp.chmod(0o600);os.replace(tmp,path)
    conn.commit()
except BaseException:
    conn.rollback()
    for path,raw in old_files.items():
        if raw is None:
            path.unlink(missing_ok=True)
        else:
            path.write_bytes(raw)
    raise
finally:
    conn.close()
report.update(passed=True,backup=str(backup))
print(json.dumps(report))
