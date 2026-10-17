<script setup lang="ts">
import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue';
import Icon from './components/Icon.vue';
import { api as desktopApi } from './api';
import { applyLanguage, language, message, resolveLanguage, t } from './i18n';
import { useCardHeight } from './useCardHeight';
import type { LanguagePreference, LanguageState } from './types';
import type { CodexState, GpuCard, GpuSettings, HostKeyRequest, JobCancelPreview, Profile, Secrets, ServerState, ShutdownNotice, SlurmJob, SlurmScope, ThemePreference, ThemeState, TmpCleanupPreview, Tunnel, UpdateState } from './types';
const api = window.lion ?? desktopApi;
const TerminalWorkspace = defineAsyncComponent(() => import('./components/TerminalWorkspace.vue'));
const terminalVisited = ref(false);
const preview = !api;
const systemTheme = window.matchMedia('(prefers-color-scheme: dark)');
const theme = ref<ThemeState>({ preference: cachedTheme(), resolved: 'light' });
let themeRevision = 0;
let languageRevision = 0;
applyLanguage(api?.initialLanguage || language.value);
const themeOptions = computed<{ value: ThemePreference; label: string; icon: string; title: string }[]>(() => [
    { value: 'light', label: t("白天"), icon: 'sun', title: t("白天主题") },
    { value: 'dark', label: t("夜晚"), icon: 'moon', title: t("夜晚主题") },
    { value: 'system', label: t("系统"), icon: 'monitor', title: t("跟随系统主题") },
]);
const shutdown = ref<ShutdownNotice | null>(null);
applyTheme(api?.initialTheme || { preference: theme.value.preference, resolved: theme.value.preference === 'system' ? systemTheme.matches ? 'dark' : 'light' : theme.value.preference });
const profiles = ref<Profile[]>([]);
const states = reactive<Record<string, ServerState>>({});
const selectedId = ref('');
const page = ref<'workspace' | 'codex' | 'terminal'>('workspace');
const codexErrors = reactive<Record<string, string>>({});
const codexRequestIds: Record<string, number> = {};
const codexRevisions: Record<string, number> = {};
const version = ref('1.0.0');
const update = ref<UpdateState | null>(null);
let updateRevision = 0;
const updatePending = computed(() => Boolean(update.value?.version && update.value.version !== version.value));
const updating = computed(() => Boolean(busy.update || ['downloading', 'ready', 'installing'].includes(update.value?.status || '')));
const updateProgress = computed(() => update.value?.total ? `${Math.min(100, Math.floor(update.value.downloaded * 100 / update.value.total))}%` : t("下载中"));
const loading = ref(true);
const startupError = ref('');
const now = ref(Date.now());
const busy = reactive<Record<string, boolean>>({});
const pendingSlurmScopes = reactive<Record<string, SlurmScope>>({});
const slurmPage = ref(1);
const slurmPageSize = 100;
const {
    cardElement: slurmCardElement, cardHeight: slurmCardHeight, currentHeight: slurmCurrentHeight,
    resizing: slurmResizing, minHeight: slurmMinHeight, maxHeight: slurmMaxHeight,
    startResize: startSlurmResize, moveResize: moveSlurmResize, finishResize: finishSlurmResize,
    resizeKey: resizeSlurmKey, resetHeight: resetSlurmHeight,
} = useCardHeight('lion-slurm-card-height');
const notices = ref<{
    id: number;
    message: string | (() => string);
    error: boolean;
}[]>([]);
const drawer = ref<'device' | 'tunnel' | null>(null);
const editing = ref(false);
const formError = ref('');
const passwordVisible = ref(false);
const devicePassword = ref('');
const devicePassphrase = ref('');
const rememberSecret = ref(false);
const expandedGpu = ref(false);
const showGpuRules = ref(false);
const gpuRules = reactive({ idleUtil: 5, idleMemoryMiB: 512, stableSamples: 2 });
const pollInterval = ref(5);
const credentialProfile = ref<Profile | null>(null);
const credentialPassword = ref('');
const credentialPassphrase = ref('');
const credentialRemember = ref(false);
const deleting = ref<{
    kind: 'device' | 'tunnel';
    id: string;
    name: string;
} | null>(null);
const jobCancellation = ref<{
    profileId: string;
    profileName: string;
    jobName: string;
    preview: JobCancelPreview;
} | null>(null);
const tmpCleanup = ref<{
    profileId: string;
    profileName: string;
    preview: TmpCleanupPreview;
} | null>(null);
const priorities = reactive<Record<string, {
    value?: number | null;
    values?: number[];
    error?: string;
}>>({});
const connectionGenerations: Record<string, number> = {};
const hostRequests = ref<HostKeyRequest[]>([]);
const runtimeSecrets: Record<string, Secrets> = {};
let clockId: ReturnType<typeof setInterval>;
let removeState: (() => void) | undefined;
let removeHost: (() => void) | undefined;
let removeTheme: (() => void) | undefined;
let removeLanguage: (() => void) | undefined;
let removeShutdown: (() => void) | undefined;
let removeBackendError: (() => void) | undefined;
let removeUpdate: (() => void) | undefined;
let initializingStates: Set<string> | undefined;
let disposed = false;
let noticeId = 0;
function receiveLanguage(state: LanguageState) {
    languageRevision += 1;
    applyLanguage(state);
}
async function changeLanguage(preference: LanguagePreference) {
    if (busy.language || shutdown.value) return;
    busy.language = true;
    try {
        receiveLanguage(api?.setLanguage ? await api.setLanguage(preference) : { preference, resolved: resolveLanguage(preference) });
    } catch (error) { notify(() => t('无法保存语言：{0}', { 0: message(cleanError(error)) }), true); }
    finally { busy.language = false; }
}
function languageChanged(event: Event) {
    const preference = (event.target as HTMLSelectElement).value;
    if (preference === 'system' || preference === 'zh' || preference === 'en') void changeLanguage(preference);
}
function systemLanguageChanged() {
    if (!api && language.value.preference === 'system') receiveLanguage({ preference: 'system', resolved: resolveLanguage('system') });
}
function cachedTheme(): ThemePreference {
    try {
        const saved = localStorage.getItem('lion-theme');
        return saved === 'light' || saved === 'dark' ? saved : 'system';
    }
    catch {
        return 'system';
    }
}
function applyTheme(next: ThemeState) {
    themeRevision += 1;
    theme.value = next;
    document.documentElement.dataset.theme = next.resolved;
    document.documentElement.style.colorScheme = next.resolved;
    document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')?.setAttribute('content', next.resolved === 'dark' ? '#141a24' : '#f4f6fa');
    try {
        localStorage.setItem('lion-theme', next.preference);
    }
    catch { /* Theme still works when browser storage is unavailable. */ }
}
async function changeTheme(preference: ThemePreference) {
    if (busy.theme || shutdown.value)
        return;
    busy.theme = true;
    try {
        applyTheme(api ? await api.setTheme(preference) : { preference, resolved: preference === 'system' ? systemTheme.matches ? 'dark' : 'light' : preference });
    }
    catch (error) {
        notify(() => t("无法保存主题：{0}", { "0": message(cleanError(error)) }), true);
    }
    finally {
        busy.theme = false;
    }
}
function systemThemeChanged() {
    if (preview && theme.value.preference === 'system')
        applyTheme({ preference: 'system', resolved: systemTheme.matches ? 'dark' : 'light' });
}
function beginShutdown(notice: ShutdownNotice) {
    shutdown.value = notice;
    drawer.value = null;
    deleting.value = null;
    jobCancellation.value = null;
    tmpCleanup.value = null;
    credentialProfile.value = null;
    hostRequests.value = [];
}
function receiveUpdate(state: UpdateState) {
    updateRevision += 1;
    update.value = state;
}
async function installUpdate() {
    if (!api?.installUpdate || updating.value || shutdown.value)
        return;
    busy.update = true;
    try {
        if (update.value?.manualOnly) {
            if (!api.openUpdateDownload)
                throw Error(t("请从 GitHub Releases 下载新版本"));
            await api.openUpdateDownload();
        }
        else {
            await api.installUpdate();
        }
    }
    catch (error) {
        shutdown.value = null;
        notify(() => t("更新未完成：{0}", { "0": message(cleanError(error)) }), true);
    }
    finally {
        busy.update = false;
    }
}
function freshProfile(): Profile {
    return {
        id: crypto.randomUUID(), name: '', host: '', port: 22, username: '',
        authType: 'key', keyPath: '', autoReconnect: true,
        gpu: { enabled: true, intervalSec: 5, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 2 },
        tunnels: [],
    };
}
function clone<T>(value: T): T { return JSON.parse(JSON.stringify(value)) as T; }
function freshTunnel(): Tunnel {
    return {
        id: crypto.randomUUID(), name: '', direction: 'local', bindHost: '127.0.0.1',
        bindPort: 6006, targetHost: '127.0.0.1', targetPort: 6006, enabled: true, healthIntervalSec: 15,
    };
}
const deviceDraft = reactive<Profile>(freshProfile());
const tunnelDraft = reactive<Tunnel>(freshTunnel());
const selected = computed(() => profiles.value.find(p => p.id === selectedId.value));
const currentState = computed(() => selected.value ? getState(selected.value) : null);
const connected = computed(() => currentState.value?.status === 'connected');
const codex = computed(() => currentState.value?.codex);
const codexBusy = computed(() => Boolean(selected.value && (busy[`codex:${selected.value.id}`] || ['checking', 'installing', 'restarting'].includes(codex.value?.status || ''))));
const codexInstalling = computed(() => ['installing', 'restarting'].includes(codex.value?.status || '') || Boolean(selected.value && busy[`codexInstall:${selected.value.id}`]));
const codexError = computed(() => selected.value ? codexErrors[selected.value.id] || codex.value?.error || '' : '');
const codexCanInstall = computed(() => Boolean(connected.value && !codexBusy.value && !codexError.value && codex.value?.canInstall && codex.value.latestVersion && (codex.value.installed === false || codex.value.installed === true && codex.value.updateAvailable && newerVersion(codex.value.latestVersion, codex.value.currentVersion))));
const codexActionLabel = computed(() => codex.value?.status === 'restarting' ? t("正在重启") : codexInstalling.value ? codex.value?.installed === false ? t("正在安装") : t("正在更新") : codex.value?.installed === false ? t("安装 Codex") : t("更新 Codex"));
const codexStatusLabel = computed(() => {
    if (!connected.value)
        return t("等待连接");
    if (codex.value?.status === 'checking' || codexBusy.value && !codexInstalling.value)
        return t("正在检测");
    if (codex.value?.status === 'restarting')
        return t("正在重启");
    if (codexInstalling.value)
        return codex.value?.installed === false ? t("正在安装") : t("正在更新");
    if (codexError.value)
        return t("检测或操作未完成");
    if (codex.value?.installed === false)
        return t("尚未安装");
    if (codex.value?.updateAvailable && newerVersion(codex.value.latestVersion, codex.value.currentVersion))
        return t("有新版本");
    if (codex.value?.currentVersion && codex.value?.latestVersion)
        return t("已是最新版本");
    return t("等待检测");
});
const connectionActive = computed(() => currentState.value && !['disconnected', 'error', 'failed', 'idle'].includes(currentState.value.status));
const onlineCount = computed(() => profiles.value.filter(p => getState(p).status === 'connected').length);
const gpu = computed(() => currentState.value?.gpu);
const gpuAvailability = computed(() => currentState.value ? gpuAvailabilityFor(currentState.value) : 'unknown');
const showGpuCard = computed(() => gpuAvailability.value === 'available');
const gpuDetecting = computed(() => ['loading', 'polling', 'checking'].includes(gpu.value?.status || ''));
const gpuDetectionText = computed(() => {
    if (gpuDetecting.value)
        return t("正在检测显卡\u2026");
    if (gpuAvailability.value === 'absent')
        return t("未检测到 NVIDIA 显卡");
    if (gpuAvailability.value === 'unavailable')
        return t("显卡监测不可用");
    if (gpu.value?.error)
        return t("无法确认显卡状态");
    if (!connected.value)
        return t("连接后检测显卡");
    if (!selected.value?.gpu.enabled)
        return t("显卡自动检测已暂停");
    return t("等待显卡检测");
});
const gpuDetectionDetail = computed(() => {
    if (gpuDetecting.value)
        return '';
    if (gpuAvailability.value === 'unavailable')
        return t("未找到 nvidia-smi，无法确认显卡硬件");
    if (gpuAvailability.value === 'absent' && connected.value && selected.value?.gpu.enabled)
        return t("每 60 秒重新检测");
    return gpu.value?.error ? gpu.value.availabilityReason || gpu.value.error : '';
});
const hasGpuSample = computed(() => Boolean(gpu.value?.lastUpdated));
const gpuFresh = computed(() => Boolean(selected.value && currentState.value && sampleIsFresh(selected.value, currentState.value)));
const gpuTimerActive = computed(() => Boolean(connected.value && selected.value?.gpu.enabled));
const gpuTimerSampling = computed(() => gpuTimerActive.value && Boolean(gpu.value?.initialConfirming || gpuDetecting.value));
const gpuSampleNotice = computed(() => !connected.value ? t("连接设备后，将自动读取显卡状态") : !selected.value?.gpu.enabled ? t("自动检测已暂停，可手动刷新") : gpu.value?.initialConfirming ? t("已读取显卡，正在快速确认空闲状态\u2026") : hasGpuSample.value ? '' : t("正在读取首次显卡状态\u2026"));
const gpuPollDuration = computed(() => {
    const interval = Math.min(300, Math.max(2, gpu.value?.intervalSec || selected.value?.gpu.intervalSec || 5)) * 1000;
    const completed = timestamp(gpu.value?.lastUpdated);
    const scheduled = timestamp(gpu.value?.nextPollAt);
    const sinceSample = scheduled - completed;
    // The worker starts the next interval after a completed sample. Preserve
    // that real deadline, and use its runtime interval when a read has failed.
    return !gpu.value?.error && completed && sinceSample >= interval && sinceSample < interval + 1000 ? sinceSample : interval;
});
const gpuTimerProgress = computed(() => {
    const next = timestamp(gpu.value?.nextPollAt);
    if (!gpuTimerActive.value || gpuTimerSampling.value || !next)
        return 0;
    return Math.max(0, Math.min(1, (next - now.value) / gpuPollDuration.value));
});
const countdown = computed(() => {
    if (!connected.value || !selected.value?.gpu.enabled)
        return '—';
    if (gpu.value?.initialConfirming)
        return t("确认中");
    if (['loading', 'polling', 'checking'].includes(gpu.value?.status || ''))
        return t("采样中");
    const next = timestamp(gpu.value?.nextPollAt);
    if (!next)
        return t("等待采样");
    return t("{0} 秒", { "0": Math.min(gpu.value?.intervalSec || selected.value?.gpu.intervalSec || 300, Math.max(0, Math.ceil((next - now.value) / 1000))) });
});
const slurm = computed(() => currentState.value?.slurm);
const showSlurmCard = computed(() => slurm.value?.availability === 'available');
const slurmScope = computed(() => selected.value && pendingSlurmScopes[selected.value.id] || slurm.value?.scope || 'mine');
const slurmLoading = computed(() => Boolean(selected.value && pendingSlurmScopes[selected.value.id]) || slurm.value?.status === 'loading');
// A refresh retains the completed result, including an empty queue. Only an
// initial read or a new scope needs to replace the content with a placeholder.
const slurmInitialLoading = computed(() => slurmLoading.value && !(slurm.value?.scope === slurmScope.value && slurm.value.lastUpdated));
const slurmLoadingHeight = ref<number | null>(null);
watch([slurmInitialLoading, selectedId], ([isLoading, id], [, previousId]) => {
    slurmLoadingHeight.value = isLoading && id === previousId
        ? slurmCardElement.value?.getBoundingClientRect().height ?? null : null;
});
const slurmMinimumHeight = computed(() => slurmCardHeight.value === null && slurmInitialLoading.value
    ? Math.max(slurmMinHeight, slurmLoadingHeight.value || 0) : slurmMinHeight);
