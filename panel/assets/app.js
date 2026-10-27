// Codex Mixin web control panel. Vanilla JS, no build step.
// Every business operation goes through /api/actions, which runs the CLI.
"use strict";

const TOKEN_KEY = "codex-mixin-panel-token";
const POLL_MS = 5000;
const TOAST_MS = 4200;
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

const PAGES = [
  { id: "home", title: "概览" },
  { id: "providers", title: "模型与服务" },
  { id: "usage", title: "用量" },
  { id: "context", title: "上下文" },
  { id: "speed", title: "测速" },
  { id: "fusion", title: "Fusion" },
  { id: "apps", title: "Agent" },
  { id: "advanced", title: "高级" },
  { id: "system", title: "系统" },
  { id: "logs", title: "日志" },
];

const state = {
  token: "",
  page: "home",
  usage: "7d",
  snapshot: null,
  snapshotKey: "",
  loadError: null,
  quota: null,
  quotaLoading: false,
  quotaError: null,
  quotaFresh: false,
  doctor: null,
  logs: null,
  context: null,
  contextError: null,
  contextLoading: false,
  logLines: 200,
  busy: false,
  timer: 0,
};

const $ = (selector, root = document) => root.querySelector(selector);

// ---------- DOM builder (text only; never parses HTML) ----------

function h(tag, props, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") element.className = value;
    else if (key === "text") element.textContent = value;
    else if (key === "style") Object.assign(element.style, value);
    else if (key === "vars") for (const [name, v] of Object.entries(value)) element.style.setProperty(name, v);
    else if (key.startsWith("on")) element.addEventListener(key.slice(2), value);
    else if (key === "checked" || key === "value" || key === "disabled" || key === "selected") element[key] = value;
    else element.setAttribute(key, value === true ? "" : String(value));
  }
  append(element, children);
  return element;
}

function append(parent, children) {
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    parent.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return parent;
}

function stagger(container) {
  [...container.children].forEach((child, index) => child.style.setProperty("--i", String(index)));
  return container;
}

// ---------- Springs ----------

// Underdamped spring step response, normalised to settle at 1.
function springCurve(stiffness, damping, samples = 48) {
  const omega = Math.sqrt(stiffness);
  const zeta = damping / (2 * omega);
  const omegaD = omega * Math.sqrt(Math.max(1 - zeta * zeta, 1e-6));
  const position = (t) =>
    1 - Math.exp(-zeta * omega * t) * (Math.cos(omegaD * t) + ((zeta * omega) / omegaD) * Math.sin(omegaD * t));
  // Duration until the envelope stays within 0.1% of rest.
  const duration = Math.log(1000) / (zeta * omega);
  const points = [];
  for (let index = 0; index <= samples; index += 1) {
    points.push(position((index / samples) * duration).toFixed(4));
  }
  points[samples] = "1";
  return { easing: `linear(${points.join(", ")})`, ms: Math.round(duration * 1000) };
}

function installSprings() {
  if (!window.CSS || !CSS.supports("transition-timing-function", "linear(0, 1)")) return;
  const root = document.documentElement.style;
  const gentle = springCurve(170, 21);
  const snappy = springCurve(380, 32);
  root.setProperty("--spring", gentle.easing);
  root.setProperty("--spring-snappy", snappy.easing);
  root.setProperty("--dur-spring", `${Math.min(gentle.ms, 760)}ms`);
}

// Physical spring driven per frame; used where the target moves while
// animating (navigation indicator), so velocity carries over smoothly.
class Spring {
  constructor(onUpdate, stiffness = 300, damping = 28) {
    this.onUpdate = onUpdate;
    this.k = stiffness;
    this.c = damping;
    this.value = null;
    this.velocity = 0;
    this.target = 0;
    this.frame = 0;
  }

  set(target) {
    this.target = target;
    if (this.value === null || reduceMotion.matches) {
      this.value = target;
      this.velocity = 0;
      this.onUpdate(target);
      return;
    }
    if (!this.frame) {
      this.last = performance.now();
      this.frame = requestAnimationFrame((now) => this.tick(now));
    }
  }

  tick(now) {
    const dt = Math.min((now - this.last) / 1000, 1 / 30);
    this.last = now;
    const force = -this.k * (this.value - this.target) - this.c * this.velocity;
    this.velocity += force * dt;
    this.value += this.velocity * dt;
    this.onUpdate(this.value);
    if (Math.abs(this.velocity) < 0.02 && Math.abs(this.value - this.target) < 0.05) {
      this.value = this.target;
      this.onUpdate(this.value);
      this.frame = 0;
      return;
    }
    this.frame = requestAnimationFrame((next) => this.tick(next));
  }
}

// ---------- Formatting ----------

const numberFormat = new Intl.NumberFormat("zh-CN");

function compact(value) {
  if (typeof value !== "number" || !Number.isFinite(value)) return "\u2014";
  if (Math.abs(value) >= 1e9) return `${(value / 1e9).toFixed(2)}B`;
  if (Math.abs(value) >= 1e6) return `${(value / 1e6).toFixed(2)}M`;
  if (Math.abs(value) >= 1e4) return `${(value / 1e3).toFixed(1)}K`;
  return numberFormat.format(value);
}

function fixed(value, digits = 1, suffix = "") {
  return typeof value === "number" && Number.isFinite(value) ? `${value.toFixed(digits)}${suffix}` : "\u2014";
}

function timeAgo(ms) {
  if (!ms) return "\u2014";
  const seconds = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (seconds < 60) return `${seconds} 秒前`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} 分钟前`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)} 小时前`;
  return new Date(ms).toLocaleDateString("zh-CN");
}

function str(value, fallback = "\u2014") {
  return value === null || value === undefined || value === "" ? fallback : String(value);
}

// ---------- API ----------

class ApiError extends Error {
  constructor(status, body) {
    const error = (body && body.error) || {};
    super(error.message || `请求失败（HTTP ${status}）`);
    this.status = status;
    this.code = error.code || "http_error";
    this.stage = error.stage ?? null;
    this.committed = error.committed ?? null;
  }
}

async function api(path, options = {}) {
  const response = await fetch(path, {
    method: options.method || "GET",
    headers: {
      Authorization: `Bearer ${state.token}`,
      ...(options.body ? { "Content-Type": "application/json" } : {}),
    },
    body: options.body ? JSON.stringify(options.body) : undefined,
    cache: "no-store",
  });
  const text = await response.text();
  let body = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = { error: { message: text.trim() || `HTTP ${response.status}` } };
  }
  if (response.status === 401) {
    showGate("令牌无效或已过期，请重新粘贴终端中的访问令牌。");
    throw new ApiError(401, body);
  }
  if (!response.ok) throw new ApiError(response.status, body);
  return body;
}

// ---------- Toasts ----------

function toast(kind, title, detail) {
  const tone = kind === "ok" ? "is-ok" : kind === "bad" ? "is-bad" : "";
  const node = h(
    "div",
    { class: "toast", role: kind === "bad" ? "alert" : null },
    h("span", { class: `dot ${tone}`, "aria-hidden": "true" }),
    h("div", { class: "toast-text" }, h("span", { class: "toast-title", text: title }), detail ? h("span", { class: "toast-detail", text: detail }) : null),
  );
  $("#toasts").append(node);
  const leave = () => {
    node.classList.add("is-leaving");
    node.addEventListener("animationend", () => node.remove(), { once: true });
    setTimeout(() => node.remove(), 400);
  };
  setTimeout(leave, kind === "bad" ? TOAST_MS * 2 : TOAST_MS);
}

function describeError(error) {
  if (error.code === "busy") return ["有操作正在执行", "请等待当前操作完成后再试。"];
  if (error.committed === true) {
    return ["已保存，但后续步骤失败", `${error.stage ? `${error.stage}：` : ""}${error.message}`];
  }
  return ["操作失败", error.message];
}

// ---------- Dialogs ----------

function closeDialog(dialog) {
  if (!dialog.open) return;
  if (reduceMotion.matches) {
    dialog.close();
    return;
  }
  dialog.classList.add("is-closing");
  const done = () => {
    dialog.classList.remove("is-closing");
    dialog.close();
  };
  dialog.addEventListener("animationend", done, { once: true });
  setTimeout(() => dialog.open && done(), 260);
}

function confirmAction(title, text, okLabel = "确认") {
  const dialog = $("#confirm");
  $("#confirm-title").textContent = title;
  $("#confirm-text").textContent = text;
  $("#confirm-ok").textContent = okLabel;
  return new Promise((resolve) => {
    const onClose = () => resolve(dialog.returnValue === "ok");
    dialog.addEventListener("close", onClose, { once: true });
    dialog.returnValue = "cancel";
    dialog.showModal();
  });
}

function openSheet(title, content) {
  const dialog = $("#sheet");
  $("#sheet-title").textContent = title;
  $("#sheet-content").replaceChildren(content);
  dialog.showModal();
  const first = content.querySelector("input:not([type=hidden]), select, textarea");
  if (first) first.focus();
}

function closeSheet() {
  closeDialog($("#sheet"));
}

// ---------- Actions ----------

// Runs one allowlisted action. `button` shows the busy state; the panel
// refreshes afterwards so the view reflects committed state.
async function runAction(button, action, params, messages = {}) {
  if (state.busy) {
    toast("bad", "有操作正在执行", "请等待当前操作完成后再试。");
    return null;
  }
  if (messages.confirm) {
    const [title, text, okLabel] = messages.confirm;
    if (!(await confirmAction(title, text, okLabel))) return null;
  }
  state.busy = true;
  document.querySelectorAll("[data-action]").forEach((node) => (node.disabled = true));
  if (button) {
    button.classList.add("is-busy");
    button.setAttribute("aria-busy", "true");
  }
  $("#view").setAttribute("aria-busy", "true");
  try {
    const result = await api("/api/actions", { method: "POST", body: params ? { action, params } : { action } });
    toast("ok", messages.ok || "已完成", lastLine(result.stdout));
    return result;
  } catch (error) {
    if (error.code === "cancelled") {
      toast("ok", "已取消等待", error.message);
    } else if (error.status !== 401) {
      const [title, detail] = describeError(error);
      toast("bad", title, detail);
    }
    return null;
  } finally {
    state.busy = false;
    if (button) {
      button.classList.remove("is-busy");
      button.removeAttribute("aria-busy");
    }
    $("#view").setAttribute("aria-busy", "false");
    await refresh(true);
    // Agent state comes from a separate CLI query; re-read it after changes.
    if (state.page === "apps") await loadAgents();
  }
}

function lastLine(text) {
  const lines = String(text || "").trim().split("\n").filter(Boolean);
  return lines.length ? lines[lines.length - 1].slice(0, 240) : "";
}

function actionButton(label, action, params, messages = {}, variant = "") {
  const button = h("button", { class: `btn ${variant}`.trim(), type: "button", "data-action": "", text: label });
  button.disabled = state.busy;
  button.addEventListener("click", () => runAction(button, action, typeof params === "function" ? params() : params, messages));
  return button;
}

// ---------- Token gate ----------

