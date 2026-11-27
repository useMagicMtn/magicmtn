---
title: (Local) Ollama inference + Apple Container VMs
slug: /open-source/guides/ollama-apple-container
description: Generate code with Ollama on your Mac and run it inside a local Apple Container VM managed by MagicMtn.
---

Run inference with Ollama on your Mac and execute generated code inside a MagicMtn Apple Container VM. This example uses Ollama's local API to write a Python script, then uses the MagicMtn CLI to run it in an isolated Canonical workspace.

## Before you start

[Install the MagicMtn CLI](../../cli-basics.md#install), [Ollama](https://ollama.com/download/mac), [Apple Container](https://github.com/apple/container#get-started), and Python 3 on your Mac. If you are signed in to MagicMtn Cloud, run `magic logout` to switch to your local installation.

## Start local inference

Open the Ollama app and download [Qwen3 4B](https://ollama.com/library/qwen3), the model used in this example:

```sh
ollama pull qwen3:4b
ollama list
```

Check that `qwen3:4b` appears in the list. Keep Ollama running while you work; requests go to its local API at `http://127.0.0.1:11434`.

## Prepare an Apple Container VM

Install the sandbox provider and start the Apple Container runtime:

```sh
magic provider install apple-container
container system start
```

Start the local MagicMtn server in a terminal you will keep open:

```sh
magic server start
```

In another terminal, prepare Canonical, MagicMtn's default machine image, and create a VM:

```sh
magic --local images prepare canonical --provider apple-container
export devbox=$(magic --local computer new --provider apple-container)
```

Keep using this terminal so `devbox` refers to the VM you created. Python is included in Canonical.

## Generate a script with Ollama

Save this as `generate.py` on your Mac:

```python
import json
from pathlib import Path
from urllib.request import Request, urlopen

payload = {
    "model": "qwen3:4b",
    "prompt": (
        "Write a Python script that prints the first 20 Fibonacci numbers, "
        "starting with 0 and 1. Return JSON with a single string field named "
        "code containing the complete script. Use only the standard library."
    ),
    "stream": False,
    "think": False,
    "format": {
        "type": "object",
        "properties": {"code": {"type": "string"}},
        "required": ["code"],
    },
    "options": {"temperature": 0},
}
request = Request(
    "http://127.0.0.1:11434/api/generate",
    data=json.dumps(payload).encode(),
    headers={"Content-Type": "application/json"},
)
with urlopen(request, timeout=300) as response:
    result = json.load(response)
code = json.loads(result["response"])["code"]
Path("fibonacci.py").write_text(code, encoding="utf-8")
print(code)
```

Run it to generate and inspect the script:

```sh
python3 generate.py
```

Ollama runs the model on your Mac. The generated Python is saved to `fibonacci.py`; it has not run yet. See Ollama's [generation API](https://docs.ollama.com/api/generate) and [structured outputs guide](https://docs.ollama.com/capabilities/structured-outputs) for the request format.

## Run the script inside the VM

Pass the generated script to Python inside your sandbox:

```sh
magic --local computer exec "$devbox" -- python3 -c "$(cat fibonacci.py)"
```

The JSON result includes `exit_code` and `stdout`. A successful run has exit code `0` and prints 20 numbers, starting with `0, 1, 1, 2, 3` and ending with `4181`. Inference stays on your Mac, while the generated code runs inside the VM.

## Clean up

Delete the VM and its workspace files, then stop the local MagicMtn server:

```sh
magic --local computer terminate "$devbox" --yes
magic server stop
```

Quit the Ollama app when you have finished using local inference. The downloaded model remains available for your next session.
