import assert from "node:assert/strict";
import {
  chmodSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import test from "node:test";

const root = new URL("..", import.meta.url).pathname;

function files(directory) {
  const found = [];
  function walk(current) {
    for (const name of readdirSync(current).sort()) {
      const path = join(current, name);
      if (statSync(path).isDirectory()) walk(path);
      else found.push(relative(directory, path));
    }
  }
  walk(directory);
  return found;
}

test("native packages contain byte-identical canonical using-lwc skills", () => {
  const canonical = join(root, "skills/using-lwc");
  const expected = files(canonical);
  for (const integration of ["codex-lwc", "claude-lwc", "pi-lwc"]) {
    const copy = join(root, `integrations/${integration}/skills/using-lwc`);
    assert.deepEqual(files(copy), expected, integration);
    for (const file of expected) {
      assert.deepEqual(
        readFileSync(join(copy, file)),
        readFileSync(join(canonical, file)),
        `${integration}/${file}`,
      );
    }
  }
});

test("Codex and Claude packages expose the stable LWC MCP and hook boundary", () => {
  for (const integration of ["codex-lwc", "claude-lwc"]) {
    const base = join(root, "integrations", integration);
    const mcp = JSON.parse(readFileSync(join(base, ".mcp.json"), "utf8"));
    assert.equal(mcp.mcpServers.codegraph.command, "lwc");
    assert.deepEqual(mcp.mcpServers.codegraph.args, ["cg", "serve", "--mcp"]);
    const hooks = readFileSync(join(base, "hooks/hooks.json"), "utf8");
    assert.match(hooks, new RegExp(`agent hook --agent ${integration.split("-")[0]}`));
    assert.doesNotMatch(hooks, /load tag|wiki\.db|codegraph prompt-hook/);
  }
  const codex = JSON.parse(
    readFileSync(join(root, "integrations/codex-lwc/.codex-plugin/plugin.json"), "utf8"),
  );
  assert.equal(codex.name, "codex-lwc");
  assert.equal(codex.mcpServers, "./.mcp.json");
  const codexMarketplace = JSON.parse(
    readFileSync(
      join(root, "integrations/codex-lwc/.agents/plugins/marketplace.json"),
      "utf8",
    ),
  );
  assert.equal(codexMarketplace.plugins[0].policy.installation, "AVAILABLE");
  assert.equal(codexMarketplace.plugins[0].policy.authentication, "ON_INSTALL");
  const claude = JSON.parse(
    readFileSync(join(root, "integrations/claude-lwc/.claude-plugin/plugin.json"), "utf8"),
  );
  assert.equal(claude.hooks, "./hooks/hooks.json");
});

test("Pi injects boundary context once through before_agent_start", async () => {
  if (process.platform === "win32") return;
  const directory = mkdtempSync(join(tmpdir(), "lwc-pi-integration-"));
  const executable = join(directory, "lwc");
  writeFileSync(executable, '#!/bin/sh\nprintf \'{"additionalContext":"CTX"}\\n\'\n');
  chmodSync(executable, 0o755);
  const previousPath = process.env.PATH;
  process.env.PATH = `${directory}:${previousPath}`;
  try {
    const handlers = new Map();
    const extension = await import(
      `${new URL("../integrations/pi-lwc/extensions/lwc.js", import.meta.url)}?test=${Date.now()}`
    );
    extension.default({ on: (event, handler) => handlers.set(event, handler) });
    await handlers.get("session_start")();
    const first = await handlers.get("before_agent_start")({ systemPrompt: "BASE" });
    assert.equal(first.systemPrompt, "BASE\n\nCTX");
    assert.equal(await handlers.get("before_agent_start")({ systemPrompt: "BASE" }), undefined);
    await handlers.get("session_before_compact")();
    const compact = await handlers.get("before_agent_start")({ systemPrompt: "NEXT" });
    assert.equal(compact.systemPrompt, "NEXT\n\nCTX");
  } finally {
    process.env.PATH = previousPath;
    rmSync(directory, { recursive: true, force: true });
  }
});

test("Pi package uses lifecycle events and an explicit CLI bridge without MCP claims", () => {
  const base = join(root, "integrations/pi-lwc");
  const manifest = JSON.parse(readFileSync(join(base, "package.json"), "utf8"));
  assert.deepEqual(manifest.pi.extensions, ["./extensions/lwc.js"]);
  assert.deepEqual(manifest.pi.skills, ["./skills"]);
  assert.equal(manifest.mcpServers, undefined);
  const extension = readFileSync(join(base, "extensions/lwc.js"), "utf8");
  for (const event of ["session_start", "session_before_compact", "before_agent_start"]) {
    assert.match(extension, new RegExp(event));
  }
  assert.match(extension, /systemPrompt/);
  assert.match(extension, /agent", "hook", "--agent", "pi"/);
});
