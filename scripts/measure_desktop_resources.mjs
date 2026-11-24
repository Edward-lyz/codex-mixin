// Replay the shared long-context corpus through isolated desktop gateways.
// The JSON spec supplies loopback ports, model names, process IDs, paths and
// optional timing overrides. For the launch contract see:
// docs/benchmarks/2026-11-24-desktop-resource-curves/REPLAY.md
import fs from "node:fs";
import http from "node:http";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createInterface } from "node:readline";
import { performance } from "node:perf_hooks";
import { createGzip, gunzipSync } from "node:zlib";

const specPath = process.argv[2];
if (!specPath) throw new Error("usage: node measure_desktop_resources.mjs SPEC.json");
const spec = JSON.parse(fs.readFileSync(specPath, "utf8"));
const products = spec.products;
const names = Object.keys(products);
if (names.length !== 3 || new Set(names).size !== 3) throw new Error("exactly three products are required");
if (!Number.isInteger(spec.upstreamPort) || spec.upstreamPort < 1 || spec.upstreamPort > 65535) throw new Error("invalid upstream port");
for (const product of Object.values(products)) {
  if (!Number.isInteger(product.port) || product.port < 1 || product.port > 65535 || !Number.isInteger(product.uiPid) || product.uiPid < 1 || !Number.isInteger(product.gatewayPid) || product.gatewayPid < 1) throw new Error("invalid product configuration");
}
const allPids = Object.values(products).map(({ uiPid, gatewayPid }) => new Set([uiPid, gatewayPid]));
if (allPids.some((pids, index) => allPids.some((other, i) => i < index && [...pids].some((pid) => other.has(pid))))) throw new Error("product PIDs must be distinct across products");
if (new Set(Object.values(products).map(({ port }) => port)).size !== names.length || Object.values(products).some(({ port }) => port === spec.upstreamPort)) throw new Error("product and upstream ports must be distinct");
for (const { gatewayPid, port } of Object.values(products)) {
  const check = spawnSync("lsof", ["-nP", "-a", "-p", String(gatewayPid), `-iTCP:${port}`, "-sTCP:LISTEN"], { encoding: "utf8" });
  if (check.status !== 0) throw new Error(`gateway PID ${gatewayPid} is not listening on TCP port ${port}: ${check.stderr || "no matching listener"}`);
}
const timing = { warmup: 20000, settle: 10000, window: 20000, gap: 5000, interval: 250, windows: 3, ...spec.timing };
if ([timing.warmup, timing.settle, timing.gap].some((value) => !Number.isInteger(value) || value < 0) || [timing.window, timing.interval, timing.windows].some((value) => !Number.isInteger(value) || value <= 0)) throw new Error("invalid timing configuration");
const levels = spec.levels ?? [0, 2, 4, 8];
if (!levels.includes(0) || levels.some((value) => !Number.isInteger(value) || value < 0)) throw new Error("levels must include idle and nonnegative session counts");
const sha = (data) => createHash("sha256").update(data).digest("hex");
const corpus = JSON.parse(gunzipSync(fs.readFileSync(spec.corpus)));
const images = spec.images.map((file) => fs.readFileSync(file));
const imageHashes = images.map(sha);
const originalRolesAndText = corpus.map((message) => [message.role, message.content.filter((part) => typeof part.text === "string").map((part) => part.text)]);
const originalTextHash = sha(JSON.stringify(originalRolesAndText));
const input = structuredClone(corpus);
input.at(-1).content.push(...images.map((image) => ({ type: "input_image", image_url: `data:image/png;base64,${image.toString("base64")}` })));
const inputBytes = Buffer.from(JSON.stringify(input));
const output = spec.output;
fs.mkdirSync(output, { recursive: true });
if (fs.existsSync(`${output}/results.json`)) throw new Error("results.json already exists");
const rawFile = fs.createWriteStream(`${output}/samples.jsonl.gz`, { fd: fs.openSync(`${output}/samples.jsonl.gz`, "wx") });
const gzip = createGzip();
gzip.pipe(rawFile);
const samples = [];
const windows = [];
const audits = [];
const completed = [];
const errors = [];
const groups = new Map();
const active = new Map(names.map((name) => [name, 0]));
const received = [];
const clientRequests = new Set();
const excludedPids = new Set([...(spec.protectedPids ?? []), process.pid]);
const seenAncestors = new Set();
let ancestor = process.ppid;
while (ancestor > 1 && !seenAncestors.has(ancestor)) {
  seenAncestors.add(ancestor);
  excludedPids.add(ancestor);
  const parent = spawnSync("ps", ["-o", "ppid=", "-p", String(ancestor)], { encoding: "utf8" });
  if (parent.status !== 0 || !/^\s*\d+\s*$/.test(parent.stdout)) break;
  ancestor = Number(parent.stdout.trim());
}
let sampler;
let samplerClosed;
let server;
let phase = "startup";
let stopped = false;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function waitFor(predicate, label, timeout = 180000) {
  const deadline = Date.now() + timeout;
  while (!predicate()) {
    if (errors.length) throw new Error(`${label}: ${errors[0]}`);
    if (Date.now() > deadline) throw new Error(`timeout waiting for ${label}; active=${JSON.stringify(Object.fromEntries(active))}`);
    await sleep(100);
  }
}
function event(response, type, data) {
  response.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
}
function validateInput(body) {
  let matched = 0;
  let extras = 0;
  const imageList = [];
  const matchedText = [];
  for (const message of body.input ?? []) {
    const parts = Array.isArray(message.content) ? message.content : [];
    const text = [message.role, parts.filter((part) => typeof part.text === "string").map((part) => part.text)];
    if (matched < originalRolesAndText.length && JSON.stringify(text) === JSON.stringify(originalRolesAndText[matched])) { matched++; matchedText.push(text); }
    else extras++;
    for (const part of parts) {
      const value = typeof part.image_url === "string" ? part.image_url : part.image_url?.url;
      if (part.type === "input_image" && value?.startsWith("data:")) imageList.push(sha(Buffer.from(value.slice(value.indexOf(",") + 1), "base64")));
    }
  }
  if (matched !== corpus.length || JSON.stringify(imageList) !== JSON.stringify(imageHashes)) throw new Error(`input mismatch: ${matched}/${corpus.length} messages and ${imageList.length} images`);
  return { extras, textHash: sha(JSON.stringify(matchedText)), imageHashes: imageList };
}
function sendInitial(entry) {
  const response = entry.response;
  const id = `resp_${entry.session.replaceAll("-", "_")}`;
  const outputId = `msg_${entry.session.replaceAll("-", "_")}`;
  const base = { id, object: "response", status: "in_progress", model: "benchmark-model", output: [] };
  event(response, "response.created", { response: base });
  event(response, "response.output_item.added", { output_index: 0, item: { id: outputId, type: "message", role: "assistant", status: "in_progress", content: [] } });
  event(response, "response.content_part.added", { item_id: outputId, output_index: 0, content_index: 0, part: { type: "output_text", text: "", annotations: [] } });
  entry.id = id;
  entry.outputId = outputId;
}
function sendDelta(group) {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  const offset = group.sequence++ % alphabet.length;
  const text = (alphabet.slice(offset) + alphabet.slice(0, offset)).repeat(2);
  group.text += text;
  for (const entry of group.entries.values()) event(entry.response, "response.output_text.delta", { item_id: entry.outputId, output_index: 0, content_index: 0, delta: text });
}
function finishGroup(group) {
  clearInterval(group.timer);
  for (const entry of group.entries.values()) {
    const part = { type: "output_text", text: group.text, annotations: [] };
    const item = { id: entry.outputId, type: "message", role: "assistant", status: "completed", content: [part] };
    event(entry.response, "response.output_text.done", { item_id: entry.outputId, output_index: 0, content_index: 0, text: group.text });
    event(entry.response, "response.content_part.done", { item_id: entry.outputId, output_index: 0, content_index: 0, part });
    event(entry.response, "response.output_item.done", { output_index: 0, item });
    event(entry.response, "response.completed", { response: { id: entry.id, object: "response", status: "completed", model: "benchmark-model", output: [item], usage: { input_tokens: 1000000, output_tokens: group.sequence * 32 } } });
    entry.finished = true;
    entry.response.end();
    active.set(entry.product, active.get(entry.product) - 1);
  }
}
function beginUpstream() {
  server = http.createServer(async (request, response) => {
    try {
      if (request.method !== "POST" || request.url !== "/v1/responses") {
        response.writeHead(404).end();
        return;
      }
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      if (!chunks.length) throw new Error(`empty body for ${request.method} ${request.url}`);
      const body = JSON.parse(Buffer.concat(chunks));
      const meta = body.metadata;
      if (!meta?.benchmark_session || !names.includes(meta?.benchmark_product)) {
        response.writeHead(200, { "content-type": "text/event-stream" });
        event(response, "response.completed", { response: { id: "capability_probe", object: "response", status: "completed", model: "benchmark-model", output: [], usage: { input_tokens: 1, output_tokens: 1 } } });
        response.end();
        return;
      }
      const { extras, textHash, imageHashes: receivedImageHashes } = validateInput(body);
      const session = meta.benchmark_session;
      const product = meta.benchmark_product;
      if (extras > (products[product].allowedExtraMessages ?? 0)) throw new Error(`${product}/${session}: unexpected extra messages: ${extras}`);
      const group = groups.get(session) ?? { session, entries: new Map(), sequence: 0, text: "", timer: null };
      if (group.entries.has(product)) throw new Error(`duplicate ${product}/${session}`);
      const entry = { product, session, response, finished: false };
      group.entries.set(product, entry);
      groups.set(session, group);
      active.set(product, active.get(product) + 1);
      received.push({ product, session, phase, injectedMessages: extras, textHash, imageHashes: receivedImageHashes });
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache", connection: "keep-alive" });
      response.flushHeaders();
      response.on("close", () => { if (!entry.finished && !stopped) errors.push(`upstream closed early: ${product}/${session}`); });
      if (group.entries.size === names.length) {
        for (const member of group.entries.values()) sendInitial(member);
        sendDelta(group);
        group.timer = setInterval(() => sendDelta(group), 250);
      }
    } catch (error) {
      errors.push(`upstream: ${error.message}`);
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  // Keep upstream connections alive across the five-second gap between loads.
  // Node's default five-second timeout races with the gateways' pooled sockets.
  server.keepAliveTimeout = 60000;
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(spec.upstreamPort, "127.0.0.1", resolve);
  });
}
function beginClient(product, session) {
  const progress = { product, session, characters: 0 };
  let firstResolve;
  const first = new Promise((resolve) => { firstResolve = resolve; });
  const done = new Promise((resolve, reject) => {
    const target = products[product];
    const prefix = Buffer.from(`{"model":${JSON.stringify(target.model)},"stream":true,"input":`);
    const suffix = Buffer.from(`,"metadata":${JSON.stringify({ benchmark_product: product, benchmark_session: session })}}`);
    const request = http.request({ hostname: "127.0.0.1", port: target.port, path: "/v1/responses", method: "POST", headers: { "content-type": "application/json", "authorization": "Bearer bench-key", "x-session-id": `${product}-${session}`, "content-length": prefix.length + inputBytes.length + suffix.length } }, (response) => {
      if (response.statusCode !== 200) {
        let errorBody = "";
        response.on("data", (chunk) => { if (errorBody.length < 1000) errorBody += chunk.toString(); });
        response.on("end", () => reject(new Error(`${product}/${session}: HTTP ${response.statusCode} ${errorBody}`)));
        return;
      }
      let buffered = "";
      let text = "";
      let sawFirst = false;
      let sawCompleted = false;
      response.setEncoding("utf8");
      response.on("data", (chunk) => {
        buffered = (buffered + chunk).replaceAll("\r\n", "\n");
        let boundary;
        while ((boundary = buffered.indexOf("\n\n")) !== -1) {
          const block = buffered.slice(0, boundary);
          buffered = buffered.slice(boundary + 2);
          for (const line of block.split("\n")) {
            if (!line.startsWith("data: ") || line === "data: [DONE]") continue;
            try {
              const payload = JSON.parse(line.slice(6));
              if (payload.type === "response.output_text.delta") {
                text += payload.delta;
                progress.characters = text.length;
                if (!sawFirst) { sawFirst = true; firstResolve(); }
              }
              if (payload.type === "response.completed") sawCompleted = true;
            } catch (error) { reject(error); }
          }
        }
      });
      response.on("error", reject);
      response.on("end", () => {
        if (!sawFirst || !sawCompleted) return reject(new Error(`${product}/${session}: incomplete response`));
        const expected = groups.get(session)?.text;
        if (text !== expected) return reject(new Error(`${product}/${session}: output differs`));
        completed.push({ product, session, characters: text.length, textHash: sha(text) });
        resolve();
      });
    });
    clientRequests.add(request);
    request.on("close", () => clientRequests.delete(request));
    request.on("error", reject);
    request.write(prefix);
    request.write(inputBytes);
    request.end(suffix);
  });
  done.catch((error) => errors.push(error.message));
  return { first, done, progress };
}
function beginSampler() {
  const args = [String(timing.interval), ...names.flatMap((name) => [String(products[name].uiPid), String(products[name].gatewayPid)])];
  sampler = spawn(spec.sampler, args, { stdio: ["ignore", "pipe", "pipe"] });
  samplerClosed = new Promise((resolve) => sampler.once("close", resolve));
  sampler.stderr.setEncoding("utf8");
  sampler.stderr.on("data", (data) => errors.push(`sampler: ${data.trim()}`));
  sampler.on("error", (error) => errors.push(`sampler: ${error.message}`));
  sampler.on("exit", (code) => { if (!stopped) errors.push(`sampler exited: ${code}`); });
  createInterface({ input: sampler.stdout }).on("line", (line) => {
    try {
      const sample = JSON.parse(line);
      sample.wallMs = Date.now();
      sample.phase = phase;
      const coalitions = sample.products.map((product) => product.coalition);
      if (coalitions.some((value) => !value) || new Set(coalitions).size !== names.length) throw new Error("missing or shared product coalition");
      for (const product of sample.products) {
        if (!product.members.some((member) => member.pid === product.ui) || !product.members.some((member) => member.pid === product.gateway)) throw new Error("required UI or gateway process disappeared");
        if (product.members.some((member) => excludedPids.has(member.pid) || member.pid === sampler.pid)) throw new Error("protected process or benchmark driver included in product coalition");
      }
      samples.push(sample);
      gzip.write(`${JSON.stringify(sample)}\n`);
    } catch (error) { errors.push(`sample: ${error.message}`); }
  });
}
async function measureWindow(level, index, clients) {
  phase = `main-${level}-window-${index}`;
  const progress = clients.map((client) => client.progress.characters);
  const start = performance.now();
  await sleep(timing.window);
  for (let i = 0; i < clients.length; i++) if (clients[i].progress.characters <= progress[i]) throw new Error(`stream did not advance during ${phase}: ${clients[i].progress.product}/${clients[i].progress.session}`);
  const selected = samples.filter((sample) => sample.phase === phase);
  if (selected.length < 3) throw new Error(`too few samples for ${phase}`);
  for (let i = 0; i < names.length; i++) {
    const first = selected[0].products[i];
    const last = selected.at(-1).products[i];
    const firstHost = first.members.find((member) => member.pid === first.gateway);
    const lastHost = last.members.find((member) => member.pid === last.gateway);
    const elapsed = lastHost.measuredMonotonic - firstHost.measuredMonotonic;
    const ticks = lastHost.cpuTicks - firstHost.cpuTicks;
    const factor = selected[0].timebase[0] / selected[0].timebase[1];
    windows.push({ product: names[i], mainSessions: level, activeRequests: level * 5, window: index, samples: selected.length, durationSeconds: elapsed, requestedDurationSeconds: (performance.now() - start) / 1000, footprintMiB: selected.reduce((sum, item) => sum + item.products[i].members.reduce((total, member) => total + member.footprint, 0), 0) / selected.length / 1048576, rssMiB: selected.reduce((sum, item) => sum + item.products[i].members.reduce((total, member) => total + member.rss, 0), 0) / selected.length / 1048576, gatewayCpuPercent: ticks * factor / 1e9 / elapsed * 100 });
  }
}
try {
  await beginUpstream();
  beginSampler();
  await waitFor(() => samples.length > 0, "initial process sample");
  console.log(`warming up for ${timing.warmup / 1000}s`);
  await sleep(timing.warmup);
  for (const level of levels) {
    phase = `main-${level}-loading`;
    const clients = [];
    const sessions = [];
    for (let parent = 0; parent < level; parent++) {
      const session = `main-${level}-parent-${parent}`;
      sessions.push(session);
      for (const name of names) {
        const client = beginClient(name, session);
        clients.push(client);
      }
    }
    if (level) {
      await waitFor(() => sessions.every((session) => groups.get(session)?.sequence > 0), "parent streams");
      for (let parent = 0; parent < level; parent++) for (let child = 0; child < 4; child++) {
        const session = `main-${level}-parent-${parent}-child-${child}`;
        sessions.push(session);
        for (const name of names) clients.push(beginClient(name, session));
      }
      await waitFor(() => names.every((name) => active.get(name) === level * 5) && sessions.every((session) => groups.get(session)?.entries.size === names.length), "all active streams");
      await sleep(timing.settle);
    }
    const activeSnapshot = Object.fromEntries(active);
    if (!names.every((name) => activeSnapshot[name] === level * 5) || errors.length) throw new Error(`invalid load at ${level}: ${JSON.stringify(activeSnapshot)} ${errors[0] ?? ""}`);
    audits.push({ mainSessions: level, active: activeSnapshot, received: received.filter((entry) => entry.phase === `main-${level}-loading`) });
    console.log(`measuring ${level * 5} active requests per product`);
    for (let i = 0; i < timing.windows; i++) await measureWindow(level, i, clients);
    phase = `main-${level}-release`;
    for (const session of sessions) finishGroup(groups.get(session));
    await Promise.all(clients.map((client) => client.done));
    if (errors.length || names.some((name) => active.get(name) !== 0)) throw new Error(`release failed: ${errors[0] ?? "active requests remain"}`);
    if (level) await sleep(timing.gap);
  }
  const median = (numbers) => { const sorted = [...numbers].sort((a, b) => a - b); const middle = Math.floor(sorted.length / 2); return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2; };
  const summary = levels.flatMap((level) => names.map((product) => {
    const rows = windows.filter((row) => row.mainSessions === level && row.product === product);
    const result = { product, mainSessions: level, activeRequests: level * 5 };
    for (const metric of ["footprintMiB", "rssMiB", "gatewayCpuPercent"]) {
      const values = rows.map((row) => row[metric]);
      result[metric] = { median: median(values), min: Math.min(...values), max: Math.max(...values) };
    }
    return result;
  }));
  const result = { metadata: { date: new Date().toISOString(), timing, levels, inputBytes: inputBytes.length, corpusMessages: corpus.length, corpusRoleTextSha256: originalTextHash, imageHashes, productConfig: products, upstreamPort: spec.upstreamPort, samplerTimebase: samples[0].timebase }, summary, windows, validation: { audits, completed, finalActive: Object.fromEntries(active), errors } };
  fs.writeFileSync(`${output}/results.json`, `${JSON.stringify(result, null, 2)}\n`, { flag: "wx" });
  console.log(`completed ${completed.length} validated requests; results: ${output}/results.json`);
} catch (error) {
  console.error(error.stack);
  process.exitCode = 1;
} finally {
  stopped = true;
  for (const request of clientRequests) request.destroy();
  for (const group of groups.values()) {
    clearInterval(group.timer);
    for (const entry of group.entries.values()) if (!entry.finished) entry.response.destroy();
  }
  sampler?.kill();
  if (samplerClosed) await samplerClosed;
  server?.closeAllConnections();
  server?.close();
  gzip.end();
  await new Promise((resolve) => rawFile.on("finish", resolve));
}
