import assert from "node:assert/strict";
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import test from "node:test";

const root = resolve(import.meta.dirname, "..");
const files = {
  en: "site/index.html",
  zh: "site/zh-CN/index.html",
  css: "site/assets/site.css",
  js: "site/assets/site.js",
  workflow: ".github/workflows/pages.yml",
  ci: ".github/workflows/ci.yml",
};

const read = (path) => readFileSync(resolve(root, path), "utf8").replaceAll("\r\n", "\n");
const page = (locale) => read(files[locale]);
const attribute = (tag, name) => tag.match(new RegExp(`\\b${name}="([^"]+)"`))?.[1];

function setupPrompt() {
  const section = read("README.md").split("## Recommended: Ask Your Agent to Set Up LWC\n")[1];
  assert.ok(section, "README setup section is missing");
  const match = section.match(/```text\n([\s\S]*?)\n```/);
  assert.ok(match, "README setup prompt is missing");
  return match[1];
}

function embeddedPrompt(html) {
  const match = html.match(/<code data-agent-prompt>([\s\S]*?)<\/code>/);
  assert.ok(match, "copyable Agent setup prompt is missing");
  return match[1]
    .replaceAll("&amp;", "&")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">");
}

function sectionContract(html) {
  const ids = [...html.matchAll(/<(?:header|section)\b[^>]*\bid="([^"]+)"/g)].map((match) => match[1]);
  const levels = ids.map((id) => {
    const start = html.indexOf(`id="${id}"`);
    const end = html.indexOf("</section>", start);
    const match = html.slice(start, end === -1 ? undefined : end).match(/<h([1-3])\b/);
    assert.ok(match, `section ${id} is missing a heading`);
    return Number(match[1]);
  });
  return { ids, levels };
}

test("the bilingual site and shared deployment files exist", () => {
  for (const path of Object.values(files)) assert.ok(existsSync(resolve(root, path)), `${path} is missing`);
});

test("locale pages share structure and independently canonical metadata", () => {
  const en = page("en");
  const zh = page("zh");
  const expected = {
    ids: ["hero", "trust", "how-it-works", "capabilities", "install", "resources"],
    levels: [1, 2, 2, 2, 2, 2],
  };

  assert.deepEqual(sectionContract(en), expected);
  assert.deepEqual(sectionContract(zh), expected);
  assert.match(en, /<html lang="en">/);
  assert.match(zh, /<html lang="zh-CN">/);
  assert.match(en, /rel="canonical" href="https:\/\/janyork\.github\.io\/llm-wiki-cli\/"/);
  assert.match(zh, /rel="canonical" href="https:\/\/janyork\.github\.io\/llm-wiki-cli\/zh-CN\/"/);

  for (const html of [en, zh]) {
    assert.match(html, /hreflang="en" href="https:\/\/janyork\.github\.io\/llm-wiki-cli\/"/);
    assert.match(html, /hreflang="zh-CN" href="https:\/\/janyork\.github\.io\/llm-wiki-cli\/zh-CN\/"/);
    assert.match(html, /hreflang="x-default" href="https:\/\/janyork\.github\.io\/llm-wiki-cli\/"/);
  }

  assert.match(en, /<a[^>]+data-locale="zh-CN"[^>]+href="zh-CN\/"/);
  assert.match(zh, /<a[^>]+data-locale="en"[^>]+href="\.\.\/"/);
});

test("both pages contain the exact normative README Agent prompt", () => {
  const expected = setupPrompt();
  assert.equal(embeddedPrompt(page("en")), expected);
  assert.equal(embeddedPrompt(page("zh")), expected);
});

