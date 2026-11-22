window.__ModuleLoader__.load({
  id: '@daozai/agent-notices',
  factory() {
    // 安装时由岛仔把真实 endpoint（127.0.0.1 + 随机 token）替换进来。
    // 留空表示这次安装没带上导航通道：插件静默不轮询，不制造无意义的本地请求。
    const endpoint = '__DAOZAI_NAV_ENDPOINT__';
    return {
      name: 'daozai-session-navigation',
      // uiWorkspace：精确切会话（openSession）；sessions：确认会话还在。
      inject: ['uiWorkspace', 'sessions'],
      apply(ctx) {
        if (!endpoint || endpoint.startsWith('__')) return;
        let disposed = false;
        let timer = null;
        let active = false;
        const ack = async (requestId, status) => {
          try {
            await fetch(endpoint, {
              method: 'POST',
              headers: { 'content-type': 'application/json' },
              body: JSON.stringify({ requestId, status }),
              cache: 'no-store',
            });
          } catch { /* 岛仔会自行过期这条请求，不阻塞下一次轮询。 */ }
        };
        const poll = async () => {
          if (disposed || active) return;
          active = true;
          try {
            const response = await fetch(endpoint, { cache: 'no-store' });
            if (response.status === 200) {
              const request = await response.json();
              if (request?.protocol === 1 && typeof request.requestId === 'string' && typeof request.sessionId === 'string') {
                const known = ctx.sessions.list.getSnapshot().byId[request.sessionId];
                if (!known) {
                  await ack(request.requestId, 'expired');
                } else {
                  try {
                    ctx.uiWorkspace.openSession(request.sessionId);
                    await ack(request.requestId, 'navigated');
                  } catch {
                    await ack(request.requestId, 'failed');
                  }
                }
              }
            }
          } catch {
            // DSH 可能比岛仔活得久，也可能比岛仔晚起来：继续轮询即可。
          } finally {
            active = false;
            if (!disposed) timer = window.setTimeout(poll, 350);
          }
        };
        ctx.effect(() => {
          timer = window.setTimeout(poll, 100);
          return () => {
            disposed = true;
            if (timer !== null) window.clearTimeout(timer);
          };
        }, 'daozai: exact Session navigation');
      },
    };
  },
});
