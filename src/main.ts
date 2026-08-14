import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";

/**
 * WhaleNest 薄壳页。
 *
 * 与后端 IPC 契约（勿改事件/命令名）：
 *  - 事件通道 "kernel-status"，载荷 { status, url?, message? }
 *  - 命令 get_state / set_working_dir / restart_kernel / get_diagnostics /
 *    open_log_file / copy_diagnostics / quit
 */

type KernelStatus = "guide" | "starting" | "ready" | "error";

interface KernelStatusPayload {
  status: KernelStatus;
  url?: string;
  message?: string;
}

interface ShellState extends KernelStatusPayload {
  cwd: string;
  autostart: boolean;
}

interface UpdatePayload {
  current: string;
  latest: string;
  has_update: boolean;
}

type ViewName = "loading" | "error" | "guide";

const INSTALL_COMMAND = "npm i -g @deepseek-ai/dsh";

const views = new Map<ViewName, HTMLElement>();
for (const name of ["loading", "error", "guide"] as const) {
  const el = document.querySelector<HTMLElement>(`[data-view="${name}"]`);
  if (!el) {
    throw new Error(`[whalenest] missing view element: ${name}`);
  }
  views.set(name, el);
}

const errorMessageEl = document.querySelector<HTMLElement>("#error-message");
const btnRestart = document.querySelector<HTMLButtonElement>("#btn-restart");
const btnRedetect = document.querySelector<HTMLButtonElement>("#btn-redetect");
const btnCopy = document.querySelector<HTMLButtonElement>("#btn-copy-command");
const updateBanner = document.querySelector<HTMLElement>("#update-banner");
const updateBannerText = document.querySelector<HTMLElement>("#update-banner-text");
const btnUpdate = document.querySelector<HTMLButtonElement>("#btn-update");

function showView(name: ViewName): void {
  for (const [viewName, el] of views) {
    el.hidden = viewName !== name;
  }
}

function renderError(message?: string): void {
  const text = message?.trim() ? message.trim() : "未知错误";
  if (errorMessageEl) {
    errorMessageEl.textContent = text;
  }
  showView("error");
}

function render(payload: KernelStatusPayload): void {
  switch (payload.status) {
    case "ready":
      if (payload.url) {
        window.location.href = payload.url;
      } else {
        renderError("内核已就绪，但缺少访问地址");
      }
      break;
    case "error":
      renderError(payload.message);
      break;
    case "guide":
      showView("guide");
      break;
    case "starting":
      showView("loading");
      break;
  }
}

async function copyInstallCommand(): Promise<void> {
  let copied = false;
  try {
    await navigator.clipboard.writeText(INSTALL_COMMAND);
    copied = true;
  } catch {
    // WebView 无异步剪贴板权限时的降级方案。
    const ta = document.createElement("textarea");
    ta.value = INSTALL_COMMAND;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.append(ta);
    ta.select();
    try {
      copied = document.execCommand("copy");
    } catch {
      copied = false;
    }
    ta.remove();
  }
  if (copied && btnCopy) {
    btnCopy.textContent = "已复制";
  }
}

function renderUpdate(payload: UpdatePayload): void {
  if (!updateBanner || !updateBannerText) {
    return;
  }
  if (payload.has_update) {
    updateBannerText.textContent = `发现新版本 dsh v${payload.latest}（当前 v${payload.current}）`;
    updateBanner.hidden = false;
  } else {
    updateBanner.hidden = true;
  }
}

async function init(): Promise<void> {
  btnRestart?.addEventListener("click", () => {
    void invoke("restart_kernel");
  });
  btnRedetect?.addEventListener("click", () => {
    void invoke("restart_kernel");
  });
  btnCopy?.addEventListener("click", () => {
    void copyInstallCommand();
  });
  btnUpdate?.addEventListener("click", () => {
    if (btnUpdate) {
      btnUpdate.disabled = true;
      btnUpdate.textContent = "更新中…";
    }
    void invoke("install_update");
  });

  try {
    await listen<KernelStatusPayload>("kernel-status", (event) => {
      render(event.payload);
    });
    await listen<UpdatePayload>("dsh-update", (event) => {
      renderUpdate(event.payload);
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    renderError(`无法订阅内核状态：${message}`);
    return;
  }

  try {
    const state = await invoke<ShellState>("get_state");
    render(state);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    renderError(message);
  }
}

void init();
