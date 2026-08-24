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
  socialEn: "site/assets/social-card.png",
  socialZh: "site/assets/social-card-zh-CN.png",
  sitemap: "site/sitemap.xml",
  workflow: ".github/workflows/pages.yml",
  ci: ".github/workflows/ci.yml",
};

const read = (path) => readFileSync(resolve(root, path), "utf8").replaceAll("\r\n", "\n");
const page = (locale) => read(files[locale]);
const attribute = (tag, name) => tag.match(new RegExp(`\\b${name}="([^"]+)"`))?.[1];

function metaContent(html, key) {
  const tag = [...html.matchAll(/<meta\b[^>]*>/g)]
    .map((match) => match[0])
    .find((candidate) => attribute(candidate, "property") === key || attribute(candidate, "name") === key);
  assert.ok(tag, `meta ${key} is missing`);
  return attribute(tag, "content");
}

function structuredData(html) {
  const blocks = [...html.matchAll(/<script\b[^>]*type="application\/ld\+json"[^>]*>([\s\S]*?)<\/script>/g)];
  assert.equal(blocks.length, 1, "expected exactly one JSON-LD block");
  return JSON.parse(blocks[0][1]);
}

function pngDimensions(path) {
  const png = readFileSync(resolve(root, path));
  assert.equal(png.subarray(1, 4).toString(), "PNG", `${path} is not a PNG`);
  assert.equal(png.subarray(12, 16).toString(), "IHDR", `${path} has no IHDR chunk`);
  return { width: png.readUInt32BE(16), height: png.readUInt32BE(20) };
}

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

