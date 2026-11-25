"""Optional real cloud probe. Receives a key over stdin; never logs or retains it."""
import importlib.util
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys

APP=Path(os.environ['SUPA_DEV_HOME'])
BACKEND=APP/'runtime/backend'
sys.path.insert(0,str(BACKEND/'scripts'))
import runtime

key=sys.stdin.read().strip()
if not key:
    raise SystemExit('No cloud key supplied')
secret_file=APP/'secrets.bin'
previous=secret_file.read_bytes() if secret_file.exists() else None
try:
    runtime.save(secret_file,{'qwen':key})
    project=Path.home()/'cloud-probe with spaces'
    project.mkdir(exist_ok=True)
    expected='SUPA_DOCKER_TOOL_OK_'+secrets.token_hex(12)
    (project/'smoke.txt').write_text(expected+'\n',encoding='utf-8')
    result=subprocess.run([sys.executable,BACKEND/'scripts/qwen-work.py','--project',str(project),
                           '-p','Read the file smoke.txt in the current directory using a file-reading tool, then reply with exactly its single line. Do not change files.',
                           '--output-format','json'],capture_output=True,text=True,timeout=180,env=runtime.environment())
    # The expected nonce is never sent in the prompt: success requires reading the fixture.
    output=result.stdout.replace(key,'[REDACTED]')
    error=result.stderr.replace(key,'[REDACTED]')
    if result.returncode or expected not in output:
        (Path.home()/'reports/cloud-failure.log').write_text(output+'\n'+error,encoding='utf-8')
        raise RuntimeError(f'Cloud CLI probe failed (exit {result.returncode}); see cloud-failure.log')
    print('REAL_CLOUD_QWEN_FILE_TOOL_PASSED',flush=True)
    (Path.home()/'reports/cloud-results.json').write_text(json.dumps({'real_cloud':True,'file_tool':True,'spaced_project_path':True,'model_executed_locally':False},indent=2))
finally:
    if previous is None:
        secret_file.unlink(missing_ok=True)
    else:
        secret_file.write_bytes(previous)
        secret_file.chmod(0o600)
