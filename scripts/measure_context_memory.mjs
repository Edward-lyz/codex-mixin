// Isolated CLI memory baseline for long Responses histories. The local
// provider emits one event and holds each request so vmmap samples live turns.
import fs from "node:fs";
import { randomUUID } from "node:crypto";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";

const repo = process.cwd();
const binary = process.env.EVAL_BINARY ?? path.join(repo, "target/release/codex-mixin");
const transport = process.env.MEMORY_TRANSPORT ?? "http";
const concurrency = (process.env.MEMORY_CONCURRENCY ?? "1,4,8").split(",").map((value) => Number(value.trim()));
const count = Number(process.env.MEMORY_MESSAGES ?? 1002);
const chars = Number(process.env.MEMORY_MESSAGE_CHARS ?? 3900);
const holdMs = Number(process.env.MEMORY_HOLD_MS ?? 30000);
const gatewayKey = "isolated-gateway-key";
const providerKey = "isolated-provider-key";
const model = "memory-model-memory";
const temp = fs.mkdtempSync(path.join(os.tmpdir(), "codex-mixin-memory-"));
let provider;
let gateway;
let providerPort;
let gatewayPort;
let active = new Set();
let gatewaySpawnError;
const webSockets = new Set();

function isolatedEnv() {
  const home = path.join(temp, "home");
  for (const dir of [home, path.join(temp, "codex"), path.join(temp, "xdg-config"),
    path.join(temp, "xdg-data"), path.join(temp, "xdg-state"), path.join(temp, "xdg-cache")]) {
    fs.mkdirSync(dir, { recursive: true });
  }
  return {
    ...process.env,
    HOME: home,
    CODEX_HOME: path.join(temp, "codex"),
    DSH_HOME: path.join(home, ".dsh"),
    CLAUDE_CONFIG_DIR: path.join(home, ".claude"),
    XDG_CONFIG_HOME: path.join(temp, "xdg-config"),
    XDG_DATA_HOME: path.join(temp, "xdg-data"),
    XDG_STATE_HOME: path.join(temp, "xdg-state"),
    XDG_CACHE_HOME: path.join(temp, "xdg-cache"),
    CODEX_GATEWAY_CONFIG: path.join(temp, "config.json"),
    CODEX_GATEWAY_RUNTIME_FILE: path.join(temp, "runtime.json"),
    CODEX_GATEWAY_DAEMON_FILE: path.join(temp, "daemon.json"),
    CODEX_GATEWAY_SERVICE_ID: `memory-${process.pid}-${path.basename(temp)}`,
    CODEX_MIXIN_CONTEXT_DB: path.join(temp, "context.db"),
    HTTP_PROXY: "http://127.0.0.1:9",
    HTTPS_PROXY: "http://127.0.0.1:9",
    http_proxy: "http://127.0.0.1:9",
    https_proxy: "http://127.0.0.1:9",
    NO_PROXY: "127.0.0.1,localhost",
    no_proxy: "127.0.0.1,localhost",
  };
}

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.removeListener("error", reject);
      resolve(server.address().port);
    });
  });
}

async function readJson(request) {
  let raw = "";
  request.setEncoding("utf8");
  for await (const chunk of request) raw += chunk;
  return JSON.parse(raw);
}

function emit(response, type, payload) {
  response.write(`event: ${type}\ndata: ${JSON.stringify(payload)}\n\n`);
}

async function startProvider() {
  provider = http.createServer(async (request, response) => {
    if (request.method !== "POST" || request.url !== "/v1/responses") {
      response.writeHead(404).end();
      return;
    }
    if (request.headers.authorization !== `Bearer ${providerKey}`) {
      response.writeHead(401).end();
      return;
    }
    let body;
    try { body = await readJson(request); } catch {
      response.writeHead(400).end();
      return;
    }
    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    response.flushHeaders();
    const id = `mem_${Date.now()}_${Math.random().toString(16).slice(2)}`;
    emit(response, "response.created", { type: "response.created", response: { id, status: "in_progress" } });
    const entry = { response, id, timer: null };
    active.add(entry);
    entry.timer = setTimeout(() => finish(entry), holdMs);
    response.on("close", () => {
      if (active.delete(entry)) clearTimeout(entry.timer);
    });
  });
  providerPort = await listen(provider);
}

function finish(entry) {
  if (!active.delete(entry)) return;
  clearTimeout(entry.timer);
  emit(entry.response, "response.completed", {
    type: "response.completed",
    response: {
      id: entry.id,
      object: "response",
      status: "completed",
      model,
      output: [{
        type: "message",
        id: `out_${entry.id}`,
        role: "assistant",
        status: "completed",
        content: [{ type: "output_text", text: "ok" }],
      }],
      usage: { input_tokens: 1000, output_tokens: 1 },
    },
  });
  entry.response.end();
}

