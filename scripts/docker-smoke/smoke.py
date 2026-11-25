"""Real Linux installation and desktop smoke test. Never downloads or runs a model."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import urllib.request

HOME=Path.home()
APP=Path(os.environ['SUPA_DEV_HOME'])
BACKEND=APP/'runtime/backend'
REPORTS=HOME/'reports'
sys.path.insert(0,str(BACKEND/'scripts'))
import runtime

def module(name, path):
    spec=importlib.util.spec_from_file_location(name,path)
    value=importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value

manager=module('manager_smoke',BACKEND/'scripts/manage.py')
results=[]
def record(name,detail='ok'):
    results.append({'check':name,'result':'passed','detail':detail})
    (REPORTS/'results.json').write_text(json.dumps(results,indent=2),encoding='utf-8')
    print(json.dumps(results[-1]),flush=True)

def run(args,timeout=300):
    result=subprocess.run(list(map(str,args)),capture_output=True,text=True,timeout=timeout)
    if result.returncode:
        raise RuntimeError(f'{args[0]} exited {result.returncode}: {(result.stderr or result.stdout)[-1200:]}')
    return result

def main():
    record('managed_python',sys.version.split()[0])
    assert 'python-3.13.18' in sys.executable
    workspace=HOME/'workspace with spaces'
    workspace.mkdir(exist_ok=True)
    runtime.save(APP/'config.json',{'workspace':str(workspace),'cloud_model':'qwen4-max','gateway_port':1236,'setup_completed':False,'yolo':True})
    runtime.save(APP/'secrets.bin',{'qwen':'synthetic-key-no-model-execution'})
    launcher=module('launcher_smoke',BACKEND/'scripts/qwen-work.py')
    if not manager.installed(runtime.qwen_programs):
        manager.vendor_install('https://qwen-code-assets.oss-cn-hangzhou.aliyuncs.com/installation/install-qwen-standalone.sh','install-qwen.sh',('--method','standalone'))
    node,cli=runtime.qwen_programs()
    record('qwen_standalone',run([node,cli,'--version']).stdout.strip())
    if not manager.installed(runtime.lm_program):
        manager.vendor_install('https://lmstudio.ai/install.sh','install-lmstudio.sh')
    record('lmstudio_runtime',runtime.lm_program())
    launcher.ensure_server()
    with urllib.request.urlopen('http://127.0.0.1:1234/v1/models',timeout=30) as response:
        data=json.load(response)
    assert not data.get('data'), 'Fresh runtime unexpectedly has a model'
    record('lmstudio_server_without_model')
    launcher.configure()
    key=launcher.ensure_gateway()
    health=launcher.gateway_request('/health')
    assert health['runtimeRoot']==str(BACKEND)
    record('gateway_start_and_health')
    key2=launcher.ensure_gateway()
    assert key==key2
    record('gateway_idempotent_start')
    status=manager.status()
    assert 'alias' not in status
    assert status['qwen']['ready'] and status['lmstudio']['ready'] and status['gateway']['ready']
    assert not status['model']['ready']
    record('component_status_without_alias')
    profile=APP/'qwen-profile/settings.json'
    settings=runtime.read(profile)
    settings['model']['name']='qwen3.7-plus'
    settings['tools']={'allowed':['read_file']}
    runtime.save(profile,settings)
    launcher.configure()
    assert runtime.read(profile)['model']['name']=='qwen3.7-plus'
    assert runtime.read(profile)['tools']=={'allowed':['read_file']}
    record('native_model_and_tool_settings_preserved')
    with (REPORTS/'xvfb.log').open('w') as log:
        subprocess.Popen(['Xvfb',':99','-screen','0','1280x960x24'],stdout=log,stderr=log)
    time.sleep(2)
    with (REPORTS/'desktop.log').open('w') as log:
        desktop=subprocess.Popen([HOME/'release/supa-dev'],stdout=log,stderr=log)
    time.sleep(3)
    assert desktop.poll() is None, 'Desktop executable exited on fresh launch'
    windows=run(['xwininfo','-root','-tree']).stdout
    assert 'Supa Dev' in windows
    record('desktop_first_launch_under_xvfb')
    desktop.terminate()
    desktop.wait(timeout=15)
    manager.launch(str(workspace))
    time.sleep(5)
    windows=run(['xwininfo','-root','-tree']).stdout
    assert 'xterm' in windows.lower() or 'qwen' in windows.lower()
    record('launch_qwen_in_terminal_with_spaced_project_path')
    assert not (HOME/'.local/bin/qwen-work').exists()
    record('no_alias_installed')
    print('SMOKE_PASSED_NO_MODEL_DOWNLOADED_OR_RUN',flush=True)

if __name__=='__main__':
    try:
        main()
    except Exception as error:
        (REPORTS/'failure.txt').write_text(str(error),encoding='utf-8')
        raise
