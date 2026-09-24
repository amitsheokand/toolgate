#!/usr/bin/env node
/**
 * OpenCode shim: forwards tool.execute events to `toolgate hook --harness opencode`.
 * No policy logic here.
 */
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

const event = process.env.TOOLGATE_EVENT ?? "tool.execute.before";
const input = readFileSync(0, "utf8");
const bin = process.env.TOOLGATE_BIN ?? "toolgate";
const child = spawnSync(
  bin,
  ["hook", "--harness", "opencode", "--event", event],
  { input, encoding: "utf8" },
);
if (child.status !== 0 || child.error) {
  process.stdout.write(JSON.stringify({ allow: true }));
  process.exit(0);
}
process.stdout.write(child.stdout || JSON.stringify({ allow: true }));
