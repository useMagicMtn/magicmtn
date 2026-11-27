const {readdir, readFile, writeFile} = require('node:fs/promises');
const path = require('node:path');
const {load} = require('cheerio');
const TurndownService = require('turndown');
const {gfm} = require('turndown-plugin-gfm');

function pageMarkdown(html, pageUrl) {
  const $ = load(html);
  const article = $('.theme-doc-markdown').first();
  if (!article.length) return null;
  article.find('[data-doc-actions], .hash-link, button, .pi-recording, script, style').remove();
  // Copy the article rather than navigation, and keep every platform tab's content.
  article.find('a[href], img[src]').each((_, element) => {
    const attribute = element.tagName === 'a' ? 'href' : 'src';
    const value = $(element).attr(attribute);
    $(element).attr(attribute, new URL(value, pageUrl).href);
  });
  const markdown = new TurndownService({headingStyle: 'atx', codeBlockStyle: 'fenced', bulletListMarker: '-'});
  markdown.use(gfm);
  markdown.addRule('codeBlocks', {
    filter: 'pre',
    replacement(_, node) {
      const code = node.querySelector('code');
      const language = (node.closest('[class*="language-"]')?.className || '').match(/language-([\w-]+)/)?.[1] || '';
      const lines = node.querySelectorAll('.token-line');
      const text = (lines.length ? Array.from(lines, line => line.textContent).join('\n') : (code || node).textContent).replace(/\n$/, '');
      const longestFence = Math.max(2, ...Array.from(text.matchAll(/`+/g), match => match[0].length));
      const fence = '`'.repeat(longestFence + 1);
      return `\n\n${fence}${language}\n${text}\n${fence}\n\n`;
    },
  });
  return `Source: ${pageUrl}\n\n${markdown.turndown(article.html())}\n`;
}

module.exports = function llmPages() {
  return {
    name: 'documentation-llm-pages',
    async postBuild({outDir, siteConfig}) {
      async function generate(directory) {
        for (const entry of await readdir(directory, {withFileTypes: true})) {
          const file = path.join(directory, entry.name);
          if (entry.isDirectory()) await generate(file);
          else if (entry.name === 'index.html') {
            const relative = path.relative(outDir, directory).split(path.sep).join('/');
            const pageUrl = new URL(`${siteConfig.baseUrl}${relative}${relative ? '/' : ''}`, siteConfig.url);
            const markdown = pageMarkdown(await readFile(file, 'utf8'), pageUrl.href);
            // The root doc permalink is /docs/, whose sibling is served as /docs/index.md.
            if (markdown) await writeFile(relative ? `${directory}.md` : path.join(outDir, 'index.md'), markdown);
          }
        }
      }
      await generate(outDir);
    },
  };
};
module.exports.pageMarkdown = pageMarkdown;
