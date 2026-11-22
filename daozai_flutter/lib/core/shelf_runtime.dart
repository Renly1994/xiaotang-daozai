//! 应用装配 —— 真实实现的组装根（main 调用；测试用 fake 注入各端口）。
//! 存储目录：应用支持目录/daozai_shelf/{vault,shelf.db}；download 默认见 D5。
import 'dart:io';
import 'package:path_provider/path_provider.dart';
import '../services/download_export.dart';
import '../services/file_intake.dart';
import '../services/file_vault.dart';
import '../services/lan_transport.dart';
import '../services/secure_keys.dart';
import '../services/sqlite_store.dart';
import '../services/sync_engine.dart';
import '../state/sync_state.dart';

class ShelfRuntime {
  final SyncState state;
  final SyncEngine engine;
  final ShelfIntake intake;
  final LanTransport transport;
  final SqliteShelfStore store;
  final int port;

  ShelfRuntime._(this.state, this.engine, this.intake, this.transport, this.store, this.port);

  static const defaultPort = 47821;

  static Future<ShelfRuntime> create({int port = defaultPort}) async {
    SqliteShelfStore.initFfi();
    final support = await getApplicationSupportDirectory();
    final root = Directory('${support.path}/daozai_shelf');
    await root.create(recursive: true);
    final keys = SecureKeys();
    final vault = FileVault(dir: '${root.path}/vault', masterKey: keys.vaultMasterKey);
    final store = SqliteShelfStore(dbPath: '${root.path}/shelf.db', masterKey: keys.vaultMasterKey);
    final download = DownloadExportImpl(vault: vault, directoryPicker: systemDirectoryPicker);
    final transport = LanTransport(keys: keys);
    // 端口被占（双实例演示/旧进程残留）→ 顺延尝试，设备页展示实际端口。
    var actualPort = port;
    for (var i = 0; i < 10; i++) {
      try {
        await transport.listen(actualPort);
        break;
      } catch (_) {
        actualPort++;
        if (i == 9) rethrow;
      }
    }
    final state = SyncState();
    final engine = SyncEngine(
      store: store, vault: vault, download: download, keys: keys,
      transport: transport, events: state,
    );
    await engine.start();
    state.bind(engine);
    state.setDownloadDir(await download.baseDir());
    state.setListenInfo(await _localIp(), transport.boundPort ?? actualPort);
    final intake = ShelfIntake(vault: vault, engine: engine, state: state);
    return ShelfRuntime._(state, engine, intake, transport, store, actualPort);
  }

  static Future<String?> _localIp() async {
    try {
      final interfaces = await NetworkInterface.list(type: InternetAddressType.IPv4);
      for (final i in interfaces) {
        for (final a in i.addresses) {
          if (!a.isLoopback) return a.address;
        }
      }
    } catch (_) {}
    return null;
  }

  Future<void> dispose() async {
    await engine.dispose();
    await store.close();
    state.dispose();
  }
}