# LWC GitHub Pages Website Design

**Status:** Approved design, awaiting implementation planning

**Date:** 2026-08-14

**Target URL:** `https://janyork.github.io/llm-wiki-cli/`

## 1. Purpose

Build a public LWC website that helps a visitor understand the product and
start using it through an AI Agent as quickly as possible.

The website is not limited to coding Agents. It serves people who use or build
AI Agents for coding, research, operations, knowledge work, and general tasks.
Its single primary conversion is:

> **Install with your Agent**

The website is a concise product entrance. The existing GitHub Wiki remains the
complete documentation system, and GitHub remains the source for releases,
issues, security policy, and source code.

## 2. Accepted Product Decisions

<!-- markdownlint-disable MD013 -->

| Decision | Accepted choice |
| --- | --- |
| Primary audience | Users and builders of any AI Agent that needs cross-session memory |
| Primary homepage job | Explain LWC quickly and lead to Agent-driven installation |
| Primary CTA | `Install with your Agent` |
| Language model | English is the normative copy; both locales are independently canonical pages |
| Language behavior | English default; explicit language switch; remember the visitor's explicit choice |
| Initial scope | One English landing page and one Chinese mirror |
| Documentation | Link to the GitHub Wiki; do not duplicate it |
| Implementation | Plain static HTML, shared CSS, and minimal native JavaScript |
| Hosting | GitHub Pages at the repository project URL |
| Visual direction | Memory Atlas |

<!-- markdownlint-enable MD013 -->

## 3. Content Architecture

The English page lives at `/llm-wiki-cli/`. The Chinese mirror lives at
`/llm-wiki-cli/zh-CN/`. Both pages have the same ordered sections and semantic
IDs.

"English canonical" describes which wording governs content maintenance. It
does not mean the Chinese URL points its HTML canonical metadata to English.
Each locale uses a self-referencing `rel="canonical"`, both pages declare
reciprocal `hreflang="en"` and `hreflang="zh-CN"` alternates, and
`hreflang="x-default"` points to the English root.

```text
Global navigation
  LWC
  How it works
  Capabilities
  Wiki
  GitHub
  Language switch

Hero
  Product thesis
  One-sentence explanation
  Install with your Agent
  Explore the Wiki
  Memory Relay visualization

Trust strip
  Agent-operated
  Source-grounded
  Explicitly scoped
  Auditable

How it works
  Add evidence
  Maintain the Wiki
  Recall with context

Capability layers
  Persistent Wiki memory
  Document and Word graphs
  Optional CodeGraph

Agent installation
  Complete copyable setup prompt
  Copy feedback
  Supported AgentTarget examples
  Manual installation link

Resource navigation
  Wiki
  Releases
  GitHub
  Issues
  Security
  License
```

The page must use real LWC terminology and current public links. It must not
invent usage statistics, customer logos, testimonials, benchmark claims, or an
exhaustive list of every supported Agent.

## 4. Installation Experience

The primary CTA scrolls to the Agent installation section. Both locale pages
present the exact English setup prompt from the fenced block in the project
README, not a shortened or translated prompt that silently drops safety,
commands, or verification requirements. The Chinese page translates the
surrounding explanation, while the copyable Agent input remains byte-for-byte
equivalent after normalizing line endings.

The prompt asks the Agent to:

1. use the LWC repository and canonical `using-lwc` Skill as sources of truth;
2. install a checksum-verified global CLI when needed;
3. run the idempotent AgentTarget installer;
4. report installed, unsupported, and user-managed integration surfaces;
5. ask before initializing a project Wiki or enabling optional graph
   capabilities;
6. finish with concrete verification evidence.

The copy button uses the Clipboard API when available and a selection-based
fallback otherwise. Its visible action and feedback use the same vocabulary:
`Copy prompt` becomes `Copied`. Failure leaves the prompt selectable and shows
`Select and copy the prompt manually.`

Manual npm, Homebrew, Cargo, and shell installation remain secondary links to
the README or Wiki. They do not compete with the primary Agent CTA.

## 5. Visual System: Memory Atlas

