"""Gateway contracts and real local prompt editing. Cloud is always a loopback stub."""
import argparse
import copy
import importlib.util
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import threading
import time
import unittest
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
RUN = uuid.uuid4().hex[:12]
TEMP = ROOT / 'temp' / ('gateway-tests-' + RUN)
PROJECTS = ROOT / 'projects' / ('gateway-tests-' + RUN)
TEMP.mkdir(parents=True)
PROJECTS.mkdir(parents=True)


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


implementation = module('gateway_server_test', ROOT / 'components/gateway/server.py')
launcher = module('gateway_launcher_test', ROOT / 'scripts/qwen-work.py')
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))
CANARY = 'SYNTHETIC_GATEWAY_CONTENT_DO_NOT_LOG_74fa'


def start(server):
    threading.Thread(target=lambda: server.serve_forever(poll_interval=0.05), daemon=True).start()
    return f'http://127.0.0.1:{server.server_address[1]}'


def request(base, token, body=None, path='/v1/chat/completions', headers=None):
    req = urllib.request.Request(base + path, json.dumps(body).encode() if body is not None else None,
        {'Content-Type': 'application/json', 'Authorization': 'Bearer ' + token, **(headers or {})})
    try:
        response = OPENER.open(req, timeout=300)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        return response.status, dict(response.headers), response.read()


def payload(stream=False):
    return {'model': 'qwen3.7-plus', 'stream': stream, 'max_tokens': 64,
        'messages': [{'role': 'user', 'content': CANARY}],
        'tools': [{'type': 'function', 'function': {'name': 'read_file', 'parameters': {'type': 'object', 'properties': {}}}}]}


