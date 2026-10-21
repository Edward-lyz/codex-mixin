// Runs the long-context ablation against a real provider.
//
// Four arms share one task set, one model and one gateway build: the original
// flow, the compression guidance without tools, tier 1 only and every tier.
// Each arm gets its own configuration, context database and gateway process,
// so a run never touches the developer live gateway or archive.
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import net from "node:net";
import path from "node:path";

const repoRoot = process.cwd();
const binary = process.env.EVAL_BINARY ?? path.join(repoRoot, "target/debug/codex-mixin");
const sourceConfig =
  process.env.EVAL_SOURCE_CONFIG ?? path.join(os.homedir(), ".codex-mixin", "config.json");
const sourceKey =
  process.env.EVAL_SOURCE_KEY ?? path.join(os.homedir(), ".codex-mixin", "config.json.key");
const model = process.env.EVAL_MODEL ?? "DeepSeek-V4.1-Flash-baidu-oneapi";
const configuredPort = process.env.EVAL_PORT ? Number(process.env.EVAL_PORT) : null;
let port = configuredPort;
const workDir = process.env.EVAL_WORKDIR ?? path.join(os.tmpdir(), "codex-mixin-context-eval");
const taskCount = Number(process.env.EVAL_TASKS ?? 1);
const fillerRounds = Number(process.env.EVAL_ROUNDS ?? 2);
const chunkCount = Number(process.env.EVAL_CHUNKS ?? 12);
const chunkChars = Number(process.env.EVAL_CHUNK_CHARS ?? 1200);

const COMMON_SETTINGS = [
  "--growth-threshold-tokens=8000",
  "--protected-messages=2",
  "--protected-tokens=500",
  "--max-internal-rounds=4",
  "--context-window-tokens=" + (process.env.EVAL_WINDOW ?? "128000"),
];

const ARMS = [
  { id: "baseline", settings: ["--enabled=false"] },
  { id: "guidance", settings: ["--enabled=true", "--guidance-only=true"] },
  { id: "t1", settings: ["--enabled=true", "--guidance-only=false", "--max-tier=1"] },
  { id: "full", settings: ["--enabled=true", "--guidance-only=false", "--max-tier=3"] },
];

function environment(directory) {
  return {
    ...process.env,
    CODEX_GATEWAY_CONFIG: path.join(directory, "config.json"),
    CODEX_GATEWAY_RUNTIME_FILE: path.join(directory, "runtime.json"),
    CODEX_GATEWAY_DAEMON_FILE: path.join(directory, "daemon.json"),
    CODEX_GATEWAY_SERVICE_ID: "codex-mixin-context-eval",
    CODEX_MIXIN_CONTEXT_DB: path.join(directory, "context.db"),
  };
}

function prepare() {
  fs.mkdirSync(workDir, { recursive: true });
  fs.copyFileSync(sourceConfig, path.join(workDir, "config.json"));
  if (fs.existsSync(sourceKey)) {
    fs.copyFileSync(sourceKey, path.join(workDir, "config.json.key"));
  }
}

function applyArm(arm) {
  const result = spawnSync(
    binary,
    ["context", "config", "set", ...COMMON_SETTINGS, ...arm.settings, "--json"],
    { env: environment(workDir), encoding: "utf8" },
  );
  if (result.status !== 0) {
    throw new Error("context config set failed: " + (result.stderr || result.stdout));
  }
}

function resetArchive() {
  for (const suffix of ["", "-wal", "-shm"]) {
    fs.rmSync(path.join(workDir, "context.db" + suffix), { force: true });
  }
}

async function freePort() {
  return await new Promise((resolve, reject) => {
    const server = net.createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => resolve(address.port));
    });
  });
}

async function waitForGateway() {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch("http://127.0.0.1:" + port + "/v1/models");
      if (response.ok) {
        return;
      }
    } catch {
      // The gateway is still starting.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("gateway did not become ready on port " + port);
}

async function startGateway(arm) {
  const log = path.join(workDir, "gateway-" + arm + ".log");
  const child = spawn(
    binary,
    ["start", "--bind", "127.0.0.1:" + port, "--no-tui", "--log-file", log],
    { env: environment(workDir), stdio: "ignore" },
  );
  await new Promise((resolve) => setTimeout(resolve, 300));
  if (child.exitCode !== null) {
    throw new Error("gateway exited during startup, see " + log);
  }
  await waitForGateway();
  return child;
}