async function reservePort() {
  const server = net.createServer();
  const port = await listen(server);
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  return port;
}

function saveConfig() {
  const config = {
    config_version: 2,
    gateway_bind: `127.0.0.1:${gatewayPort}`,
    ...(transport === "http" ? { gateway_api_key: gatewayKey } : {}),
    context: {
      enabled: true,
      growth_threshold_tokens: 100,
      pressure_ratio: 0.75,
      urgent_ratio: 0.95,
      protected_messages: 1,
      protected_tokens: 1,
      max_internal_rounds: 10,
      max_tier: 1,
      guidance_only: false,
      context_window_tokens: 128000,
    },
    providers: [{
      id: "memory",
      display_name: "Local memory benchmark",
      enabled: true,
      protocol: "open_ai_responses",
      base_url: `http://127.0.0.1:${providerPort}`,
      api_path: "/v1/responses",
      model_source: { kind: "static" },
      auth: { header: "authorization_bearer", api_key: providerKey },
      selected_models: ["memory-model"],
      cached_models: [{ id: "memory-model", context_window: 128000, protocol: "open_ai_responses", api_path: "/v1/responses" }],
    }],
  };
  fs.writeFileSync(path.join(temp, "config.json"), JSON.stringify(config));
}

async function startGateway() {
  gatewaySpawnError = null;
  gateway = spawn(binary, ["serve", "--no-client-sync"], {
    cwd: repo,
    env: isolatedEnv(),
    stdio: "ignore",
  });
  gateway.once("error", (error) => { gatewaySpawnError = error; });
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    if (gatewaySpawnError) throw new Error(`failed to launch gateway: ${gatewaySpawnError.message}`);
    if (gateway.exitCode !== null) throw new Error(`gateway exited early with ${gateway.exitCode}`);
    try {
      const response = await fetch(`http://127.0.0.1:${gatewayPort}/v1/models`, {
        headers: { authorization: `Bearer ${gatewayKey}` },
      });
      if (response.ok) return;
    } catch { /* listener not ready */ }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("gateway readiness timed out");
}

function requestInput() {
  const sample = "transcript fact: diagnostic observed during build step. ";
  const text = sample.repeat(Math.ceil(chars / sample.length)).slice(0, chars);
  const input = [{ type: "message", role: "developer", content: [{ type: "input_text", text: "Memory benchmark." }] }];
  for (let i = 1; i < count; i += 1) {
    input.push({
      type: "message",
      role: i % 2 ? "user" : "assistant",
      content: [{ type: i % 2 ? "input_text" : "output_text", text: `${i}: ${text}` }],
    });
  }
  return input;
}

async function beginRequest(input, session) {
  const response = await fetch(`http://127.0.0.1:${gatewayPort}/v1/responses`, {
    method: "POST",
    headers: { authorization: `Bearer ${gatewayKey}`, "content-type": "application/json", "x-session-id": session },
    body: JSON.stringify({ model, stream: true, input }),
  });
  if (!response.ok || !response.body) throw new Error(`gateway returned HTTP ${response.status}`);
  const reader = response.body.getReader();
  const first = await reader.read();
  if (first.done) throw new Error("response stream ended before first event");
  return { reader, first: first.value };
}

async function beginWebSocketRequest(serializedRequest) {
  const socket = new WebSocket(`ws://127.0.0.1:${gatewayPort}/v1/responses`);
  webSockets.add(socket);
  await new Promise((resolve, reject) => {
    const onOpen = () => {
      socket.removeEventListener("error", onError);
      resolve();
    };
    const onError = () => {
      socket.removeEventListener("open", onOpen);
      reject(new Error("gateway WebSocket connection failed"));
    };
    socket.addEventListener("open", onOpen, { once: true });
    socket.addEventListener("error", onError, { once: true });
  });
  const completed = new Promise((resolve, reject) => {
    const cleanup = () => {
      socket.removeEventListener("message", onMessage);
      socket.removeEventListener("error", onError);
      socket.removeEventListener("close", onClose);
    };
    const onMessage = (event) => {
      let payload;
      try { payload = JSON.parse(String(event.data)); } catch {
        cleanup();
        reject(new Error("gateway returned a non-JSON WebSocket event"));
        return;
      }
      if (payload.type === "response.completed") {
        cleanup();
        resolve();
      } else if (payload.type === "response.failed" || payload.type === "error") {
        cleanup();
        reject(new Error(`gateway WebSocket response failed: ${payload.type}`));
      }
    };
    const onError = () => {
      cleanup();
      reject(new Error("gateway WebSocket request failed"));
    };
    const onClose = () => {
      cleanup();
      reject(new Error("gateway WebSocket closed before response.completed"));
    };
    socket.addEventListener("message", onMessage);
    socket.addEventListener("error", onError, { once: true });
    socket.addEventListener("close", onClose, { once: true });
    socket.send(serializedRequest);
  });
  return { socket, completed };
}

