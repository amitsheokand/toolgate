#!/usr/bin/env node
/**
 * Pi extension shim: spawn `toolgate hook --harness pi` on tool events.
 */
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

const event = process.env.TOOLGATE_EVENT ?? "tool_call";
const input = readFileSync(0, "utf8");
const bin = process.env.TOOLGATE_BIN ?? "toolgate";
const child = spawnSync(
  bin,
  ["hook", "--harness", "pi", "--event", event],
  { input, encoding: "utf8" },
);
if (child.status !== 0 || child.error) {
  process.stdout.write("{}");
  process.exit(0);
}
process.stdout.write(child.stdout || "{}");
