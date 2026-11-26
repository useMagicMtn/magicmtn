import React from 'react';

// App routes belong to the backend, outside Docusaurus's /docs/ router.
export default function AppLink({href, children}) {
  return <a href={new URL(href, 'https://control.magicmtn.ai').href} target="_blank" rel="noopener noreferrer">{children}</a>;
}