const slurmJobs = computed(() => slurm.value?.scope === slurmScope.value ? slurm.value.jobs : []);
const slurmPageCount = computed(() => Math.max(1, Math.ceil(slurmJobs.value.length / slurmPageSize)));
const slurmPageRows = computed(() => slurmJobs.value.slice((slurmPage.value - 1) * slurmPageSize, slurmPage.value * slurmPageSize));
watch([selectedId, slurmScope], () => { slurmPage.value = 1; });
watch([page, selectedId, connected], ([view, id, isConnected]) => {
    if (view === 'codex' && id && isConnected)
        void refreshCodex();
});
watch([() => slurmJobs.value.length, slurmLoading], ([count, isLoading]) => {
    if (!isLoading)
        slurmPage.value = Math.min(slurmPage.value, Math.max(1, Math.ceil(Number(count) / slurmPageSize)));
});
watch(slurmPage, async () => {
    await nextTick();
    document.querySelector<HTMLDivElement>('.slurm-table-wrap')?.scrollTo({ top: 0 });
});
const slurmWarning = computed(() => {
    if (!connected.value)
        return slurm.value?.lastUpdated ? t("连接已断开，当前展示上次结果。") : t("连接设备后读取作业状态。");
    if (slurm.value?.error)
        return t("{0}{1}", { "0": message(slurm.value.error), "1": slurm.value.lastUpdated ? t(" \u00B7 当前展示上次结果") : '' });
    return '';
});
const slurmDetectionError = computed(() => !showSlurmCard.value && slurm.value?.availability === 'unknown' && slurm.value?.status === 'error' ? slurm.value.error || slurm.value.availabilityReason || t("暂时无法确认作业监测是否可用") : '');
const storage = computed(() => currentState.value?.storage);
const storageLoading = computed(() => storage.value?.status === 'loading' || Boolean(selected.value && busy[`storageRefresh:${selected.value.id}`]));
const storageWarning = computed(() => {
    if (storage.value?.error)
        return t("{0}{1}", { "0": message(storage.value.error), "1": storage.value.lastUpdated ? t(" \u00B7 当前展示上次结果") : '' });
    if (!connected.value)
        return storage.value?.lastUpdated ? t("连接已断开，当前展示上次结果。") : t("连接设备后读取空间占用。");
    return storage.value?.partial ? t("部分目录无法读取，统计可能不完整。") : '';
});
const confirmationExpired = computed(() => {
    const confirmation = jobCancellation.value || tmpCleanup.value;
    return Boolean(confirmation && now.value >= confirmation.preview.expiresAt);
});
const recentEvents = computed(() => [...(currentState.value?.events || [])]
    .sort((a, b) => timestamp(b.time) - timestamp(a.time)).slice(0, 40));
