(() => {
  if (window.__WHALENEST_DEBUG_ATTACHED__) return;
  window.__WHALENEST_DEBUG_ATTACHED__ = true;

  const logs = [];
  function addLog(type, msg, data) {
    const time = new Date().toTimeString().slice(0, 8);
    const entry = { time, type, msg, data };
    logs.push(entry);
    console.log(`[WhaleNest Debug][${time}][${type}] ${msg}`, data !== undefined ? data : "");
    renderLog();
  }

  // Create UI element
  const root = document.createElement("div");
  root.id = "whalenest-debug-hud";
  root.style.cssText = [
    "position: fixed",
    "bottom: 12px",
    "right: 12px",
    "z-index: 2147483647",
    "font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
    "font-size: 11px",
    "color: #e2e8f0",
    "background: rgba(15, 23, 42, 0.94)",
    "border: 1px solid rgba(148, 163, 184, 0.35)",
    "border-radius: 8px",
    "box-shadow: 0 10px 25px -5px rgba(0,0,0,0.6)",
    "max-width: 520px",
    "width: min(92vw, 520px)",
    "user-select: text",
    "line-height: 1.4"
  ].join(";");

  let isCollapsed = true;
  root.innerHTML = `
    <div id="wn-hud-header" style="display:flex;align-items:center;justify-content:space-between;padding:8px 12px;cursor:pointer;border-bottom:1px solid rgba(148,163,184,0.2);">
      <span style="font-weight:600;display:flex;align-items:center;gap:6px;">
        <span style="display:inline-block;width:8px;height:8px;border-radius:50%;background:#38bdf8;"></span>
        📋 剪贴板 & 拖放日志排查器 (含原生剪贴板直通)
      </span>
      <div style="display:flex;gap:6px;">
        <button id="wn-hud-copy" style="padding:2px 8px;border-radius:4px;background:#334155;color:#fff;border:none;cursor:pointer;font-size:10px;">复制日志</button>
        <button id="wn-hud-toggle" style="padding:2px 8px;border-radius:4px;background:#334155;color:#fff;border:none;cursor:pointer;font-size:10px;">展开</button>
      </div>
    </div>
    <div id="wn-hud-body" style="display:none;padding:10px 12px;max-height:280px;overflow-y:auto;display:flex;flex-direction:column;gap:8px;">
      <div style="display:flex;gap:6px;">
        <button id="wn-test-clip" style="flex:1;padding:5px 8px;background:#2563eb;color:#fff;border:none;border-radius:4px;cursor:pointer;font-size:11px;font-weight:500;">主动读取原生剪贴板图片</button>
        <button id="wn-clear-log" style="padding:5px 8px;background:#475569;color:#fff;border:none;border-radius:4px;cursor:pointer;font-size:11px;">清空</button>
      </div>
      <div id="wn-hud-list" style="display:flex;flex-direction:column;gap:6px;max-height:200px;overflow-y:auto;">
        <div style="color:#94a3b8;">等待操作中... 尝试在此窗口内粘贴图片（Ctrl+V）或拖拽文件。</div>
      </div>
    </div>
  `;

  function mount() {
    if (document.body) {
      document.body.appendChild(root);
    } else {
      setTimeout(mount, 50);
    }
  }
  mount();

  const header = root.querySelector("#wn-hud-header");
  const body = root.querySelector("#wn-hud-body");
  const toggleBtn = root.querySelector("#wn-hud-toggle");
  const copyBtn = root.querySelector("#wn-hud-copy");
  const list = root.querySelector("#wn-hud-list");
  const testClipBtn = root.querySelector("#wn-test-clip");
  const clearBtn = root.querySelector("#wn-clear-log");

  function setCollapsed(col) {
    isCollapsed = col;
    body.style.display = isCollapsed ? "none" : "flex";
    toggleBtn.textContent = isCollapsed ? "展开" : "收起";
  }
  setCollapsed(true);

  header?.addEventListener("click", (e) => {
    if (e.target && e.target.tagName === "BUTTON") return;
    setCollapsed(!isCollapsed);
  });
  toggleBtn?.addEventListener("click", () => setCollapsed(!isCollapsed));

  copyBtn?.addEventListener("click", () => {
    const text = logs.map(l => `[${l.time}][${l.type}] ${l.msg} ${l.data !== undefined ? JSON.stringify(l.data, null, 2) : ""}`).join("\n");
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(() => {
        copyBtn.textContent = "已复制!";
        setTimeout(() => copyBtn.textContent = "复制日志", 1500);
      }).catch(() => {
        copyBtn.textContent = "复制失败";
      });
    }
  });

  clearBtn?.addEventListener("click", () => {
    logs.length = 0;
    renderLog();
  });

  async function callTauriInvoke(cmd, args = {}) {
    if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke) {
      return await window.__TAURI__.core.invoke(cmd, args);
    }
    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
      return await window.__TAURI_INTERNALS__.invoke(cmd, args);
    }
    throw new Error("Tauri IPC 接口不可用");
  }

  // Helper to inject File via Drop event into document / DSH Attachments
  function dispatchImageDrop(file) {
    const dt = new DataTransfer();
    dt.items.add(file);
    const enterEvt = new DragEvent("dragenter", { bubbles: true, cancelable: true, dataTransfer: dt });
    const overEvt = new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: dt });
    const dropEvt = new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: dt });

    document.dispatchEvent(enterEvt);
    document.dispatchEvent(overEvt);
    document.dispatchEvent(dropEvt);
  }

  // Active bridge: try fetching OS clipboard image if standard paste has no files
  async function tryNativeClipboardImageBridge() {
    try {
      addLog("BRIDGE_START", "正在尝试通过 Rust 原生层读取剪贴板图片...");
      const bytes = await callTauriInvoke("read_clipboard_image_binary");
      if (!bytes || bytes.length === 0) {
        addLog("BRIDGE_EMPTY", "系统剪贴板中当前没有检测到图片数据");
        return false;
      }
      const u8 = new Uint8Array(bytes);
      const blob = new Blob([u8], { type: "image/png" });
      const filename = "clipboard-" + Date.now() + ".png";
      const file = new File([blob], filename, { type: "image/png" });

      addLog("BRIDGE_SUCCESS", `从原生剪贴板成功提取图片 (${file.size} 字节)，正在注入到输入框...`, {
        name: filename,
        size: file.size,
        type: file.type
      });

      dispatchImageDrop(file);
      return true;
    } catch (err) {
      addLog("BRIDGE_ERR", `原生剪贴板读取失败: ${err && err.message ? err.message : String(err)}`);
      return false;
    }
  }

  testClipBtn?.addEventListener("click", async () => {
    await tryNativeClipboardImageBridge();
  });

  function renderLog() {
    if (!list) return;
    if (logs.length === 0) {
      list.innerHTML = `<div style="color:#94a3b8;">等待操作中... 尝试在此窗口内粘贴图片（Ctrl+V）或拖拽文件。</div>`;
      return;
    }
    list.innerHTML = logs.slice(-25).map(l => {
      const color = l.type.includes("ERR") ? "#f87171" : l.type.includes("PASTE") || l.type.includes("BRIDGE") ? "#38bdf8" : l.type.includes("DROP") ? "#4ade80" : "#cbd5e1";
      return `
        <div style="border-left:2px solid ${color};padding-left:6px;line-height:1.4;word-break:break-all;">
          <span style="color:#64748b;">${l.time}</span> <b style="color:${color};">[${l.type}]</b> ${l.msg}
          ${l.data !== undefined ? `<pre style="margin:2px 0 0 0;color:#94a3b8;font-size:10px;white-space:pre-wrap;">${JSON.stringify(l.data, null, 2)}</pre>` : ""}
        </div>
      `;
    }).join("");
    body.scrollTop = body.scrollHeight;
  }

  // 1. Paste Event Listener (Capture Phase)
  window.addEventListener("paste", (e) => {
    setCollapsed(false);
    const target = e.target ? `${e.target.tagName}${e.target.className ? '.' + String(e.target.className) : ''}` : "unknown";
    const types = e.clipboardData ? Array.from(e.clipboardData.types) : [];
    const items = e.clipboardData ? Array.from(e.clipboardData.items) : [];
    const files = e.clipboardData ? Array.from(e.clipboardData.files) : [];

    const itemDetails = items.map((it, idx) => {
      let fileInfo = null;
      if (it.kind === "file") {
        const f = it.getAsFile();
        fileInfo = f ? { name: f.name, size: f.size, type: f.type } : "getAsFile() returned null";
      }
      return { index: idx, kind: it.kind, type: it.type, file: fileInfo };
    });

    const hasFile = files.length > 0 || itemDetails.some(it => it.kind === "file" && it.file && typeof it.file === "object");

    addLog("PASTE_EVENT", `捕获 paste 事件 (目标元素: ${target})`, {
      types,
      itemsCount: items.length,
      itemDetails,
      filesCount: files.length,
      files: files.map(f => ({ name: f.name, size: f.size, type: f.type })),
      defaultPrevented: e.defaultPrevented
    });

    // If Webview failed to deliver clipboard image in standard event, trigger native bridge
    if (!hasFile) {
      tryNativeClipboardImageBridge();
    }
  }, true);

  // 2. Drag & Drop Listeners (Capture Phase)
  window.addEventListener("dragenter", (e) => {
    const types = e.dataTransfer ? Array.from(e.dataTransfer.types) : [];
    addLog("DRAG_ENTER", `文件拖入窗口`, { types });
  }, true);

  window.addEventListener("drop", (e) => {
    setCollapsed(false);
    const target = e.target ? `${e.target.tagName}${e.target.className ? '.' + String(e.target.className) : ''}` : "unknown";
    const types = e.dataTransfer ? Array.from(e.dataTransfer.types) : [];
    const files = e.dataTransfer ? Array.from(e.dataTransfer.files) : [];
    addLog("DROP_EVENT", `捕获 drop 事件 (目标元素: ${target})`, {
      types,
      filesCount: files.length,
      files: files.map(f => ({ name: f.name, size: f.size, type: f.type }))
    });
  }, true);

  // 3. Keydown Listener for Ctrl+V / Cmd+V
  window.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && (e.key === "v" || e.key === "V")) {
      const active = document.activeElement ? `${document.activeElement.tagName}${document.activeElement.className ? '.' + String(document.activeElement.className) : ''}` : "none";
      addLog("KEY_PASTE", `按键触发 ${e.ctrlKey ? 'Ctrl' : 'Cmd'}+V (当前聚焦: ${active})`);
    }
  }, true);

  addLog("INIT", "WhaleNest 调试排查器已挂载就绪 (含原生剪贴板直通桥接)");
})();
