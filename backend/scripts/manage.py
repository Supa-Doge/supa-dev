"""Installer/status/terminal bridge. JSON-lines is the desktop application's IPC."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import urllib.request
sys.path.insert(0, str(Path(__file__).resolve().parent))
import runtime

def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded

def emit(kind, **value):
    print(json.dumps({'type': kind, **value}, ensure_ascii=False), flush=True)

def progress(message, value=0):
    emit('progress', message=message, progress=value)

def installed(check):
    try:
        return check()
    except (OSError, RuntimeError):
        return None

def status():
    qwen = installed(runtime.qwen_programs)
    lm = installed(runtime.lm_program)
    model = runtime.read(runtime.ROOT / 'config/local-model.json')
    path = runtime.model_path()
    result = {
        'python': {'ready': True, 'detail': sys.version.split()[0]},
        'qwen': {'ready': bool(qwen), 'detail': str(Path(qwen[1]).parent) if qwen else 'Нужна установка'},
        'lmstudio': {'ready': bool(lm), 'detail': lm or 'Официальный runtime LM Studio / llmster'},
        'model': {'ready': path.is_file() and path.stat().st_size == model['sizeBytes'], 'detail': str(path)},
        'gateway': {'ready': False, 'detail': f'127.0.0.1:{runtime.gateway_config()["port"]}'},
    }
    try:
        launcher = module('qwen_launcher', runtime.ROOT / 'scripts/qwen-work.py')
        health = launcher.gateway_request('/health', timeout=2)
        result['gateway']['ready'] = health.get('runtimeRoot') == str(runtime.ROOT)
    except (OSError, ValueError):
        pass
    emit('status', components=result)
    return result

def vendor_install(url, filename, arguments=()):
    directory = runtime.HOME / 'downloads/installers'
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / filename
    with urllib.request.urlopen(url, timeout=60) as response:
        path.write_bytes(response.read(4 * 1024 * 1024))
    if runtime.WIN:
        command = ['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(path), *arguments]
    else:
        command = ['bash', str(path), *arguments]
    result = runtime.run(command, timeout=900)
    log = runtime.HOME / 'logs' / (filename + '.log')
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text(result.stdout + result.stderr, encoding='utf-8')
    if result.returncode:
        raise RuntimeError(f'Установщик {filename} завершился с ошибкой. Журнал: {log}')

def install():
    runtime.qwen_key()
    progress('Проверка Qwen Code')
    if not installed(runtime.qwen_programs):
        progress('Установка Qwen Code с официального сервера')
        suffix = 'ps1' if runtime.WIN else 'sh'
        vendor_install(f'https://qwen-code-assets.oss-cn-hangzhou.aliyuncs.com/installation/install-qwen-standalone.{suffix}',
                       f'install-qwen.{suffix}', () if runtime.WIN else ('--method', 'standalone'))
        runtime.qwen_programs()
    progress('Проверка LM Studio')
    if not installed(runtime.lm_program):
        progress('Установка официального LM Studio runtime (llmster)')
        suffix = 'ps1' if runtime.WIN else 'sh'
        vendor_install(f'https://lmstudio.ai/install.{suffix}', f'install-lmstudio.{suffix}')
        runtime.lm_program()
    launcher = module('qwen_launcher', runtime.ROOT / 'scripts/qwen-work.py')
    progress('Запуск локального API LM Studio')
    launcher.ensure_server()
    downloader = module('model_downloader', runtime.ROOT / 'scripts/download-model.py')
    downloader.download(progress)
    progress('Настройка облачной модели и шлюза')
    launcher.configure()
    launcher.ensure_gateway()
    status()
    progress('Окружение готово', 1)

def launch(project):
    folder = Path(project).expanduser().resolve() if project else runtime.workspace()
    if not folder.is_dir():
        raise RuntimeError('Выбранная папка проекта не существует.')
    runtime.qwen_programs()
    runtime.qwen_key()
    launcher = module('qwen_launcher', runtime.ROOT / 'scripts/qwen-work.py')
    launcher.configure()
    launcher.ensure_gateway()
    env = runtime.environment()
    executable = str(Path(sys.executable).resolve())
    script = str(runtime.ROOT / 'scripts/qwen-work.py')
    if runtime.WIN:
        quote = lambda value: "'" + value.replace("'", "''") + "'"
        expression = f'& {quote(executable)} {quote(script)} --project {quote(str(folder))}'
        # Visible by design: this is the terminal the user explicitly requested.
        command = ['powershell.exe', '-NoExit', '-Command', expression]
        if shutil.which('wt.exe'):
            command = ['wt.exe', '-d', str(folder), *command]
            subprocess.Popen(command, cwd=folder, env=env)
        else:
            subprocess.Popen(command, cwd=folder, env=env, creationflags=subprocess.CREATE_NEW_CONSOLE)
    else:
        args = [executable, script, '--project', str(folder)]
        for terminal, separator in [('x-terminal-emulator','-e'), ('gnome-terminal','--'), ('konsole','-e'), ('xfce4-terminal','-x'), ('xterm','-e')]:
            if shutil.which(terminal):
                subprocess.Popen([terminal, separator, *args], cwd=folder, env=env, start_new_session=True)
                break
        else:
            raise RuntimeError('Не найден терминал. Установите xterm и повторите запуск.')
    emit('log', message='Qwen открыт в терминале.')

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('action', choices=['status','install','configure','launch'])
    parser.add_argument('--project')
    args = parser.parse_args()
    if args.action == 'status':
        status()
    elif args.action == 'install':
        install()
    elif args.action == 'launch':
        launch(args.project)
    else:
        module('qwen_launcher', runtime.ROOT / 'scripts/qwen-work.py').configure()
        status()
    emit('done')

if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        emit('error', message=str(error))
        sys.exit(1)
