import React, {type ReactNode} from 'react';
import Link from '@docusaurus/Link';
import useBaseUrl from '@docusaurus/useBaseUrl';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';

export default function NavbarLogo(): ReactNode {
  const {siteConfig} = useDocusaurusContext();
  const {navbar: {title}} = siteConfig.themeConfig as {navbar: {title?: string}};

  return (
    <Link to={useBaseUrl('/')} className="navbar__brand">
      {title != null && (
        <b className="navbar__title text--truncate">{title}</b>
      )}
    </Link>
  );
}
