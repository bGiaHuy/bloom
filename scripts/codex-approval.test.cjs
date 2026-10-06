const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const assert = require('node:assert/strict');
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bloom-approval-test-'));
const script = path.resolve(__dirname, 'codex-hook.ps1');
let heartbeat;
function run(name, turn = 'turn-1', timeout = 10) {
  const child = spawn('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', script, '-StateRoot', root, '-ApprovalWaitSeconds', String(timeout)], { windowsHide: true });
  let output = '';
  child.stdout.on('data', b => output += b);
  child.stdin.end(JSON.stringify({ hook_event_name: name, session_id: 'test-chat', turn_id: turn, cwd: 'C:\\test', tool_name: 'Bash', tool_input: { command: 'echo explicit-approval-test', description: 'Test request' } }));
  return new Promise((resolve, reject) => { child.on('error', reject); child.on('exit', code => { if (code) reject(new Error(`Hook exited ${code}`)); else resolve(output.trim()); }); });
}
async function requestFile() {
  const end = Date.now() + 20000;
  while (Date.now() < end) {
    const dir = path.join(root, 'requests');
    const file = fs.existsSync(dir) && fs.readdirSync(dir).find(f => f.endsWith('.json'));
    if (file) { const full = path.join(dir, file); return { full, row: JSON.parse(fs.readFileSync(full)) }; }
    await new Promise(r => setTimeout(r, 30));
  }
  throw new Error('No live request');
}
(async () => {
  assert.equal(await run('PermissionRequest'), '', 'Disconnected island must not approve');
  fs.writeFileSync(path.join(root, 'island-approvals-enabled'), 'enabled');
  heartbeat = setInterval(() => fs.writeFileSync(path.join(root, 'island-heartbeat'), 'active'), 200);
  fs.writeFileSync(path.join(root, 'island-heartbeat'), 'active');
  for (const decision of ['allow', 'deny']) {
    await run('UserPromptSubmit');
    const result = run('PermissionRequest');
    const { full, row } = await requestFile();
    fs.writeFileSync(full.replace(/\.json$/, '.response'), JSON.stringify({ requestId: 'wrong-request', decision: 'allow' }));
    await new Promise(r => setTimeout(r, 200));
    assert(fs.existsSync(full), 'Wrong request ID must not authorize anything');
    fs.writeFileSync(full.replace(/\.json$/, '.response'), JSON.stringify({ requestId: row.requestId, decision }));
    assert.equal(JSON.parse(await result).hookSpecificOutput.decision.behavior, decision);
    assert(!fs.existsSync(full), 'Transient command must be removed');
  }
  await run('UserPromptSubmit');
  assert.equal(await run('PermissionRequest', 'turn-1', 1), '', 'Timeout must return to native approval');
  await run('UserPromptSubmit');
  const cancelled = run('PermissionRequest');
  await requestFile();
  await run('Interrupt');
  assert.equal(await cancelled, '', 'Cancelled turn must not approve');
  clearInterval(heartbeat); heartbeat = null;
  fs.unlinkSync(path.join(root, 'island-heartbeat'));
  assert.equal(await run('PermissionRequest'), '', 'Closed Bloom must not hold approvals');
  console.log('Explicit allow/deny, wrong ID, expiry, cancellation and disconnected fallback passed.');
})().catch(e => { console.error(e); process.exitCode = 1; }).finally(() => {
  if (heartbeat) clearInterval(heartbeat);
  if (root.startsWith(os.tmpdir()) && path.basename(root).startsWith('bloom-approval-test-')) fs.rmSync(root, { recursive: true, force: true });
});
