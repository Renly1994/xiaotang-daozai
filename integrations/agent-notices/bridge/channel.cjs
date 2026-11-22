'use strict';
// The descriptor contains only local discovery information, never request or reply contents.
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const { randomBytes, createHmac } = require('node:crypto');
const descriptor = path.join(__dirname, 'live.json');
const limit = 192 * 1024;
const mac = (secret, ...parts) => createHmac('sha256', secret).update(JSON.stringify(parts)).digest('hex');
const nonce = () => randomBytes(24).toString('hex');
function frame(stream, fn) {
  let buffer = '';
  stream.setEncoding('utf8');
  stream.on('data', chunk => {
    buffer += chunk;
    for (;;) {
      const end = buffer.indexOf('\n');
      if (end < 0) break;
      if (end > limit) { stream.destroy(); return; }
      const text = buffer.slice(0,end);
      buffer = buffer.slice(end + 1);
      try { fn(JSON.parse(text)); } catch { stream.destroy(); return; }
    }
    if (buffer.length > limit) stream.destroy();
  });
}
const send = (stream, msg) => stream.write(JSON.stringify(msg) + '\n');
function discover() {
  try {
    const entry=JSON.parse(fs.readFileSync(descriptor, 'utf8'));
    const prefix=process.platform==='win32'?'\\\\.\\pipe\\daozai-':path.join(require('node:os').tmpdir(),'daozai-');
    const suffix=process.platform==='win32'?'':'.sock';
    if (typeof entry.endpoint!=='string'||!entry.endpoint.startsWith(prefix)||!entry.endpoint.endsWith(suffix)||
        !/^[a-f0-9]{48}$/.test(entry.endpoint.slice(prefix.length,entry.endpoint.length-suffix.length))||
        !/^[a-f0-9]{48}$/.test(entry.secret)) return {};
    return entry;
  } catch { return {}; }
}
function enabled(provider,flag){const p=discover().policy;return !!(p?.[provider]&&p[flag]);}
function wantsText(provider) {
  const {policy} = discover();
  return !!(policy?.[provider] && policy?.completion && policy?.content);
}
// A mounted DSH plugin keeps one authenticated presence socket. A legacy status
// notification (or an installed bundle on disk) is not evidence of a live responder.
function mounted() {
  const controller = new AbortController();
  let retry;
  const connect = async () => {
    try { await exchange('dsh', 'ready', () => ({}), controller.signal); }
    finally { if (!controller.signal.aborted) { retry = setTimeout(connect, 3000); retry.unref(); } }
  };
  void connect();
  return () => { clearTimeout(retry); controller.abort(); };
}
function accepts(policy, provider, kind) {
  if (!policy?.[provider]) return false;
  if (kind === 'ready') return provider === 'dsh';
  return kind === 'finished' ? !!policy.completion : ['approval','question'].includes(kind) && !!policy.interactive;
}
function exchange(provider, kind, makePayload, signal) {
  const {endpoint, secret, policy} = discover();
  if (!secret || !endpoint || signal?.aborted || !accepts(policy,provider,kind)) return Promise.resolve(null);
  return new Promise(resolve => {
    const key = nonce();
    const socket = net.createConnection(endpoint);
    let verified = false, settled = false, timer;
    const end = result => {
      if (settled) return;
      settled = true; clearTimeout(timer); signal?.removeEventListener('abort', abort);
      socket.destroy(); resolve(result);
    };
    const abort = () => end(null);
    signal?.addEventListener('abort', abort, {once:true});
    timer = setTimeout(abort, 1200);
    socket.on('error', abort).on('close', abort);
    socket.once('connect', () => send(socket, {key, provider, kind, proof:mac(secret,'client',key,provider,kind)}));
    frame(socket, msg => {
      if (!verified) {
        if (msg.proof !== mac(secret,'server',key,msg.content)) return abort();
        verified = true;
        const data = makePayload(msg.content);
        if (!data || JSON.stringify(data).length > limit - 1024) return abort();
        clearTimeout(timer);
        // DSH owns the request lifetime (including timed question cancellation).
        if (kind === 'finished' || provider === 'codex') timer = setTimeout(abort, kind === 'finished' ? 2000 : 6*3600000);
        send(socket, data);
      } else end(msg);
    });
  });
}

function serve() {
  const secret = nonce(), instance = nonce();
  const endpoint = process.platform === 'win32' ? `\\\\.\\pipe\\daozai-${instance}` :
    path.join(require('node:os').tmpdir(),`daozai-${instance}.sock`);
  let policy = {}, listening = false;
  const open = new Map(), mountedClients = new Set();
  const output = msg => send(process.stdout,msg);
  const publish = () => {
    if (!listening || !Object.hasOwn(policy,'interactive')) return;
    const tmp = descriptor + '.' + instance + '.tmp';
    fs.writeFileSync(tmp, JSON.stringify({endpoint,secret,policy}), {mode:0o600});
    fs.renameSync(tmp,descriptor);
  };
  const server = net.createServer(socket => {
    socket.on('error', () => {});
    socket.setTimeout(1500, () => socket.destroy());
    const id = nonce();
    let hello, posted = false;
    socket.once('close', () => {
      open.delete(id);
      if (mountedClients.delete(socket) && !mountedClients.size) output({dshReady:false});
      if (posted) output({cancel:id});
    });
    frame(socket, msg => {
      if (!hello) {
        const {provider, kind, key, proof} = msg;
        if (!['dsh','codex'].includes(provider) || !accepts(policy,provider,kind) ||
            typeof key !== 'string' || key.length !== 48 || proof !== mac(secret,'client',key,provider,kind)) return socket.destroy();
        hello = {provider,kind}; const content = policy.content || 0;
        return send(socket,{content, proof:mac(secret,'server',key,content)});
      }
      if (hello.kind === 'ready') {
        if (mountedClients.has(socket)) return socket.destroy();
        mountedClients.add(socket);
        if (mountedClients.size === 1) output({dshReady:true});
        socket.setTimeout(0);
        return;
      }
      if (posted || open.size >= 32) return socket.destroy();
      posted = true;
      output({id,...hello,data:msg});
      if (hello.kind === 'finished') { posted=false; return socket.end('{"done":true}\n'); }
      open.set(id,socket);
      socket.setTimeout(0);
    });
  });
  server.maxConnections = 40;
  const stop = () => {
    try { if (discover().secret === secret) fs.unlinkSync(descriptor); } catch {}
    process.exit(0);
  };
  process.stdin.once('end',stop); process.stdout.once('error',stop);
  frame(process.stdin, msg => {
    if (msg.policy) {
      policy=msg.policy;
      if (!policy.dsh) for (const socket of mountedClients) socket.destroy();
      publish(); return;
    }
    const socket=open.get(msg.id);
    if (socket) { open.delete(msg.id); socket.end(JSON.stringify(msg.reply)+'\n'); }
  });
  server.on('error',stop);
  server.listen(endpoint, () => { listening=true; publish(); });
}
module.exports = {exchange,wantsText,enabled,mounted};
if (require.main === module) serve();