function readHash() {
  const raw = window.location.hash.replace(/^#/, "");
  const parts = raw.split("&");
  const page = parts[0] && !parts[0].includes("=") ? parts[0] : null;
  const token = new URLSearchParams(parts.filter((part) => part.includes("=")).join("&")).get("token");
  return { page, token };
}

function showGate(message) {
  sessionStorage.removeItem(TOKEN_KEY);
  state.token = "";
  clearTimeout(state.timer);
  $("#shell").hidden = true;
  $("#gate").hidden = false;
  const error = $("#gate-error");
  error.hidden = !message;
  error.textContent = message || "";
  $("#gate-token").focus();
}

function enterPanel(token) {
  state.token = token;
  sessionStorage.setItem(TOKEN_KEY, token);
  $("#gate").hidden = true;
  $("#shell").hidden = false;
  navigate(state.page, { replace: true, focus: false });
  refresh(true);
}

// ---------- Navigation ----------

let navSpring = null;

function buildNav() {
  const list = $("#nav-list");
  list.replaceChildren(
    ...PAGES.map((page) =>
      h(
        "li",
        null,
        h(
          "button",
          { class: "nav-item", type: "button", "data-page": page.id, onclick: () => navigate(page.id) },
          h("span", { text: page.title }),
        ),
      ),
    ),
  );
  const indicator = $("#nav-indicator");
  const horizontal = () => window.matchMedia("(max-width: 760px)").matches;
  navSpring = new Spring((value) => {
    indicator.style.transform = horizontal() ? `translateX(${value}px)` : `translateY(${value}px)`;
  });
  window.addEventListener("resize", () => placeIndicator(true));
}

function placeIndicator(immediate = false) {
  const active = $(`.nav-item[data-page="${state.page}"]`);
  const indicator = $("#nav-indicator");
  if (!active || !navSpring) return;
  const horizontal = window.matchMedia("(max-width: 760px)").matches;
  indicator.style.width = horizontal ? `${active.offsetWidth}px` : "";
  const target = horizontal ? active.offsetLeft : active.offsetTop;
  if (immediate) navSpring.value = null;
  navSpring.set(target);
  indicator.classList.add("is-ready");
}

function navigate(pageId, options = {}) {
  if (pageId === "models") pageId = "providers"; // merged into 模型与服务
  const page = PAGES.find((candidate) => candidate.id === pageId) || PAGES[0];
  const changed = state.page !== page.id;
  state.page = page.id;
  const url = `#${page.id}`;
  if (options.replace) history.replaceState(null, "", url);
  else if (changed) history.pushState(null, "", url);
  document.querySelectorAll(".nav-item").forEach((item) => {
    if (item.dataset.page === page.id) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  });
  document.body.dataset.page = page.id;
  $("#page-title").textContent = page.title;
  document.title = `${page.title} · Codex Mixin`;
  placeIndicator();
  render({ animate: true });
  if (changed && options.focus !== false) $("#main").focus({ preventScroll: true });
  if (page.id === "logs" && !state.logs) loadLogs();
  if (page.id === "context" && !state.context && !state.contextLoading) loadContext();
}

// ---------- Data refresh ----------

function userIsEditing() {
  const active = document.activeElement;
  if ($("#sheet").open || $("#confirm").open) return true;
  return Boolean(active && $("#view").contains(active) && active.matches("input, select, textarea"));
}

async function refresh(force = false) {
  clearTimeout(state.timer);
  if (!state.token) return;
  const button = $("#refresh");
  button.classList.add("is-spinning");
  try {
    const snapshot = await api(`/api/snapshot?usage=${state.usage}`);
    const key = JSON.stringify({ ...snapshot, refreshed_at_ms: 0 });
    const changed = key !== state.snapshotKey || state.loadError;
    state.snapshot = snapshot;
    state.snapshotKey = key;
    state.loadError = null;
    updateChrome();
    if (changed && (force || !userIsEditing())) render({ animate: false });
  } catch (error) {
    if (error.status === 401) return;
    state.loadError = error;
    updateChrome();
    if (!state.snapshot) render({ animate: false });
  } finally {
    setTimeout(() => button.classList.remove("is-spinning"), 420);
    if (state.token && document.visibilityState === "visible") {
      state.timer = setTimeout(() => refresh(false), POLL_MS);
    }
  }
}

function gatewayRunning() {
  return state.snapshot?.status?.gateway === "running";
}

function configured() {
  return state.snapshot?.status?.configured !== false;
}

function updateChrome() {
  const dot = $("#foot-dot");
  const text = $("#foot-text");
  dot.className = "dot";
  if (state.loadError) {
    dot.classList.add("is-bad");
    text.textContent = "无法读取状态";
  } else if (gatewayRunning()) {
    dot.classList.add("is-ok", "is-live");
    text.textContent = `网关运行中 · v${str(state.snapshot.status.gateway_version, "?")}`;
  } else {
    text.textContent = configured() ? "网关未运行" : "尚未配置";
  }
  const refreshed = state.snapshot?.refreshed_at_ms;
  $("#refreshed").textContent = refreshed ? new Date(refreshed).toLocaleTimeString("zh-CN", { hour12: false }) : "";
}

const RENDERERS = {};

function render({ animate }) {
  const view = $("#view");
  const scroll = window.scrollY;
  let content;
  if (!state.snapshot && !state.loadError) {
    content = [h("div", { class: "skeleton" }), h("div", { class: "grid grid-3" }, h("div", { class: "skeleton" }), h("div", { class: "skeleton" }), h("div", { class: "skeleton" }))];
  } else if (!state.snapshot) {
    content = [errorCard(state.loadError)];
  } else {
    content = [state.loadError ? errorCard(state.loadError) : null, RENDERERS[state.page]()];
  }
  view.replaceChildren(...content.flat().filter(Boolean));
  view.classList.toggle("enter", animate && !reduceMotion.matches);
  stagger(view);
  view.setAttribute("aria-busy", String(state.busy));
  if (!animate) window.scrollTo(0, scroll);
}

function errorCard(error) {
  return h(
    "div",
    { class: "notice", role: "alert" },
    h("span", { class: "dot is-bad notice-mark", "aria-hidden": "true" }),
    h("div", null, h("strong", { text: "无法读取网关状态" }), h("div", { class: "card-note", text: error.message })),
  );
}

function segmented(options, current, onSelect, label) {
  const thumb = h("span", { class: "segmented-thumb", "aria-hidden": "true" });
  const group = h("div", { class: "segmented", role: "group", "aria-label": label }, thumb);
  for (const [value, text] of options) {
    group.append(
      h("button", {
        type: "button",
        "aria-pressed": String(value === current),
        text,
        onclick: (event) => {
          moveThumb(group, event.currentTarget);
          onSelect(value);
        },
      }),
    );
  }
  requestAnimationFrame(() => moveThumb(group, group.querySelector('[aria-pressed="true"]')));
  return group;
}

function moveThumb(group, button) {
  if (!button) return;
  group.querySelectorAll("button").forEach((node) => node.setAttribute("aria-pressed", String(node === button)));
  const thumb = group.querySelector(".segmented-thumb");
  thumb.style.width = `${button.offsetWidth}px`;
  thumb.style.transform = `translateX(${button.offsetLeft}px)`;
}

function card(title, note, body, actions, extraClass = "") {
  return h(
    "section",
    { class: `card ${extraClass}`.trim(), "aria-label": title },
    title || actions ? h("header", { class: "card-head" }, h("div", null, h("h2", { class: "card-title", text: title }), note ? h("p", { class: "card-note", text: note }) : null), actions ? h("div", { class: "card-actions" }, actions) : null) : null,
    body,
  );
}

function emptyState(title, text, action) {
  return h("div", { class: "empty" }, h("p", { class: "empty-title", text: title }), text ? h("p", { text }) : null, action || null);
}

function field(label, control, hint) {
  // Groups of chips already contain their own <label>s; wrap them in a
  // labelled group instead so clicking the caption toggles nothing.
  const group = control.classList.contains("chips") || control.classList.contains("chip");
  return h(
    group ? "div" : "label",
    { class: "field", role: group ? "group" : null, "aria-label": group ? label : null },
    h("span", { class: "field-label", text: label, "aria-hidden": group ? "true" : null }),
    control,
    hint ? h("span", { class: "field-hint", text: hint }) : null,
  );
}

function selectControl(name, options, value) {
  return h(
    "div",
    { class: "select-wrap" },
    h("select", { class: "select", name }, ...options.map(([optionValue, text]) => h("option", { value: optionValue, text, selected: optionValue === value }))),
  );
}

function textInput(name, value = "", props = {}) {
  return h("input", { class: "input", name, value, spellcheck: "false", autocomplete: "off", ...props });
}

function secretInput(name, placeholder = "") {
  return h("input", { class: "input mono", name, type: "password", autocomplete: "new-password", spellcheck: "false", placeholder });
}

function formValues(form) {
  const values = {};
  for (const element of form.elements) {
    if (!element.name) continue;
    if (element.type === "checkbox") {
      if (element.dataset.list) {
        values[element.name] = values[element.name] || [];
        if (element.checked) values[element.name].push(element.value);
      } else values[element.name] = element.checked;
    } else if (element.type === "radio") {
      if (element.checked) values[element.name] = element.value;
    } else values[element.name] = element.value.trim();
  }
  return values;
}

function submitButton(label) {
  return h("button", { class: "btn btn-primary", type: "submit", "data-action": "", text: label, disabled: state.busy });
}

// ---------- Home ----------

RENDERERS.home = () => {
  const status = state.snapshot.status || {};
  if (!configured()) {
    return card(null, null, emptyState("还没有可用的 Provider", "添加第一个 Provider 后，网关即可为 Codex 等 Agent 提供模型。", h("button", { class: "btn btn-primary", type: "button", text: "添加 Provider", onclick: () => { navigate("providers"); openAddProvider(); } })));
  }
  const running = gatewayRunning();
  const counts = status.provider_counts || {};
  return [
    exposureNotice(),
    h(
      "section",
      { class: "card hero", "aria-label": "网关状态" },
      h(
        "div",
        null,
        h("div", { class: "hero-state" }, h("span", { class: `dot dot-lg ${running ? "is-ok is-live" : ""}`, "aria-hidden": "true" }), h("span", { class: "hero-label", text: running ? "网关运行中" : "网关已停止" })),
        h(
          "div",
          { class: "hero-meta" },
          h("span", null, "接口 ", h("b", { class: "mono", text: str(status.endpoint) })),
          h("span", null, "监听 ", h("b", { class: "mono", text: str(status.bind) })),
          status.pid ? h("span", null, "PID ", h("b", { class: "mono", text: String(status.pid) })) : null,
          status.gateway_version ? h("span", null, "版本 ", h("b", { class: "mono", text: status.gateway_version })) : null,
        ),
      ),
      h(
        "div",
        { class: "card-actions" },
        running && status.endpoint ? h("button", { class: "btn btn-ghost", type: "button", text: "复制接口", onclick: () => copyText(status.endpoint) }) : null,
        running
          ? [actionButton("重启", "service_restart", null, { ok: "网关已重启" }), actionButton("停止", "service_stop", null, { ok: "网关已停止" }, "btn-danger")]
          : actionButton("启动网关", "service_start", null, { ok: "网关已启动" }, "btn-primary"),
      ),
    ),
    stagger(
      h(
        "div",
        { class: "grid grid-3" },
        metricCard("Providers", counts.total, `${str(counts.healthy, 0)} 健康 · ${str(counts.degraded, 0)} 降级 · ${str(counts.disabled, 0)} 停用`),
        metricCard("可用模型", (state.snapshot.models || []).length),
        metricCard("Codex 接入", codexModeLabel(state.snapshot.codex_install_mode)),
      ),
    ),
    usageCard(),
    quotaCard(),
  ];
};

function exposureNotice() {
  const bind = String(state.snapshot.status?.bind || state.snapshot.gateway_bind || "");
  const open = bind.startsWith("0.0.0.0") || bind.startsWith("[::]");
  const enforced = state.snapshot.panel?.client_auth_build && state.snapshot.gateway_auth_configured;
  if (!open || enforced) return null;
  return h(
    "div",
    { class: "notice", role: "note" },
    h("span", { class: "dot is-bad notice-mark", "aria-hidden": "true" }),
    h("div", null, h("strong", { text: "网关对局域网开放且不校验访问密钥" }), h("div", { class: "card-note", text: "同一网络中的其他设备可以直接使用你的 Provider 额度。需要限制时，使用 client-auth 构建并设置网关密钥，或用 --bind 127.0.0.1:PORT 只监听本机。" })),
  );
}

function metricCard(label, value, sub) {
  return h("div", { class: "card card-tight metric" }, h("span", { class: "metric-label", text: label }), h("span", { class: "metric-value", text: typeof value === "number" ? compact(value) : str(value) }), sub ? h("span", { class: "metric-sub", text: sub }) : null);
}

function codexModeLabel(mode) {
  if (mode === "codex_oauth_proxy" || mode === "oauth_proxy") return "官方账号 + 自定义";
  if (mode === "custom_only") return "仅自定义";
  return mode ? String(mode) : "未接入";
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast("ok", "已复制", text);
  } catch {
    toast("bad", "无法访问剪贴板", "浏览器只允许在安全上下文中写入剪贴板，请手动复制。");
  }
}

// The overview keeps a short summary; the usage page has the full table.
const HOME_USAGE_ROWS = 5;

function usageCard() {
  const rows = [...(state.snapshot.usage || [])].sort((a, b) => (b.request_count || 0) - (a.request_count || 0));
  const range = segmented(
    [["1d", "1 天"], ["7d", "7 天"], ["30d", "30 天"], ["all", "全部"]],
    state.usage,
    (value) => {
      state.usage = value;
      refresh(true);
    },
    "用量时间范围",
  );
  let body;
  if (!gatewayRunning()) body = emptyState("网关未运行", "启动网关后即可查看请求与 Token 用量。");
  else if (!rows.length) body = emptyState("暂无用量", "所选时间范围内还没有经过网关的请求。");
  else {
    const total = (key) => rows.reduce((sum, row) => sum + (row[key] || 0), 0);
    body = h(
      "div",
      { class: "table-wrap" },
      h(
        "table",
        { class: "table" },
        h("thead", null, h("tr", null, ...["Provider / 模型", "请求", "输入", "缓存命中", "输出", "首字延迟", "输出速度"].map((text, index) => h("th", { class: index ? "r" : null, scope: "col", text })))),
        h(
          "tbody",
          null,
          ...rows.slice(0, HOME_USAGE_ROWS).map((row) =>
            h(
              "tr",
              null,
              h("td", null, h("div", { text: row.model_id }), h("div", { class: "row-sub", text: row.provider_id })),
              h("td", { class: "r", text: compact(row.request_count) }),
              h("td", { class: "r", text: compact(row.input_tokens) }),
              h("td", { class: "r", text: fixed(row.cache_hit_percent, 1, "%") }),
              h("td", { class: "r", text: compact(row.output_tokens) }),
              h("td", { class: "r", text: fixed(row.average_ttft_ms, 0, " ms") }),
              h("td", { class: "r", text: fixed(row.output_tps, 1, " t/s") }),
            ),
          ),
        ),
        h("tfoot", null, h("tr", null, h("td", { text: "合计" }), h("td", { class: "r", text: compact(total("request_count")) }), h("td", { class: "r", text: compact(total("input_tokens")) }), h("td", { class: "r muted", text: "" }), h("td", { class: "r", text: compact(total("output_tokens")) }), h("td", { class: "r muted", text: "" }), h("td", { class: "r muted", text: "" }))),
      ),
    );
  }
  const details = h("button", { class: "btn btn-ghost btn-sm", type: "button", text: "查看明细", onclick: () => navigate("usage") });
  return card("用量", rows.length > HOME_USAGE_ROWS ? `用量最高的 ${HOME_USAGE_ROWS} 个模型` : null, body, [range, details], "card-flush");
}

// ---------- Usage ----------
// DeepSeek-style usage console: range and provider filters, totals, daily
// charts that aggregate or split by provider, and the per-turn ledger the CLI
// records for each upstream reply.

const usagePage = { days: "7", provider: "", split: false, session: "", data: null, loading: false, error: null };
const USAGE_DAYS = [["1", "1 天"], ["7", "7 天"], ["30", "30 天"]];
const DAY_MS = 86_400_000;
// Prompts smaller than this are too short for provider caches to act on, so
// a miss there is not worth flagging (matches the gateway's own threshold).
const CACHE_VERDICT_MIN_PROMPT = 8_192;
const CACHE_LOW_PERCENT = 50;
const SERIES_COLORS = 4;

// Each load gets a sequence number; a response for a superseded range is
// dropped, so a fast range switch never renders old data on the new axis.
let usageRequest = 0;

async function loadUsageDetails() {
  const request = ++usageRequest;
  const days = usagePage.days;
  usagePage.loading = true;
  usagePage.error = null;
  try {
    const data = await api(`/api/usage?days=${days}`);
    if (request === usageRequest) usagePage.data = data;
  } catch (error) {
    if (request === usageRequest) usagePage.error = error.message;
  } finally {
    if (request === usageRequest) {
      usagePage.loading = false;
      if (state.page === "usage") render({ animate: false });
    }
  }
}

function tokenTotal(row) {
  return (row.input_tokens || 0) + (row.cache_read_tokens || 0) + (row.cache_creation_tokens || 0) + (row.output_tokens || 0);
}

function dayLabel(day) {
  const date = new Date(day * DAY_MS);
  return `${date.getMonth() + 1}/${date.getDate()}`;
}

// Daily rows -> one bucket per day in range (missing days stay at zero),
// each holding totals per series key (provider id, or "all" when aggregated).
function dailyBuckets(rows) {
  const today = Math.floor(Date.now() / DAY_MS);
  const days = Number(usagePage.days);
  const buckets = [];
  for (let day = today - days + 1; day <= today; day += 1) buckets.push({ day, series: new Map() });
  const index = new Map(buckets.map((bucket) => [bucket.day, bucket]));
  for (const row of rows) {
    const bucket = index.get(row.day);
    if (!bucket) continue;
    const key = usagePage.split ? row.provider_id : "all";
    const total = bucket.series.get(key) || { tokens: 0, requests: 0 };
    total.tokens += tokenTotal(row);
    total.requests += row.request_count || 0;
    bucket.series.set(key, total);
  }
  return buckets;
}

function seriesKeys(buckets) {
  const sums = new Map();
  for (const bucket of buckets) for (const [key, total] of bucket.series) sums.set(key, (sums.get(key) || 0) + total.tokens);
  return [...sums.entries()].sort((a, b) => b[1] - a[1]).map(([key]) => key);
}

function seriesClass(keys, key) {
  const index = keys.indexOf(key);
  return index >= 0 && index < SERIES_COLORS ? `ring-${index + 1}` : "is-extra";
}

function barChart(title, buckets, keys, metric, format) {
  const max = Math.max(1, ...buckets.map((bucket) => [...bucket.series.values()].reduce((sum, total) => sum + total[metric], 0)));
  const sum = buckets.reduce((all, bucket) => all + [...bucket.series.values()].reduce((part, total) => part + total[metric], 0), 0);
  const labelEvery = Math.ceil(buckets.length / 8);
  const columns = buckets.map((bucket, index) => {
    const total = [...bucket.series.values()].reduce((part, value) => part + value[metric], 0);
    const stack = h("div", { class: "uchart-stack" }, keys.filter((key) => bucket.series.has(key)).map((key) =>
      h("span", { class: `uchart-seg ${seriesClass(keys, key)}`, style: { height: `${(bucket.series.get(key)[metric] / max) * 100}%` } })));
    return h("div", { class: "uchart-col", title: `${dayLabel(bucket.day)} · ${format(total)}` },
      stack,
      h("span", { class: "uchart-x", text: index % labelEvery === 0 || index === buckets.length - 1 ? dayLabel(bucket.day) : "" }));
  });
  return h("section", { class: "uchart", "aria-label": `${title}：合计 ${format(sum)}` },
    h("div", { class: "uchart-head" }, h("span", { class: "metric-label", text: title }), h("strong", { class: "num", text: format(sum) })),
    h("div", { class: "uchart-plot", "aria-hidden": "true" }, columns));
}