test("assets resolve from both locale directories and controls stay accessible", () => {
  for (const locale of ["en", "zh"]) {
    const path = files[locale];
    const html = page(locale);
    const tags = [
      ...html.matchAll(/<link\b[^>]*rel="stylesheet"[^>]*>/g),
      ...html.matchAll(/<link\b[^>]*rel="icon"[^>]*>/g),
      ...html.matchAll(/<script\b[^>]*src="[^"]+"[^>]*>/g),
    ].map((match) => match[0]);

    for (const tag of tags) {
      const reference = attribute(tag, tag.startsWith("<link") ? "href" : "src");
      if (!reference || reference.startsWith("https://")) continue;
      assert.ok(existsSync(resolve(root, dirname(path), reference)), `${path}: broken asset ${reference}`);
    }

    assert.match(html, /<button[^>]+data-copy-target="agent-setup-prompt"[^>]*>\s*[^<\s][^<]*<\/button>/);
    assert.match(html, /<summary>\s*[^<\s][^<]*<\/summary>/);
    assert.match(html, /<nav[^>]+aria-label="[^"]+"/);
    assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)/);
    assert.doesNotMatch(html, /\sstyle="/);
  }
});

test("public links are real HTTPS project destinations without private or fabricated content", () => {
  const html = `${page("en")}\n${page("zh")}`;
  const required = [
    "https://github.com/JanYork/llm-wiki-cli",
    "https://github.com/JanYork/llm-wiki-cli/wiki",
    "https://github.com/JanYork/llm-wiki-cli/releases",
    "https://github.com/JanYork/llm-wiki-cli/issues",
    "https://github.com/JanYork/llm-wiki-cli/security/policy",
    "https://github.com/JanYork/llm-wiki-cli/blob/main/LICENSE",
  ];
  for (const url of required) assert.ok(html.includes(`href="${url}`), `${url} is missing`);

  assert.doesNotMatch(html, /(?:\/Users\/|\/home\/|C:\\Users\\|localhost|example\.com|lorem ipsum)/i);
  assert.doesNotMatch(html, /(?:trusted by|10x|industry-leading|thousands of users)/i);
  for (const match of html.matchAll(/href="(http[^"]+)"/g)) assert.ok(match[1].startsWith("https://"));
});

test("native behavior is bounded to copy and explicit locale persistence", () => {
  const js = read(files.js);
  assert.match(js, /lwc-site-locale/);
  assert.match(js, /navigator\.clipboard/);
  assert.match(js, /document\.createRange/);
  assert.match(js, /zh-CN\//);
  assert.doesNotMatch(js, /navigator\.(?:language|languages)/);
  assert.doesNotMatch(js, /fetch\(|XMLHttpRequest|sendBeacon|analytics/i);
  assert.ok(statSync(resolve(root, files.js)).size < 8_192, "site.js should stay under 8 KiB");
});

test("the visual system includes accessibility and reduced-motion safeguards", () => {
  const css = read(files.css);
  assert.match(css, /^\/\* Hallmark · pre-emit critique:[^\n]+\n \* macrostructure: Split Studio/);
  assert.match(css, /:root\s*{/);
  assert.match(css, /oklch\(/);
  assert.match(css, /overflow-x:\s*clip/);
  assert.match(css, /:focus-visible/);
  assert.match(css, /prefers-reduced-motion:\s*reduce/);
  assert.match(css, /min-height:\s*44px/);
  assert.ok(statSync(resolve(root, files.css)).size < 40_960, "site.css should stay under 40 KiB");
});

test("Pages verifies and uploads only the static site with least privilege", () => {
  const workflow = read(files.workflow);
  assert.match(workflow, /actions\/checkout@v7/);
  assert.match(workflow, /actions\/setup-node@v6/);
  assert.match(workflow, /actions\/configure-pages@v6/);
  assert.match(workflow, /actions\/upload-pages-artifact@v5/);
  assert.match(workflow, /actions\/deploy-pages@v5/);
  assert.match(workflow, /node-version:\s*["']?24["']?/);
  assert.match(workflow, /node --test tests\/site\.mjs/);
  assert.match(workflow, /path:\s*["']?site\/?["']?/);
  assert.match(workflow, /environment:\s*\n\s*name:\s*github-pages/);
  assert.match(workflow, /pages:\s*write/);
  assert.match(workflow, /id-token:\s*write/);
  assert.match(workflow, /contents:\s*read/);
  assert.doesNotMatch(workflow, /contents:\s*write/);
  assert.match(read(files.ci), /node --test tests\/site\.mjs/);
});
