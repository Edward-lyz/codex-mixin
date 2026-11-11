// The official CLI owns the request and connection. Copy only main-turn SSE.
(() => {
  const fs = require("fs");
  const file = process.env.CODEX_MIXIN_CLAUDE_TAP;
  delete process.env.CODEX_MIXIN_CLAUDE_TAP;
  const preload = /(^|\s)--preload(?:=|\s+)\S*claude-tap\.js/g;
  process.env.BUN_OPTIONS = (process.env.BUN_OPTIONS || "").replace(preload, "").trim();
  if (!file) return;
  const put = value => fs.appendFileSync(file, JSON.stringify(value) + "\n");
  const Real = globalThis.Bun?.FetchSession;
  if (typeof Real !== "function") { put({ tap: "unavailable" }); return; }
  async function copy(body) {
    const reader = body.getReader();
    const decoder = new TextDecoder();
    let pending = "";
    try {
      while (true) {
        const { value, done } = await reader.read();
        pending = (pending + decoder.decode(value, { stream: !done })).replace(/\r\n/g, "\n");
        let end;
        while ((end = pending.indexOf("\n\n")) >= 0) {
          const frame = pending.slice(0, end); pending = pending.slice(end + 2);
          const data = frame.split("\n").filter(l => l.startsWith("data:")).map(l => l.slice(5).trimStart()).join("\n");
          if (data && data !== "[DONE]") put({ type: "stream_event", event: JSON.parse(data), parent_tool_use_id: null });
        }
        if (pending.length > 8 * 1024 * 1024) throw Error("oversized SSE frame");
        if (done) break;
      }
    } catch { put({ type: "result", is_error: true, result: "Claude interactive response stream was interrupted" }); }
  }
  Bun.FetchSession = class extends Real {
    async fetch(input, init) {
      const res = await super.fetch(input, init);
      const url = String(input?.url || input);
      const headers = new Headers(init?.headers || input?.headers);
      if (/\/v1\/messages(?:\?|$)/.test(url) && headers.get("x-claude-code-request-class") === "main") {
        // Plan allowance rides on every main reply, including refusals.
        const limits = {};
        res.headers.forEach((value, name) => {
          if (/^anthropic-ratelimit-unified-(?:5h|7d)-(?:utilization|reset|status)$/i.test(name)) limits[name.toLowerCase()] = value;
        });
        if (Object.keys(limits).length) put({ rate: limits });
        if (!res.ok) put({ type: "result", is_error: true, result: `Claude interactive request returned HTTP ${res.status}` });
        else if (res.body && /text\/event-stream/.test(res.headers.get("content-type") || "")) {
          const [original, observed] = res.body.tee();
          Object.defineProperty(res, "body", { value: original, configurable: true });
          copy(observed);
        }
      }
      return res;
    }
  };
  put({ tap: "on" });
})();
