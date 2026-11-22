import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';

const kit = path.dirname(fileURLToPath(import.meta.url));
const bridge = path.join(kit, 'bridge');
const pkg = '@daozai/agent-notices';
const events = [['Stop', 'finished'], ['PermissionRequest', 'approval'], ['PreToolUse', 'question', '^request_user_input$']];

function object(value) { return value !== null && typeof value === 'object' && !Array.isArray(value); }
function failure(message, exitCode) { return Object.assign(Error(message), {exitCode}); }
function samePath(left, right) { return path.relative(left, right) === ''; }
// 之前某次运行留下的「自己的旧接入目录」：别的岛仔数据目录导出的同一套 kit（package.json 里 name 就是本包，
// 且路径以 agent-notices/bridge 结尾）。这种要能自愈替换，否则一个旧链接会把之后所有安装都挡在 exit 4。
// 注意还要覆盖「旧目录已经被删掉」的情况：链接仍指向一个不存在的旧 kit 时读不到 package.json，
// 但路径形状本身就说明是本 kit 的残留（2026-11-21 实测：Temp 里拷出的 kit 被清理后，
// 「重试接入」一直报 exit 4「接入配置被其他插件占用」，用户点重试毫无反应）。
function ownKit(dir) {
  const resolved = path.resolve(dir);
  if (!/[\\/]agent-notices[\\/]bridge$/.test(resolved)) return false;
  let packageJson;
  try { packageJson = fs.readFileSync(path.join(resolved, 'package.json'), 'utf8'); }
  catch { return !fs.existsSync(resolved); }
  try { return JSON.parse(packageJson)?.name === pkg; }
  catch { return false; }
}
function ownLinkSpec(spec) {
  return typeof spec === 'string' && spec.startsWith('link:') && ownKit(spec.slice(5));
}
function read(file) {
  try { return fs.readFileSync(file, 'utf8'); }
  catch (e) { if (e.code !== 'ENOENT') throw e; return null; }
}
function save(file, before, value) {
  const after = JSON.stringify(value, null, 2) + '\n';
  if (read(file) !== before) throw Error('Configuration changed; retry.');
  const nonce = randomUUID(), tmp = `${file}.${nonce}.tmp`;
  if (before !== null) fs.copyFileSync(file, `${file}.daozai-${nonce}.bak`, fs.constants.COPYFILE_EXCL);
  fs.writeFileSync(tmp, after, { flag: 'wx', mode: 0o600 });
  try { fs.renameSync(tmp, file); } finally { fs.rmSync(tmp, { force: true }); }
}
function connect(provider, install, dir) {
  const file = path.join(dir, provider === 'codex' ? 'hooks.json' : 'package.json');
  const before = read(file);
  if (!install && before === null) return;
  if (before === null && provider === 'dsh') throw failure('DeepSeek Harness desktop profile not found.',3);
  const config = before === null ? {} : JSON.parse(before.replace(/^\uFEFF/, ''));
  if (!object(config)) throw Error('Unsupported configuration.');
  const initial = JSON.stringify(config);
  if (install) {
    const exe = JSON.parse(fs.readFileSync(path.join(bridge, 'target.json'), 'utf8'));
    if (!path.isAbsolute(exe) || !fs.statSync(exe).isFile()) throw Error('Export the kit from the running Daozai first.');
  }
  if (provider === 'codex') {
    if (!install && !Object.hasOwn(config,'hooks')) return;
    if (!Object.hasOwn(config,'hooks')) config.hooks = {};
    if (!object(config.hooks)) throw Error('Unsupported hooks object.');
    const script = path.join(bridge, 'notify.cjs').replaceAll('\\', '/');
    const node = process.execPath.replaceAll('\\', '/');
    if ([script,node].some(p=>/["`$%!\r\n^&|<>]/.test(p))) throw Error('Hook path contains unsupported shell characters.');
    for (const [event, status, matcher] of events) {
      const legacy = `node "${script}" ${status}`;
      // The desktop host may not inherit Daozai's PATH. CALL preserves quoted paths in cmd.
      const command = process.platform === 'win32' ? `cmd.exe /d /c call "${node}" "${script}" ${status}` : legacy;
      const groups = Object.hasOwn(config.hooks,event) ? config.hooks[event] : [];
      if (!Array.isArray(groups)) throw Error(`Unsupported hook list: ${event}`);
      const owned = g => g?.matcher === matcher && Array.isArray(g?.hooks) &&
        g.hooks.length === 1 && g.hooks[0]?.type === 'command' &&
        g.hooks[0]?.statusMessage === 'Daozai' &&
        (g.hooks[0].command === legacy || (typeof g.hooks[0].command === 'string' &&
          /^cmd\.exe \/d \/c call "[^"]+\/node\.exe" "/i.test(g.hooks[0].command) &&
          g.hooks[0].command.endsWith(`" "${script}" ${status}`)));
      const timeout = status==='approval' ? 21605 : 3;
      if (install && (!groups.some(owned) || groups.some(g=>owned(g) && (g.hooks[0].command!==command || g.hooks[0].timeout!==timeout)))) {
        config.hooks[event] = [...groups.filter(g=>!owned(g)), { ...(matcher && { matcher }),
          hooks: [{ type: 'command', command, timeout, statusMessage: 'Daozai' }] }];
      } else if (!install && groups.some(owned)) {
        config.hooks[event] = groups.filter(g => !owned(g));
        if (!config.hooks[event].length) delete config.hooks[event];
      }
    }
    if (JSON.stringify(config) !== initial) { fs.mkdirSync(dir,{recursive:true}); save(file,before,config); }
    return;
  }
  for (const value of [config.dependencies, config.dsh, config.dsh?.profile]) {
    if (value !== undefined && !object(value)) throw Error('Unsupported DSH profile.');
  }
  const bundles = config.dsh?.profile?.bundles ?? [];
  if (!Array.isArray(bundles) || !bundles.every(x => typeof x === 'string')) throw Error('Unsupported DSH bundles.');
  const link = path.join(dir,'node_modules',...pkg.split('/'));
  const stat = fs.lstatSync(link,{throwIfNoEntry:false});
  const linked = stat?.isSymbolicLink() ? path.resolve(path.dirname(link),fs.readlinkSync(link)) : null;
  // 指向「别的岛仔数据目录导出的同一套 kit」的旧链接视为可自愈的残留：
  // 用户换过数据目录/换过机器时，旧链接会让每次安装都 exit 4，表现就是「DSH 来源开着但岛上没有交互卡片」。
  const stale = !!linked && !samePath(linked,bridge) && ownKit(linked);
  if (stat && (linked === null || (!samePath(linked,bridge) && !stale)))
    throw failure('Another DeepSeek Harness package occupies the target. Nothing was replaced.',4);
  const spec = 'link:' + bridge.replaceAll('\\','/');
  const dep = config.dependencies?.[pkg];
  if (dep !== undefined && dep !== spec && !ownLinkSpec(dep)) throw failure('Another DeepSeek Harness dependency uses this name.',4);
  if (!install && dep === undefined && !stat) return;
  let created = false;
  try {
    if (install) {
      (config.dependencies ??= {})[pkg] = spec;
      ((config.dsh ??= {}).profile ??= {}).bundles = bundles.includes(pkg) ? bundles : [...bundles,pkg];
      if (stale) fs.unlinkSync(link);
      if (!stat || stale) {
        fs.mkdirSync(path.dirname(link), {recursive:true});
        fs.symlinkSync(bridge,link,'junction'); created = true;
      }
    } else {
      if (config.dependencies) delete config.dependencies[pkg];
      if (bundles.includes(pkg)) config.dsh.profile.bundles = bundles.filter(x=>x!==pkg);
    }
    if (JSON.stringify(config) !== initial) save(file,before,config);
  } catch (e) { if (created) fs.unlinkSync(link); throw e; }
  if (!install && stat) fs.unlinkSync(link);
}

try {
  const [provider, action, override, ...rest] = process.argv.slice(2);
  if (!['dsh','codex'].includes(provider) || !['install','remove'].includes(action) || rest.length)
    throw Error('Usage: node connect.mjs <codex|dsh> <install|remove> [profile-directory]');
  const dir = path.resolve(override || (provider === 'codex'
    ? process.env.CODEX_HOME || path.join(os.homedir(),'.codex')
    : path.join(process.env.DSH_HOME || path.join(os.homedir(),'.dsh'),'profiles','desktop')));
  connect(provider,action === 'install',dir);
  console.log(`${provider}: ${action} OK (${dir})`);
} catch (e) { console.error(e.message); process.exitCode = e.exitCode || 1; }