function usageLegend(keys) {
  if (!usagePage.split || !keys.length) return null;
  return h("ul", { class: "uchart-legend" }, keys.map((key) =>
    h("li", null, h("span", { class: `quota-swatch ${seriesClass(keys, key)}`, "aria-hidden": "true" }), h("span", { text: key }))));
}

function reasoningText(turn) {
  if (typeof turn.reasoning_tokens === "number") return compact(turn.reasoning_tokens);
  // Anthropic-style upstreams stream thinking without a token counter.
  return turn.reasoning_chars ? `${compact(turn.reasoning_chars)} 字` : "\u2014";
}

function cacheIsLow(turn) {
  const prompt = (turn.input_tokens || 0) + (turn.cache_read_tokens || 0) + (turn.cache_creation_tokens || 0);
  return typeof turn.cache_hit_percent === "number" && prompt >= CACHE_VERDICT_MIN_PROMPT && turn.cache_hit_percent < CACHE_LOW_PERCENT;
}

function turnTime(ms) {
  return new Date(ms).toLocaleString("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
}

function turnsTable(turns) {
  if (!turns.length) return emptyState("暂无回复记录", "所选范围内还没有经过网关的模型回复。");
  const sessionButton = (turn) => h("button", {
    class: "chip-btn mono",
    type: "button",
    text: turn.session_display || turn.session,
    title: usagePage.session ? "显示全部会话" : "只看这个会话",
    onclick: () => { usagePage.session = usagePage.session ? "" : turn.session; render({ animate: false }); },
  });
  const modelCell = (turn) => {
    const mismatch = turn.model_matches === false;
    return h("td", null,
      h("div", { text: turn.upstream_model_id }),
      h("div", { class: "row-sub" }, turn.provider_id,
        mismatch ? h("span", { class: "badge badge-bad", title: "上游返回的模型与请求的模型不一致", text: `返回 ${turn.returned_model}` }) : null));
  };
  return h("div", { class: "table-wrap" }, h("table", { class: "table turns" },
    h("thead", null, h("tr", null, ...["时间", "会话", "模型", "未缓存输入", "命中缓存", "输出", "思考", "首字", "速度"].map((text, index) => h("th", { class: index > 2 ? "r" : null, scope: "col", text })))),
    h("tbody", null, ...turns.map((turn) => h("tr", { class: turn.model_matches === false ? "is-flagged" : null },
      h("td", { class: "mono nowrap", text: turnTime(turn.at_ms) }),
      h("td", null, sessionButton(turn)),
      modelCell(turn),
      h("td", { class: "r", text: compact(turn.input_tokens) }),
      h("td", { class: `r ${cacheIsLow(turn) ? "is-low" : ""}`.trim(), title: cacheIsLow(turn) ? "长提示的缓存命中低于 50%" : null },
        h("div", { text: compact(turn.cache_read_tokens) }), h("div", { class: "row-sub", text: fixed(turn.cache_hit_percent, 1, "%") })),
      h("td", { class: "r", text: compact(turn.output_tokens) }),
      h("td", { class: "r", text: reasoningText(turn) }),
      h("td", { class: "r", text: fixed(turn.ttft_ms, 0, " ms") }),
      h("td", { class: "r", text: fixed(turn.output_tps, 1, " t/s") }),
    )))));
}

RENDERERS.usage = () => {
  if (!gatewayRunning()) return card(null, null, emptyState("网关未运行", "启动网关后即可查看用量明细。"));
  if (!usagePage.data && !usagePage.loading && !usagePage.error) queueMicrotask(loadUsageDetails);
  const reload = (patch) => { Object.assign(usagePage, patch); loadUsageDetails(); };
  const daily = usagePage.data?.daily || [];
  const allTurns = usagePage.data?.turns || [];
  const providers = [...new Set([...daily.map((row) => row.provider_id), ...allTurns.map((turn) => turn.provider_id)])].sort();
  const byProvider = (row) => !usagePage.provider || row.provider_id === usagePage.provider;
  const rows = daily.filter(byProvider);
  const turns = allTurns.filter(byProvider).filter((turn) => !usagePage.session || turn.session === usagePage.session);

  const providerSelect = h("select", { class: "select select-sm", "aria-label": "Provider", onchange: (event) => { usagePage.provider = event.target.value; usagePage.session = ""; render({ animate: false }); } },
    h("option", { value: "", text: "全部 Provider", selected: !usagePage.provider }),
    providers.map((id) => h("option", { value: id, text: id, selected: usagePage.provider === id })));
  const toolbar = h("div", { class: "usage-toolbar" },
    segmented(USAGE_DAYS, usagePage.days, (days) => reload({ days, session: "" }), "时间范围"),
    providerSelect,
    segmented([["merge", "聚合"], ["split", "按 Provider 拆分"]], usagePage.split ? "split" : "merge", (value) => { usagePage.split = value === "split"; render({ animate: false }); }, "图表分组"),
    h("button", { class: `btn btn-ghost btn-sm ${usagePage.loading ? "is-busy" : ""}`.trim(), type: "button", text: "刷新", disabled: usagePage.loading, onclick: () => loadUsageDetails() }));

  if (usagePage.error) return [toolbar, card(null, null, h("p", { class: "issue", text: usagePage.error }))];
  if (!usagePage.data) return [toolbar, h("div", { class: "skeleton" })];

  const sum = (key) => rows.reduce((total, row) => total + (row[key] || 0), 0);
  const cacheRows = rows.filter((row) => typeof row.cache_hit_percent === "number");
  const cached = cacheRows.reduce((total, row) => total + (row.cache_read_tokens || 0), 0);
  const prompt = cached + cacheRows.reduce((total, row) => total + (row.input_tokens || 0) + (row.cache_creation_tokens || 0), 0);
  const mismatches = turns.filter((turn) => turn.model_matches === false).length;
  const buckets = dailyBuckets(rows);
  const keys = seriesKeys(buckets);
  return [
    toolbar,
    stagger(h("div", { class: "grid grid-4" },
      metricCard("请求次数", sum("request_count"), `${providers.length} 个 Provider`),
      metricCard("Tokens", rows.reduce((total, row) => total + tokenTotal(row), 0), `输出 ${compact(sum("output_tokens"))}`),
      metricCard("缓存命中", prompt ? `${((cached / prompt) * 100).toFixed(1)}%` : "\u2014", `命中 ${compact(cached)} tokens`),
      metricCard("模型不一致", mismatches, mismatches ? "上游返回了其他模型" : "所有回复与请求一致"))),
    card("每日用量", usagePage.split ? "按 Provider 拆分" : "全部 Provider 聚合", h("div", { class: "uchart-grid" },
      barChart("Tokens", buckets, keys, "tokens", compact),
      barChart("请求次数", buckets, keys, "requests", compact),
      usageLegend(keys))),
    card("回复明细", usagePage.session ? `会话 ${turns[0]?.session_display || usagePage.session} · ${turns.length} 条` : `最近 ${turns.length} 条回复；点击会话标签只看该会话`, turnsTable(turns), null, "card-flush"),
  ];
};

// ---------- Quota ----------
// Rings mirror Apple Activity: one concentric ring per quota window of a
// provider. Bars keep the classic linear view. Both read the same CLI rows.

const QUOTA_VIEW_KEY = "codex-mixin-quota-view";
const QUOTA_VIEWS = [["ring", "圆环"], ["bar", "条形"]];
const QUOTA_WARN_RATIO = 0.9;
// Ring geometry in SVG user units; more than four rings stop being legible.
const RING_SIZE = 120;
// Rings share a fixed band so the hole keeps room for the percentage label;
// more rings means thinner strokes rather than a smaller center.
const RING_STROKE_MAX = 11;
const RING_BAND = 30;
const RING_GAP = 3;
const RING_MAX = 4;
const QUOTA_LABELS = { five_hour: "5 小时", weekly: "每周", monthly: "每月", balance: "余额", quota: "额度" };
const SVG_NS = "http://www.w3.org/2000/svg";
const amountFormat = new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 2 });

function quotaView() {
  try {
    return localStorage.getItem(QUOTA_VIEW_KEY) === "bar" ? "bar" : "ring";
  } catch {
    return "ring";
  }
}

function finiteAmount(value) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

// Normalise one CLI quota row; ratio is null when no limit is known.
function quotaEntry(row) {
  const used = finiteAmount(row.used ?? row.value);
  const limit = finiteAmount(row.limit);
  const remaining = finiteAmount(row.remaining);
  const percent = row.unit === "percent";
  let ratio = null;
  if (limit && used !== null) ratio = used / limit;
  else if (limit && remaining !== null) ratio = (limit - remaining) / limit;
  const id = row.quota_id || "quota";
  const label = QUOTA_LABELS[id] || row.label || row.display_name || id;
  const currency = percent ? "" : str(row.currency, "");
  let value = "额度未知";
  if (row.error) value = "查询失败";
  else if (percent && used !== null) value = `剩余 ${amountFormat.format(Math.max(100 - used, 0))}%`;
  else if (used !== null && limit) value = `${amountFormat.format(used)} / ${amountFormat.format(limit)} ${currency}`.trim();
  else if (remaining !== null) value = `余额 ${amountFormat.format(remaining)} ${currency}`.trim();
  else if (used !== null) value = `已用 ${amountFormat.format(used)} ${currency}`.trim();
  const detail = !row.error && !percent && remaining !== null && limit ? `剩余 ${amountFormat.format(remaining)} ${currency}`.trim() : "";
  const reset = row.reset_at ? new Date(row.reset_at) : null;
  return {
    label,
    value,
    detail,
    error: row.error ? String(row.error) : "",
    ratio: ratio === null || !Number.isFinite(ratio) ? null : Math.max(ratio, 0),
    reset: reset && !Number.isNaN(reset.getTime()) ? `${reset.toLocaleString("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" })} 重置` : "",
  };
}

function quotaGroups(rows) {
  const groups = new Map();
  for (const row of rows) {
    const id = row.provider_id || "unknown";
    if (!groups.has(id)) groups.set(id, { id, name: row.provider_display_name || row.display_name || id, entries: [] });
    groups.get(id).entries.push(quotaEntry(row));
  }
  return [...groups.values()];
}

function svgNode(tag, attrs) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value));
  return node;
}

function percentText(ratio) {
  return ratio === null ? "\u2014" : `${Math.round(ratio * 100)}%`;
}

function quotaRings(entries, animate) {
  const center = RING_SIZE / 2;
  const svg = svgNode("svg", { viewBox: `0 0 ${RING_SIZE} ${RING_SIZE}`, class: "quota-rings", "aria-hidden": "true" });
  const turn = svgNode("g", { transform: `rotate(-90 ${center} ${center})` });
  svg.append(turn);
  const shown = entries.slice(0, RING_MAX);
  const stroke = Math.min(RING_STROKE_MAX, (RING_BAND - RING_GAP * (shown.length - 1)) / shown.length);
  shown.forEach((entry, index) => {
    const radius = center - stroke / 2 - index * (stroke + RING_GAP);
    const circumference = 2 * Math.PI * radius;
    const idle = entry.error || entry.ratio === null ? "is-idle" : "";
    const ring = svgNode("g", { class: `quota-ring ring-${index + 1} ${idle}`.trim() });
    ring.style.setProperty("--ring-stroke", String(stroke));
    ring.append(svgNode("circle", { class: "quota-ring-track", cx: center, cy: center, r: radius }));
    const fill = Math.min(entry.ratio ?? 0, 1);
    if (fill > 0) {
      const target = circumference * (1 - fill);
      const arc = svgNode("circle", { class: "quota-ring-fill", cx: center, cy: center, r: radius, "stroke-dasharray": circumference.toFixed(2), "stroke-dashoffset": (animate ? circumference : target).toFixed(2) });
      if (animate) requestAnimationFrame(() => requestAnimationFrame(() => arc.setAttribute("stroke-dashoffset", target.toFixed(2))));
      ring.append(arc);
    }
    turn.append(ring);
  });
  return svg;
}

