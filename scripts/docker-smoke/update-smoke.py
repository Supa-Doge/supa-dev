"""Exercise a released binary's automatic download and real executable replacement."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import signal
import time
import urllib.request
import uuid

HOME=Path.home()
ROOT=HOME/('update-test-'+uuid.uuid4().hex[:8])
ROOT.mkdir(exist_ok=True)
LATEST='0.1.1'
NAME='supa-dev-linux-x86_64'

def fetch(version,name):
    url=f'https://github.com/Supa-Doge/supa-dev/releases/download/v{version}/{name}'
    with urllib.request.urlopen(url,timeout=90) as response:
        return response.read()

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def checksum(version):
    return next(line.split()[0] for line in fetch(version,'SHA256SUMS').decode().splitlines() if line.split()[-1]==NAME)

target=ROOT/'supa-dev'
target.write_bytes(fetch('0.1.0',NAME))
target.chmod(0o700)
old_hash=checksum('0.1.0')
assert digest(target)==old_hash
env={**os.environ,'SUPA_DEV_HOME':str(ROOT/'appdata'),'DISPLAY':':100'}
appdata=ROOT/'appdata'
appdata.mkdir(mode=0o700)
config_file=appdata/'config.json'
config_data=json.dumps({'workspace':str(ROOT/'custom workspace'),'cloud_model':'qwen3.7-plus','gateway_port':32123,'yolo':False,'setup_completed':True}).encode()
config_file.write_bytes(config_data)
secret_file=appdata/'secrets.bin'
secret_data=b'{"qwen":"synthetic-update-preservation-test"}'
secret_file.write_bytes(secret_data)
secret_file.chmod(0o600)
with (HOME/'reports/update.log').open('w') as log:
    display=subprocess.Popen(['Xvfb',':100','-screen','0','1280x960x24'],stdout=log,stderr=log)
    time.sleep(2)
    previous=subprocess.Popen([target],env=env,stdout=log,stderr=log)
    try:
        update=ROOT/'appdata/updates'/LATEST/NAME
        deadline=time.monotonic()+120
        while not update.is_file() and time.monotonic()<deadline:
            assert previous.poll() is None,'Old app exited before downloading update'
            time.sleep(1)
        assert update.is_file(),'Automatic update did not download within 120 seconds'
        expected=checksum(LATEST)
        assert digest(update)==expected,'Automatic download checksum mismatch'
        print('AUTOMATIC_PUBLIC_DOWNLOAD_PASSED',flush=True)
        helper=subprocess.Popen([update,'--apply-update',target,update,str(previous.pid)],env=env,stdout=log,stderr=log)
        time.sleep(1)
        previous.terminate()
        previous.wait(timeout=10)
        assert helper.wait(timeout=40)==0,'Update helper failed'
        assert digest(target)==expected
        assert digest(target.with_suffix('.previous'))==old_hash
        version=subprocess.run([target,'--version'],capture_output=True,text=True,check=True).stdout.strip()
        assert version==f'supa-dev {LATEST}'
        time.sleep(2)
        windows=subprocess.run(['xwininfo','-root','-tree'],env=env,capture_output=True,text=True,check=True).stdout
        assert 'Supa Dev' in windows,'Updated desktop was not relaunched'
        assert config_file.read_bytes()==config_data,'Update changed user settings'
        assert secret_file.read_bytes()==secret_data,'Update changed user credentials'
        print('REPLACE_BACKUP_AND_RELAUNCH_PASSED '+version,flush=True)
        (HOME/'reports/update-results.json').write_text(json.dumps({'automatic_download':True,'sha256':True,'replacement':True,'previous_backup':True,'settings_and_key_preserved':True,'version':version},indent=2))
    finally:
        if previous.poll() is None:
            previous.terminate()
            previous.wait(timeout=15)
        for item in Path('/proc').iterdir():
            if item.name.isdigit():
                try:
                    if (item/'exe').resolve()==target:
                        os.kill(int(item.name),signal.SIGTERM)
                except (OSError,ProcessLookupError):
                    pass
        display.terminate()
        display.wait(timeout=10)
