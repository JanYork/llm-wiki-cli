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
import { EventEmitter } from "node:events";
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

test("canonical LWC Skills are bundled identically for every native plugin", () => {
  for (const skill of [
    "using-lwc",
    "using-todo",
    "using-plan",
    "using-sync",
    "using-tutor",
    "using-book",
    "using-practice",
  ]) {
    const canonical = join(root, `skills/${skill}`);
    const expected = files(canonical);
    if (skill === "using-lwc") assert.ok(expected.includes("references/temporal-memory.md"));
    for (const integration of ["codex-lwc", "claude-lwc", "pi-lwc"]) {
      const copy = join(root, `integrations/${integration}/skills/${skill}`);
      assert.deepEqual(files(copy), expected, `${integration}/${skill}`);
      for (const file of expected) {
        assert.deepEqual(readFileSync(join(copy, file)), readFileSync(join(canonical, file)), `${integration}/${skill}/${file}`);
      }
    }
  }
});

test("canonical and mirrored guidance keeps the archive workflow Agent-safe", () => {
  for (const directory of [
    "skills/using-lwc",
    "integrations/codex-lwc/skills/using-lwc",
    "integrations/claude-lwc/skills/using-lwc",
    "integrations/pi-lwc/skills/using-lwc",
  ]) {
    const guidance = [
      readFileSync(join(root, directory, "SKILL.md"), "utf8"),
      readFileSync(join(root, directory, "references/memory-archive.md"), "utf8"),
    ].join("\n");
    assert.match(guidance, /Agent-first/i, `${directory} Agent-first routing`);
    assert.match(
      guidance,
      /memory.*plaintext.*trusted recipient/is,
      `${directory} plaintext recipient warning`,
    );
    assert.match(
      guidance,
      /archive.*untrusted data.*not (?:as )?instructions/is,
      `${directory} untrusted archive boundary`,
    );
    assert.match(
      guidance,
      /separate human confirmation.*overwrite/is,
      `${directory} overwrite confirmation`,
    );
  }
});

