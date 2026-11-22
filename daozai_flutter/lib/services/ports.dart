//! 可测试性抽象 —— 存储与通知接口，便于单元测试注入 fake。
//! 真实实现见 local_store.dart / notification_service.dart。
//! T1 置物架扩展：ShelfStore/BlobVault/DownloadExport/PeerTransport/Discovery/KeyVault/FileIntake。
import 'dart:typed_data';
import '../core/shelf_index.dart';
import 'local_store.dart';

/// 待办存储抽象。
abstract class TodoStore {
  Future<List<TodoItem>> loadTodos();
  Future<void> saveTodos(List<TodoItem> items);
}

/// 系统通知抽象。
abstract class ReminderNotifier {
  Future<void> init();
  Future<bool> schedule(int id, String text, DateTime when);
  Future<void> cancel(int id);
}

/// 时钟抽象：测试可注入固定/可控时间，验证提醒触发等时间相关逻辑。
typedef Clock = DateTime Function();
DateTime defaultClock() => DateTime.now();

// ================= T1 置物架端口 =================

/// 置物架索引存储。
abstract class ShelfStore {
  Future<List<ShelfEntry>> loadEntries();
  Future<void> upsert(ShelfEntry e); // 按 sha256+relPath 幂等
  Future<void> remove(String entryId);
  Future<void> clearTrash(DateTime before); // 回收站超期清理（T3）
}

/// 加密内容仓库：只认 sha256，不认路径。
abstract class BlobVault {
  Future<bool> has(String sha256);
  Future<void> put(String sha256, Stream<List<int>> plain); // 加密落盘，临时名+原子改名
  Future<Stream<List<int>>> get(String sha256); // 解密流出
  Future<void> delete(String sha256);
  Future<int> usedBytes();
  Future<void> sweepTemp(); // 启动清扫半文件残留
}

/// 明文导出层（D5）：download/ 目录管理，按 relPath 还原结构；支持自定义目录。
abstract class DownloadExport {
  Future<String> export(String sha256, String relPath); // vault→解密→baseDir/relPath，返回实际路径
  Future<String> ensureExported(String sha256, String relPath); // 缺失重导（验收 15）
  Future<void> remove(String relPath);
  Future<String> baseDir(); // 当前生效目录（默认或自定义）
  Future<bool> writable(); // 不可写时回落默认
  Future<String?> pickDirectory(); // 系统目录选择器，返回选中路径/bookmark/URI
  Future<void> setBaseDir(String picked); // 持久化；校验可写，失败抛错不落地
  Future<void> resetToDefault();
}

/// 设备间可靠消息通道（MVP=局域网 TCP；T4 可换实现）。
abstract class PeerTransport {
  Future<void> listen(int port);
  int? get boundPort;
  Future<PeerLink> connect(String host, int port, {String? pairToken});
  Stream<PeerLink> get incoming;
  PairingSessionView startPairing(DateTime now);
  Future<void> close();
}

abstract class PeerLink {
  String get remoteFingerprint; // 握手验证后的对端公钥指纹
  String get remoteName; // auth 帧交换的设备名
  bool get isPairing; // 本连接是否处于配对会话（SAS 未确认）
  set isPairing(bool v); // 配对完成后由引擎转正常态
  String get sas; // 配对验证码（两端一致才可信）
  PairingSessionView? get pairingSession; // advertising 端的会话（join 端为 null）
  Uint8List get peerIdentityPub; // 对端 Ed25519 公钥（建立信任用）
  Uint8List get peerStaticXPub; // 对端 X25519 静态公钥
  Stream<Uint8List> get frames; // 已解密的完整帧（含 5B 头）
  void send(Uint8List frame); // 自动加密分帧
  Future<void> close();
}

/// 配对会话只读视图（避免 ports 依赖 pairing 实现细节）。
abstract class PairingSessionView {
  String get token;
  String? get sas;
  String? get remoteName;
  String? get remoteFingerprint;
  String? get failReason;
  bool get isFailed;
  bool get isConfirmed;
  bool get fullyConfirmed;
}

/// 发现层（T1 手动 IP 无发现；T2 bonsoir）。
abstract class Discovery {
  Future<void> advertise(String fingerprint, int port);
  Stream<DiscoveredPeer> get peers;
  Future<void> stop();
}

class DiscoveredPeer {
  final String fingerprint;
  final String host;
  final int port;
  DiscoveredPeer(this.fingerprint, this.host, this.port);
}

/// 设备身份与信任表。
class DeviceIdentity {
  final Uint8List ed25519Private;
  final Uint8List ed25519Public;
  final Uint8List x25519Private;
  final Uint8List x25519Public;
  final String fingerprint; // SHA-256(ed25519Public) 前 8 字节 hex
  DeviceIdentity({
    required this.ed25519Private,
    required this.ed25519Public,
    required this.x25519Private,
    required this.x25519Public,
    required this.fingerprint,
  });
}

class TrustedPeer {
  final String fingerprint;
  final String name;
  final Uint8List ed25519Public;
  final Uint8List x25519Public;
  DateTime lastSeen;
  TrustedPeer({
    required this.fingerprint,
    required this.name,
    required this.ed25519Public,
    required this.x25519Public,
    DateTime? lastSeen,
  }) : lastSeen = lastSeen ?? DateTime.fromMillisecondsSinceEpoch(0);
}

/// 密钥安全存储。
abstract class KeyVault {
  Future<DeviceIdentity> loadOrCreate();
  Future<void> saveTrusted(TrustedPeer p);
  Future<List<TrustedPeer>> trustedPeers();
  Future<void> removeTrusted(String fingerprint);
  Future<Uint8List> vaultMasterKey(); // 落盘加密主密钥（首次随机生成）
  Future<String> deviceName();
  Future<void> setDeviceName(String name);
}

/// 拖入/选中的文件批。
class DroppedBatch {
  final List<String> paths; // 绝对路径
  DroppedBatch(this.paths);
}

class PickedFile {
  final String path;
  final String name;
  PickedFile(this.path, this.name);
}

/// 文件进入口（桌面拖放 / 安卓 SAF 统一抽象）。
abstract class FileIntake {
  Stream<DroppedBatch> get drops;
  Future<List<PickedFile>> pick();
}