async function stopGateway(child) {
  if (child.exitCode !== null) {
    return;
  }
  child.kill("SIGTERM");
  const deadline = Date.now() + 10000;
  while (child.exitCode === null && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

function chunk(index) {
  const words = [];
  for (let word = 0; word < chunkChars / 6; word += 1) {
    const pick = (index * 31 + word * 17) % 97;
    words.push("op" + pick.toString(16).padStart(2, "0"));
  }
  return "build log chunk " + index + ": " + words.join(" ");
}

// Each kind plants one fact early in a long transcript and asks for it at the
// end, so a compression that loses the fact shows up as a wrong answer.
const TASK_KINDS = ["logs", "files", "goals"];

const TASK_CASES = {
  logs: {
    needle: (index) => "E_ALPHA_" + (7300 + index * 17),
    plant: (needle) => ({ position: 2, suffix: " error " + needle + " raised by the linker" }),
    question: "Which exact error code appeared in build log chunk 2? Answer with the code only.",
  },
  files: {
    needle: () => "src/gateway/router.rs",
    plant: () => ({
      position: 3,
      suffix: " decision: stable routing lives in src/gateway/router.rs",
    }),
    question: "Which file implements stable routing? Answer with the path only.",
  },
  goals: {
    needle: () => "cache bug",
    plant: () => ({ position: 4, suffix: " goal: refactor the parser" }),
    later: () => ({
      position: 20,
      suffix: " new goal: fix the cache bug; the parser refactor is dropped",
    }),
    question: "What is the current goal? Answer with the goal only.",
  },
};

function buildTask(index) {
  const kind = TASK_KINDS[index % TASK_KINDS.length];
  const taskCase = TASK_CASES[kind];
  const needle = taskCase.needle(index);
  const plant = taskCase.plant(needle);
  const later = taskCase.later ? taskCase.later() : null;
  const messages = [];
  for (let position = 0; position < chunkCount; position += 1) {
    const suffix =
      position === plant.position
        ? plant.suffix
        : later && position === later.position
          ? later.suffix
          : "";
    messages.push({
      type: "message",
      role: "user",
      content: [{ type: "input_text", text: chunk(position) + suffix }],
    });
    messages.push({
      type: "message",
      role: "assistant",
      content: [{ type: "output_text", text: "chunk " + position + " noted" }],
    });
  }
  return { index, kind, needle, question: taskCase.question, messages };
}

// The gateway reloads its stored configuration once at startup and briefly
// drops the listener, so a connection error is retried instead of failing the
// whole run.
async function postResponses(payload, session) {
  let lastError = null;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    try {
      return await fetch("http://127.0.0.1:" + port + "/v1/responses", {
        method: "POST",
        headers: { "content-type": "application/json", "x-session-id": session },
        body: JSON.stringify(payload),
      });
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 1000));
    }
  }
  throw lastError;
}

async function sendRound(input, session) {
  const started = Date.now();
  const response = await postResponses({ model, stream: true, input }, session);
  if (!response.ok) {
    throw new Error("request failed: " + response.status + " " + (await response.text()));
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let answer = "";
  let firstDelta = null;
  let usage = null;
  for (;;) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }
    buffer += decoder.decode(value, { stream: true });
    const frames = buffer.split("\n\n");
    buffer = frames.pop() ?? "";
    for (const frame of frames) {
      const lines = frame.split("\n");
      const event = lines.find((line) => line.startsWith("event: "))?.slice(7);
      const data = lines
        .filter((line) => line.startsWith("data: "))
        .map((line) => line.slice(6))
        .join("\n");
      if (!data) {
        continue;
      }
      let payload;
      try {
        payload = JSON.parse(data);
      } catch {
        continue;
      }
      if (event === "response.output_text.delta") {
        firstDelta ??= Date.now() - started;
        answer += payload.delta ?? "";
      }
      if (event === "response.completed") {
        usage = payload.response?.usage ?? null;
        if (!answer) {
          answer = (payload.response?.output ?? [])
            .flatMap((item) => item.content ?? [])
            .map((part) => part.text ?? "")
            .join("");
        }
      }
    }
  }
  return { answer, usage, firstDelta, elapsed: Date.now() - started };
}