### 5.1 Thesis

The page is a quiet atlas for an Agent's continuity. Evidence enters, maintained
memory forms, and a future session begins with context instead of zero.

The visual system comes from LWC's subject matter: sources, Wiki pages,
provenance, graphs, and cross-session continuity. It must not use a generic
purple gradient, floating glass cards, decorative graph nodes, or fabricated
AI imagery.

### 5.2 Tokens

| Role | Token |
| --- | --- |
| Atlas Paper | `#EAF0FF` |
| Deep Ink | `#0B1739` |
| Atlas Blue | `#2452D6` |
| Evidence Ember | `#FF6848` |
| Map Mist | `#B8C6EE` |

Typography roles:

- **Literata:** restrained display headings, chosen for durable long-form
  knowledge rather than decoration.
- **Atkinson Hyperlegible Next:** body copy, chosen for accessibility and
  clarity.
- **Commit Mono:** commands, provenance labels, graph states, and utility text.

All fonts require practical local fallbacks. Font loading failure must not move
or hide core content.

### 5.3 Signature Element

The Memory Relay connects three real states:

```text
source evidence -> maintained Wiki -> next session
```

Secondary nodes may represent a tag, document graph, Word Graph, or CodeGraph,
but every visible node must encode a real LWC concept. The relay animates once
as an orchestrated page-load moment. It then becomes quiet. The visualization
is decorative to assistive technology and cannot be required to understand the
page.

### 5.4 Layout

Desktop uses a split hero: thesis and CTA on the left, Memory Relay on the
right. The rest of the page alternates light explanatory sections with one Deep
Ink capability layer. Atlas labels and hairline rules encode real section and
state information.

Mobile collapses to a single column. The primary CTA remains visible without
horizontal scrolling. Navigation becomes a compact disclosure or focused set
of essential links. No content or action depends on hover.

## 6. Static Architecture

```text
site/
├── index.html
├── zh-CN/
│   └── index.html
└── assets/
    ├── site.css
    └── site.js

tests/
└── site.mjs

.github/workflows/
└── pages.yml
```

No framework, runtime package, client router, analytics SDK, CMS, or static-site
generator is required for the first release.

The two HTML documents intentionally duplicate translated prose while sharing
styles and behavior. This keeps each language independently indexable and
usable without JavaScript. A small test prevents structural drift between the
mirrors.

All internal assets use relative URLs so the site works under
`/llm-wiki-cli/`, in a local static server, and after a future custom-domain
move. Each page implements the self-canonical and reciprocal `hreflang`
contract defined in Section 3.

## 7. Browser Behavior

JavaScript has only three responsibilities:

1. copy the Agent installation prompt with a safe fallback;
2. store an explicit language choice in local storage;
3. apply small progressive enhancements to navigation or the Memory Relay.

The complete page, links, prompt, and language switch work when JavaScript is
disabled. The site never redirects from browser-language detection. Clicking a
language switch stores `en` or `zh-CN` under `lwc-site-locale` before normal
navigation. A later visit to the bare English project root redirects to
`/llm-wiki-cli/zh-CN/` only when the stored value is `zh-CN`. An explicit visit
to `/zh-CN/` always stays Chinese; its English switch stores `en` before
navigating to the root. Missing, blocked, or invalid storage leaves the current
URL unchanged. Crawlers and no-JavaScript visitors therefore receive the
requested URL without locale inference.

Motion respects `prefers-reduced-motion: reduce`. Keyboard focus is always
visible. Semantic landmarks, heading order, link names, and button names must
remain useful without styling.

## 8. Deployment

GitHub Pages uses a custom Actions workflow with the repository's `site/`
directory as the deployment artifact.

Before the first deployment, a repository administrator must set
**Settings → Pages → Build and deployment → Source** to **GitHub Actions** and
confirm that the `github-pages` environment can deploy from `main`. This
configuration is a release preflight item, not an assumption hidden inside the
workflow.

The workflow:

1. runs on pushes to `main` that affect the site, its test, or the Pages
   workflow, and supports `workflow_dispatch`;
