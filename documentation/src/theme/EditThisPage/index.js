import React from 'react';
import Link from '@docusaurus/Link';
import Translate from '@docusaurus/Translate';

export default function EditThisPage({editUrl}) {
  return (
    <Link
      to={editUrl}
      className="theme-edit-this-page docs-source-link"
      target="_blank"
      rel="noopener noreferrer">
      <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
        <path d="M9 19c-4.3 1.4-4.3-2.5-6-3m12 6v-3.9a3.4 3.4 0 0 0-.9-2.7c3-.3 6.2-1.5 6.2-6.9a5.4 5.4 0 0 0-1.5-3.7 5 5 0 0 0-.1-3.7s-1.2-.4-3.9 1.4a13.4 13.4 0 0 0-7 0C5.1-.3 3.9.1 3.9.1a5 5 0 0 0-.1 3.7A5.4 5.4 0 0 0 2.3 7.5c0 5.4 3.2 6.6 6.2 6.9a3.4 3.4 0 0 0-.9 2.7V22" transform="translate(1 1) scale(.9)" />
      </svg>
      <span>
        <Translate id="theme.common.editThisPage">View this page on GitHub</Translate>
      </span>
    </Link>
  );
}
