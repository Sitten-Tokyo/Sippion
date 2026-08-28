import path from 'node:path';
import process from 'node:process';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';

const binary = path.resolve(process.argv[2] ?? 'target/release/sippion');
const root = path.resolve(process.argv[3] ?? 'eval/fixture');

function assert(condition, message) {
  if (!condition) throw new Error(message);
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
    assert(text.includes('PACK adaptive=true'), 'bounded adaptive pack metadata missing');
  } finally {
    await client.close();
  }
}

await runClient(
  { versionNegotiation: { mode: { pin: '2026-07-28' } } },
  'modern',
);
await runClient({}, 'legacy');
console.log('official MCP client conformance: modern + legacy stdio OK');