class Stub:
    def __init__(self, status=200):
        self.status, self.statuses, self.calls = status, [], []
        self.content = None
        self.extra_message = {}
        self.finish_reason = 'stop'
        self.error = {'error': {'message': 'Please rephrase the user request concisely.', 'code': 'InvalidPrompt'}}
        self.release_stream = threading.Event()
        self.release_stream.set()
        self.stream_finished = False
        stub = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                stub.calls.append({'body': body, 'authorization': self.headers.get('Authorization')})
                status = stub.statuses.pop(0) if stub.statuses else stub.status
                self.send_response(status)
                if status == 200 and body.get('stream'):
                    self.send_header('Content-Type', 'text/event-stream')
                    self.end_headers()
                    self.wfile.write(b'data: {"choices":[{"delta":{"content":"CLOUD_STREAM_OK"}}]}\n\n')
                    self.wfile.flush()
                    stub.release_stream.wait(5)
                    time.sleep(0.05)
                    stub.stream_finished = True
                    self.wfile.write(b'data: [DONE]\n\n')
                else:
                    self.send_header('Content-Type', 'application/json')
                    if status == 302:
                        self.send_header('Location', 'https://example.invalid/must-not-follow')
                    self.end_headers()
                    content = stub.content
                    if callable(content):
                        content = content(body)
                    if content is None and body['messages'][0].get('content') == implementation.REPAIR_INSTRUCTION:
                        entry = json.loads(body['messages'][1]['content'])['entries'][0]
                        content = json.dumps({'edits': [{'id': entry['id'], 'text': 'Rephrased: ' + entry['text']}]})
                    result = stub.error if status != 200 else {'choices': [{'finish_reason': stub.finish_reason,
                        'message': {'role': 'assistant', 'content': content if content is not None else 'CLOUD_OK', **stub.extra_message}}]}
                    self.wfile.write(json.dumps(result).encode())
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.base = start(self.server)

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class GatewayTests(unittest.TestCase):
    def setUp(self):
        self.cloud, self.local = Stub(), Stub()
        self.folder = TEMP / self._testMethodName
        self.project = PROJECTS / self._testMethodName
        self.project.mkdir()
        config = copy.deepcopy(launcher.GATEWAY)
        config.update(cloudBaseUrl=self.cloud.base + '/v1', localBaseUrl=self.local.base + '/v1')
        self.gateway = implementation.Gateway(config, self.folder, PROJECTS, self.folder / 'events.jsonl', 'test-control',
            credential_provider=lambda _: 'synthetic-cloud-key', local_preparer=lambda _: None, gpu_provider=lambda: 3000, allow_test_upstream=True)
        self.server = implementation.make_server(self.gateway, port=0)
        self.base = start(self.server)
        self.session = self.gateway.register(self.project)

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.cloud.close()
        self.local.close()

    def call(self, body=None, session=None):
        return request(self.base, (session or self.session)['token'], body if body is not None else payload())

    def test_cloud_success_stays_primary(self):
        for _ in range(2):
            status, headers, data = self.call()
            self.assertEqual(status, 200)
            self.assertEqual(headers['X-Qwen-Route'], 'cloud')
            self.assertIn(b'CLOUD_OK', data)
        self.assertEqual(len(self.cloud.calls), 2)
        self.assertEqual(len(self.local.calls), 0)
        self.assertEqual(self.cloud.calls[0]['authorization'], 'Bearer synthetic-cloud-key')

    def test_selected_cloud_model_survives_prompt_repair(self):
        self.cloud.statuses = [400, 200]
        body = payload()
        body['model'] = 'qwen4-max'
        self.assertEqual(self.call(body)[0], 200)
        self.assertEqual([call['body']['model'] for call in self.cloud.calls], ['qwen4-max', 'qwen4-max'])
        self.assertEqual(self.local.calls[0]['body']['model'], 'local-coder')
        self.assertEqual(self.gateway.public_session(self.gateway.session(self.session['token']))['model'], 'qwen4-max')
        body['model'] = 'qwen3.8-flash'
        self.assertEqual(self.call(body)[0], 200)
        self.assertEqual(self.cloud.calls[-1]['body']['model'], 'qwen3.8-flash')

    def test_model_catalog_allows_custom_cloud_models(self):
        status, _, data = request(self.base, self.session['token'], path='/v1/models')
        self.assertEqual(status, 200)
        self.assertEqual([entry['id'] for entry in json.loads(data)['data']], self.gateway.cloud_models)
        body = payload()
        body['model'] = 'not-configured'
        self.assertEqual(self.call(body)[0], 200)
        self.assertEqual(self.cloud.calls[-1]['body']['model'], 'not-configured')
        self.assertEqual(len(self.local.calls), 0)
        local_session = self.gateway.register(self.project, force_local=True)
        status, _, data = request(self.base, local_session['token'], path='/v1/models')
        self.assertNotIn('local-coder', [entry['id'] for entry in json.loads(data)['data']])
        body['model'] = 'qwen4-max'
        self.assertEqual(self.call(body, session=local_session)[1]['X-Qwen-Route'], 'cloud')
        self.assertEqual(len(self.local.calls), 0)
        self.assertEqual(len(self.cloud.calls), 2)

    def test_400_edits_prompt_without_tools_then_streams_cloud(self):
        self.cloud.statuses = [400, 200]
        body = payload(True)
        status, headers, data = self.call(body)
        self.assertEqual(status, 200)
        self.assertEqual(headers['X-Qwen-Route'], 'cloud')
        self.assertEqual(headers['X-Qwen-Prompt-Repairs'], '1')
        self.assertIn(b'CLOUD_STREAM_OK', data)
        self.assertIn(b'[DONE]', data)
        local = self.local.calls[0]['body']
        self.assertFalse(local['stream'])
        self.assertNotIn('tools', local)
        self.assertNotIn('tool_choice', local)
        self.assertEqual(local['model'], 'local-coder')
        self.assertEqual(local['max_tokens'], 4096)
        self.assertEqual(local['messages'][0]['content'], implementation.REPAIR_INSTRUCTION)
        source = json.loads(local['messages'][1]['content'])
        self.assertIn('InvalidPrompt', source['cloud_error'])
        self.assertEqual(source['entries'][0]['text'], CANARY)
        self.assertEqual(self.local.calls[0]['authorization'], 'Bearer lm-studio-local')
        repaired = self.cloud.calls[1]['body']
        self.assertEqual(repaired['messages'][0]['content'], 'Rephrased: ' + CANARY)
        self.assertEqual(repaired['tools'], body['tools'])
        self.assertEqual(repaired['max_tokens'], body['max_tokens'])
        self.assertFalse((self.project / '.local-only').exists())
        self.assertEqual(self.call()[1]['X-Qwen-Route'], 'cloud')
        self.assertEqual(len(self.cloud.calls), 3)
        self.assertEqual(len(self.local.calls), 1)
        log = (self.folder / 'events.jsonl').read_text('utf-8')
        for secret in (CANARY, 'synthetic-cloud-key', self.session['token'], 'InvalidPrompt'):
            self.assertNotIn(secret, log)

    def test_repeated_400_stops_after_two_repairs(self):
        self.cloud.status = 400
        status, headers, data = self.call()
        self.assertEqual(status, 400)
        self.assertEqual(json.loads(data), self.cloud.error)
        self.assertEqual(headers['X-Qwen-Prompt-Repairs'], '2')
        self.assertEqual(len(self.cloud.calls), 3)
        self.assertEqual(len(self.local.calls), 2)
        source = json.loads(self.local.calls[1]['body']['messages'][1]['content'])
        self.assertEqual(source['entries'][0]['text'], 'Rephrased: ' + CANARY)
        self.assertEqual(source['repair_round'], 2)
        self.assertEqual(self.gateway.public_session(self.gateway.session(self.session['token']))['route'], 'cloud')
        self.assertFalse((self.project / '.local-only').exists())

    def test_other_errors_are_not_repaired(self):
        for status in (401, 403, 404, 413, 429, 500, 503):
            self.cloud.status = status
            self.assertEqual(self.call()[0], status)
        self.assertEqual(len(self.local.calls), 0)

    def test_stream_and_status_do_not_wait_for_cloud_completion(self):
        self.cloud.statuses = [400, 200]
        self.cloud.release_stream.clear()
        req = urllib.request.Request(self.base + '/v1/chat/completions', json.dumps(payload(True)).encode(),
            {'Content-Type': 'application/json', 'Authorization': 'Bearer ' + self.session['token']})
        try:
            with OPENER.open(req, timeout=10) as response:
                self.assertIn(b'CLOUD_STREAM_OK', response.readline())
                self.assertFalse(self.cloud.stream_finished)
                status, _, data = request(self.base, self.session['token'], path='/session')
                self.assertEqual(status, 200)
                self.assertEqual(json.loads(data)['route'], 'cloud')
                self.assertEqual(json.loads(data)['promptRepairCount'], 1)
                self.assertFalse(self.cloud.stream_finished)
                self.cloud.release_stream.set()
                self.assertIn(b'[DONE]', response.read())
        finally:
            self.cloud.release_stream.set()

    def test_local_failure_does_not_execute_or_pin_task(self):
        self.cloud.status, self.local.status = 400, 500
        self.assertEqual(self.call()[0], 503)
        self.assertEqual(len(self.cloud.calls), 1)
        self.assertEqual(len(self.local.calls), 1)
        self.cloud.status = 200
        self.assertEqual(self.call()[1]['X-Qwen-Route'], 'cloud')
        self.assertEqual(len(self.local.calls), 1)
        self.assertFalse((self.project / '.local-only').exists())

    def test_invalid_local_output_never_becomes_task_answer(self):
        self.cloud.status = 400
        for content in ('I have edited your file.', '{bad json', '{"edits":[{"id":"0:content","text":""}]}',
                        '{"edits":[{"id":"999:content","text":"x"}]}',
                        '{"edits":[{"id":"0:content","text":"x"},{"id":"0:content","text":"y"}]}'):
            with self.subTest(content=content):
                self.local.content = content
                count = len(self.cloud.calls)
                status, _, data = self.call()
                self.assertEqual(status, 503)
                self.assertNotIn(b'I have edited', data)
                self.assertEqual(len(self.cloud.calls), count + 1)

    def test_local_tool_call_and_truncated_output_are_rejected(self):
        self.cloud.status = 400
        self.local.extra_message = {'tool_calls': [{'type': 'function', 'function': {'name': 'write_file'}}]}
        self.assertEqual(self.call()[0], 503)
        self.local.extra_message = {}
        self.local.finish_reason = 'length'
        self.assertEqual(self.call()[0], 503)
        self.assertEqual(len(self.cloud.calls), 2)

    def test_unchanged_prompt_returns_cloud_error_without_retry(self):
        self.cloud.status = 400
        for content in ('{"edits":[]}', json.dumps({'edits': [{'id': '0:content', 'text': CANARY}]})):
            self.local.content = content
            count = len(self.cloud.calls)
            status, _, data = self.call()
            self.assertEqual(status, 400)
            self.assertEqual(json.loads(data), self.cloud.error)
            self.assertEqual(len(self.cloud.calls), count + 1)

    def test_system_tools_images_and_parameters_are_preserved(self):
        self.cloud.statuses = [400, 200]
        body = payload()
        body.update(tool_choice='auto', parallel_tool_calls=False, response_format={'type': 'json_object'})
        body['messages'] = [
            {'role': 'system', 'content': 'SYSTEM_UNCHANGED'},
            {'role': 'user', 'content': [{'type': 'text', 'text': 'Исправь multiply.py'},
                {'type': 'image_url', 'image_url': {'url': 'data:image/png;base64,c3ludGhldGlj'}}]},
            {'role': 'assistant', 'content': None, 'tool_calls': [{'id': 'call1', 'type': 'function',
                'function': {'name': 'read_file', 'arguments': '{"path":"multiply.py"}'}}]},
            {'role': 'tool', 'tool_call_id': 'call1', 'content': 'def multiply(a,b): return a+b'}]
        original = copy.deepcopy(body)
        self.assertEqual(self.call(body)[0], 200)
        expected = copy.deepcopy(original)
        expected['messages'][1]['content'][0]['text'] = 'Rephrased: Исправь multiply.py'
        self.assertEqual(self.cloud.calls[1]['body'], expected)
        self.assertEqual(body, original)
        local = self.local.calls[0]['body']
        self.assertNotIn('SYSTEM_UNCHANGED', json.dumps(local))
        self.assertNotIn('def multiply', json.dumps(local))

    def test_repair_cannot_modify_system_text(self):
        self.cloud.status = 400
        self.local.content = '{"edits":[{"id":"0:content","text":"replacement system"}]}'
        body = payload()
        body['messages'].insert(0, {'role': 'system', 'content': 'original system'})
        self.assertEqual(self.call(body)[0], 503)
        self.assertEqual(len(self.cloud.calls), 1)

    def test_local_preparation_failure_and_vram_guard(self):
        self.cloud.status = 400
        def unavailable(_):
            raise RuntimeError('synthetic load failure')
        self.gateway.local_preparer = unavailable
        self.assertEqual(self.call()[0], 503)
        self.gateway.local_preparer = lambda _: None
        self.gateway.gpu_provider = lambda: 512
        self.assertEqual(self.call()[0], 503)
        self.assertEqual(len(self.cloud.calls), 2)
        self.assertEqual(len(self.local.calls), 0)
        self.assertFalse((self.project / '.local-only').exists())

    def test_old_local_markers_and_restored_history_use_cloud(self):
        restored = str(uuid.uuid4())
        status, headers, _ = request(self.base, self.session['token'], payload(), headers={'X-Qwen-Conversation-Id': restored})
        self.assertEqual(status, 200)
        self.assertEqual(headers['X-Qwen-Route'], 'cloud')
        (self.project / '.local-only').touch()
        self.assertEqual(self.call()[1]['X-Qwen-Route'], 'cloud')
        self.assertEqual(len(self.cloud.calls), 2)
        self.assertEqual(len(self.local.calls), 0)

    def test_legacy_force_local_and_local_model_selection_cannot_execute_locally(self):
        session = self.gateway.register(self.project, force_local=True)
        self.local.status = 400
        body = payload()
        body['model'] = 'local-coder'
        self.assertEqual(self.call(body, session=session)[0], 200)
        self.assertEqual(len(self.local.calls), 0)
        self.assertEqual(self.cloud.calls[0]['body']['model'], self.gateway.config['cloudModel'])

    def test_old_persisted_local_session_migrates_to_cloud(self):
        session = self.gateway.session(self.session['token'])
        session.update(route='local', reason='local_only_project')
        implementation.atomic_json(self.gateway.state_file, self.gateway.state)
        migrated = implementation.Gateway(self.gateway.config, self.folder, PROJECTS, self.folder / 'events.jsonl', 'test-control',
            credential_provider=lambda _: 'synthetic-cloud-key', local_preparer=lambda _: None, gpu_provider=lambda: 3000, allow_test_upstream=True)
        self.assertEqual(migrated.session(self.session['token'])['route'], 'cloud')

    def test_project_outside_projects_directory_is_allowed(self):
        outside = TEMP / ('independent-' + uuid.uuid4().hex)
        outside.mkdir()
        session = self.gateway.register(outside)
        self.assertEqual(self.call(session=session)[1]['X-Qwen-Route'], 'cloud')
        self.assertEqual(len(self.local.calls), 0)

    def test_restart_and_new_session_keep_cloud_primary(self):
        self.cloud.statuses = [400, 200]
        self.assertEqual(self.call()[0], 200)
        self.server.shutdown()
        self.server.server_close()
        self.gateway = implementation.Gateway(self.gateway.config, self.folder, PROJECTS, self.folder / 'events.jsonl', 'test-control',
            credential_provider=lambda _: 'synthetic-cloud-key', local_preparer=lambda _: None, gpu_provider=lambda: 3000, allow_test_upstream=True)
        self.server = implementation.make_server(self.gateway, port=0)
        self.base = start(self.server)
        for session in (self.session, self.gateway.register(self.project)):
            self.assertEqual(self.call(session=session)[1]['X-Qwen-Route'], 'cloud')
        self.assertEqual(len(self.cloud.calls), 4)
        self.assertEqual(len(self.local.calls), 1)
        self.assertEqual(self.gateway.public_session(self.gateway.session(self.session['token']))['promptRepairCount'], 1)

    def test_authentication_and_redirects(self):
        self.assertEqual(request(self.base, 'wrong', payload())[0], 401)
        self.assertEqual(request(self.base, 'wrong', {'project': str(self.project)}, '/admin/sessions')[0], 401)
        self.assertEqual(len(self.cloud.calls), 0)
        self.cloud.status = 302
        self.assertEqual(self.call()[0], 502)
        self.assertEqual(len(self.cloud.calls), 1)
        self.cloud.statuses = [400, 302]
        self.assertEqual(self.call()[0], 502)
        self.assertEqual(len(self.cloud.calls), 3)

    def test_missing_editable_text_fails_without_execution(self):
        self.cloud.status = 400
        body = payload()
        body['messages'] = [{'role': 'system', 'content': 'No editable text.'}]
        self.assertEqual(self.call(body)[0], 503)
        self.assertEqual(len(self.local.calls), 0)
        self.assertEqual(len(self.cloud.calls), 1)

    def test_long_prompt_is_fully_edited_in_bounded_chunks(self):
        self.cloud.statuses = [400, 200]
        body = payload()
        body['messages'][0]['content'] = 'x' * 25000
        self.assertEqual(self.call(body)[0], 200)
        entries = [json.loads(c['body']['messages'][1]['content'])['entries'][0] for c in self.local.calls]
        self.assertEqual(''.join(e['text'] for e in entries), 'x' * 25000)
        self.assertTrue(all(len(e['text'].encode('utf-8')) <= 6000 for e in entries))
        self.assertEqual(len(self.cloud.calls), 2)
        self.assertEqual(self.cloud.calls[-1]['body']['messages'][0]['content'].count('x'), 25000)

    def test_content_error_repairs_tool_history_and_reuses_accepted_edits(self):
        self.cloud.error = {'error': {'code': 'data_inspection_failed', 'message': 'Input text data may contain inappropriate content.'}}
        self.cloud.statuses = [400, 200]
        def rewrite(body):
            entries = json.loads(body['messages'][1]['content'])['entries']
            return json.dumps({'edits': [{'id': e['id'], 'text': e['text'].replace('RUDE', 'neutral')}
                                       for e in entries if 'RUDE' in e['text']]})
        self.local.content = rewrite
        body = payload()
        body['messages'] = [
            {'role': 'system', 'content': 'SYSTEM_UNCHANGED'},
            {'role': 'user', 'content': 'Update only the version number.'},
            {'role': 'assistant', 'content': 'RUDE history', 'tool_calls': [{'id': 'call1', 'type': 'function',
                'function': {'name': 'read_file', 'arguments': '{"path":"rules.json"}'}}]},
            {'role': 'tool', 'tool_call_id': 'call1', 'content': json.dumps({'version': 2, 'description': 'RUDE narrative'})},
            {'role': 'user', 'content': 'Continue RUDE'}]
        self.assertEqual(self.call(body)[0], 200)
        sent = self.cloud.calls[-1]['body']
        self.assertEqual(sent['messages'][0], body['messages'][0])
        self.assertEqual(sent['messages'][2]['tool_calls'], body['messages'][2]['tool_calls'])
        self.assertEqual(json.loads(sent['messages'][3]['content']), {'version': 2, 'description': 'neutral narrative'})
        self.assertNotIn('RUDE', json.dumps(sent))
        entries = [entry for call in self.local.calls for entry in json.loads(call['body']['messages'][1]['content'])['entries']]
        self.assertTrue(all(entry.get('kind') == 'quoted_tool_text' for entry in entries if entry['role'] == 'tool'))
        self.assertTrue(all('kind' not in entry for entry in entries if entry['role'] == 'user'))
        file_call = next(call['body'] for call in self.local.calls
                         if call['body']['messages'][0]['content'] == implementation.FILE_REPAIR_INSTRUCTION)
        schema = file_call['response_format']['json_schema']['schema']['properties']['edits']['items']
        self.assertEqual(set(schema['required']), {'id', 'text'})
        self.assertNotIn('old', schema['properties'])
        self.assertEqual(file_call['temperature'], 0)
        calls = len(self.local.calls)
        self.assertEqual(self.call(body)[0], 200)
        self.assertEqual(len(self.local.calls), calls)
        self.assertNotIn('RUDE', json.dumps(self.cloud.calls[-1]['body']))
        other = self.gateway.register(self.project)
        self.assertEqual(self.call(body, session=other)[0], 200)
        self.assertIn('RUDE', json.dumps(self.cloud.calls[-1]['body']))

    def test_content_repair_cannot_corrupt_json_or_change_numeric_values(self):
        self.cloud.status = 400
        self.cloud.error = {'error': {'code': 'data_inspection_failed'}}
        body = payload()
        body['messages'] = [{'role': 'tool', 'tool_call_id': 'call1', 'content': '{"version": 2, "description": "text"}'}]
        self.local.content = '{"edits":[{"id":"0:content","text":"{\\"version\\":3,\\"description\\":\\"neutral\\"}"}]}'
        self.assertEqual(self.call(body)[0], 503)
        self.assertEqual(len(self.cloud.calls), 1)

    def test_exact_substring_edits_and_file_references(self):
        self.cloud.statuses = [400, 200]
        body = payload()
        body['messages'][0]['content'] = 'RUDE please update rules.json. RUDE'
        self.local.content = '{"edits":[{"id":"0:content","old":"RUDE","text":""}]}'
        self.assertEqual(self.call(body)[0], 200)
        self.assertEqual(self.cloud.calls[-1]['body']['messages'][0]['content'], ' please update rules.json. ')
        self.cloud.status = 400
        other = self.gateway.register(self.project)
        self.local.content = '{"edits":[{"id":"0:content","old":"rules.json","text":"other.json"}]}'
        self.assertEqual(self.call(body, session=other)[0], 503)
        self.local.content = '{"edits":[{"id":"0:content","old":"not in the input","text":"neutral"}]}'
        self.assertEqual(self.call(body, session=other)[0], 503)

    def test_nested_json_exposes_only_text_values_to_editor(self):
        self.cloud.error = {'error': {'code': 'data_inspection_failed'}}
        self.cloud.statuses = [400, 200]
        original_file = {'RUDE_key': 'RUDE narrative', 'version': '2.0', 'count': 7, 'file': 'rules.json'}
        body = payload()
        body['messages'] = [{'role': 'tool', 'tool_call_id': 'call1',
                             'content': json.dumps({'output': json.dumps(original_file)})}]
        def rewrite(body):
            entries = json.loads(body['messages'][1]['content'])['entries']
            self.assertNotIn('2.0', [e['text'] for e in entries])
            self.assertNotIn('RUDE_key', [e['text'] for e in entries])
            return json.dumps({'edits': [{'id': e['id'], 'old': 'RUDE', 'text': 'neutral'}
                                       for e in entries if 'RUDE' in e['text']]})
        self.local.content = rewrite
        self.assertEqual(self.call(body)[0], 200)
        result = json.loads(json.loads(self.cloud.calls[-1]['body']['messages'][0]['content'])['output'])
        self.assertEqual(result, {**original_file, 'RUDE_key': 'neutral narrative'})

    def test_invalid_batch_is_retried_in_smaller_groups(self):
        self.cloud.error = {'error': {'code': 'data_inspection_failed'}}
        self.cloud.statuses = [400, 200]
        body = payload()
        body['messages'] = [{'role': 'tool', 'tool_call_id': 'call1',
                             'content': json.dumps({'first': 'RUDE one', 'second': 'RUDE two'})}]
        def rewrite(body):
            entries = json.loads(body['messages'][1]['content'])['entries']
            if len(entries) > 1:
                return '{bad json'
            return json.dumps({'edits': [{'id': entries[0]['id'], 'old': 'RUDE', 'text': 'neutral'}]})
        self.local.content = rewrite
        self.assertEqual(self.call(body)[0], 200)
        self.assertEqual(len(self.local.calls), 3)
        self.assertEqual(len(self.cloud.calls), 2)
        self.assertEqual(json.loads(self.cloud.calls[-1]['body']['messages'][0]['content']),
                         {'first': 'neutral one', 'second': 'neutral two'})

    def test_native_reminders_do_not_replace_the_actual_task_context(self):
        self.cloud.error = {'error': {'code': 'data_inspection_failed'}}
        self.cloud.statuses = [400, 200]
        task = '<state_snapshot>Update JSON versions and check file references.</state_snapshot>'
        body = payload()
        body['messages'] = [
            {'role': 'user', 'content': [
                {'type': 'text', 'text': '<system-reminder>\nThe following skills are available: RUDE metadata.</system-reminder>'},
                {'type': 'text', 'text': task}]},
            {'role': 'user', 'content': [
                {'type': 'text', 'text': 'Current memory catalog (full snapshot; supersedes earlier catalogs): RUDE metadata'},
                {'type': 'text', 'text': 'Continue RUDE'}]}]
        def rewrite(body):
            source = json.loads(body['messages'][1]['content'])
            self.assertEqual(source['task_context'], [task, 'Continue RUDE'])
            self.assertTrue(all('metadata' not in entry['text'] for entry in source['entries']))
            return json.dumps({'edits': [{'id': e['id'], 'old': 'RUDE', 'text': ''}
                                       for e in source['entries'] if 'RUDE' in e['text']]})
        self.local.content = rewrite
        self.assertEqual(self.call(body)[0], 200)
        self.assertEqual(self.cloud.calls[-1]['body']['messages'][0], body['messages'][0])

    def test_invalid_single_edit_is_retried_with_validation_feedback(self):
        self.cloud.statuses = [400, 200]
        preparations = []
        self.gateway.local_preparer = lambda _: preparations.append(True)
        def rewrite(body):
            source = json.loads(body['messages'][1]['content'])
            if 'validation_feedback' not in source:
                return '{"edits":[{"id":"0:content","old":"absent substring","text":"neutral"}]}'
            return json.dumps({'edits': [{'id': '0:content', 'old': CANARY, 'text': 'Please continue.'}]})
        self.local.content = rewrite
        self.assertEqual(self.call()[0], 200)
        self.assertEqual(len(self.local.calls), 2)
        self.assertEqual(len(self.cloud.calls), 2)
        self.assertEqual(len(preparations), 1)

    def test_diagnostic_capture_is_scoped_and_consumed_once(self):
        previous_root = implementation.ROOT
        implementation.ROOT = self.folder
        try:
            trigger = self.folder / 'temp/gateway-capture-next.json'
            implementation.atomic_json(trigger, {'sessionId': self.session['sessionId']})
            other = self.gateway.register(self.project)
            self.assertEqual(self.call(session=other)[0], 200)
            self.assertTrue(trigger.exists())
            self.assertEqual(self.call()[0], 200)
            self.assertFalse(trigger.exists())
            self.assertEqual(self.call()[0], 200)
            captures = list((self.folder / 'temp/live-gateway-capture').glob('*/original.json'))
            self.assertEqual(len(captures), 1)
            self.assertEqual(implementation.read_json(captures[0]), payload())
            self.assertNotIn(self.session['token'], captures[0].read_text('utf-8'))
        finally:
            implementation.ROOT = previous_root

    def test_import_accepted_edits_requires_control_auth_and_preserves_protocol(self):
        original = payload()
        repaired = copy.deepcopy(original)
        repaired['messages'][0]['content'] = 'Continue the maintenance task.'
        data = {'sessionId': self.session['sessionId'], 'original': original, 'repaired': repaired}
        self.assertEqual(request(self.base, 'wrong', data, '/admin/prompt-cache')[0], 401)
        invalid = copy.deepcopy(data)
        invalid['repaired']['tools'] = []
        self.assertEqual(request(self.base, 'test-control', invalid, '/admin/prompt-cache')[0], 400)
        self.assertEqual(request(self.base, 'test-control', data, '/admin/prompt-cache')[0], 200)
        self.assertEqual(self.call(original)[0], 200)
        self.assertEqual(self.cloud.calls[-1]['body'], repaired)
        self.assertEqual(len(self.local.calls), 0)
        other = self.gateway.register(self.project)
        self.assertEqual(self.call(original, session=other)[0], 200)
        self.assertEqual(self.cloud.calls[-1]['body'], original)

    def test_repair_can_be_disabled(self):
        self.gateway.max_repairs = 0
        self.cloud.status = 400
        self.assertEqual(self.call()[0], 400)
        self.assertEqual(len(self.cloud.calls), 1)
        self.assertEqual(len(self.local.calls), 0)


