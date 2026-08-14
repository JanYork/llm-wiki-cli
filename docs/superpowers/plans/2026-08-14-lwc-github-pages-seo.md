# LWC GitHub Pages SEO Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the bilingual LWC GitHub Pages SEO and social-preview metadata, then deploy and publicly verify it.

**Architecture:** Keep the existing dependency-free static site. Extend the current Node contract test first, add two deterministic Memory Atlas social cards, complete localized metadata and truthful generic `SoftwareApplication` JSON-LD, and publish a two-URL sitemap. Reuse the existing Pages workflow unchanged.

**Tech Stack:** Static HTML, SVG/PNG, XML, Node.js standard library tests, `rsvg-convert`, GitHub Pages Actions, Playwright browser acceptance.

**Specification:** `docs/superpowers/specs/2026-08-14-lwc-github-pages-seo-design.md`

---

## File Map

- Modify `site/index.html`: English Open Graph, Twitter Card, JSON-LD, and visible Apache-2.0 license correction.
- Modify `site/zh-CN/index.html`: complete localized mirror metadata and visible Apache-2.0 license correction.
- Create `site/assets/social-card.svg`: editable English Memory Atlas card source.
- Create `site/assets/social-card-zh-CN.svg`: editable Chinese card source.
- Create `site/assets/social-card.png`: rendered 1200 x 630 English share asset.
- Create `site/assets/social-card-zh-CN.png`: rendered 1200 x 630 Chinese share asset.
- Create `site/sitemap.xml`: exactly two canonical localized URLs and reciprocal alternates.
- Modify `tests/site.mjs`: dependency-free SEO, JSON-LD, sitemap, and PNG regression contracts.

No Rust, CLI, npm installer, CSS, JavaScript behavior, or Pages workflow file changes are planned.

### Task 1: Social preview assets

**Files:**

- Modify: `tests/site.mjs`
- Create: `site/assets/social-card.svg`
- Create: `site/assets/social-card-zh-CN.svg`
- Create: `site/assets/social-card.png`
- Create: `site/assets/social-card-zh-CN.png`

- [ ] **Step 1: Write the failing asset test**

Add both PNG paths to the `files` map and add a PNG header reader using only
Node's `readFileSync`:

```js
function pngDimensions(path) {
  const png = readFileSync(resolve(root, path));
  assert.equal(png.subarray(1, 4).toString(), "PNG");
  assert.equal(png.subarray(12, 16).toString(), "IHDR");
  return { width: png.readUInt32BE(16), height: png.readUInt32BE(20) };
}

test("localized social cards are 1200 by 630 PNGs", () => {
  assert.deepEqual(pngDimensions(files.socialEn), { width: 1200, height: 630 });
  assert.deepEqual(pngDimensions(files.socialZh), { width: 1200, height: 630 });
});
```

- [ ] **Step 2: Run the test and confirm RED**

Run: `node --test tests/site.mjs`

Expected: FAIL because both social-card PNG files are missing.

- [ ] **Step 3: Create the minimum deterministic card artwork**

Create two 1200 x 630 SVG sources using the existing Atlas Paper, Deep Ink,
Atlas Blue, Evidence Ember, and Map Mist tokens. Each card contains only:

```text
LWC
Proactive Memory for AI Agents / AI Agent 的主动记忆系统
source evidence -> maintained Wiki -> next session
```

Render them without introducing a dependency:

```bash
rsvg-convert -w 1200 -h 630 site/assets/social-card.svg \
  -o site/assets/social-card.png
rsvg-convert -w 1200 -h 630 site/assets/social-card-zh-CN.svg \
  -o site/assets/social-card-zh-CN.png
```

- [ ] **Step 4: Run the test and confirm GREEN**

Run: `node --test tests/site.mjs`

Expected: 9 tests pass, 0 fail.

- [ ] **Step 5: Visually inspect both cards and commit**

Open both PNGs at original resolution and confirm exact text, no clipping, and
legibility at thumbnail scale.

```bash
git add tests/site.mjs site/assets/social-card*.svg site/assets/social-card*.png
git commit -m "feat(site): add localized social preview cards"
```

### Task 2: Open Graph and Twitter metadata

**Files:**

- Modify: `tests/site.mjs`
- Modify: `site/index.html`
- Modify: `site/zh-CN/index.html`

- [ ] **Step 1: Write the failing metadata test**

Add helpers that extract `<meta>` values by `property` or `name`. Assert exact
localized titles, descriptions, alt text, and absolute card URLs. Shared
assertions require:

```text
og:site_name = LWC
og:image:type = image/png
og:image:width = 1200
og:image:height = 630
twitter:card = summary_large_image
```

