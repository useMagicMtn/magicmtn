const {test} = require('node:test');
const assert = require('node:assert/strict');
const {mkdtemp, mkdir, writeFile, rm} = require('node:fs/promises');
const {tmpdir} = require('node:os');
const path = require('node:path');
const linkPolicy = require('./link-policy');
const {violations} = require('./link-policy');

const page = new URL('https://example.com/docs/coding-agents/pi/');

test('docs links, fragments, and relative guide links stay in the current tab', () => {
  for (const href of ['/docs', '/docs/', '/docs/account/', '#install', '?q=pi', '../../account/', 'https://example.com/docs/faq/']) {
    assert.deepEqual(violations(`<a href="${href}">Guide</a>`, page), [], href);
  }
});

test('all destinations outside docs require a secure new tab', () => {
  for (const href of ['/', '/agents', '/terms', '/docs-other', '../../../account', 'https://other.example/docs/', '//other.example/docs/', 'mailto:help@example.com']) {
    for (const attrs of ['', 'target="_self" rel="noopener noreferrer"', 'target="_blank"', 'target="_blank" rel="noopener"']) {
      assert.deepEqual(violations(`<a href="${href}" ${attrs}>App</a>`, page), [href]);
    }
    assert.deepEqual(violations(`<a href="${href}" target="_blank" rel="noopener noreferrer">App</a>`, page), []);
  }
});

test('build hook rejects violations in nested pages and accepts corrected links', async (t) => {
  const outDir = await mkdtemp(path.join(tmpdir(), 'docs-link-policy-'));
  t.after(() => rm(outDir, {recursive: true, force: true}));
  await mkdir(path.join(outDir, 'account'));
  const file = path.join(outDir, 'account/index.html');
  const props = {outDir, siteConfig: {url: 'https://example.com', baseUrl: '/docs/'}};
  await writeFile(file, '<a href="/billing">Billing</a>');
  await assert.rejects(linkPolicy().postBuild(props), /account\/index.html: \/billing/);
  await writeFile(file, '<a href="/billing" target="_blank" rel="noopener noreferrer">Billing</a>');
  await linkPolicy().postBuild(props);
});
