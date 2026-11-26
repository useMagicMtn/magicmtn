import React from 'react';
import {useWindowSize} from '@docusaurus/theme-common';
import SearchBar from '@theme-original/SearchBar';

// Desktop search lives above the docs sidebar; keep it in the mobile navbar.
export default function MobileSearchBar(props) {
  const windowSize = useWindowSize();
  return windowSize === 'mobile' ? <SearchBar {...props} /> : null;
}
