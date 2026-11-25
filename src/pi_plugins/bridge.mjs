import { randomUUID, timingSafeEqual } from 'node:crypto';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { join } from 'node:path';
import {
  DefaultResourceLoader, ModelRuntime, SessionManager, SettingsManager, VERSION,
  createAgentSessionFromServices,
} from '@earendil-works/pi-coding-agent';

const agentDir = process.env.PI_CODING_AGENT_DIR;
const token = process.env.MIXIN_PI_BRIDGE_TOKEN;
if (!agentDir || !token) throw new Error('Missing Pi bridge environment');
delete process.env.MIXIN_PI_BRIDGE_TOKEN;

const cwd = process.cwd();
const modelRuntime = await ModelRuntime.create({
  authPath: join(agentDir, 'auth.json'),
  modelsPath: join(agentDir, 'models.json'),
  refreshOnCreate: false,
});
const settingsManager = SettingsManager.create(cwd, agentDir);
const resourceLoader = new DefaultResourceLoader({
  cwd, agentDir, settingsManager,
  disabledBuiltinExtensions: ['mcp', 'codemode', 'tool-search'],
  noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true,
});
await resourceLoader.reload({ resolveProjectTrust: async () => false });
const diagnostics = [];
const loadedExtensions = resourceLoader.getExtensions();
const pathToSource = new Map();
for (const ext of loadedExtensions.extensions) {
  const source = ext.sourceInfo?.source ?? null;
  if (ext.path) pathToSource.set(ext.path, source);
  if (ext.resolvedPath) pathToSource.set(ext.resolvedPath, source);
}
const providerMeta = new Map();
for (const { name, config, extensionPath } of loadedExtensions.runtime.pendingProviderRegistrations ?? []) {
  providerMeta.set(name, {
    display_name: typeof config?.name === 'string' && config.name.trim() ? config.name.trim() : name,
    package: pathToSource.get(extensionPath) ?? null,
  });
  try {
    modelRuntime.registerProvider(name, config);
  } catch (error) {
    diagnostics.push({ type: 'error', message: `Extension "${extensionPath}" error: ${error instanceof Error ? error.message : String(error)}` });
  }
}
loadedExtensions.runtime.pendingProviderRegistrations = [];
for (const { provider, extensionPath } of loadedExtensions.runtime.pendingNativeProviderRegistrations ?? []) {
  providerMeta.set(provider.id, {
    display_name: typeof provider.name === 'string' && provider.name.trim() ? provider.name.trim() : provider.id,
    package: pathToSource.get(extensionPath) ?? null,
  });
  try {
    modelRuntime.registerNativeProvider(provider);
  } catch (error) {
    diagnostics.push({ type: 'error', message: `Extension "${extensionPath}" error: ${error instanceof Error ? error.message : String(error)}` });
  }
}
loadedExtensions.runtime.pendingNativeProviderRegistrations = [];
for (const { definition, extensionPath } of loadedExtensions.runtime.pendingVirtualModelRegistrations ?? []) {
  try { modelRuntime.registerVirtualModel(definition); }
  catch (error) { diagnostics.push({ type: 'error', message: `Extension "${extensionPath}" error: ${error instanceof Error ? error.message : String(error)}` }); }
}
loadedExtensions.runtime.pendingVirtualModelRegistrations = [];
await modelRuntime.refresh({ allowNetwork: false });
const services = { cwd, agentDir, modelRuntime, settingsManager, resourceLoader, diagnostics };
let lifecycleErrors = 0;
const { session, extensionsResult } = await createAgentSessionFromServices({
  services, sessionManager: SessionManager.inMemory(cwd), noTools: 'all',
});
await session.bindExtensions({ mode: 'print', onError: () => { lifecycleErrors++; } });
const runner = session.extensionRunner;
const extensionErrors = () => lifecycleErrors + extensionsResult.errors.length +
  services.diagnostics.filter(diagnostic => diagnostic.type === 'error').length;
