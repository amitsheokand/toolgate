#!/usr/bin/env node
/**
 * OpenCode plugin: forwards tool.execute.before/after to `toolgate hook --harness opencode`.
 * No policy logic here.
 */
import { spawnSync } from "node:child_process";

const bin = process.env.TOOLGATE_BIN ?? "toolgate";

function runHook(eventName, payload) {
  const child = spawnSync(
    bin,
    ["hook", "--harness", "opencode", "--event", eventName],
    { input: JSON.stringify(payload), encoding: "utf8" },
  );
  if (child.status !== 0 || child.error) {
    return {};
  }
  const text = (child.stdout ?? "").trim();
  if (!text) return {};
  try {
    return JSON.parse(text);
  } catch {
    return {};
  }
}

function applyBefore(reply, output) {
  if (reply.deny) {
    throw new Error(reply.message ?? "toolgate denied tool");
  }
  if (reply.args && typeof reply.args === "object") {
    output.args = { ...(output.args ?? {}), ...reply.args };
  }
}

function applyAfter(reply, output) {
  if (typeof reply.output === "string") {
    output.output = reply.output;
  }
}

export default function () {
  return {
    "tool.execute.before": async (input, output) => {
      const payload = {
        tool: input.tool,
        input: output.args,
        sessionId: input.sessionID,
        cwd: process.cwd(),
      };
      applyBefore(runHook("tool.execute.before", payload), output);
    },
    "tool.execute.after": async (input, output) => {
      const payload = {
        tool: input.tool,
        input: output.args,
        output: output.output,
        sessionId: input.sessionID,
        cwd: process.cwd(),
      };
      applyAfter(runHook("tool.execute.after", payload), output);
    },
  };
}
