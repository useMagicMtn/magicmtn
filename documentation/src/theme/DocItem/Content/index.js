import React from 'react';
import {MDXProvider} from '@mdx-js/react';
import {useDoc} from '@docusaurus/plugin-content-docs/client';
import Heading from '@theme/Heading';
import MDXContent from '@theme/MDXContent';
import DocActions from '@site/src/components/DocActions';

function Title(props) {
  return <><Heading as="h1" {...props} /><DocActions /></>;
}

const components = {h1: Title};

export default function DocItemContent({children}) {
  const {metadata, frontMatter, contentTitle} = useDoc();
  const syntheticTitle = !frontMatter.hide_title && contentTitle === undefined;
  return (
    <div className="theme-doc-markdown markdown">
      {syntheticTitle && <header><Title>{metadata.title}</Title></header>}
      {frontMatter.hide_title && contentTitle === undefined && <DocActions />}
      <MDXContent><MDXProvider components={components}>{children}</MDXProvider></MDXContent>
    </div>
  );
}