const providerIds = () => modelRuntime.getRegisteredProviderIds().filter(provider => provider !== 'codex-mixin');
const modelId = model => `${model.provider}/${model.id}`;
const models = () => providerIds().flatMap(provider => modelRuntime.getModels(provider));

function providerDisplayName(providerId) {
  const meta = providerMeta.get(providerId);
  if (meta?.display_name) return meta.display_name;
  const reg = modelRuntime.getRegisteredProvider?.(providerId);
  if (typeof reg?.name === 'string' && reg.name.trim()) return reg.name.trim();
  return providerId;
}

function providerDetails() {
  return providerIds().map(provider => ({
    id: provider,
    display_name: providerDisplayName(provider),
    package: providerMeta.get(provider)?.package ?? null,
    auth_status: modelRuntime.getProviderAuthStatus(provider).configured ? 'configured' : 'not_configured',
    using_oauth: modelRuntime.isUsingOAuth(provider),
    model_count: modelRuntime.getModels(provider).length,
  }));
}

async function commandArguments(command) {
  if (typeof command.getArgumentCompletions !== 'function') return [];
  let timer;
  try {
    const raw = await Promise.race([
      Promise.resolve().then(() => command.getArgumentCompletions('')),
      new Promise(resolve => { timer = setTimeout(() => resolve(null), 500); }),
    ]);
    if (!Array.isArray(raw)) return [];
    const seen = new Set();
    const out = [];
    for (const item of raw) {
      if (!item || typeof item.value !== 'string') continue;
      const value = item.value.trim();
      if (!value || seen.has(value)) continue;
      seen.add(value);
      const label = typeof item.label === 'string' && item.label.trim() ? item.label.trim() : value;
      const description = typeof item.description === 'string' && item.description.trim() ? item.description.trim() : null;
      out.push({ value, label, description });
      if (out.length >= 24) break;
    }
    return out;
  } catch {
    return [];
  } finally {
    clearTimeout(timer);
  }
}

const commands = () => Promise.all(runner.getRegisteredCommands().map(async command => ({
  name: command.invocationName,
  description: command.description ?? '',
  package: command.sourceInfo?.source ?? null,
  arguments: await commandArguments(command),
})));

async function refreshModels() {
  const providers = providerIds();
  if (providers.length === 0) return [];
  const result = await modelRuntime.refresh({
    allowNetwork: true, providers, signal: AbortSignal.timeout(12_000),
  });
  if (result.aborted) throw httpError(504, 'Pi model discovery timed out');
  return [...result.errors.keys()];
}

function respond(response, status, body) {
  response.writeHead(status, { 'Content-Type': 'application/json' });
  response.end(JSON.stringify(body));
}

function httpError(status, message) {
  return Object.assign(new Error(message), { status });
}

async function readJson(request, limit) {
  const buffers = [];
  let size = 0;
  for await (const buffer of request.iterator({ destroyOnReturn: false })) {
    size += buffer.length;
    if (size > limit) {
      request.resume();
      throw httpError(413, 'Pi bridge request is too large');
    }
    buffers.push(buffer);
  }
  try {
    return JSON.parse(Buffer.concat(buffers));
  } catch {
    throw httpError(400, 'Invalid JSON for Pi bridge');
  }
}

function findModel(requested) {
  const catalog = models();
  const exact = catalog.find(model => modelId(model) === requested);
  if (exact) return exact;
  const matches = catalog.filter(model => model.id === requested);
  if (matches.length === 1) return matches[0];
  if (matches.length > 1) throw httpError(400, 'Use provider/model to disambiguate this Pi model');
  throw httpError(404, 'Pi plugin model was not found');
}

