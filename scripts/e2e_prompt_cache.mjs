// Payload builder and wire checker for the prompt-prefix cache E2E.
//
// `build` writes two-turn Responses payloads containing a real oversized PNG so
// the vision budget applies. `verify` asserts what the gateway actually sent
// upstream: fresh tool images inlined once, replayed ones reduced to a stable
// marker, and the earlier prompt replayed byte-for-byte.
import fs from "node:fs";
import zlib from "node:zlib";

const [command, dir] = process.argv.slice(2);
if (!dir || !["build", "verify"].includes(command)) {
  throw new Error("usage: node e2e_prompt_cache.mjs <build|verify> <dir>");
}

const MARKER = "[tool image omitted from replay to preserve prompt cache]";
const RELOCATED = "[tool images follow in the next user message]";
const SESSION_KEY = "prompt-cache-e2e-session";
const VISION_SIDE = 1568;
const SOURCE_WIDTH = 2000;
const SOURCE_HEIGHT = 1000;
const PROBE_ANCHOR = "look at the screenshot";
// A system prompt on the scale Codex actually sends. The provider cache
// diagnostics only judge a cache hit once the stable prefix is large enough to
// be worth caching, so a one-line system prompt would never exercise them.
const BASE_INSTRUCTIONS = `You are Codex.\n${"Operating guidance that stays byte-identical across turns.\n".repeat(
  700,
)}`;

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const typed = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(typed));
  return Buffer.concat([length, typed, crc]);
}

function png(width, height) {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // bit depth
  header[9] = 6; // RGBA
  const raw = Buffer.alloc(height * (1 + width * 4));
  let offset = 0;
  for (let y = 0; y < height; y += 1) {
    raw[offset] = 0; // filter: none
    offset += 1;
    for (let x = 0; x < width; x += 1) {
      raw[offset] = x & 0xff;
      raw[offset + 1] = y & 0xff;
      raw[offset + 2] = (x ^ y) & 0xff;
      raw[offset + 3] = 0xff;
      offset += 4;
    }
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", header),
    pngChunk("IDAT", zlib.deflateSync(raw)),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

function pngDimensions(dataUrl) {
  const base64 = dataUrl.split(";base64,")[1];
  const buffer = Buffer.from(base64, "base64");
  return [buffer.readUInt32BE(16), buffer.readUInt32BE(20)];
}

function build() {
  const imageUrl = `data:image/png;base64,${png(SOURCE_WIDTH, SOURCE_HEIGHT).toString("base64")}`;
  const tools = [
    {
      type: "function",
      name: "view_image",
      description: "look at a file",
      parameters: { type: "object", properties: { path: { type: "string" } } },
    },
  ];
  // Turn 1 ends with a tool result the model has not answered yet.
  const unanswered = [
    {
      type: "message",
      role: "user",
      content: [{ type: "input_text", text: PROBE_ANCHOR }],
    },
    {
      type: "function_call",
      call_id: "call_shot",
      name: "view_image",
      arguments: '{"path":"/tmp/a.png"}',
    },
    {
      type: "function_call_output",
      call_id: "call_shot",
      output: [
        { type: "input_text", text: "screenshot captured" },
        { type: "input_image", image_url: imageUrl },
      ],
    },
  ];
  // Turn 2 appends the model reply and a new user turn, changing nothing else.
  const answered = [
    ...unanswered,
    {
      type: "message",
      role: "assistant",
      content: [{ type: "output_text", text: "alpha" }],
    },
    // Codex appends one of these every turn. It must stay in the transcript:
    // lifting it into the system prompt would prepend bytes ahead of the whole
    // history and drop the prefix cache on every turn.
    {
      type: "message",
      role: "developer",
      content: [
        {
          type: "input_text",
          text: "<workspace_context>git status: dirty</workspace_context>",
        },
      ],
    },
    {
      type: "message",
      role: "user",
      content: [{ type: "input_text", text: "now summarise it" }],
    },
  ];
  const request = (model, input) => ({
    model,
    stream: true,
    instructions: BASE_INSTRUCTIONS,
    prompt_cache_key: SESSION_KEY,
    tools,
    input,
  });

  for (const [name, model] of [
    ["anthropic", "shared-alpha"],
    ["chat", "shared-beta"],
  ]) {
    fs.writeFileSync(
      `${dir}/${name}-turn1.json`,
      JSON.stringify(request(model, unanswered)),
    );
    fs.writeFileSync(
      `${dir}/${name}-turn2.json`,
      JSON.stringify(request(model, answered)),
    );
  }
  console.log(`source image: ${SOURCE_WIDTH}x${SOURCE_HEIGHT}, data URL ${imageUrl.length} bytes`);
}

const failures = [];
const passes = [];
function check(label, condition, detail = "") {
  if (condition) {
    passes.push(`  ok   ${label}`);
  } else {
    failures.push(`  FAIL ${label}${detail ? ` -- ${detail}` : ""}`);
  }
}

function upstreamTurns(name) {
  return fs
    .readFileSync(`${dir}/${name}`, "utf8")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line).body)
    // The gateway also probes provider capabilities, so keep only the turns
    // this test sent.
    .filter((body) => JSON.stringify(body).includes(PROBE_ANCHOR));
}