function quotaLegend(entries, view) {
  return h("ul", { class: "quota-legend" }, entries.map((entry, index) => {
    const tone = entry.error ? "is-bad" : entry.ratio !== null && entry.ratio >= QUOTA_WARN_RATIO ? "is-warn" : "";
    const swatch = view === "ring" ? h("span", { class: `quota-swatch ${index < RING_MAX ? `ring-${index + 1}` : "is-extra"}`, "aria-hidden": "true" }) : null;
    const bar = view === "bar" && entry.ratio !== null && !entry.error
      ? h("div", { class: "progress quota-bar", role: "progressbar", "aria-label": `${entry.label}已用`, "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(Math.round(Math.min(entry.ratio, 1) * 100)) },
        h("div", { class: `progress-bar ${tone}`.trim(), style: { transform: `scaleX(${Math.min(entry.ratio, 1)})` } }))
      : null;
    return h("li", { class: `quota-entry ${tone}`.trim(), title: entry.error || null },
      h("div", { class: "quota-entry-head" },
        swatch,
        h("span", { class: "quota-entry-label", text: entry.label }),
        h("span", { class: "quota-entry-pct num", text: entry.error ? "" : percentText(entry.ratio) })),
      h("div", { class: "quota-entry-value num", text: entry.value }),
      bar,
      entry.error ? h("p", { class: "quota-entry-note", text: entry.error }) : null,
      !entry.error && (entry.detail || entry.reset) ? h("p", { class: "quota-entry-note", text: [entry.detail, entry.reset].filter(Boolean).join(" · ") }) : null);
  }));
}

function quotaTile(group, view, animate) {
  // The center shows the tightest window, the one that runs out first.
  const ringed = group.entries.filter((entry) => !entry.error && entry.ratio !== null);
  const primary = ringed.reduce((top, entry) => (!top || entry.ratio > top.ratio ? entry : top), null);
  const label = group.entries.map((entry) => `${entry.label} ${entry.error ? "查询失败" : `${entry.value}${entry.ratio === null ? "" : `，已用 ${percentText(entry.ratio)}`}`}`).join("；");
  const visual = view === "ring"
    ? h("div", { class: "quota-visual" }, quotaRings(group.entries, animate),
      h("div", { class: "quota-center" }, h("strong", { class: `num ${primary && primary.ratio >= QUOTA_WARN_RATIO ? "is-warn" : ""}`.trim(), text: primary ? percentText(primary.ratio) : "\u2014" })))
    : null;
  return h("article", { class: `quota-tile quota-tile-${view}`, "aria-label": `${group.name}：${label}` },
    visual,
    h("div", { class: "quota-body" }, h("h3", { class: "quota-name", text: group.name }), quotaLegend(group.entries, view)));
}

async function loadQuota() {
  if (state.quotaLoading) return;
  state.quotaLoading = true;
  state.quotaError = null;
  if (state.page === "home") render({ animate: false });
  try {
    state.quota = await api("/api/quota");
    state.quotaFresh = true;
  } catch (error) {
    state.quotaError = error.message;
    if (error.status !== 401) toast("bad", "额度查询失败", error.message);
  } finally {
    state.quotaLoading = false;
    if (state.page === "home") render({ animate: false });
  }
}

function quotaCard() {
  // First visit queries once; later refreshes are explicit to spare remote quota APIs.
  if (state.quota === null && !state.quotaLoading && state.quotaError === null) queueMicrotask(loadQuota);
  const view = quotaView();
  const toggle = segmented(QUOTA_VIEWS, view, (value) => {
    try { localStorage.setItem(QUOTA_VIEW_KEY, value); } catch { /* private mode keeps the default */ }
    state.quotaFresh = true;
    render({ animate: false });
  }, "额度展示方式");
  const reload = h("button", { class: `btn btn-ghost btn-sm ${state.quotaLoading ? "is-busy" : ""}`.trim(), type: "button", text: state.quota ? "重新查询" : "查询额度", disabled: state.quotaLoading, onclick: loadQuota });
  let body;
  if (state.quotaLoading && !state.quota) body = h("p", { class: "card-note", text: "正在向 Provider 查询额度…" });
  else if (!state.quota) body = h("p", { class: state.quotaError ? "issue" : "card-note", text: state.quotaError || "向支持额度接口的 Provider 查询额度用量。" });
  else if (!state.quota.length) body = h("p", { class: "card-note", text: "没有已启用的 Provider 提供额度接口。" });
  else {
    const animate = state.quotaFresh && !reduceMotion.matches;
    state.quotaFresh = false;
    body = h("div", { class: `quota-grid quota-grid-${view}` }, quotaGroups(state.quota).map((group) => quotaTile(group, view, animate)));
  }
  return card("额度", null, body, [toggle, reload]);
}

// Generic, read-only rendering of CLI JSON whose shape varies by provider.
function valueView(value, depth = 0) {
  if (Array.isArray(value)) {
    if (!value.length) return h("span", { class: "card-note", text: "无" });
    return h("div", { class: "grid", style: { gap: "12px" } }, ...value.map((item) => valueView(item, depth + 1)));
  }
  if (value && typeof value === "object") {
    const list = h("dl", { class: "kv" });
    for (const [key, item] of Object.entries(value)) {
      list.append(h("dt", { text: key.replaceAll("_", " ") }), h("dd", null, item && typeof item === "object" && depth < 3 ? valueView(item, depth + 1) : h("span", { class: typeof item === "number" ? "num" : null, text: item && typeof item === "object" ? JSON.stringify(item) : str(item) })));
    }
    return list;
  }
  return h("span", { text: str(value) });
}

// ---------- Models and services ----------
// Mirrors the macOS "模型与服务" window: provider list on the left, a
// models table and connection settings for the selected provider.

function providerCatalog() {
  const catalog = state.snapshot?.provider_catalog;
  if (!catalog || !Array.isArray(catalog.presets) || !Array.isArray(catalog.protocols)
      || !catalog.presets.some((preset) => preset.id === catalog.default_preset)
      || !catalog.protocols.some((protocol) => protocol.id === catalog.default_protocol)) {
    throw new Error("CLI provider catalog is missing or invalid; update the complete app bundle.");
  }
  return catalog;
}
function providerProtocols() {
  return providerCatalog().protocols.map((protocol) => [protocol.id, protocol.display_name, protocol.api_path]);
}
function providerDefinition(id) {
  const definition = providerCatalog().presets.find((preset) => preset.id === id);
  if (!definition) throw new Error(`CLI provider catalog does not describe ${id}`);
  if (!["api_key", "local_oauth", "aws_sigv4"].includes(definition.auth_mode)
      || ["api_key_required", "custom_endpoint", "quota_username_required", "quota_credentials_required"]
        .some((flag) => typeof definition[flag] !== "boolean")) {
    throw new Error(`CLI provider catalog contains an invalid definition for ${id}`);
  }
  return definition;
}
const ISSUE_LABELS = {
  credentials_missing: "缺少凭据",
  no_routable_models: "没有可路由模型",
  selected_models_unavailable: "所选模型不可用",
  model_refresh_failed: "模型刷新失败",
};
const CONTEXT_UNIT = 1000;

const svc = {
  selected: null,
  tab: "models",
  filter: "all",
  query: "",
  sort: { key: "selected", reversed: false },
  benchMode: 1,
  benchTimeout: 60,
  manualOpen: false,
  drafts: {}, // provider id -> { models:Set, contexts:{id:tokens|null}, manual:Set }
};

function configuredProviders() {
  return (state.snapshot.providers || []).filter((provider) => provider.kind === "configured");
}

function providerName(provider) {
  return provider.display_name || provider.id;
}

function providerIcon(provider, size = 28) {
  const icon = h("img", { class: "svc-icon", src: `/provider-icons/${encodeURIComponent(provider.icon || "custom")}`, alt: "", width: String(size), height: String(size) });
  icon.addEventListener("error", () => icon.replaceWith(h("span", { class: "svc-icon svc-icon-fallback", "aria-hidden": "true", text: providerName(provider).slice(0, 1) })), { once: true });
  return icon;
}

function providerHealth(provider) {
  if (!provider.enabled) return { tone: "", text: "停用" };
  if (provider.readiness === "degraded") return { tone: "is-bad", text: "需要处理" };
  return { tone: "is-ok", text: "正常" };
}

function draftFor(provider) {
  if (!svc.drafts[provider.id]) {
    svc.drafts[provider.id] = { models: new Set(provider.selected_models || []), contexts: {}, manual: new Set() };
  }
  return svc.drafts[provider.id];
}

// Pending model changes compared with the saved provider document.
function modelChanges(provider) {
  const draft = svc.drafts[provider.id];
  if (!draft) return 0;
  const saved = new Set(provider.selected_models || []);
  let count = Object.keys(draft.contexts).length;
  for (const id of draft.models) if (!saved.has(id)) count += 1;
  for (const id of saved) if (!draft.models.has(id)) count += 1;
  return count;
}

// Small popover menu anchored to a button; closes on outside click or Esc.
function popoverMenu(label, items, buttonClass = "btn btn-ghost btn-icon") {
  const button = h("button", { class: buttonClass, type: "button", "aria-haspopup": "menu", "aria-expanded": "false", "aria-label": label, text: "\u22ef" });
  const menu = h("div", { class: "menu", role: "menu", hidden: true });
  const close = () => {
    menu.hidden = true;
    button.setAttribute("aria-expanded", "false");
    document.removeEventListener("pointerdown", outside, true);
  };
  const outside = (event) => {
    if (!menu.contains(event.target) && event.target !== button) close();
  };
  for (const item of items) {
    if (item === "-") {
      menu.append(h("div", { class: "menu-sep", role: "separator" }));
      continue;
    }
    if (item.heading) {
      menu.append(h("div", { class: "menu-heading", text: item.heading }));
      continue;
    }
    menu.append(
      h("button", {
        class: `menu-item ${item.checked ? "is-checked" : ""}`.trim(),
        type: "button",
        role: item.checked === undefined ? "menuitem" : "menuitemradio",
        "aria-checked": item.checked === undefined ? null : String(item.checked),
        disabled: item.disabled,
        text: item.label,
        onclick: () => {
          close();
          item.run();
        },
      }),
    );
  }
  menu.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      close();
      button.focus();
    }
  });
  button.addEventListener("click", () => {
    if (!menu.hidden) return close();
    menu.hidden = false;
    button.setAttribute("aria-expanded", "true");
    document.addEventListener("pointerdown", outside, true);
    menu.querySelector(".menu-item:not(:disabled)")?.focus();
  });
  return h("div", { class: "menu-anchor" }, button, menu);
}

function serviceList(providers, selected) {
  const order = configuredProviders().map((provider) => provider.id);
  const list = h("div", { class: "svc-list-rows", role: "listbox", "aria-label": "服务商" });
  let dragged = null;
  const reorder = (id, target) => {
    const next = order.filter((item) => item !== id);
    next.splice(Math.max(0, Math.min(next.length, target)), 0, id);
    if (next.join() !== order.join()) runAction(null, "provider_reorder", { ids: next }, { ok: "顺序已更新" });
  };
  for (const provider of providers) {
    const health = providerHealth(provider);
    const configured = provider.kind === "configured";
    const row = h(
      "div",
      {
        class: `svc-row ${provider.id === selected.id ? "is-selected" : ""}`.trim(),
        role: "option",
        tabindex: "0",
        "aria-selected": String(provider.id === selected.id),
        draggable: configured ? "true" : null,
        title: configured ? "拖动排序；Alt + ↑/↓ 也可调整顺序" : null,
      },
      providerIcon(provider),
      h(
        "div",
        { class: "svc-row-main" },
        h("div", { class: "svc-row-name", text: providerName(provider) }),
        h("div", { class: "svc-row-sub" }, h("span", { class: `dot ${health.tone}`, "aria-hidden": "true" }), `${health.text} · ${(provider.selected_models || []).length}/${(provider.cached_models || []).length} 个模型`),
      ),
      configured ? h("span", { class: "svc-grip", "aria-hidden": "true" }) : null,
    );
    const choose = () => {
      svc.selected = provider.id;
      render({ animate: false });
      $(".svc-row.is-selected")?.focus();
    };
    row.addEventListener("click", choose);
    row.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        choose();
      } else if (configured && event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
        event.preventDefault();
        reorder(provider.id, order.indexOf(provider.id) + (event.key === "ArrowUp" ? -1 : 1));
      }
    });
    if (configured) {
      row.addEventListener("dragstart", (event) => {
        dragged = provider.id;
        row.classList.add("is-dragging");
        event.dataTransfer.effectAllowed = "move";
      });
      row.addEventListener("dragend", () => row.classList.remove("is-dragging"));
      row.addEventListener("dragover", (event) => {
        if (!dragged) return;
        event.preventDefault();
        row.classList.add("is-drop-target");
      });
      row.addEventListener("dragleave", () => row.classList.remove("is-drop-target"));
      row.addEventListener("drop", (event) => {
        event.preventDefault();
        row.classList.remove("is-drop-target");
        if (dragged && dragged !== provider.id) reorder(dragged, order.indexOf(provider.id));
        dragged = null;
      });
    }
    list.append(row);
  }
  const remove = h("button", { class: "btn btn-ghost btn-icon btn-sm", type: "button", "aria-label": "删除服务商", text: "\u2212", disabled: selected.kind !== "configured" || state.busy, "data-action": "" });
  remove.addEventListener("click", () => runAction(remove, "provider_remove", { id: selected.id }, { ok: "服务商已删除", confirm: [`删除 ${providerName(selected)}？`, "这会移除该服务商的配置与模型选择，操作无法撤销。", "删除"] }));
  return h(
    "aside",
    { class: "svc-list" },
    h("header", { class: "svc-list-head" }, h("span", { text: "服务商" }), h("div", { class: "card-actions" }, h("button", { class: "btn btn-ghost btn-icon btn-sm", type: "button", "aria-label": "新增服务商", text: "+", onclick: openAddProvider }), remove)),
    list,
  );
}
function serviceHeader(provider) {
  const official = provider.kind === "official";
  const health = providerHealth(provider);
  const badge = official ? h("span", { class: "badge badge-ok", text: "官方" }) : provider.enabled ? h("span", { class: `badge ${health.tone === "is-bad" ? "badge-bad" : "badge-ok"}`, text: health.tone === "is-bad" ? "需要处理" : "配置就绪" }) : h("span", { class: "badge", text: "停用" });
  const refreshed = provider.models_refreshed_at_ms ? `上次刷新 ${new Date(provider.models_refreshed_at_ms).toLocaleString("zh-CN", { hour12: false })}` : "尚未刷新";
  const sub = [official ? "官方服务商 · 只读" : provider.id, `已加入 ${(provider.selected_models || []).length} / 可选 ${(provider.cached_models || []).length}`, refreshed].join(" · ");
  const tabs = segmented([["models", "模型"], ["connection", "连接设置"]], svc.tab, (value) => {
    svc.tab = value;
    render({ animate: false });
  }, "详情页面");
  const items = official
    ? [{ label: "刷新模型列表", run: () => runAction(null, "refresh_codex_catalog", null, { ok: "官方模型目录已刷新" }) }]
    : [
        { label: "检查模型接口", run: () => testProvider(provider) },
        { label: "刷新模型列表", run: () => runAction(null, "provider_discover", { id: provider.id }, { ok: "模型列表已刷新" }) },
        "-",
        { label: provider.enabled ? "停用服务商" : "启用服务商", run: () => runAction(null, provider.enabled ? "provider_disable" : "provider_enable", { id: provider.id }, { ok: provider.enabled ? "已停用" : "已启用" }) },
      ];
  return h(
    "header",
    { class: "svc-head" },
    providerIcon(provider, 36),
    h("div", { class: "svc-head-main", title: sub }, h("div", { class: "svc-head-title" }, h("span", { text: providerName(provider) }), badge)),
    tabs,
    h("div", { class: "svc-head-actions" }, popoverMenu("更多服务商操作", items)),
  );
}

async function testProvider(provider) {
  const result = await runAction(null, "provider_test", { id: provider.id }, { ok: "模型接口检查完成" });
  if (result?.json) openSheet(`${providerName(provider)} · 接口检查`, h("div", { class: "form" }, valueView(result.json)));
}

function benchmarkFor(provider) {
  const results = state.snapshot.benchmark?.results || [];
  const map = new Map();
  for (const result of results) if (result.provider_id === provider.id) map.set(result.upstream_model || result.model, result);
  return map;
}

const CAPABILITIES = [
  ["supports_image", "图片输入", "M3 5h14v10H3z M3 12l4-4 4 4 2-2 4 4 M13 8.5a1 1 0 1 0 0.01 0"],
  ["supports_thinking", "Thinking", "M10 3a5 5 0 0 0-3 9v3h6v-3a5 5 0 0 0-3-9z M8 17h4"],
  ["supports_function_tools", "Function Tools", "M12.5 4.5a3 3 0 0 0-4 4L4 13l3 3 4.5-4.5a3 3 0 0 0 4-4l-2 2-2-1-1-2z"],
  ["supports_tool_search", "Tool Search", "M9 4a5 5 0 1 0 0 10A5 5 0 0 0 9 4z M13 13l4 4"],
  ["supports_web_search", "Web Search", "M10 3a7 7 0 1 0 0 14A7 7 0 0 0 10 3z M3 10h14 M10 3c-2.5 2.5-2.5 11.5 0 14 M10 3c2.5 2.5 2.5 11.5 0 14"],
];

function capabilityIcons(model) {
  const wrap = h("span", { class: "caps" });
  for (const [key, title, path] of CAPABILITIES) {
    const supported = model[key];
    const label = `${title}：${supported === true ? "支持" : supported === false ? "不支持" : "未知"}${model.capability_probe_error ? `。能力探测：${model.capability_probe_error}` : ""}`;
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 20 20");
    svg.setAttribute("class", `cap ${supported === true ? "is-on" : supported === false ? "is-off" : "is-unknown"}`);
    svg.setAttribute("role", "img");
    svg.setAttribute("aria-label", label);
    const titleNode = document.createElementNS("http://www.w3.org/2000/svg", "title");
    titleNode.textContent = label;
    const shape = document.createElementNS("http://www.w3.org/2000/svg", "path");
    shape.setAttribute("d", path);
    svg.append(titleNode, shape);
    wrap.append(svg);
  }
  return wrap;
}

