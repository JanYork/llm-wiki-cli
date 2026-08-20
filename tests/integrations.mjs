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

test("canonical LWC Skill is bundled identically for every native plugin", () => {
  const canonical = join(root, "skills/using-lwc");
  const expected = files(canonical);
  assert.ok(expected.includes("references/temporal-memory.md"));
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
    assert.equal(mcp.mcpServers.lwc.command, "lwc");
    assert.deepEqual(mcp.mcpServers.lwc.args, ["serve", "--mcp"]);
    assert.equal(mcp.mcpServers.codegraph, undefined);
    const hooks = readFileSync(join(base, "hooks/hooks.json"), "utf8");
    assert.match(hooks, new RegExp(`agent hook --agent ${integration.split("-")[0]}`));
    assert.doesNotMatch(hooks, /load tag|wiki\.db|codegraph prompt-hook/);
    if (integration === "codex-lwc") assert.doesNotMatch(hooks, /PostCompact/);
  }
  const codex = JSON.parse(
    readFileSync(join(root, "integrations/codex-lwc/.codex-plugin/plugin.json"), "utf8"),
  );
  assert.equal(codex.name, "codex-lwc");
  assert.equal(codex.mcpServers, "./.mcp.json");
  assert.equal(codex.skills, "./skills");
  assert.equal(codex.hooks, "./hooks/hooks.json");
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
  assert.equal(claude.mcpServers, "./.mcp.json");
  assert.equal(claude.skills, "./skills");
});

test("Pi injects boundary context and bridges lwc_explore over MCP", async () => {
  if (process.platform === "win32") return;
  const directory = mkdtempSync(join(tmpdir(), "lwc-pi-integration-"));
  const executable = join(directory, "lwc");
  const server = join(directory, "server.mjs");
  writeFileSync(
    server,
    `
  let buffer = "";
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", chunk => {
    buffer += chunk;
    for (;;) {
      const end = buffer.indexOf("\\n");
      if (end < 0) break;
      const line = buffer.slice(0, end).trim(); buffer = buffer.slice(end + 1);
      if (!line) continue;
      const message = JSON.parse(line);
      if (message.id == null) continue;
      const result = message.method === "tools/call"
        ? { content: [{ type: "text", text: "MCP_OK" }], structuredContent: { arguments: message.params.arguments } }
        : { protocolVersion: "2024-11-05", capabilities: {}, serverInfo: { name: "fake", version: "1" } };
      console.log(JSON.stringify({ jsonrpc: "2.0", id: message.id, result }));
    }
  });
`,
  );
  writeFileSync(
    executable,
    `#!/bin/sh
if [ "$1" = "agent" ] || [ "$3" = "agent" ]; then
  printf '%s\n' '{"additionalContext":"CTX"}'
else
  exec node '${server}'
fi
`,
  );
  chmodSync(executable, 0o755);
  const previousPath = process.env.PATH;
  process.env.PATH = `${directory}:${previousPath}`;
  try {
    const handlers = new Map();
    let tool;
    const source = readFileSync(join(root, "integrations/pi-lwc/extensions/lwc.js"), "utf8")
      .replace(
        'import { Type } from "typebox";',
        'const Type = { String: () => ({}), Integer: options => ({ type: "integer", ...options }), Literal: value => ({ const: value }), Union: values => ({ anyOf: values }), Optional: value => value, Object: properties => ({ type: "object", properties }) };',
      );
    const extension = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
    extension.default({
      on: (event, handler) => handlers.set(event, handler),
      registerTool: (definition) => { tool = definition; },
    });
    await handlers.get("session_start")();
    const first = await handlers.get("before_agent_start")({ systemPrompt: "BASE" });
    assert.match(first.systemPrompt, /^BASE\n\nUse the `using-lwc` Skill/);
    assert.match(first.systemPrompt, /current absolute project path/);
    assert.match(first.systemPrompt, /CTX$/);
    assert.equal(await handlers.get("before_agent_start")({ systemPrompt: "BASE" }), undefined);
    await handlers.get("session_before_compact")();
    const compact = await handlers.get("before_agent_start")({ systemPrompt: "NEXT" });
    assert.match(compact.systemPrompt, /^NEXT\n\nUse the `using-lwc` Skill/);
    assert.match(compact.systemPrompt, /CTX$/);
    assert.ok(tool.parameters.properties.maxDocuments);
    assert.equal(tool.parameters.properties.maxDocuments.type, "integer");
    assert.equal(tool.parameters.properties.maxDocuments.maximum, 20);
    assert.equal(tool.parameters.properties.limit, undefined);
    const result = await tool.execute("call-1", {
      query: "memory",
      projectPath: directory,
      maxDocuments: 3,
    });
    assert.equal(result.content[0].text, "MCP_OK");
    assert.equal(result.details.arguments.maxDocuments, 3);
    await handlers.get("session_shutdown")();
  } finally {
    process.env.PATH = previousPath;
    rmSync(directory, { recursive: true, force: true });
  }
});

test("Pi package uses lifecycle events and an MCP tool bridge", () => {
  const base = join(root, "integrations/pi-lwc");
  const manifest = JSON.parse(readFileSync(join(base, "package.json"), "utf8"));
  assert.deepEqual(manifest.pi.extensions, ["./extensions/lwc.js"]);
  assert.deepEqual(manifest.pi.skills, ["./skills"]);
  assert.deepEqual(manifest.peerDependencies, { typebox: "*" });
  assert.equal(manifest.mcpServers, undefined);
  const extension = readFileSync(join(base, "extensions/lwc.js"), "utf8");
  for (const event of ["session_start", "session_before_compact", "before_agent_start"]) {
    assert.match(extension, new RegExp(event));
  }
  assert.match(extension, /systemPrompt/);
  assert.match(extension, /Use the `using-lwc` Skill/);
  assert.match(extension, /current absolute project path/);
  assert.match(extension, /"--scope", "all", "agent", "hook", "--agent", "pi"/);
  assert.match(extension, /registerTool/);
  assert.match(extension, /name: "lwc_explore"/);
  assert.match(extension, /spawn\("lwc", \["serve", "--mcp"\]/);
});