function readStats() {
  const result = spawnSync(binary, ["context", "stats", "--json"], {
    env: environment(workDir),
    encoding: "utf8",
  });
  if (result.status !== 0) {
    return null;
  }
  return JSON.parse(result.stdout);
}

async function runArm(arm) {
  applyArm(arm);
  resetArchive();
  const child = await startGateway(arm.id);
  const rows = [];
  try {
    for (let taskIndex = 0; taskIndex < taskCount; taskIndex += 1) {
      const task = buildTask(taskIndex);
      const session = "eval-" + arm.id + "-" + taskIndex;
      const input = task.messages.slice();
      let inputTokens = 0;
      let cachedTokens = 0;
      let outputTokens = 0;
      for (let round = 0; round < fillerRounds; round += 1) {
        input.push({
          type: "message",
          role: "user",
          content: [
            {
              type: "input_text",
              text: chunk(100 + round) + " Summarize this chunk in one short sentence.",
            },
          ],
        });
        const turn = await sendRound(input, session);
        inputTokens += turn.usage?.input_tokens ?? 0;
        cachedTokens += turn.usage?.input_tokens_details?.cached_tokens ?? 0;
        outputTokens += turn.usage?.output_tokens ?? 0;
        input.push({
          type: "message",
          role: "assistant",
          content: [{ type: "output_text", text: turn.answer }],
        });
      }
      input.push({
        type: "message",
        role: "user",
        content: [
          { type: "input_text", text: task.question },
        ],
      });
      const final = await sendRound(input, session);
      inputTokens += final.usage?.input_tokens ?? 0;
      cachedTokens += final.usage?.input_tokens_details?.cached_tokens ?? 0;
      outputTokens += final.usage?.output_tokens ?? 0;
      const recalled = final.answer.toLowerCase().includes(task.needle.toLowerCase());
      rows.push({
        arm: arm.id,
        task: taskIndex,
        needle: task.needle,
        recalled,
        answer: final.answer.trim().slice(0, 200),
        input_tokens: inputTokens,
        cached_tokens: cachedTokens,
        output_tokens: outputTokens,
        first_token_ms: final.firstDelta,
        elapsed_ms: final.elapsed,
      });
      process.stdout.write(
        arm.id + " task " + taskIndex + " (" + task.kind + "): recalled=" + recalled +
          " input=" + inputTokens + " cached=" + cachedTokens + "\n",
      );
    }
  } finally {
    await stopGateway(child);
  }
  return { arm: arm.id, rows, stats: readStats() };
}

function markdown(results) {
  const lines = [
    "| arm | recalled | input tokens | cached tokens | output tokens | first token ms | compressions | saved tokens |",
    "| --- | --- | --- | --- | --- | --- | --- | --- |",
  ];
  for (const result of results) {
    const rows = result.rows;
    const recalled = rows.filter((row) => row.recalled).length + "/" + rows.length;
    const sum = (key) => rows.reduce((total, row) => total + (row[key] ?? 0), 0);
    const totals = result.stats?.totals ?? {};
    const firstToken = Math.min(...rows.map((row) => row.first_token_ms ?? 0));
    lines.push(
      "| " + result.arm + " | " + recalled + " | " + sum("input_tokens") + " | " +
        sum("cached_tokens") + " | " + sum("output_tokens") + " | " + firstToken + " | " +
        (totals.compressions ?? 0) + " | " + (totals.saved_tokens ?? 0) + " |",
    );
  }
  return lines.join("\n");
}

if (!port) {
  port = await freePort();
}
prepare();
const results = [];
for (const arm of ARMS) {
  results.push(await runArm(arm));
}
const report = { model, generated_at: new Date().toISOString(), results };
fs.writeFileSync(path.join(workDir, "results.json"), JSON.stringify(report, null, 1));
process.stdout.write("\n" + markdown(results) + "\n");
process.stdout.write("raw results: " + path.join(workDir, "results.json") + "\n");
