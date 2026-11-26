import React from 'react';
import CopyButton from '@theme-original/CodeBlock/Buttons/CopyButton';

export default function VisibleCopyButton({className, ...props}) {
  return <CopyButton {...props} className={`docs-copy-button ${className || ''}`} />;
}
