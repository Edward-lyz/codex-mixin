#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const probePath = path.join(scriptDirectory, "ducx_app_server_probe.mjs");
const imageURL =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

const requests = [];
const server = http.createServer((request, response) => {
  const chunks = [];
  request.on("data", (chunk) => chunks.push(chunk));
  request.on("end", () => {
    const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    requests.push({ headers: request.headers, body, url: request.url });
    const completed = {
      id: "resp_ducx_transport_probe",
      object: "response",
      status: "completed",
      model: body.model,
      output: [
        {
          id: "msg_ducx_transport_probe",
          type: "message",
          role: "assistant",
          status: "completed",
          content: [
            {
              type: "output_text",
              text: "DUCX_TRANSPORT_OK",
              annotations: [],
            },
          ],
        },
      ],
      usage: {
        input_tokens: 1,
        output_tokens: 1,
        total_tokens: 2,
      },
    };
    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    response.end(
      `event: response.completed\ndata: ${JSON.stringify({
        type: "response.completed",
        response: completed,
      })}\n\ndata: [DONE]\n\n`,
    );
  });
});

await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const address = server.address();
const upstreamBaseURL = `http://127.0.0.1:${address.port}/v1`;
const child = spawn(process.execPath, [probePath], {
  cwd: scriptDirectory,
  env: {
    ...process.env,
    DUCX_PROBE_ALLOW_INSTALLED_CONFIG: "1",
    DUCX_PROBE_BASE_INSTRUCTIONS: "DUCX_TRANSPORT_PROBE_BASE",
    DUCX_PROBE_ONEAPI_BASE_URL: upstreamBaseURL,
    DUCX_PROBE_RUN_TURN: "1",
    DUCX_PROBE_TURN_INPUT: JSON.stringify([
      { type: "text", text: "Describe the supplied image." },
      { type: "image", url: imageURL, detail: "high" },
    ]),
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

const exitCode = await Promise.race([
  new Promise((resolve) => child.once("exit", resolve)),
  new Promise((_, reject) =>
    setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error("DUCX transport verification timed out"));
    }, 30_000),
  ),
]);
server.close();

assert.equal(exitCode, 0, stderr);
assert.ok(requests.length > 0, "DUCX did not send a request to the mock OneAPI");
const request = requests[0];
const injectedToolNames = (request.body.input || [])
  .filter((item) => item.type === "additional_tools")
  .flatMap((item) => item.tools || [])
  .map((tool) => tool.name);
assert.match(request.url, /\/responses$/);
assert.ok(
  typeof request.headers.comate_custom_header === "string" &&
    request.headers.comate_custom_header.length > 0,
  "DUCX request is missing comate_custom_header",
);
assert.ok(
  JSON.stringify(request.body).includes(imageURL),
  "DUCX request dropped the image input",
);
assert.ok(
  JSON.stringify(request.body).includes("DUCX_TRANSPORT_PROBE_BASE"),
  "DUCX request dropped the supplied base instructions",
);
if (process.env.DUCX_VERIFY_REQUIRE_THIN === "1") {
  assert.deepEqual(
    injectedToolNames,
    [],
    `DUCX injected undeclared tools: ${injectedToolNames.join(", ")}`,
  );
}
const summary = JSON.parse(stdout);
assert.equal(summary.turnStatus, "completed");

process.stdout.write(
  `${JSON.stringify(
    {
      ok: true,
      requestPath: request.url,
      ducxHeaderPresent: true,
      multimodalImagePreserved: true,
      suppliedInstructionsPreserved: true,
      injectedToolNames,
      thinGateway: injectedToolNames.length === 0,
      turnStatus: summary.turnStatus,
    },
    null,
    2,
  )}\n`,
);