function modelRows(provider) {
  const draft = draftFor(provider);
  const saved = new Set(provider.selected_models || []);
  const fresh = new Set(provider.new_models || []);
  const unavailable = new Set(provider.unavailable_selected_models || []);
  const bench = benchmarkFor(provider);
  const seen = new Set();
  const rows = [];
  const push = (model, flags) => {
    if (seen.has(model.id)) return;
    seen.add(model.id);
    const override = provider.model_context_overrides?.[model.id];
    const draftContext = draft.contexts[model.id];
    rows.push({
      model,
      selected: draft.models.has(model.id),
      savedSelected: saved.has(model.id),
      isNew: fresh.has(model.id),
      unavailable: flags.unavailable,
      manual: flags.manual,
      context: draftContext === undefined ? override ?? model.context_window : draftContext ?? model.source_context_window ?? model.context_window,
      overridden: draftContext === undefined ? override !== undefined : draftContext !== null,
      bench: bench.get(model.id),
    });
  };
  for (const model of provider.cached_models || []) push(typeof model === "string" ? { id: model } : model, {});
  for (const id of unavailable) push({ id, description: "该已选模型不在当前模型列表中" }, { unavailable: true });
  for (const id of draft.manual) push({ id, manually_added: true, description: "用户手动指定；能力将由后台自动补齐" }, { manual: true });
  return rows;
}

function rowMatches(row) {
  const query = svc.query.trim().toLowerCase();
  if (query && !`${row.model.id} ${row.model.display_name || ""}`.toLowerCase().includes(query)) return false;
  if (svc.filter === "selected") return row.selected;
  if (svc.filter === "new") return row.isNew;
  if (svc.filter === "attention") return row.unavailable || Boolean(row.model.capability_probe_error);
  return true;
}

const SORTERS = {
  selected: (row) => (row.selected ? 0 : 1),
  model: (row) => row.model.id,
  ttft: (row) => row.bench?.ttft_ms ?? Infinity,
  tps: (row) => -(row.bench?.tps ?? -Infinity),
  context: (row) => -(row.context ?? 0),
  ratio: (row) => Number.parseFloat(String(row.model.ratio || "").replace(/x/i, "")) || Infinity,
};

function sortRows(rows) {
  const key = SORTERS[svc.sort.key] || SORTERS.selected;
  const direction = svc.sort.reversed ? -1 : 1;
  return rows.sort((a, b) => {
    const left = key(a);
    const right = key(b);
    if (left === right) return a.model.id.localeCompare(b.model.id);
    return (left < right ? -1 : 1) * direction;
  });
}
function contextCell(provider, row) {
  const draft = draftFor(provider);
  const input = h("input", {
    class: "ctx-input mono",
    type: "text",
    inputmode: "numeric",
    value: row.context ? String(Math.round(row.context / CONTEXT_UNIT)) : "",
    placeholder: "—",
    "aria-label": `${row.model.id} 上下文（K）`,
    title: "模型上下文，单位 K；修改后保存",
  });
  input.addEventListener("change", () => {
    const value = Number(input.value.trim());
    if (!input.value.trim()) draft.contexts[row.model.id] = null;
    else if (Number.isFinite(value) && value > 0) draft.contexts[row.model.id] = Math.round(value * CONTEXT_UNIT);
    else {
      toast("bad", "上下文无效", "请输入以 K 为单位的正数。");
      return;
    }
    render({ animate: false });
  });
  const revert = row.overridden
    ? h("button", {
        class: "ctx-revert",
        type: "button",
        title: "还原为自动发现的上下文；保存后生效",
        "aria-label": `还原 ${row.model.id} 的上下文`,
        text: "\u21ba",
        onclick: () => {
          draft.contexts[row.model.id] = null;
          render({ animate: false });
        },
      })
    : null;
  return h("span", { class: "ctx" }, input, h("span", { class: "ctx-unit", text: "K" }), revert);
}

function modelsTab(provider) {
  const draft = draftFor(provider);
  const showRatio = provider.preset_id === "baidu-oneapi";
  const all = modelRows(provider);
  const rows = sortRows(all.filter(rowMatches));
  const changes = modelChanges(provider);
  const appliedCount = (provider.selected_models || []).length;

  const search = textInput("svc-query", svc.query, { type: "search", placeholder: "搜索当前服务商的模型", "aria-label": "搜索当前服务商的模型" });
  search.addEventListener("input", () => {
    svc.query = search.value;
    const body = $(".svc-table tbody");
    if (!body) return;
    const keep = new Set(all.filter(rowMatches).map((row) => row.model.id));
    for (const tr of body.children) tr.hidden = !keep.has(tr.dataset.model);
  });
  const filterLabels = { all: "全部", selected: "已加入", new: "新增", attention: "需处理" };
  const filter = popoverMenu("筛选", [
    ["all", "全部模型"],
    ["selected", "已加入 Codex"],
    ["new", "新增"],
    ["attention", "需要处理"],
  ].map(([value, label]) => ({ label, checked: svc.filter === value, run: () => { svc.filter = value; render({ animate: false }); } })), "btn btn-ghost btn-sm");
  filter.querySelector("button").textContent = `${filterLabels[svc.filter]} \u25be`;
  const visibleIds = rows.map((row) => row.model.id);
  const more = popoverMenu("更多模型操作", [
    { heading: "测速内容" },
    { label: "仅测试延迟（TTFT）", checked: svc.benchMode === 1, run: () => { svc.benchMode = 1; render({ animate: false }); } },
    { label: "测试延迟与吞吐", checked: svc.benchMode === 100, run: () => { svc.benchMode = 100; render({ animate: false }); } },
    { heading: "请求超时" },
    ...[15, 30, 60, 120].map((seconds) => ({ label: `${seconds} 秒`, checked: svc.benchTimeout === seconds, run: () => { svc.benchTimeout = seconds; render({ animate: false }); } })),
    "-",
    ...(provider.kind === "official" ? [] : [{ label: "重新探测已加入模型的能力", disabled: state.busy || changes > 0 || appliedCount === 0, run: () => runAction(null, "provider_probe", { id: provider.id }, { ok: "能力探测完成，请查看能力列" }) }, "-"]),
    { label: "全选当前结果", run: () => { visibleIds.forEach((id) => draft.models.add(id)); render({ animate: false }); } },
    { label: "取消选择当前结果", run: () => { visibleIds.forEach((id) => draft.models.delete(id)); render({ animate: false }); } },
    { label: "手动添加模型…", run: () => { svc.manualOpen = true; render({ animate: false }); $("#svc-manual")?.focus(); } },
  ], "btn btn-ghost btn-sm");
  more.querySelector("button").textContent = "\u22ef \u25be";

  const bench = actionButton(`测试已加入的 ${appliedCount} 个模型`, "benchmark_start", () => ({ timeout_seconds: svc.benchTimeout, target_output_tokens: svc.benchMode, providers: [provider.id] }), { ok: "测速已开始，结果会实时更新" }, "btn-sm");
  bench.disabled = state.busy || changes > 0 || appliedCount === 0 || !gatewayRunning() || provider.kind === "official";
  bench.title = changes > 0 ? "先应用更改，再测试。测速不会自动保存选择。" : !gatewayRunning() ? "网关未运行" : "";
  const save = h("button", { class: "btn btn-primary btn-sm", type: "button", "data-action": "", disabled: state.busy || changes === 0, text: changes ? `保存模型选择（${changes}）` : "保存模型选择" });
  save.addEventListener("click", () => saveModels(provider, save));

  const manual = svc.manualOpen
    ? (() => {
        const input = h("input", { class: "input mono", id: "svc-manual", placeholder: "手动输入模型 ID（可不在 /v1/models 中）", spellcheck: "false", autocomplete: "off" });
        const add = () => {
          const id = input.value.trim();
          if (!id || id.startsWith("-")) return;
          draft.manual.add(id);
          draft.models.add(id);
          render({ animate: false });
        };
        input.addEventListener("keydown", (event) => event.key === "Enter" && add());
        return h("div", { class: "svc-manual" }, input, h("button", { class: "btn btn-sm", type: "button", text: "添加并选中", onclick: add }), h("span", { class: "card-note", text: "能力将由后台自动补齐" }), h("button", { class: "btn btn-ghost btn-icon btn-sm", type: "button", "aria-label": "收起手动添加", text: "\u00d7", onclick: () => { svc.manualOpen = false; render({ animate: false }); } }));
      })()
    : null;

  const columns = [["selected", "是否加入"], ["model", "模型"], ["ttft", "首 Token"], ["tps", "生成速度"], ["context", "上下文"], ...(showRatio ? [["ratio", "倍率"]] : []), ["caps", "能力"]];
  const head = h("tr", null, ...columns.map(([key, label]) => {
    if (key === "caps") return h("th", { scope: "col", text: label });
    const active = svc.sort.key === key;
    return h("th", { scope: "col", class: key === "model" ? null : "r", "aria-sort": active ? (svc.sort.reversed ? "descending" : "ascending") : "none" },
      h("button", { class: `th-sort ${active ? "is-active" : ""}`.trim(), type: "button", text: `${label}${active ? (svc.sort.reversed ? " \u2191" : " \u2193") : ""}`, onclick: () => { svc.sort = { key, reversed: active ? !svc.sort.reversed : false }; render({ animate: false }); } }));
  }));
  const body = h("tbody", null, ...rows.map((row) => {
    const check = h("input", { class: "check", type: "checkbox", checked: row.selected, "aria-label": `将 ${row.model.id} 加入 Codex` });
    check.addEventListener("change", () => {
      if (check.checked) draft.models.add(row.model.id);
      else draft.models.delete(row.model.id);
      render({ animate: false });
    });
    const tags = [];
    if (row.isNew) tags.push(h("span", { class: "badge badge-ok", text: "新增" }));
    if (row.unavailable) tags.push(h("span", { class: "badge badge-bad", text: "不可用" }));
    if (row.selected !== row.savedSelected) tags.push(h("span", { class: "badge", text: row.selected ? "待加入" : "待移除" }));
    return h("tr", { "data-model": row.model.id, class: row.selected ? "is-selected" : null, title: row.model.description || null },
      h("td", { class: "c-check" }, check),
      h("td", { class: "c-model" }, h("span", { class: "mono", text: row.model.id }), ...tags),
      h("td", { class: "r mono", text: fixed(row.bench?.ttft_ms, 0, " ms").replace("\u2014", "-") }),
      h("td", { class: "r mono", text: fixed(row.bench?.tps, 1, " t/s").replace("\u2014", "-") }),
      h("td", { class: "r" }, contextCell(provider, row)),
      showRatio ? h("td", { class: "r mono", title: row.model.price_type || null, text: row.model.ratio || "-" }) : null,
      h("td", null, capabilityIcons(row.model)),
    );
  }));
  return [
    h("div", { class: "svc-toolbar" }, h("div", { class: "toolbar-search" }, search), filter, more, h("div", { class: "card-actions svc-toolbar-end" }, bench, save)),
    manual,
    rows.length
      ? h("div", { class: "table-wrap svc-table-wrap" }, h("table", { class: "table svc-table" }, h("thead", null, head), body))
      : emptyState(all.length ? "没有匹配的模型" : "当前服务商没有模型", all.length ? "调整搜索或筛选条件。" : "从更多操作中刷新模型列表，或手动添加模型。"),
  ];
}

async function saveModels(provider, button) {
  const draft = draftFor(provider);
  const params = { id: provider.id, models: [...draft.models], model_contexts: {}, clear_model_contexts: [] };
  for (const [model, tokens] of Object.entries(draft.contexts)) {
    if (tokens === null) {
      if (provider.model_context_overrides?.[model] !== undefined) params.clear_model_contexts.push(model);
    } else params.model_contexts[model] = tokens;
  }
  if (!params.models.length) {
    toast("bad", "至少选择一个模型", "客户端需要至少一个可路由模型。");
    return;
  }
  const result = await runAction(button, "provider_select", params, { ok: "模型选择已保存，Codex 模型目录已更新" });
  if (!result) return;
  delete svc.drafts[provider.id];
  render({ animate: false });
}
// macOS-style grouped form row: label on the left, control on the right.
function formRow(label, control, hint) {
  return h("div", { class: "frow" }, h("div", { class: "frow-label" }, h("span", { text: label }), hint ? h("span", { class: "frow-hint", text: hint }) : null), h("div", { class: "frow-control" }, control));
}

function formGroup(title, ...rows) {
  return h("section", { class: "fgroup" }, title ? h("h3", { class: "fgroup-title", text: title }) : null, h("div", { class: "fgroup-body" }, ...rows));
}

function inlineInput(name, value, props = {}) {
  return h("input", { class: "input input-inline", name, value: value || "", spellcheck: "false", autocomplete: "off", ...props });
}

function inlineSecret(name, configured) {
  return h("input", { class: "input input-inline mono", name, type: "password", autocomplete: "new-password", spellcheck: "false", placeholder: configured ? "已配置；留空保留" : "未配置" });
}

function clearButton(label, provider, options, okText) {
  return actionButton(label, "provider_update", { id: provider.id, options }, { ok: okText, confirm: [`${label}？`, "保存的凭据会被删除，之后需要重新填写。", label] }, "btn-sm");
}

