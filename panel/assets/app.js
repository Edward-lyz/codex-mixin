// Codex Mixin web control panel. Vanilla JS, no build step.
// Every business operation goes through /api/actions, which runs the CLI.
"use strict";

const TOKEN_KEY = "codex-mixin-panel-token";
const POLL_MS = 5000;
const TOAST_MS = 4200;
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

const PAGES = [
  { id: "home", title: "概览", glyph: "\u2302", subtitle: "网关状态、用量与额度" },
  { id: "providers", title: "Providers", glyph: "\u25c6", subtitle: "管理上游服务、顺序与连通性" },
  { id: "models", title: "模型", glyph: "\u2261", subtitle: "选择暴露给客户端的模型与上下文窗口" },
  { id: "speed", title: "测速", glyph: "\u21af", subtitle: "首字延迟与输出速度基准" },
  { id: "fusion", title: "Fusion", glyph: "\u2726", subtitle: "多模型协同与分时路由" },
  { id: "apps", title: "Agent", glyph: "\u25c7", subtitle: "把编码 Agent 接入本地网关，查看版本与接入状态" },
  { id: "system", title: "系统", glyph: "\u2699", subtitle: "更新、修复与维护" },
  { id: "logs", title: "日志", glyph: "\u2637", subtitle: "服务日志与诊断报告" },
];

