const [url] = process.argv.slice(2);
if (!url) {
  throw new Error("usage: node e2e_ws_client.mjs <gateway-websocket-url>");
}

const socket = new WebSocket(url);
const terminalTypes = new Set([
  "response.completed",
  "response.failed",
  "response.incomplete",
  "error",
]);

function receiveResponse() {
  return new Promise((resolve, reject) => {
    const events = [];
    const onMessage = (message) => {
      try {
        const event = JSON.parse(String(message.data));
        events.push(event);
        if (terminalTypes.has(event.type)) {
          socket.removeEventListener("message", onMessage);
          resolve(events);
        }
      } catch (error) {
        reject(error);
      }
    };
    socket.addEventListener("message", onMessage);
  });
}

await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

const firstResponse = receiveResponse();
socket.send(
  JSON.stringify({
    type: "response.create",
    model: "shared-alpha",
    generate: false,
    input: [],
  }),
);
const firstEvents = await firstResponse;
const previousResponseId = firstEvents.find(
  (event) => event.type === "response.completed",
)?.response?.id;
if (!previousResponseId) {
  throw new Error("alpha no-op response did not complete");
}

const secondResponse = receiveResponse();
socket.send(
  JSON.stringify({
    type: "response.create",
    model: "shared-beta",
    previous_response_id: previousResponseId,
    generate: false,
    input: [],
  }),
);
const secondEvents = await secondResponse;
const failure = secondEvents.find((event) => event.type === "response.failed");
if (!failure || !JSON.stringify(failure).includes("belongs to model shared-alpha")) {
  throw new Error("cross-provider previous_response_id was not rejected");
}
socket.close();
console.log(JSON.stringify({ ok: true, previous_response_id: previousResponseId }));