function verifyAnthropic() {
  const turns = upstreamTurns("anthropic.ndjson");
  check("anthropic: two turns reached the provider", turns.length === 2, `got ${turns.length}`);
  if (turns.length !== 2) {
    return;
  }
  const [first, second] = turns;
  const toolResult = (body) =>
    body.messages
      .flatMap((message) => (Array.isArray(message.content) ? message.content : []))
      .find((block) => block.type === "tool_result");

  const fresh = toolResult(first);
  const image = fresh.content.find((block) => block.type === "image");
  check("anthropic turn 1: fresh tool result keeps the screenshot", Boolean(image));
  if (image) {
    const [width, height] = pngDimensions(
      `data:${image.source.media_type};base64,${image.source.data}`,
    );
    check(
      "anthropic turn 1: screenshot fits the vision budget",
      Math.max(width, height) === VISION_SIDE,
      `${width}x${height}`,
    );
  }
  check(
    "anthropic turn 1: tool text preserved",
    fresh.content.some((block) => block.text === "screenshot captured"),
  );

  const replayed = toolResult(second);
  check(
    "anthropic turn 2: replayed tool result has no image",
    !replayed.content.some((block) => block.type === "image"),
  );
  check(
    "anthropic turn 2: replayed tool result is marked",
    replayed.content.some((block) => block.text === MARKER),
    JSON.stringify(replayed).slice(0, 200),
  );
  check(
    "anthropic turn 2: no image bytes in the request",
    !JSON.stringify(second).includes("data:image/"),
  );
  check(
    "anthropic: system prompt byte-identical",
    JSON.stringify(first.system) === JSON.stringify(second.system),
  );
  // A developer message that Codex appends mid-history must not be lifted into
  // the system prompt, and must still reach the model.
  check(
    "anthropic: appended developer message stays out of the system prompt",
    !JSON.stringify(second.system).includes("workspace_context"),
    JSON.stringify(second.system).slice(0, 200),
  );
  check(
    "anthropic: appended developer message stays in the transcript",
    JSON.stringify(second.messages).includes("workspace_context"),
  );
  check(
    "anthropic: tool preamble byte-identical",
    JSON.stringify(first.tools) === JSON.stringify(second.tools),
  );
  const stable = first.messages.length - 1;
  check(
    `anthropic: first ${stable} messages replayed byte-for-byte`,
    JSON.stringify(first.messages.slice(0, stable)) ===
      JSON.stringify(second.messages.slice(0, stable)),
  );
  check(
    "anthropic: only the previous tail was rewritten",
    JSON.stringify(first.messages[stable]) !== JSON.stringify(second.messages[stable]),
  );
  check("anthropic: turn 2 appended messages", second.messages.length > first.messages.length);
}

function verifyChat() {
  const turns = upstreamTurns("chat.ndjson");
  check("chat: two turns reached the provider", turns.length === 2, `got ${turns.length}`);
  if (turns.length !== 2) {
    return;
  }
  const [first, second] = turns;
  const toolMessages = (body) => body.messages.filter((message) => message.role === "tool");

  // Chat Completions rejects images inside `tool` messages.
  check(
    "chat: every tool message is plain text",
    [...toolMessages(first), ...toolMessages(second)].every(
      (message) => typeof message.content === "string",
    ),
  );

  const toolIndex = first.messages.findIndex((message) => message.role === "tool");
  check(
    "chat turn 1: tool message points at the relocated images",
    first.messages[toolIndex].content.includes(RELOCATED),
  );
  check(
    "chat turn 1: tool result stays adjacent to its tool_calls",
    first.messages[toolIndex - 1]?.role === "assistant" &&
      Array.isArray(first.messages[toolIndex - 1]?.tool_calls),
  );
  const relocated = first.messages[toolIndex + 1];
  check("chat turn 1: images handed over as a user message", relocated?.role === "user");
  const imagePart = (relocated?.content ?? []).find((part) => part.type === "image_url");
  check("chat turn 1: relocated message carries the image", Boolean(imagePart));
  if (imagePart) {
    const [width, height] = pngDimensions(imagePart.image_url.url);
    check(
      "chat turn 1: screenshot fits the vision budget",
      Math.max(width, height) === VISION_SIDE,
      `${width}x${height}`,
    );
  }

  check(
    "chat turn 2: replayed tool message is marked",
    toolMessages(second)[0].content.includes(MARKER),
  );
  check(
    "chat turn 2: no image bytes in the request",
    !JSON.stringify(second).includes("data:image/"),
  );
  check(
    "chat turn 2: no relocated image message remains",
    !second.messages.some(
      (message) =>
        Array.isArray(message.content) &&
        message.content.some((part) => part.type === "image_url"),
    ),
  );
}

