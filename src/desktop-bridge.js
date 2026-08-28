/**
 * WhaleNest Desktop Bridge
 *
 * 桌面端只补 WebView 缺的那一块，不碰 dsh 自己能做好的事：
 *
 * 1. 外链拦截：非本地链接与 window.open 交给系统默认浏览器。
 * 2. 剪贴板图片兜底：仅当 clipboardData 里既没有图片也没有文本时（Windows
 *    截图走 CF_DIB，标准 paste 事件为空），才用原生剪贴板取字节，合成一个
 * 正常的 paste 事件派发给输入框。
 *
 * 刻意不做的事：
 * - 不拦截「clipboardData 里已有图片」的粘贴 —— dsh 前端自己就会把它收进
 *   附件轨（onPaste → intakeImages → ComposerAttachments），出缩略图、走内容
 *   寻址存储（.dsh/attachments/v1/objects）、带尺寸与数量校验。我们抢过来只会
 *退化成一行文本路径，还在 .dsh/attachments 顶层堆下不去重、无人回收的文件。
 * - 不做拖放 —— dsh 的 ComposerAttachments 已在 document 上挂了完整的
 *   dragenter/dragover/dragleave/drop 与 DropOverlay，含拖拽深度计数。
 */
(() => {
  if (window.__WHALENEST_BRIDGE_LOADED__) return;
  window.__WHALENEST_BRIDGE_LOADED__ = true;

  // ── 0. Tauri IPC 通信封装 ──────────────────────────────────────────────────
  //
  // 本脚本注入到「所有帧」：壳页（tauri:// 本地源）与承载 dsh 的 iframe 子帧
  // （http://127.0.0.1:<port> 远程源）都会执行。
  //
  // Tauri v2 的 ACL 只对本地源放行自定义命令；远程源在没有显式 remote
  // capability 时一律被拒绝。所以子帧不直接 invoke，而是用 postMessage 把请求
  // 中继给壳页，由壳页（本地源）代为执行后回传结果。
  const IS_TOP_FRAME = (() => {
    try {
      return window.top === window.self;
    } catch {
      return false; // 跨源读取 window.top 抛错，说明自己在子帧里
    }
  })();

  const RELAY_TAG = "whalenest-ipc";
  let relaySeq = 0;
  const relayPending = new Map();

  function invokeDirect(cmd, args) {
    if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke) {
      return window.__TAURI__.core.invoke(cmd, args);
    }
    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
      return window.__TAURI_INTERNALS__.invoke(cmd, args);
    }
    return Promise.reject(new Error("Tauri IPC 接口不可用"));
  }

  function invokeViaShell(cmd, args) {
    return new Promise((resolve, reject) => {
      const id = String(Date.now()) + "-" + String(++relaySeq);
      const timer = setTimeout(() => {
        relayPending.delete(id);
        reject(new Error("IPC 中继超时: " + cmd));
      }, 20000);
      relayPending.set(id, { resolve, reject, timer });
      window.parent.postMessage({ tag: RELAY_TAG, kind: "request", id, cmd, args }, "*");
    });
  }

  // 子帧侧：接收壳页回传的执行结果
  if (!IS_TOP_FRAME) {
    window.addEventListener("message", (event) => {
      const data = event.data;
      if (!data || data.tag !== RELAY_TAG || data.kind !== "response") return;
      const entry = relayPending.get(data.id);
      if (!entry) return;
      relayPending.delete(data.id);
      clearTimeout(entry.timer);
      if (data.ok) {
        entry.resolve(data.result);
      } else {
        entry.reject(new Error(data.error || "IPC 中继失败"));
      }
    });
  }

  async function callTauri(cmd, args = {}) {
    if (IS_TOP_FRAME) {
      return await invokeDirect(cmd, args);
    }
    return await invokeViaShell(cmd, args);
  }

  // ── 1. 网址点击拦截（系统默认浏览器打开） ────────────────────────────────────
  function isExternalUrl(href) {
    if (!href) return false;
    const trimmed = String(href).trim();
    if (!trimmed.startsWith("http://") && !trimmed.startsWith("https://") && !trimmed.startsWith("mailto:")) {
      return false;
    }
    try {
      const url = new URL(trimmed, window.location.href);
      const currentHost = window.location.hostname;
      const currentPort = window.location.port;

      // 如果是当前 dsh 本地服务端口或 tauri 内部协议，则不算外部链接
      if ((url.hostname === "127.0.0.1" || url.hostname === "localhost") && url.port === currentPort) {
        return false;
      }
      return true;
    } catch {
      return false;
    }
  }

  function openInSystemBrowser(url) {
    callTauri("open_external_url", { url }).catch((err) => {
      console.error("[WhaleNest] 打开外部网址失败:", err);
    });
  }

  // 捕获全局所有 <a> 标签点击（左键、中键、Ctrl+点击）
  document.addEventListener("click", (event) => {
    const anchor = event.target && event.target.closest ? event.target.closest("a") : null;
    if (!anchor || !anchor.href) return;
    if (isExternalUrl(anchor.href)) {
      event.preventDefault();
      event.stopPropagation();
      openInSystemBrowser(anchor.href);
    }
  }, true);

  document.addEventListener("auxclick", (event) => {
    if (event.button !== 1) return; // 仅处理中键滚轮点击
    const anchor = event.target && event.target.closest ? event.target.closest("a") : null;
    if (!anchor || !anchor.href) return;
    if (isExternalUrl(anchor.href)) {
      event.preventDefault();
      event.stopPropagation();
      openInSystemBrowser(anchor.href);
    }
  }, true);

  // 拦截 window.open 调用
  const originalWindowOpen = window.open;
  window.open = function (url, target, features) {
    if (typeof url === "string" && isExternalUrl(url)) {
      openInSystemBrowser(url);
      return null;
    }
    return originalWindowOpen.call(this, url, target, features);
  };


  // ── 2. 剪贴板图片兜底（仅补 clipboardData 为空的平台缺陷） ──────────────────
  //
  // Windows 截图工具常只放 CF_DIB，WebView2 的标准 paste 事件里 items 为空，
  // dsh 的 onPaste 拿不到任何 File。这时才由原生 arboard 取 PNG 字节，合成
  // DataTransfer 再派发一个真正的 paste 事件 —— 让 dsh 走它自己那条路，
  // 该出缩略图就出缩略图，附件也进它的内容寻址库，而不是我们另开一套账。
  let fallbackInFlight = false;

  function findComposer() {
    const active = document.activeElement;
    if (active instanceof HTMLTextAreaElement && !active.disabled) return active;
    return document.querySelector("textarea:not([disabled])");
  }

  /** 把 PNG 字节包成 File，合成 paste 事件交给 dsh 的 onPaste。 */
  function dispatchSyntheticPaste(target, bytes) {
    const file = new File([new Uint8Array(bytes)], `clipboard-${Date.now()}.png`, {
      type: "image/png",
    });
    const dt = new DataTransfer();
    dt.items.add(file);
    const event = new ClipboardEvent("paste", {
      clipboardData: dt,
      bubbles: true,
      cancelable: true,
    });
    // clipboardData 是只读的，某些引擎会忽略构造参数；此时补一个可读属性。
    if (event.clipboardData !== dt) {
      Object.defineProperty(event, "clipboardData", { value: dt, configurable: true });
    }
    target.focus();
    return target.dispatchEvent(event);
  }

  window.addEventListener("paste", (e) => {
    if (fallbackInFlight) return;

    const cd = e.clipboardData;
    const types = cd ? Array.from(cd.types) : [];

    // clipboardData 已有图片 → 什么都不做，让 dsh 自己收进附件轨。
    const hasImage =
      types.includes("Files") ||
      (cd ? Array.from(cd.items).some((it) => it.type && it.type.startsWith("image/")) : false);
    if (hasImage) return;

    // 有文本 → 正常文本粘贴，也不插手。
    const text = cd ? cd.getData("text/plain") : "";
    if (text && text.trim().length > 0) return;

    // 既无图片也无文本：可能是 CF_DIB 那类缺陷，问一次原生剪贴板。
    // 注意这里不调 preventDefault —— 异步回调跑到时事件早已派发完，调了也
    // 无效；而这个分支本就没有默认行为可拦（剪贴板里既没图也没文本）。
    const composer = findComposer();
    if (!composer) return;

    fallbackInFlight = true;
    callTauri("read_clipboard_image_binary")
      .then((bytes) => {
        if (!bytes || bytes.length === 0) return; // 剪贴板确实没图，静默放过
        dispatchSyntheticPaste(composer, bytes);
      })
      .catch((err) => {
        console.warn("[WhaleNest] 原生剪贴板兜底失败:", err);
      })
      .finally(() => {
        fallbackInFlight = false;
      });
  }, true);

  console.log("[WhaleNest] 桌面桥接就绪（外链直开 + 剪贴板兜底；图片粘贴与拖放交由 dsh 原生处理）");
})();