async function closeWebSocket(socket) {
  if (socket.readyState === WebSocket.CLOSED) {
    webSockets.delete(socket);
    return;
  }
  await new Promise((resolve) => {
    const timer = setTimeout(resolve, 3000);
    socket.addEventListener("close", () => {
      clearTimeout(timer);
      resolve();
    }, { once: true });
    if (socket.readyState === WebSocket.OPEN) socket.close(1000, "memory benchmark complete");
  });
  webSockets.delete(socket);
}

async function completeRequest(turn) {
  let data = new TextDecoder().decode(turn.first);
  for (;;) {
    const result = await turn.reader.read();
    if (result.done) break;
    data += new TextDecoder().decode(result.value);
  }
  if (!data.includes("response.completed")) throw new Error("missing response.completed");
}

function bytes(value) {
  const match = value.match(/^([\d.]+)([KMG])?$/i);
  if (!match) return null;
  const multiplier = { K: 1024, M: 1024 ** 2, G: 1024 ** 3 }[(match[2] ?? "").toUpperCase()] ?? 1;
  return Math.round(Number(match[1]) * multiplier);
}

function sampleMemory(pid) {
  const result = spawnSync("vmmap", ["-summary", String(pid)], { encoding: "utf8", maxBuffer: 4 * 1024 * 1024 });
  if (result.status !== 0) throw new Error("vmmap failed for the isolated gateway child");
  const footprint = result.stdout.match(/^Physical footprint:\s+(\S+)/m)?.[1];
  const peak = result.stdout.match(/^Physical footprint \(peak\):\s+(\S+)/m)?.[1];
  const mallocTable = result.stdout.slice(result.stdout.indexOf("\nMALLOC ZONE"));
  const allocated = mallocTable.match(/^TOTAL\s+\S+\s+\S+\s+\S+\s+\S+\s+\S+\s+(\S+)/m)?.[1];
  if (!footprint || !peak || !allocated) throw new Error("unrecognized vmmap summary");
  return { footprint_bytes: bytes(footprint), allocated_bytes: bytes(allocated), peak_bytes: bytes(peak) };
}

async function runLevel(n) {
  await stopGateway();
  for (const suffix of ["", "-wal", "-shm"]) {
    fs.rmSync(path.join(temp, `context.db${suffix}`), { force: true });
  }
  await startGateway();
  const idle = sampleMemory(gateway.pid);
  const input = requestInput();
  const sessions = Array.from({ length: n }, (_, i) => `memory-${n}-${i}-${Date.now()}`);
  const turns = await Promise.all(sessions.map((session) => beginRequest(input, session)));
  const deadline = Date.now() + 10000;
  while (active.size < n && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 20));
  if (active.size < n) throw new Error(`only ${active.size}/${n} upstream requests are being held`);
  const samples = [];
  for (let i = 0; i < 5; i += 1) {
    samples.push(sampleMemory(gateway.pid));
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  const live = {
    footprint_bytes: Math.max(...samples.map((sample) => sample.footprint_bytes)),
    allocated_bytes: Math.max(...samples.map((sample) => sample.allocated_bytes)),
    peak_bytes: Math.max(...samples.map((sample) => sample.peak_bytes)),
    samples: samples.length,
  };
  if (active.size !== n) throw new Error(`upstream hold expired during sampling (${active.size}/${n} remain)`);
  for (const entry of [...active]) finish(entry);
  await Promise.all(turns.map(completeRequest));
  await new Promise((resolve) => setTimeout(resolve, 500));
  const post = sampleMemory(gateway.pid);
  return {
    concurrency: n,
    history_items: input.length,
    history_json_bytes: Buffer.byteLength(JSON.stringify(input)),
    active: live,
    idle,
    post,
  };
}