const hostRequest = computed(() => hostRequests.value[0]);
const anyModal = computed(() => Boolean(drawer.value || credentialProfile.value || deleting.value || jobCancellation.value || tmpCleanup.value || hostRequest.value));
function getState(profile: Profile): ServerState {
    return states[profile.id] || {
        profileId: profile.id, status: 'disconnected',
        gpu: { status: 'unknown', availability: 'unknown', cards: [], idleCount: 0, total: 0, intervalSec: profile.gpu.intervalSec },
        tunnels: [], events: [],
    };
}
function newerVersion(latest?: string | null, current?: string | null): boolean {
    const parse = (value?: string | null) => value?.trim().match(/^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/);
    const next = parse(latest), installed = parse(current);
    if (!next || !installed)
        return false;
    for (let index = 1; index <= 3; index += 1) {
        if (Number(next[index]) !== Number(installed[index]))
            return Number(next[index]) > Number(installed[index]);
    }
    if (!next[4] || !installed[4])
        return Boolean(installed[4] && !next[4]);
    const left = next[4].split('.'), right = installed[4].split('.');
    for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
        if (left[index] === undefined || right[index] === undefined)
            return left[index] !== undefined;
        if (left[index] === right[index])
            continue;
        const numericLeft = /^\d+$/.test(left[index]), numericRight = /^\d+$/.test(right[index]);
        if (numericLeft && numericRight)
            return Number(left[index]) > Number(right[index]);
        if (numericLeft !== numericRight)
            return !numericLeft;
        return left[index] > right[index];
    }
    return false;
}
function openCodex() {
    page.value = 'codex';
    window.scrollTo({ top: 0 });
    void nextTick(() => document.querySelector<HTMLButtonElement>('[data-testid="codex-back"]')?.focus());
}
function backFromCodex() {
    page.value = 'workspace';
    void nextTick(() => document.querySelector<HTMLButtonElement>('[data-testid="tool-codex"]')?.focus());
}
function openTerminal() {
    terminalVisited.value = true;
    page.value = 'terminal';
    window.scrollTo({ top: 0 });
    void nextTick(() => document.querySelector<HTMLButtonElement>('[data-testid="terminal-back"]')?.focus());
}
function backFromTerminal() {
    page.value = 'workspace';
    void nextTick(() => document.querySelector<HTMLButtonElement>('[data-testid="tool-terminal"]')?.focus());
}
async function runCodexOperation(install: boolean) {
    const profile = selected.value;
    if (!profile || !connected.value || codexBusy.value || shutdown.value || install && !codexCanInstall.value)
        return;
    if (!api?.refreshCodex || install && !api.installCodex) {
        codexErrors[profile.id] = "当前界面未连接支持小工具的桌面后台。";
        return;
    }
    const id = profile.id;
    const generation = connectionGenerations[id] || 0;
    const revision = codexRevisions[id] || 0;
    const requestId = codexRequestIds[id] = (codexRequestIds[id] || 0) + 1;
    delete codexErrors[id];
    busy[`codex:${id}`] = true;
    busy[`codexInstall:${id}`] = install;
    try {
        const result = await (install ? api.installCodex(id) : api.refreshCodex(id));
        if (disposed || codexRequestIds[id] !== requestId || (connectionGenerations[id] || 0) !== generation || states[id]?.status !== 'connected')
            return;
        // A live event may already contain a newer operation stage or result.
        if ((codexRevisions[id] || 0) === revision)
            states[id] = { ...states[id], codex: result };
    }
    catch (error) {
        if (!disposed && codexRequestIds[id] === requestId && (connectionGenerations[id] || 0) === generation && states[id]?.status === 'connected')
            codexErrors[id] = cleanError(error);
    }
    finally {
        if (codexRequestIds[id] === requestId) {
            busy[`codex:${id}`] = false;
            busy[`codexInstall:${id}`] = false;
        }
    }
}
function refreshCodex() { return runCodexOperation(false); }
function installCodex() { return runCodexOperation(true); }
function codexMethodLabel(method?: CodexState['installMethod']) {
    return method === 'npm' ? t("npm 全局安装") : method === 'native' ? t("官方原生安装") : method === 'unknown' ? t("安装来源未识别") : t("检测后显示");
}
function gpuAvailabilityFor(state: ServerState): NonNullable<ServerState['gpu']['availability']> {
    return state.gpu.availability || (state.gpu.cards.length > 0 ? 'available' : 'unknown');
}
function sampleIsFresh(profile: Profile, state: ServerState): boolean {
    return gpuAvailabilityFor(state) === 'available' && state.status === 'connected' && profile.gpu.enabled && Boolean(state.gpu.lastUpdated) && !state.gpu.error && !['error', 'stale', 'disabled', 'unknown', 'paused', 'unavailable'].includes(state.gpu.status);
}
function statusLabel(status?: string): string {
    return ({ connected: t("已连接"), connecting: t("连接中"), reconnecting: t("恢复中"), recovering: t("恢复中"),
        disconnecting: t("关闭中"), disconnected: t("未连接"), error: t("连接失败"), failed: t("连接失败"),
        'awaiting-host-key': t("等待指纹确认"), 'awaiting-trust': t("等待指纹确认"), 'waiting-host-key': t("等待指纹确认"), 'host-key': t("等待指纹确认"), idle: t("未连接"),
    } as Record<string, string>)[status || 'disconnected'] || status || t("未连接");
}
function statusClass(status?: string): string {
    if (['connected', 'healthy', 'active', 'running', 'listening', 'ready', 'up'].includes(status || ''))
        return 'positive';
    if (['error', 'failed', 'degraded', 'target-unavailable', 'unhealthy'].includes(status || ''))
        return 'negative';
    if (['connecting', 'reconnecting', 'recovering', 'checking', 'starting', 'stopping', 'disconnecting', 'waiting-port', 'awaiting-trust', 'awaiting-host-key', 'waiting-host-key'].includes(status || ''))
        return 'pending';
    return 'neutral';
}
function tunnelState(tunnel: Tunnel) {
    return currentState.value?.tunnels.find(t => t.id === tunnel.id);
}
function tunnelStatus(tunnel: Tunnel): string {
    if (!tunnel.enabled)
        return t("已暂停");
    const status = tunnelState(tunnel)?.status;
    if (!status || status === 'stopped' || status === 'disabled' || status === 'idle')
        return connected.value ? t("等待启动") : t("等待连接");
    if (['running', 'active', 'healthy', 'listening', 'ready', 'up', 'unverified'].includes(status)) {
        const health = tunnelState(tunnel);
        if (health?.healthStatus === 'tcp-ok' || health?.healthVerified || /TCP.*(?:可连接|正常)/i.test(health?.error || ''))
            return t("TCP 连通正常");
        if (health?.healthStatus === 'probe-restricted')
            return t("监听中 \u00B7 检测受限");
        if (health?.healthStatus === 'pending')
            return t("监听已建立 \u00B7 检测中");
        return t("监听已建立 \u00B7 未验证");
    }
    return ({ active: t("运行中"), healthy: t("连通正常"), listening: t("监听中"), checking: t("检测中"),
        starting: t("建立中"), recovering: t("恢复中"), reconnecting: t("恢复中"), 'waiting-port': t("等待端口释放"),
        'target-unavailable': t("目标不可用"), degraded: t("目标不可用"), unverified: t("监听已建立 \u00B7 未验证"),
        error: t("异常"), failed: t("建立失败"), stopping: t("关闭中"), ready: t("运行中"), up: t("连通正常"),
    } as Record<string, string>)[status] || status;
}
function tunnelDetail(tunnel: Tunnel): string {
    const health = tunnelState(tunnel);
    if (health?.healthStatus === 'tcp-ok' || health?.healthVerified || /TCP.*(?:可连接|正常)/i.test(health?.error || ''))
        return message(health?.healthDetail) || t("TCP 路径可连接；未检查应用响应");
    if (health?.healthStatus === 'probe-restricted')
        return message(health.healthDetail) || message(health.error) || t("服务器拒绝主动探测；监听已建立，完整路径尚未验证");
    return message(health?.healthDetail) || message(health?.error) || (health?.lastChecked ? t("检测于 {0}", { "0": timeText(health.lastChecked) }) : t("每 {0} 秒检测", { "0": tunnel.healthIntervalSec }));
}
function processCount(card: GpuCard): number { return Array.isArray(card.computeProcesses) ? card.computeProcesses.length : card.computeProcesses || 0; }
function cardLabel(card: GpuCard): string {
    if (!gpuFresh.value)
        return t("上次结果");
    if (card.idle)
        return t("空闲");
    if (card.memoryUsedMiB === null || card.memoryTotalMiB === null || card.utilization === null)
        return t("未知");
    if (processCount(card))
        return t("使用中");
    if (card.idleCandidate)
        return t("待确认");
    if (card.utilization > (selected.value?.gpu.idleUtil ?? 5) || card.memoryUsedMiB > (selected.value?.gpu.idleMemoryMiB ?? 512))
        return t("使用中");
    return t("未知");
}
function numberText(value: number | null): string { return value === null ? '—' : value.toLocaleString(); }
function metricPercent(value: number | null): number | null { return value === null || !Number.isFinite(value) ? null : Math.max(0, Math.min(100, value)); }
function memoryPercent(card: GpuCard): number | null { return card.memoryUsedMiB === null || card.memoryTotalMiB === null || card.memoryTotalMiB <= 0 ? null : metricPercent(card.memoryUsedMiB / card.memoryTotalMiB * 100); }
function percentText(value: number | null): string { return value === null ? '—' : `${value.toLocaleString(undefined, { maximumFractionDigits: 1 })}%`; }
function memoryTotal(card: GpuCard): string { return card.memoryTotalMiB === null ? '—' : (card.memoryTotalMiB / 1024).toFixed(1); }
function timestamp(value?: number | string | null): number {
    if (!value)
        return 0;
    return typeof value === 'number' ? value : Number.isNaN(Number(value)) ? new Date(value).getTime() : Number(value);
}
function timeText(value?: number | string | null, full = false): string {
    const time = timestamp(value);
    if (!time)
        return t("尚未更新");
    return new Intl.DateTimeFormat(language.value.resolved === 'zh' ? 'zh-CN' : 'en', { ...(full ? { month: '2-digit', day: '2-digit' } as const : {}), hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }).format(time);
}
function cleanError(error: unknown): string {
    return (error instanceof Error ? error.message : String(error)).replace(/^Error invoking remote method '[^']+': (?:Error: )?/, '');
}
function noticeText(source: string | (() => string)) {
    return message(typeof source === 'function' ? source() : t(source));
}
function notify(message: string | (() => string), error = false) {
    const id = ++noticeId;
    notices.value.push({ id, message, error });
    setTimeout(() => { notices.value = notices.value.filter(n => n.id !== id); }, error ? 8000 : 3800);
}
async function action(key: string, work: () => Promise<unknown>): Promise<boolean> {
    if (shutdown.value)
        return false;
    if (preview) {
        notify(() => t("这是浏览器界面预览，请在桌面应用中操作连接。"));
        return false;
    }
    if (busy[key])
        return false;
    busy[key] = true;
    try {
        await work();
        return true;
    }
    catch (error) {
        notify(cleanError(error), true);
        return false;
    }
    finally {
        busy[key] = false;
    }
}
async function changeSlurmScope(scope: SlurmScope) {
    const profile = selected.value;
    if (!profile || !connected.value || scope === slurmScope.value || busy[`slurmScope:${profile.id}`])
        return;
    pendingSlurmScopes[profile.id] = scope;
    if (!await action(`slurmScope:${profile.id}`, () => api!.setSlurmScope(profile.id, scope)))
        delete pendingSlurmScopes[profile.id];
}
function slurmStateTitle(job: SlurmJob): string {
    return job.stateCode ? t("Slurm 状态代码：{0}", { "0": job.stateCode }) : '';
}
function slurmReasonTitle(job: SlurmJob): string {
    return job.reasonCode ? t("Slurm 原因：{0}", { "0": job.reasonCode }) : '';
}
function ownsJob(job: SlurmJob): boolean {
    return Boolean(selected.value && (slurm.value?.scope === 'mine' || job.user === selected.value.username));
}
function priorityKey(job: SlurmJob, profileId = selected.value?.id || ''): string { return `${profileId}:${job.cluster || ''}:${job.id}`; }
function clearPriorities(profileId: string) {
    for (const key of Object.keys(priorities))
        if (key.startsWith(`${profileId}:`))
            delete priorities[key];
}
async function requestPriority(job: SlurmJob) {
    const profile = selected.value;
    if (!profile || !connected.value)
        return;
    const key = priorityKey(job, profile.id);
    const generation = connectionGenerations[profile.id] || 0;
    await action(`priority:${key}`, async () => {
        delete priorities[key];
        try {
            const result = await api!.getJobPriority(profile.id, job.id);
            if ((connectionGenerations[profile.id] || 0) === generation && states[profile.id]?.status === 'connected')
                priorities[key] = { value: result.priority, values: result.priorities };
        }
        catch (error) {
            if ((connectionGenerations[profile.id] || 0) === generation)
                priorities[key] = { error: cleanError(error) };
            throw error;
        }
    });
}
async function requestJobCancellation(job: SlurmJob) {
    const profile = selected.value;
    if (!profile || !connected.value || !ownsJob(job))
        return;
    const generation = connectionGenerations[profile.id] || 0;
    await action(`prepareCancel:${priorityKey(job, profile.id)}`, async () => {
        const result = await api!.prepareJobCancel(profile.id, job.id);
        if (selectedId.value !== profile.id || (connectionGenerations[profile.id] || 0) !== generation || states[profile.id]?.status !== 'connected')
            return;
        tmpCleanup.value = null;
        jobCancellation.value = { profileId: profile.id, profileName: profile.name, jobName: job.name, preview: result };
        void focusFirst();
    });
}
async function cancelJobConfirmed() {
    const target = jobCancellation.value;
    if (!target || confirmationExpired.value)
        return;
    if (await action('cancelJob', async () => {
        await api!.cancelJob(target.profileId, target.preview.jobId, target.preview.token);
        for (const key of Object.keys(priorities))
            if (key.startsWith(`${target.profileId}:`) && key.endsWith(`:${target.preview.jobId}`))
                delete priorities[key];
    })) {
        jobCancellation.value = null;
        notify(() => t("已取消作业 {0}", { "0": target.preview.jobId }));
    }
    else
        jobCancellation.value = null;
}
function bytesText(bytes: number | null | undefined): string {
    if (bytes === null || bytes === undefined || !Number.isFinite(bytes))
        return '—';
    if (bytes === 0)
        return '0 B';
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
    const exponent = Math.min(units.length - 1, Math.max(0, Math.floor(Math.log(Math.abs(bytes)) / Math.log(1024))));
    return `${(bytes / 1024 ** exponent).toLocaleString(language.value.resolved === 'zh' ? 'zh-CN' : 'en', { maximumFractionDigits: exponent ? 2 : 0 })} ${units[exponent]}`;
}
async function requestTmpCleanup() {
    const profile = selected.value;
    if (!profile || !connected.value)
        return;
    const generation = connectionGenerations[profile.id] || 0;
    await action(`prepareCleanup:${profile.id}`, async () => {
        const result = await api!.prepareTmpCleanup(profile.id);
        if (selectedId.value !== profile.id || (connectionGenerations[profile.id] || 0) !== generation || states[profile.id]?.status !== 'connected')
            return;
        jobCancellation.value = null;
        tmpCleanup.value = { profileId: profile.id, profileName: profile.name, preview: result };
        void focusFirst();
    });
}
async function cleanupTmpConfirmed() {
    const target = tmpCleanup.value;
    if (!target || confirmationExpired.value)
        return;
    if (await action('cleanupTmp', async () => {
        const result = await api!.cleanupTmp(target.profileId, target.preview.token);
        notify(() => result.cleanupStatus === 'unknown' ? t("清理结果暂时无法确认，请刷新空间统计并查看系统日志。") : t("{0}{1}{2}", { "0": result.cleanupStatus === 'partial' ? t("部分清理完成") : t("清理完成"), "1": result.deletedCount === null ? '' : t("，已删除 {0} 项", { "0": result.deletedCount.toLocaleString() }), "2": result.warnings?.length ? t("，请查看系统日志中的未清理项目") : '' }), result.cleanupStatus !== 'complete');
    }))
        tmpCleanup.value = null;
    else
        tmpCleanup.value = null;
}
function selectProfile(profile: Profile) {
    if (selectedId.value !== profile.id) {
        jobCancellation.value = null;
        tmpCleanup.value = null;
    }
    selectedId.value = profile.id;
    pollInterval.value = profile.gpu.intervalSec;
    Object.assign(gpuRules, { idleUtil: profile.gpu.idleUtil, idleMemoryMiB: profile.gpu.idleMemoryMiB, stableSamples: profile.gpu.stableSamples });
    showGpuRules.value = false;
}
async function focusFirst() {
    await nextTick();
    const panel = activePanel();
    panel?.querySelector<HTMLInputElement>('input, button')?.focus();
}
function activePanel(): HTMLElement | null { return document.querySelector('.host-modal') || document.querySelector('.confirm-modal') || document.querySelector('.credential-modal') || document.querySelector('.drawer'); }
function openDevice(profile?: Profile) {
    Object.assign(deviceDraft, clone(profile || freshProfile()));
    editing.value = Boolean(profile);
    devicePassword.value = '';
    devicePassphrase.value = '';
    rememberSecret.value = Boolean(profile?.hasSavedSecret);
    passwordVisible.value = false;
    formError.value = '';
    drawer.value = 'device';
    void focusFirst();
}
function openTunnel(tunnel?: Tunnel) {
    if (!selected.value)
        return;
    Object.assign(tunnelDraft, clone(tunnel || freshTunnel()));
    editing.value = Boolean(tunnel);
    formError.value = '';
    drawer.value = 'tunnel';
    void focusFirst();
}
function closeDrawer() { if (!busy.save)
    drawer.value = null; }