2. checks out the repository;
3. runs the built-in Node test without installing site dependencies;
4. configures Pages with `actions/configure-pages`;
5. uploads only `site/` with `actions/upload-pages-artifact`;
6. deploys with `actions/deploy-pages` to the `github-pages` environment.

The workflow declares permissions explicitly. The verification/upload job has
`contents: read` for checkout and no write permission. The deploy job has only
the permissions required by the selected current Pages Action versions,
including `pages: write` and `id-token: write`; it does not inherit repository
write permissions. The implementation must verify the current official Action
major versions and their documented permissions before pinning the workflow.

References:

- [Using custom workflows with GitHub Pages](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages)
- [`actions/deploy-pages`](https://github.com/actions/deploy-pages)

## 9. Error and Degradation Behavior

The site has no application backend and no runtime data dependency.

<!-- markdownlint-disable MD013 -->

| Failure | Required behavior |
| --- | --- |
| JavaScript unavailable | Content, navigation, language switch, prompt, and external links remain usable |
| Clipboard denied | Prompt remains selectable and manual-copy guidance appears |
| Web font unavailable | Fallback stack preserves hierarchy and layout |
| Animation unsupported or reduced | Memory Relay renders as a complete static diagram |
| External destination unavailable | Link remains a normal destination; the website does not fake success |
| Local storage unavailable | Language switch still navigates; the choice simply is not remembered |

<!-- markdownlint-enable MD013 -->

No error message apologizes vaguely. It states what failed and the available
next action.

## 10. Verification

### 10.1 Automated checks

`node --test tests/site.mjs` verifies:

- both language pages and shared assets exist;
- both pages use the same ordered section IDs and heading levels;
- language switches are reciprocal;
- canonical and `hreflang` metadata target the correct Pages URLs;
- required resource links are present and HTTPS;
- the fenced setup prompt extracted from `README.md` is byte-for-byte equal,
  after line-ending normalization, to the copyable prompt in both locale pages;
- no private filesystem path, placeholder URL, or unsupported claim appears;
- asset references resolve from both the root and `/zh-CN/` pages;
- buttons and navigation controls have accessible names.

The repository's existing CI invokes this test so site correctness is checked
before deployment.

### 10.2 Real browser acceptance

Before publication, verify the actual static artifact in a browser at desktop
and mobile widths:

- layout at approximately 1440 px, 768 px, and 360 px;
- English and Chinese language navigation;
- copy success and fallback behavior;
- keyboard-only navigation and visible focus;
- reduced-motion rendering;
- no horizontal overflow;
- no console errors;
- no broken local assets;
- GitHub, Wiki, release, issue, security, and license links.

After deployment, read back the public Pages URL, its Chinese mirror, MIME
types, canonical metadata, and one complete Agent prompt. A successful workflow
alone is not publication proof.

## 11. Deliberate Exclusions

The first release does not include:

- a documentation mirror or Wiki renderer;
- blog, changelog, roadmap, search, account system, analytics, telemetry, or
  newsletter;
- a client framework or package manager for site runtime;
- a custom domain;
- browser-language or otherwise unchosen locale redirection; the explicit
  stored-choice behavior in Section 7 remains included;
- invented social proof or product metrics;
- an interactive graph whose complexity exceeds its explanatory value.

Add these only when a concrete product need appears. The static architecture is
not a promise to remain framework-free forever; it is the smallest design that
fully serves the approved first release.

## 12. Acceptance Criteria

The design is implemented when:

1. the English and Chinese single-page sites match the approved Memory Atlas
   direction;
2. the primary CTA exposes the complete Agent-driven setup prompt;
3. the page is responsive, keyboard accessible, and reduced-motion safe;
4. both pages work without JavaScript;
5. all automated and browser checks pass;
6. GitHub Pages deploys from `main` to
   `https://janyork.github.io/llm-wiki-cli/`;
7. the Pages source and `github-pages` environment preflight is recorded before
   the first deployment;
8. the public English and Chinese URLs are read back successfully;
9. the existing GitHub Wiki remains the authoritative detailed documentation.