def real_local_repair():
    cloud = Stub()
    cloud.statuses = [400, 200]
    folder, project = TEMP / 'real-editor', PROJECTS / 'real-editor'
    project.mkdir()
    config = copy.deepcopy(launcher.GATEWAY)
    config['cloudBaseUrl'] = cloud.base + '/v1'
    gateway = implementation.Gateway(config, folder, PROJECTS, folder / 'events.jsonl', 'test-control',
        credential_provider=lambda _: 'synthetic-cloud-key', allow_test_upstream=True)
    server = implementation.make_server(gateway, port=0)
    base = start(server)
    session = gateway.register(project)
    body = payload()
    body['messages'][0]['content'] = ('Please please please, I need you to explain what this function does. '
        'Explain what it does, just explain, in simple terms. Function: def add(a, b): return a + b. '
        'I repeat, please explain this function simply, in one short sentence.')
    try:
        status, headers, data = request(base, session['token'], body)
        assert status == 200, f'Prompt repair failed with HTTP {status}: {data.decode()}'
        assert headers['X-Qwen-Route'] == 'cloud' and b'CLOUD_OK' in data
        assert len(cloud.calls) == 2, 'Expected initial rejection and one repaired cloud retry.'
        repaired = cloud.calls[1]['body']['messages'][0]['content']
        assert repaired != body['messages'][0]['content'] and 'add' in repaired
        assert cloud.calls[1]['body']['tools'] == body['tools']
        assert not (project / '.local-only').exists()
        assert not list(project.iterdir()), 'The local editor must not modify project files.'
        return {'realLocalPromptRepair': True, 'cloudRequests': len(cloud.calls),
                'taskAnswerFromCloudStub': True, 'projectFilesUntouched': True}
    finally:
        server.shutdown()
        server.server_close()
        cloud.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--synthetic-only', action='store_true')
    options = parser.parse_args()
    report = {'syntheticDataOnly': True, 'realCloudUsed': False, 'routing': 'local_prompt_repair_then_cloud'}
    try:
        result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(GatewayTests))
        report['contractTests'] = {'run': result.testsRun, 'failures': len(result.failures), 'errors': len(result.errors)}
        assert result.wasSuccessful(), 'Gateway contract tests failed.'
        if not options.synthetic_only:
            report.update(real_local_repair())
        report['status'] = 'passed'
    except Exception as error:
        report.update(status='failed', error=str(error))
        raise
    finally:
        launcher.save(ROOT / 'output/gateway-tests.json', report)
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