function replaceProfile(profile: Profile) {
    const index = profiles.value.findIndex(p => p.id === profile.id);
    if (index < 0)
        profiles.value.push(profile);
    else
        profiles.value[index] = profile;
    if (selectedId.value === profile.id)
        selectProfile(profile);
}
async function saveDevice() {
    formError.value = '';
    if (!deviceDraft.name.trim() || !deviceDraft.host.trim() || !deviceDraft.username.trim()) {
        formError.value = "请填写设备名称、服务器地址和用户名。";
        return;
    }
    if (deviceDraft.authType === 'key' && !deviceDraft.keyPath.trim()) {
        formError.value = "请选择或填写私钥文件路径。";
        return;
    }
    const profile = clone(deviceDraft);
    profile.name = profile.name.trim();
    profile.host = profile.host.trim();
    profile.username = profile.username.trim();
    profile.keyPath = profile.keyPath.replace(/[\u202a-\u202e\u2066-\u2069]/g, '').trim();
    const secrets: Secrets = profile.authType === 'password' ? { password: devicePassword.value || undefined, remember: rememberSecret.value } : { passphrase: devicePassphrase.value || undefined, remember: rememberSecret.value };
    const success = await action('save', async () => {
        const saved = await api!.saveProfile(profile, secrets);
        replaceProfile(saved);
        if (secrets.password || secrets.passphrase)
            runtimeSecrets[saved.id] = secrets;
        selectProfile(saved);
    });
    if (success) {
        drawer.value = null;
        notify(editing.value ? "设备配置已保存" : "设备已添加");
    }
}
async function saveTunnel() {
    if (!selected.value)
        return;
    const profile = clone(selected.value);
    const tunnel = clone(tunnelDraft);
    tunnel.name = tunnel.name.trim() || t("{0} {1}", { "0": tunnel.direction === 'local' ? t("正向") : t("反向"), "1": tunnel.bindPort });
    tunnel.bindHost = tunnel.bindHost.trim();
    tunnel.targetHost = tunnel.targetHost.trim();
    if (profile.tunnels.some(t => t.id !== tunnel.id && t.direction === tunnel.direction && t.bindHost === tunnel.bindHost && t.bindPort === tunnel.bindPort)) {
        formError.value = "该监听地址和端口已有同类型隧道，请选择其他端口。";
        return;
    }
    const index = profile.tunnels.findIndex(t => t.id === tunnel.id);
    if (index < 0)
        profile.tunnels.push(tunnel);
    else
        profile.tunnels[index] = tunnel;
    if (await action('save', async () => replaceProfile(await api!.saveProfile(profile)))) {
        drawer.value = null;
        notify(editing.value ? "隧道配置已保存" : "隧道已添加");
    }
}
async function chooseKey() {
    await action('pickKey', async () => { const path = await api!.pickKey(); if (path)
        deviceDraft.keyPath = path; });
}
async function requestConnection(profile: Profile) {
    if (profile.authType === 'password' && !profile.hasSavedSecret && !runtimeSecrets[profile.id]?.password) {
        credentialProfile.value = profile;
        credentialPassword.value = '';
        credentialPassphrase.value = '';
        credentialRemember.value = false;
        void focusFirst();
        return;
    }
    await action(`connect:${profile.id}`, () => api!.connect(profile.id, runtimeSecrets[profile.id]));
}
async function submitCredentials() {
    const profile = credentialProfile.value;
    if (!profile)
        return;
    const secrets: Secrets = { password: credentialPassword.value || undefined, passphrase: credentialPassphrase.value || undefined };
    if (credentialPassword.value || credentialPassphrase.value)
        runtimeSecrets[profile.id] = secrets;
    credentialProfile.value = null;
    await action(`connect:${profile.id}`, async () => {
        if (credentialRemember.value && (secrets.password || secrets.passphrase))
            replaceProfile(await api!.saveProfile(clone(profile), { ...secrets, remember: true }));
        await api!.connect(profile.id, secrets);
    });
}
async function disconnect(profile: Profile) {
    await action(`disconnect:${profile.id}`, () => api!.disconnect(profile.id));
}
async function applyGpu(changes: Partial<GpuSettings>) {
    const profile = selected.value;
    if (!profile)
        return;
    const settings = { ...profile.gpu, ...changes };
    if (settings.intervalSec < 2 || settings.intervalSec > 300 || !Number.isFinite(settings.intervalSec)) {
        pollInterval.value = profile.gpu.intervalSec;
        notify(() => t("检测间隔需要在 2 到 300 秒之间。"), true);
        return;
    }
    if (await action('gpuSettings', () => api!.updateGpu(profile.id, settings)))
        replaceProfile({ ...profile, gpu: settings });
}
async function toggleTunnel(tunnel: Tunnel) {
    const profile = selected.value;
    if (!profile)
        return;
    const enabled = !tunnel.enabled;
    if (await action(`tunnel:${tunnel.id}`, () => api!.setTunnel(profile.id, tunnel.id, enabled))) {
        replaceProfile({ ...profile, tunnels: profile.tunnels.map(t => t.id === tunnel.id ? { ...t, enabled } : t) });
    }
}
async function deleteConfirmed() {
    const target = deleting.value;
    if (!target)
        return;
    const success = await action('delete', async () => {
        if (target.kind === 'device') {
            await api!.deleteProfile(target.id);
            profiles.value = profiles.value.filter(p => p.id !== target.id);
            delete states[target.id];
            delete runtimeSecrets[target.id];
            if (selectedId.value === target.id) {
                selectedId.value = '';
                if (profiles.value[0])
                    selectProfile(profiles.value[0]);
            }
        }
        else if (selected.value) {
            const profile = clone(selected.value);
            profile.tunnels = profile.tunnels.filter(t => t.id !== target.id);
            replaceProfile(await api!.saveProfile(profile));
        }
    });
    if (success) {
        deleting.value = null;
        notify(target.kind === 'device' ? "设备已移除" : "隧道已移除");
    }
}
async function trustHost(accepted: boolean) {
    const request = hostRequest.value;
    if (!request)
        return;
    if (await action('trust', () => api!.trustHost(request.profileId, accepted, request.fingerprint))) {
        hostRequests.value = hostRequests.value.filter(pending => pending.profileId !== request.profileId || pending.fingerprint !== request.fingerprint);
    }
}
function keyHandler(event: KeyboardEvent) {
    if (shutdown.value) {
        event.preventDefault();
        return;
    }
    if (event.key === 'Escape') {
        if (hostRequest.value)
            return;
        if (jobCancellation.value && !busy.cancelJob)
            jobCancellation.value = null;
        else if (tmpCleanup.value && !busy.cleanupTmp)
            tmpCleanup.value = null;
        else if (deleting.value && !busy.delete)
            deleting.value = null;
        else if (credentialProfile.value)
            credentialProfile.value = null;
        else
            closeDrawer();
    }
    if (event.key === 'Tab' && anyModal.value) {
        const panel = activePanel();
        if (!panel)
            return;
        const elements = [...panel.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex="0"]')];
        const first = elements[0], last = elements[elements.length - 1];
        if (event.shiftKey && document.activeElement === first) {
            event.preventDefault();
            last?.focus();
        }
        else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault();
            first?.focus();
        }
    }
}
async function initialize() {
    startupError.value = '';
    loading.value = true;
    const receivedStates = new Set<string>();
    initializingStates = receivedStates;
    const startingThemeRevision = themeRevision;
    const startingLanguageRevision = languageRevision;
    const startingUpdateRevision = updateRevision;
    try {
        if (api) {
            const initial = await api.getInitial();
            if (disposed)
                return;
            profiles.value = initial.profiles;
            for (const state of initial.states || [])
                if (!receivedStates.has(state.profileId))
                    states[state.profileId] = state;
            version.value = initial.version;
            if (initial.update && updateRevision === startingUpdateRevision)
                receiveUpdate(initial.update);
            if (initial.theme && themeRevision === startingThemeRevision)
                applyTheme(initial.theme);
            if (initial.language && languageRevision === startingLanguageRevision)
                receiveLanguage(initial.language);
            if (initial.shutdown)
                beginShutdown(initial.shutdown);
            if (!shutdown.value)
                for (const request of initial.pendingHostKeys || []) {
                    if (states[request.profileId]?.status === 'awaiting-trust')
                        queueHostKey(request);
                }
        }
        if (profiles.value[0])
            selectProfile(profiles.value[0]);
        loading.value = false;
    }
    catch (error) {
        if (!disposed)
            startupError.value = cleanError(error);
    }
    finally {
        if (initializingStates === receivedStates)
            initializingStates = undefined;
    }
}
function queueHostKey(request: HostKeyRequest) {
    if (shutdown.value)
        return;
    if (states[request.profileId] && states[request.profileId].status !== 'awaiting-trust')
        return;
    if (!hostRequests.value.some(r => r.profileId === request.profileId && r.fingerprint === request.fingerprint))
        hostRequests.value.push(request);
    void focusFirst();
}
onMounted(() => {
    clockId = setInterval(() => { now.value = Date.now(); }, 100);
    document.addEventListener('keydown', keyHandler);
    systemTheme.addEventListener('change', systemThemeChanged);
    window.addEventListener('languagechange', systemLanguageChanged);
    if (api) {
        removeTheme = api.onTheme?.(applyTheme);
        removeLanguage = api.onLanguage?.(receiveLanguage);
        removeUpdate = api.onUpdate?.(receiveUpdate);
        removeShutdown = api.onShutdown?.(beginShutdown);
        removeBackendError = api.onBackendError?.(error => {
            startupError.value = error.message;
            loading.value = true;
        });
        removeState = api.onState(state => {
            initializingStates?.add(state.profileId);
            if (state.status !== 'awaiting-trust')
                hostRequests.value = hostRequests.value.filter(request => request.profileId !== state.profileId);
            const previous = states[state.profileId];
            if (state.codex && JSON.stringify(previous?.codex) !== JSON.stringify(state.codex)) {
                codexRevisions[state.profileId] = (codexRevisions[state.profileId] || 0) + 1;
                delete codexErrors[state.profileId];
            }
            if (previous?.status !== state.status && (previous?.status === 'connected' || state.status === 'connected')) {
                connectionGenerations[state.profileId] = (connectionGenerations[state.profileId] || 0) + 1;
                clearPriorities(state.profileId);
            }
            if (state.status !== 'connected') {
                if (jobCancellation.value?.profileId === state.profileId && !busy.cancelJob)
                    jobCancellation.value = null;
                if (tmpCleanup.value?.profileId === state.profileId && !busy.cleanupTmp)
                    tmpCleanup.value = null;
            }
            if (state.slurm?.status === 'ready' && jobCancellation.value?.profileId === state.profileId && !busy.cancelJob && !state.slurm.jobs.some(job => job.id === jobCancellation.value?.preview.jobId)) {
                jobCancellation.value = null;
                notify(() => t("该作业已不在队列中，请刷新作业列表。"), true);
            }
            states[state.profileId] = state;
            const requestedScope = pendingSlurmScopes[state.profileId];
            if (requestedScope && (state.status !== 'connected' || state.slurm?.availability === 'unavailable' || state.slurm?.scope === requestedScope && state.slurm.status !== 'loading'))
                delete pendingSlurmScopes[state.profileId];
        });
        removeHost = api.onHostKey(queueHostKey);
    }
    void initialize();
});
onUnmounted(() => { disposed = true; clearInterval(clockId); removeState?.(); removeHost?.(); removeTheme?.(); removeLanguage?.(); removeShutdown?.(); removeBackendError?.(); removeUpdate?.(); systemTheme.removeEventListener('change', systemThemeChanged); window.removeEventListener('languagechange', systemLanguageChanged); document.removeEventListener('keydown', keyHandler); });
</script>

