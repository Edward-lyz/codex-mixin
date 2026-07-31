#!/usr/bin/env node

import http from "node:http";
import { createHash, randomUUID } from "node:crypto";
import { readFile, stat } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { performance } from "node:perf_hooks";

const ducc =
  process.env.DUCC_BIN ||
  path.join(os.homedir(), ".baidu-cc", "baidu-cc", "bin", "ducc");
const timeoutMilliseconds = Number(process.env.DUCC_PROBE_TIMEOUT_MS || "30000");
const turnCount = Number(process.env.DUCC_PROBE_TURNS || "1");
const expectedModel = process.env.DUCC_PROBE_MODEL || "Claude Sonnet 5";
const isolationMode = process.env.DUCC_PROBE_ISOLATION_MODE || "bare";
const isolationArguments =
  isolationMode === "safe-mode"
    ? ["--safe-mode"]
    : isolationMode === "none"
      ? []
      : ["--bare"];
const turnMarkers = Array.from(
  { length: turnCount },
  () => `codex-mixin-ducc-probe-${randomUUID()}`,
);
const pendingMarkers = new Set(turnMarkers);
const captured = [];
const forwarded = [];
const protectedConfigurationPaths = [
  ".claude/settings.json",
  ".baidu-cc/user.json",
  ".baidu-cc/meta.json",
  ".baidu-cc/baidu-cc/settings.json",
  ".codex/config.toml",
  ".codex-mixin/config.json",
  ".zshrc",
].map((relativePath) => path.join(os.homedir(), relativePath));
const imageData =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const registeredBody = {
  model: expectedModel,
  max_tokens: 128,
  stream: true,
  system: [{ type: "text", text: "MIXIN_ORIGINAL_SYSTEM" }],
  messages: [
    {
      role: "user",
      content: [
        { type: "text", text: "MIXIN_ORIGINAL_TEXT" },
        {
          type: "image",
          source: {
            type: "base64",
            media_type: "image/png",
            data: imageData,
          },
        },
      ],
    },
  ],
  tools: [
    {
      name: "mixin_original_tool",
      description: "A user-declared tool retained by the bridge.",
      input_schema: { type: "object", properties: {} },
    },
  ],
};

async function configurationFingerprints() {
  return Object.fromEntries(
    await Promise.all(
      protectedConfigurationPaths.map(async (filePath) => {
        try {
          const [contents, metadata] = await Promise.all([
            readFile(filePath),
            stat(filePath),
          ]);
          return [
            filePath,
            {
              sha256: createHash("sha256").update(contents).digest("hex"),
              size: metadata.size,
              mtimeMs: metadata.mtimeMs,
            },
          ];
        } catch (error) {
          if (error?.code === "ENOENT") {
            return [filePath, null];
          }
          throw error;
        }
      }),
    ),
  );
}

const configurationBefore = await configurationFingerprints();

function bodyShape(body) {
  if (!body || typeof body !== "object") {
    return null;
  }
  return {
    keys: Object.keys(body).sort(),
    model: body.model || null,
    systemPresent: body.system != null,
    systemBlockCount: Array.isArray(body.system) ? body.system.length : 0,
    toolCount: Array.isArray(body.tools) ? body.tools.length : 0,
    toolNames: Array.isArray(body.tools)
      ? body.tools.map((tool) => tool?.name || null)
      : [],
    messageCount: Array.isArray(body.messages) ? body.messages.length : 0,
    messageShapes: Array.isArray(body.messages)
      ? body.messages.map((message) => ({
          role: message?.role || null,
          contentTypes: Array.isArray(message?.content)
            ? message.content.map((block) => block?.type || null)
            : [],
          textLengths: Array.isArray(message?.content)
            ? message.content
                .filter((block) => block?.type === "text")
                .map((block) => (typeof block.text === "string" ? block.text.length : 0))
            : [],
        }))
      : [],
    imageBlockCount: Array.isArray(body.messages)
      ? body.messages.reduce(
          (total, message) =>
            total +
            (Array.isArray(message.content)
              ? message.content.filter((block) => block?.type === "image").length
              : 0),
          0,
        )
      : 0,
    metadataKeys:
      body.metadata && typeof body.metadata === "object"
        ? Object.keys(body.metadata).sort()
        : [],
    outputConfigKeys:
      body.output_config && typeof body.output_config === "object"
        ? Object.keys(body.output_config).sort()
        : [],
  };
}

