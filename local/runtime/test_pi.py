"""Exercise the real supervisor, PTY, durable mailbox and Pi harness adapter."""
import base64
import importlib.util
import json
import multiprocessing
import os
import pathlib
import signal
import shutil
import subprocess
import threading
import sqlite3
import sys
import tempfile
import time
import unittest
import urllib.request
import uuid

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "runtime"))
spec = importlib.util.spec_from_file_location("local_pi", HERE / "pi.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class SupervisorTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = pathlib.Path(self.temporary.name)
        self.home = self.directory / "home"
        self.home.mkdir()
        (self.home / ".magicmtn").mkdir()
        self.bin = self.directory / "bin"
        self.bin.mkdir()
        executable = self.bin / "pi"
        executable.write_text(f'''#!{sys.executable}
import json,os,pathlib,sys,urllib.request
root=pathlib.Path(os.environ['PI_CODING_AGENT_DIR'])
model=json.loads((root/'models.json').read_text())['providers']['magicmtn']
print('PI_READY '+os.getcwd(),flush=True)
for line in sys.stdin:
    if line.strip()=='infer':
        body={{'model':model['models'][0]['id'],'messages':[{{'role':'user','content':'hello'}}],'stream':True}}
        request=urllib.request.Request(model['baseUrl']+'/chat/completions',json.dumps(body).encode(),{{'Authorization':'Bearer '+model['apiKey'],'Content-Type':'application/json'}})
        with urllib.request.urlopen(request,timeout=10) as response:
            print('INFERENCE '+response.read().decode(),flush=True)
    else:
        print('INPUT '+line.strip(),flush=True)
''')
        executable.chmod(0o700)
        self.config = {"session_id": str(uuid.uuid4()), "workspace": str(self.directory),
                       "model": {"id": "model-id", "name": "Test", "contextWindow": 32768, "maxTokens": 4096}}
        self.processes = []
        self.addCleanup(self.cleanup_processes)
        self.start()
        self.wait(lambda: runtime.exchange(self.directory, {})["status"] == "running")

    def cleanup_processes(self):
        try:
            runtime.exchange(self.directory, {"desired": "stopped"})
        except Exception:
            pass
        for process in self.processes:
            process.join(5)
            if process.is_alive():
                process.terminate()
                process.join(5)

    def start(self):
        def run():
            os.environ['HOME'] = str(self.home)
            os.environ['PATH'] = str(self.bin) + os.pathsep + os.environ['PATH']
            os.umask(0o077)
            runtime.serve(self.directory, self.config)
        process = multiprocessing.get_context("fork").Process(target=run)
        process.start()
        self.processes.append(process)

    def wait(self, predicate):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.03)
        self.fail("supervisor did not reach expected state")

    def output(self):
        chunks = []
        cursor = 0
        while True:
            value = runtime.exchange(self.directory, {"cursor": cursor})
            chunk = base64.b64decode(value["output"])
            chunks.append(chunk)
            if not chunk:
                return b"".join(chunks).decode(errors="replace")
            cursor = value["cursor"]

    def input(self, identity, value):
        runtime.exchange(self.directory, {"input_id":identity,"input":base64.b64encode(value.encode()).decode()})

    def test_detach_resume_and_restart_keep_the_same_conversation(self):
        self.wait(lambda: "PI_READY" in self.output())
        self.input("one", "hello\n")
        self.wait(lambda: "INPUT hello" in self.output())
        # Retransmission and terminal detach do not restart or duplicate input.
        self.input("one", "hello\n")
        time.sleep(0.1)
        self.assertEqual(self.output().count("INPUT hello"), 1)
        with runtime.database(self.directory) as db:
            pid = runtime.get(db, "pid")
        self.start()  # A repeated bootstrap cannot start a second Pi.
        self.processes[-1].join(2)
        self.assertFalse(self.processes[-1].is_alive())
        with runtime.database(self.directory) as db:
            self.assertEqual(runtime.get(db, "pid"), pid)
        files = list((self.directory / 'pi' / 'sessions').glob('*.jsonl'))
        self.assertEqual(len(files), 1)
        original = files[0].read_bytes()
        runtime.exchange(self.directory, {"desired":"stopped"})
        self.processes[0].join(5)
        self.assertFalse(self.processes[0].is_alive())
        runtime.exchange(self.directory, {"desired":"running"})
        self.start()
        self.wait(lambda: runtime.exchange(self.directory, {})["status"] == "running")
        self.assertEqual(files[0].read_bytes(), original)
        self.assertEqual(len(list(files[0].parent.glob('*.jsonl'))), 1)

    def test_proxy_uses_scoped_mailbox_and_emits_tool_call_sse(self):
        self.input("infer", "infer\n")
        self.wait(lambda: runtime.exchange(self.directory,{})["request"] is not None)
        pending = runtime.exchange(self.directory,{})["request"]
        with runtime.database(self.directory) as db:
            body = json.loads(db.execute("SELECT request FROM requests WHERE id=?",(pending["id"],)).fetchone()[0])
        self.assertEqual(body['model'], 'model-id')
        response = {"id":"completion", "object":"chat.completion","created":0,"model":"model-id",
                    "choices":[{"index":0,"message":{"role":"assistant","content":"", "tool_calls":[{"id":"call_1","type":"function","function":{"name":"bash","arguments":"{}"}}]},"finish_reason":"tool_calls"}],
                    "usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}
        (self.directory / (pending['id']+'.response')).write_text(json.dumps(response))
        runtime.exchange(self.directory,{"response_id":pending['id']})
        self.wait(lambda: '[DONE]' in self.output())
        self.assertIn('chat.completion.chunk', self.output())
        self.assertIn('"index": 0',self.output())
        self.assertIn('tool_calls',self.output())
        self.wait(lambda: runtime.exchange(self.directory,{})['request'] is None)
        with runtime.database(self.directory) as db:
            row = db.execute("SELECT acknowledged,request,response FROM requests WHERE id=?",(pending['id'],)).fetchone()
        self.assertEqual(row,(1,'{}',None))
        config = json.loads((self.directory / 'pi' / 'models.json').read_text())
        self.assertTrue(config['providers']['magicmtn']['baseUrl'].startswith('http://127.0.0.1:'))

    def test_real_pi_adapter_accepts_text_and_tool_calls_from_the_relay(self):
        module = HERE.parents[1] / 'interfaces/pi/node_modules/@earendil-works/pi-ai/dist/api/openai-completions.js'
        if not module.exists() or not shutil.which('node'):
            self.skipTest('requires the Pi extension development dependencies')
        proxy = runtime.http.server.ThreadingHTTPServer(('127.0.0.1', 0), runtime.Proxy)
        proxy.daemon_threads = True
        proxy.directory = self.directory
        proxy.key = 'guest-scoped-key'
        threading.Thread(target=proxy.serve_forever, daemon=True).start()
        self.addCleanup(proxy.server_close)
        self.addCleanup(proxy.shutdown)
        source = f"""
import {{ streamSimple }} from {json.dumps(module.as_uri())};
const model = {{id:'model-id',name:'Test',provider:'magicmtn',api:'openai-completions',
  baseUrl:'http://127.0.0.1:{proxy.server_port}/v1/harness/agent',reasoning:false,input:['text'],
  contextWindow:32768,maxTokens:4096,cost:{{input:0,output:0,cacheRead:0,cacheWrite:0}},
  compat:{{supportsStore:false,supportsDeveloperRole:false,maxTokensField:'max_tokens'}}}};
const result = await streamSimple(model, {{messages:[{{role:'user',content:[{{type:'text',text:'hello'}}],timestamp:0}}]}},
  {{apiKey:'guest-scoped-key',maxTokens:32}}).result();
console.log(JSON.stringify(result));
"""
        child = subprocess.Popen(['node','--input-type=module','-e',source],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        self.addCleanup(lambda: child.poll() is None and child.kill())
        self.wait(lambda: runtime.exchange(self.directory,{})['request'] is not None)
        pending = runtime.exchange(self.directory,{})['request']
        with runtime.database(self.directory) as db:
            request = json.loads(db.execute('SELECT request FROM requests WHERE id=?',(pending['id'],)).fetchone()[0])
        self.assertEqual(request['model'],'model-id')
        self.assertNotIn('store',request)
        self.assertEqual(request['max_tokens'],32)
        response = {'id':'chatcmpl-test','object':'chat.completion','created':0,'model':'model-id',
            'choices':[{'index':0,'message':{'role':'assistant','content':'Working',
                'tool_calls':[{'id':'call_real','type':'function','function':{'name':'bash','arguments':'{\"command\":\"pwd\"}'}}]},'finish_reason':'tool_calls'}],
            'usage':{'prompt_tokens':11,'completion_tokens':7,'total_tokens':18}}
        (self.directory/(pending['id']+'.response')).write_text(json.dumps(response))
        runtime.exchange(self.directory,{'response_id':pending['id']})
        output, errors = child.communicate(timeout=8)
        self.assertEqual(child.returncode,0,errors)
        result = json.loads(output)
        self.assertEqual(result['stopReason'],'toolUse',result)
        self.assertEqual(result['content'][0]['text'],'Working')
        self.assertEqual(result['content'][1]['name'],'bash')
        self.assertEqual(result['content'][1]['arguments'],{'command':'pwd'})
        self.assertEqual(result['usage']['totalTokens'],18)


if __name__ == '__main__':
    unittest.main()
