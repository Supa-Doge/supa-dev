"""Portable installer regressions. Temporary directories and loopback HTTP only."""
import copy
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import io
import json
import subprocess
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch, Mock
import urllib.request
import runtime

def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).parent / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value

launcher = module('launcher_test', 'qwen-work.py')
manager = module('manager_test', 'manage.py')
downloader = module('downloader_test', 'download-model.py')

class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='supa-runtime-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.backend = self.home / 'runtime/backend'
        self.backend.mkdir(parents=True)
        for target, key, value in [(runtime,'HOME',self.home), (runtime,'ROOT',self.backend), (launcher,'ROOT',self.backend)]:
            patcher = patch.object(target,key,value)
            patcher.start()
            self.addCleanup(patcher.stop)
        user = patch.object(Path,'home',return_value=self.home / 'user')
        user.start()
        self.addCleanup(user.stop)
        runtime.save(self.home / 'config.json', {'workspace':str(self.home/'work'), 'cloud_model':'cloud-a', 'gateway_port':45678})
        gateway = copy.deepcopy(launcher.GATEWAY)
        gateway.update(cloudModels=['cloud-a','cloud-b'],cloudModel='cloud-a',port=45678)
        runtime.save(self.backend/'config/gateway.json',gateway)
        replace = patch.object(launcher,'GATEWAY',gateway)
        replace.start()
        self.addCleanup(replace.stop)

    def test_native_model_and_tool_preferences_survive_repeated_configuration(self):
        path = self.home/'qwen-profile/settings.json'
        runtime.save(path, {'tools':{'allowed':['*']},'mcpServers':{'example':{'command':'node'}},'model':{'name':'original'}})
        launcher.configure()
        configured=runtime.read(path)
        self.assertEqual(configured['model']['name'],'cloud-a')
        configured['model']['name']='cloud-b'  # native /model persists its own choice
        runtime.save(path,configured)
        launcher.configure()
        after=runtime.read(path)
        self.assertEqual(after['model']['name'],'cloud-b')
        self.assertEqual(after['tools'],{'allowed':['*']})
        self.assertEqual(after['mcpServers'],configured['mcpServers'])
        launcher.GATEWAY['cloudModel']='cloud-c'
        launcher.configure()
        self.assertEqual(runtime.read(path)['model']['name'],'cloud-c')

    def test_install_does_not_modify_shell_profiles_or_create_aliases(self):
        # All external operations are stubs; never download or run the real model.
        user=Path.home()
        user.mkdir()
        profiles=[user/'.bashrc', user/'profile.ps1']
        for path in profiles: path.write_text('user profile',encoding='utf-8')
        fake_launcher, fake_downloader=Mock(), Mock()
        with patch.object(runtime,'qwen_key',return_value='synthetic'), patch.object(manager,'installed',return_value=True), patch.object(manager,'module',side_effect=[fake_launcher,fake_downloader]), patch.object(manager,'status'), patch.object(manager,'emit'), patch.object(runtime,'run',side_effect=AssertionError('Unexpected shell command')):
            manager.install()
        fake_launcher.ensure_gateway.assert_called_once()
        fake_launcher.ensure_server.assert_not_called()
        fake_launcher.ensure_model.assert_not_called()
        fake_downloader.download.assert_called_once()
        for path in profiles: self.assertEqual(path.read_text('utf-8'),'user profile')
        self.assertFalse((user/'.local/bin/qwen-work').exists())
        self.assertFalse((runtime.HOME/'alias-installed.json').exists())

    def test_stopping_editor_persists_pause_even_when_unload_fails(self):
        fake_launcher = Mock()
        fake_launcher.release_model.side_effect = RuntimeError('unload timeout')
        with patch.object(manager, 'module', return_value=fake_launcher), patch.object(manager, 'status'), patch.object(manager, 'emit'):
            with self.assertRaisesRegex(RuntimeError, 'unload timeout'):
                manager.stop_local()
            self.assertFalse(runtime.editor_enabled())
            manager.resume_local()
            self.assertTrue(runtime.editor_enabled())
            fake_launcher.ensure_model.assert_not_called()
            fake_launcher.ensure_server.assert_not_called()

    def test_paused_editor_cannot_restart_runtime(self):
        runtime.save(runtime.HOME/'local-editor.json', {'enabled': False})
        with patch.object(launcher, 'ensure_server', side_effect=AssertionError('must not start')):
            with self.assertRaisesRegex(RuntimeError, 'остановлен'):
                launcher.ensure_model()

    def test_release_never_starts_stopped_daemon_and_targets_only_editor(self):
        status = Mock(returncode=0, stdout='{"status":"not-running"}', stderr='')
        with patch.object(runtime, 'lm_program', return_value='lms'), patch.object(runtime, 'qwen_programs', return_value=('node','cli')), patch.object(runtime, 'run', return_value=status) as run:
            launcher.release_model()
            self.assertEqual(run.call_count, 1)
            self.assertEqual(run.call_args.args[0], ['lms','daemon','status','--json'])
            run.reset_mock()
            status.stdout = '{"status":"running"}'
            launcher.release_model()
            self.assertEqual(run.call_count, 2)
            self.assertEqual(run.call_args.args[0][-1], '--unload')
            self.assertEqual(run.call_args.kwargs['timeout'], 15)
            run.side_effect = [status, subprocess.TimeoutExpired('synthetic unload', 15)]
            with self.assertRaisesRegex(RuntimeError, 'не завершил выгрузку'):
                launcher.release_model()

    def test_unresponsive_status_does_not_report_success_or_start_daemon(self):
        result = Mock(returncode=1, stdout='not json', stderr='failed')
        with patch.object(runtime, 'lm_program', return_value='lms'), patch.object(runtime, 'run', return_value=result) as run:
            with self.assertRaisesRegex(RuntimeError, 'не подтверждена'):
                launcher.release_model()
            self.assertEqual(run.call_count, 1)

    def test_public_installer_accepts_product_user_agent_and_preserves_error_details(self):
        script=b'#!/bin/sh\nexit 0\n'
        def endpoint(request,timeout):
            # LM Studio rejects the default Python urllib User-Agent with HTTP 403.
            if not request.get_header('User-agent','').startswith('SupaDev/'):
                raise RuntimeError('HTTP 403: default Python user agent')
            return io.BytesIO(script)
        result=Mock(returncode=0,stdout='ready',stderr='')
        with patch.object(urllib.request,'urlopen',side_effect=endpoint),patch.object(runtime,'run',return_value=result):
            manager.vendor_install('https://example.invalid/install.sh','installer.sh')
            self.assertEqual((runtime.HOME/'downloads/installers/installer.sh').read_bytes(),script)
            result.returncode=1
            result.stderr='Missing required dependency: libatomic'
            with self.assertRaisesRegex(RuntimeError,'Missing required dependency: libatomic'):
                manager.vendor_install('https://example.invalid/install.sh','installer.sh')

    def test_non_default_lmstudio_home_and_download_folder(self):
        user=Path.home()
        user.mkdir()
        custom=self.home/'custom lm'
        (user/'.lmstudio-home-pointer').write_text(str(custom),encoding='utf-8')
        runtime.save(custom/'settings.json',{'downloadsFolder':str(self.home/'GGUF models')})
        self.assertEqual(runtime.model_directory(),self.home/'GGUF models')

    def test_native_arguments_are_forwarded(self):
        args=['--resume','some-session','--model','custom-cloud','--approval-mode=default','--include-directories','../other']
        self.assertEqual(launcher.qwen_command(('', '', 'node','cli'),{},self.home,args),['node','--expose-gc','cli',*args])

    def test_resumable_model_download_hash_and_existing_file_reuse(self):
        body=b'synthetic gguf bytes\x00' * 256
        digest=hashlib.sha256(body).hexdigest()
        model={'repository':'test/model','revision':'fixed','filename':'synthetic.gguf','sizeBytes':len(body),'sha256':digest}
        runtime.save(self.backend/'config/local-model.json',model)
        pieces=self.home/'downloads'/digest
        pieces.mkdir(parents=True)
        (pieces/'00000.part').write_bytes(body[:13])
        requests=[]
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args): pass
            def do_GET(self):
                start,end=map(int,self.headers['Range'].removeprefix('bytes=').split('-'))
                requests.append((start,end))
                self.send_response(206)
                self.send_header('Content-Range',f'bytes {start}-{end}/{len(body)}')
                self.end_headers()
                self.wfile.write(body[start:end+1])
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        worker=threading.Thread(target=server.serve_forever,daemon=True)
        worker.start()
        real_open=urllib.request.build_opener(urllib.request.ProxyHandler({})).open
        def local_open(request,timeout):
            return real_open(urllib.request.Request(f'http://127.0.0.1:{server.server_port}/model',headers=dict(request.header_items())),timeout=timeout)
        try:
            with patch.object(urllib.request,'urlopen',side_effect=local_open):
                target=downloader.download()
                self.assertEqual(target.read_bytes(),body)
                self.assertEqual(requests,[(13,len(body)-1)])
                self.assertFalse(list(pieces.glob('*.part')))
                downloader.download()
                self.assertEqual(len(requests),1)
        finally:
            server.shutdown()
            server.server_close()
        target.write_bytes(b'x'*len(body))
        with self.assertRaisesRegex(RuntimeError,'контрольная сумма'): downloader.download()
        self.assertEqual(target.read_bytes(),b'x'*len(body))

if __name__=='__main__': unittest.main(verbosity=2)