test("localized social cards are 1200 by 630 PNGs", () => {
  assert.deepEqual(pngDimensions(files.socialEn), { width: 1200, height: 630 });
  assert.deepEqual(pngDimensions(files.socialZh), { width: 1200, height: 630 });
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

test("both locales expose complete localized social metadata", () => {
  const expected = {
    en: {
      title: "LWC — Proactive Memory for AI Agents",
      description: "A durable, source-grounded Wiki that your Agent can recall and maintain across sessions.",
      image: "https://janyork.github.io/llm-wiki-cli/assets/social-card.png",
      alt: "LWC Memory Atlas: source evidence flows into a maintained Wiki and the next Agent session.",
      locale: "en_US",
      alternate: "zh_CN",
    },
    zh: {
      title: "LWC — AI Agent 的主动记忆系统",
      description: "一套由 Agent 主动维护、来源可追溯、能够跨会话持续使用的持久 Wiki。",
      image: "https://janyork.github.io/llm-wiki-cli/assets/social-card-zh-CN.png",
      alt: "LWC 记忆图册：来源证据进入持续维护的 Wiki，并在下一次 Agent 会话中成为可用上下文。",
      locale: "zh_CN",
      alternate: "en_US",
    },
  };

  for (const locale of ["en", "zh"]) {
    const html = page(locale);
    const values = expected[locale];
    assert.equal(metaContent(html, "og:site_name"), "LWC");
    assert.equal(metaContent(html, "og:title"), values.title);
    assert.equal(metaContent(html, "og:description"), values.description);
    assert.equal(metaContent(html, "og:locale"), values.locale);
    assert.equal(metaContent(html, "og:locale:alternate"), values.alternate);
    assert.equal(metaContent(html, "og:image"), values.image);
    assert.equal(metaContent(html, "og:image:secure_url"), values.image);
    assert.equal(metaContent(html, "og:image:type"), "image/png");
    assert.equal(metaContent(html, "og:image:width"), "1200");
    assert.equal(metaContent(html, "og:image:height"), "630");
    assert.equal(metaContent(html, "og:image:alt"), values.alt);
    assert.equal(metaContent(html, "twitter:card"), "summary_large_image");
    assert.equal(metaContent(html, "twitter:title"), values.title);
    assert.equal(metaContent(html, "twitter:description"), values.description);
    assert.equal(metaContent(html, "twitter:image"), values.image);
    assert.equal(metaContent(html, "twitter:image:alt"), values.alt);
  }
});

test("both locales expose truthful SoftwareApplication data and license text", () => {
  const expected = {
    en: {
      url: "https://janyork.github.io/llm-wiki-cli/",
      description: "LWC gives AI Agents durable, source-grounded memory across sessions, projects, and tools.",
    },
    zh: {
      url: "https://janyork.github.io/llm-wiki-cli/zh-CN/",
      description: "LWC 为 AI Agent 提供可跨会话、跨项目与跨工具持续使用的、来源可追溯的持久记忆。",
    },
  };

  for (const locale of ["en", "zh"]) {
    const html = page(locale);
    const data = structuredData(html);
    assert.equal(data["@context"], "https://schema.org");
    assert.equal(data["@type"], "SoftwareApplication");
    assert.equal(data.name, "LWC");
    assert.equal(data.url, expected[locale].url);
    assert.equal(data.description, expected[locale].description);
    assert.equal(data.description, metaContent(html, "description"));
    assert.equal(data.applicationCategory, "DeveloperApplication");
    assert.equal(data.operatingSystem, "macOS, Linux, Windows");
    assert.equal(data.isAccessibleForFree, true);
    assert.deepEqual(data.offers, { "@type": "Offer", price: 0, priceCurrency: "USD" });
    assert.equal(data.license, "https://github.com/JanYork/llm-wiki-cli/blob/main/LICENSE");
    assert.equal(data.downloadUrl, "https://github.com/JanYork/llm-wiki-cli/releases");
    assert.equal(data.sameAs, "https://github.com/JanYork/llm-wiki-cli");
    assert.ok(html.includes("Apache-2.0"), `${locale} visible license is not Apache-2.0`);
    assert.doesNotMatch(html, /\bMIT(?: License)?\b/);
    assert.doesNotMatch(html, /<meta\b[^>]*name="keywords"/i);
    for (const unsupported of ["aggregateRating", "review", "softwareVersion"])
      assert.equal(data[unsupported], undefined, `${unsupported} must not be fabricated or stale`);
  }
});

test("the sitemap contains exactly the two reciprocal locale URLs", () => {
  const sitemap = read(files.sitemap);
  assert.equal([...sitemap.matchAll(/<url>/g)].length, 2);
  assert.equal([...sitemap.matchAll(/<loc>/g)].length, 2);
  for (const url of [
    "https://janyork.github.io/llm-wiki-cli/",
    "https://janyork.github.io/llm-wiki-cli/zh-CN/",
  ]) assert.ok(sitemap.includes(`<loc>${url}</loc>`), `${url} is missing from sitemap`);
  for (const hreflang of ["en", "zh-CN", "x-default"])
    assert.equal([...sitemap.matchAll(new RegExp(`hreflang="${hreflang}"`, "g"))].length, 2);
  assert.doesNotMatch(sitemap, /<(?:lastmod|changefreq|priority)>/);
  assert.equal(existsSync(resolve(root, "site/robots.txt")), false);
});

test("both pages contain the exact normative README Agent prompt", () => {
  const expected = setupPrompt();
  assert.equal(embeddedPrompt(page("en")), expected);
  assert.equal(embeddedPrompt(page("zh")), expected);
});

test("both locales expose continuity and Learning Suite capabilities", () => {
  for (const [locale, names] of [
    ["en", ["Temporal memory", "Durable Todo", "Current Plan", "Tutor", "Book", "Practice"]],
    ["zh", ["时序记忆", "持久 Todo", "当前 Plan", "Tutor 教学", "Book 整书阅读", "Practice 练习"]],
  ]) {
    const html = page(locale);
    for (const name of names) assert.ok(html.includes(`<h3>${name}</h3>`), `${locale}: ${name} is missing`);
  }
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
    for (const script of html.matchAll(/<script\b[^>]*>/g)) {
      if (attribute(script[0], "src")) continue;
      assert.equal(attribute(script[0], "type"), "application/ld+json", "executable inline script is forbidden");
    }
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
