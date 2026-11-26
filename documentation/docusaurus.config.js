/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'MagicMtn Documentation',
  favicon: 'img/favicon.svg',
  tagline: 'Guides to using MagicMtn',
  url: process.env.DOCS_URL || 'https://magicmtn.ai',
  baseUrl: '/docs/',
  trailingSlash: true,
  onBrokenLinks: 'throw',
  plugins: [require.resolve('./plugins/link-policy')],
  markdown: { mermaid: true, hooks: { onBrokenMarkdownLinks: 'throw' } },
  themes: ['@docusaurus/theme-mermaid', [require.resolve('@easyops-cn/docusaurus-search-local'), {
    hashed: 'filename',
    docsRouteBasePath: '/',
    indexBlog: false,
    indexPages: false,
    searchBarPosition: 'left',
  }]],
  presets: [
    ['classic', {
      docs: {
        routeBasePath: '/',
        sidebarPath: require.resolve('./sidebars.js'),
        editUrl: 'https://github.com/useMagicMtn/magicmtn/blob/main/documentation/',
      },
      blog: false,
      pages: false,
      theme: { customCss: require.resolve('./src/css/custom.css') },
      sitemap: false,
    }],
  ],
  themeConfig: {
    docs: { sidebar: { hideable: true } },
    navbar: {
      logo: {
        alt: 'MagicMtn',
        src: 'img/wordmark.svg',
        srcDark: 'img/wordmark-dark.svg',
        href: 'https://magicmtn.ai/',
        target: '_blank',
        width: 160,
        height: 34,
      },
      items: [
        {type: 'docSidebar', sidebarId: 'home', label: 'Home', position: 'left'},
        {type: 'docSidebar', sidebarId: 'cloud', label: 'MagicMtn Cloud', position: 'left'},
        {type: 'docSidebar', sidebarId: 'openSource', label: 'Open Source', position: 'left'},
        {type: 'docSidebar', sidebarId: 'cli', label: 'CLI', position: 'left'},
      ],
    },
    colorMode: { respectPrefersColorScheme: true },
    footer: { copyright: '© 2026 MagicMtn, Inc. <a href="/terms" target="_blank" rel="noopener noreferrer">Terms</a>' },
  },
};
module.exports = config;
