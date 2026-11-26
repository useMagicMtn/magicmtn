import React from 'react';
import {useWindowSize} from '@docusaurus/theme-common';
import Content from '@theme-original/DocSidebar/Desktop/Content';
import SearchBar from '@theme-original/SearchBar';

export default function SidebarContent(props) {
  const windowSize = useWindowSize();
  return (
    <>
      {windowSize === 'desktop' && (
        <div className="docs-sidebar-search"><SearchBar /></div>
      )}
      <Content {...props} />
    </>
  );
}
