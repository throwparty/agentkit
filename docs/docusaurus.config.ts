import path from "node:path";
import process from "node:process";
import type { Config } from "@docusaurus/types";
import type * as Preset from "@docusaurus/preset-classic";
import type { Configuration as WebpackConfiguration } from "webpack";
import { themes as prismThemes } from "prism-react-renderer";
import type { Configuration as WebpackConfig } from "webpack";
import type { ConfigureWebpackUtils } from "@docusaurus/types";

const config: Config = {
  title: "AgentKit",
  tagline: "A composable set of agentic software engineering tools.",
  favicon: "img/favicon.ico",

  future: {
    faster: process.env.DOCUSAURUS_FASTER === "true",
    v4: true,
  },

  url: "https://agentkit.throw.party",
  baseUrl: "/",

  organizationName: "throwparty",
  projectName: "agentkit",

  onBrokenLinks: "throw",

  i18n: {
    defaultLocale: "en",
    locales: ["en"],
  },

  markdown: {
    mermaid: true,
  },

  plugins: [
    "@cmfcmf/docusaurus-search-local",
    function pnpResolverPlugin() {
      return {
        name: "pnp-resolver",
        configureWebpack(
          _config: WebpackConfig,
          _isServer: boolean,
          utils: ConfigureWebpackUtils,
        ): WebpackConfiguration {
          const bundler = utils?.currentBundler?.name;
          const alias = {
            "vscode-jsonrpc/lib/common/events.js": path.resolve(
              __dirname,
              "src/webpack-shims/vscode-jsonrpc-events.js",
            ),
            "vscode-jsonrpc/lib/common/events": path.resolve(
              __dirname,
              "src/webpack-shims/vscode-jsonrpc-events.js",
            ),
            "vscode-jsonrpc/lib/common/cancellation.js": path.resolve(
              __dirname,
              "src/webpack-shims/vscode-jsonrpc-cancellation.js",
            ),
            "vscode-jsonrpc/lib/common/cancellation": path.resolve(
              __dirname,
              "src/webpack-shims/vscode-jsonrpc-cancellation.js",
            ),
          };
          if (bundler === "rspack") {
            const pnpManifest = path.resolve(__dirname, ".pnp.cjs");
            return {
              mergeStrategy: {
                "resolve.modules": "replace",
                "resolveLoader.modules": "replace",
                "resolve.byDependency": "replace",
              },
              resolve: {
                pnp: true,
                pnpManifest,
                alias,
                modules: [],
                byDependency: {
                  esm: {
                    pnp: true,
                    pnpManifest,
                    modules: [],
                  },
                  commonjs: {
                    pnp: true,
                    pnpManifest,
                    modules: [],
                  },
                },
              },
              resolveLoader: {
                pnp: true,
                pnpManifest,
                modules: [],
                byDependency: {
                  esm: {
                    pnp: true,
                    pnpManifest,
                    modules: [],
                  },
                  commonjs: {
                    pnp: true,
                    pnpManifest,
                    modules: [],
                  },
                },
              },
            };
          } else if (bundler === "webpack") {
            return {
              resolve: {
                alias,
              },
            };
          } else {
            throw new Error(`Unsupported bundler: ${bundler}`);
          }
        },
      };
    },
  ],

  presets: [
    [
      "classic",
      {
        docs: {
          sidebarPath: "./sidebars.ts",
          editUrl: "https://github.com/throwparty/agentkit/tree/main/docs/",
        },
        theme: {
          customCss: "./src/css/custom.css",
        },
      } satisfies Preset.Options,
    ],
  ],

  themes: ["@docusaurus/theme-mermaid"],

  themeConfig: {
    image: "img/docusaurus-social-card.jpg",
    colorMode: {
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: "AgentKit",
      logo: {
        alt: "Tools used to maintain the agents.",
        src: "img/icon.png",
      },
      items: [
        {
          type: "docSidebar",
          sidebarId: "gettingStartedSidebar",
          position: "left",
          label: "Getting started",
        },
        {
          href: "https://github.com/throwparty/agentkit",
          label: "GitHub",
          position: "right",
        },
      ],
    },
    footer: {
      style: "dark",
      links: [
        {
          title: "Docs",
          items: [
            {
              label: "Getting started",
              to: "/docs/intro",
            },
          ],
        },
        {
          title: "Community",
          items: [
            {
              label: "Discussions",
              href: "https://github.com/throwparty/agentkit/discussions",
            },
          ],
        },
        {
          title: "More",
          items: [
            {
              label: "GitHub",
              href: "https://github.com/throwparty/agentkit",
            },
            {
              label: "throw.party",
              href: "https://throw.party/",
            },
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} throw.party`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
