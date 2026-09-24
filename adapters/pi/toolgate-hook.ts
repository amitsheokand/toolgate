/**
 * Pi extension: registers tool_call / tool_result handlers that call
 * `toolgate hook --harness pi`. No policy logic here.
 */
import { spawnSync } from "node:child_process";

const bin = "@TOOLGATE_BIN@";

function runHook(eventName: string, event: unknown) {
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
    return JSON.parse(text) as Record<string, unknown>;
  } catch {
    return {};
  }
}

function mergeInput(event: { input?: Record<string, unknown> }, patch: Record<string, unknown>) {
  if (!patch || typeof patch !== "object") return;
  event.input = { ...(event.input ?? {}), ...patch };
}

export default function (pi: {
  on: (
    name: string,
    fn: (event: Record<string, unknown>) => Promise<Record<string, unknown> | void>,
  ) => void;
}) {
  pi.on("tool_call", async (event) => {
    const reply = runHook("tool_call", event) as {
      block?: boolean;
      reason?: string;
      message?: string;
      input?: Record<string, unknown>;
    };
    if (reply.block) {
      return { block: true, reason: reply.reason ?? reply.message ?? "blocked" };
    }
    if (reply.input) {
      mergeInput(event, reply.input);
    }
  });

  pi.on("tool_result", async (event) => {
    const reply = runHook("tool_result", event) as { content?: unknown[] };
    if (Array.isArray(reply.content)) {
      return { content: reply.content, details: event.details };
    }
  });
}
