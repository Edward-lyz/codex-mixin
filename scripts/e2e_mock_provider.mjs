import fs from "node:fs";
import http from "node:http";

const [protocol, expectedKey, requestLog, readyFile, quotaValue] =
  process.argv.slice(2);
if (!protocol || !expectedKey || !requestLog || !readyFile || !quotaValue) {
  throw new Error(
    "usage: node e2e_mock_provider.mjs <anthropic|openai> <key> <request-log> <ready-file> <quota>",
  );
}

function sendJson(response, status, value) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(value));
}

function anthropicSse() {
  return [
    'event: message_start\ndata: {"type":"message_start","message":{"id":"msg_mock","type":"message","role":"assistant","content":[],"model":"shared","stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}}}\n\n',
    'event: content_block_start\ndata: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}\n\n',
    'event: content_block_delta\ndata: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"alpha"}}\n\n',
    'event: content_block_stop\ndata: {"type":"content_block_stop","index":0}\n\n',
    'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}\n\n',
    'event: message_stop\ndata: {"type":"message_stop"}\n\n',
  ].join("");
}

function openAiSse() {
  return [
    'data: {"id":"chat_mock","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"beta"},"finish_reason":null}]}\n\n',
    'data: {"id":"chat_mock","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}\n\n',
    "data: [DONE]\n\n",
  ].join("");
}

const server = http.createServer((request, response) => {
  const authorization = request.headers.authorization ?? null;
  if (authorization !== `Bearer ${expectedKey}`) {
    sendJson(response, 401, { error: "unexpected authorization" });
    return;
  }
  if (request.method === "GET" && request.url === "/quota") {
    sendJson(response, 200, { data: { used: Number(quotaValue) } });
    return;
  }
  if (request.method === "GET" && request.url === "/v1/models") {
    sendJson(response, 200, {
      object: "list",
      data: [{ id: "shared" }, { id: "hidden" }, { id: "broken" }],
    });
    return;
  }

  let raw = "";
  request.setEncoding("utf8");
  request.on("data", (chunk) => {
    raw += chunk;
  });
  request.on("end", () => {
    let body;
    try {
      body = JSON.parse(raw);
    } catch {
      sendJson(response, 400, { error: "invalid JSON" });
      return;
    }
    fs.appendFileSync(
      requestLog,
      `${JSON.stringify({ path: request.url, authorization, body })}\n`,
    );
    if (body.model === "broken") {
      sendJson(response, 502, { error: "intentional alpha failure" });
      return;
    }
    const expectedPath =
      protocol === "anthropic" ? "/v1/messages" : "/v1/chat/completions";
    if (request.method !== "POST" || request.url !== expectedPath) {
      sendJson(response, 404, { error: "unexpected path" });
      return;
    }
    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    response.end(protocol === "anthropic" ? anthropicSse() : openAiSse());
  });
});

server.listen(0, "127.0.0.1", () => {
  const address = server.address();
  if (typeof address !== "object" || address === null) {
    throw new Error("mock provider did not expose a TCP address");
  }
  fs.writeFileSync(readyFile, String(address.port));
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
