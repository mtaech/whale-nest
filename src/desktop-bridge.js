/**
 * WhaleNest Desktop Bridge
 * 
 * 1. 网址点击拦截：自动将非本地外部链接与 window.open 重定向至系统默认浏览器打开
 * 2. 剪贴板图片粘贴：捕获 Ctrl+V / Paste 事件与系统原生剪贴板，自动保存为工作区附件并插入 @.dsh/attachments/ 引用
 * 3. 拖放图片支持：拖拽图片进入窗口高亮提示，松开自动保存并插入引用
 * 4. 质感浮动提示：显示附件已保存通知与缩略图
 */
(() => {
  if (window.__WHALENEST_BRIDGE_LOADED__) return;
  window.__WHALENEST_BRIDGE_LOADED__ = true;

  // ── 0. Tauri IPC 通信封装 ──────────────────────────────────────────────────
  async function callTauri(cmd, args = {}) {
    if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke) {
      return await window.__TAURI__.core.invoke(cmd, args);
    }
    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
      return await window.__TAURI_INTERNALS__.invoke(cmd, args);
    }
    throw new Error("Tauri IPC 接口不可用");
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

  // ── 2. 输入框文本注入（兼容 React 受控组件） ─────────────────────────────────
  function insertTextToInput(textToInsert) {
    // 优先寻找未禁用的 textarea
    let ta = document.querySelector("textarea:not([disabled])");
    if (document.activeElement && document.activeElement.tagName === "TEXTAREA" && !document.activeElement.disabled) {
      ta = document.activeElement;
    }
    if (!ta) {
      console.warn("[WhaleNest] 未找到可用输入框注入文本");
      return false;
    }

    ta.focus();

    // 方案 1: 尝试 document.execCommand (保留撤销历史)
    let inserted = false;
    try {
      inserted = document.execCommand("insertText", false, textToInsert);
    } catch {
      inserted = false;
    }

    // 方案 2: 使用 React 原型链 setter 并触发 input/change 事件
    if (!inserted) {
      const start = typeof ta.selectionStart === "number" ? ta.selectionStart : ta.value.length;
      const end = typeof ta.selectionEnd === "number" ? ta.selectionEnd : ta.value.length;
      const original = ta.value || "";
      const next = original.slice(0, start) + textToInsert + original.slice(end);

      const prototype = Object.getPrototypeOf(ta);
      const descriptor = Object.getOwnPropertyDescriptor(prototype, "value")
        || Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value");

      if (descriptor && descriptor.set) {
        descriptor.set.call(ta, next);
      } else {
        ta.value = next;
      }

      const newPos = start + textToInsert.length;
      ta.selectionStart = newPos;
      ta.selectionEnd = newPos;

      ta.dispatchEvent(new Event("input", { bubbles: true, cancelable: true }));
      ta.dispatchEvent(new Event("change", { bubbles: true, cancelable: true }));
    }

    return true;
  }

  // ── 3. 质感浮动提示 (WhaleNest Toast) ─────────────────────────────────────────
  function showToast({ title, subtitle, thumbnail, relativePath }) {
    let container = document.getElementById("whalenest-toast-container");
    if (!container) {
      container = document.createElement("div");
      container.id = "whalenest-toast-container";
      container.style.cssText = [
        "position: fixed",
        "top: 24px",
        "right: 24px",
        "z-index: 2147483647",
        "display: flex",
        "flex-direction: column",
        "gap: 10px",
        "pointer-events: none"
      ].join(";");
      (document.body || document.documentElement).appendChild(container);
    }

    const toast = document.createElement("div");
    toast.style.cssText = [
      "pointer-events: auto",
      "display: flex",
      "align-items: center",
      "gap: 12px",
      "background: rgba(15, 23, 42, 0.94)",
      "backdrop-filter: blur(16px)",
      "-webkit-backdrop-filter: blur(16px)",
      "border: 1px solid rgba(148, 163, 184, 0.25)",
      "border-radius: 10px",
      "padding: 10px 14px",
      "box-shadow: 0 12px 28px -6px rgba(0, 0, 0, 0.55), 0 0 0 1px rgba(255,255,255,0.06) inset",
      "color: #f8fafc",
      "font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'PingFang SC', 'Microsoft YaHei', sans-serif",
      "font-size: 13px",
      "line-height: 1.4",
      "min-width: 280px",
      "max-width: 420px",
      "opacity: 0",
      "transform: translateY(-12px) scale(0.96)",
      "transition: all 0.28s cubic-bezier(0.16, 1, 0.3, 1)"
    ].join(";");

    toast.innerHTML = `
      ${thumbnail
        ? `<img src="${thumbnail}" style="width:38px;height:38px;object-fit:cover;border-radius:6px;border:1px solid rgba(255,255,255,0.2);flex-shrink:0;" />`
        : `<div style="width:38px;height:38px;border-radius:6px;background:rgba(56,189,248,0.18);color:#38bdf8;display:flex;align-items:center;justify-content:center;font-size:18px;flex-shrink:0;">📸</div>`
      }
      <div style="flex:1;min-width:0;">
        <div style="font-weight:600;color:#38bdf8;margin-bottom:2px;display:flex;align-items:center;gap:6px;">
          <span>${title}</span>
        </div>
        <div style="color:#94a3b8;font-size:11px;font-family:ui-monospace,Menlo,Monaco,Consolas,monospace;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${subtitle || ""}</div>
      </div>
      ${relativePath ? `<button id="btn-open-dir" style="padding:3px 8px;border-radius:4px;background:#334155;color:#cbd5e1;border:none;cursor:pointer;font-size:11px;white-space:nowrap;margin-left:4px;">目录</button>` : ""}
      <button id="btn-close-toast" style="background:none;border:none;color:#64748b;cursor:pointer;padding:4px;font-size:14px;line-height:1;margin-left:2px;">✕</button>
    `;

    toast.querySelector("#btn-close-toast")?.addEventListener("click", () => {
      toast.style.opacity = "0";
      toast.style.transform = "translateY(-12px) scale(0.96)";
      setTimeout(() => toast.remove(), 280);
    });

    if (relativePath) {
      toast.querySelector("#btn-open-dir")?.addEventListener("click", () => {
        callTauri("open_attachment_folder", { relativePath }).catch(console.error);
      });
    }

    container.appendChild(toast);

    requestAnimationFrame(() => {
      toast.style.opacity = "1";
      toast.style.transform = "translateY(0) scale(1)";
    });

    setTimeout(() => {
      toast.style.opacity = "0";
      toast.style.transform = "translateY(-12px) scale(0.96)";
      setTimeout(() => toast.remove(), 280);
    }, 3800);
  }

  // ── 4. 剪贴板图片粘贴（Ctrl+V / Paste 事件拦截与原生回退） ─────────────────────
  let isProcessingPaste = false;

  window.addEventListener("paste", async (e) => {
    if (isProcessingPaste) return;

    const items = e.clipboardData ? Array.from(e.clipboardData.items) : [];
    const imageItem = items.find(it => it.type && it.type.startsWith("image/"));

    // 1. 网页标准剪贴板事件中直接包含了图片 Blob
    if (imageItem) {
      const file = imageItem.getAsFile();
      if (file) {
        e.preventDefault();
        e.stopPropagation();
        isProcessingPaste = true;

        try {
          const buffer = await file.arrayBuffer();
          const bytes = Array.from(new Uint8Array(buffer));
          const res = await callTauri("save_dropped_image", {
            fileName: file.name || "clipboard.png",
            pngBytes: bytes
          });

          if (res && res.success) {
            insertTextToInput(`${res.relative_path} `);
            showToast({
              title: "已粘贴图片并引用",
              subtitle: res.relative_path,
              thumbnail: URL.createObjectURL(file),
              relativePath: res.relative_path
            });
          }
        } catch (err) {
          console.error("[WhaleNest] 保存剪贴板图片失败:", err);
        } finally {
          isProcessingPaste = false;
        }
        return;
      }
    }

    // 2. 如果标准事件中无纯文本（如 Windows 截图 CF_DIB 导致标准 paste 事件为空），调用原生 Rust 剪贴板
    const types = e.clipboardData ? Array.from(e.clipboardData.types) : [];
    const plainText = e.clipboardData ? e.clipboardData.getData("text/plain") : "";
    const hasText = types.includes("text/plain") && plainText && plainText.trim().length > 0;

    if (!hasText) {
      try {
        isProcessingPaste = true;
        const res = await callTauri("save_clipboard_image");
        if (res && res.success) {
          e.preventDefault();
          e.stopPropagation();
          insertTextToInput(`${res.relative_path} `);
          showToast({
            title: "已从剪贴板提取图片",
            subtitle: res.relative_path,
            relativePath: res.relative_path
          });
        }
      } catch (err) {
        // 剪贴板无图片时静默忽略，不影响后续输入
      } finally {
        isProcessingPaste = false;
      }
    }
  }, true);

  // ── 5. 文件拖放支持 (Drag & Drop) ─────────────────────────────────────────────
  let dragCounter = 0;
  const dropOverlay = document.createElement("div");
  dropOverlay.id = "whalenest-drop-overlay";
  dropOverlay.style.cssText = [
    "position: fixed",
    "inset: 12px",
    "background: rgba(14, 165, 233, 0.08)",
    "border: 2px dashed #0284c7",
    "border-radius: 14px",
    "z-index: 2147483646",
    "display: none",
    "flex-direction: column",
    "align-items: center",
    "justify-content: center",
    "gap: 12px",
    "color: #0284c7",
    "backdrop-filter: blur(6px)",
    "-webkit-backdrop-filter: blur(6px)",
    "pointer-events: none",
    "transition: all 0.2s ease"
  ].join(";");
  dropOverlay.innerHTML = `
    <div style="width:56px;height:56px;border-radius:14px;background:rgba(2,132,199,0.12);display:flex;align-items:center;justify-content:center;">
      <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="#0284c7" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="3" width="18" height="18" rx="2" ry="2"/>
        <circle cx="8.5" cy="8.5" r="1.5"/>
        <polyline points="21 15 16 10 5 21"/>
      </svg>
    </div>
    <div style="font-size: 16px; font-weight: 600; color: #0369a1;">松开鼠标以添加图片附件</div>
    <div style="font-size: 12px; color: #64748b;">将自动保存至当前工作区 .dsh/attachments/ 并插入引用</div>
  `;

  function ensureDropOverlay() {
    if (!document.body) return;
    if (!document.getElementById("whalenest-drop-overlay")) {
      document.body.appendChild(dropOverlay);
    }
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", ensureDropOverlay);
  } else {
    ensureDropOverlay();
  }

  window.addEventListener("dragenter", (e) => {
    if (e.dataTransfer && Array.from(e.dataTransfer.types).includes("Files")) {
      dragCounter++;
      ensureDropOverlay();
      dropOverlay.style.display = "flex";
    }
  }, true);

  window.addEventListener("dragleave", () => {
    dragCounter--;
    if (dragCounter <= 0) {
      dragCounter = 0;
      dropOverlay.style.display = "none";
    }
  }, true);

  window.addEventListener("dragover", (e) => {
    if (e.dataTransfer && Array.from(e.dataTransfer.types).includes("Files")) {
      e.preventDefault();
    }
  }, true);

  window.addEventListener("drop", async (e) => {
    dragCounter = 0;
    dropOverlay.style.display = "none";

    const files = e.dataTransfer ? Array.from(e.dataTransfer.files) : [];
    const imageFiles = files.filter(f => f.type.startsWith("image/") || /\.(png|jpe?g|webp|gif|svg|bmp)$/i.test(f.name));

    if (imageFiles.length > 0) {
      e.preventDefault();
      e.stopPropagation();

      for (const file of imageFiles) {
        try {
          const buffer = await file.arrayBuffer();
          const bytes = Array.from(new Uint8Array(buffer));
          const res = await callTauri("save_dropped_image", {
            fileName: file.name,
            pngBytes: bytes
          });

          if (res && res.success) {
            insertTextToInput(`${res.relative_path} `);
            showToast({
              title: "已添加图片附件",
              subtitle: res.relative_path,
              thumbnail: URL.createObjectURL(file),
              relativePath: res.relative_path
            });
          }
        } catch (err) {
          console.error("[WhaleNest] 保存拖入图片失败:", err);
        }
      }
    }
  }, true);

  console.log("[WhaleNest] 桌面端增强桥接 (链接直开外部浏览器 + 剪贴板图片/拖放) 已就绪");
})();
