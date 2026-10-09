import type { ReactNode } from "react";
import clsx from "clsx";
import Link from "@docusaurus/Link";
import useDocusaurusContext from "@docusaurus/useDocusaurusContext";
import Layout from "@theme/Layout";
import Heading from "@theme/Heading";
import Logo from "@theme/Logo";

import styles from "./index.module.css";

function HomepageHeader() {
  const { siteConfig } = useDocusaurusContext();
  return (
    <header className="tp-hero">
      <div className="container">
        <p className="tp-hero-eyebrow">throw.party</p>
        <Heading as="h1" className="tp-hero-title">
          {siteConfig.title}
        </Heading>
        <p className="tp-hero-lede">{siteConfig.tagline}</p>
        <div className={styles.buttons}>
          <Link className="button button--primary button--lg" to="/docs/user">
            👋 Get started
          </Link>
        </div>
      </div>
    </header>
  );
}

export default function Home(): ReactNode {
  const { siteConfig } = useDocusaurusContext();
  return (
    <Layout title={siteConfig.title} description={siteConfig.tagline}>
      <HomepageHeader />
      <main>
        <div className={clsx("container", styles.logoRow)}>
          <Logo imageClassName={styles.heroLogo} hideTitle />
        </div>
      </main>
    </Layout>
  );
}
