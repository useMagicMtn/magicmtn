const {test} = require('node:test');
const assert = require('node:assert/strict');
const {pageMarkdown} = require('./llm-pages');

test('exports article content, code, links and tables without page controls', () => {
  const markdown = pageMarkdown(`
    <nav>Sidebar</nav><div class="theme-doc-markdown">
    <h1>Run Pi</h1><div data-doc-actions><button>Copy for LLM</button><a href="https://chatgpt.com/">ChatGPT</a></div>
    <h2>Start<a class="hash-link" href="#start">#</a></h2>
    <p>Use <a href="/docs/cli/">the CLI</a>.</p>
    <div class="language-sh"><pre><code>magic server start\necho '\u0060\u0060\u0060'</code></pre><button>Copy</button></div>
    <div role="tabpanel"><h3>macOS</h3><p>Mac instructions</p></div>
    <div role="tabpanel" hidden><h3>Linux</h3><p>Linux instructions</p></div>
    <table><thead><tr><th>Name</th><th>Value</th></tr></thead><tbody><tr><td>Model</td><td>Kimi</td></tr></tbody></table>
    </div><footer>Footer</footer>`, 'https://magicmtn.ai/docs/pi/');
  assert.match(markdown, /^Source: https:\/\/magicmtn.ai\/docs\/pi\//);
  assert.match(markdown, /# Run Pi/);
  assert.match(markdown, /\[the CLI\]\(https:\/\/magicmtn.ai\/docs\/cli\/\)/);
  assert.match(markdown, /````sh\nmagic server start\necho '```'\n````/);
  assert.match(markdown, /Mac instructions/);
  assert.match(markdown, /Linux instructions/);
  assert.match(markdown, /\| Model \| Kimi \|/);
  assert.doesNotMatch(markdown, /Sidebar|Footer|Copy|ChatGPT|<div|hash-link/);
});

test('does not export non-document pages', () => {
  assert.equal(pageMarkdown('<h1>Not found</h1>', 'https://magicmtn.ai/docs/404/'), null);
});

test('preserves highlighted code lines, indentation and blank lines', () => {
  const markdown = pageMarkdown('<div class="theme-doc-markdown"><div class="language-python"><pre><code><div class="token-line"><span>if True:</span><br></div><div class="token-line"><span>  print(1)</span><br></div><div class="token-line"><br></div><div class="token-line"><span>print(2)</span><br></div></code></pre></div></div>', 'https://magicmtn.ai/docs/example/');
  assert.match(markdown, /```python\nif True:\n  print\(1\)\n\nprint\(2\)\n```/);
});