function decodeBlocks(content) {
  if (typeof content === 'string') return [{ type: 'text', text: content }];
  if (content == null) return [];
  if (!Array.isArray(content)) throw httpError(400, 'Unsupported Pi message content');
  return content.map(part => {
    if (part.type === 'text') return { type: 'text', text: part.text };
    if (part.type === 'image_url') {
      const match = /^data:(image\/[a-z0-9.+-]+);base64,([a-z0-9+/=\r\n]+)$/i.exec(part.image_url?.url ?? '');
      if (match) return { type: 'image', mimeType: match[1], data: match[2] };
    }
    throw httpError(400, 'Pi bridge supports text and base64 data URL images');
  });
}

function decodeCall(call) {
  if (!call || typeof call !== 'object' || !call.function || typeof call.function.name !== 'string') {
    throw httpError(400, 'Pi bridge tool call requires a function name');
  }
  let args;
  try {
    args = JSON.parse(call.function.arguments ?? '{}');
    if (!args || typeof args !== 'object' || Array.isArray(args)) throw new Error();
  } catch {
    throw httpError(400, 'Pi bridge tool arguments must be a JSON object');
  }
  return { type: 'toolCall', id: call.id, name: call.function.name, arguments: args };
}

function decodeMessage(message, model, toolNames) {
  const timestamp = Date.now();
  const content = decodeBlocks(message.content);
  switch (message.role) {
    case 'system':
    case 'developer':
      if (content.some(block => block.type !== 'text')) throw httpError(400, 'System instructions must be text');
      return { role: 'system', timestamp, content };
    case 'user':
      return { role: 'user', timestamp, content };
    case 'tool':
      if (!toolNames.has(message.tool_call_id)) throw httpError(400, 'Pi tool result has no matching tool call');
      return { role: 'toolResult', timestamp, content, toolCallId: message.tool_call_id,
        toolName: toolNames.get(message.tool_call_id), isError: false };
    case 'assistant': {
      if (content.some(block => block.type !== 'text')) throw httpError(400, 'Assistant history must be text');
      const calls = (message.tool_calls ?? []).map(decodeCall);
      for (const call of calls) toolNames.set(call.id, call.name);
      const thinking = message.reasoning_content ? [{ type: 'thinking', thinking: message.reasoning_content }] : [];
      return { role: 'assistant', timestamp, content: [...thinking, ...content, ...calls],
        api: model.api, provider: model.provider, model: model.id,
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
        stopReason: calls.length ? 'toolUse' : 'stop' };
    }
    default:
      throw httpError(400, 'Unsupported Pi message role');
  }
}

function contextFor(body, model) {
  if (!Array.isArray(body.messages)) throw httpError(400, 'Pi bridge requires messages');
  if (body.tools !== undefined && !Array.isArray(body.tools)) {
    throw httpError(400, 'Pi bridge tools must be an array');
  }
  const toolNames = new Map();
  const tools = body.tools?.map(tool => {
    if (tool?.type !== 'function' || !tool.function?.name) throw httpError(400, 'Pi bridge supports function tools');
    return { name: tool.function.name, description: tool.function.description ?? '',
      parameters: tool.function.parameters ?? { type: 'object', properties: {} } };
  });
  return { messages: body.messages.map(message => decodeMessage(message, model, toolNames)), tools };
}

function usageOf(usage) {
  return {
    prompt_tokens: usage.input + usage.cacheRead + usage.cacheWrite,
    completion_tokens: usage.output,
    total_tokens: usage.totalTokens,
    prompt_tokens_details: { cached_tokens: usage.cacheRead },
    completion_tokens_details: { reasoning_tokens: usage.reasoning },
  };
}

function reasonOf(message) {
  if (message.stopReason === 'toolUse') return 'tool_calls';
  if (message.stopReason === 'length') return 'length';
  return 'stop';
}

