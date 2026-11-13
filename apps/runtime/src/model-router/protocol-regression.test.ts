/** Regression scenarios from CC Switch src-tauri/src/proxy/providers/
 * codex_responses_arguments.rs, codex.rs, transform_responses.rs,
 * streaming_responses.rs, transform_codex_responses_xai_sanitize.rs and proxy/forwarder.rs (MIT).
 * Commit: 3c76d3bda670297dd8c27d7d9be9b18a4c9143f5
 * License: ../../../../packages/contracts/third-party/subscription-accounts-reference-LICENSE
 * Fixture shape also follows CLIProxyAPI v8.0.69 (f7ae8fee, MIT)
 * internal/runtime/executor/codex_executor_commentary_test.go and plan E8 protocol-probe.py.
 * No converter implementation here. Only a pinned process and loopback fake upstream.
 * The OpenAI duplicate-arguments exception is retained. Codex-specific include/stream
 * normalization is checked separately from third-party field stripping. Orphan receipts
 * are preserved as user text, matching the reference repair intent without a second converter.
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer, type Server } from 'node:http';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomBytes } from 'node:crypto';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const binary = process.env.MATOU_MODEL_ROUTER_BINARY;
// Use stderr directly: Vitest suppresses captured console output for a fully skipped suite.
if (!binary) process.stderr.write('[protocol-regression] SKIPPED: MATOU_MODEL_ROUTER_BINARY is unset; pinned CLIProxyAPI 8.0.69 integration was NOT run.\n');
const suite = binary ? describe : describe.skip;
type Json = Record<string, any>; // test-only heterogeneous wire fixtures
const event = (payload: Json) => `data: ${JSON.stringify(payload)}\n\n`;
function responseEvents(mode: 'text' | 'tool' | 'length'): string {
  const output: Json[] = [
    { id: 'reasoning-1', type: 'reasoning', summary: [{ type: 'summary_text', text: 'Reasoning summary' }], encrypted_content: 'fixture-signature' },
    { type: 'message', role: 'assistant', phase: 'commentary', content: [{ type: 'output_text', text: 'Public progress' }] },
    { type: 'message', role: 'assistant', phase: 'final_answer', content: [{ type: 'output_text', text: 'Final answer' }] },
  ];
  const events: Json[] = [
    { type: 'response.created', response: { id: 'resp_fixture', model: 'gpt-6.1-sol' } },
    { type: 'response.output_item.added', output_index: 0, item: { id: 'reasoning-1', type: 'reasoning', summary: [] } },
    { type: 'response.reasoning_summary_part.added', output_index: 0, summary_index: 0, part: { type: 'summary_text', text: '' } },
    { type: 'response.reasoning_summary_text.delta', output_index: 0, summary_index: 0, delta: 'Reasoning summary' },
    { type: 'response.reasoning_summary_text.done', output_index: 0, summary_index: 0, text: 'Reasoning summary' },
    { type: 'response.output_item.done', output_index: 0, item: output[0] },
  ];
  for (let i = 1; i < output.length; i++) {
    const item = output[i]!;
    events.push({ type: 'response.output_item.added', output_index: i, item: { type: 'message', role: 'assistant', phase: item.phase } },
      { type: 'response.output_text.delta', output_index: i, delta: item.content[0].text },
      { type: 'response.output_item.done', output_index: i, item });
  }
  if (mode === 'tool') {
    const call = { type: 'function_call', id: 'fc_1', call_id: 'call_next', name: 'Read', arguments: '{"file_path":"fixture.txt"}' };
    output.push(call);
    events.push({ type: 'response.output_item.added', output_index: 3, item: { ...call, arguments: '' } },
      { type: 'response.function_call_arguments.delta', output_index: 3, item_id: 'fc_1', delta: call.arguments },
      { type: 'response.function_call_arguments.done', output_index: 3, item_id: 'fc_1', arguments: call.arguments },
      { type: 'response.output_item.done', output_index: 3, item: call });
  }
  events.push({ type: mode === 'length' ? 'response.incomplete' : 'response.completed', response: { id: 'resp_fixture', model: 'gpt-6.1-sol', status: mode === 'length' ? 'incomplete' : 'completed',
    ...(mode === 'length' ? { incomplete_details: { reason: 'max_output_tokens' } } : {}), output, usage: { input_tokens: 5, output_tokens: 7 } } });
  return events.map(event).join('');
}
const listen = async (server: Server): Promise<number> => {
  await new Promise<void>((yes, no) => { server.once('error', no); server.listen(0, '127.0.0.1', () => { server.off('error', no); yes(); }); });
  const address = server.address();
  if (!address || typeof address === 'string') throw Error('loopback address unavailable');
  return address.port;
};
const close = async (server?: Server) => {
  if (!server?.listening) return;
  server.closeAllConnections();
  await new Promise<void>((yes, no) => server.close(e => e ? no(e) : yes()));
};
const delay = (ms: number) => new Promise(yes => setTimeout(yes, ms));
function streamEvents(raw: string): Json[] {
  return raw.split('\n').filter(line => line.startsWith('data: ') && line !== 'data: [DONE]').map(line => JSON.parse(line.slice(6)) as Json);
}
const tool = { name: 'Read', description: 'read local fixture', input_schema: { type: 'object', properties: { file_path: { type: 'string' } }, required: ['file_path'] } };
const messages = (stream: boolean): Json => ({ model: 'fixture-gpt', max_tokens: 128000, stream, tools: [tool], messages: [
  { role: 'user', content: 'Read fixture.txt' },
  { role: 'assistant', content: [
    { type: 'thinking', thinking: 'Previous progress', signature: 'cpa:commentary:v1' },
    { type: 'text', text: 'Previous progress' },
    { type: 'tool_use', id: 'toolu_fixture', name: 'Read', input: { file_path: 'fixture.txt' } },
  ] }, { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'toolu_fixture', content: 'local content' }] },
] });

suite('pinned router protocol regressions (fake upstream, no OAuth)', () => {
  let upstream: Server | undefined, child: ChildProcess | undefined, temp: string | undefined, base = '', routerLog = '';
  let mode: 'text' | 'tool' | 'length' = 'text';
  const captured: { path: string; body: Json }[] = [];
  const clientKey = randomBytes(24).toString('hex');
  async function request(path: string, body: Json) {
    const response = await fetch(base + path, { method: 'POST', headers: { Authorization: `Bearer ${clientKey}`, 'Content-Type': 'application/json' },
      body: JSON.stringify(body), signal: AbortSignal.timeout(6000) });
    const raw = await response.text();
    return { status: response.status, raw };
  }
  beforeAll(async () => {
    // All generated files are inside this worktree. Never os.tmpdir(), user OAuth dirs or main checkout.
    const root = fileURLToPath(new URL('../../../../', import.meta.url));
    await mkdir(join(root, '.cache', 'subscription-protocol'), { recursive: true });
    temp = await mkdtemp(join(root, '.cache', 'subscription-protocol', 'run-'));
    await mkdir(join(temp, 'auth'), { mode: 0o700 });
    upstream = createServer(async (req, res) => {
      try {
        const chunks: Buffer[] = [];
        for await (const chunk of req) chunks.push(Buffer.from(chunk));
        const body = JSON.parse(Buffer.concat(chunks).toString()) as Json;
        captured.push({ path: req.url ?? '', body });
        if (req.headers.authorization !== 'Bearer fixture-local-not-real') { res.writeHead(401).end(); return; }
        // A strict upstream rejects a receipt with no matching call id. The router must expose
        // the failure, never fabricate an id or tell Claude the tool call succeeded.
        if ((body.input ?? []).some((item: Json) => item.type === 'function_call_output' && !item.call_id)) {
          res.writeHead(400, { 'Content-Type': 'application/json' }).end(JSON.stringify({ error: { type: 'invalid_request_error', message: 'function_call_output requires call_id' } })); return;
        }
        res.writeHead(200, { 'Content-Type': 'text/event-stream' }).end(responseEvents(mode));
      } catch { res.writeHead(400).end(); }
    });
    const upstreamPort = await listen(upstream);
    const reservation = createServer();
    const port = await listen(reservation); await close(reservation);
    const config = {
      server: { host: '127.0.0.1', port }, management: { 'disable-control-panel': true, 'disable-auto-update-panel': true },
      access: { 'api-keys': [clientKey] }, oauth: { 'auth-dir': join(temp, 'auth') },
      'api-keys': { codex: [{ name: 'fixture', 'base-url': `http://127.0.0.1:${upstreamPort}`, models: [{ name: 'gpt-6.1-sol', alias: 'fixture-gpt' }], keys: [{ 'api-key': 'fixture-local-not-real' }] }] },
      client: { claude: { 'show-codex-commentary': true } }, multimedia: { 'disable-image-generation': 'passthrough' },
      plugins: { enabled: false }, routing: { retry: { 'request-retry': 0 } },
      observability: { logs: { debug: false, 'logging-to-file': false, 'request-log': false }, usage: { 'usage-statistics-enabled': false } },
    };
    const configPath = join(temp, 'config.yaml');
    await writeFile(configPath, JSON.stringify(config), { mode: 0o600 });
    child = spawn(resolve(binary!), ['-config', configPath, '-local-model', '-no-browser'], { cwd: temp, env: { PATH: process.env.PATH ?? '', LANG: 'en_US.UTF-8' }, stdio: ['ignore', 'pipe', 'pipe'] });
    let spawnError: Error | undefined;
    child.once('error', e => { spawnError = e; });
    child.stdout?.on('data', b => { routerLog = (routerLog + String(b)).slice(-32000); });
    child.stderr?.on('data', b => { routerLog = (routerLog + String(b)).slice(-32000); });
    base = `http://127.0.0.1:${port}`;
    for (let i = 0; i < 200; i++) {
      if (spawnError) throw spawnError;
      if (child.exitCode !== null) throw Error(`pinned router exited early (${child.exitCode})`);
      try {
        const ready = await fetch(base + '/v1/models', { headers: { Authorization: `Bearer ${clientKey}` }, signal: AbortSignal.timeout(300) });
        if (ready.ok) {
          expect(routerLog).toMatch(/Version: 8\.0\.69, Commit: f7ae8fee/);
          console.info('[protocol-regression] Running pinned 8.0.69 / f7ae8fee against loopback fake upstream; no real accounts.');
          return;
        }
      } catch { /* bounded readiness retry */ }
      await delay(25);
    }
    throw Error('pinned router readiness deadline exceeded');
  }, 15000);
  afterAll(async () => {
    try {
      if (child && child.exitCode === null && child.signalCode === null) {
        child.kill('SIGTERM');
        for (let i = 0; i < 80 && child.exitCode === null && child.signalCode === null; i++) await delay(25);
        if (child.exitCode === null && child.signalCode === null) {
          child.kill('SIGKILL');
          await new Promise<void>(yes => child!.once('exit', () => yes()));
        }
      }
    } finally {
      await close(upstream);
      if (temp) await rm(temp, { recursive: true, force: true });
    }
  }, 10000);
  it.each([false, true])('replays paired tools/thinking and keeps progress + final text, stream=%s', async stream => {
    mode = 'text';
    const { status, raw } = await request('/v1/messages', messages(stream));
    expect(status).toBe(200);
    const content: Json[] = stream ? streamEvents(raw) : JSON.parse(raw).content;
    const text = stream ? content.filter(e => e.delta?.type === 'text_delta').map(e => e.delta.text).join('') : content.filter(b => b.type === 'text').map(b => b.text).join('');
    expect(text).toContain('Public progress'); expect(text).toContain('Final answer');
    expect(raw).toContain('Reasoning summary');
    const sent = captured.at(-1)!;
    expect(sent.path).toBe('/responses');
    expect(sent.body.model).toBe('gpt-6.1-sol');
    expect(sent.body.input).toEqual(expect.arrayContaining([
      expect.objectContaining({ type: 'function_call', call_id: 'toolu_fixture', name: 'Read' }),
      expect.objectContaining({ type: 'function_call_output', call_id: 'toolu_fixture', output: 'local content' }),
    ]));
    expect(sent.body.input.filter((item: Json) => item.type === 'message' && JSON.stringify(item).includes('Previous progress'))).toHaveLength(1);
    expect(sent.body.tools.every((t: Json) => t.type === 'function')).toBe(true);
    if (stream) {
      expect(content.filter(e => e.type === 'message_stop')).toHaveLength(1);
      expect(content.find(e => e.type === 'message_delta')?.delta.stop_reason).toBe('end_turn');
    } else expect(JSON.parse(raw).stop_reason).toBe('end_turn');
  });
  it('preserves duplicate raw argument keys for the official OpenAI exception', async () => {
    mode = 'text';
    const args = '{ "time":null, "time":5, "nested":{"x":1,"x":2} }';
    const result = await request('/v1/responses', { model: 'fixture-gpt', input: [
      { type: 'function_call', call_id: 'call_dup', name: 'Read', arguments: args },
      { type: 'function_call_output', call_id: 'call_dup', output: 'ok' },
    ], stream: false });
    expect(result.status).toBe(200);
    expect(captured.at(-1)!.body.input.find((i: Json) => i.type === 'function_call').arguments).toBe(args);
  });
  it('normalizes Codex transport fields while retaining official-only metadata', async () => {
    mode = 'text';
    const result = await request('/v1/responses', { model: 'fixture-gpt', stream: false,
      include: ['output.encrypted_content'], access_programs: ['fixture-program'], stream_options: { reasoning_summary_delivery: 'auto' },
      input: [{ type: 'message', role: 'user', content: 'hi', internal_chat_message_metadata_passthrough: { fixture: true } }] });
    expect(result.status).toBe(200);
    const body = captured.at(-1)!.body;
    // Pinned ConvertOpenAIResponsesRequestToCodex canonicalizes these two transport fields.
    expect(body.include).toEqual(['reasoning.encrypted_content']);
    expect(body.access_programs).toEqual(['fixture-program']);
    expect(body.stream_options).toBeUndefined();
    expect(body.input[0].internal_chat_message_metadata_passthrough).toEqual({ fixture: true });
  });
  it('preserves orphan tool output as user text without inventing a call_id', async () => {
    const result = await request('/v1/responses', { model: 'fixture-gpt', stream: false, input: [{ type: 'function_call_output', output: 'orphan result' }] });
    expect(result.status).toBe(200);
    const receipt = captured.at(-1)!.body.input[0];
    expect(receipt.type).toBe('message');
    expect(receipt.role).toBe('user');
    expect(receipt.content).toEqual([{ type: 'input_text', text: 'Tool output from tool:\norphan result' }]);
    expect(receipt.call_id).toBeUndefined();
  });
  it.each([['tool', 'tool_use'], ['length', 'max_tokens']] as const)('maps streaming termination %s to %s', async (nextMode, stopReason) => {
    mode = nextMode;
    const result = await request('/v1/messages', { model: 'fixture-gpt', max_tokens: 128000, stream: true, messages: [{ role: 'user', content: 'hi' }], tools: [tool] });
    expect(result.status).toBe(200);
    const events = streamEvents(result.raw);
    expect(events.filter(e => e.type === 'message_stop')).toHaveLength(1);
    expect(events.find(e => e.type === 'message_delta')?.delta.stop_reason).toBe(stopReason);
    if (nextMode === 'tool') expect(events.some(e => e.content_block?.type === 'tool_use' && e.content_block.id === 'call_next')).toBe(true);
  });
});
