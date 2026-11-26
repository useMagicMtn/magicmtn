---
title: Agents
---

import AppLink from '@site/src/components/AppLink';

Define an agent in Python, install it with the MagicMtn CLI, and manage its versions on the Agents page. MagicMtn saves the agent's source code and versions in your account or organization workspace.

## Define the agent

`./error-investigator` is a project directory. For this example, it contains:

```text
error-investigator/
├── agent.py          # Agent definition shown below
└── pyproject.toml    # Python dependencies, including LangChain
```

Put this decorated factory in `error-investigator/agent.py`:

```python title="agent.py"
from langchain.agents import create_agent
import magicmtn.agents as magicmtn

SYSTEM_PROMPT = (
    "Investigate errors, cite evidence, and separate facts from hypotheses. "
    "Compare Sentry issues and logs with Grafana metrics and logs. "
    "Summarize the impact, likely cause, and next steps."
)


@magicmtn.agent(
    sdk="langchain",
    default_model="kimi-k3",
    mcps=[
        magicmtn.MCP("sentry", tools=["search_issues", "search_logs"]),
        magicmtn.MCP("grafana"),  # Include all tools from the Grafana MCP.
    ],
)
def agent(ctx: magicmtn.AgentContext):
    return create_agent(
        name="Error Investigator",
        model=ctx.model,
        tools=ctx.tools,
        system_prompt=SYSTEM_PROMPT,
    )
```

The factory receives MagicMtn's selected model and the authorized tools from the declared MCPs. MagicMtn uses the name on the LangChain agent. The source declares which MCPs and tools the agent needs without containing account credentials or connection IDs.

The decorator keeps the factory callable for local tests. Add any supporting Python modules to the same project directory and list their third-party dependencies in `pyproject.toml`.

Add the dependencies to `error-investigator/pyproject.toml`:

```toml title="pyproject.toml"
[project]
name = "error-investigator"
version = "0.1.0"
requires-python = ">=3.11"
dependencies = ["magicmtn-agents[langchain]>=0.1,<1"]

[build-system]
requires = ["setuptools>=68"]
build-backend = "setuptools.build_meta"

[tool.setuptools]
py-modules = ["agent"]
```

## Install the agent

From the directory containing `error-investigator/`, sign in to the MagicMtn CLI and prepare the project's Python environment. Install the SDK from your standalone `sdk-python` checkout together with the project:

```sh
python3 -m venv error-investigator/.venv
error-investigator/.venv/bin/pip install -e "$HOME/src/sdk-python" -e ./error-investigator
```

Then install the agent:

```sh
magic login
magic agents install ./error-investigator
```

The CLI asks you to confirm whether to create an agent or update one with the same name in your workspace.

Installation runs the factory locally to read the agent name. Keep the factory free of model calls or external requests. The CLI uses the project’s `.venv/bin/python` when available, or `python3` from your shell.

Installation saves an immutable version and shows the agent on <AppLink href="/run">Run on MagicMtn</AppLink>. Reinstalling changed source adds a version under the same name. Agent names are unique within each account or organization workspace.

Open the agent on <AppLink href="/run">Run on MagicMtn</AppLink> to inspect the installed version, source files, model, and MCP declarations. Previous versions remain available.
