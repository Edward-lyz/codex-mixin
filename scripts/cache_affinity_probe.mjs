#!/usr/bin/env node

import { createHash, randomUUID } from "node:crypto";

const gatewayUrl = process.env.CACHE_PROBE_URL || "http://127.0.0.1:8787/v1/responses";
const model = process.env.CACHE_PROBE_MODEL || "gpt-5.6-sol-baidu-oneapi";
const sessionCount = Number(process.env.CACHE_PROBE_SESSIONS || "2");
const turnCount = Number(process.env.CACHE_PROBE_TURNS || "4");
const prefixBytes = Number(process.env.CACHE_PROBE_PREFIX_BYTES || "48000");
const interTurnDelayMs = Number(process.env.CACHE_PROBE_DELAY_MS || "1000");
const mode = process.env.CACHE_PROBE_MODE || "interleaved";
const runId = process.env.CACHE_PROBE_RUN_ID || randomUUID();

if (!Number.isInteger(sessionCount) || sessionCount < 1) {
  throw new Error("CACHE_PROBE_SESSIONS must be a positive integer");
}
if (!Number.isInteger(turnCount) || turnCount < 1) {
  throw new Error("CACHE_PROBE_TURNS must be a positive integer");
}
if (!Number.isInteger(prefixBytes) || prefixBytes < 4096) {
  throw new Error("CACHE_PROBE_PREFIX_BYTES must be an integer >= 4096");
}
if (!new Set(["interleaved", "sequential", "parallel"]).has(mode)) {
  throw new Error("CACHE_PROBE_MODE must be interleaved, sequential, or parallel");
}

const sleep = (milliseconds) =>
  new Promise((resolve) => setTimeout(resolve, milliseconds));

function stablePrefix(label) {
  const lines = [];
  let index = 0;
  while (lines.join("\n").length < prefixBytes) {
    const digest = createHash("sha256")
      .update(`${runId}:${label}:${index}`)
      .digest("hex");
    lines.push(`Cache affinity probe ${label} line ${index}: ${digest}`);
    index += 1;
  }
  return lines.join("\n").slice(0, prefixBytes);
}

function parseSse(text) {
  const events = [];
  for (const block of text.split(/\n\n+/)) {
    let event = "message";
    const data = [];
    for (const line of block.split("\n")) {
      if (line.startsWith("event:")) event = line.slice(6).trim();
      if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
    }
    if (data.length === 0 || data.join("\n") === "[DONE]") continue;
    try {
      events.push({ event, data: JSON.parse(data.join("\n")) });
    } catch {
      events.push({ event, data: data.join("\n") });
    }
  }
  return events;
}

function findUsage(events) {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const value = events[index].data;
    const response = value?.response || value;
    if (response?.usage) return response.usage;
  }
  return null;
}

async function postTurn(session, turn) {
  const input = [
    {
      type: "message",
      role: "user",
      content: [
        {
          type: "input_text",
          text: session.prefix,
        },
      ],
    },
  ];

  for (let index = 1; index <= turn; index += 1) {
    input.push({
      type: "message",
      role: "user",
      content: [{ type: "input_text", text: `Probe turn ${index}: reply with OK.` }],
    });
    if (index < turn) {
      input.push({
        type: "message",
        role: "assistant",
        content: [{ type: "output_text", text: "OK" }],
      });
    }
  }

  const startedAt = new Date();
  const response = await fetch(gatewayUrl, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "thread-id": session.threadId,
      authorization: "Bearer cache-affinity-probe",
    },
    body: JSON.stringify({
      model,
      stream: true,
      max_output_tokens: 16,
      input,
    }),
  });
  const responseText = await response.text();
  const events = parseSse(responseText);
  const usage = findUsage(events);
  const elapsedMs = Date.now() - startedAt.getTime();

  const result = {
    started_at: startedAt.toISOString(),
    session: session.label,
    thread_id_sha256: createHash("sha256").update(session.threadId).digest("hex").slice(0, 16),
    turn,
    status: response.status,
    elapsed_ms: elapsedMs,
    usage,
  };
  console.log(JSON.stringify(result));

  if (!response.ok) {
    throw new Error(`session ${session.label} turn ${turn} failed: ${responseText.slice(0, 500)}`);
  }
}

const sessions = Array.from({ length: sessionCount }, (_, index) => ({
  label: String.fromCharCode(65 + index),
  threadId: `cache-affinity-probe-${runId}-${index}`,
  prefix: stablePrefix(String.fromCharCode(65 + index)),
}));

console.log(
  JSON.stringify({
    experiment: "cache-affinity",
    started_at: new Date().toISOString(),
    model,
    mode,
    session_count: sessionCount,
    turn_count: turnCount,
    prefix_bytes: prefixBytes,
  }),
);

if (mode === "sequential") {
  for (const session of sessions) {
    for (let turn = 1; turn <= turnCount; turn += 1) {
      await postTurn(session, turn);
      await sleep(interTurnDelayMs);
    }
  }
} else if (mode === "parallel") {
  for (let turn = 1; turn <= turnCount; turn += 1) {
    await Promise.all(sessions.map((session) => postTurn(session, turn)));
    await sleep(interTurnDelayMs);
  }
} else {
  for (let turn = 1; turn <= turnCount; turn += 1) {
    for (const session of sessions) {
      await postTurn(session, turn);
      await sleep(interTurnDelayMs);
    }
  }
}
