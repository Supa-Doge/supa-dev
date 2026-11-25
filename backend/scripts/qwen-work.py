"""Native Qwen Code launcher. All task execution stays in the cloud."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import time
import urllib.error
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parent))
import runtime

ROOT = runtime.ROOT
GATEWAY = runtime.gateway_config()
GATEWAY_URL = f'http://127.0.0.1:{GATEWAY["port"]}'
read, save = runtime.read, runtime.save
FLAGS = runtime.FLAGS

def programs():
    node, cli = runtime.qwen_programs()
    return runtime.lm_program(), '', node, cli

def environment(mode='gateway'):
    return runtime.environment()

def configure():
    env = environment()
    profile = Path(env['QWEN_HOME'])
    path = profile / 'settings.json'
    if path.exists():
        config = read(path)
    else:
        config = read(Path.home() / '.qwen/settings.json')
        config.pop('env', None)
    providers = config.setdefault('modelProviders', {}).setdefault('openai', [])
    by_id = {item.get('id'): item for item in providers}
    for model in GATEWAY['cloudModels']:
        if model not in by_id:
            item = {'id': model, 'name': model, 'generationConfig': {'contextWindowSize': 1000000}}
            providers.append(item)
        else:
            item = by_id[model]
        item.update(baseUrl=GATEWAY_URL + '/v1', envKey='OPENAI_API_KEY')
    selected = {'model': GATEWAY['cloudModel'], 'baseUrl': GATEWAY_URL + '/v1'}
    # Apply the desktop selection only when it changes. Native /model choices persist.
    stamp = runtime.HOME / 'qwen-selection.json'
    if read(stamp) != selected or not config.get('model', {}).get('name'):
        config.setdefault('security', {}).setdefault('auth', {})['selectedType'] = 'openai'
        config.setdefault('model', {}).update(name=selected['model'], baseUrl=selected['baseUrl'])
    save(path, config)
    save(stamp, selected)
    save(ROOT / 'config/gateway.runtime.json', GATEWAY)
    for folder in ('projects', 'scripts', 'temp', 'output', 'cache/npm', 'cache/pip', 'cache/uv'):
        (runtime.workspace() / folder).mkdir(parents=True, exist_ok=True)

def gateway_request(path, token=None, data=None, timeout=10):
    headers = {'Content-Type': 'application/json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    request = urllib.request.Request(GATEWAY_URL + path, json.dumps(data).encode() if data is not None else None, headers)
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=timeout) as response:
        return json.load(response)

def ensure_gateway():
    configure()
    key_path = ROOT / 'config/gateway-control.key'
    if not key_path.exists():
        key_path.write_text(secrets.token_urlsafe(48), encoding='ascii')
        if not runtime.WIN:
            key_path.chmod(0o600)
    key = key_path.read_text('ascii').strip()
    expected = hashlib.sha256((ROOT / 'components/gateway/server.py').read_bytes()).hexdigest()
    try:
        health = gateway_request('/health')
    except (OSError, urllib.error.URLError):
        health = None
    if health:
        if health.get('service') != 'qwen-work-gateway' or health.get('runtimeRoot') != str(ROOT):
            raise RuntimeError(f'Порт {GATEWAY["port"]} занят другим шлюзом. Измените порт в Supa Dev.')
        if health.get('build') == expected:
            return key
        try:
            gateway_request('/admin/shutdown', key, {})
        except urllib.error.HTTPError as error:
            if error.code == 409:
                # Existing sessions keep their active stream; update at the next idle launch.
                return key
            raise
        for _ in range(40):
            try:
                gateway_request('/health', timeout=1)
            except (OSError, urllib.error.URLError):
                break
            time.sleep(0.1)
    log_path = ROOT / 'output/gateway-server.log'
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open('a', encoding='utf-8') as log:
        process = subprocess.Popen([sys.executable, '-u', str(ROOT / 'components/gateway/server.py'),
                                    '--config', str(ROOT / 'config/gateway.runtime.json')],
                                   env=environment(), cwd=ROOT, stdout=log, stderr=log,
                                   creationflags=FLAGS, start_new_session=not runtime.WIN)
    save(ROOT / 'output/gateway-process.json', {'pid': process.pid, 'script': str(ROOT / 'components/gateway/server.py')})
    for _ in range(80):
        try:
            health = gateway_request('/health', timeout=1)
            if health.get('build') == expected and health.get('runtimeRoot') == str(ROOT):
                return key
        except (OSError, urllib.error.URLError):
            pass
        if process.poll() is not None:
            break
        time.sleep(0.15)
    raise RuntimeError('Шлюз не запустился. Откройте журнал в Supa Dev.')

def ensure_server(paths=None, env=None):
    lms = runtime.lm_program()
    result = runtime.run([lms, 'daemon', 'up'], timeout=120)
    if result.returncode:
        raise RuntimeError('Не удалось запустить LM Studio runtime: ' + result.stderr[-500:])
    result = runtime.run([lms, 'server', 'start', '--port', '1234', '--bind', '127.0.0.1'], timeout=90)
    if result.returncode and 'already' not in (result.stdout + result.stderr).lower():
        raise RuntimeError('Не удалось запустить локальный API LM Studio: ' + result.stderr[-500:])

def ensure_model(paths=None, env=None, context=16384):
    ensure_server()
    model = read(ROOT / 'config/local-model.json')
    path = runtime.model_path()
    if not path.is_file() or path.stat().st_size != model['sizeBytes']:
        raise RuntimeError('Локальная модель ещё не скачана. Завершите установку окружения в Supa Dev.')
    node, _ = runtime.qwen_programs()
    result = runtime.run([node, ROOT / 'components/lmstudio-client/load-model.bundle.mjs', str(context)], timeout=300)
    if result.returncode:
        raise RuntimeError('Не удалось подготовить редактор промпта. См. журнал загрузки модели в Supa Dev.')

def qwen_command(paths, env, project, args):
    return [paths[2], '--expose-gc', paths[3], *args]

def project_path(value=None, mode='gateway'):
    result = Path(value).expanduser().resolve() if value else Path.cwd()
    if not result.is_dir():
        raise RuntimeError('Рабочая папка не существует.')
    return result

def main():
    parser = argparse.ArgumentParser(add_help=False, allow_abbrev=False)
    parser.add_argument('--project')
    parser.add_argument('--gateway-only', action='store_true')
    parser.add_argument('--setup-only', action='store_true')
    parser.add_argument('--status', action='store_true')
    options, args = parser.parse_known_args()
    if options.setup_only:
        ensure_model()
        return 0
    if options.status:
        print(json.dumps(gateway_request('/health'), indent=2))
        return 0
    runtime.qwen_key()
    key = ensure_gateway()
    if options.gateway_only:
        print(GATEWAY_URL)
        return 0
    project = project_path(options.project)
    session = gateway_request('/admin/sessions', key, {'project': str(project)})
    env = environment()
    env['OPENAI_API_KEY'] = session['token']
    if runtime.settings().get('yolo', True) and not any(arg == '--yolo' or arg.startswith('--approval-mode') for arg in args):
        args.insert(0, '--approval-mode=yolo')
    node, cli = runtime.qwen_programs()
    print(f'Supa Dev: облако {GATEWAY["cloudModel"]}; локальная модель редактирует только отклонённые промпты.', flush=True)
    process = subprocess.Popen([node, '--expose-gc', cli, *args], cwd=project, env=env)
    repairs = 0
    while process.poll() is None:
        try:
            status = gateway_request('/session', session['token'], timeout=2)
            if status['promptRepairCount'] > repairs:
                print('\nHTTP 400: локальная правка контекста, затем повтор в облако.', file=sys.stderr, flush=True)
                repairs = status['promptRepairCount']
        except (OSError, urllib.error.URLError):
            pass
        try:
            process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            pass
    return process.returncode

if __name__ == '__main__':
    try:
        sys.exit(main())
    except (RuntimeError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        print('qwen-work: ' + str(error), file=sys.stderr)
        sys.exit(1)
