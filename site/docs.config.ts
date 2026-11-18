/**
 * Everything that differs between the documentation sites of the organisation.
 *
 * The values below describe Hatchery. The rest of the site follows
 * aiviarium/docs-base.
 */

export interface NavLink {
  text: string;
  url: string;
  /** Opens outside the site. Links that start with http are external anyway. */
  external?: boolean;
}

export interface Feature {
  title: string;
  description: string;
  url?: string;
}

/** One image in a dark and a light version, as paths under public/. */
export interface Themed {
  dark: string;
  light: string;
}

export interface DocsConfig {
  /** The name in title case, as it appears in prose: "Hatchery". */
  name: string;
  /** The lowercase name used in URLs, packages and commands: "hatchery". */
  slug: string;
  /** One sentence, shown under the wordmark and in the page metadata. */
  description: string;
  /** The area after "Aiviarium /" in the small label above the wordmark. */
  area: string;
  /** The project colour. `light` is the darker tone that passes 4.5:1 on white. */
  colour: Themed;
  /** The repository, linked from the header and the "Edit on GitHub" links. */
  repo: string;
  /** The folder of the docs inside the repository, for "Edit on GitHub". */
  repoDocsPath: string;
  /**
   * The badge beside the name in the header, such as the version. Drawn like the
   * organisation's badges: the label on a dark field, the value on the project
   * colour. Leave it out to hide it.
   */
  chip?: { label: string; value: string };
  /**
   * Badges from the organisation's badge endpoint, shown on the landing page and
   * in the README.
   */
  badges: {
    /** The badge endpoint. */
    host: string;
    /** The file name of a GitHub Actions workflow, for a CI badge. */
    ci?: string;
    /** Show the latest GitHub release. */
    release?: boolean;
  };
  install: {
    label: string;
    command: string;
  };
  /** The two buttons under the description. The first one is outlined in the project colour. */
  actions: [NavLink, NavLink];
  assets: {
    /** The detailed icon, shown at 128 px on the landing page. */
    icon: Themed;
    /** The `-icon` file, drawn for 24 to 48 px, used in the header lockup. */
    navIcon: Themed;
    /** The `-16` file, drawn on the pixel grid for 16 px: the favicon. */
    favicon: Themed;
    /** The faceted wordmark, for word sizes of 24 px and above. */
    wordmark: Themed;
    /** The plain wordmark in `currentColor` (`-mono`), for the 18 px header lockup. */
    wordmarkSolid: string;
  };
  /** Links in the header, after the name. */
  nav: NavLink[];
  /** Cards on the landing page. Leave the list empty to hide them. */
  features: Feature[];
}

const config: DocsConfig = {
  name: 'Hatchery',
  slug: 'hatchery',
  description: 'Packaging and distribution for plugins and modular software.',
  area: 'Plugin distribution',
  colour: { dark: '#E879F9', light: '#A21CAF' },
  repo: 'https://github.com/aiviarium/hatchery',
  repoDocsPath: 'docs',
  badges: {
    host: 'https://aiviarium-badges.helight.workers.dev',
    ci: 'ci.yml',
  },
  install: {
    label: 'Install from source',
    command: 'cargo install --locked --git https://github.com/aiviarium/hatchery hatchery-cli',
  },
  actions: [
    { text: 'Get started', url: '/docs' },
    { text: 'Command line', url: '/docs/cli' },
  ],
  assets: {
    icon: { dark: '/brand/hatchery.svg', light: '/brand/hatchery-light.svg' },
    navIcon: { dark: '/brand/hatchery-icon.svg', light: '/brand/hatchery-icon-light.svg' },
    favicon: { dark: '/brand/hatchery-16.svg', light: '/brand/hatchery-16-light.svg' },
    wordmark: {
      dark: '/brand/hatchery-wordmark.svg',
      light: '/brand/hatchery-wordmark-light.svg',
    },
    wordmarkSolid: '/brand/hatchery-wordmark-mono.svg',
  },
  nav: [
    { text: 'Guide', url: '/docs' },
    { text: 'Formats', url: '/docs/package-kdl' },
    { text: 'Command line', url: '/docs/cli' },
    { text: 'Rust API', url: '/docs/api' },
  ],
  features: [
    {
      title: 'Eggs and targets',
      description: 'One package holds files for several tools, and each tool installs only its own part.',
      url: '/docs/concepts#eggs',
    },
    {
      title: 'Git registries',
      description: 'A git repository with a packages.kdl is a registry, with no server to run.',
      url: '/docs/registries#git-repository-as-a-registry',
    },
    {
      title: 'Signing and minimum age',
      description: 'Ed25519 signatures on releases, and new versions wait a set time before they install.',
      url: '/docs/concepts#minimum-release-age',
    },
    {
      title: 'Playground',
      description: 'A folder with example packages and a registry, to try the command in.',
      url: '/docs/playground',
    },
  ],
};

export default config;