function verifyDiagnostics() {
  // Strip ANSI so the assertions do not depend on how the gateway colours its
  // log, and surface the tail on failure instead of an empty match list.
  const log = fs
    .readFileSync(`${dir}/gateway.log`, "utf8")
    .replace(/\u001b\[[0-9;]*m/g, "");
  // Shape lines and provider-usage lines share field names, so each assertion
  // reads only the lines it is about.
  const lines = log.split("\n");
  const shape = lines
    .filter((line) => line.includes("provider prompt prefix cache"))
    .join("\n");
  const usage = lines.filter(
    (line) =>
      line.includes("provider prompt cache usage") ||
      line.includes("provider recomputed a prompt prefix"),
  );
  const states = [...shape.matchAll(/prefix_state="(\w+)"/g)].map((match) => match[1]);
  const reused = [...shape.matchAll(/reused_turns=(\d+)/g)].map((match) => Number(match[1]));
  const changed = [...shape.matchAll(/changed_regions="([^"]*)"/g)].map((match) => match[1]);
  check("diagnostics: a prefix state per turn", states.length === 4, states.join(","));
  check(
    "diagnostics: both sessions started cold",
    states.filter((state) => state === "cold_start").length === 2,
    states.join(","),
  );
  // Anthropic only loses the previous tail. Chat also rewrites the tool message
  // itself, because the relocated image message disappears with it.
  check("diagnostics: anthropic reported a tail rewrite", states.includes("tail_rewritten"), states.join(","));
  check("diagnostics: chat reported the extra rewritten turn", states.includes("turn_rewritten"), states.join(","));
  check("diagnostics: prefix reuse was measured", reused.some((value) => value > 0), reused.join(","));
  // Nothing outside the message list may drift between turns. This is the check
  // that catches a system prompt growing every turn.
  check(
    "diagnostics: no cache region drifted between turns",
    changed.length === 4 && changed.every((regions) => regions === ""),
    changed.map((regions) => regions || "-").join(" | "),
  );
  // The provider counters have to be judged against the prefix we know we kept,
  // otherwise an upstream cache eviction is indistinguishable from a gateway bug.
  check(
    "diagnostics: provider cache counters were matched to the sent prefix",
    usage.length === 2,
    `${usage.length} usage line(s)`,
  );
  const dropped = usage.filter((line) =>
    line.includes("provider recomputed a prompt prefix"),
  );
  check(
    "diagnostics: a provider dropping a preserved prefix is reported as upstream",
    dropped.length === 1 && dropped[0].includes("cache_read_tokens=3456"),
    dropped.join("\n") || "no upstream cache warning",
  );
  if (states.length !== 4) {
    console.log(`\ngateway.log tail:\n${log.trimEnd().split("\n").slice(-15).join("\n")}`);
  }
  return { states, reused };
}

function reportSize(name, label) {
  const turns = upstreamTurns(name);
  if (turns.length !== 2) {
    return;
  }
  const kib = (value) => `${(JSON.stringify(value).length / 1024).toFixed(1)} KiB`;
  console.log(`  ${label}: turn 1 ${kib(turns[0])}, turn 2 ${kib(turns[1])}`);
}

function verify() {
  verifyAnthropic();
  verifyChat();
  const { states, reused } = verifyDiagnostics();

  console.log(passes.join("\n"));
  console.log(`\nprefix_state: ${states.join(" -> ")}`);
  console.log(`reused_turns: ${reused.join(" -> ")}`);
  console.log("upstream payload size:");
  reportSize("anthropic.ndjson", "anthropic");
  reportSize("chat.ndjson", "chat     ");
  if (failures.length > 0) {
    console.log(`\n${failures.join("\n")}`);
    console.log(`\n${failures.length} prompt cache check(s) failed`);
    process.exit(1);
  }
  console.log(`\nprompt cache E2E passed (${passes.length} checks)`);
}

if (command === "build") {
  build();
} else {
  verify();
}
