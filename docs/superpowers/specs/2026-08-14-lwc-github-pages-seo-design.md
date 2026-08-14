# LWC GitHub Pages SEO Design

**Status:** Approved direction, awaiting written-spec review

**Date:** 2026-08-14

**Target URLs:**

- `https://janyork.github.io/llm-wiki-cli/`
- `https://janyork.github.io/llm-wiki-cli/zh-CN/`

## 1. Purpose

Improve discovery and link previews for the existing bilingual LWC website
without adding a framework, analytics, marketing claims, or metadata that the
current GitHub Pages project URL cannot support correctly.

The current self-canonical URLs, reciprocal `hreflang`, localized titles,
descriptions, and basic Open Graph fields remain authoritative. This change
completes that foundation; it does not redesign page content.

## 2. Accepted Scope

The approved option is a complete but restrained SEO pass:

1. complete localized Open Graph and X/Twitter Card metadata;
2. add one 1200 x 630 PNG social card per locale using the existing Memory
   Atlas visual language;
3. add localized, truthful `SoftwareApplication` JSON-LD grounded in public
   repository facts, without claiming Google Software App rich-result
   eligibility;
4. publish a two-URL XML sitemap with reciprocal language alternates;
5. add static regression coverage and publicly verify the deployed result.

The change must not add `meta keywords`, fabricated ratings, usage numbers,
testimonials, FAQ markup, analytics, a sitemap generator, or a runtime package.

## 3. Metadata Contract

Each locale page keeps its existing localized `<title>`, description,
self-canonical URL, and reciprocal `hreflang` links.

Each page adds:

- `og:site_name` with value `LWC`;
- an absolute localized `og:image` URL;
- `og:image:secure_url`, `og:image:type`, `og:image:width`,
  `og:image:height`, and localized `og:image:alt`;
- `twitter:card` with value `summary_large_image`;
- localized `twitter:title`, `twitter:description`, `twitter:image`, and
  `twitter:image:alt`.

The English page references `assets/social-card.png`. The Chinese page
references `assets/social-card-zh-CN.png`. Both URLs are absolute in metadata
because social crawlers resolve absolute share assets most reliably.

The card artwork must use only verified product concepts already visible on
the website: source evidence, maintained Wiki memory, and the next session.
It must remain legible when downscaled and must not contain fake UI, customer
logos, usage metrics, or unsupported claims.

## 4. Structured Data Contract

Each page embeds one static JSON-LD block with `@type: SoftwareApplication`.
Its user-facing text and URL are localized; stable product facts are shared.
This is generic schema.org product metadata for machine-readable identity. It
is not expected to qualify for Google's Software App rich result because Google
currently requires a real `aggregateRating` or `review` in addition to the app
name and offer. LWC has no first-party rating or review evidence, so adding
either field would be misleading.

Required fields:

```text
@context              https://schema.org
@type                 SoftwareApplication
name                  LWC
url                   current locale canonical URL
description           current locale meta description
applicationCategory   DeveloperApplication
operatingSystem       macOS, Linux, Windows
isAccessibleForFree   true
offers                Offer, price 0, priceCurrency USD
license               repository Apache-2.0 license URL
downloadUrl           repository releases URL
sameAs                repository URL
```

The markup intentionally omits the current release version so a site-only
change cannot leave structured data stale after the next CLI release. It also
omits ratings and reviews because no such first-party evidence exists. Tests
validate truthful schema syntax and field consistency; they must not claim
Google rich-result eligibility.

`WebSite` site-name markup is excluded while LWC is hosted under the
`/llm-wiki-cli/` path. Google's site-name feature applies at the domain or
subdomain level; claiming `janyork.github.io` as the LWC website would be
incorrect. Reconsider it only after a dedicated custom domain exists.

## 5. Sitemap and Crawling Contract

Add `site/sitemap.xml` with the two self-canonical public URLs. Each `<url>`
contains reciprocal `xhtml:link` entries for `en`, `zh-CN`, and `x-default`.
Use absolute HTTPS URLs and UTF-8 XML. Omit `lastmod`, `changefreq`, and
`priority` because this static repository has no reliable automatic source for
those values and search engines do not need invented hints.

Do not add `site/robots.txt` for the current deployment. A robots file is only
effective at the host root (`https://janyork.github.io/robots.txt`), while this
repository can publish only
`https://janyork.github.io/llm-wiki-cli/robots.txt`. Shipping an ineffective
file would imply crawler control the project does not possess. Add a root
robots file only after moving to a dedicated custom domain or gaining control
of the user-site root.

## 6. Files and Dependencies

```text
site/
├── index.html                         # metadata + English JSON-LD
├── zh-CN/index.html                   # metadata + Chinese JSON-LD
├── sitemap.xml
└── assets/
    ├── social-card.png                # 1200 x 630 English card
    └── social-card-zh-CN.png          # 1200 x 630 Chinese card

tests/
└── site.mjs                           # extended static SEO contract
```

No production dependency is added. Existing static Pages packaging already
uploads the complete `site/` directory, so no workflow change is required
unless a failing test proves otherwise.

## 7. Verification

The dependency-free Node test must verify:

- both locale pages retain correct canonical and reciprocal `hreflang` links;
- required Open Graph and Twitter fields are present, localized, and absolute;
- both PNG assets exist and their IHDR dimensions are exactly 1200 x 630;
- both JSON-LD blocks parse and contain the exact approved facts;
- JSON-LD descriptions match their corresponding meta descriptions;
- the sitemap contains exactly the two canonical URLs and reciprocal language
  alternatives;
- no `meta keywords`, fake review/rating data, or project-path `robots.txt`
  appears.

Before deployment, run the site test, JavaScript syntax checks, workflow YAML
parse, and a real-browser head inspection for both locales. After deployment,
require:

1. successful Pages and repository CI workflows;
2. HTTP 200 and correct MIME types for both pages, the sitemap, and both PNGs;
3. public metadata and JSON-LD matching the reviewed local files;
4. public social-card dimensions and hashes matching the reviewed assets;
5. zero browser console errors or warnings on both locale pages.

## 8. Source Standards

- Google recommends concise localized titles and supports title signals from
  `<title>`, headings, Open Graph, and structured data:
  <https://developers.google.com/search/docs/appearance/title-link>
- Google recommends JSON-LD and requires structured data to represent visible,
  truthful page content:
  <https://developers.google.com/search/docs/appearance/structured-data/sd-policies>
- Google's `SoftwareApplication` rich-result documentation additionally
  requires a real rating or review; the generic schema in this design does not
  claim that eligibility:
  <https://developers.google.com/search/docs/appearance/structured-data/software-app>
- Google recommends root-level, absolute canonical URLs in XML sitemaps:
  <https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap>
- Open Graph requires title, type, URL, and image, and recommends image alt and
  dimensions:
  <https://ogp.me/>
