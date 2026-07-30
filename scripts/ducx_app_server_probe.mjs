#!/usr/bin/env node

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { realpathSync } from "node:fs";
import {
  chmod,
  copyFile,
  mkdtemp,
  mkdir,
  readFile,
  readdir,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";

const DEFAULT_DUCX_PATHS = [
  path.join(os.homedir(), ".baidu-cx", "baidu-cx", "bin", "ducx"),
  path.join(os.homedir(), ".baidu-cx", "baidu-cx", "bin", "codex"),
];
const REQUEST_TIMEOUT_MS = 10_000;

function resolveDucxPath() {
  return process.env.DUCX_BIN || DEFAULT_DUCX_PATHS[0];
}

async function fingerprint(filePath) {
  try {
    const [metadata, bytes] = await Promise.all([stat(filePath), readFile(filePath)]);
    return {
      exists: true,
      size: metadata.size,
      modifiedMs: metadata.mtimeMs,
      sha256: createHash("sha256").update(bytes).digest("hex"),
    };
  } catch (error) {
    if (error?.code === "ENOENT") {
      return { exists: false };
    }
    throw error;
  }
}

async function fingerprintAll(paths) {
  return Object.fromEntries(
    await Promise.all(paths.map(async (filePath) => [filePath, await fingerprint(filePath)])),
  );
}

async function seedDucxIdentity(isolatedHome, codexHome) {
  const installedHome = path.join(os.homedir(), ".baidu-cx");
  const copied = [];
  for (const name of ["user.json", "installation_id"]) {
    const source = path.join(installedHome, name);
    if (!(await fingerprint(source)).exists) {
      continue;
    }
    const destination = path.join(codexHome, name);
    await copyFile(source, destination);
    await chmod(destination, 0o600);
    copied.push(destination);
  }
  const legacyUsers = path.join(os.homedir(), ".comate", "login-user");
  let usernames = [];
  try {
    usernames = await readdir(legacyUsers);
  } catch (error) {
    if (error?.code !== "ENOENT") {
      throw error;
    }
  }
  if (usernames.length === 1) {
    const username = usernames[0];
    const legacyHome = path.join(isolatedHome, ".comate");
    const loginUsers = path.join(legacyHome, "login-user");
    await mkdir(loginUsers, { recursive: true, mode: 0o700 });
    const token = path.join(loginUsers, username);
    const login = path.join(legacyHome, "login");
    await copyFile(path.join(legacyUsers, username), token);
    await writeFile(login, username, { mode: 0o600 });
    await chmod(token, 0o600);
    copied.push(token, login);
  }
  return copied;
}

function stableJson(value) {
  return JSON.stringify(value, Object.keys(value).sort());
}

function assertUnchanged(before, after) {
  for (const [filePath, previous] of Object.entries(before)) {
    const current = after[filePath];
    if (stableJson(previous) !== stableJson(current)) {
      throw new Error(`DUCX changed protected file: ${filePath}`);
    }
  }
}

function configLayerPath(layer) {
  const source = layer?.name;
  return source?.file || source?.dotCodexFolder || null;
}

function isInside(candidate, root) {
  const canonical = (filePath) => {
    try {
      return realpathSync(filePath);
    } catch {
      return path.resolve(filePath);
    }
  };
  const relative = path.relative(canonical(root), canonical(candidate));
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function assertIsolatedConfig(layers, allowedRoots, allowedFiles = []) {
  const escapedLayers = (layers || []).filter((layer) => {
    const sourcePath = configLayerPath(layer);
    const sourceType = layer?.name?.type;
    const hostManaged =
      sourceType === "system" ||
      sourceType === "mdm" ||
      sourceType === "enterpriseManaged" ||
      sourceType === "legacyManagedConfigTomlFromFile" ||
      sourceType === "legacyManagedConfigTomlFromMdm";
    return (
      sourcePath &&
      !hostManaged &&
      !allowedFiles.some(
        (allowedFile) => path.resolve(sourcePath) === path.resolve(allowedFile),
      ) &&
      !allowedRoots.some((root) => isInside(sourcePath, root))
    );
  });
  if (escapedLayers.length > 0) {
    const details = escapedLayers
      .map((layer) => `${JSON.stringify(layer.name)} (${configLayerPath(layer)})`)
      .join(", ");
    throw new Error(`DUCX loaded config outside isolated root: ${details}`);
  }
}

function assertNoHooks(response) {
  const entries = response?.data || [];
  const hooks = entries.flatMap((entry) => entry.hooks || []);
  if (hooks.length > 0) {
    const details = hooks
      .map((hook) => `${hook.eventName}:${hook.sourcePath}`)
      .join(", ");
    throw new Error(`DUCX discovered hooks in isolated mode: ${details}`);
  }
}

class AppServerClient {
  constructor(child) {
    this.child = child;
    this.nextId = 1;
    this.pending = new Map();
    this.notifications = [];
    this.notificationWaiters = [];
    this.stderr = [];

    const stdout = readline.createInterface({ input: child.stdout });
    stdout.on("line", (line) => this.#handleLine(line));
    const stderr = readline.createInterface({ input: child.stderr });
    stderr.on("line", (line) => this.stderr.push(line));
    child.once("exit", (code, signal) => {
      const reason = `DUCX app-server exited (code=${code}, signal=${signal})`;
      for (const pending of this.pending.values()) {
        clearTimeout(pending.timer);
        pending.reject(new Error(reason));
      }
      this.pending.clear();
    });
  }

  #handleLine(line) {
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      this.stderr.push(`non-JSON stdout: ${line}`);
      return;
    }
    if (
      message.id !== undefined &&
      message.method === "item/tool/call" &&
      !this.pending.has(message.id)
    ) {
      this.child.stdin.write(
        `${JSON.stringify({
          id: message.id,
          result: {
            contentItems: [
              {
                type: "inputText",
                text: "The protocol probe does not execute tools.",
              },
            ],
            success: false,
          },
        })}\n`,
      );
      return;
    }
    if (message.id !== undefined && this.pending.has(message.id)) {
      const pending = this.pending.get(message.id);
      this.pending.delete(message.id);
      clearTimeout(pending.timer);
      if (message.error) {
        pending.reject(new Error(`${pending.method}: ${JSON.stringify(message.error)}`));
      } else {
        pending.resolve(message.result);
      }
      return;
    }
    this.notifications.push(message);
    for (const waiter of [...this.notificationWaiters]) {
      if (waiter.predicate(message)) {
        this.notificationWaiters.splice(this.notificationWaiters.indexOf(waiter), 1);
        clearTimeout(waiter.timer);
        waiter.resolve(message);
      }
    }
  }

  notify(method, params = {}) {
    this.child.stdin.write(`${JSON.stringify({ method, params })}\n`);
  }

  request(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out after ${REQUEST_TIMEOUT_MS}ms`));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, { method, resolve, reject, timer });
      this.child.stdin.write(`${JSON.stringify({ id, method, params })}\n`);
    });
  }

  waitForNotification(predicate, timeoutMs = 60_000) {
    const existing = this.notifications.find(predicate);
    if (existing) {
      return Promise.resolve(existing);
    }
    return new Promise((resolve, reject) => {
      const waiter = {
        predicate,
        resolve,
        reject,
        timer: setTimeout(() => {
          this.notificationWaiters.splice(
            this.notificationWaiters.indexOf(waiter),
            1,
          );
          reject(new Error(`notification timed out after ${timeoutMs}ms`));
        }, timeoutMs),
      };
      this.notificationWaiters.push(waiter);
    });
  }
}

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }
  child.kill("SIGTERM");
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    new Promise((resolve) =>
      setTimeout(() => {
        child.kill("SIGKILL");
        resolve();
      }, 2_000),
    ),
  ]);
}

async function main() {
  const sandboxRoot = await mkdtemp(path.join(os.tmpdir(), "codex-mixin-ducx-"));
  const requestedHome = process.env.DUCX_ISOLATED_HOME;
  const isolatedHome = path.resolve(requestedHome || path.join(sandboxRoot, "home"));
  if (isolatedHome === path.resolve(os.homedir())) {
    throw new Error("DUCX_ISOLATED_HOME must not be the real user home");
  }
  const codexHome = path.join(isolatedHome, ".codex");
  const workspace = path.join(sandboxRoot, "workspace");
  await mkdir(isolatedHome, { recursive: true, mode: 0o700 });
  await Promise.all([
    mkdir(codexHome, { recursive: true, mode: 0o700 }),
    mkdir(workspace),
  ]);
  const seededIdentityPaths = await seedDucxIdentity(isolatedHome, codexHome);

  const protectedPaths = [
    path.join(os.homedir(), ".codex", "config.toml"),
    path.join(os.homedir(), ".baidu-cx", "config.toml"),
    path.join(os.homedir(), ".baidu-cx", "hooks.json"),
    path.join(os.homedir(), ".baidu-cx", "reportlog", "data-report.log"),
    path.join(os.homedir(), ".comate", ".baidu-cx", "config.toml"),
    path.join(os.homedir(), ".comate", ".baidu-cx", "hooks.json"),
    path.join(os.homedir(), ".comate", "login"),
  ];
  const before = await fingerprintAll(protectedPaths);
  const disableModelProxy = process.env.DUCX_PROBE_DISABLE_MODEL_PROXY === "1";
  const installedConfig = path.join(os.homedir(), ".baidu-cx", "config.toml");
  const allowInstalledConfig =
    process.env.DUCX_PROBE_ALLOW_INSTALLED_CONFIG === "1";
  const allowDiscoveredHooks =
    process.env.DUCX_PROBE_ALLOW_DISCOVERED_HOOKS === "1";
  const hookEvents = [
    "preToolUse",
    "permissionRequest",
    "postToolUse",
    "preCompact",
    "postCompact",
    "sessionStart",
    "sessionEnd",
    "userPromptSubmit",
    "subagentStart",
    "subagentStop",
    "stop",
  ];
  const disabledFeatures = [
    "apps",
    "browser_use",
    "browser_use_external",
    "browser_use_full_cdp_access",
    "code_mode_host",
    "computer_use",
    "goals",
    "hooks",
    "image_generation",
    "in_app_browser",
    "multi_agent",
    "plugin_sharing",
    "plugins",
    "remote_plugin",
    "shell_tool",
    "skill_mcp_dependency_install",
    "skill_search",
    "tool_call_mcp_elicitation",
    "tool_suggest",
    "unified_exec",
    "workspace_dependencies",
  ];
  const args = [
    ...(disableModelProxy ? ["--disable-model-proxy"] : []),
    ...disabledFeatures.flatMap((feature) => ["--disable", feature]),
    "app-server",
    "--listen",
    "stdio://",
    "-c",
    'history.persistence="none"',
    "-c",
    "analytics.enabled=false",
    "-c",
    "feedback.enabled=false",
    "-c",
    "features.shell_tool=false",
    "-c",
    "agents.enabled=false",
    "-c",
    "apps._default.enabled=false",
    "-c",
    'web_search="disabled"',
    "-c",
    "project_doc_max_bytes=0",
    "-c",
    "project_doc_fallback_filenames=[]",
    ...hookEvents.flatMap((eventName) => [
      "-c",
      `hooks.${eventName}=[]`,
    ]),
  ];
  const child = spawn(
    resolveDucxPath(),
    args,
    {
      cwd: workspace,
      env: {
        ...process.env,
        CODEX_HOME: codexHome,
        HOME: isolatedHome,
      },
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
  const client = new AppServerClient(child);
  let succeeded = false;

  try {
    const initialize = await client.request("initialize", {
      clientInfo: {
        name: "codex_mixin_ducx_probe",
        title: "Codex Mixin DUCX Probe",
        version: "0.1.0",
      },
      capabilities: { experimentalApi: true },
    });
    client.notify("initialized");

    const effectiveConfig = await client.request("config/read", {
      cwd: workspace,
      includeLayers: true,
    });
    assertIsolatedConfig(
      effectiveConfig.layers,
      [sandboxRoot, isolatedHome],
      allowInstalledConfig ? [installedConfig] : [],
    );

    const hooks = await client.request("hooks/list", { cwds: [workspace] });
    if (!allowDiscoveredHooks) {
      assertNoHooks(hooks);
    }
    const discoveredHooks = (hooks?.data || []).flatMap(
      (entry) => entry.hooks || [],
    );

    const thread = await client.request("thread/start", {
      approvalPolicy: "never",
      model: process.env.DUCX_PROBE_MODEL || "gpt-5.6-luna",
      modelProvider: "oneapi",
      baseInstructions: "",
      developerInstructions: "",
      dynamicTools: [],
      config: {
        hooks: Object.fromEntries(
          hookEvents.map((eventName) => [eventName, []]),
        ),
        mcp_servers: {},
      },
      environments: [],
      ephemeral: true,
      experimentalRawEvents: true,
      cwd: workspace,
      runtimeWorkspaceRoots: [],
      sandbox: "read-only",
    });

    let turnStatus = null;
    if (process.env.DUCX_PROBE_RUN_TURN === "1") {
      await client.request("turn/start", {
        threadId: thread.thread.id,
        input: [
          {
            type: "text",
            text: "Reply with exactly DUCX_PROBE_OK. Do not call tools.",
          },
        ],
        approvalPolicy: "never",
        cwd: workspace,
        environments: [],
        runtimeWorkspaceRoots: [],
        sandboxPolicy: {
          type: "readOnly",
          networkAccess: false,
        },
      });
      const completed = await client.waitForNotification(
        (message) =>
          message.method === "turn/completed" &&
          message.params?.threadId === thread.thread.id,
      );
      turnStatus = completed.params?.turn?.status || null;
    } else {
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    const hookNotifications = client.notifications.filter((message) =>
      String(message.method || "").toLowerCase().includes("hook"),
    );
    if (hookNotifications.length > 0) {
      throw new Error(
        `DUCX emitted hook notifications in isolated mode: ${hookNotifications
          .map((message) => message.method)
          .join(", ")}`,
      );
    }
    const problemNotifications = client.notifications
      .filter((message) => ["error", "warning"].includes(message.method))
      .map((message) => ({
        method: message.method,
        message:
          message.params?.message ||
          message.params?.error?.message ||
          message.params?.error ||
          null,
      }));

    succeeded = true;
    process.stdout.write(
      `${JSON.stringify(
        {
          ok: true,
          ducx: resolveDucxPath(),
          modelProxyDisabled: disableModelProxy,
          installedConfigReadOnly: allowInstalledConfig,
          isolatedHome,
          persistentIsolatedHome: Boolean(requestedHome),
          isolatedCodexHome: codexHome,
          isolatedWorkspace: workspace,
          initialized: Boolean(initialize),
          configLayers: (effectiveConfig.layers || []).map((layer) => layer.name),
          discoveredHooks: discoveredHooks.length,
          emittedHookNotifications: 0,
          disabledFeatures,
          threadId: thread?.thread?.id || null,
          turnStatus,
          problems: problemNotifications,
          notificationMethods: [
            ...new Set(client.notifications.map((message) => message.method).filter(Boolean)),
          ],
          protectedFilesUnchanged: true,
        },
        null,
        2,
      )}\n`,
    );
  } finally {
    await stopChild(child);
    await Promise.all(
      seededIdentityPaths.map((filePath) => rm(filePath, { force: true })),
    );
    const after = await fingerprintAll(protectedPaths);
    let protectedFileError = null;
    try {
      assertUnchanged(before, after);
    } catch (error) {
      protectedFileError = error;
      succeeded = false;
    }
    if (succeeded && process.env.DUCX_PROBE_KEEP_TEMP !== "1") {
      await rm(sandboxRoot, { recursive: true, force: true });
    } else if (!succeeded) {
      if (client.stderr.length > 0) {
        process.stderr.write(`DUCX stderr:\n${client.stderr.join("\n")}\n`);
      }
      process.stderr.write(`DUCX probe artifacts kept at ${sandboxRoot}\n`);
    }
    if (protectedFileError) {
      throw protectedFileError;
    }
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack || error}\n`);
  process.exitCode = 1;
});