const state = {
  token: "",
  page: "home",
  usage: "7d",
  snapshot: null,
  snapshotKey: "",
  loadError: null,
  quota: null,
  doctor: null,
  logs: null,
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
    if (error.status !== 401) {
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
          h("span", { class: "nav-glyph", "aria-hidden": "true", text: page.glyph }),
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
  $("#page-title").textContent = page.title;
  $("#page-subtitle").textContent = page.subtitle;
  document.title = `${page.title} · Codex Mixin`;
  placeIndicator();
  render({ animate: true });
  if (changed && options.focus !== false) $("#main").focus({ preventScroll: true });
  if (page.id === "logs" && !state.logs) loadLogs();
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
  return h("div", { class: "empty" }, h("p", { class: "empty-title", text: title }), h("p", { text }), action || null);
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
        metricCard("可用模型", (state.snapshot.models || []).length, "可被客户端与 Fusion 选择"),
        metricCard("Codex 接入", codexModeLabel(state.snapshot.codex_install_mode), "在「Agent」页切换"),
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
  return h("div", { class: "card card-tight metric" }, h("span", { class: "metric-label", text: label }), h("span", { class: "metric-value", text: typeof value === "number" ? compact(value) : str(value) }), h("span", { class: "metric-sub", text: sub }));
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
          ...rows.map((row) =>
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
  return card("用量", "按 Provider 与模型汇总", body, range, "card-flush");
}

function quotaCard() {
  const load = h("button", { class: "btn btn-ghost btn-sm", type: "button", text: state.quota ? "重新查询" : "查询额度" });
  load.addEventListener("click", async () => {
    load.classList.add("is-busy");
    try {
      state.quota = await api("/api/quota");
      render({ animate: false });
    } catch (error) {
      if (error.status !== 401) toast("bad", "额度查询失败", error.message);
    } finally {
      load.classList.remove("is-busy");
    }
  });
  const body = state.quota ? h("div", { class: "card-body" }, valueView(state.quota)) : h("p", { class: "card-note", text: "向支持额度接口的 Provider 查询剩余额度。" });
  return card("额度", null, body, load);
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

// ---------- Providers ----------

const PROVIDER_PRESETS = [
  ["custom", "自定义（OpenAI / Anthropic 兼容）"],
  ["deepseek", "DeepSeek"],
  ["openrouter", "OpenRouter"],
  ["baidu-oneapi", "Baidu OneAPI"],
  ["opencode-go", "OpenCode Go"],
  ["aws-bedrock", "AWS Bedrock"],
];

function configuredProviders() {
  return (state.snapshot.providers || []).filter((provider) => provider.kind === "configured");
}

function providerName(provider) {
  return provider.display_name || provider.id;
}

const ISSUE_LABELS = {
  credentials_missing: "缺少凭据",
  no_routable_models: "没有可路由模型",
  selected_models_unavailable: "所选模型不可用",
  model_refresh_failed: "模型刷新失败",
};

function readinessBadge(provider) {
  if (!provider.enabled) return h("span", { class: "badge", text: "已停用" });
  if (provider.readiness === "healthy") return h("span", { class: "badge badge-ok", text: "可用" });
  if (provider.readiness === "degraded") return h("span", { class: "badge badge-bad", text: "降级" });
  return h("span", { class: "badge", text: provider.readiness ? String(provider.readiness) : "待检查" });
}

RENDERERS.providers = () => {
  const providers = state.snapshot.providers || [];
  const add = h("button", { class: "btn btn-primary", type: "button", text: "添加 Provider", onclick: openAddProvider });
  if (!providers.length) return card(null, null, emptyState("还没有 Provider", "添加一个兼容 OpenAI 或 Anthropic 协议的上游服务。", add));
  const configuredIds = configuredProviders().map((provider) => provider.id);
  return [
    h("div", { class: "card-actions", style: { justifyContent: "flex-end" } }, add),
    stagger(h("div", { class: "grid grid-2" }, ...providers.map((provider) => providerCard(provider, configuredIds)))),
  ];
};

function providerCard(provider, order) {
  const official = provider.kind === "official";
  const index = order.indexOf(provider.id);
  const toggle = h("input", { class: "switch", type: "checkbox", role: "switch", checked: provider.enabled, disabled: official || state.busy, "aria-label": `启用 ${providerName(provider)}`, "data-action": "" });
  toggle.addEventListener("change", () => runAction(null, toggle.checked ? "provider_enable" : "provider_disable", { id: provider.id }, { ok: toggle.checked ? "已启用" : "已停用" }));
  const move = (delta) => {
    const next = [...order];
    next.splice(index, 1);
    next.splice(index + delta, 0, provider.id);
    return { ids: next };
  };
  const issues = provider.readiness_issues || [];
  return h(
    "article",
    { class: `card provider ${provider.enabled ? "" : "is-off"}`.trim(), "aria-label": providerName(provider) },
    h(
      "div",
      { class: "provider-top" },
      h("span", { class: "provider-avatar", "aria-hidden": "true", text: providerName(provider).slice(0, 1) }),
      h("div", { class: "row-main" }, h("div", { class: "provider-name", text: providerName(provider) }), h("div", { class: "provider-url mono", text: provider.base_url || "" })),
      toggle,
    ),
    h(
      "div",
      { class: "provider-stats" },
      readinessBadge(provider),
      h("span", { class: "badge", text: `${(provider.selected_models || []).length} 个模型已选` }),
      (provider.new_models || []).length ? h("span", { class: "badge badge-ok", text: `${provider.new_models.length} 个新模型` }) : null,
      provider.auxiliary_model_upstream ? h("span", { class: "badge", text: "辅助上游" }) : null,
      official ? h("span", { class: "badge", text: "由 Codex 管理" }) : null,
    ),
    issues.length ? h("div", { class: "chips" }, ...issues.map((issue) => h("span", { class: "badge badge-bad", text: ISSUE_LABELS[issue] || String(issue) }))) : null,
    provider.last_model_refresh_error ? h("p", { class: "issue issue-clamp", title: provider.last_model_refresh_error, text: provider.last_model_refresh_error }) : null,
    official
      ? null
      : h(
          "div",
          { class: "provider-foot" },
          h(
            "div",
            { class: "card-actions" },
            actionButton("测试", "provider_test", { id: provider.id }, { ok: "连通性测试完成" }, "btn-sm"),
            actionButton("刷新模型", "provider_discover", { id: provider.id }, { ok: "模型目录已刷新" }, "btn-sm btn-ghost"),
            actionButton("探测能力", "provider_probe", { id: provider.id }, { ok: "能力探测完成" }, "btn-sm btn-ghost"),
          ),
          h(
            "div",
            { class: "card-actions" },
            index > 0 ? actionButton("\u2191", "provider_reorder", () => move(-1), { ok: "顺序已更新" }, "btn-sm btn-ghost btn-icon") : null,
            index >= 0 && index < order.length - 1 ? actionButton("\u2193", "provider_reorder", () => move(1), { ok: "顺序已更新" }, "btn-sm btn-ghost btn-icon") : null,
            h("button", { class: "btn btn-sm btn-ghost", type: "button", text: "编辑", onclick: () => openEditProvider(provider) }),
            actionButton("删除", "provider_remove", { id: provider.id }, { ok: "已删除", confirm: [`删除 ${providerName(provider)}？`, "这会移除该 Provider 的配置与模型选择，操作无法撤销。", "删除"] }, "btn-sm btn-danger"),
          ),
        ),
  );
}

function providerFields(kind, provider) {
  const groups = {
    key: field(kind === "add" ? "API Key" : "新 API Key", secretInput("key", kind === "add" ? "" : "留空则保持不变")),
    display: field("显示名称", textInput("display_name", provider?.display_name || "")),
    base: field("Base URL", textInput("base_url", provider?.base_url || "", { placeholder: "https://api.example.com/v1", inputmode: "url" })),
    website: field("官网", textInput("website_url", provider?.website_url || "", { inputmode: "url" })),
    quota: field("额度用户名", textInput("quota_username", provider?.quota_username || "")),
    awsRegion: field("AWS Region", textInput("aws_region", provider?.aws_region || "", { placeholder: "us-east-1" })),
    awsId: field("Access Key ID", secretInput("aws_access_key_id", kind === "add" ? "" : "留空则保持不变")),
    awsSecret: field("Secret Access Key", secretInput("aws_secret_access_key", kind === "add" ? "" : "留空则保持不变")),
    awsToken: field("Session Token", secretInput("aws_session_token", "可选")),
    models: field("模型", textInput("model", "", { placeholder: "model-a, model-b" }), "仅自定义 Provider：没有模型列表接口时手动填写，逗号分隔"),
    id: field("ID", textInput("id", "", { placeholder: "可选，默认按预设生成" })),
  };
  return groups;
}

function openAddProvider() {
  const preset = selectControl("preset", PROVIDER_PRESETS, "custom");
  const fields = providerFields("add");
  const sync = () => {
    const value = preset.querySelector("select").value;
    const aws = value === "aws-bedrock";
    fields.key.hidden = aws;
    for (const name of ["awsRegion", "awsId", "awsSecret", "awsToken"]) fields[name].hidden = !aws;
    fields.base.hidden = value !== "custom";
    fields.models.hidden = value !== "custom";
    fields.quota.hidden = value !== "baidu-oneapi";
  };
  preset.querySelector("select").addEventListener("change", sync);
  sync();
  const form = h(
    "form",
    { class: "form", autocomplete: "off" },
    h("div", { class: "form-row" }, field("预设", preset), fields.id),
    fields.key,
    h("div", { class: "form-row" }, fields.awsId, fields.awsSecret),
    h("div", { class: "form-row" }, fields.awsRegion, fields.awsToken),
    h("div", { class: "form-row" }, fields.display, fields.website),
    fields.base,
    fields.models,
    fields.quota,
    h("div", { class: "form-actions" }, h("button", { class: "btn btn-ghost", type: "button", text: "取消", onclick: closeSheet }), submitButton("添加")),
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const values = formValues(form);
    const options = {};
    for (const [name, value] of Object.entries(values)) {
      if (name === "preset" || !value) continue;
      const container = form.elements[name].closest(".field");
      if (container && container.hidden) continue;
      options[name] = name === "model" ? value.split(",").map((item) => item.trim()).filter(Boolean) : value;
    }
    const result = await runAction(event.submitter, "provider_add", { preset: values.preset, options }, { ok: "Provider 已添加" });
    if (result) closeSheet();
  });
  openSheet("添加 Provider", form);
}

function openEditProvider(provider) {
  const fields = providerFields("edit", provider);
  const aws = provider.preset_id === "aws-bedrock" || Boolean(provider.aws_region);
  const clearKey = h("label", { class: "chip" }, h("input", { type: "checkbox", name: "clear_key" }), "清除已保存的 Key");
  const auxiliary = h("label", { class: "chip" }, h("input", { type: "checkbox", name: "auxiliary_model_upstream", checked: Boolean(provider.auxiliary_model_upstream) }), "作为辅助模型上游");
  const review = field("自动审查模型", textInput("auto_review_model", provider.auto_review_model || ""), "辅助上游时用于 Codex auto review，留空表示清除");
  const form = h(
    "form",
    { class: "form", autocomplete: "off" },
    h("div", { class: "form-row" }, fields.display, fields.website),
    fields.base,
    aws ? [h("div", { class: "form-row" }, fields.awsId, fields.awsSecret), h("div", { class: "form-row" }, fields.awsRegion, fields.awsToken)] : [fields.key, h("div", { class: "chips" }, clearKey)],
    provider.quota_username || provider.preset_id === "baidu-oneapi" ? fields.quota : null,
    h("div", { class: "chips" }, auxiliary),
    review,
    h("div", { class: "form-actions" }, h("button", { class: "btn btn-ghost", type: "button", text: "取消", onclick: closeSheet }), submitButton("保存")),
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const values = formValues(form);
    const options = {};
    const changed = (name, original) => values[name] !== undefined && values[name] !== (original || "");
    for (const name of ["display_name", "base_url", "website_url", "quota_username", "aws_region"]) {
      if (changed(name, provider[name]) && values[name]) options[name] = values[name];
    }
    for (const name of ["key", "aws_access_key_id", "aws_secret_access_key", "aws_session_token"]) {
      if (values[name]) options[name] = values[name];
    }
    if (values.clear_key && !values.key) options.clear_key = true;
    if (values.auxiliary_model_upstream !== Boolean(provider.auxiliary_model_upstream)) options.auxiliary_model_upstream = values.auxiliary_model_upstream;
    if (changed("auto_review_model", provider.auto_review_model)) {
      if (values.auto_review_model) options.auto_review_model = values.auto_review_model;
      else options.clear_auto_review_model = true;
    }
    if (!Object.keys(options).length) {
      closeSheet();
      return;
    }
    const result = await runAction(event.submitter, "provider_update", { id: provider.id, options }, { ok: "已保存" });
    if (result) closeSheet();
  });
  openSheet(`编辑 ${providerName(provider)}`, form);
}

// ---------- Models ----------

const modelFilter = { provider: "", query: "" };

RENDERERS.models = () => {
  const providers = configuredProviders();
  if (!providers.length) return card(null, null, emptyState("没有可配置的 Provider", "先添加 Provider，再选择要暴露的模型。"));
  if (!providers.some((provider) => provider.id === modelFilter.provider)) modelFilter.provider = providers[0].id;
  const provider = providers.find((candidate) => candidate.id === modelFilter.provider);
  const picker = selectControl("provider", providers.map((item) => [item.id, providerName(item)]), provider.id);
  picker.querySelector("select").addEventListener("change", (event) => {
    modelFilter.provider = event.target.value;
    render({ animate: true });
  });
  const search = textInput("query", modelFilter.query, { type: "search", placeholder: "筛选模型", "aria-label": "筛选模型" });
  const selected = new Set(provider.selected_models || []);
  const fresh = new Set(provider.new_models || []);
  const overrides = provider.model_context_overrides || {};
  const models = (provider.cached_models || []).map((model) => (typeof model === "string" ? { id: model } : model));
  for (const id of selected) if (!models.some((model) => model.id === id)) models.push({ id });
  models.sort((a, b) => Number(selected.has(b.id)) - Number(selected.has(a.id)) || a.id.localeCompare(b.id));

  const rows = h("div", { class: "rows", role: "list" });
  const applyFilter = () => {
    const query = search.value.trim().toLowerCase();
    modelFilter.query = search.value;
    for (const row of rows.children) row.hidden = Boolean(query) && !row.dataset.search.includes(query);
  };
  search.addEventListener("input", applyFilter);
  for (const model of models) {
    const override = overrides[model.id];
    rows.append(
      h(
        "div",
        { class: "row", role: "listitem", "data-search": `${model.id} ${model.display_name || ""}`.toLowerCase() },
        h("input", { class: "check", type: "checkbox", name: "models", value: model.id, "data-list": "1", checked: selected.has(model.id), "aria-label": `选择 ${model.id}` }),
        h("div", { class: "row-main" }, h("div", { class: "row-title mono", text: model.id }), h("div", { class: "row-sub", text: [model.display_name, model.context_window ? `上下文 ${compact(model.context_window)}` : null].filter(Boolean).join(" · ") || " " })),
        fresh.has(model.id) ? h("span", { class: "badge badge-ok", text: "新" }) : null,
        h("input", { class: "input input-small mono", type: "number", min: "1", step: "1", inputmode: "numeric", name: `context:${model.id}`, value: override ? String(override) : "", placeholder: "上下文覆盖", "aria-label": `${model.id} 上下文窗口覆盖` }),
      ),
    );
  }
  const form = h("form", { class: "card card-flush" });
  const head = h(
    "header",
    { class: "card-head" },
    h("div", { class: "card-actions" }, picker, search),
    h("div", { class: "card-actions" }, actionButton("刷新目录", "provider_discover", { id: provider.id }, { ok: "模型目录已刷新" }, "btn-ghost"), submitButton("保存选择")),
  );
  append(form, [head, models.length ? rows : emptyState("模型目录为空", "点击「刷新目录」从上游拉取模型列表。")]);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const values = formValues(form);
    const params = { id: provider.id, models: values.models || [], model_contexts: {}, clear_model_contexts: [] };
    for (const model of models) {
      const raw = values[`context:${model.id}`] || "";
      const previous = overrides[model.id];
      if (raw && Number(raw) !== previous) params.model_contexts[model.id] = Number(raw);
      if (!raw && previous) params.clear_model_contexts.push(model.id);
    }
    if (!params.models.length) {
      toast("bad", "至少选择一个模型", "客户端需要至少一个可路由模型。");
      return;
    }
    runAction(event.submitter, "provider_select", params, { ok: "模型选择已保存" });
  });
  requestAnimationFrame(applyFilter);
  return [form];
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
  return [card("发起测速", "按目标输出长度测量首字延迟与输出速度", form), benchmarkResults(snapshot)];
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
    card(profile ? `fusion/${profile.id}` : "新建 Fusion Profile", "客户端会看到一个以 fusion/ 开头的模型", form),
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
      h("div", { class: "agent-status", title: detailParts.join(" · ") }, h("span", { class: `dot ${status.tone}`, "aria-hidden": "true" }), h("span", { class: "agent-status-text", text: detailParts.join(" · ") })),
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
  return card("诊断报告", "doctor --quick", body, [rerun, actionButton("自动修复", "doctor_fix", null, { ok: "修复完成" }, "btn-primary")], "card-flush");
}

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