function connectionTab(provider) {
  if (provider.kind === "official") {
    return h("div", { class: "notice notice-quiet" }, h("span", { class: "dot notice-mark", "aria-hidden": "true" }), h("div", { text: "此服务商由 Codex 官方 OAuth 登录管理，不能在这里修改连接参数。" }));
  }
  const preset = provider.preset_id || "custom";
  const definition = providerDefinition(preset);
  const aws = definition.auth_mode === "aws_sigv4";
  const form = h("form", { class: "svc-form", autocomplete: "off" });
  const connection = [formRow("服务商 ID", h("span", { class: "mono muted", text: provider.id }))];
  if (definition.custom_endpoint) {
    connection.push(
      formRow("站点名称", inlineInput("display_name", provider.display_name)),
      formRow("API 地址", inlineInput("base_url", provider.base_url, { placeholder: "https://api.example.com/v1", inputmode: "url" }), "填写到版本前缀，例如 /v1"),
      formRow("API 端点", selectControl("protocol", providerProtocols().map(([id, label]) => [id, label]), provider.protocol || providerCatalog().default_protocol)),
      formRow("官网地址", inlineInput("website_url", provider.website_url, { inputmode: "url" })),
    );
  }
  if (aws) {
    connection.push(
      formRow("AWS Region", inlineInput("aws_region", provider.aws_region, { placeholder: providerCatalog().default_aws_region })),
      formRow("Access Key ID", inlineSecret("aws_access_key_id", provider.aws_sigv4_configured)),
      formRow("Secret Access Key", inlineSecret("aws_secret_access_key", provider.aws_sigv4_configured)),
      formRow("Session Token", h("div", { class: "card-actions" }, inlineSecret("aws_session_token", provider.aws_session_token_configured), provider.aws_session_token_configured ? clearButton("清除 Session Token", provider, { clear_aws_session_token: true }, "Session Token 已清除") : null)),
    );
    if (provider.aws_sigv4_configured) connection.push(formRow("已保存的 AWS 凭据", clearButton("清除 AWS 凭据", provider, { clear_aws_credentials: true }, "AWS 凭据已清除")));
  } else {
    connection.push(formRow("API 密钥", h("div", { class: "card-actions" }, inlineSecret("key", provider.api_key_configured), provider.api_key_configured ? clearButton("清除密钥", provider, { clear_key: true }, "密钥已清除") : null)));
  }
  const groups = [formGroup("连接配置", ...connection)];
  if (definition.quota_username_required) groups.push(formGroup("百度额度", formRow("额度用户名", inlineInput("quota_username", provider.quota_username, { placeholder: "Baidu OneAPI 额度接口必填" }))));
  if (definition.quota_credentials_required) {
    groups.push(formGroup("OpenCode Go",
      formRow("工作区 ID", inlineInput("quota_workspace_id", provider.quota_workspace_id, { placeholder: "例如：wrk_abc123" })),
      formRow("Auth Cookie", h("div", { class: "card-actions" }, inlineSecret("quota_auth_cookie", provider.quota_auth_cookie_configured), provider.quota_auth_cookie_configured ? clearButton("清除额度凭据", provider, { clear_quota_workspace_id: true, clear_quota_auth_cookie: true }, "额度凭据已清除") : null)),
    ));
  }
  const modelOptions = (provider.selected_models || []).map((id) => [id, id]);
  const reviewEnabled = h("input", { class: "switch", type: "checkbox", role: "switch", name: "review_override", checked: Boolean(provider.auto_review_model), disabled: !modelOptions.length, "aria-label": "Codex 审查模型覆盖" });
  const review = selectControl("auto_review_model", modelOptions, provider.auto_review_model || modelOptions[0]?.[0]);
  const titleEnabled = h("input", { class: "switch", type: "checkbox", role: "switch", name: "title_override", checked: Boolean(provider.codex_thread_title_model), disabled: !modelOptions.length, "aria-label": "Codex 会话命名模型覆盖" });
  const title = selectControl("codex_thread_title_model", modelOptions, provider.codex_thread_title_model || modelOptions[0]?.[0]);
  const syncOverrides = () => {
    review.hidden = !reviewEnabled.checked;
    title.hidden = !titleEnabled.checked;
  };
  reviewEnabled.addEventListener("change", syncOverrides);
  titleEnabled.addEventListener("change", syncOverrides);
  syncOverrides();
  const advanced = [];
  if (definition.quota_username_required) {
    advanced.push(
      formRow("认证桥接", selectControl("baidu_auth_bridge", [["disabled", "关闭（默认）"], ["ducx_loopback", "DUCX 核心（loopback）"]], provider.baidu_auth_bridge || "disabled")),
      formRow("上报 AI 代码使用数据", h("input", { class: "switch", type: "checkbox", role: "switch", name: "baidu_code_report", checked: Boolean(provider.baidu_code_report), "aria-label": "上报 AI 代码使用数据" })),
    );
  }
  advanced.push(
    formRow("绘图接口路径", inlineInput("image_generation_path", provider.image_generation_path, { placeholder: "/v1/images/generations" })),
    formRow("Codex 审查模型覆盖", h("div", { class: "form" }, reviewEnabled, review), "关闭时使用默认审查路由"),
    formRow("Codex 会话命名模型覆盖", h("div", { class: "form" }, titleEnabled, title), "仅覆盖会话命名请求，普通对话保留原模型"),
  );
  groups.push(h("details", { class: "fgroup fgroup-details", open: true }, h("summary", { class: "fgroup-title", text: "高级选项" }), h("div", { class: "fgroup-body" }, ...advanced)));

  const apply = submitButton("应用更改");
  const reset = h("button", { class: "btn btn-ghost", type: "button", text: "放弃更改", onclick: () => render({ animate: false }) });
  append(form, [groups, h("div", { class: "form-actions svc-form-actions" }, reset, apply)]);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const options = connectionChanges(provider, formValues(form));
    if (options === null) return;
    if (!Object.keys(options).length) {
      toast("ok", "没有需要应用的更改");
      return;
    }
    runAction(event.submitter, "provider_update", { id: provider.id, options }, { ok: "连接设置已应用" });
  });
  return form;
}

// Only changed fields are sent; secrets are sent when typed.
function connectionChanges(provider, values) {
  const options = {};
  const text = (name, original) => {
    if (values[name] === undefined || values[name] === (original || "")) return;
    options[name] = values[name];
  };
  if ((provider.preset_id || "custom") === "custom") {
    if (!values.display_name || !values.base_url) {
      toast("bad", "缺少站点信息", "站点名称和 API 地址不能为空。");
      return null;
    }
    text("display_name", provider.display_name);
    text("base_url", provider.base_url);
    text("website_url", provider.website_url);
    if (values.protocol && values.protocol !== provider.protocol) {
      options.protocol = values.protocol;
      options.api_path = providerProtocols().find(([id]) => id === values.protocol)?.[2];
    }
  }
  for (const name of ["key", "aws_access_key_id", "aws_secret_access_key", "aws_session_token", "quota_auth_cookie"]) if (values[name]) options[name] = values[name];
  text("aws_region", provider.aws_region);
  if (provider.preset_id === "baidu-oneapi") {
    if (!values.quota_username) {
      toast("bad", "缺少额度用户名", "Baidu OneAPI 查询额度必须填写用户名。");
      return null;
    }
    text("quota_username", provider.quota_username);
    if (values.baidu_auth_bridge !== (provider.baidu_auth_bridge || "disabled")) options.baidu_auth_bridge = values.baidu_auth_bridge;
    if (values.baidu_code_report !== Boolean(provider.baidu_code_report)) options.baidu_code_report = values.baidu_code_report;
  }
  text("quota_workspace_id", provider.quota_workspace_id);
  if (values.image_generation_path !== (provider.image_generation_path || "")) {
    if (values.image_generation_path) options.image_generation_path = values.image_generation_path;
    else options.clear_image_generation = true;
  }
  const reviewModel = values.review_override ? values.auto_review_model : "";
  if ((reviewModel || "") !== (provider.auto_review_model || "")) {
    if (reviewModel) options.auto_review_model = reviewModel;
    else options.clear_auto_review_model = true;
  }
  const titleModel = values.title_override ? values.codex_thread_title_model : "";
  if ((titleModel || "") !== (provider.codex_thread_title_model || "")) {
    if (titleModel) options.codex_thread_title_model = titleModel;
    else options.clear_codex_thread_title_model = true;
  }
  return options;
}

function openAddProvider() {
  const catalog = providerCatalog();
  const preset = selectControl("preset", catalog.presets.map((entry) => [entry.id, entry.display_name]), catalog.default_preset);
  const rows = {
    id: formRow("服务商 ID", inlineInput("id", "", { placeholder: "可选，默认按预设生成" })),
    key: formRow("API 密钥", inlineSecret("key", false)),
    display: formRow("站点名称", inlineInput("display_name", "")),
    base: formRow("API 地址", inlineInput("base_url", "", { placeholder: "https://api.example.com/v1", inputmode: "url" })),
    protocol: formRow("API 端点", selectControl("protocol", providerProtocols().map(([id, label]) => [id, label]), catalog.default_protocol)),
    website: formRow("官网地址", inlineInput("website_url", "", { inputmode: "url" })),
    quota: formRow("额度用户名", inlineInput("quota_username", "", { placeholder: "Baidu OneAPI 额度接口必填" })),
    workspace: formRow("工作区 ID", inlineInput("quota_workspace_id", "", { placeholder: "例如：wrk_abc123" })),
    cookie: formRow("Auth Cookie", inlineSecret("quota_auth_cookie", false)),
    region: formRow("AWS Region", inlineInput("aws_region", catalog.default_aws_region)),
    awsId: formRow("Access Key ID", inlineSecret("aws_access_key_id", false)),
    awsSecret: formRow("Secret Access Key", inlineSecret("aws_secret_access_key", false)),
    awsToken: formRow("Session Token", inlineSecret("aws_session_token", false)),
    timeout: formRow("OAuth 授权超时（秒）", inlineInput("oauth_timeout_seconds", catalog.oauth_timeout_seconds ?? "", { inputmode: "numeric" })),
  };
  let pending = false;
  const hint = h("p", { class: "muted", hidden: true, text: "点击新增后将在浏览器授权。关闭网页不会回传取消，可在此取消等待；超过时限会自动结束。" });
  const sync = () => {
    const definition = providerDefinition(preset.querySelector("select").value);
    const show = new Set(definition.allow_multiple === false ? [] : ["id"]);
    if (definition.auth_mode === "aws_sigv4") {
      for (const name of ["region", "awsId", "awsSecret", "awsToken"]) show.add(name);
    } else if (definition.auth_mode === "local_oauth") show.add("timeout");
    else show.add("key");
    if (definition.custom_endpoint) for (const name of ["display", "base", "protocol", "website"]) show.add(name);
    if (definition.quota_username_required) show.add("quota");
    if (definition.quota_credentials_required) for (const name of ["workspace", "cookie"]) show.add(name);
    const required = new Set();
    if (definition.api_key_required) required.add("key");
    if (definition.custom_endpoint) for (const name of ["display", "base"]) required.add(name);
    if (definition.auth_mode === "aws_sigv4") for (const name of ["awsId", "awsSecret", "region"]) required.add(name);
    if (definition.quota_username_required) required.add("quota");
    if (definition.quota_credentials_required) for (const name of ["workspace", "cookie"]) required.add(name);
    for (const [name, row] of Object.entries(rows)) {
      row.hidden = !show.has(name);
      row.querySelector("input, select").required = required.has(name);
    }
    hint.hidden = definition.auth_mode !== "local_oauth";
  };
  preset.querySelector("select").addEventListener("change", sync);
  sync();
  const form = h("form", { class: "svc-form", autocomplete: "off" }, formGroup(null, formRow("预设", preset), hint, ...Object.values(rows)), h("div", { class: "form-actions" }, h("button", { class: "btn btn-ghost", type: "button", text: "取消", onclick: closeSheet }), submitButton("新增服务商")));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const values = formValues(form);
    const options = {};
    for (const [name, value] of Object.entries(values)) {
      if (name === "preset" || !value || form.elements[name].closest(".frow")?.hidden) continue;
      options[name] = value;
    }
    if (options.protocol) options.api_path = providerProtocols().find(([id]) => id === options.protocol)?.[2];
    pending = true;
    const result = await runAction(event.submitter, "provider_add", { preset: values.preset, options }, { ok: "服务商已添加，模型列表已刷新" });
    pending = false;
    if (!result) return;
    closeSheet();
    svc.selected = options.id || providerDefinition(values.preset).default_id || values.preset;
  });
  openSheet("新增服务商", form);
  $("#sheet").addEventListener("close", () => {
    if (pending) api("/api/actions/cancel", { method: "POST" }).catch(error => toast("bad", "取消失败", error.message));
  }, { once: true });
}

RENDERERS.providers = () => {
  const providers = state.snapshot.providers || [];
  if (!providers.length) return card(null, null, emptyState("还没有服务商", "通过新增服务商选择 OAuth 登录或兼容 API 预设。", h("button", { class: "btn btn-primary", type: "button", text: "新增服务商", onclick: openAddProvider })));
  const selected = providers.find((provider) => provider.id === svc.selected) || providers[0];
  svc.selected = selected.id;
  const issue = (selected.readiness_issues || []).map((id) => ISSUE_LABELS[id] || id);
  return h(
    "div",
    { class: "svc card card-flush" },
    serviceList(providers, selected),
    h(
      "section",
      { class: "svc-detail", "aria-label": providerName(selected) },
      serviceHeader(selected),
      issue.length || selected.last_model_refresh_error
        ? h("div", { class: "svc-issues" }, ...issue.map((text) => h("span", { class: "badge badge-bad", text })), selected.last_model_refresh_error ? h("span", { class: "issue issue-clamp", title: selected.last_model_refresh_error, text: selected.last_model_refresh_error }) : null)
        : null,
      h("div", { class: "svc-body" }, svc.tab === "connection" ? connectionTab(selected) : modelsTab(selected)),
    ),
  );
};

// ---------- Speed ----------

RENDERERS.speed = () => {
  if (!gatewayRunning()) return card(null, null, emptyState("网关未运行", "测速通过运行中的网关发起请求。", actionButton("启动网关", "service_start", null, { ok: "网关已启动" }, "btn-primary")));
  const snapshot = state.snapshot.benchmark;
  const providers = configuredProviders().filter((provider) => provider.enabled);
  const form = h(
    "form",
    { class: "form" },
    field("Provider", h("div", { class: "chips" }, ...providers.map((provider) => h("label", { class: "chip" }, h("input", { type: "checkbox", name: "providers", value: provider.id, "data-list": "1" }), providerName(provider)))), "不选则测试全部已启用 Provider"),
    h("div", { class: "form-row" }, field("单模型超时（秒）", textInput("timeout_seconds", "60", { type: "number", min: "1", max: "3600", inputmode: "numeric" }))),
    h("div", { class: "form-actions" }, submitButton(snapshot?.status === "running" ? "测速进行中" : "开始测速")),
  );
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const values = formValues(form);
    const timeout = Number(values.timeout_seconds);
    if (!Number.isInteger(timeout) || timeout < 1 || timeout > 3600) {
      toast("bad", "超时无效", "请输入 1 到 3600 之间的整数秒数。");
      return;
    }
    runAction(event.submitter, "benchmark_start", { timeout_seconds: timeout, providers: values.providers || [] }, { ok: "测速已开始" });
  });
  return [card("发起测速", null, form), benchmarkResults(snapshot)];
};