function containsString(value, expected) {
  if (typeof value === "string") {
    return value.includes(expected);
  }
  if (Array.isArray(value)) {
    return value.some((item) => containsString(item, expected));
  }
  if (value && typeof value === "object") {
    return Object.values(value).some((item) => containsString(item, expected));
  }
  return false;
}

function writeSseResponse(response, model) {
  response.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
  });
  const events = [
    {
      event: "message_start",
      data: {
        type: "message_start",
        message: {
          id: "msg_ducc_loopback_probe",
          type: "message",
          role: "assistant",
          model: model || "probe-model",
          content: [],
          stop_reason: null,
          usage: { input_tokens: 1, output_tokens: 0 },
        },
      },
    },
    {
      event: "content_block_start",
      data: {
        type: "content_block_start",
        index: 0,
        content_block: { type: "text", text: "" },
      },
    },
    {
      event: "content_block_delta",
      data: {
        type: "content_block_delta",
        index: 0,
        delta: { type: "text_delta", text: "DUCC_LOOPBACK_OK" },
      },
    },
    {
      event: "content_block_stop",
      data: { type: "content_block_stop", index: 0 },
    },
    {
      event: "message_delta",
      data: {
        type: "message_delta",
        delta: { stop_reason: "end_turn", stop_sequence: null },
        usage: { output_tokens: 1 },
      },
    },
    { event: "message_stop", data: { type: "message_stop" } },
  ];
  for (const event of events) {
    response.write(`event: ${event.event}\n`);
    response.write(`data: ${JSON.stringify(event.data)}\n\n`);
  }
  response.end();
}

const upstreamServer = http.createServer((request, response) => {
  const chunks = [];
  request.on("data", (chunk) => chunks.push(chunk));
  request.on("end", () => {
    let body = null;
    try {
      body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    } catch {
      body = null;
    }
    forwarded.push({
      method: request.method,
      url: request.url,
      headerNames: Object.keys(request.headers).sort(),
      duccHeaderPresent:
        typeof request.headers.comate_custom_header === "string" &&
        request.headers.comate_custom_header.length > 0,
      placeholderAuthPresent:
        typeof request.headers.authorization === "string" ||
        typeof request.headers["x-api-key"] === "string",
      body: bodyShape(body),
      bodyMatchesRegistered: JSON.stringify(body) === JSON.stringify(registeredBody),
    });
    writeSseResponse(response, body?.model);
  });
});

await new Promise((resolve, reject) => {
  upstreamServer.once("error", reject);
  upstreamServer.listen(0, "127.0.0.1", resolve);
});
const upstreamAddress = upstreamServer.address();
const upstreamBaseURL = `http://127.0.0.1:${upstreamAddress.port}`;

const server = http.createServer((request, response) => {
  const chunks = [];
  request.on("data", (chunk) => chunks.push(chunk));
  request.on("end", async () => {
    let body = null;
    try {
      body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    } catch {
      body = null;
    }
    captured.push({
      method: request.method,
      url: request.url,
      headerNames: Object.keys(request.headers).sort(),
      duccHeaderPresent:
        typeof request.headers.comate_custom_header === "string" &&
        request.headers.comate_custom_header.length > 0,
      body: bodyShape(body),
    });

    if (request.method === "HEAD") {
      response.writeHead(200);
      response.end();
      return;
    }
    const marker = [...pendingMarkers].find((candidate) =>
      containsString(body, candidate),
    );
    if (body?.model !== expectedModel || !marker) {
      writeSseResponse(response, body?.model);
      return;
    }
    pendingMarkers.delete(marker);

    const headers = { ...request.headers };
    for (const name of [
      "accept-encoding",
      "connection",
      "content-length",
      "host",
      "keep-alive",
      "proxy-authenticate",
      "proxy-authorization",
      "te",
      "trailer",
      "transfer-encoding",
      "upgrade",
    ]) {
      delete headers[name];
    }
    delete headers.authorization;
    delete headers["x-api-key"];
    try {
      const upstream = await fetch(`${upstreamBaseURL}${request.url}`, {
        method: "POST",
        headers,
        body: JSON.stringify(registeredBody),
      });
      response.writeHead(upstream.status, Object.fromEntries(upstream.headers));
      response.end(Buffer.from(await upstream.arrayBuffer()));
    } catch {
      response.writeHead(502);
      response.end();
    }
  });
});

