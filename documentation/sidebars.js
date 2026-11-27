module.exports = {
  "home": [
    "introduction",
    "faq"
  ],
  "cloud": [
    {
      "type": "link",
      "label": "FAQ",
      "href": "/docs/faq/"
    },
    {
      "type": "doc",
      "id": "cloud/index",
      "label": "MagicMtn Cloud"
    },
    {
      "type": "link",
      "label": "Install the CLI",
      "href": "/docs/cli-basics/#install",
      "key": "cloud-index-Install the CLI"
    },
    {
      "type": "category",
      "label": "Coding agents",
      "collapsed": false,
      "items": [
        {
          "type": "category",
          "label": "Pi",
          "link": {
            "type": "doc",
            "id": "coding-agents/pi"
          },
          "items": [
            "coding-agents/pi/inference-only"
          ]
        },
        "coding-agents/opencode"
      ],
      "key": "cloud-index-Coding agents"
    },
    "templates",
    {
      "type": "category",
      "label": "Sandbox backends",
      "link": {
        "type": "doc",
        "id": "coding-agents/sandbox-backends/index"
      },
      "items": [
        "coding-agents/sandbox-backends/local"
      ]
    },
    {
      "type": "category",
      "label": "Integrations",
      "link": {
        "type": "doc",
        "id": "integrations"
      },
      "items": [
        "integrations/github",
        "integrations/slack"
      ]
    },
    "mcp",
    "security",
    "coding-agents/config",
    "account"
  ],
  "openSource": [
    {
      "type": "link",
      "label": "FAQ",
      "href": "/docs/faq/"
    },
    {
      "type": "doc",
      "id": "open-source/index",
      "label": "Open source MagicMtn"
    },
    {
      "type": "link",
      "label": "Install the CLI",
      "href": "/docs/cli-basics/#install",
      "key": "open-source-index-Install the CLI"
    },
    {
      "type": "link",
      "label": "Start the server",
      "href": "/docs/cli-reference/server/"
    },
    {
      "type": "category",
      "label": "Guides",
      "link": { "type": "doc", "id": "open-source/guides/index" },
      "items": ["open-source/guides/modal-firecracker"]
    },
    "web-ui",
    "providers",
    {
      "type": "category",
      "label": "Coding agents",
      "collapsed": false,
      "items": [
        "coding-agents/codex",
        "coding-agents/claude-code"
      ],
      "key": "open-source-index-Coding agents"
    }
  ],
  "cli": [
    {
      "type": "link",
      "label": "FAQ",
      "href": "/docs/faq/"
    },
    "cli-basics",
    {
      "type": "category",
      "label": "Command reference",
      "collapsed": false,
      "link": {
        "type": "doc",
        "id": "cli-reference"
      },
      "items": [
        "cli-reference/login",
        "cli-reference/pi",
        "cli-reference/computer",
        "cli-reference/server",
        "cli-reference/images",
        "cli-reference/provider",
        "cli-reference/agents",
        "cli-reference/mcp",
        "cli-reference/update",
        "cli-reference/whoami",
        "cli-reference/help",
        "cli-reference/completion",
        "cli-reference/logout",
        "cli-reference/uninstall"
      ]
    }
  ]
};
