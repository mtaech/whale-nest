import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./styles.css";
import "./desktop-bridge.js";

/**
 * WhaleNest 桌面薄壳页（支持首次 Step 向导模式）
 *
 * 与后端 IPC 契约：
 *  - 事件通道 "kernel-status", "dsh-update", "dsh-installed", "plugin-installed"
 *  - 命令 get_state / set_working_dir / restart_kernel / get_diagnostics /
 *    check_env / get_recommended_plugins / install_plugin / complete_setup /
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
  initialized?: boolean;
  is_attached?: boolean;
}

interface UpdatePayload {
  current: string;
  latest: string;
  has_update: boolean;
}

interface ToolInfo {
  name: string;
  found: boolean;
  version?: string;
  path?: string;
}

interface EnvCheckResult {
  node: ToolInfo;
  npm: ToolInfo;
  pnpm: ToolInfo;
  dsh: ToolInfo;
  all_passed: boolean;
}

interface RepoPluginItem {
  id: string;
  name: string;
  package_name: string;
  description: string;
  installed: boolean;
  version?: string;
  category: string;
}

type ViewName = "loading" | "error" | "guide" | "wizard";

const INSTALL_COMMAND = "npm i -g @deepseek-ai/dsh";

const views = new Map<ViewName, HTMLElement>();
for (const name of ["loading", "error", "guide", "wizard"] as const) {
  const el = document.querySelector<HTMLElement>(`[data-view="${name}"]`);
  if (!el) {
    throw new Error(`[whalenest] missing view element: ${name}`);
  }
  views.set(name, el);
}

// 错误视图元素
const errorMessageEl = document.querySelector<HTMLElement>("#error-message");
const btnRestart = document.querySelector<HTMLButtonElement>("#btn-restart");

// 引导视图元素（备用）
const btnRedetect = document.querySelector<HTMLButtonElement>("#btn-redetect");
const btnCopy = document.querySelector<HTMLButtonElement>("#btn-copy-command");
const btnInstallDsh = document.querySelector<HTMLButtonElement>("#btn-install-dsh");

// 更新提示条
const updateBanner = document.querySelector<HTMLElement>("#update-banner");
const updateBannerText = document.querySelector<HTMLElement>("#update-banner-text");
const btnUpdate = document.querySelector<HTMLButtonElement>("#btn-update");

// 宿主与标题栏
const shellEl = document.querySelector<HTMLElement>("#shell");
const hostEl = document.querySelector<HTMLElement>("#view-host");
const dshFrame = document.querySelector<HTMLIFrameElement>("#dsh-frame");
const tbStatus = document.querySelector<HTMLElement>("#tb-status");
const titlebarEl = document.querySelector<HTMLElement>("#titlebar");
const btnMin = document.querySelector<HTMLButtonElement>("#btn-min");
const btnMax = document.querySelector<HTMLButtonElement>("#btn-max");
const btnClose = document.querySelector<HTMLButtonElement>("#btn-close");

// 向导元素（Step 模式）
const stepNav1 = document.querySelector<HTMLElement>("#step-nav-1");
const stepNav2 = document.querySelector<HTMLElement>("#step-nav-2");
const wizardStep1 = document.querySelector<HTMLElement>("#wizard-step-1");
const wizardStep2 = document.querySelector<HTMLElement>("#wizard-step-2");
const envCheckListEl = document.querySelector<HTMLElement>("#env-check-list");
const dshGuideBoxEl = document.querySelector<HTMLElement>("#dsh-guide-box");
const btnCopyWizardCmd = document.querySelector<HTMLButtonElement>("#btn-copy-wizard-cmd");
const btnWizardInstallDsh = document.querySelector<HTMLButtonElement>("#btn-wizard-install-dsh");
const btnWizardRecheck = document.querySelector<HTMLButtonElement>("#btn-wizard-recheck");
const btnStep1Recheck = document.querySelector<HTMLButtonElement>("#btn-step1-recheck");
const btnStep1Next = document.querySelector<HTMLButtonElement>("#btn-step1-next");
const pluginsGridEl = document.querySelector<HTMLElement>("#plugins-recommend-list");
const btnStep2Prev = document.querySelector<HTMLButtonElement>("#btn-step2-prev");
const btnStep2Finish = document.querySelector<HTMLButtonElement>("#btn-step2-finish");

let currentStep = 1;
let recommendedPlugins: RepoPluginItem[] = [];
let isInitialized = true;

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

function hideHost(): void {
  if (hostEl) {
    hostEl.hidden = true;
  }
  if (shellEl) {
    shellEl.hidden = false;
  }
  if (dshFrame && dshFrame.src && !dshFrame.src.endsWith("about:blank")) {
    dshFrame.src = "about:blank";
  }
}

function render(payload: ShellState | KernelStatusPayload): void {
  if ("initialized" in payload && payload.initialized !== undefined) {
    isInitialized = payload.initialized;
  }

  // 如果尚未完成首次初始化，优先进入 Step 向导模式
  if (!isInitialized) {
    hideHost();
    showView("wizard");
    setWizardStep(currentStep);
    return;
  }

  if (tbStatus) {
    tbStatus.classList.toggle("is-ready", payload.status === "ready");
  }

  switch (payload.status) {
    case "ready":
      if (payload.url) {
        window.location.replace(payload.url);
      } else {
        renderError("内核已就绪，但缺少访问地址");
      }
      break;
    case "error":
      hideHost();
      renderError(payload.message);
      break;
    case "guide":
      hideHost();
      showView("guide");
      break;
    case "starting":
      hideHost();
      showView("loading");
      break;
  }
}

function setWizardStep(step: number): void {
  currentStep = step;
  if (stepNav1 && stepNav2 && wizardStep1 && wizardStep2) {
    if (step === 1) {
      stepNav1.classList.add("is-active");
      stepNav1.classList.remove("is-passed");
      stepNav2.classList.remove("is-active");
      wizardStep1.hidden = false;
      wizardStep2.hidden = true;
      void runEnvCheck();
    } else {
      stepNav1.classList.remove("is-active");
      stepNav1.classList.add("is-passed");
      stepNav2.classList.add("is-active");
      wizardStep1.hidden = true;
      wizardStep2.hidden = false;
      void loadPluginsList();
    }
  }
}

async function runEnvCheck(): Promise<void> {
  if (!envCheckListEl) return;
  envCheckListEl.innerHTML = `
    <div class="env-item">
      <div class="env-item-main">
        <span class="env-item-title">正在检测系统环境…</span>
        <span class="env-item-desc">正在检查 Node.js, npm, pnpm, dsh</span>
      </div>
      <span class="env-item-badge loading">
        <span class="status-dot"></span> 检测中
      </span>
    </div>
  `;

  try {
    const res = await invoke<EnvCheckResult>("check_env");
    renderEnvCheckResult(res);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    envCheckListEl.innerHTML = `
      <div class="env-item">
        <div class="env-item-main">
          <span class="env-item-title">环境检测异常</span>
          <span class="env-item-desc">${msg}</span>
        </div>
        <span class="env-item-badge error">
          <span class="status-dot"></span> 出错
        </span>
      </div>
    `;
  }
}

function renderEnvCheckResult(res: EnvCheckResult): void {
  if (!envCheckListEl) return;

  const items = [
    { label: "Node.js 运行环境", tool: res.node, required: true },
    { label: "npm 包管理器", tool: res.npm, required: true },
    { label: "pnpm (推荐)", tool: res.pnpm, required: false },
    { label: "DeepSeek Harness (dsh 内核)", tool: res.dsh, required: true },
  ];

  envCheckListEl.innerHTML = items
    .map(({ label, tool, required }) => {
      let badgeClass = "error";
      let badgeText = "未安装";
      let descText = "未检测到可执行文件";

      if (tool.found) {
        badgeClass = "success";
        badgeText = tool.version ? `v${tool.version.replace(/^v/, "")}` : "已就绪";
        descText = tool.path || "已在环境变量 PATH 中";
      } else if (!required) {
        badgeClass = "warning";
        badgeText = "未检测到";
        descText = "可选工具（安装后加速依赖解析）";
      }

      return `
        <div class="env-item">
          <div class="env-item-main">
            <span class="env-item-title">${label}</span>
            <span class="env-item-desc">${descText}</span>
          </div>
          <span class="env-item-badge ${badgeClass}">
            <span class="status-dot"></span> ${badgeText}
          </span>
        </div>
      `;
    })
    .join("");

  // 处理 dsh 是否已安装
  if (!res.dsh.found) {
    if (dshGuideBoxEl) dshGuideBoxEl.hidden = false;
    if (btnStep1Next) btnStep1Next.disabled = true;
  } else {
    if (dshGuideBoxEl) dshGuideBoxEl.hidden = true;
    if (btnStep1Next) btnStep1Next.disabled = false;
  }
}

async function loadPluginsList(): Promise<void> {
  if (!pluginsGridEl) return;
  pluginsGridEl.innerHTML = `
    <div style="padding: 24px; text-align: center; color: var(--fg-muted); font-size: 0.85rem;">
      正在获取推荐插件列表…
    </div>
  `;

  try {
    recommendedPlugins = await invoke<RepoPluginItem[]>("get_recommended_plugins");
    renderPluginsList(recommendedPlugins);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    pluginsGridEl.innerHTML = `
      <div style="padding: 24px; text-align: center; color: var(--danger); font-size: 0.85rem;">
        获取插件列表失败：${msg}
      </div>
    `;
  }
}

function renderPluginsList(plugins: RepoPluginItem[]): void {
  if (!pluginsGridEl) return;

  if (plugins.length === 0) {
    pluginsGridEl.innerHTML = `
      <div style="padding: 24px; text-align: center; color: var(--fg-muted); font-size: 0.85rem;">
        暂无插件推荐
      </div>
    `;
    return;
  }

  pluginsGridEl.innerHTML = plugins
    .map((plugin) => {
      const btnLabel = plugin.installed ? "已安装" : "安装";
      const btnClass = plugin.installed ? "btn ghost success-btn" : "btn primary";
      const disabled = plugin.installed ? "disabled" : "";

      return `
        <div class="plugin-card" data-package="${plugin.package_name}">
          <div class="plugin-card-info">
            <div class="plugin-card-title-row">
              <span class="plugin-card-title">${plugin.name}</span>
              <span class="plugin-tag">${plugin.category}</span>
            </div>
            <p class="plugin-card-desc">${plugin.description}</p>
          </div>
          <div class="plugin-card-action">
            <button class="${btnClass} btn-plugin-install" data-pkg="${plugin.package_name}" ${disabled}>
              ${btnLabel}
            </button>
          </div>
        </div>
      `;
    })
    .join("");

  // 绑定各插件安装按钮
  const buttons = pluginsGridEl.querySelectorAll<HTMLButtonElement>(".btn-plugin-install");
  buttons.forEach((btn) => {
    btn.addEventListener("click", () => {
      const pkg = btn.dataset.pkg;
      if (!pkg) return;
      btn.disabled = true;
      btn.textContent = "安装中…";
      void invoke("install_plugin", { packageName: pkg });
    });
  });
}

async function copyText(text: string, buttonEl?: HTMLButtonElement | null): Promise<void> {
  let copied = false;
  try {
    await navigator.clipboard.writeText(text);
    copied = true;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
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
  if (copied && buttonEl) {
    const oldText = buttonEl.textContent;
    buttonEl.textContent = "已复制";
    setTimeout(() => {
      buttonEl.textContent = oldText;
    }, 2000);
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
  // 基础按钮事件
  btnRestart?.addEventListener("click", () => {
    void invoke("restart_kernel");
  });
  btnRedetect?.addEventListener("click", () => {
    void invoke("restart_kernel");
  });
  btnCopy?.addEventListener("click", () => {
    void copyText(INSTALL_COMMAND, btnCopy);
  });
  btnInstallDsh?.addEventListener("click", () => {
    if (!btnInstallDsh) return;
    btnInstallDsh.disabled = true;
    btnInstallDsh.textContent = "安装中…";
    void invoke("install_dsh");
  });
  btnUpdate?.addEventListener("click", () => {
    if (btnUpdate) {
      btnUpdate.disabled = true;
      btnUpdate.textContent = "更新中…";
    }
    void invoke("install_update");
  });

  // 向导事件绑定
  btnCopyWizardCmd?.addEventListener("click", () => {
    void copyText(INSTALL_COMMAND, btnCopyWizardCmd);
  });

  btnWizardInstallDsh?.addEventListener("click", () => {
    if (!btnWizardInstallDsh) return;
    btnWizardInstallDsh.disabled = true;
    btnWizardInstallDsh.textContent = "安装中…";
    void invoke("install_dsh");
  });

  btnWizardRecheck?.addEventListener("click", () => {
    void runEnvCheck();
  });

  // 确保宿主 iframe 加载与窗口聚焦时焦点正确落入，保障剪贴板（Ctrl+V / Cmd+V）直接可用
  dshFrame?.addEventListener("load", () => {
    try {
      dshFrame.focus();
      dshFrame.contentWindow?.focus();
    } catch {
      // 跨域时直接对 iframe 元素调用 focus
      dshFrame.focus();
    }
  });

  window.addEventListener("focus", () => {
    if (hostEl && !hostEl.hidden && dshFrame) {
      dshFrame.focus();
    }
  });

  btnStep1Recheck?.addEventListener("click", () => {
    void runEnvCheck();
  });

  btnStep1Next?.addEventListener("click", () => {
    setWizardStep(2);
  });

  btnStep2Prev?.addEventListener("click", () => {
    setWizardStep(1);
  });

  btnStep2Finish?.addEventListener("click", async () => {
    if (btnStep2Finish) {
      btnStep2Finish.disabled = true;
      btnStep2Finish.textContent = "正在启动内核…";
    }
    try {
      await invoke("complete_setup");
      isInitialized = true;
      showView("loading");
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      renderError(msg);
    }
  });

  // 自定义标题栏
  const win = getCurrentWindow();
  const syncMax = async (): Promise<void> => {
    const maximized = await win.isMaximized();
    document.body.classList.toggle("is-maximized", maximized);
    if (!btnMax) return;
    btnMax.classList.toggle("is-max", maximized);
    btnMax.title = maximized ? "还原" : "最大化";
    btnMax.setAttribute("aria-label", maximized ? "还原" : "最大化");
  };

  btnMin?.addEventListener("click", () => {
    void win.minimize();
  });
  btnMax?.addEventListener("click", () => {
    void win.toggleMaximize().then(syncMax);
  });
  btnClose?.addEventListener("click", () => {
    void win.close();
  });

  titlebarEl?.addEventListener("dblclick", (event) => {
    if (
      event.target instanceof Element &&
      event.target.closest(".titlebar-controls")
    ) {
      return;
    }
    void win.toggleMaximize().then(syncMax);
  });

  let resizeTimer: number | null = null;
  void win.onResized(() => {
    if (resizeTimer !== null) cancelAnimationFrame(resizeTimer);
    resizeTimer = requestAnimationFrame(() => {
      void syncMax();
      resizeTimer = null;
    });
  });
  void syncMax();
  setTimeout(() => {
    void win.maximize().then(syncMax);
  }, 400);

  try {
    await listen<KernelStatusPayload>("kernel-status", (event) => {
      render(event.payload);
    });
    await listen<UpdatePayload>("dsh-update", (event) => {
      renderUpdate(event.payload);
    });
    await listen("dsh-installed", () => {
      if (btnInstallDsh) {
        btnInstallDsh.textContent = "已安装";
      }
      if (btnWizardInstallDsh) {
        btnWizardInstallDsh.textContent = "已安装";
      }
      void runEnvCheck();
      void refreshState();
    });
    await listen<{ package_name: string; success: boolean }>("plugin-installed", (event) => {
      const { package_name, success } = event.payload;
      const btn = pluginsGridEl?.querySelector<HTMLButtonElement>(`button[data-pkg="${package_name}"]`);
      if (btn) {
        if (success) {
          btn.textContent = "已安装";
          btn.className = "btn ghost success-btn btn-plugin-install";
          btn.disabled = true;
        } else {
          btn.textContent = "安装失败，重试";
          btn.className = "btn primary btn-plugin-install";
          btn.disabled = false;
        }
      }
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    renderError(`无法订阅内核状态：${message}`);
    return;
  }

  await refreshState();
}

async function refreshState(): Promise<void> {
  try {
    const state = await invoke<ShellState>("get_state");
    render(state);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    renderError(message);
  }
}

void init();