function completeMessage(message) {
  const content = message.content.filter(block => block.type === 'text').map(block => block.text).join('');
  const reasoning = message.content.filter(block => block.type === 'thinking').map(block => block.thinking).join('');
  const calls = message.content.filter(block => block.type === 'toolCall');
  return {
    role: 'assistant', content: content || null, reasoning_content: reasoning || undefined,
    tool_calls: calls.length ? calls.map(call => ({ id: call.id, type: 'function',
      function: { name: call.name, arguments: JSON.stringify(call.arguments) } })) : undefined,
  };
}

function requestOptions(body, signal, request) {
  const toolChoice = body.tool_choice;
  if (toolChoice !== undefined && toolChoice !== 'auto' && toolChoice !== 'none') {
    throw httpError(400, 'Pi bridge supports only auto and none for tool_choice');
  }
  const reasoning = body.reasoning_effort;
  const levels = ['minimal', 'low', 'medium', 'high', 'xhigh', 'max'];
  return {
    signal, toolChoice, temperature: body.temperature,
    maxTokens: body.max_completion_tokens ?? body.max_tokens,
    maxRetries: 0,
    reasoning: levels.includes(reasoning) ? reasoning : undefined,
    sessionId: request.headers['x-mixin-pi-session'],
    onPayload: payload => runner.emitBeforeProviderRequest(payload),
    transformHeaders: headers => runner.emitBeforeProviderHeaders(headers),
    onResponse: ({ status, headers }) => runner.emit({ type: 'after_provider_response', status, headers }),
  };
}

function streamEvent(event, calls) {
  if (event.type === 'start') return { role: 'assistant' };
  if (event.type === 'text_delta') return { content: event.delta };
  if (event.type === 'thinking_delta') return { reasoning_content: event.delta };
  if (event.type === 'toolcall_start') {
    const call = event.partial.content[event.contentIndex];
    const index = calls.size;
    calls.set(event.contentIndex, { index, streamed: false });
    return { tool_calls: [{ index, id: call.id, type: 'function', function: { name: call.name, arguments: '' } }] };
  }
  const call = calls.get(event.contentIndex);
  if (!call) return;
  if (event.type === 'toolcall_delta' && event.delta) {
    call.streamed = true;
    return { tool_calls: [{ index: call.index, function: { arguments: event.delta } }] };
  }
  if (event.type === 'toolcall_end' && !call.streamed) {
    return { tool_calls: [{ index: call.index, function: { arguments: JSON.stringify(event.toolCall.arguments) } }] };
  }
}

async function complete(request, response) {
  const body = await readJson(request, 32 * 1024 * 1024);
  const model = findModel(body.model);
  const signal = new AbortController();
  const abort = () => signal.abort();
  response.on('close', abort);
  try {
    const stream = modelRuntime.streamSimple(model, contextFor(body, model), requestOptions(body, signal.signal, request));
    const reply = { id: `chatcmpl-pi-${randomUUID()}`, created: Math.floor(Date.now() / 1000), model: body.model };
    const calls = new Map();
    const write = async data => {
      if (!response.write(`data: ${data}\n\n`)) await once(response, 'drain', { signal: signal.signal });
    };
    const chunk = (choices, usage) => write(JSON.stringify({ ...reply, object: 'chat.completion.chunk', choices, usage }));
    for await (const event of stream) {
      if (event.type === 'error') throw httpError(502, 'Pi provider request failed');
      if (event.type === 'done') {
        if (event.reason === 'deferred') throw httpError(502, 'Pi provider returned a deferred response');
        const usage = usageOf(event.message.usage);
        const finish_reason = reasonOf(event.message);
        if (body.stream) {
          await chunk([{ index: 0, delta: {}, finish_reason }]);
          if (body.stream_options?.include_usage) await chunk([], usage);
          await write('[DONE]');
          response.end();
        } else {
          respond(response, 200, { ...reply, object: 'chat.completion', usage,
            choices: [{ index: 0, message: completeMessage(event.message), finish_reason }] });
        }
        return;
      }
      if (!body.stream) continue;
      if (!response.headersSent) response.writeHead(200, {
        'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache',
      });
      const delta = streamEvent(event, calls);
      if (delta) await chunk([{ index: 0, delta, finish_reason: null }]);
    }
    throw httpError(502, 'Pi provider closed the event stream early');
  } finally {
    response.off('close', abort);
    signal.abort();
  }
}