async function runWebSocketLevel(n) {
  await stopGateway();
  for (const suffix of ["", "-wal", "-shm"]) {
    fs.rmSync(path.join(temp, `context.db${suffix}`), { force: true });
  }
  await startGateway();
  const idle = sampleMemory(gateway.pid);
  const input = requestInput();
  const turns = await Promise.all(Array.from({ length: n }, (_, i) => {
    const serializedRequest = JSON.stringify({
      model,
      stream: true,
      prompt_cache_key: `memory-ws-${n}-${i}-${randomUUID()}`,
      input,
    });
    return beginWebSocketRequest(serializedRequest);
  }));
  const deadline = Date.now() + 10000;
  while (active.size < n && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 20));
  if (active.size < n) throw new Error(`only ${active.size}/${n} WebSocket upstream requests are being held`);
  const activeSamples = [];
  for (let i = 0; i < 5; i += 1) {
    activeSamples.push(sampleMemory(gateway.pid));
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  const activeUpstream = {
    footprint_bytes: Math.max(...activeSamples.map((sample) => sample.footprint_bytes)),
    allocated_bytes: Math.max(...activeSamples.map((sample) => sample.allocated_bytes)),
    peak_bytes: Math.max(...activeSamples.map((sample) => sample.peak_bytes)),
    samples: activeSamples.length,
  };
  if (active.size !== n) throw new Error(`upstream hold expired during sampling (${active.size}/${n} remain)`);
  for (const entry of [...active]) finish(entry);
  await Promise.all(turns.map((turn) => turn.completed));
  await new Promise((resolve) => setTimeout(resolve, 500));
  const openSamples = [];
  for (let i = 0; i < 5; i += 1) {
    openSamples.push(sampleMemory(gateway.pid));
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  const postOpen = {
    footprint_bytes: Math.max(...openSamples.map((sample) => sample.footprint_bytes)),
    allocated_bytes: Math.max(...openSamples.map((sample) => sample.allocated_bytes)),
    peak_bytes: Math.max(...openSamples.map((sample) => sample.peak_bytes)),
    samples: openSamples.length,
  };
  const sessionCountResult = spawnSync("sqlite3", ["-readonly", path.join(temp, "context.db"), "SELECT count(*) FROM sessions;"], {
    encoding: "utf8",
  });
  if (sessionCountResult.status !== 0) throw new Error("failed to inspect isolated context sessions");
  const contextSessions = Number(sessionCountResult.stdout.trim());
  if (contextSessions !== n) throw new Error(`expected ${n} context sessions but found ${contextSessions}`);
  await Promise.all(turns.map((turn) => closeWebSocket(turn.socket)));
  await new Promise((resolve) => setTimeout(resolve, 500));
  const postClosed = sampleMemory(gateway.pid);
  return {
    concurrency: n,
    history_items: input.length,
    history_json_bytes: Buffer.byteLength(JSON.stringify(input)),
    context_sessions: contextSessions,
    active: activeUpstream,
    idle,
    postOpen,
    postClosed,
  };
}

async function stopGateway() {
  if (!gateway || !gateway.pid || gateway.exitCode !== null) return;
  gateway.kill("SIGTERM");
  const deadline = Date.now() + 5000;
  while (gateway.exitCode === null && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 50));
  if (gateway.exitCode === null) {
    gateway.kill("SIGKILL");
    await new Promise((resolve) => {
      if (gateway.exitCode !== null) resolve();
      else gateway.once("exit", resolve);
    });
  }
}

let failure;
try {
  if (process.platform !== "darwin") throw new Error("this script requires macOS vmmap");
  if (!fs.existsSync(binary)) throw new Error(`binary not found: ${binary}`);
  if (transport !== "http" && transport !== "ws") throw new Error("MEMORY_TRANSPORT must be http or ws");
  if (!concurrency.length || concurrency.some((value) => !Number.isInteger(value) || value < 1)) {
    throw new Error("MEMORY_CONCURRENCY must be a comma-separated list of positive integers");
  }
  if (!Number.isInteger(count) || count < 1) throw new Error("MEMORY_MESSAGES must be a positive integer");
  if (!Number.isInteger(chars) || chars < 0) throw new Error("MEMORY_MESSAGE_CHARS must be a non-negative integer");
  if (!Number.isFinite(holdMs) || holdMs < 1000) throw new Error("MEMORY_HOLD_MS must be at least 1000");
  if (transport === "ws" && typeof WebSocket !== "function") throw new Error("this Node.js runtime does not provide a native WebSocket");
  await startProvider();
  gatewayPort = await reservePort();
  saveConfig();
  const results = [];
  for (const n of concurrency) results.push(await (transport === "ws" ? runWebSocketLevel(n) : runLevel(n)));
  process.stdout.write(`${JSON.stringify({ binary: path.resolve(binary), transport, generated_at: new Date().toISOString(), results }, null, 2)}\n`);
} catch (error) {
  failure = error;
} finally {
  await Promise.all([...webSockets].map(closeWebSocket));
  for (const entry of [...active]) finish(entry);
  await stopGateway();
  if (provider) await new Promise((resolve) => provider.close(resolve));
  fs.rmSync(temp, { recursive: true, force: true });
}
if (failure) {
  process.stderr.write(`context memory measurement failed: ${failure.message}\n`);
  process.exitCode = 1;
}
