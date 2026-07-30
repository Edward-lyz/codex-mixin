#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const probePath = path.join(scriptDirectory, "ducx_app_server_probe.mjs");

const MOCK_SERVER = String.raw`#!/usr/bin/env node
import readline from "node:readline";

const mode = process.env.MOCK_DUCX_MODE || "safe";
const input = readline.createInterface({ input: process.stdin });

function send(message) {
  process.stdout.write(JSON.stringify(message) + "\n");
}

input.on("line", (line) => {
  const message = JSON.parse(line);
  if (message.method === "initialize") {
    send({ id: message.id, result: { userAgent: "mock-ducx" } });
  } else if (message.method === "config/read") {
    const layers =
      mode === "external-config"
        ? [{ name: { type: "user", file: "/Users/example/.baidu-cx/config.toml" }, version: "1", config: {} }]
        : [];
    send({ id: message.id, result: { config: {}, origins: {}, layers } });
  } else if (message.method === "hooks/list") {
    const hooks =
      mode === "hook"
        ? [{
            eventName: "sessionStart",
            sourcePath: "/Users/example/.baidu-cx/hooks.json",
          }]
        : [];
    send({
      id: message.id,
      result: {
        data: [{
          cwd: message.params.cwds[0],
          errors: [],
          hooks,
          warnings: [],
        }],
      },
    });
  } else if (message.method === "thread/start") {
    send({ id: message.id, result: { thread: { id: "thread_mock" } } });
  } else if (message.method === "turn/start") {
    send({
      id: message.id,
      result: {
        turn: { id: "turn_mock", items: [], status: "inProgress" },
      },
    });
    send({
      method: "turn/completed",
      params: {
        threadId: message.params.threadId,
        turn: { id: "turn_mock", items: [], status: "completed" },
      },
    });
  }
});
`;

async function runProbe(mode, { runTurn = false } = {}) {
  const directory = await mkdtemp(path.join(os.tmpdir(), "ducx-probe-test-"));
  const mockPath = path.join(directory, "mock-ducx.mjs");
  await writeFile(mockPath, MOCK_SERVER);
  await chmod(mockPath, 0o700);

  try {
    const child = spawn(process.execPath, [probePath], {
      cwd: scriptDirectory,
      env: {
        ...process.env,
        DUCX_BIN: mockPath,
        MOCK_DUCX_MODE: mode,
        DUCX_PROBE_RUN_TURN: runTurn ? "1" : "0",
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    const exitCode = await new Promise((resolve) => child.once("exit", resolve));
    return { exitCode, stdout, stderr };
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

test("accepts an isolated app-server", async () => {
  const result = await runProbe("safe");
  assert.equal(result.exitCode, 0, result.stderr);
  const summary = JSON.parse(result.stdout);
  assert.equal(summary.ok, true);
  assert.equal(summary.discoveredHooks, 0);
  assert.equal(summary.emittedHookNotifications, 0);
  assert.equal(summary.threadId, "thread_mock");
  assert.equal(summary.protectedFilesUnchanged, true);
});

test("rejects a user config layer outside the isolated home", async () => {
  const result = await runProbe("external-config");
  assert.equal(result.exitCode, 1);
  assert.match(result.stderr, /loaded config outside isolated root/);
});

test("rejects discovered hooks before starting a thread", async () => {
  const result = await runProbe("hook");
  assert.equal(result.exitCode, 1);
  assert.match(result.stderr, /discovered hooks in isolated mode/);
});

test("starts a turn and waits for completion", async () => {
  const result = await runProbe("safe", { runTurn: true });
  assert.equal(result.exitCode, 0, result.stderr);
  const summary = JSON.parse(result.stdout);
  assert.equal(summary.turnStatus, "completed");
  assert.ok(summary.notificationMethods.includes("turn/completed"));
});