Also require the existing `og:locale` and `og:locale:alternate` values to remain
reciprocal.

- [ ] **Step 2: Run the test and confirm RED**

Run: `node --test tests/site.mjs`

Expected: FAIL on missing `og:image` and Twitter fields.

- [ ] **Step 3: Add only the specified metadata**

Add the exact fields from Section 3 of the SEO spec to both `<head>` elements.
Do not add `meta keywords`, explicit default robots directives, analytics, or
JavaScript-generated metadata.

- [ ] **Step 4: Run the test and confirm GREEN**

Run: `node --test tests/site.mjs`

Expected: 10 tests pass, 0 fail.

- [ ] **Step 5: Commit**

```bash
git add tests/site.mjs site/index.html site/zh-CN/index.html
git commit -m "feat(site): complete social metadata"
```

### Task 3: Truthful JSON-LD and localized sitemap

**Files:**

- Modify: `tests/site.mjs`
- Modify: `site/index.html`
- Modify: `site/zh-CN/index.html`
- Create: `site/sitemap.xml`

- [ ] **Step 1: Write failing JSON-LD and sitemap tests**

Parse the single `application/ld+json` block with `JSON.parse`. Require the
approved stable fields and require its localized description to equal the
page's meta description. Explicitly reject `aggregateRating`, `review`, and
`softwareVersion`.

Update the existing inline-script safeguard so the only script without `src`
may be `type="application/ld+json"`; executable inline scripts remain forbidden.

Read `site/sitemap.xml` and assert it contains exactly two `<url>` elements,
both canonical URLs, and reciprocal `en`, `zh-CN`, and `x-default` alternates.
Assert `site/robots.txt` does not exist.

Add a truthfulness regression requiring both locale pages to display
`Apache-2.0` and rejecting the stale `MIT` license text.

- [ ] **Step 2: Run the test and confirm RED**

Run: `node --test tests/site.mjs`

Expected: FAIL because JSON-LD and `site/sitemap.xml` are missing.

- [ ] **Step 3: Add the minimum JSON-LD and sitemap**

Embed the static `SoftwareApplication` object from Section 4 of the SEO spec in
each locale page. Correct the two visible license references on each page from
MIT to Apache-2.0 so the rendered content, repository license, and JSON-LD agree.
Create the UTF-8 sitemap with the `xhtml` namespace and no `lastmod`,
`changefreq`, or `priority` fields. Do not add a non-standard sitemap discovery
link to the HTML head.

- [ ] **Step 4: Run the test and confirm GREEN**

Run: `node --test tests/site.mjs`

Expected: 12 tests pass, 0 fail.

- [ ] **Step 5: Commit**

```bash
git add tests/site.mjs site/index.html site/zh-CN/index.html site/sitemap.xml
git commit -m "feat(site): add structured metadata and sitemap"
```

### Task 4: Scoped acceptance, integration, and deployment

**Files:** No new implementation files.

- [ ] **Step 1: Run the full affected local gate**

```bash
node --check tests/site.mjs
node --check site/assets/site.js
node --test tests/site.mjs
ruby -e 'require "yaml"; YAML.parse_file(".github/workflows/pages.yml"); YAML.parse_file(".github/workflows/ci.yml")'
git diff --check main...HEAD
```

Expected: all commands exit 0 with 12 site tests passing.

- [ ] **Step 2: Run real-browser acceptance**

Run `python3 -m http.server 4173 --directory site --bind 127.0.0.1` and inspect
`http://127.0.0.1:4173/` plus `/zh-CN/`. Confirm metadata and
JSON-LD in the rendered DOM, both card URLs load as 1200 x 630 PNGs, sitemap is
served as XML, existing copy/locale behavior still works, and the console has
zero warnings or errors.

- [ ] **Step 3: Integrate without touching user-owned files**

Fast-forward `main` to the reviewed feature branch. Preserve untracked
`.agents/`, `.superpowers/`, and root `AGENTS.md`. Push `main` and verify the
remote commit by readback.

- [ ] **Step 4: Monitor GitHub Actions**

Require the Pages workflow and complete CI workflow for the deployment commit
to finish with `success`.

- [ ] **Step 5: Public readback**

Verify both pages, `sitemap.xml`, and both PNG assets return HTTP 200 with the
correct MIME types. Compare public bytes or hashes to the reviewed local files,
re-run browser acceptance against the public URLs, and verify zero console
warnings/errors.

- [ ] **Step 6: Persist verified LWC knowledge and clean up**

Update one existing project Wiki page or one focused SEO deployment page with
the verified decisions and public evidence. Run project lint and fixed
retrieval checks. Remove the merged worktree and feature branch only after
public verification succeeds.