function benchmarkResults(snapshot) {
  if (!snapshot) return card("结果", null, emptyState("尚无测速记录", "开始一次测速后，结果会在这里实时更新。"));
  const results = [...(snapshot.results || [])].sort((a, b) => (b.tps || 0) - (a.tps || 0));
  const best = Math.max(1, ...results.map((result) => result.tps || 0));
  const done = results.length;
  const total = snapshot.total_models || done || 1;
  const progress = h("div", { class: "progress", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": String(total), "aria-valuenow": String(done) }, h("div", { class: "progress-bar", style: { transform: `scaleX(${Math.min(1, done / total)})` } }));
  const note = snapshot.status === "running" ? `进行中 · ${done}/${total}${snapshot.current_model ? ` · ${snapshot.current_model}` : ""}` : `${str(snapshot.status)} · ${done}/${total}${snapshot.estimated_cost ? ` · 预估花费 ${snapshot.estimated_cost.toFixed(4)} ${snapshot.cost_currency || ""}` : ""}`;
  const table = h(
    "div",
    { class: "table-wrap" },
    h(
      "table",
      { class: "table" },
      h("thead", null, h("tr", null, ...["模型", "首字延迟", "输出速度", "", "状态"].map((text, index) => h("th", { class: index && index < 3 ? "r" : null, scope: "col", text })))),
      h(
        "tbody",
        null,
        ...results.map((result, index) =>
          h(
            "tr",
            null,
            h("td", null, h("div", { text: result.model }), h("div", { class: "row-sub", text: result.provider_name || result.provider_id })),
            h("td", { class: "r", text: fixed(result.ttft_ms, 0, " ms") }),
            h("td", { class: "r", text: fixed(result.tps, 1, " t/s") }),
            h("td", { style: { width: "28%" } }, result.tps ? h("div", { class: `bar ${index === 0 ? "is-best" : ""}`, style: { width: `${Math.max(2, (result.tps / best) * 100)}%` } }) : null),
            h("td", null, result.error ? h("span", { class: "badge badge-bad", title: result.error, text: "失败" }) : h("span", { class: "badge badge-ok", text: "完成" })),
          ),
        ),
      ),
    ),
  );
  return card("结果", note, h("div", null, h("div", { style: { padding: "0 20px 14px" } }, progress), results.length ? table : null, snapshot.error ? h("p", { class: "issue", style: { padding: "0 20px 16px" }, text: snapshot.error }) : null), null, "card-flush");
}

// ---------- Fusion ----------

RENDERERS.fusion = () => {
  const models = state.snapshot.models || [];
  const profile = state.snapshot.fusion_profile;
  if (models.length < 2) return card(null, null, emptyState("可选模型不足", "Fusion 至少需要两个可用模型，请先在「模型」页选择。"));
  const options = models.map((model) => [model.id, model.display_name ? `${model.display_name}` : model.id]);
  const current = profile || { id: "fusion", mode: "orchestration", panel_models: [], judge_model: options[0][0], final_model: options[0][0], min_successful: 2, max_completion_tokens: 4096, timeout_ms: 120000, show_intermediate_results: true };
  const panel = new Set(current.panel_models || []);
  const form = h(
    "form",
    { class: "form", autocomplete: "off" },
    h("div", { class: "form-row" }, field("Profile ID", textInput("id", current.id)), field("模式", selectControl("mode", [["orchestration", "协同编排"], ["time_rotation", "分时路由"]], current.mode || "orchestration"))),
    field("参与模型", h("div", { class: "chips" }, ...options.map(([id, label]) => h("label", { class: "chip", title: id }, h("input", { type: "checkbox", name: "panel_models", value: id, "data-list": "1", checked: panel.has(id) }), label))), "编排模式下并行作答的模型"),
    h("div", { class: "form-row" }, field("评审模型", selectControl("judge_model", options, current.judge_model)), field("终稿模型", selectControl("final_model", options, current.final_model))),
    h("div", { class: "form-row" }, field("最少成功数", textInput("min_successful", String(current.min_successful ?? 2), { type: "number", min: "1" })), field("超时（毫秒）", textInput("timeout_ms", String(current.timeout_ms ?? 120000), { type: "number", min: "1000", step: "1000" }))),
    h("div", { class: "form-row" }, field("最大输出 Token", textInput("max_completion_tokens", String(current.max_completion_tokens ?? 4096), { type: "number", min: "1" })), field("中间结果", h("label", { class: "chip" }, h("input", { type: "checkbox", name: "show_intermediate_results", checked: current.show_intermediate_results !== false }), "在回答中展示各模型结果"))),
    h(
      "div",
      { class: "form-actions" },
      profile ? actionButton("删除 Profile", "fusion_delete", { id: profile.id }, { ok: "Fusion Profile 已删除", confirm: ["删除 Fusion Profile？", "客户端将不再看到对应的 Fusion 模型。", "删除"] }, "btn-danger") : null,
      submitButton("保存"),
    ),
  );
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const values = formValues(form);
    // Keep fields this form does not edit (time routes, panel tools).
    const next = {
      ...current,
      id: values.id,
      mode: values.mode,
      panel_models: values.panel_models || [],
      judge_model: values.judge_model,
      final_model: values.final_model,
      min_successful: Number(values.min_successful),
      timeout_ms: Number(values.timeout_ms),
      max_completion_tokens: Number(values.max_completion_tokens),
      show_intermediate_results: values.show_intermediate_results,
    };
    const params = { profile: next };
    if (profile) params.replace_id = profile.id;
    runAction(event.submitter, "fusion_set", params, { ok: "Fusion Profile 已保存" });
  });
  const routes = (current.time_routes || []).map((route) => h("div", { class: "row" }, h("span", { class: "mono", text: `${minutes(route.start_minute)} – ${minutes(route.end_minute)}` }), h("span", { class: "row-main", text: route.model })));
  return [
    card(profile ? `fusion/${profile.id}` : "新建 Fusion Profile", null, form),
    routes.length ? card("分时路由", "在 CLI 中编辑；保存本页时保留", h("div", { class: "rows" }, routes), null, "card-flush") : null,
  ];
};

function minutes(value) {
  const hours = String(Math.floor(value / 60)).padStart(2, "0");
  return `${hours}:${String(value % 60).padStart(2, "0")}`;
}

// ---------- Agents ----------

const agentView = { query: "", loading: false, versions: {} };

async function loadAgents() {
  if (agentView.loading) return;
  agentView.loading = true;
  try {
    state.agents = await api("/api/agents");
  } catch (error) {
    if (error.status !== 401) state.agentsError = error;
  } finally {
    agentView.loading = false;
  }
  if (state.page === "apps" && !userIsEditing()) render({ animate: false });
}

async function checkAgentVersions(button) {
  button.classList.add("is-busy");
  try {
    const result = await api("/api/agents/versions");
    for (const entry of result.agents || []) if (entry && entry.id) agentView.versions[entry.id] = entry.cli || entry;
    toast("ok", "版本检查完成");
  } catch (error) {
    if (error.status !== 401) toast("bad", "版本检查失败", error.message);
  } finally {
    button.classList.remove("is-busy");
  }
  render({ animate: false });
}

function agentStatus(agent) {
  const integration = agent.integration || {};
  const mode = integration.mode === "custom_only" ? "仅自定义模式" : integration.mode ? "官方账号模式" : null;
  switch (integration.state) {
    case "managed":
      return { tone: "is-ok", text: ["已接入", agent.id === "codex" ? mode : null].filter(Boolean).join(" · "), on: true };
    case "unmanaged":
      return { tone: "", text: "未接入", on: false };
    case "unsupported":
      return { tone: "", text: "暂不支持", on: false, locked: true, hint: integration.detail };
    default:
      return { tone: "is-bad", text: `状态异常：${integration.error || "未知错误"}`, on: false };
  }
}

function agentRow(agent, templates) {
  const status = agentStatus(agent);
  const cli = { ...(agent.cli || {}), ...(agentView.versions[agent.id] || {}) };
  const versions = [cli.version ? (cli.app_version ? `CLI ${cli.version}` : cli.version) : null, cli.app_version ? `App ${cli.app_version}` : null].filter(Boolean).join("  ");
  const icon = h("img", { class: `agent-icon ${templates.has(agent.id) ? "is-template" : ""}`.trim(), src: `/agent-icons/${encodeURIComponent(agent.id)}`, alt: "", width: "32", height: "32", loading: "lazy" });
  icon.addEventListener("error", () => icon.replaceWith(h("span", { class: "agent-icon agent-icon-fallback", "aria-hidden": "true", text: agent.name.slice(0, 1) })), { once: true });
  const toggle = h("input", { class: "switch", type: "checkbox", role: "switch", checked: status.on, disabled: status.locked || state.busy, "aria-label": `${status.on ? "断开" : "接入"} ${agent.name}`, "data-action": "", title: status.hint || null });
  toggle.addEventListener("change", async () => {
    const connecting = toggle.checked;
    toggle.checked = !connecting; // reflect real state until the CLI confirms
    const params = { id: agent.id };
    if (connecting && agent.id === "codex") params.mode = "official_account";
    if (connecting) await runAction(null, "agent_connect", params, { ok: `${agent.name} 已接入` });
    else await runAction(null, "agent_disconnect", params, { ok: `${agent.name} 已断开`, confirm: [`断开 ${agent.name}？`, agent.integration?.detail || "将恢复该 Agent 的原始配置。", "断开"] });
  });
  const detailParts = [status.text];
  if (cli.error && agent.detected) detailParts.push(`版本检查失败：${cli.error}`);
  return h(
    "div",
    { class: "agent-row", role: "listitem", "data-search": `${agent.id} ${agent.name}`.toLowerCase() },
    icon,
    h(
      "div",
      { class: "agent-main" },
      h("div", { class: "agent-title" }, h("span", { class: "agent-name", text: agent.name }), versions ? h("span", { class: "agent-version mono", text: versions }) : null),
      status.locked ? null : h("div", { class: "agent-status", title: detailParts.join(" · ") }, h("span", { class: `dot ${status.tone}`, "aria-hidden": "true" }), h("span", { class: "agent-status-text", text: status.text })),
    ),
    agent.id === "codex" && status.on
      ? segmented([["official_account", "官方账号"], ["custom_only", "仅自定义"]], agent.integration.mode === "custom_only" ? "custom_only" : "official_account", async (mode) => {
          await runAction(null, "agent_connect", { id: "codex", mode }, { ok: "Codex 接入模式已切换" });
        }, "Codex 接入模式")
      : null,
    cli.update && cli.latest
      ? actionButton(`更新到 ${cli.latest}`, "agent_upgrade", { id: agent.id }, { ok: `${agent.name} 已更新` }, "btn-sm")
      : null,
    toggle,
  );
}

RENDERERS.apps = () => {
  if (!state.agents && !state.agentsError) {
    loadAgents();
    return [h("div", { class: "skeleton" })];
  }
  if (!state.agents) return [errorCard(state.agentsError)];
  const agents = state.agents.agents || [];
  const templates = new Set(state.agents.template_icons || []);
  const installed = agents.filter((agent) => agent.detected);
  const missing = agents.filter((agent) => !agent.detected);
  const connected = agents.filter((agent) => agent.integration?.state === "managed").length;

  const search = textInput("agent-query", agentView.query, { type: "search", placeholder: `搜索 ${agents.length} 个 Agent`, "aria-label": "搜索 Agent" });
  const lists = [];
  const applyFilter = () => {
    agentView.query = search.value;
    const query = search.value.trim().toLowerCase();
    for (const list of lists) {
      let visible = 0;
      for (const row of list.querySelectorAll(".agent-row")) {
        row.hidden = Boolean(query) && !row.dataset.search.includes(query);
        if (!row.hidden) visible += 1;
      }
      list.closest(".agent-group").hidden = visible === 0;
    }
  };
  search.addEventListener("input", applyFilter);
  const check = h("button", { class: "btn btn-ghost", type: "button", text: "检查更新" });
  check.addEventListener("click", () => checkAgentVersions(check));
  const reload = h("button", { class: "btn btn-ghost btn-icon", type: "button", "aria-label": "重新检测", onclick: loadAgents }, h("span", { class: "icon icon-refresh", "aria-hidden": "true" }));

  const group = (title, items, collapsed) => {
    const list = h("div", { class: "agent-list", role: "list" }, ...items.map((agent) => agentRow(agent, templates)));
    lists.push(list);
    if (!collapsed) return h("section", { class: "agent-group card card-flush", "aria-label": title }, h("header", { class: "agent-group-head", text: `${title} · ${items.length}` }), list);
    return h("details", { class: "agent-group card card-flush", open: agentView.query ? true : null }, h("summary", { class: "agent-group-head", text: `${title} · ${items.length}` }), list);
  };
  requestAnimationFrame(applyFilter);
  return [
    h("div", { class: "toolbar" }, h("div", { class: "toolbar-search" }, search), h("span", { class: "card-note", text: `${connected} 个已接入 · ${installed.length} 个已安装` }), h("div", { class: "card-actions" }, check, reload)),
    installed.length ? group("已安装", installed, false) : null,
    missing.length ? group("未检测到", missing, true) : null,
  ];
};

// ---------- System ----------

RENDERERS.system = () => {
  const panel = state.snapshot.panel || {};
  const status = state.snapshot.status || {};
  const maintenance = [
    ["检查更新", "下载并安装最新版本的 Codex Mixin", actionButton("更新", "update", null, { ok: "更新完成" })],
    ["快速修复", "运行诊断并自动修复可修复的问题", actionButton("修复", "doctor_fix", null, { ok: "修复完成" })],
    ["刷新 Codex 模型目录", "重新生成 Codex 可见的模型列表", actionButton("刷新", "refresh_codex_catalog", null, { ok: "模型目录已刷新" })],
    ["停止并恢复客户端", "停止网关，并把已接入的客户端恢复为原始配置", actionButton("停止并恢复", "service_stop_restore_clients", null, { ok: "已停止并恢复", confirm: ["停止网关并恢复客户端？", "所有客户端将不再经过 Codex Mixin，需要时可重新接入。", "停止并恢复"] }, "btn-danger")],
  ];
  return [
    exposureNotice(),
    card("维护", null, h("div", { class: "rows", style: { margin: "0 -22px -20px" } }, ...maintenance.map(([title, text, button]) => h("div", { class: "row" }, h("div", { class: "row-main" }, h("div", { class: "row-title", text: title }), h("div", { class: "row-sub", text })), button)))),
    card(
      "环境",
      null,
      h(
        "dl",
        { class: "kv" },
        ...[
          ["面板版本", panel.version],
          ["网关版本", status.gateway_version],
          ["网关监听", status.bind || state.snapshot.gateway_bind],
          ["网关访问密钥", state.snapshot.gateway_auth_configured ? "已设置" : "未设置"],
          ["入站鉴权", panel.client_auth_build ? "client-auth 构建，按密钥校验" : "默认构建，不校验"],
          ["已安装命令", panel.installed_cli_path],
          ["日志文件", status.log],
        ].flatMap(([key, value]) => [h("dt", { text: key }), h("dd", { class: "mono", text: str(value) })]),
      ),
    ),
  ];
};

// ---------- Advanced ----------

const advanced = { ech: null, echTest: null, report: null };

async function loadEch() {
  try {
    advanced.ech = await api("/api/ech");
  } catch (error) {
    if (error.status !== 401) advanced.ech = { error: error.message };
  }
  if (state.page === "advanced") render({ animate: false });
}

const DUCX_EVENTS = { "user-prompt-submit": "用户请求", "pre-tool-use": "代码生成", "post-tool-use": "代码采纳", stop: "Session 文件" };

function replayReportView(report) {
  const line = (item, label) => h("div", { class: "row" }, h("span", { class: `badge ${label === "OK" ? "badge-ok" : "badge-bad"}`, text: label }), h("div", { class: "row-main" }, h("div", { class: "row-title", text: `${DUCX_EVENTS[item.event] || item.event} · ${item.provider_id}` }), h("div", { class: "row-sub mono", text: item.error ? `${item.session_id} · ${item.error}` : item.session_id })));
  return h(
    "div",
    { class: "form" },
    h("dl", { class: "kv" }, ...[["本地 Session 新加入队列", report.queued_from_local_sessions], ["上传成功", report.delivered?.length], ["上传失败，等待最后一次重试", report.retained?.length], ["超过重试次数并丢弃", report.discarded?.length]].flatMap(([key, value]) => [h("dt", { text: key }), h("dd", { class: "mono", text: str(value, "0") })])),
    h("div", { class: "rows card card-flush" }, ...(report.delivered || []).map((item) => line(item, "OK")), ...(report.retained || []).map((item) => line(item, "ERROR")), ...(report.discarded || []).map((item) => line(item, "DROP"))),
  );
}

async function manualReport(button) {
  button.classList.add("is-busy");
  let estimate;
  try {
    estimate = await api("/api/report/estimate");
  } catch (error) {
    if (error.status !== 401) toast("bad", "手动上报预估失败", error.message);
    return;
  } finally {
    button.classList.remove("is-busy");
  }
  const result = await runAction(button, "report_replay", null, {
    ok: "本地 Session 上报已处理",
    confirm: ["手动上报全部本地 Session？", `预计上报 ${estimate.estimated_sessions} 个会话，共 ${estimate.estimated_reports} 次。此功能仅用于自动上报数据异常时的修复，确认异常确实需要修复后再继续。`, "开始上报"],
  });
  if (result?.json) openSheet("DUCX 手动上报结果", replayReportView(result.json));
}

async function exportBackup(button) {
  const ok = await confirmAction("导出配置备份？", "备份使用 Base64 编码但未加密，包含 API Key、AWS 凭据和本地访问密钥，请妥善保管，勿公开分享。", "导出");
  if (!ok) return;
  button.classList.add("is-busy");
  try {
    const response = await fetch("/api/config/export", { headers: { Authorization: `Bearer ${state.token}` }, cache: "no-store" });
    if (!response.ok) throw new ApiError(response.status, await response.json().catch(() => null));
    const url = URL.createObjectURL(await response.blob());
    const link = h("a", { href: url, download: `codex-mixin-${new Date().toISOString().slice(0, 10)}.backup` });
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    toast("ok", "配置备份已导出", "Base64 不是加密，请勿公开分享。");
  } catch (error) {
    const [title, detail] = describeError(error);
    toast("bad", title === "操作失败" ? "导出配置备份失败" : title, detail);
  } finally {
    button.classList.remove("is-busy");
  }
}

