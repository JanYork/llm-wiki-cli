import { execFileSync } from "node:child_process";

function load(event) {
  try {
    const output = execFileSync(
      "lwc",
      ["agent", "hook", "--agent", "pi", "--event", event],
      { input: "{}", encoding: "utf8", timeout: 2000 },
    );
    return JSON.parse(output).additionalContext || "";
  } catch {
    return "";
  }
}

export default function (pi) {
  let pending = "";
  pi.on("session_start", async () => {
    pending = load("session_start");
  });
  pi.on("session_before_compact", async () => {
    pending = load("session_before_compact");
  });
  pi.on("before_agent_start", async (event) => {
    if (!pending) return;
    const current = pending;
    pending = "";
    return { systemPrompt: `${event.systemPrompt}\n\n${current}` };
  });
}
