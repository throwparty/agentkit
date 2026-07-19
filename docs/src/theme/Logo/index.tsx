import React, {type ReactNode} from 'react';
import Link from '@docusaurus/Link';
import useBaseUrl from '@docusaurus/useBaseUrl';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';

interface LogoProps {
  className?: string;
  imageClassName?: string;
  titleClassName?: string;
  hideTitle?: boolean;
}

export default function Logo(props: LogoProps): ReactNode {
  const {className, imageClassName, titleClassName, hideTitle, ...restProps} = props;

  const {siteConfig} = useDocusaurusContext();
  const {navbar: {logo, title}} = siteConfig.themeConfig as {navbar: {logo?: {src?: string; alt?: string; href?: string}; title?: string}};

  const logoLink = useBaseUrl(logo?.href || '/');
  const logoSrc = useBaseUrl(logo?.src || '');

  return (
    <Link to={logoLink} className={className} {...restProps}>
      {logo && (
        <svg className={imageClassName} viewBox="0 0 1408 768" fill="currentColor" role="img" aria-label={logo.alt}>
          <use href={`${logoSrc}#logo-root`} />
        </svg>
      )}
      {!hideTitle && title != null && (
        <b className={titleClassName}>{title}</b>
      )}
    </Link>
  );
}