let pendingCommand = false;
async function runCommand(request, response) {
  const { name, args = '' } = await readJson(request, 16_384);
  const command = typeof name === 'string' && runner.getCommand(name);
  if (!command) throw httpError(404, 'Pi command was not found');
  if (typeof args !== 'string') throw httpError(400, 'Pi command arguments must be a string');
  if (pendingCommand) throw httpError(409, 'Another Pi command is in progress');
  pendingCommand = true;
  const notifications = [];
  try {
    const context = runner.createCommandContext();
    const commandUI = Object.create(context.ui);
    for (const method of ['select', 'confirm', 'input', 'custom', 'editor']) {
      commandUI[method] = async () => {
        throw httpError(422, 'Pi command requires interactive UI');
      };
    }
    commandUI.notify = (message, type = 'info') => {
      if (notifications.length < 32) notifications.push({ type, message: String(message).slice(0, 4096) });
    };
    Object.defineProperty(context, 'ui', { value: commandUI });
    await command.handler(args, context);
    respond(response, 200, { name, notifications });
  } finally {
    pendingCommand = false;
  }
}

const server = createServer(async (request, response) => {
  const actual = Buffer.from(request.headers.authorization ?? '');
  const expected = Buffer.from(`Bearer ${token}`);
  if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) {
    respond(response, 401, { error: { message: 'Unauthorized' } });
    return;
  }
  try {
    switch (`${request.method} ${request.url}`) {
      case 'GET /health':
        respond(response, 200, {
          ok: true,
          version: VERSION,
          providers: providerIds(),
          provider_details: providerDetails(),
          model_count: models().length,
          commands: await commands(),
          extension_errors: extensionErrors(),
        });
        break;
      case 'GET /v1/models':
        respond(response, 200, { object: 'list', extension_errors: extensionErrors(), data: models().map(model => ({
          id: modelId(model), object: 'model', provider: model.provider,
          provider_display_name: providerDisplayName(model.provider),
          package: providerMeta.get(model.provider)?.package ?? null,
          name: model.name,
          context_window: model.contextWindow, max_tokens: model.maxTokens, reasoning: model.reasoning,
          supports_image: model.input.includes('image'),
          auth_status: modelRuntime.getProviderAuthStatus(model.provider).configured ? 'configured' : 'not_configured',
          using_oauth: modelRuntime.isUsingOAuth(model.provider),
        })) });
        break;
      case 'POST /admin/models/refresh':
        respond(response, 200, { failed_providers: await refreshModels() });
        break;
      case 'POST /v1/chat/completions':
        await complete(request, response);
        break;
      case 'GET /admin/commands':
        respond(response, 200, await commands());
        break;
      case 'POST /admin/commands/run':
        await runCommand(request, response);
        break;
      default:
        respond(response, 404, { error: { message: 'Pi bridge route was not found' } });
    }
  } catch (error) {
    if (response.destroyed) return;
    const message = error.status ? error.message : 'Pi plugin operation failed';
    if (response.headersSent) {
      response.end(`data: ${JSON.stringify({ error: { message } })}\n\ndata: [DONE]\n\n`);
    } else {
      respond(response, error.status ?? 502, { error: { message } });
    }
  }
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
process.stdout.write(`mixin-pi-ready ${server.address().port}\n`);
process.stdin.resume();
process.stdin.on('end', () => {
  const deadline = setTimeout(() => process.exit(0), 30_000);
  deadline.unref();
  server.close(async () => {
    await modelRuntime.settleCredentialWrites();
    session.dispose();
    process.exit(0);
  });
});
