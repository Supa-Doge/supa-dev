"""Portable runtime locations and credentials. Uses only the Python standard library."""
import ctypes
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
HOME = Path(os.environ.get('SUPA_DEV_HOME', ROOT.parent.parent))
FLAGS = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
WIN = os.name == 'nt'

def read(path, default=None):
    try:
        return json.loads(Path(path).read_text('utf-8-sig'))
    except FileNotFoundError:
        return {} if default is None else default

def save(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.tmp')
    temporary.write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    if not WIN:
        temporary.chmod(0o600)
    os.replace(temporary, path)

def settings():
    return read(HOME / 'config.json')

def editor_enabled():
    return read(HOME / 'local-editor.json').get('enabled', True) is not False

def secrets():
    path = HOME / 'secrets.bin'
    if not path.exists():
        return {}
    data = path.read_bytes()
    if WIN:
        from ctypes import wintypes
        class Blob(ctypes.Structure):
            _fields_ = [('length', wintypes.DWORD), ('data', ctypes.POINTER(ctypes.c_ubyte))]
        buffer = (ctypes.c_ubyte * len(data)).from_buffer_copy(data)
        source, target = Blob(len(data), buffer), Blob()
        decrypt = ctypes.windll.crypt32.CryptUnprotectData
        decrypt.argtypes = [ctypes.POINTER(Blob), ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(Blob)]
        decrypt.restype = wintypes.BOOL
        if not decrypt(ctypes.byref(source), None, None, None, None, 1, ctypes.byref(target)):
            raise RuntimeError('Не удалось открыть ключи текущего пользователя Windows.')
        try:
            data = ctypes.string_at(target.data, target.length)
        finally:
            release = ctypes.windll.kernel32.LocalFree
            release.argtypes = [ctypes.c_void_p]
            release.restype = ctypes.c_void_p
            release(target.data)
    return json.loads(data)

def qwen_key():
    value = secrets().get('qwen') or os.environ.get('QWENCLOUD_TOKEN_PLAN_API_KEY')
    if not value:
        value = read(Path.home() / '.qwen/settings.json').get('env', {}).get('QWENCLOUD_TOKEN_PLAN_API_KEY')
    if not value or not value.strip():
        raise RuntimeError('Введите ключ Qwen в Supa Dev и сохраните настройки.')
    return value.strip()

def workspace():
    return Path(settings().get('workspace') or Path.home() / 'Desktop/qwen').expanduser().resolve()

def gateway_config():
    result = read(ROOT / 'config/gateway.json')
    result['port'] = settings().get('gateway_port', 1236)
    result['cloudModel'] = settings().get('cloud_model', result['cloudModel'])
    if result['cloudModel'] not in result['cloudModels']:
        result['cloudModels'].append(result['cloudModel'])
    return result

def qwen_programs():
    if WIN:
        bases = [Path(os.environ.get('LOCALAPPDATA', Path.home() / 'AppData/Local')) / 'qwen-code/qwen-code']
        node_relative = 'node/node.exe'
    else:
        bases = [Path.home() / '.local/lib/qwen-code', Path('/usr/local/lib/qwen-code'),
                 Path.home() / '.local/share/qwen-code/qwen-code', Path.home() / '.local/share/qwen-code']
        node_relative = 'node/bin/node'
    custom = settings().get('qwen_directory')
    if custom:
        bases.insert(0, Path(custom))
    for base in bases:
        node, cli = base / node_relative, base / 'lib/cli.js'
        if node.is_file() and cli.is_file():
            return str(node), str(cli)
    raise RuntimeError('Qwen Code не установлен. Нажмите «Установить окружение».')

def lm_program():
    candidates = [lm_home() / 'bin' / ('lms.exe' if WIN else 'lms')]
    if WIN:
        candidates.insert(0, Path(os.environ.get('LOCALAPPDATA', '')) / 'Programs/LM Studio/resources/app/.webpack/lms.exe')
    found = shutil.which('lms')
    if found:
        candidates.insert(0, Path(found))
    for path in candidates:
        if path.is_file():
            return str(path)
    raise RuntimeError('LM Studio runtime не установлен. Нажмите «Установить окружение».')

def lm_home():
    pointer = Path.home() / '.lmstudio-home-pointer'
    if pointer.is_file() and pointer.read_text('utf-8-sig').strip():
        return Path(pointer.read_text('utf-8-sig').strip()).expanduser()
    if (Path.home() / '.lmstudio').exists():
        return Path.home() / '.lmstudio'
    legacy = Path(os.environ.get('LOCALAPPDATA', '')) / 'lm-studio' if WIN else Path.home() / '.cache/lm-studio'
    return legacy if legacy.is_dir() else Path.home() / '.lmstudio'

def model_directory():
    configured = read(lm_home() / 'settings.json').get('downloadsFolder')
    return Path(configured).expanduser() if configured else lm_home() / 'models'

def model_path():
    model = read(ROOT / 'config/local-model.json')
    return model_directory() / model['repository'] / model['filename']

def environment():
    env = dict(os.environ)
    work = workspace()
    dirs = {'TEMP':'temp','TMP':'temp','TMPDIR':'temp','XDG_CACHE_HOME':'cache',
            'npm_config_cache':'cache/npm','PIP_CACHE_DIR':'cache/pip','UV_CACHE_DIR':'cache/uv'}
    for key, relative in dirs.items():
        directory = work / relative
        directory.mkdir(parents=True, exist_ok=True)
        env[key] = str(directory)
    profile = HOME / 'qwen-profile'
    profile.mkdir(parents=True, exist_ok=True)
    env.update(SUPA_DEV_HOME=str(HOME), PYTHONUTF8='1', PYTHONIOENCODING='utf-8',
               QWEN_HOME=str(profile), QWEN_RUNTIME_DIR=str(profile / 'runtime'),
               QWEN_CONVERSATIONS_DIR=str(profile / 'conversations'))
    return env

def run(args, timeout=300, **kwargs):
    return subprocess.run(list(map(str,args)), creationflags=FLAGS, encoding='utf-8', errors='replace',
                          capture_output=True, timeout=timeout, env=environment(), **kwargs)
