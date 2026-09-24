#!/usr/bin/env node
/**
 * Pi extension: registers tool_call / tool_result handlers that call
 * `toolgate hook --harness pi`. No policy logic here.
 */
import { spawnSync } from "node:child_process";

const bin = process.env.TOOLGATE_BIN ?? "toolgate";

function runHook(eventName, event) {
  const child = spawnSync(
    bin,
    ["hook", "--harness", "pi", "--event", eventName],
    { input: JSON.stringify(event), encoding: "utf8" },
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

function mergeInput(event, patch) {
  if (!patch || typeof patch !== "object") return;
  event.input = { ...(event.input ?? {}), ...patch };
}

export default function (pi) {
  pi.on("tool_call", async (event) => {
    const reply = runHook("tool_call", event);
    if (reply.block) {
      return { block: true, reason: reply.reason ?? reply.message ?? "blocked" };
    }
    if (reply.input) {
      mergeInput(event, reply.input);
    }
  });

  pi.on("tool_result", async (event) => {
    const reply = runHook("tool_result", event);
    if (Array.isArray(reply.content)) {
      return { content: reply.content, details: event.details };
    }
  });
}