await new Promise((resolve, reject) => {
  server.once("error", reject);
  server.listen(0, "127.0.0.1", resolve);
});
const address = server.address();
const baseURL = `http://127.0.0.1:${address.port}`;
const proxyEnvironment =
  process.env.DUCC_PROBE_USE_PROXY === "1"
    ? {
        HTTP_PROXY: baseURL,
        HTTPS_PROXY: baseURL,
        http_proxy: baseURL,
        https_proxy: baseURL,
        NO_PROXY: "127.0.0.1,localhost",
        no_proxy: "127.0.0.1,localhost",
      }
    : {};
const startedAt = performance.now();
const child = spawn(
  "/usr/bin/time",
  [
    "-l",
    ducc,
    ...isolationArguments,
    "--no-ducc-system-prompt",
    "--disable-slash-commands",
    "--no-session-persistence",
    "--permission-mode",
    "dontAsk",
    "--prompt-suggestions",
    "false",
    "--tools",
    "",
    "--model",
    process.env.DUCC_PROBE_MODEL || "Claude Sonnet 5",
    "--settings",
    JSON.stringify({
      env: {
        ANTHROPIC_BASE_URL: baseURL,
        ANTHROPIC_API_KEY: "codex-mixin-loopback",
      },
    }),
    "--print",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
  ],
  {
    cwd: os.tmpdir(),
    env: {
      ...process.env,
      ...proxyEnvironment,
      ANTHROPIC_BASE_URL: baseURL,
      ANTHROPIC_API_KEY: "codex-mixin-loopback",
      DISABLE_BAIDU_CLAUDE_UPDATE: "1",
      DISABLE_DUCC_CLI_UPDATE: "1",
    },
    stdio: ["pipe", "pipe", "pipe"],
  },
);
const sessionId = randomUUID();
const inputLines = Array.from({ length: turnCount }, (_, index) =>
  JSON.stringify({
    type: "user",
    message: {
      role: "user",
      content: [
        {
          type: "text",
          text: `${turnMarkers[index]}\nOpen model request ${index + 1} and return its response without calling tools.`,
        },
        {
          type: "image",
          source: {
            type: "base64",
            media_type: "image/png",
            data: imageData,
          },
        },
      ],
    },
    parent_tool_use_id: null,
    session_id: sessionId,
  }),
);
child.stdin.end(`${inputLines.join("\n")}\n`);
let stdout = "";
let stderr = "";
child.stdout.on("data", (chunk) => {
  stdout += chunk.toString("utf8");
});
child.stderr.on("data", (chunk) => {
  stderr += chunk.toString("utf8");
});
const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMilliseconds);
const exit = await new Promise((resolve) => child.once("exit", (code, signal) => resolve({ code, signal })));
clearTimeout(timer);
server.close();
upstreamServer.close();
const elapsedMilliseconds = Math.round(performance.now() - startedAt);
const maxResidentBytesMatch = stderr.match(/(\d+)\s+maximum resident set size/);
const configurationAfter = await configurationFingerprints();
const configurationUnchanged =
  JSON.stringify(configurationAfter) === JSON.stringify(configurationBefore);

const result = {
  ducc,
  exit,
  isolationMode,
  turnCount,
  elapsedMilliseconds,
  maxResidentBytes: maxResidentBytesMatch ? Number(maxResidentBytesMatch[1]) : null,
  requestCount: captured.length,
  requests: captured,
  forwardedRequestCount: forwarded.length,
  forwardedRequests: forwarded,
  configurationUnchanged,
  protectedConfigurationPaths,
  responseObserved: stdout.includes("DUCC_LOOPBACK_OK"),
  stderrPresent: stderr.trim().length > 0,
  stdoutEvents: stdout
    .split(/\r?\n/)
    .filter(Boolean)
    .slice(-10)
    .map((line) => {
      try {
        const event = JSON.parse(line);
        return {
          type: event.type || null,
          subtype: event.subtype || null,
          isError: event.is_error || false,
          result:
            typeof event.result === "string" ? event.result.slice(0, 500) : null,
        };
      } catch {
        return { type: "non_json", length: line.length };
      }
    }),
};
process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
if (
  exit.code !== 0 ||
  captured.filter((request) => request.method === "POST").length === 0 ||
  !captured
    .filter((request) => request.method === "POST")
    .every((request) => request.duccHeaderPresent) ||
  forwarded.length !== turnCount ||
  !forwarded.every(
    (request) =>
      request.duccHeaderPresent &&
      !request.placeholderAuthPresent &&
      request.bodyMatchesRegistered &&
      request.body?.imageBlockCount === 1,
  ) ||
  !configurationUnchanged ||
  !result.responseObserved
) {
  process.exitCode = 1;
}