function importBackup(button) {
  const picker = h("input", { type: "file", accept: ".backup,.txt,application/octet-stream,text/plain", hidden: true });
  picker.addEventListener("change", async () => {
    const file = picker.files?.[0];
    picker.remove();
    if (!file) return;
    const ok = await confirmAction("导入配置备份？", `将用「${file.name}」替换当前 Provider、模型选择、Fusion 和凭据配置，然后重启本地网关。`, "导入并重启");
    if (!ok) return;
    button.classList.add("is-busy");
    try {
      const response = await fetch("/api/config/import", { method: "POST", headers: { Authorization: `Bearer ${state.token}`, "Content-Type": "application/octet-stream" }, body: file });
      if (!response.ok) throw new ApiError(response.status, await response.json().catch(() => null));
      toast("ok", "配置备份已导入", "配置已安全写入并应用，本地网关已重启。");
      svc.drafts = {};
    } catch (error) {
      const [title, detail] = describeError(error);
      toast("bad", title === "操作失败" ? "导入配置备份失败" : title, detail);
    } finally {
      button.classList.remove("is-busy");
      refresh(true);
    }
  });
  document.body.append(picker);
  picker.click();
}

RENDERERS.advanced = () => {
  if (!advanced.ech) loadEch();
  const ech = advanced.ech || {};
  const echOn = Boolean(ech.enabled);
  const echToggle = h("input", { class: "switch", type: "checkbox", role: "switch", checked: echOn, disabled: state.busy || !advanced.ech || ech.error, "aria-label": "ECH 代理访问 GPT", "data-action": "" });
  echToggle.addEventListener("change", async () => {
    const enabling = echToggle.checked;
    echToggle.checked = !enabling;
    await runAction(null, enabling ? "ech_enable" : "ech_disable", null, { ok: enabling ? "ECH 已启用，网关已重启" : "ECH 已关闭，已恢复普通网络设置" });
    loadEch();
  });
  const echTest = h("button", { class: "btn btn-sm", type: "button", "data-action": "", text: "仅测试连接" });
  echTest.addEventListener("click", async () => {
    const result = await runAction(echTest, "ech_test", null, { ok: "ECH 连接测试完成" });
    if (result) {
      advanced.echTest = result.json || result.stdout;
      render({ animate: false });
    }
  });
  const echStatus = ech.error ? `无法读取：${ech.error}` : !advanced.ech ? "读取中…" : echOn ? "已启用" : ech.fallback_reason ? `已自动关闭，恢复普通网络设置：${ech.fallback_reason}` : "未启用";
  const echCard = card(
    "ECH 代理访问 GPT",
    "仅接管本地网关的官方 GPT 请求，保留端到端 TLS；连接失败会自动关闭并恢复普通网络设置（环境变量代理、系统静态代理，否则直连）。",
    h(
      "div",
      { class: "svc-form" },
      formGroup(null,
        formRow("启用 ECH", echToggle, "启用前会先测试连接，再保存设置并重启网关"),
        formRow("当前状态", h("span", { class: ech.fallback_reason && !echOn ? "issue" : "muted", text: echStatus })),
        formRow("DoH", h("span", { class: "mono muted", text: str(ech.doh_url) })),
        formRow("接管的域名", h("span", { class: "mono muted", text: (ech.hosts || []).join("、") || "—" })),
        formRow("连接测试", echTest, "不发送凭据或模型请求"),
      ),
      advanced.echTest ? h("div", { class: "output-wrap" }, typeof advanced.echTest === "string" ? h("pre", { class: "output", text: advanced.echTest }) : valueView(advanced.echTest)) : null,
      h("p", { class: "card-note", text: "ECH 成功不能证明中继出口位于 Azure；不影响浏览器登录或第三方 Provider。" }),
    ),
  );
  const report = h("button", { class: "btn", type: "button", "data-action": "", text: "手动触发上报…" });
  report.addEventListener("click", () => manualReport(report));
  const exportButton = h("button", { class: "btn", type: "button", text: "导出配置备份…" });
  exportButton.addEventListener("click", () => exportBackup(exportButton));
  const importButton = h("button", { class: "btn", type: "button", "data-action": "", text: "导入配置备份…" });
  importButton.addEventListener("click", () => importBackup(importButton));
  return [
    echCard,
    card("手动上报", "清除旧上报凭据、执行 DUCX warmup，再扫描并重放全部本地 Session。仅在自动上报数据异常时使用。", h("div", { class: "card-actions" }, report)),
    card("配置备份", "备份包含 Provider、模型选择、Fusion 与凭据；使用 Base64 编码但未加密，导入会替换当前配置并重启网关。", h("div", { class: "card-actions" }, exportButton, importButton)),
  ];
};

// ---------- Logs ----------

let logTab = "service";

async function loadLogs() {
  try {
    const result = await api(`/api/logs?lines=${state.logLines}`);
    state.logs = result.text || "";
  } catch (error) {
    if (error.status === 401) return;
    state.logs = `无法读取日志：${error.message}`;
  }
  if (state.page === "logs") render({ animate: false });
}

async function loadDoctor(button) {
  button?.classList.add("is-busy");
  try {
    state.doctor = await api("/api/doctor");
  } catch (error) {
    if (error.status !== 401) toast("bad", "诊断失败", error.message);
  } finally {
    button?.classList.remove("is-busy");
  }
  if (state.page === "logs") render({ animate: false });
}

RENDERERS.logs = () => {
  const tabs = segmented([["service", "服务日志"], ["doctor", "诊断"]], logTab, (value) => {
    logTab = value;
    if (value === "doctor" && !state.doctor) loadDoctor();
    render({ animate: true });
  }, "日志类型");
  if (logTab === "doctor") return [h("div", null, tabs), doctorCard()];
  const lines = selectControl("lines", [["200", "最近 200 行"], ["500", "最近 500 行"], ["2000", "最近 2000 行"]], String(state.logLines));
  lines.querySelector("select").addEventListener("change", (event) => {
    state.logLines = Number(event.target.value);
    loadLogs();
  });
  const reload = h("button", { class: "btn btn-ghost", type: "button", text: "刷新", onclick: loadLogs });
  const pre = h("pre", { class: "log", tabindex: "0", "aria-label": "服务日志", text: state.logs ?? "读取中…" });
  requestAnimationFrame(() => (pre.scrollTop = pre.scrollHeight));
  return [h("div", null, tabs), card("服务日志", null, pre, [lines, reload], "card-flush")];
};

function doctorCard() {
  const rerun = h("button", { class: "btn btn-ghost", type: "button", text: "重新诊断" });
  rerun.addEventListener("click", () => loadDoctor(rerun));
  const checks = state.doctor?.checks || [];
  const body = !state.doctor
    ? h("div", { class: "skeleton", style: { margin: "16px 20px" } })
    : h(
        "div",
        { class: "rows" },
        ...checks.map((check) =>
          h(
            "div",
            { class: "row" },
            h("span", { class: `dot ${check.status === "ok" ? "is-ok" : check.status === "error" ? "is-bad" : "is-warn"}`, "aria-label": check.status }),
            h("div", { class: "row-main" }, h("div", { class: "row-title", text: check.name || check.id }), h("div", { class: "row-sub", text: [check.message, check.fix_hint].filter(Boolean).join(" · ") })),
          ),
        ),
      );
  return card("诊断报告", null, body, [rerun, actionButton("自动修复", "doctor_fix", null, { ok: "修复完成" }, "btn-primary")], "card-flush");
}

// ---------- Context ----------

async function loadContext() {
  state.contextLoading = true;
  try {
    state.context = await api("/api/context");
    state.contextError = null;
  } catch (error) {
    if (error.status === 401) return;
    state.contextError = error;
  } finally {
    state.contextLoading = false;
  }
  if (state.page === "context") render({ animate: false });
}

function contextSettingsCard(context) {
  const settings = context.settings || {};
  const toggle = h("input", { class: "switch", type: "checkbox", role: "switch", checked: Boolean(settings.enabled), disabled: state.busy, "aria-label": "启用上下文引擎", "data-action": "" });
  toggle.addEventListener("change", async () => {
    const enabled = toggle.checked;
    toggle.disabled = true;
    await runAction(null, "context_configure", { enabled }, { ok: enabled ? "已开启上下文引擎" : "已关闭上下文引擎" });
    await loadContext();
  });
  const rows = [
    ["增长阈值", settings.growth_threshold_tokens ? compact(settings.growth_threshold_tokens) + " tokens" : "\u2014"],
    ["压力 / 紧急阈值", fixed((settings.pressure_ratio ?? 0) * 100, 0, "%") + " / " + fixed((settings.urgent_ratio ?? 0) * 100, 0, "%")],
    ["保护最近消息", str(settings.protected_messages) + " 条 · " + compact(settings.protected_tokens ?? 0) + " tokens"],
    ["内部轮上限", str(settings.max_internal_rounds)],
    ["上下文窗口", settings.context_window_tokens ? compact(settings.context_window_tokens) + " tokens" : "跟随模型"],
  ];
  const body = h(
    "div",
    { class: "rows" },
    h(
      "div",
      { class: "row" },
      h(
        "div",
        { class: "row-main" },
        h("div", { class: "row-title", text: "启用上下文引擎", title: "保存更改后自动重启运行中的网关并生效" }),
      ),
      toggle,
    ),
    ...rows.map(([label, value]) =>
      h(
        "div",
        { class: "row" },
        h("div", { class: "row-main" }, h("div", { class: "row-title", text: label })),
        h("span", { class: "row-sub", text: value }),
      ),
    ),
  );
  return card("上下文引擎", null, body);
}

function contextMetrics(totals) {
  const cache = totals.cache || {};
  return h(
    "div",
    { class: "grid grid-4" },
    metricCard("会话", totals.sessions || 0),
    metricCard("压缩次数", totals.compressions || 0),
    metricCard("节省 token", totals.saved_tokens || 0),
    metricCard("缓存命中", fixed(cache.hit_percent, 1, "%")),
  );
}

function contextSessionsCard(sessions, totals) {
  if (!sessions.length) {
    return card("会话", null, emptyState("暂无会话", ""));
  }
  const maxSaved = Math.max(1, ...sessions.map((row) => row.saved_tokens || 0));
  const head = ["会话", "消息", "压缩次数", "节省 token", "视图 token", "缓存命中", "最近活动"].map((label, index) =>
    h("th", { class: index > 0 && index < 6 ? "r" : null, text: label }),
  );
  const body = sessions.map((row) => {
    const cache = row.cache || {};
    const saved = Math.min(1, (row.saved_tokens || 0) / maxSaved);
    const hit = Math.min(1, (cache.hit_percent || 0) / 100);
    return h(
      "tr",
      null,
      h("td", { title: `${row.session} · 修订 ${str(row.revision)}` }, h("div", { class: "row-title", text: row.display })),
      h("td", { class: "r", text: compact(row.messages || 0) }),
      h("td", { class: "r", text: compact(row.compressions || 0) }),
      h(
        "td",
        { class: "r" },
        h("div", { text: compact(row.saved_tokens || 0) }),
        h("div", { class: "progress", style: { marginTop: "4px" }, role: "progressbar", "aria-label": "节省 token 占比", "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(Math.round(saved * 100)) },
          h("div", { class: "progress-bar", style: { transform: "scaleX(" + fixed(saved, 3, "") + ")" } })),
      ),
      h("td", { class: "r", text: compact(row.view_tokens || 0) }),
      h(
        "td",
        { class: "r" },
        h("div", { text: fixed(cache.hit_percent, 1, "%") }),
        cache.hit_percent == null ? null : h("div", { class: "progress", style: { marginTop: "4px" }, role: "progressbar", "aria-label": "缓存命中率", "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(Math.round(hit * 100)) },
          h("div", { class: "progress-bar", style: { transform: "scaleX(" + fixed(hit, 3, "") + ")" } })),
      ),
      h("td", { class: "r", text: timeAgo((row.updated_at || 0) * 1000) }),
    );
  });
  const cache = totals.cache || {};
  const foot = h(
    "tfoot",
    null,
    h(
      "tr",
      null,
      h("td", { text: "合计" }),
      h("td", { class: "r", text: compact(totals.messages || 0) }),
      h("td", { class: "r", text: compact(totals.compressions || 0) }),
      h("td", { class: "r", text: compact(totals.saved_tokens || 0) }),
      h("td", { class: "r", text: compact(totals.view_tokens || 0) }),
      h("td", { class: "r", text: fixed(cache.hit_percent, 1, "%") }),
      h("td", { class: "r", text: compact(cache.turns || 0) + " 轮" }),
    ),
  );
  return card("会话", null, h("div", { class: "table-wrap" }, h("table", { class: "table" }, h("thead", null, h("tr", null, head)), h("tbody", null, body), foot)), null, "card-flush ctx-sessions");
}

RENDERERS.context = () => {
  const reload = h("button", { class: "btn btn-ghost", type: "button", text: "刷新", onclick: loadContext });
  if (!state.context) {
    const body = state.contextError
      ? h("div", { class: "notice", role: "alert" }, h("span", { class: "dot is-bad notice-mark", "aria-hidden": "true" }), h("div", null, h("strong", { text: "无法读取上下文统计" }), h("div", { class: "card-note", text: state.contextError.message })))
      : h("div", { class: "skeleton", style: { margin: "16px 20px" } });
    return [card("上下文引擎", null, body, [reload], "card-flush")];
  }
  const context = state.context;
  const sessions = context.sessions || [];
  const totals = context.totals || { cache: {}, sessions: sessions.length };
  return [contextSettingsCard(context), contextMetrics(totals), contextSessionsCard(sessions, totals)];
};

// ---------- Boot ----------

function boot() {
  installSprings();
  buildNav();
  const { page, token } = readHash();
  if (page && PAGES.some((candidate) => candidate.id === page)) state.page = page;
  // Drop the token from the address bar and history right away.
  history.replaceState(null, "", `#${state.page}`);
  $("#refresh").addEventListener("click", () => {
    refresh(true);
    if (state.page === "apps") loadAgents();
  });
  $("#sheet-close").addEventListener("click", closeSheet);
  $("#sheet").addEventListener("cancel", (event) => {
    event.preventDefault();
    closeSheet();
  });
  $("#gate-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const value = $("#gate-token").value.trim();
    if (value) enterPanel(value);
  });
  window.addEventListener("popstate", () => navigate(readHash().page || "home", { replace: true }));
  // A restarted panel prints a new token; opening that URL in an existing
  // tab only changes the hash, so pick the token up here too.
  window.addEventListener("hashchange", () => {
    const next = readHash();
    if (!next.token) return;
    if (next.page && PAGES.some((candidate) => candidate.id === next.page)) state.page = next.page;
    history.replaceState(null, "", `#${state.page}`);
    enterPanel(next.token);
  });
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible" && state.token) refresh(false);
    else clearTimeout(state.timer);
  });
  const saved = token || sessionStorage.getItem(TOKEN_KEY);
  if (saved) enterPanel(saved);
  else showGate();
}

boot();