test("Codex and Claude packages expose the stable LWC MCP and hook boundary", () => {
  for (const integration of ["codex-lwc", "claude-lwc"]) {
    const base = join(root, "integrations", integration);
    const mcp = JSON.parse(readFileSync(join(base, ".mcp.json"), "utf8"));
    assert.equal(mcp.mcpServers.lwc.command, "lwc");
    assert.deepEqual(mcp.mcpServers.lwc.args, ["serve", "--mcp"]);
    assert.equal(mcp.mcpServers.codegraph, undefined);
    const hooksText = readFileSync(join(base, "hooks/hooks.json"), "utf8");
    const hooks = JSON.parse(hooksText).hooks;
    const agent = integration.split("-")[0];
    assert.match(hooksText, new RegExp(`agent hook --agent ${agent}`));
    assert.doesNotMatch(hooksText, /load tag|wiki\.db|codegraph prompt-hook/);
    assert.ok(hooks.SessionStart, `${integration} SessionStart`);
    assert.ok(hooks.Stop, `${integration} guarded Stop continuation`);
    assert.match(JSON.stringify(hooks.Stop), new RegExp(`--agent ${agent} --event Stop`));
    assert.equal(hooks.PreToolUse[0].matcher, "Bash", `${integration} shell-only matcher`);
    assert.equal(hooks.PreToolUse[0].hooks[0].timeout, 2, `${integration} bounded PreToolUse`);
    if (integration === "codex-lwc") {
      assert.ok(hooks.UserPromptSubmit, "Codex prompt context hook");
      assert.doesNotMatch(hooksText, /PostCompact/);
      assert.deepEqual(Object.keys(hooks).sort(), [
        "PostToolUse",
        "PreToolUse",
        "SessionStart",
        "Stop",
        "SubagentStart",
        "UserPromptSubmit",
      ]);
    } else {
      assert.deepEqual(Object.keys(hooks).sort(), [
        "PostToolUse",
        "PostToolUseFailure",
        "PreToolUse",
        "SessionStart",
        "Stop",
        "SubagentStart",
        "UserPromptSubmit",
      ]);
    }
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

test("Pi injects boundary context and bridges both read-only LWC tools over MCP", async () => {
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
        ? { content: [{ type: "text", text: "MCP_OK", annotations: { audience: ["assistant"] }, futureContentField: true }], structuredContent: { name: message.params.name, arguments: message.params.arguments } }
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
    const tools = new Map();
    const source = readFileSync(join(root, "integrations/pi-lwc/extensions/lwc.js"), "utf8")
      .replace(
        'import { Type } from "typebox";',
        'const Type = { String: () => ({}), Integer: options => ({ type: "integer", ...options }), Literal: value => ({ const: value }), Union: values => ({ anyOf: values }), Optional: value => value, Object: (properties, options = {}) => ({ type: "object", properties, ...options }) };',
      );
    const extension = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
    extension.default({
      on: (event, handler) => handlers.set(event, handler),
      registerTool: (definition) => { tools.set(definition.name, definition); },
    });
    const context = { sessionManager: { getSessionId: () => "pi-session" } };
    assert.equal(handlers.has("stop"), false, "Pi must not fabricate Stop continuation");
    assert.equal(handlers.has("subagent_start"), false, "Pi has no native subagent lifecycle");
    assert.equal(handlers.has("subagent_stop"), false, "Pi has no native subagent lifecycle");
    assert.equal(
      handlers.has("session_compact_failed"),
      false,
      "Pi 0.84.2 has no session_compact_failed extension event",
    );
    await handlers.get("session_start")({}, context);
    const first = await handlers.get("before_agent_start")({ systemPrompt: "BASE" }, context);
    assert.match(first.systemPrompt, /^BASE\n\nUse the `using-lwc` Skill/);
    assert.match(first.systemPrompt, /current absolute project path/);
    assert.match(first.systemPrompt, /two read-only tools.*`lwc_explore`.*`lwc_codegraph`/);
    assert.match(first.systemPrompt, /unsolicited lifecycle Plan\/Todo progress signals/);
    assert.match(first.systemPrompt, /agent_context\.status=bound/);
    assert.match(first.systemPrompt, /plan\.tracking.*plan\.additional_trackings.*todo\.reminders/);
    assert.match(first.systemPrompt, /context-qualified.*plan\.current.*todo\.list/);
    assert.match(first.systemPrompt, /never `track` or start work from a reminder/);
    assert.match(first.systemPrompt, /tool receipt or follow-up.*own just-issued LWC Plan\/Todo command/);
    assert.match(first.systemPrompt, /Treat graphs independently: ask for CodeGraph only for a code-structure task with code evidence/);
    assert.match(first.systemPrompt, /new stable request_id per mutation.*before display/);
    assert.match(first.systemPrompt, /one plain sentence about the learning outcome or next teaching action.*never expose Tutor, using-tutor, Skill, LWC, storage, persistence, recording, progress, status, or IDs/);
    assert.match(first.systemPrompt, /CTX$/);
    assert.equal(await handlers.get("before_agent_start")({ systemPrompt: "BASE" }, context), undefined);
    await handlers.get("session_before_compact")({}, context);
    assert.equal(
      await handlers.get("before_agent_start")({ systemPrompt: "NEXT" }, context),
      undefined,
      "pre-compact observation must not masquerade as post-compact context",
    );
    await handlers.get("session_compact")({}, context);
    const compact = await handlers.get("before_agent_start")({ systemPrompt: "NEXT" }, context);
    assert.match(compact.systemPrompt, /^NEXT\n\nUse the `using-lwc` Skill/);
    assert.match(compact.systemPrompt, /CTX$/);
    assert.equal(
      await handlers.get("before_agent_start")({ systemPrompt: "NEXT" }, context),
      undefined,
      "a compact boundary must inject exactly once",
    );
    assert.deepEqual([...tools.keys()], ["lwc_explore", "lwc_codegraph"]);
    const explore = tools.get("lwc_explore");
    assert.match(explore.description, /Read/);
    assert.ok(explore.parameters.properties.maxDocuments);
    assert.equal(explore.parameters.properties.maxDocuments.type, "integer");
    assert.equal(explore.parameters.properties.maxDocuments.maximum, 20);
    assert.equal(explore.parameters.properties.limit, undefined);
    const result = await explore.execute("call-1", {
      query: "memory",
      projectPath: directory,
      maxDocuments: 3,
    });
    assert.equal(result.content[0].text, "MCP_OK");
    assert.equal(result.details.name, "lwc_explore");
    assert.equal(result.details.arguments.maxDocuments, 3);
    const codegraph = tools.get("lwc_codegraph");
    assert.match(codegraph.description, /node\/search\/callers\/callees.*precise.*explore.*broad/);
    assert.ok(codegraph.parameters.properties.projectPath);
    assert.ok(codegraph.parameters.properties.command);
    assert.ok(codegraph.parameters.properties.arguments);
    assert.equal(codegraph.parameters.properties.arguments.additionalProperties, true);
    const codegraphResult = await codegraph.execute("call-2", {
      projectPath: directory,
      command: "node",
      arguments: { symbol: "GetScanBatch", includeCode: true },
    });
    assert.equal(codegraphResult.content[0].text, "MCP_OK");
    assert.deepEqual(codegraphResult.content[0].annotations, { audience: ["assistant"] });
    assert.equal(codegraphResult.content[0].futureContentField, true);
    assert.equal(codegraphResult.details.name, "lwc_codegraph");
    assert.deepEqual(codegraphResult.details.arguments, {
      projectPath: directory,
      command: "node",
      arguments: { symbol: "GetScanBatch", includeCode: true },
    });
    await handlers.get("session_shutdown")();
  } finally {
    process.env.PATH = previousPath;
    rmSync(directory, { recursive: true, force: true });
  }
});

test("Pi gives CodeGraph calls a longer timeout and replaces a timed-out MCP child", async () => {
  const timers = new Map();
  const scheduledDelays = [];
  const children = [];
  const held = new Map();
  let nextTimer = 1;

  const respond = (child, message, text = "MCP_OK") => {
    child.stdout.emit("data", `${JSON.stringify({
      jsonrpc: "2.0",
      id: message.id,
      result: message.method === "initialize"
        ? { protocolVersion: "2024-11-05", capabilities: {} }
        : { content: [{ type: "text", text }] },
    })}\n`);
  };
  const spawn = () => {
    const child = new EventEmitter();
    child.stdout = new EventEmitter();
    child.stdout.setEncoding = () => {};
    child.killed = false;
    child.stdin = {
      write(line) {
        const message = JSON.parse(line);
        if (message.id == null) return;
        const toolArguments = message.params?.arguments;
        const marker = toolArguments?.query ?? toolArguments?.arguments?.query;
        if (marker === "TIMEOUT" || marker === "AFTER_TIMEOUT") {
          held.set(marker, { child, message });
        } else {
          respond(child, message, `MCP_CHILD_${children.indexOf(child) + 1}`);
        }
      },
    };
    child.kill = () => {
      child.killed = true;
    };
    children.push(child);
    return child;
  };
  const setTimeout = (callback, delay) => {
    const id = nextTimer++;
    scheduledDelays.push(delay);
    timers.set(id, callback);
    return id;
  };
  const clearTimeout = (id) => timers.delete(id);
  const fireTimer = (delay) => {
    const entry = [...timers.entries()].find(([, callback]) => callback.delay === delay)
      || [...timers.entries()].find(([, callback]) => callback && delay === scheduledDelays.at(-1));
    assert.ok(entry, `missing ${delay}ms timer`);
    timers.delete(entry[0]);
    entry[1]();
  };

  globalThis.__piLwcBridgeHarness = {
    execFileSync: () => "{}",
    spawn,
    setTimeout: (callback, delay) => {
      callback.delay = delay;
      return setTimeout(callback, delay);
    },
    clearTimeout,
  };
  try {
    const source = readFileSync(join(root, "integrations/pi-lwc/extensions/lwc.js"), "utf8")
      .replace(
        'import { execFileSync, spawn } from "node:child_process";',
        "const { execFileSync, spawn, setTimeout, clearTimeout } = globalThis.__piLwcBridgeHarness;",
      )
      .replace(
        'import { Type } from "typebox";',
        'const Type = { String: () => ({}), Integer: options => ({ type: "integer", ...options }), Literal: value => ({ const: value }), Union: values => ({ anyOf: values }), Optional: value => value, Object: (properties, options = {}) => ({ type: "object", properties, ...options }) };',
      );
    const extension = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
    const tools = new Map();
    extension.default({
      on() {},
      registerTool: (definition) => tools.set(definition.name, definition),
    });
    const explore = tools.get("lwc_explore");
    const codegraph = tools.get("lwc_codegraph");

    await explore.execute("memory", { query: "memory", projectPath: root });
    assert.equal(scheduledDelays.at(-1), 5_000, "memory retrieval keeps the short timeout");
    await explore.execute("code", { query: "code", projectPath: root, mode: "code" });
    assert.equal(scheduledDelays.at(-1), 65_000, "code exploration covers the 60s server bound");
    await explore.execute("all", { query: "all", projectPath: root, mode: "all" });
    assert.equal(scheduledDelays.at(-1), 65_000, "all-mode exploration covers CodeGraph");
    await codegraph.execute("codegraph", {
      projectPath: root,
      command: "node",
      arguments: { symbol: "target" },
    });
    assert.equal(scheduledDelays.at(-1), 65_000, "the dedicated CodeGraph tool uses 65s");

    const timedOut = codegraph.execute("timeout", {
      projectPath: root,
      command: "search",
      arguments: { query: "TIMEOUT" },
    });
    await Promise.resolve();
    await Promise.resolve();
    fireTimer(65_000);
    await assert.rejects(timedOut, /LWC MCP timeout/);
    assert.equal(children[0].killed, true, "timeout closes the poisoned bridge");

    const afterTimeout = explore.execute("after-timeout", {
      query: "AFTER_TIMEOUT",
      projectPath: root,
    });
    await Promise.resolve();
    await Promise.resolve();
    assert.equal(children.length, 2, "the next call starts a fresh MCP child");
    children[0].emit("exit", 1);
    const next = held.get("AFTER_TIMEOUT");
    respond(next.child, next.message, "NEW_CHILD_OK");
    assert.equal((await afterTimeout).content[0].text, "NEW_CHILD_OK");
    assert.equal(children.length, 2, "a late old-child exit must not clear the new bridge");
  } finally {
    delete globalThis.__piLwcBridgeHarness;
  }
});

test("Pi sends only the bounded current prompt and injects transient turn context", async () => {
  if (process.platform === "win32") return;
  const directory = mkdtempSync(join(tmpdir(), "lwc-pi-prompt-"));
  const executable = join(directory, "lwc");
  const calls = join(directory, "calls.jsonl");
  const quotedCalls = calls.replaceAll("'", "'\"'\"'");
  writeFileSync(
    executable,
    `#!/bin/sh
input="$(cat)"
printf '%s\\t%s\\n' "$*" "$input" >> '${quotedCalls}'
case "$input" in
  *UNRELATED*) printf '%s\\n' '{"additionalContext":""}' ;;
  *MALFORMED*) printf '%s\\n' 'not-json' ;;
  *OVERSIZE*) node -e 'process.stdout.write(JSON.stringify({additionalContext:"x".repeat(65536)}))' ;;
  *\"prompt\"*) printf '%s\\n' '{"additionalContext":"PROMPT_CTX"}' ;;
  *) printf '%s\\n' '{"additionalContext":"LIFECYCLE_CTX"}' ;;
esac
`,
  );
  chmodSync(executable, 0o755);
  const previousPath = process.env.PATH;
  process.env.PATH = `${directory}:${previousPath}`;
  try {
    const handlers = new Map();
    const source = readFileSync(join(root, "integrations/pi-lwc/extensions/lwc.js"), "utf8")
      .replace(
        'import { Type } from "typebox";',
        'const Type = { String: () => ({}), Integer: options => ({ type: "integer", ...options }), Literal: value => ({ const: value }), Union: values => ({ anyOf: values }), Optional: value => value, Object: properties => ({ type: "object", properties }) };',
      );
    assert.doesNotMatch(source, /transcript|conversation_history|event\.messages/);
    assert.doesNotMatch(source, /node:fs|readFile|writeFile/);
    assert.match(source, /MAX_PROMPT_CHARS\s*=\s*4096/);
    assert.match(source, /timeout:\s*2000/);
    assert.match(source, /maxBuffer:\s*MAX_OUTPUT_BYTES/);
    const extension = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
    extension.default({
      on: (event, handler) => handlers.set(event, handler),
      registerTool() {},
    });
    const context = { sessionManager: { getSessionId: () => "pi-session" } };

    await handlers.get("session_start")({}, context);
    const first = await handlers.get("before_agent_start")({
      systemPrompt: "BASE",
      prompt: "first prompt",
    }, context);
    assert.match(first.systemPrompt, /^BASE\n\nUse the `using-lwc` Skill/);
    assert.match(first.systemPrompt, /LIFECYCLE_CTX/);
    assert.match(first.systemPrompt, /PROMPT_CTX$/);

    const subsequent = await handlers.get("before_agent_start")({
      systemPrompt: "NEXT",
      prompt: "subsequent prompt",
    }, context);
    assert.equal(subsequent.systemPrompt, "NEXT\n\nPROMPT_CTX");
    assert.doesNotMatch(subsequent.systemPrompt, /using-lwc/);

    const huge = `😀`.repeat(5000) + "SECRET_AFTER_LIMIT";
    assert.deepEqual(
      await handlers.get("before_agent_start")({ systemPrompt: "HUGE", prompt: huge }, context),
      { systemPrompt: "HUGE\n\nPROMPT_CTX" },
    );
    const recorded = readFileSync(calls, "utf8")
      .trimEnd()
      .split("\n")
      .map((line) => {
        const tab = line.indexOf("\t");
        return { argv: line.slice(0, tab), payload: JSON.parse(line.slice(tab + 1)) };
      });
    const promptCalls = recorded.filter(({ argv }) => argv.includes("--event before_agent_start"));
    const lifecycleCalls = recorded.filter(({ argv }) => argv.includes("--event session_start"));
    assert.deepEqual(lifecycleCalls.map(({ payload }) => payload), [
      { session_id: "pi-session" },
    ]);
    assert.deepEqual(promptCalls.slice(0, 2).map(({ payload }) => payload), [
      { prompt: "first prompt", session_id: "pi-session" },
      { prompt: "subsequent prompt", session_id: "pi-session" },
    ]);
    assert.equal(Array.from(promptCalls.at(-1).payload.prompt).length, 4096);
    assert.doesNotMatch(promptCalls.at(-1).payload.prompt, /SECRET_AFTER_LIMIT/);

    const privateEvent = {
      systemPrompt: "PRIVATE",
      prompt: "privacy prompt",
      get transcript_path() { throw new Error("transcript read"); },
      get messages() { throw new Error("messages read"); },
      get conversation_history() { throw new Error("history read"); },
    };
    assert.deepEqual(await handlers.get("before_agent_start")(privateEvent, context), {
      systemPrompt: "PRIVATE\n\nPROMPT_CTX",
    });

    assert.equal(
      await handlers.get("before_agent_start")({ systemPrompt: "EMPTY", prompt: "UNRELATED" }, context),
      undefined,
    );
    assert.equal(
      await handlers.get("before_agent_start")({ systemPrompt: "BAD", prompt: "MALFORMED" }, context),
      undefined,
    );
    assert.equal(
      await handlers.get("before_agent_start")({ systemPrompt: "LARGE", prompt: "OVERSIZE" }, context),
      undefined,
    );
    const beforeMissing = readFileSync(calls, "utf8");
    assert.equal(await handlers.get("before_agent_start")({ systemPrompt: "MISSING" }, context), undefined);
    assert.equal(
      await handlers.get("before_agent_start")({ systemPrompt: "INVALID", prompt: ["array"] }, context),
      undefined,
    );
    assert.equal(
      readFileSync(calls, "utf8"),
      beforeMissing,
      "missing or non-string prompt must not spawn LWC",
    );
  } finally {
    process.env.PATH = previousPath;
    rmSync(directory, { recursive: true, force: true });
  }
});

test("Pi package uses only official 0.84.2 lifecycle events and an MCP tool bridge", () => {
  const base = join(root, "integrations/pi-lwc");
  const manifest = JSON.parse(readFileSync(join(base, "package.json"), "utf8"));
  assert.deepEqual(manifest.pi.extensions, ["./extensions/lwc.js"]);
  assert.deepEqual(manifest.pi.skills, ["./skills"]);
  assert.deepEqual(manifest.peerDependencies, { typebox: "*" });
  assert.equal(manifest.mcpServers, undefined);
  const extension = readFileSync(join(base, "extensions/lwc.js"), "utf8");
  for (const event of [
    "session_start",
    "session_before_compact",
    "session_compact",
    "before_agent_start",
    "session_shutdown",
  ]) {
    assert.match(extension, new RegExp(event));
  }
  for (const unsupported of [
    "session_compact_failed",
    "stop",
    "subagent_start",
    "subagent_stop",
  ]) {
    assert.doesNotMatch(
      extension,
      new RegExp(`pi\\.on\\(["']${unsupported}["']`),
      `Pi must not register a synthetic ${unsupported} hook`,
    );
  }
  assert.match(extension, /systemPrompt/);
  assert.match(extension, /Use the `using-lwc` Skill/);
  assert.match(extension, /current absolute project path/);
  assert.match(extension, /unsolicited lifecycle Plan\/Todo progress signals/);
  assert.match(extension, /agent_context\.status=bound/);
  assert.match(extension, /plan\.tracking.*plan\.additional_trackings.*todo\.reminders/);
  assert.match(extension, /context-qualified.*plan\.current.*todo\.list/);
  assert.match(extension, /never `track` or start work from a reminder/);
  assert.match(extension, /tool receipt or follow-up.*own just-issued LWC Plan\/Todo command/);
  assert.match(extension, /"--scope", "all", "agent", "hook", "--agent", "pi"/);
  assert.match(extension, /registerTool/);
  assert.match(extension, /name: "lwc_explore"/);
  assert.match(extension, /spawn\("lwc", \["serve", "--mcp"\]/);
});
