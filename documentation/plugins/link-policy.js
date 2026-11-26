const {readdir, readFile} = require('node:fs/promises');
const path = require('node:path');
const {load} = require('cheerio');

function violations(html, pageUrl) {
  const $ = load(html);
  const errors = [];
  $('a[href]').each((_, element) => {
    const link = $(element);
    const href = link.attr('href');
    const destination = new URL(href, pageUrl);
    const insideDocs = destination.origin === pageUrl.origin &&
      (destination.pathname === '/docs' || destination.pathname.startsWith('/docs/'));
    if (insideDocs) return;
    const rel = new Set((link.attr('rel') || '').split(/\s+/));
    if (link.attr('target') !== '_blank' || !rel.has('noopener') || !rel.has('noreferrer')) {
      errors.push(href);
    }
  });
  return errors;
}

module.exports = function linkPolicy() {
  return {
    name: 'documentation-link-policy',
    async postBuild({outDir, siteConfig}) {
      const errors = [];
      async function inspect(directory) {
        for (const entry of await readdir(directory, {withFileTypes: true})) {
          const file = path.join(directory, entry.name);
          if (entry.isDirectory()) {
            await inspect(file);
          } else if (entry.name.endsWith('.html')) {
            const relative = path.relative(outDir, file).split(path.sep).join('/');
            const pageUrl = new URL(siteConfig.baseUrl + relative, siteConfig.url);
            for (const href of violations(await readFile(file, 'utf8'), pageUrl)) {
              errors.push(`${relative}: ${href}`);
            }
          }
        }
      }
      await inspect(outDir);
      if (errors.length) {
        throw new Error('Links outside /docs must use target="_blank" and rel="noopener noreferrer":\n' + errors.join('\n'));
      }
    },
  };
};
module.exports.violations = violations;
