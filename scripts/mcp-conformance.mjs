import path from 'node:path';
import process from 'node:process';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';

const binary = path.resolve(process.argv[2] ?? 'target/release/sippion');
const root = path.resolve(process.argv[3] ?? 'eval/fixture');

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function assertCompactContextContract(text) {
  const header = text.split('\n').find((line) => line.includes('CTX v=4 '));
  assert(header, 'compact CTX v=4 header missing');
  assert(/\btarget_t=\d+\b/.test(header), 'compact context target token budget missing');
  assert(/\bhard_b=\d+\b/.test(header), 'compact context hard byte budget missing');
  assert(/\bscan_b=\d+\b/.test(header), 'compact context scan byte metadata missing');
}

async function runClient(options, expectedEra) {
  const client = new Client(
    { name: `sippion-conformance-${expectedEra}`, version: '1.0.0' },
    options,
  );
  const transport = new StdioClientTransport({
    command: binary,
    args: ['mcp', '--root', root],
    stderr: 'pipe',
  });
  try {
    await client.connect(transport);
    assert(client.getProtocolEra() === expectedEra, `expected ${expectedEra} era`);
    const listed = await client.listTools();
    assert(listed.tools.length === 1, 'Sippion must expose exactly one MCP tool');
    const tool = listed.tools[0];
    assert(tool.name === 'repo_context', 'repo_context tool missing');
    assert(tool.annotations?.readOnlyHint === true, 'repo_context must advertise readOnlyHint');
    const result = await client.callTool({
      name: 'repo_context',
      arguments: { q: 'validate_session_token' },
    });
    const text = result.content?.find((item) => item.type === 'text')?.text ?? '';
    assert(text.includes('src/auth.rs'), 'repo_context did not return expected fixture path');

    // The assertions above exercise MCP negotiation, discovery, and tool invocation through the
    // official client. This separate check guards Sippion's compact model-visible context contract;
    // it intentionally does not depend on removed internal metadata such as `PACK adaptive=true`.
    assertCompactContextContract(text);
  } finally {
    await client.close();
  }
}

await runClient(
  { versionNegotiation: { mode: { pin: '2026-07-28' } } },
  'modern',
);
await runClient({}, 'legacy');
console.log('official MCP client conformance: modern + legacy stdio + compact context OK');