<template>
  <main v-if="loading" class="startup-screen" data-testid="startup-screen" :aria-busy="!startupError" :inert="Boolean(shutdown)">
    <section class="startup-panel" :role="startupError ? 'alert' : 'status'" aria-live="polite">
      <div class="startup-mark"><Icon name="shield" :size="37" /></div>
      <h1>Lion <span>SSH</span></h1>
      <template v-if="startupError"><h2>{{ t('暂时无法打开工作台') }}</h2><p class="startup-error">{{ message(startupError) }}</p><button class="button primary" data-testid="startup-retry" @click="initialize()"><Icon name="refresh" :size="16" />{{ t('重新加载') }}</button></template>
      <template v-else><span class="startup-spinner" aria-hidden="true" /><h2>{{ t('正在打开工作台') }}</h2><p>{{ t('正在读取本机配置…') }}</p></template>
    </section>
    <span class="startup-caption">{{ t('让连接，始终在线') }}</span>
  </main>
  <div v-else class="app-shell" :class="{ 'has-modal': anyModal }" :inert="anyModal || Boolean(shutdown)" :aria-busy="Boolean(shutdown)">
    <aside class="sidebar" :aria-label="t('设备导航')">
      <div class="brand">
        <div class="brand-mark">
          <svg viewBox="0 0 40 40" fill="none" aria-hidden="true"><path d="m9 10 5-4 6 2 6-2 5 4 3 11-6 12-8 3-8-3-6-12 3-11Z" fill="currentColor" opacity=".18"/><path d="m12 13 4 1 4-3 4 3 4-1 2 9-4 7-6 3-6-3-4-7 2-9Z" stroke="currentColor" stroke-width="1.8"/><path d="m14 20 3 1m9-1-3 1m-5 3h4l-2 3-2-3Zm2 3v2" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
        </div>
        <div><strong>Lion <span>SSH</span></strong><small>{{ t('让连接，始终在线') }}</small></div>
      </div>
      <div class="workspace-nav"><Icon name="server" :size="18" /><span>{{ t('连接工作台') }}</span><span class="nav-pill">{{ profiles.length }}</span></div>
      <div class="sidebar-section-title"><span>{{ t('我的设备') }}</span><button class="icon-button subtle" :title="t('添加设备')" :aria-label="t('添加设备')" @click="openDevice()"><Icon name="plus" :size="18" /></button></div>
      <div class="server-list">
        <button v-for="profile in profiles" :key="profile.id" class="server-item" :class="{ selected: selectedId === profile.id }" @click="selectProfile(profile)">
          <span class="server-item-icon"><Icon name="server" :size="19" /></span>
          <span class="server-item-text"><strong>{{ profile.name }}</strong><small>{{ profile.host }}</small><span class="server-item-status"><i class="status-dot" :class="statusClass(getState(profile).status)" />{{ statusLabel(getState(profile).status) }}<span v-if="sampleIsFresh(profile, getState(profile))" class="sidebar-gpu">{{ getState(profile).gpu.initialConfirming ? t("确认空闲中") : getState(profile).gpu.idleCount + t(" 张空闲") }}</span></span></span>
          <Icon v-if="selectedId === profile.id" name="chevron" :size="15" />
        </button>
        <div v-if="!profiles.length && !loading" class="sidebar-empty">{{ t('还没有设备') }}<br />{{ t('添加后即可管理连接') }}</div>
      </div>
      <button class="add-device-sidebar" data-testid="add-device" @click="openDevice()"><Icon name="plus" :size="17" />{{ t('添加设备') }}</button>
      <div class="sidebar-bottom">
        <label class="language-control"><span>{{ t('界面语言') }}</span><select data-testid="language-select" :value="language.preference" :aria-label="t('界面语言')" :disabled="busy.language" @change="languageChanged"><option value="system">{{ t('跟随系统') }}</option><option value="zh">中文</option><option value="en">English</option></select></label>
        <div class="theme-control" data-testid="theme-select" role="group" :aria-label="t('界面主题')"><button v-for="option in themeOptions" :key="option.value" :data-testid="`theme-${option.value}`" :class="{ active: theme.preference === option.value }" :aria-pressed="theme.preference === option.value" :title="option.title" :aria-label="option.title" :disabled="busy.theme" @click="changeTheme(option.value)"><Icon :name="option.icon" :size="13" /><span>{{ option.label }}</span></button></div>
        <div class="sidebar-footer">
          <span><i class="status-dot positive" />{{ onlineCount }} {{ t('台在线') }}</span>
          <div class="version-control">
            <span>v{{ version }}</span>
            <button v-if="updatePending" class="update-button" data-testid="install-update" :disabled="updating || update?.status === 'checking'" :aria-label="t('{0} v{1}', { '0': update?.manualOnly ? t('下载') : t('更新到'), '1': update?.version })" :title="message(update?.error) || (update?.manualOnly ? t('打开 GitHub Releases 下载新版本') : t('下载并更新到 v{0}，安装前会安全断开连接', { '0': update?.version }))" @click="installUpdate">
              <Icon :name="updating ? 'refresh' : 'download'" :size="12" :class="{ spinning: updating }" /><span>{{ updating ? update?.status === 'installing' ? t("安装中") : updateProgress : update?.manualOnly ? t("下载") : t("更新") }}</span>
            </button>
          </div>
        </div>
      </div>
    </aside>

    <main class="workspace">
      <header v-if="page === 'workspace'" class="page-header"><div><div class="eyebrow">YOUR CONNECTIONS, AT A GLANCE</div><h1>{{ t('连接工作台') }}<span class="title-dot">.</span></h1></div><div class="header-actions"><div class="wall-clock"><Icon name="clock" :size="15" />{{ timeText(now) }}</div></div></header>
      <div v-if="preview" class="preview-banner"><Icon name="info" :size="17" /><span>{{ t('浏览器界面预览 · 此处未连接服务器，连接操作请使用桌面应用。') }}</span></div>

      <TerminalWorkspace v-if="terminalVisited" v-show="page === 'terminal'" :visible="page === 'terminal'" :api="api" :profile-id="selectedId" :profiles="profiles" :states="states" :disabled="Boolean(shutdown)" @back="backFromTerminal" @connect="selected && requestConnection(selected)" />

      <section v-if="page === 'codex'" class="codex-page" data-testid="codex-page">
        <div class="tool-page-toolbar"><button class="button secondary small" data-testid="codex-back" @click="backFromCodex"><Icon name="back" :size="16" />{{ t('返回工作台') }}</button><span>{{ t('小工具 / Codex') }}</span></div>
        <header class="page-header codex-page-header"><h1>{{ t('更新 Codex') }}<span class="title-dot">.</span></h1></header>
        <template v-if="selected && currentState">
          <section class="codex-summary card" :aria-busy="codexBusy">
            <div class="codex-heading"><div class="codex-app-icon"><Icon name="chatgpt" :size="29" /></div><div class="codex-heading-text"><h2>Codex CLI</h2><span data-testid="codex-server">{{ selected.name }}<small>{{ selected.username }}@{{ selected.host }}:{{ selected.port }}</small></span></div><span class="status-badge" :class="!connected ? 'neutral' : codexError ? 'negative' : codexBusy ? 'pending' : codex?.updateAvailable ? 'pending' : codex?.installed ? 'positive' : 'neutral'" data-testid="codex-status" role="status"><i class="status-dot" :class="!connected ? 'neutral' : codexError ? 'negative' : codexBusy || codex?.updateAvailable ? 'pending' : codex?.installed ? 'positive' : 'neutral'" />{{ codexStatusLabel }}</span></div>
            <div class="codex-versions"><div><span>{{ t('服务器上的版本') }}</span><strong data-testid="codex-current-version">{{ codex?.currentVersion || (codex?.installed === false ? t("未安装") : t("尚未检测")) }}</strong><small>{{ codexMethodLabel(codex?.installMethod) }}</small></div><span class="codex-version-arrow" aria-hidden="true"><Icon name="arrow" :size="23" /></span><div><span>{{ t('最新稳定版') }}</span><strong data-testid="codex-latest-version">{{ codex?.latestVersion || t("尚未检测") }}</strong><small>{{ t('OpenAI 官方发布') }}</small></div></div>
            <div v-if="codex?.executablePath" class="codex-executable"><Icon name="folder" :size="14" /><span :title="codex.executablePath">{{ codex.executablePath }}</span></div>
            <div v-if="!connected" class="codex-note" data-testid="codex-connection-note" role="status"><Icon name="info" :size="17" /><div><strong>{{ t('连接服务器后即可检测和更新') }}</strong><p>{{ codex?.lastChecked ? t("当前显示上次检测结果。") : t("安装状态尚未确认。") }}</p></div><button v-if="!connectionActive" class="button secondary small" data-testid="codex-connect" :disabled="busy[`connect:${selected.id}`]" @click="requestConnection(selected)"><Icon name="link" :size="14" />{{ t('连接设备') }}</button><span v-else>{{ statusLabel(currentState.status) }}</span></div>
            <div v-if="codexError" class="codex-note error" data-testid="codex-error" role="alert"><Icon name="alert" :size="17" /><div><strong>{{ t('操作未完成') }}</strong><p>{{ message(codexError) }}</p></div></div>
            <div v-if="codex?.unsupportedReason" class="codex-note" data-testid="codex-unsupported"><Icon name="info" :size="17" /><p>{{ message(codex.unsupportedReason) }}</p></div>
            <div class="codex-actions"><span>{{ codex?.lastChecked ? t("检测于 {0}", { "0": timeText(codex.lastChecked, true) }) : t("进入页面后自动检测版本") }}</span><div><button class="button secondary" data-testid="codex-refresh" :disabled="!connected || codexBusy" @click="refreshCodex"><Icon name="refresh" :size="16" :class="{ spinning: codexBusy && !codexInstalling }" />{{ codexBusy && !codexInstalling ? t("正在检测") : t("检查版本") }}</button><button class="button primary" data-testid="codex-install" :disabled="!codexCanInstall" @click="installCodex"><Icon :name="codexInstalling ? 'refresh' : 'download'" :size="16" :class="{ spinning: codexInstalling }" />{{ codexActionLabel }}</button></div></div>
          </section>
          <section v-if="codex?.lastResult || codex?.restartMessage || codex?.log?.length" class="codex-result card" data-testid="codex-result"><div class="section-heading"><div class="heading-label"><span class="section-icon blue"><Icon name="activity" :size="19" /></span><h2>{{ codexInstalling ? t("操作进度") : t("操作结果") }}</h2></div><Icon v-if="codexInstalling" name="refresh" :size="16" class="spinning" /></div><div class="codex-result-body"><p v-if="codex.lastResult">{{ message(codex.lastResult) }}</p><div v-if="codex.restartMessage" class="codex-restart" :class="{ warning: ['manual', 'failed'].includes(codex.restartStatus || '') }" data-testid="codex-restart"><Icon :name="codex.restartStatus === 'restarted' ? 'check' : 'info'" :size="16" /><span>{{ message(codex.restartMessage) }}</span></div><ol v-if="codex.log?.length" class="codex-log" data-testid="codex-log"><li v-for="(line, index) in codex.log" :key="index">{{ message(line) }}</li></ol></div></section>
        </template>
        <section v-else class="codex-no-server card"><Icon name="server" :size="28" /><h2>{{ t('先选择一台服务器') }}</h2><p>{{ t('从左侧选择设备，或添加你的第一台设备。') }}</p><button class="button secondary" @click="openDevice()"><Icon name="plus" :size="15" />{{ t('添加设备') }}</button></section>
      </section>

      <template v-if="page === 'workspace' && selected && currentState">
        <section class="device-card card">
          <div class="device-identity"><div class="device-avatar"><Icon name="server" :size="27" /></div><div class="device-title"><div class="device-title-line"><h2>{{ selected.name }}</h2><span class="status-badge" :class="statusClass(currentState.status)"><i class="status-dot" :class="statusClass(currentState.status)" />{{ statusLabel(currentState.status) }}</span></div><div class="device-address">{{ selected.username }}@{{ selected.host }}<span>:{{ selected.port }}</span><span class="divider">/</span><Icon :name="selected.authType === 'key' ? 'key' : 'lock'" :size="13" />{{ selected.authType === 'key' ? t("密钥认证") : t("密码认证") }}</div></div></div>
          <div class="device-actions"><button class="icon-button" data-testid="edit-device" :title="t('编辑设备')" :aria-label="t('编辑设备')" @click="openDevice(selected)"><Icon name="edit" :size="18" /></button><button class="icon-button danger-hover" :title="t('移除设备')" :aria-label="t('移除设备')" @click="deleting = { kind: 'device', id: selected.id, name: selected.name }"><Icon name="trash" :size="18" /></button><span class="action-divider" /><button v-if="connectionActive" class="button secondary disconnect-button" :disabled="busy[`disconnect:${selected.id}`] || currentState.status === 'disconnecting'" @click="disconnect(selected)"><Icon name="power" :size="16" />{{ currentState.status === 'disconnecting' ? t("正在关闭") : t("断开连接") }}</button><button v-else data-testid="connect-button" class="button primary" :disabled="busy[`connect:${selected.id}`]" @click="requestConnection(selected)"><Icon name="link" :size="17" />{{ t('连接设备') }}</button></div>
          <div v-if="!showGpuCard" data-testid="gpu-detect-note" class="gpu-detect-note" role="status"><Icon :name="gpuDetecting ? 'refresh' : gpuAvailability === 'absent' ? 'info' : 'chip'" :size="14" :class="{ spinning: gpuDetecting }" /><span>{{ gpuDetectionText }}</span><small v-if="gpuDetectionDetail" :title="message(gpuDetectionDetail)">{{ message(gpuDetectionDetail) }}</small><span v-if="gpuAvailability === 'unavailable' && connected && selected.gpu.enabled" class="gpu-recheck-schedule">{{ t('每 60 秒重新检测') }}</span><button v-if="connected" data-testid="gpu-recheck" class="text-button" :disabled="busy.refresh || gpuDetecting" @click="action('refresh', () => api!.refreshGpu(selected!.id))">{{ t('重新检测') }}</button></div>
          <div v-if="slurmDetectionError" class="slurm-detect-note" data-testid="slurm-detect-note" role="status"><Icon name="info" :size="14" /><span>{{ t('作业监测待确认') }}</span><small :title="message(slurmDetectionError)">{{ message(slurmDetectionError) }}</small><button v-if="connected" class="text-button" :disabled="busy[`slurmRefresh:${selected.id}`]" @click="action(`slurmRefresh:${selected!.id}`, () => api!.refreshSlurm(selected!.id))">{{ t('重新检测') }}</button></div>
          <div v-if="currentState.error" class="inline-error"><Icon name="alert" :size="16" /><span>{{ message(currentState.error) }}</span><button v-if="selected.authType === 'key' && !connectionActive" class="text-button" @click="credentialProfile = selected; credentialPassphrase = ''; credentialPassword = ''; credentialRemember = false; focusFirst()">{{ t('填写密钥口令') }}</button></div>
        </section>

        <section v-if="showGpuCard" data-testid="gpu-card" class="gpu-card card">
          <div class="section-heading"><div class="heading-label"><span class="section-icon purple"><Icon name="chip" :size="20" /></span><h2>{{ t('显卡监测') }}</h2><span class="light-tag">NVIDIA</span></div><div class="monitor-controls"><span class="switch-label">{{ t('自动检测') }}</span><button class="switch" :class="{ on: selected.gpu.enabled }" role="switch" :aria-checked="selected.gpu.enabled" :aria-label="t('自动检测显卡')" :disabled="busy.gpuSettings" @click="applyGpu({ enabled: !selected.gpu.enabled })"><span /></button><span class="control-divider" /><label class="interval-control">{{ t('每') }} <input data-testid="gpu-interval" v-model.number="pollInterval" type="number" min="2" max="300" step="1" :aria-label="t('显卡检测间隔秒数')" :disabled="busy.gpuSettings" @change="applyGpu({ intervalSec: Math.round(pollInterval) })" /> {{ t('秒') }}</label><button class="icon-button" data-testid="gpu-refresh" :title="t('立即刷新显卡状态')" :aria-label="t('立即刷新显卡状态')" :disabled="!connected || busy.refresh || ['loading', 'polling'].includes(gpu?.status || '')" @click="action('refresh', () => api!.refreshGpu(selected!.id))"><Icon name="refresh" :size="18" :class="{ spinning: ['loading', 'polling'].includes(gpu?.status || '') }" /></button></div></div>
          <div class="gpu-summary-row"><div class="gpu-count"><span class="gpu-count-label">{{ hasGpuSample ? (gpuFresh ? gpu?.initialConfirming ? t("正在确认空闲") : t("可用算力") : t("上次采样")) : t("等待采样") }}</span><div class="gpu-number"><strong>{{ hasGpuSample && !gpu?.initialConfirming ? gpu?.idleCount : '—' }}</strong><span>/ {{ hasGpuSample ? gpu?.total : '—' }} <small>{{ t('张空闲') }}</small></span></div><p v-if="gpuSampleNotice">{{ gpuSampleNotice }}</p></div><div class="gpu-pulse-chart" aria-hidden="true"><div class="pulse-grid"/><svg viewBox="0 0 290 62" preserveAspectRatio="none"><path d="M0 42H39l9-8 14 9 20-4 12-20 13 38 13-26 13 5h34l9-8 13 14 12-5h69" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg><span>{{ connected && selected.gpu.enabled ? t("自动监测已开启") : t("监测待命") }}</span></div><div class="poll-clock"><span class="clock-circle gpu-progress-ring" :class="{ paused: !gpuTimerActive, sampling: gpuTimerSampling }" data-testid="gpu-progress-ring" :data-progress="gpuTimerProgress.toFixed(4)" :data-sampling="gpuTimerSampling" aria-hidden="true">
                <svg width="44" height="44" viewBox="0 0 44 44"><circle class="gpu-progress-track" cx="22" cy="22" r="18" pathLength="100" /><circle class="gpu-progress-value" cx="22" cy="22" r="18" pathLength="100" transform="rotate(-90 22 22)" :stroke-dasharray="gpuTimerSampling ? '25 100' : `${gpuTimerProgress * 100} 100`" :stroke-linecap="gpuTimerSampling || gpuTimerProgress > 0 ? 'round' : 'butt'" /></svg>
                <Icon class="gpu-progress-center" name="clock" :size="19" />
              </span><div><span class="clock-label">{{ t('下一次检测') }}</span><strong data-testid="gpu-countdown">{{ countdown }}</strong><small>{{ t('更新于') }} {{ timeText(gpu?.lastUpdated) }}</small></div></div></div>
          <div v-if="gpu?.error" class="gpu-warning"><Icon name="alert" :size="16" />{{ message(gpu.error) }}<span v-if="hasGpuSample"> {{ t('· 当前展示上次结果') }}</span></div>
          <div class="gpu-bottom"><div class="gpu-rule-note"><i class="status-dot" :class="gpuFresh ? 'positive' : 'neutral'" /><span>{{ gpuFresh ? t("采样正常") : hasGpuSample ? t("结果待更新") : t("尚无显卡数据") }}</span><span class="rule-divider">·</span><button class="text-button muted" @click="showGpuRules = !showGpuRules">{{ t('空闲判定规则') }}<Icon name="down" :size="12" :class="{ rotated: showGpuRules }" /></button></div><button class="text-button" :disabled="!gpu?.cards.length" @click="expandedGpu = !expandedGpu">{{ expandedGpu ? t("收起显卡详情") : t("查看显卡详情") }}<Icon name="down" :size="14" :class="{ rotated: expandedGpu }" /></button></div>
          <form v-if="showGpuRules" class="gpu-rules" @submit.prevent="applyGpu(gpuRules)"><label>{{ t('利用率 ≤') }}<span><input v-model.number="gpuRules.idleUtil" type="number" min="0" max="100" required /> %</span></label><label>{{ t('已用显存 ≤') }}<span><input v-model.number="gpuRules.idleMemoryMiB" type="number" min="0" max="131072" required /> MiB</span></label><label>{{ t('连续确认') }}<span><input v-model.number="gpuRules.stableSamples" type="number" min="1" max="10" required /> {{ t('次') }}</span></label><div class="rule-help">{{ t('同时要求没有计算进程。显示进程的少量占用允许存在。') }}</div><button class="button secondary small" type="submit" :disabled="busy.gpuSettings">{{ t('保存规则') }}</button></form>
          <div v-if="expandedGpu && gpu?.cards.length" class="gpu-detail-grid">
            <div v-for="card in gpu.cards" :key="card.uuid || card.index" class="gpu-detail" :class="{ idle: card.idle && gpuFresh }">
              <div class="gpu-detail-heading"><span>GPU {{ card.index }}</span><span class="mini-badge" :class="card.idle && gpuFresh ? 'positive' : card.idle ? 'neutral' : 'pending'">{{ cardLabel(card) }}</span></div>
              <strong>{{ card.name }}</strong>
              <div class="gpu-metrics">
                <div class="gpu-metric utilization-metric">
                  <div class="gpu-metric-label"><span>{{ t('GPU 利用率') }}</span><b>{{ percentText(metricPercent(card.utilization)) }}</b></div>
                  <div class="gpu-meter utilization-meter" data-testid="gpu-utilization-meter" role="progressbar" :aria-label="t('GPU 利用率')" aria-valuemin="0" aria-valuemax="100" :aria-valuenow="metricPercent(card.utilization) ?? undefined" :aria-valuetext="metricPercent(card.utilization) === null ? t('未知') : percentText(metricPercent(card.utilization))" :class="{ unknown: metricPercent(card.utilization) === null }"><span :style="{ width: `${metricPercent(card.utilization) ?? 0}%` }" /></div>
                </div>
                <div class="gpu-metric memory-metric">
                  <div class="gpu-metric-label"><span>{{ t('显存占用') }}</span><b>{{ numberText(card.memoryUsedMiB) }} MiB<span v-if="memoryPercent(card) !== null"> · {{ percentText(memoryPercent(card)) }}</span></b></div>
                  <div class="gpu-meter memory-meter" data-testid="gpu-memory-meter" role="progressbar" :aria-label="t('显存占用')" aria-valuemin="0" aria-valuemax="100" :aria-valuenow="memoryPercent(card) ?? undefined" :aria-valuetext="memoryPercent(card) === null ? t('未知') : `${numberText(card.memoryUsedMiB)} MiB · ${percentText(memoryPercent(card))}`" :class="{ unknown: memoryPercent(card) === null }"><span :style="{ width: `${memoryPercent(card) ?? 0}%` }" /></div>
                </div>
              </div>
              <small>{{ t('{count} 个计算进程 · 总显存 {memory} GiB', { count: processCount(card), memory: memoryTotal(card) }) }}</small>
            </div>
          </div>
        </section>

        <section class="storage-card card" data-testid="storage-card" :aria-busy="storageLoading">
          <div class="section-heading"><div class="heading-label"><span class="section-icon blue"><Icon name="folder" :size="20" /></span><h2>{{ t('我的空间') }}</h2><span class="light-tag">{{ selected.username }}</span></div><div class="storage-controls"><button class="button secondary small" data-testid="tmp-cleanup" :disabled="!connected || busy[`prepareCleanup:${selected.id}`] || busy.cleanupTmp" @click="requestTmpCleanup()"><Icon name="trash" :size="14" />{{ busy[`prepareCleanup:${selected.id}`] ? t("正在统计") : t("清理我的 /tmp") }}</button><button class="icon-button" data-testid="storage-refresh" :aria-label="t('立即刷新空间占用')" :title="t('立即刷新空间占用')" :disabled="!connected || storageLoading" @click="action(`storageRefresh:${selected!.id}`, () => api!.refreshStorage(selected!.id))"><Icon name="refresh" :size="18" :class="{ spinning: storageLoading }" /></button></div></div>
          <div class="storage-metrics"><div><span class="storage-path" :title="storage?.homePath || `/home/${selected.username}`">{{ storage?.homePath || `/home/${selected.username}` }}</span><strong data-testid="storage-home">{{ bytesText(storage?.homeBytes) }}</strong><small>{{ t('个人目录') }}</small></div><div><span class="storage-path">/tmp</span><strong data-testid="storage-tmp">{{ bytesText(storage?.tmpBytes) }}</strong><small>{{ t('仅统计属于我的项目') }}</small></div><div class="storage-total"><span class="storage-path">{{ t('合计') }}</span><strong data-testid="storage-total">{{ bytesText(storage?.totalBytes) }}</strong><small>{{ t('个人目录 + 我的临时文件') }}</small></div></div>
          <div v-if="storageWarning" class="storage-warning" role="status"><Icon name="info" :size="14" /><span>{{ message(storageWarning) }}</span></div>
          <div v-if="storage?.warnings?.length" class="storage-warnings"><span v-for="warning in storage.warnings" :key="warning">{{ message(warning) }}</span></div>
          <div class="storage-footer"><span><i v-if="storageLoading" class="status-dot pending" />{{ storageLoading ? t("正在统计空间占用\u2026") : connected ? t("每 {0} 分钟自动更新", { "0": Math.round((storage?.intervalSec || 300) / 60) }) : t("等待连接") }}</span><time v-if="storage?.lastUpdated">{{ t('更新于') }} {{ timeText(storage.lastUpdated) }}</time></div>
        </section>

        <section v-if="showSlurmCard" :ref="element => slurmCardElement = element as HTMLElement | null" class="slurm-card card" :class="{ 'slurm-card-sized': slurmCardHeight !== null, 'slurm-card-resizing': slurmResizing }" :style="{ minHeight: `${slurmMinimumHeight}px`, height: slurmCardHeight !== null ? `${slurmCardHeight}px` : undefined }" data-testid="slurm-card" :aria-busy="slurmLoading">
          <div class="section-heading">
            <div class="heading-label"><span class="section-icon blue"><Icon name="activity" :size="20" /></span><h2>{{ t('Slurm 作业') }}</h2><span class="count-tag">{{ slurmInitialLoading ? '—' : slurmJobs.length }}</span></div>
            <div class="slurm-controls">
              <div class="slurm-scope" role="group" :aria-label="t('作业查看范围')">
                <button data-testid="slurm-scope-mine" :class="{ active: slurmScope === 'mine' }" :aria-pressed="slurmScope === 'mine'" :disabled="!connected || busy[`slurmScope:${selected.id}`]" @click="changeSlurmScope('mine')">{{ t('个人') }}</button>
                <button data-testid="slurm-scope-all" :class="{ active: slurmScope === 'all' }" :aria-pressed="slurmScope === 'all'" :disabled="!connected || busy[`slurmScope:${selected.id}`]" @click="changeSlurmScope('all')">{{ t('全部') }}</button>
              </div>
              <button class="icon-button" data-testid="slurm-refresh" :title="t('立即刷新作业')" :aria-label="t('立即刷新作业')" :disabled="!connected || slurmLoading || busy[`slurmRefresh:${selected.id}`]" @click="action(`slurmRefresh:${selected!.id}`, () => api!.refreshSlurm(selected!.id))"><Icon name="refresh" :size="18" :class="{ spinning: slurmLoading }" /></button>
            </div>
          </div>
          <div v-if="slurmWarning" class="slurm-warning" role="status"><Icon name="info" :size="15" /><span>{{ message(slurmWarning) }}</span></div>
          <div v-if="slurmInitialLoading" class="slurm-empty" data-testid="slurm-loading" role="status"><Icon name="refresh" :size="19" class="spinning" /><span>{{ t(slurmScope === 'mine' ? '正在读取个人作业…' : '正在读取全部作业…') }}</span></div>
          <div v-else-if="slurmJobs.length" class="slurm-table-wrap" tabindex="0" role="region" :aria-label="t('Slurm 作业列表，可横向滚动')">
            <table class="slurm-table" data-testid="slurm-table">
              <thead><tr><th scope="col">{{ t('作业编号') }}</th><th scope="col">{{ t('分区') }}</th><th scope="col">{{ t('作业名称') }}</th><th scope="col">{{ t('用户') }}</th><th scope="col">{{ t('状态') }}</th><th scope="col">{{ t('已运行时间') }}</th><th scope="col">{{ t('节点数') }}</th><th scope="col">{{ t('节点列表') }}</th><th scope="col">{{ t('原因') }}</th><th scope="col">{{ t('权重') }}</th><th scope="col" class="align-right">{{ t('操作') }}</th></tr></thead>
              <tbody><tr v-for="job in slurmPageRows" :key="`${job.cluster || ''}:${job.id}:${job.partition}`">
                <td class="slurm-job-id"><span :title="job.id">{{ job.id }}</span><small v-if="job.cluster" :title="job.cluster">{{ job.cluster }}</small></td>
                <td><span class="slurm-cell-text" :title="job.partition">{{ job.partition || '—' }}</span></td>
                <td><span class="slurm-cell-text slurm-job-name" :title="job.name">{{ job.name || '—' }}</span></td>
                <td><span class="slurm-cell-text" :title="job.user">{{ job.user || '—' }}</span></td>
                <td><span class="slurm-job-state" :class="job.stateTone" :title="slurmStateTitle(job)"><i class="status-dot" :class="job.stateTone" />{{ message(job.stateLabel) }}</span></td>
                <td class="slurm-elapsed">{{ job.elapsed || '—' }}</td>
                <td class="slurm-node-count">{{ numberText(job.nodes) }}</td>
                <td><span class="slurm-cell-text" :title="job.nodeList">{{ job.nodeList || '—' }}</span></td>
                <td class="slurm-job-reason"><span :title="slurmReasonTitle(job)">{{ message(job.reasonLabel) || '—' }}</span></td>
                <td class="slurm-priority"><button v-if="priorities[priorityKey(job)]?.value !== undefined" class="text-button priority-value" :data-testid="`job-priority-${job.id}`" :title="priorities[priorityKey(job)]?.values?.length ? t('各作业权重：{0}；点击重新读取', { '0': priorities[priorityKey(job)]?.values?.join('、') }) : t('点击重新读取权重')" :disabled="!connected || busy[`priority:${priorityKey(job)}`]" @click="requestPriority(job)">{{ priorities[priorityKey(job)]?.value === null ? priorities[priorityKey(job)]?.values?.length ? t("多值") : '—' : priorities[priorityKey(job)]?.value?.toLocaleString() }}</button><button v-else class="text-button" :data-testid="`job-priority-query-${job.id}`" :title="message(priorities[priorityKey(job)]?.error) || t('读取该作业的调度权重')" :disabled="!connected || busy[`priority:${priorityKey(job)}`]" @click="requestPriority(job)">{{ busy[`priority:${priorityKey(job)}`] ? t("读取中") : priorities[priorityKey(job)]?.error ? t("重试") : t("查看") }}</button></td>
                <td class="align-right"><button v-if="ownsJob(job)" class="icon-button danger-hover" :data-testid="`job-cancel-${job.id}`" :title="t('删除自己的作业 {0}', { '0': job.id })" :aria-label="t('删除自己的作业 {0}', { '0': job.id })" :disabled="!connected || busy[`prepareCancel:${priorityKey(job)}`] || busy.cancelJob" @click="requestJobCancellation(job)"><Icon :name="busy[`prepareCancel:${priorityKey(job)}`] ? 'refresh' : 'trash'" :size="16" :class="{ spinning: busy[`prepareCancel:${priorityKey(job)}`] }" /></button><span v-else class="slurm-no-action">—</span></td>
              </tr></tbody>
              <tfoot v-if="slurmCardHeight !== null" class="slurm-grid-fill" aria-hidden="true"><tr><td v-for="column in 11" :key="column" /></tr></tfoot>
            </table>
          </div>
          <div v-else class="slurm-empty" data-testid="slurm-empty"><Icon :name="slurm?.error ? 'info' : 'check'" :size="19" /><span>{{ slurm?.error ? t("暂时无法读取作业") : slurmScope === 'mine' ? t("该用户暂无作业") : t("当前暂无可见作业") }}</span></div>
          <div class="slurm-footer"><span>{{ slurmScope === 'mine' ? t("当前用户 {0}", { "0": selected.username }) : t("服务器允许查看的全部作业") }}<span class="slurm-footer-divider">·</span>{{ connected ? t("每 {0} 秒自动更新", { "0": slurm?.intervalSec || 30 }) : t("等待连接") }}</span><div class="slurm-footer-actions"><div v-if="slurmJobs.length > slurmPageSize" class="slurm-pagination" data-testid="slurm-pagination" role="group" :aria-label="t('作业列表分页')"><button class="icon-button previous" data-testid="slurm-prev-page" :title="t('上一页作业')" :aria-label="t('上一页作业')" :disabled="slurmPage <= 1" @click="slurmPage--"><Icon name="chevron" :size="14" /></button><span data-testid="slurm-page-number" aria-live="polite">{{ slurmPage }} / {{ slurmPageCount }} {{ t('页') }}</span><button class="icon-button" data-testid="slurm-next-page" :title="t('下一页作业')" :aria-label="t('下一页作业')" :disabled="slurmPage >= slurmPageCount" @click="slurmPage++"><Icon name="chevron" :size="14" /></button></div><time v-if="slurm?.lastUpdated">{{ t('更新于') }} {{ timeText(slurm.lastUpdated) }}</time></div></div>
          <div class="slurm-resize-handle" data-testid="slurm-resize-handle" role="separator" tabindex="0" aria-orientation="horizontal" :aria-label="t('调整 Slurm 卡片高度')" :aria-valuemin="slurmMinHeight" :aria-valuemax="slurmMaxHeight" :aria-valuenow="slurmCurrentHeight" :title="t('拖动调整高度，双击恢复默认；也可使用上下方向键')" @pointerdown="startSlurmResize" @pointermove="moveSlurmResize" @pointerup="finishSlurmResize" @pointercancel="finishSlurmResize" @lostpointercapture="finishSlurmResize" @keydown="resizeSlurmKey" @dblclick="resetSlurmHeight"><span aria-hidden="true" /></div>
        </section>

        <section class="tunnel-card card"><div class="section-heading"><div class="heading-label"><span class="section-icon blue"><Icon name="exchange" :size="20" /></span><h2>{{ t('端口隧道') }}</h2><span class="count-tag">{{ selected.tunnels.length }}</span></div><button data-testid="add-tunnel" class="button secondary small" @click="openTunnel()"><Icon name="plus" :size="15" />{{ t('添加隧道') }}</button></div>
          <div v-if="selected.tunnels.length" class="tunnel-table-wrap"><table class="tunnel-table"><thead><tr><th>{{ t('名称 / 类型') }}</th><th>{{ t('端口映射') }}</th><th>{{ t('状态') }}</th><th class="align-right">{{ t('操作') }}</th></tr></thead><tbody><tr v-for="tunnel in selected.tunnels" :key="tunnel.id"><td><strong class="tunnel-name">{{ tunnel.name }}</strong><span class="direction-label" :class="tunnel.direction"><Icon name="arrow" :size="11" />{{ tunnel.direction === 'local' ? t("正向隧道") : t("反向隧道") }}</span></td><td class="mapping-cell"><div class="mapping-route"><span><small>{{ tunnel.direction === 'local' ? t("本机") : t("服务器") }}</small><strong>{{ tunnel.bindPort }}</strong></span><Icon name="arrow" :size="17" /><span><small>{{ tunnel.direction === 'local' ? t("远端") : t("本机侧") }}</small><strong>{{ tunnel.targetPort }}</strong></span></div><span class="mapping-address" :title="`${tunnel.bindHost} → ${tunnel.targetHost}`">{{ tunnel.bindHost }} → {{ tunnel.targetHost }}</span></td><td><div class="tunnel-status" :class="!tunnel.enabled ? 'neutral' : statusClass(tunnelState(tunnel)?.status)"><i class="status-dot" :class="!tunnel.enabled ? 'neutral' : statusClass(tunnelState(tunnel)?.status)" />{{ tunnelStatus(tunnel) }}</div><small class="tunnel-error" :title="tunnelDetail(tunnel)">{{ tunnelDetail(tunnel) }}</small></td><td><div class="tunnel-actions"><button class="switch small-switch" :class="{ on: tunnel.enabled }" role="switch" :aria-checked="tunnel.enabled" :aria-label="t('{0}隧道 {1}', { '0': tunnel.enabled ? t('暂停') : t('启用'), '1': tunnel.name })" :disabled="busy[`tunnel:${tunnel.id}`]" @click="toggleTunnel(tunnel)"><span /></button><button class="icon-button" :title="t('重启隧道 {0}', { '0': tunnel.name })" :aria-label="t('重启隧道 {0}', { '0': tunnel.name })" :disabled="!connected || !tunnel.enabled || busy[`restart:${tunnel.id}`]" @click="action(`restart:${tunnel.id}`, () => api!.restartTunnel(selected!.id, tunnel.id))"><Icon name="refresh" :size="16" /></button><button class="icon-button" :title="t('编辑隧道 {0}', { '0': tunnel.name })" :aria-label="t('编辑隧道 {0}', { '0': tunnel.name })" @click="openTunnel(tunnel)"><Icon name="edit" :size="16" /></button><button class="icon-button danger-hover" :title="t('移除隧道 {0}', { '0': tunnel.name })" :aria-label="t('移除隧道 {0}', { '0': tunnel.name })" @click="deleting = { kind: 'tunnel', id: tunnel.id, name: tunnel.name }"><Icon name="trash" :size="16" /></button></div></td></tr></tbody></table></div>
          <div v-else class="tunnel-empty"><span class="empty-tunnel-icon"><Icon name="exchange" :size="25" /></span><div><strong>{{ t('为这台设备添加第一条隧道') }}</strong><p>{{ t('正向访问远端服务，或反向连接本机服务。') }}</p></div><button class="text-button" @click="openTunnel()">{{ t('添加隧道') }}<Icon name="arrow" :size="15" /></button></div>
          <div class="tunnel-footer"><Icon name="shield" :size="14" /><span>{{ t('异常时先关闭旧转发，再恢复连接。远端端口未释放时会自动等待重试。') }}</span></div>
        </section>

        <section class="tools-card card" data-testid="tools-card"><div class="section-heading"><div class="heading-label"><span class="section-icon blue"><Icon name="tools" :size="19" /></span><h2>{{ t('小工具') }}</h2></div><span class="tools-caption">{{ selected.name }}</span></div><div class="tools-grid"><button class="tool-entry" data-testid="tool-codex" @click="openCodex"><span class="tool-entry-icon"><Icon name="chatgpt" :size="26" /></span><span class="tool-entry-text"><strong>{{ t('更新 Codex') }}</strong><small>{{ t('查看版本、安装与更新') }}</small></span><Icon name="chevron" :size="16" /></button><button class="tool-entry" data-testid="tool-terminal" @click="openTerminal"><span class="tool-entry-icon"><Icon name="terminal" :size="26" /></span><span class="tool-entry-text"><strong>{{ t('终端') }}</strong><small>{{ t('连接服务器、保留多个会话') }}</small></span><Icon name="chevron" :size="16" /></button></div></section>

        <section class="events-card card"><div class="section-heading"><div class="heading-label"><span class="section-icon gray"><Icon name="activity" :size="19" /></span><h2>{{ t('系统日志') }}</h2><span class="light-tag">{{ selected.name }}</span></div></div><div v-if="recentEvents.length" class="event-list" role="log" :aria-label="t('系统日志')"><div v-for="event in recentEvents" :key="event.id" class="event-line"><time>{{ timeText(event.time) }}</time><span class="event-level" :class="['error', 'warn', 'warning'].includes(event.level) ? event.level === 'error' ? 'negative' : 'pending' : 'positive'"><Icon :name="event.level === 'error' ? 'alert' : ['warn', 'warning'].includes(event.level) ? 'info' : 'check'" :size="13" /></span><span>{{ message(event.message) }}</span></div></div><div v-else class="event-empty"><i class="status-dot neutral" />{{ t('暂无事件，连接设备后将在这里记录状态变化。') }}</div></section>
      </template>
      <section v-else-if="page === 'workspace'" class="welcome-empty card"><div class="welcome-icon"><Icon name="server" :size="40" /></div><span class="eyebrow">A QUIET PLACE FOR YOUR SERVERS</span><h2>{{ t('从你的第一台设备开始') }}</h2><p>{{ t('安全连接多台服务器，管理隧道，随时了解空闲算力。') }}</p><button class="button primary" @click="openDevice()"><Icon name="plus" :size="17" />{{ t('添加设备') }}</button><div class="welcome-features"><span><Icon name="shield" :size="16" />{{ t('密码与密钥认证') }}</span><span><Icon name="exchange" :size="16" />{{ t('双向隧道与自动恢复') }}</span><span><Icon name="chip" :size="16" />{{ t('定时监测显卡') }}</span></div></section>
      <footer class="workspace-footer"><span>Lion SSH</span></footer>
    </main>
  </div>

  <Transition name="drawer"><div v-if="drawer" class="drawer-backdrop" @click.self="closeDrawer()"><section class="drawer" role="dialog" aria-modal="true" :aria-labelledby="drawer === 'device' ? 'device-form-title' : 'tunnel-form-title'"><div class="drawer-header"><div><span class="eyebrow">{{ drawer === 'device' ? 'DEVICE SETTINGS' : 'TUNNEL SETTINGS' }}</span><h2 :id="drawer === 'device' ? 'device-form-title' : 'tunnel-form-title'">{{ t(editing ? drawer === 'device' ? '编辑设备' : '编辑隧道' : drawer === 'device' ? '添加设备' : '添加隧道') }}</h2></div><button class="icon-button" :aria-label="t('关闭设置')" @click="closeDrawer()"><Icon name="close" /></button></div>
      <form v-if="drawer === 'device'" class="drawer-form" @submit.prevent="saveDevice"><div class="form-body"><div class="form-section-title"><Icon name="server" :size="16" />{{ t('连接信息') }}</div><label class="field">{{ t('设备名称') }}<input v-model="deviceDraft.name" :placeholder="t('例如：训练服务器')" maxlength="80" required /></label><div class="form-row host-row"><label class="field">{{ t('服务器地址') }}<input v-model="deviceDraft.host" :placeholder="t('IP 地址或主机名')" maxlength="255" required /></label><label class="field">{{ t('端口') }}<input v-model.number="deviceDraft.port" type="number" min="1" max="65535" required /></label></div><label class="field">{{ t('用户名') }}<input v-model="deviceDraft.username" :placeholder="t('例如：user')" maxlength="128" required autocomplete="off" /></label><div class="form-section-title"><Icon name="shield" :size="16" />{{ t('认证方式') }}</div><div class="segmented-control"><button type="button" :class="{ active: deviceDraft.authType === 'key' }" @click="deviceDraft.authType = 'key'"><Icon name="key" :size="16" />{{ t('SSH 密钥') }}</button><button type="button" :class="{ active: deviceDraft.authType === 'password' }" @click="deviceDraft.authType = 'password'"><Icon name="lock" :size="16" />{{ t('密码') }}</button></div><template v-if="deviceDraft.authType === 'key'"><label class="field">{{ t('私钥文件') }}<div class="input-with-button"><input v-model="deviceDraft.keyPath" :placeholder="t('选择本机私钥文件')" required /><button type="button" class="icon-button" :title="t('选择私钥文件')" :aria-label="t('选择私钥文件')" @click="chooseKey()"><Icon name="folder" :size="18" /></button></div><small>{{ t('支持 OpenSSH 私钥，文件内容仅用于建立连接。') }}</small></label><label class="field">{{ t('密钥口令') }}<span class="field-optional">{{ t('可选') }}</span><input v-model="devicePassphrase" type="password" :placeholder="editing ? t('留空使用已有口令或未加密密钥') : t('私钥未加密时留空')" autocomplete="new-password" /></label></template><label v-else class="field">{{ t('连接密码') }}<div class="input-with-button"><input v-model="devicePassword" :type="passwordVisible ? 'text' : 'password'" :placeholder="editing ? t('留空保留已有密码') : t('可稍后在连接时输入')" autocomplete="new-password" /><button type="button" class="icon-button" :aria-label="passwordVisible ? t('隐藏密码') : t('显示密码')" @click="passwordVisible = !passwordVisible"><Icon name="eye" :size="17" /></button></div></label><label class="checkbox-row"><input v-model="rememberSecret" type="checkbox" /><span>{{ t(deviceDraft.authType === 'password' ? '记住密码' : '记住密钥口令') }}<small>{{ t('使用系统加密存储；不勾选则仅在本次运行中使用。') }}</small></span></label><div class="form-section-title"><Icon name="activity" :size="16" />{{ t('连接与监测') }}</div><label class="checkbox-row"><input v-model="deviceDraft.autoReconnect" type="checkbox" /><span>{{ t('断线后自动重连') }}<small>{{ t('保留配置，关闭旧会话后按递增间隔恢复。') }}</small></span></label><label class="checkbox-row"><input v-model="deviceDraft.gpu.enabled" type="checkbox" /><span>{{ t('自动监测 NVIDIA 显卡') }}<small>{{ t('只执行 nvidia-smi 读取显卡状态。') }}</small></span></label><label class="field inline-field">{{ t('检测间隔') }}<div><input v-model.number="deviceDraft.gpu.intervalSec" type="number" min="2" max="300" required /><span>{{ t('秒') }}</span></div></label><div class="form-note"><Icon name="info" :size="17" /><span>{{ t('首次连接需要确认服务器指纹。连接后，隧道和显卡监测会随设备独立运行。') }}</span></div><div v-if="formError" class="form-error" role="alert">{{ message(formError) }}</div></div><div class="drawer-footer"><button type="button" class="button secondary" @click="closeDrawer()">{{ t('取消') }}</button><button type="submit" data-testid="device-save" class="button primary" :disabled="busy.save">{{ busy.save ? t("正在保存") : t("保存设备") }}<Icon name="check" :size="16" /></button></div></form>
      <form v-else class="drawer-form" @submit.prevent="saveTunnel"><div class="form-body"><div class="form-context"><Icon name="server" :size="16" />{{ selected?.name }}<span>{{ selected?.host }}</span></div><label class="field">{{ t('隧道名称') }}<span class="field-optional">{{ t('可选') }}</span><input v-model="tunnelDraft.name" :placeholder="t('例如：TensorBoard')" maxlength="80" /></label><label class="field">{{ t('转发方向') }}</label><div class="segmented-control"><button type="button" :class="{ active: tunnelDraft.direction === 'local' }" @click="tunnelDraft.direction = 'local'"><Icon name="arrow" :size="16" />{{ t('正向隧道') }}</button><button type="button" :class="{ active: tunnelDraft.direction === 'remote' }" @click="tunnelDraft.direction = 'remote'"><Icon name="exchange" :size="16" />{{ t('反向隧道') }}</button></div><div class="direction-explanation"><span>{{ tunnelDraft.direction === 'local' ? t("本机端口") : t("服务器端口") }}</span><Icon name="arrow" :size="16" /><span>SSH</span><Icon name="arrow" :size="16" /><span>{{ tunnelDraft.direction === 'local' ? t("远端服务") : t("本机侧服务") }}</span></div><div class="form-section-title"><Icon name="link" :size="16" />{{ tunnelDraft.direction === 'local' ? t("本机监听") : t("服务器监听") }}</div><div class="form-row host-row"><label class="field">{{ t('监听地址') }}<input v-model="tunnelDraft.bindHost" placeholder="127.0.0.1" required maxlength="255" /></label><label class="field">{{ t('监听端口') }}<input v-model.number="tunnelDraft.bindPort" type="number" min="1" max="65535" required /></label></div><div class="form-section-title"><Icon name="arrow" :size="16" />{{ tunnelDraft.direction === 'local' ? t("远端目标") : t("本机侧目标") }}</div><div class="form-row host-row"><label class="field">{{ t('目标地址') }}<input v-model="tunnelDraft.targetHost" placeholder="127.0.0.1" required maxlength="255" /></label><label class="field">{{ t('目标端口') }}<input v-model.number="tunnelDraft.targetPort" type="number" min="1" max="65535" required /></label></div><label class="field inline-field">{{ t('连通性检测间隔') }}<div><input v-model.number="tunnelDraft.healthIntervalSec" type="number" min="5" max="300" required /><span>{{ t('秒') }}</span></div></label><label class="checkbox-row"><input v-model="tunnelDraft.enabled" type="checkbox" /><span>{{ t('启用这条隧道') }}<small>{{ t('设备连接后自动建立；手动暂停后不会自动重启。') }}</small></span></label><div class="form-note"><Icon name="info" :size="17" /><span>{{ tunnelDraft.direction === 'remote' ? t("远端监听地址能否对外访问取决于服务器的转发权限。无法探测完整路径时，状态会显示未验证。") : t("目标地址从服务器一侧访问。目标服务未启动时会提示不可用，并继续检测。") }}</span></div><div v-if="formError" class="form-error" role="alert">{{ message(formError) }}</div></div><div class="drawer-footer"><button type="button" class="button secondary" @click="closeDrawer()">{{ t('取消') }}</button><button type="submit" data-testid="tunnel-save" class="button primary" :disabled="busy.save">{{ busy.save ? t("正在保存") : t("保存隧道") }}<Icon name="check" :size="16" /></button></div></form>
    </section></div></Transition>

  <div v-if="credentialProfile" class="modal-backdrop" @click.self="credentialProfile = null"><form class="modal credential-modal" role="dialog" aria-modal="true" aria-labelledby="credential-title" @submit.prevent="submitCredentials"><div class="modal-top"><span class="modal-icon blue"><Icon name="key" :size="25" /></span><button class="icon-button" type="button" :aria-label="t('关闭认证')" @click="credentialProfile = null"><Icon name="close" :size="19" /></button></div><h2 id="credential-title">{{ t('连接') }} {{ credentialProfile.name }}</h2><p>{{ credentialProfile.username }}@{{ credentialProfile.host }}:{{ credentialProfile.port }}</p><label v-if="credentialProfile.authType === 'password'" class="field">{{ t('连接密码') }}<input v-model="credentialPassword" type="password" :placeholder="t('输入密码，或留空使用已记住的密码')" autocomplete="current-password" /></label><label v-else class="field">{{ t('密钥口令') }}<input v-model="credentialPassphrase" type="password" :placeholder="t('未加密私钥可留空')" autocomplete="current-password" /></label><label class="checkbox-row"><input v-model="credentialRemember" type="checkbox" /><span>{{ t('使用系统加密记住凭据') }}</span></label><div class="modal-actions"><button type="button" class="button secondary" @click="credentialProfile = null">{{ t('取消') }}</button><button type="submit" class="button primary"><Icon name="link" :size="16" />{{ t('连接') }}</button></div></form></div>
  <div v-if="deleting" class="modal-backdrop" @click.self="!busy.delete && (deleting = null)"><section class="modal confirm-modal" role="alertdialog" aria-modal="true" aria-labelledby="delete-title"><span class="modal-icon red"><Icon name="trash" :size="25" /></span><h2 id="delete-title">{{ t(deleting.kind === 'device' ? '移除设备？' : '移除隧道？') }}</h2><p>{{ t(deleting.kind === 'device' ? '「{name}」的连接会先关闭，然后移除设备配置。' : '「{name}」的转发会先停止，然后移除隧道配置。', { name: deleting.name }) }}</p><div class="modal-actions"><button class="button secondary" :disabled="busy.delete" @click="deleting = null">{{ t('取消') }}</button><button class="button danger" :disabled="busy.delete" @click="deleteConfirmed()">{{ busy.delete ? t("正在移除") : t("确认移除") }}</button></div></section></div>
  <div v-if="jobCancellation" class="modal-backdrop" @click.self="!busy.cancelJob && (jobCancellation = null)"><section class="modal confirm-modal" data-testid="job-cancel-confirmation" role="alertdialog" aria-modal="true" aria-labelledby="job-cancel-title"><span class="modal-icon red"><Icon name="trash" :size="25" /></span><h2 id="job-cancel-title">{{ t('删除自己的作业？') }}</h2><p>{{ t('将取消排队中的作业，或终止正在运行的作业。') }}</p><dl class="confirmation-details"><div><dt>{{ t('服务器') }}</dt><dd>{{ jobCancellation.profileName }}</dd></div><div><dt>{{ t('作业编号') }}</dt><dd>{{ jobCancellation.preview.jobId }}</dd></div><div><dt>{{ t('作业名称') }}</dt><dd>{{ jobCancellation.jobName || '—' }}</dd></div><div><dt>{{ t('所属用户') }}</dt><dd>{{ jobCancellation.preview.owner }}</dd></div></dl><div v-if="confirmationExpired" class="confirmation-warning" role="status">{{ t('确认已过期，请取消后重新检查作业。') }}</div><div class="modal-actions"><button class="button secondary" data-testid="job-cancel-dismiss" :disabled="busy.cancelJob" @click="jobCancellation = null">{{ t('取消') }}</button><button class="button danger" data-testid="job-cancel-approve" :disabled="busy.cancelJob || confirmationExpired" @click="cancelJobConfirmed()">{{ busy.cancelJob ? t("正在删除") : t("确认删除作业") }}</button></div></section></div>
  <div v-if="tmpCleanup" class="modal-backdrop" @click.self="!busy.cleanupTmp && (tmpCleanup = null)"><section class="modal confirm-modal cleanup-modal" data-testid="tmp-cleanup-confirmation" role="alertdialog" aria-modal="true" aria-labelledby="tmp-cleanup-title"><span class="modal-icon red"><Icon name="trash" :size="25" /></span><h2 id="tmp-cleanup-title">{{ t('清理自己的临时文件？') }}</h2><p>{{ t('将删除 /tmp 内属于你的文件及可移除的空目录，可能影响正在使用这些文件的程序。此操作无法撤销。') }}</p><dl class="confirmation-details"><div><dt>{{ t('服务器') }}</dt><dd>{{ tmpCleanup.profileName }}</dd></div><div><dt>{{ t('目录') }}</dt><dd>{{ tmpCleanup.preview.root }}</dd></div><div><dt>{{ t('所属用户') }}</dt><dd>{{ t('{user}（UID {uid}）', { user: tmpCleanup.preview.username, uid: tmpCleanup.preview.uid }) }}</dd></div><div><dt>{{ t('文件及链接') }}</dt><dd>{{ tmpCleanup.preview.fileCount.toLocaleString() }} {{ t('项') }}<small>{{ t('另清理可移除的本人空目录') }}</small></dd></div><div><dt>{{ t('我的空间占用') }}</dt><dd>{{ bytesText(tmpCleanup.preview.bytes) }}</dd></div></dl><div v-if="tmpCleanup.preview.warnings?.length" class="confirmation-warning"><div v-for="warning in tmpCleanup.preview.warnings" :key="warning">{{ message(warning) }}</div></div><div v-if="confirmationExpired" class="confirmation-warning" role="status">{{ t('确认已过期，请取消后重新统计文件。') }}</div><div class="modal-actions"><button class="button secondary" data-testid="tmp-cleanup-dismiss" :disabled="busy.cleanupTmp" @click="tmpCleanup = null">{{ t('取消') }}</button><button class="button danger" data-testid="tmp-cleanup-approve" :disabled="busy.cleanupTmp || confirmationExpired || !tmpCleanup.preview.fileCount" @click="cleanupTmpConfirmed()">{{ busy.cleanupTmp ? t("正在清理") : t("确认清理 /tmp") }}</button></div></section></div>
  <div v-if="hostRequest" class="modal-backdrop host-backdrop"><section class="modal host-modal" role="alertdialog" aria-modal="true" aria-labelledby="host-title"><span class="modal-icon" :class="hostRequest.changed ? 'red' : 'blue'"><Icon :name="hostRequest.changed ? 'alert' : 'shield'" :size="28" /></span><h2 id="host-title">{{ hostRequest.changed ? t("服务器指纹已变化") : t("确认服务器身份") }}</h2><p>{{ hostRequest.changed ? t("当前服务器密钥与此前记录不同。请核对来源，确认服务器是否更换过密钥。") : t("首次连接这台设备，请与服务器管理员提供的指纹核对。确认后会记住此服务器身份。") }}</p><div class="fingerprint-box"><span>{{ hostRequest.host }}:{{ hostRequest.port }}<small>{{ hostRequest.keyType }}</small></span><code>{{ hostRequest.fingerprint }}</code></div><div class="modal-actions"><button class="button secondary" :disabled="busy.trust" @click="trustHost(false)">{{ t('取消连接') }}</button><button class="button" :class="hostRequest.changed ? 'danger' : 'primary'" data-testid="host-accept" :disabled="busy.trust" @click="trustHost(true)">{{ t('确认并信任') }}<Icon name="check" :size="16" /></button></div></section></div>
  <div v-if="shutdown" class="shutdown-backdrop" data-testid="shutdown-overlay"><section class="shutdown-panel" role="status" aria-live="assertive"><span class="shutdown-spinner" aria-hidden="true" /><h2>{{ t('正在安全退出') }}</h2><p>{{ message(shutdown.message) }}</p><small>{{ t('连接关闭后，软件会自动退出') }}</small></section></div>
  <div class="toast-stack" :inert="Boolean(shutdown)" role="status" aria-live="polite"><TransitionGroup name="toast"><div v-for="notice in notices" :key="notice.id" class="toast" :class="{ error: notice.error }"><Icon :name="notice.error ? 'alert' : 'check'" :size="18" /><span>{{ noticeText(notice.message) }}</span><button class="icon-button" :aria-label="t('关闭通知')" @click="notices = notices.filter(n => n.id !== notice.id)"><Icon name="close" :size="15" /></button></div></TransitionGroup></div>
</template>

<style scoped>
.language-control { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-bottom: 10px; color: var(--text-secondary); font-size: 11px; }
.language-control select { min-width: 104px; max-width: 126px; padding: 6px 8px; color: var(--text); background: var(--surface-input); border: 1px solid var(--surface-border); border-radius: 8px; font: inherit; cursor: pointer; }
.language-control select:focus-visible { outline: 2px solid var(--blue); outline-offset: 2px; }
</style>
