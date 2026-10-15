<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue';
import Icon from './components/Icon.vue';
import { api as desktopApi } from './api';
import type { GpuCard, GpuSettings, HostKeyRequest, JobCancelPreview, Profile, Secrets, ServerState, ShutdownNotice, SlurmJob, SlurmScope, ThemePreference, ThemeState, TmpCleanupPreview, Tunnel, UpdateState } from './types';

const api = window.lion ?? desktopApi;
const preview = !api;
const systemTheme = window.matchMedia('(prefers-color-scheme: dark)');
const theme = ref<ThemeState>({ preference: cachedTheme(), resolved: 'light' });
let themeRevision = 0;
const themeOptions: { value: ThemePreference; label: string; icon: string; title: string }[] = [
  { value: 'light', label: '白天', icon: 'sun', title: '白天主题' },
  { value: 'dark', label: '夜晚', icon: 'moon', title: '夜晚主题' },
  { value: 'system', label: '系统', icon: 'monitor', title: '跟随系统主题' },
];
const shutdown = ref<ShutdownNotice | null>(null);
applyTheme(api?.initialTheme || { preference: theme.value.preference, resolved: theme.value.preference === 'system' ? systemTheme.matches ? 'dark' : 'light' : theme.value.preference });
const profiles = ref<Profile[]>([]);
const states = reactive<Record<string, ServerState>>({});
const selectedId = ref('');
const version = ref('1.0.0');
const update = ref<UpdateState | null>(null);
let updateRevision = 0;
const updatePending = computed(() => Boolean(update.value?.version && update.value.version !== version.value));
const updating = computed(() => Boolean(busy.update || ['downloading', 'ready', 'installing'].includes(update.value?.status || '')));
const updateProgress = computed(() => update.value?.total ? `${Math.min(100, Math.floor(update.value.downloaded * 100 / update.value.total))}%` : '下载中');
const loading = ref(true);
const startupError = ref('');
const now = ref(Date.now());
const busy = reactive<Record<string, boolean>>({});
const pendingSlurmScopes = reactive<Record<string, SlurmScope>>({});
const slurmPage = ref(1);
const slurmPageSize = 100;
const notices = ref<{ id: number; message: string; error: boolean }[]>([]);
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
const deleting = ref<{ kind: 'device' | 'tunnel'; id: string; name: string } | null>(null);
const jobCancellation = ref<{ profileId: string; profileName: string; jobName: string; preview: JobCancelPreview } | null>(null);
const tmpCleanup = ref<{ profileId: string; profileName: string; preview: TmpCleanupPreview } | null>(null);
const priorities = reactive<Record<string, { value?: number | null; values?: number[]; error?: string }>>({});
const connectionGenerations: Record<string, number> = {};
const hostRequests = ref<HostKeyRequest[]>([]);
const runtimeSecrets: Record<string, Secrets> = {};
let clockId: ReturnType<typeof setInterval>;
let removeState: (() => void) | undefined;
let removeHost: (() => void) | undefined;
let removeTheme: (() => void) | undefined;
let removeShutdown: (() => void) | undefined;
let removeBackendError: (() => void) | undefined;
let removeUpdate: (() => void) | undefined;
let initializingStates: Set<string> | undefined;
let disposed = false;
let noticeId = 0;

function cachedTheme(): ThemePreference {
  try {
    const saved = localStorage.getItem('lion-theme');
    return saved === 'light' || saved === 'dark' ? saved : 'system';
  } catch { return 'system'; }
}
function applyTheme(next: ThemeState) {
  themeRevision += 1;
  theme.value = next;
  document.documentElement.dataset.theme = next.resolved;
  document.documentElement.style.colorScheme = next.resolved;
  document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')?.setAttribute('content', next.resolved === 'dark' ? '#141a24' : '#f4f6fa');
  try { localStorage.setItem('lion-theme', next.preference); } catch { /* Theme still works when browser storage is unavailable. */ }
}
async function changeTheme(preference: ThemePreference) {
  if (busy.theme || shutdown.value) return;
  busy.theme = true;
  try {
    applyTheme(api ? await api.setTheme(preference) : { preference, resolved: preference === 'system' ? systemTheme.matches ? 'dark' : 'light' : preference });
  } catch (error) { notify(`无法保存主题：${cleanError(error)}`, true); }
  finally { busy.theme = false; }
}
function systemThemeChanged() {
  if (preview && theme.value.preference === 'system') applyTheme({ preference: 'system', resolved: systemTheme.matches ? 'dark' : 'light' });
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
  if (!api?.installUpdate || updating.value || shutdown.value) return;
  busy.update = true;
  try {
    if (update.value?.manualOnly) {
      if (!api.openUpdateDownload) throw Error('请从 GitHub Releases 下载新版本');
      await api.openUpdateDownload();
    } else { await api.installUpdate(); }
  }
  catch (error) {
    shutdown.value = null;
    notify(`更新未完成：${cleanError(error)}`, true);
  } finally { busy.update = false; }
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
const connectionActive = computed(() => currentState.value && !['disconnected', 'error', 'failed', 'idle'].includes(currentState.value.status));
const onlineCount = computed(() => profiles.value.filter(p => getState(p).status === 'connected').length);
const gpu = computed(() => currentState.value?.gpu);
const gpuAvailability = computed(() => currentState.value ? gpuAvailabilityFor(currentState.value) : 'unknown');
const showGpuCard = computed(() => gpuAvailability.value === 'available');
const gpuDetecting = computed(() => ['loading', 'polling', 'checking'].includes(gpu.value?.status || ''));
const gpuDetectionText = computed(() => {
  if (gpuDetecting.value) return '正在检测显卡…';
  if (gpuAvailability.value === 'absent') return '未检测到 NVIDIA 显卡';
  if (gpuAvailability.value === 'unavailable') return '显卡监测不可用';
  if (gpu.value?.error) return '无法确认显卡状态';
  if (!connected.value) return '连接后检测显卡';
  if (!selected.value?.gpu.enabled) return '显卡自动检测已暂停';
  return '等待显卡检测';
});
const gpuDetectionDetail = computed(() => {
  if (gpuDetecting.value) return '';
  if (gpuAvailability.value === 'unavailable') return '未找到 nvidia-smi，无法确认显卡硬件';
  if (gpuAvailability.value === 'absent' && connected.value && selected.value?.gpu.enabled) return '每 60 秒重新检测';
  return gpu.value?.error ? gpu.value.availabilityReason || gpu.value.error : '';
});
const hasGpuSample = computed(() => Boolean(gpu.value?.lastUpdated));
const gpuFresh = computed(() => Boolean(selected.value && currentState.value && sampleIsFresh(selected.value, currentState.value)));
const gpuTimerActive = computed(() => Boolean(connected.value && selected.value?.gpu.enabled));
const gpuTimerSampling = computed(() => gpuTimerActive.value && Boolean(gpu.value?.initialConfirming || gpuDetecting.value));
const gpuSampleNotice = computed(() => !connected.value ? '连接设备后，将自动读取显卡状态' : !selected.value?.gpu.enabled ? '自动检测已暂停，可手动刷新' : gpu.value?.initialConfirming ? '已读取显卡，正在快速确认空闲状态…' : hasGpuSample.value ? '' : '正在读取首次显卡状态…');
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
  if (!gpuTimerActive.value || gpuTimerSampling.value || !next) return 0;
  return Math.max(0, Math.min(1, (next - now.value) / gpuPollDuration.value));
});
const countdown = computed(() => {
  if (!connected.value || !selected.value?.gpu.enabled) return '—';
  if (gpu.value?.initialConfirming) return '确认中';
  if (['loading', 'polling', 'checking'].includes(gpu.value?.status || '')) return '采样中';
  const next = timestamp(gpu.value?.nextPollAt);
  if (!next) return '等待采样';
  return `${Math.min(gpu.value?.intervalSec || selected.value?.gpu.intervalSec || 300, Math.max(0, Math.ceil((next - now.value) / 1000)))} 秒`;
});
const slurm = computed(() => currentState.value?.slurm);
const showSlurmCard = computed(() => slurm.value?.availability === 'available');
const slurmScope = computed(() => selected.value && pendingSlurmScopes[selected.value.id] || slurm.value?.scope || 'mine');
const slurmLoading = computed(() => Boolean(selected.value && pendingSlurmScopes[selected.value.id]) || slurm.value?.status === 'loading');
const slurmJobs = computed(() => slurmLoading.value ? [] : slurm.value?.jobs || []);
const slurmPageCount = computed(() => Math.max(1, Math.ceil(slurmJobs.value.length / slurmPageSize)));
const slurmPageRows = computed(() => slurmJobs.value.slice((slurmPage.value - 1) * slurmPageSize, slurmPage.value * slurmPageSize));
watch([selectedId, slurmScope], () => { slurmPage.value = 1; });
watch([() => slurm.value?.jobs.length || 0, slurmLoading], ([count, isLoading]) => {
  if (!isLoading) slurmPage.value = Math.min(slurmPage.value, Math.max(1, Math.ceil(Number(count) / slurmPageSize)));
});
watch(slurmPage, async () => {
  await nextTick();
  document.querySelector<HTMLDivElement>('.slurm-table-wrap')?.scrollTo({ top: 0 });
});
const slurmWarning = computed(() => {
  if (!connected.value) return slurm.value?.lastUpdated ? '连接已断开，当前展示上次结果。' : '连接设备后读取作业状态。';
  if (slurm.value?.error) return `${slurm.value.error}${slurm.value.lastUpdated ? ' · 当前展示上次结果' : ''}`;
  return '';
});
const slurmDetectionError = computed(() => !showSlurmCard.value && slurm.value?.availability === 'unknown' && slurm.value?.status === 'error' ? slurm.value.error || slurm.value.availabilityReason || '暂时无法确认作业监测是否可用' : '');
const storage = computed(() => currentState.value?.storage);
const storageLoading = computed(() => storage.value?.status === 'loading' || Boolean(selected.value && busy[`storageRefresh:${selected.value.id}`]));
const storageWarning = computed(() => {
  if (storage.value?.error) return `${storage.value.error}${storage.value.lastUpdated ? ' · 当前展示上次结果' : ''}`;
  if (!connected.value) return storage.value?.lastUpdated ? '连接已断开，当前展示上次结果。' : '连接设备后读取空间占用。';
  return storage.value?.partial ? '部分目录无法读取，统计可能不完整。' : '';
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
function gpuAvailabilityFor(state: ServerState): NonNullable<ServerState['gpu']['availability']> {
  return state.gpu.availability || (state.gpu.cards.length > 0 ? 'available' : 'unknown');
}
function sampleIsFresh(profile: Profile, state: ServerState): boolean {
  return gpuAvailabilityFor(state) === 'available' && state.status === 'connected' && profile.gpu.enabled && Boolean(state.gpu.lastUpdated) && !state.gpu.error && !['error', 'stale', 'disabled', 'unknown', 'paused', 'unavailable'].includes(state.gpu.status);
}
function statusLabel(status?: string): string {
  return ({ connected: '已连接', connecting: '连接中', reconnecting: '恢复中', recovering: '恢复中',
    disconnecting: '关闭中', disconnected: '未连接', error: '连接失败', failed: '连接失败',
    'awaiting-host-key': '等待指纹确认', 'awaiting-trust': '等待指纹确认', 'waiting-host-key': '等待指纹确认', 'host-key': '等待指纹确认', idle: '未连接',
  } as Record<string, string>)[status || 'disconnected'] || status || '未连接';
}
function statusClass(status?: string): string {
  if (['connected', 'healthy', 'active', 'running', 'listening', 'ready', 'up'].includes(status || '')) return 'positive';
  if (['error', 'failed', 'degraded', 'target-unavailable', 'unhealthy'].includes(status || '')) return 'negative';
  if (['connecting', 'reconnecting', 'recovering', 'checking', 'starting', 'stopping', 'disconnecting', 'waiting-port', 'awaiting-trust', 'awaiting-host-key', 'waiting-host-key'].includes(status || '')) return 'pending';
  return 'neutral';
}
function tunnelState(tunnel: Tunnel) {
  return currentState.value?.tunnels.find(t => t.id === tunnel.id);
}
function tunnelStatus(tunnel: Tunnel): string {
  if (!tunnel.enabled) return '已暂停';
  const status = tunnelState(tunnel)?.status;
  if (!status || status === 'stopped' || status === 'disabled' || status === 'idle') return connected.value ? '等待启动' : '等待连接';
  if (['running', 'active', 'healthy', 'listening', 'ready', 'up', 'unverified'].includes(status)) {
    const health = tunnelState(tunnel);
    if (health?.healthStatus === 'tcp-ok' || health?.healthVerified || /TCP.*(?:可连接|正常)/i.test(health?.error || '')) return 'TCP 连通正常';
    if (health?.healthStatus === 'probe-restricted') return '监听中 · 检测受限';
    if (health?.healthStatus === 'pending') return '监听已建立 · 检测中';
    return '监听已建立 · 未验证';
  }
  return ({ active: '运行中', healthy: '连通正常', listening: '监听中', checking: '检测中',
    starting: '建立中', recovering: '恢复中', reconnecting: '恢复中', 'waiting-port': '等待端口释放',
    'target-unavailable': '目标不可用', degraded: '目标不可用', unverified: '监听已建立 · 未验证',
    error: '异常', failed: '建立失败', stopping: '关闭中', ready: '运行中', up: '连通正常',
  } as Record<string, string>)[status] || status;
}
function tunnelDetail(tunnel: Tunnel): string {
  const health = tunnelState(tunnel);
  if (health?.healthStatus === 'tcp-ok' || health?.healthVerified || /TCP.*(?:可连接|正常)/i.test(health?.error || '')) return health?.healthDetail || 'TCP 路径可连接；未检查应用响应';
  if (health?.healthStatus === 'probe-restricted') return health.healthDetail || health.error || '服务器拒绝主动探测；监听已建立，完整路径尚未验证';
  return health?.healthDetail || health?.error || (health?.lastChecked ? `检测于 ${timeText(health.lastChecked)}` : `每 ${tunnel.healthIntervalSec} 秒检测`);
}
function processCount(card: GpuCard): number { return Array.isArray(card.computeProcesses) ? card.computeProcesses.length : card.computeProcesses || 0; }
function cardLabel(card: GpuCard): string {
  if (!gpuFresh.value) return '上次结果';
  if (card.idle) return '空闲';
  if (card.memoryUsedMiB === null || card.memoryTotalMiB === null || card.utilization === null) return '未知';
  if (processCount(card)) return '使用中';
  if (card.idleCandidate) return '待确认';
  if (card.utilization > (selected.value?.gpu.idleUtil ?? 5) || card.memoryUsedMiB > (selected.value?.gpu.idleMemoryMiB ?? 512)) return '使用中';
  return '未知';
}
function numberText(value: number | null): string { return value === null ? '—' : value.toLocaleString(); }
function memoryPercent(card: GpuCard): number { return card.memoryUsedMiB === null || !card.memoryTotalMiB ? 0 : Math.min(100, card.memoryUsedMiB / card.memoryTotalMiB * 100); }
function memoryTotal(card: GpuCard): string { return card.memoryTotalMiB === null ? '—' : (card.memoryTotalMiB / 1024).toFixed(1); }
function timestamp(value?: number | string | null): number {
  if (!value) return 0;
  return typeof value === 'number' ? value : Number.isNaN(Number(value)) ? new Date(value).getTime() : Number(value);
}
function timeText(value?: number | string | null, full = false): string {
  const time = timestamp(value);
  if (!time) return '尚未更新';
  return new Intl.DateTimeFormat('zh-CN', { ...(full ? { month: '2-digit', day: '2-digit' } as const : {}), hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }).format(time);
}
function cleanError(error: unknown): string {
  return (error instanceof Error ? error.message : String(error)).replace(/^Error invoking remote method '[^']+': (?:Error: )?/, '');
}
function notify(message: string, error = false) {
  const id = ++noticeId;
  notices.value.push({ id, message, error });
  setTimeout(() => { notices.value = notices.value.filter(n => n.id !== id); }, error ? 8000 : 3800);
}
async function action(key: string, work: () => Promise<unknown>): Promise<boolean> {
  if (shutdown.value) return false;
  if (preview) { notify('这是浏览器界面预览，请在桌面应用中操作连接。'); return false; }
  if (busy[key]) return false;
  busy[key] = true;
  try { await work(); return true; }
  catch (error) { notify(cleanError(error), true); return false; }
  finally { busy[key] = false; }
}
async function changeSlurmScope(scope: SlurmScope) {
  const profile = selected.value;
  if (!profile || !connected.value || scope === slurmScope.value || busy[`slurmScope:${profile.id}`]) return;
  pendingSlurmScopes[profile.id] = scope;
  if (!await action(`slurmScope:${profile.id}`, () => api!.setSlurmScope(profile.id, scope))) delete pendingSlurmScopes[profile.id];
}
function slurmStateTitle(job: SlurmJob): string {
  return job.stateCode ? `Slurm 状态代码：${job.stateCode}` : '';
}
function slurmReasonTitle(job: SlurmJob): string {
  return job.reasonCode ? `Slurm 原因：${job.reasonCode}` : '';
}
function ownsJob(job: SlurmJob): boolean {
  return Boolean(selected.value && (slurm.value?.scope === 'mine' || job.user === selected.value.username));
}
function priorityKey(job: SlurmJob, profileId = selected.value?.id || ''): string { return `${profileId}:${job.cluster || ''}:${job.id}`; }
function clearPriorities(profileId: string) {
  for (const key of Object.keys(priorities)) if (key.startsWith(`${profileId}:`)) delete priorities[key];
}
async function requestPriority(job: SlurmJob) {
  const profile = selected.value;
  if (!profile || !connected.value) return;
  const key = priorityKey(job, profile.id);
  const generation = connectionGenerations[profile.id] || 0;
  await action(`priority:${key}`, async () => {
    delete priorities[key];
    try {
      const result = await api!.getJobPriority(profile.id, job.id);
      if ((connectionGenerations[profile.id] || 0) === generation && states[profile.id]?.status === 'connected') priorities[key] = { value: result.priority, values: result.priorities };
    } catch (error) {
      if ((connectionGenerations[profile.id] || 0) === generation) priorities[key] = { error: cleanError(error) };
      throw error;
    }
  });
}
async function requestJobCancellation(job: SlurmJob) {
  const profile = selected.value;
  if (!profile || !connected.value || !ownsJob(job)) return;
  const generation = connectionGenerations[profile.id] || 0;
  await action(`prepareCancel:${priorityKey(job, profile.id)}`, async () => {
    const result = await api!.prepareJobCancel(profile.id, job.id);
    if (selectedId.value !== profile.id || (connectionGenerations[profile.id] || 0) !== generation || states[profile.id]?.status !== 'connected') return;
    tmpCleanup.value = null;
    jobCancellation.value = { profileId: profile.id, profileName: profile.name, jobName: job.name, preview: result };
    void focusFirst();
  });
}
async function cancelJobConfirmed() {
  const target = jobCancellation.value;
  if (!target || confirmationExpired.value) return;
  if (await action('cancelJob', async () => {
    await api!.cancelJob(target.profileId, target.preview.jobId, target.preview.token);
    for (const key of Object.keys(priorities)) if (key.startsWith(`${target.profileId}:`) && key.endsWith(`:${target.preview.jobId}`)) delete priorities[key];
  })) { jobCancellation.value = null; notify(`已取消作业 ${target.preview.jobId}`); }
  else jobCancellation.value = null;
}
function bytesText(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes)) return '—';
  if (bytes === 0) return '0 B';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
  const exponent = Math.min(units.length - 1, Math.max(0, Math.floor(Math.log(Math.abs(bytes)) / Math.log(1024))));
  return `${(bytes / 1024 ** exponent).toLocaleString('zh-CN', { maximumFractionDigits: exponent ? 2 : 0 })} ${units[exponent]}`;
}
async function requestTmpCleanup() {
  const profile = selected.value;
  if (!profile || !connected.value) return;
  const generation = connectionGenerations[profile.id] || 0;
  await action(`prepareCleanup:${profile.id}`, async () => {
    const result = await api!.prepareTmpCleanup(profile.id);
    if (selectedId.value !== profile.id || (connectionGenerations[profile.id] || 0) !== generation || states[profile.id]?.status !== 'connected') return;
    jobCancellation.value = null;
    tmpCleanup.value = { profileId: profile.id, profileName: profile.name, preview: result };
    void focusFirst();
  });
}
async function cleanupTmpConfirmed() {
  const target = tmpCleanup.value;
  if (!target || confirmationExpired.value) return;
  if (await action('cleanupTmp', async () => {
    const result = await api!.cleanupTmp(target.profileId, target.preview.token);
    notify(result.cleanupStatus === 'unknown' ? '清理结果暂时无法确认，请刷新空间统计并查看系统日志。' : `${result.cleanupStatus === 'partial' ? '部分清理完成' : '清理完成'}${result.deletedCount === null ? '' : `，已删除 ${result.deletedCount.toLocaleString()} 项`}${result.warnings?.length ? '，请查看系统日志中的未清理项目' : ''}`, result.cleanupStatus !== 'complete');
  })) tmpCleanup.value = null;
  else tmpCleanup.value = null;
}
function selectProfile(profile: Profile) {
  if (selectedId.value !== profile.id) { jobCancellation.value = null; tmpCleanup.value = null; }
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
  if (!selected.value) return;
  Object.assign(tunnelDraft, clone(tunnel || freshTunnel()));
  editing.value = Boolean(tunnel);
  formError.value = '';
  drawer.value = 'tunnel';
  void focusFirst();
}
function closeDrawer() { if (!busy.save) drawer.value = null; }
function replaceProfile(profile: Profile) {
  const index = profiles.value.findIndex(p => p.id === profile.id);
  if (index < 0) profiles.value.push(profile); else profiles.value[index] = profile;
  if (selectedId.value === profile.id) selectProfile(profile);
}
async function saveDevice() {
  formError.value = '';
  if (!deviceDraft.name.trim() || !deviceDraft.host.trim() || !deviceDraft.username.trim()) { formError.value = '请填写设备名称、服务器地址和用户名。'; return; }
  if (deviceDraft.authType === 'key' && !deviceDraft.keyPath.trim()) { formError.value = '请选择或填写私钥文件路径。'; return; }
  const profile = clone(deviceDraft);
  profile.name = profile.name.trim(); profile.host = profile.host.trim(); profile.username = profile.username.trim();
  profile.keyPath = profile.keyPath.replace(/[\u202a-\u202e\u2066-\u2069]/g, '').trim();
  const secrets: Secrets = profile.authType === 'password' ? { password: devicePassword.value || undefined, remember: rememberSecret.value } : { passphrase: devicePassphrase.value || undefined, remember: rememberSecret.value };
  const success = await action('save', async () => {
    const saved = await api!.saveProfile(profile, secrets);
    replaceProfile(saved);
    if (secrets.password || secrets.passphrase) runtimeSecrets[saved.id] = secrets;
    selectProfile(saved);
  });
  if (success) { drawer.value = null; notify(editing.value ? '设备配置已保存' : '设备已添加'); }
}
async function saveTunnel() {
  if (!selected.value) return;
  const profile = clone(selected.value);
  const tunnel = clone(tunnelDraft);
  tunnel.name = tunnel.name.trim() || `${tunnel.direction === 'local' ? '正向' : '反向'} ${tunnel.bindPort}`;
  tunnel.bindHost = tunnel.bindHost.trim(); tunnel.targetHost = tunnel.targetHost.trim();
  if (profile.tunnels.some(t => t.id !== tunnel.id && t.direction === tunnel.direction && t.bindHost === tunnel.bindHost && t.bindPort === tunnel.bindPort)) { formError.value = '该监听地址和端口已有同类型隧道，请选择其他端口。'; return; }
  const index = profile.tunnels.findIndex(t => t.id === tunnel.id);
  if (index < 0) profile.tunnels.push(tunnel); else profile.tunnels[index] = tunnel;
  if (await action('save', async () => replaceProfile(await api!.saveProfile(profile)))) {
    drawer.value = null;
    notify(editing.value ? '隧道配置已保存' : '隧道已添加');
  }
}
async function chooseKey() {
  await action('pickKey', async () => { const path = await api!.pickKey(); if (path) deviceDraft.keyPath = path; });
}
async function requestConnection(profile: Profile) {
  if (profile.authType === 'password' && !profile.hasSavedSecret && !runtimeSecrets[profile.id]?.password) {
    credentialProfile.value = profile;
    credentialPassword.value = ''; credentialPassphrase.value = ''; credentialRemember.value = false;
    void focusFirst();
    return;
  }
  await action(`connect:${profile.id}`, () => api!.connect(profile.id, runtimeSecrets[profile.id]));
}
async function submitCredentials() {
  const profile = credentialProfile.value;
  if (!profile) return;
  const secrets: Secrets = { password: credentialPassword.value || undefined, passphrase: credentialPassphrase.value || undefined };
  if (credentialPassword.value || credentialPassphrase.value) runtimeSecrets[profile.id] = secrets;
  credentialProfile.value = null;
  await action(`connect:${profile.id}`, async () => {
    if (credentialRemember.value && (secrets.password || secrets.passphrase)) replaceProfile(await api!.saveProfile(clone(profile), { ...secrets, remember: true }));
    await api!.connect(profile.id, secrets);
  });
}
async function disconnect(profile: Profile) {
  await action(`disconnect:${profile.id}`, () => api!.disconnect(profile.id));
}
async function applyGpu(changes: Partial<GpuSettings>) {
  const profile = selected.value;
  if (!profile) return;
  const settings = { ...profile.gpu, ...changes };
  if (settings.intervalSec < 2 || settings.intervalSec > 300 || !Number.isFinite(settings.intervalSec)) { pollInterval.value = profile.gpu.intervalSec; notify('检测间隔需要在 2 到 300 秒之间。', true); return; }
  if (await action('gpuSettings', () => api!.updateGpu(profile.id, settings))) replaceProfile({ ...profile, gpu: settings });
}
async function toggleTunnel(tunnel: Tunnel) {
  const profile = selected.value;
  if (!profile) return;
  const enabled = !tunnel.enabled;
  if (await action(`tunnel:${tunnel.id}`, () => api!.setTunnel(profile.id, tunnel.id, enabled))) {
    replaceProfile({ ...profile, tunnels: profile.tunnels.map(t => t.id === tunnel.id ? { ...t, enabled } : t) });
  }
}
async function deleteConfirmed() {
  const target = deleting.value;
  if (!target) return;
  const success = await action('delete', async () => {
    if (target.kind === 'device') {
      await api!.deleteProfile(target.id);
      profiles.value = profiles.value.filter(p => p.id !== target.id);
      delete states[target.id]; delete runtimeSecrets[target.id];
      if (selectedId.value === target.id) { selectedId.value = ''; if (profiles.value[0]) selectProfile(profiles.value[0]); }
    } else if (selected.value) {
      const profile = clone(selected.value);
      profile.tunnels = profile.tunnels.filter(t => t.id !== target.id);
      replaceProfile(await api!.saveProfile(profile));
    }
  });
  if (success) { deleting.value = null; notify(target.kind === 'device' ? '设备已移除' : '隧道已移除'); }
}
async function trustHost(accepted: boolean) {
  const request = hostRequest.value;
  if (!request) return;
  if (await action('trust', () => api!.trustHost(request.profileId, accepted, request.fingerprint))) {
    hostRequests.value = hostRequests.value.filter(pending => pending.profileId !== request.profileId || pending.fingerprint !== request.fingerprint);
  }
}
function keyHandler(event: KeyboardEvent) {
  if (shutdown.value) { event.preventDefault(); return; }
  if (event.key === 'Escape') {
    if (hostRequest.value) return;
    if (jobCancellation.value && !busy.cancelJob) jobCancellation.value = null;
    else if (tmpCleanup.value && !busy.cleanupTmp) tmpCleanup.value = null;
    else if (deleting.value && !busy.delete) deleting.value = null;
    else if (credentialProfile.value) credentialProfile.value = null;
    else closeDrawer();
  }
  if (event.key === 'Tab' && anyModal.value) {
    const panel = activePanel();
    if (!panel) return;
    const elements = [...panel.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex="0"]')];
    const first = elements[0], last = elements[elements.length - 1];
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
  }
}
async function initialize() {
  startupError.value = '';
  loading.value = true;
  const receivedStates = new Set<string>();
  initializingStates = receivedStates;
  const startingThemeRevision = themeRevision;
  const startingUpdateRevision = updateRevision;
  try {
    if (api) {
      const initial = await api.getInitial();
      if (disposed) return;
      profiles.value = initial.profiles;
      for (const state of initial.states || []) if (!receivedStates.has(state.profileId)) states[state.profileId] = state;
      version.value = initial.version;
      if (initial.update && updateRevision === startingUpdateRevision) receiveUpdate(initial.update);
      if (initial.theme && themeRevision === startingThemeRevision) applyTheme(initial.theme);
      if (initial.shutdown) beginShutdown(initial.shutdown);
      if (!shutdown.value) for (const request of initial.pendingHostKeys || []) {
        if (states[request.profileId]?.status === 'awaiting-trust') queueHostKey(request);
      }
    }
    if (profiles.value[0]) selectProfile(profiles.value[0]);
    loading.value = false;
  } catch (error) {
    if (!disposed) startupError.value = cleanError(error);
  } finally {
    if (initializingStates === receivedStates) initializingStates = undefined;
  }
}
function queueHostKey(request: HostKeyRequest) {
  if (shutdown.value) return;
  if (states[request.profileId] && states[request.profileId].status !== 'awaiting-trust') return;
  if (!hostRequests.value.some(r => r.profileId === request.profileId && r.fingerprint === request.fingerprint)) hostRequests.value.push(request);
  void focusFirst();
}
onMounted(() => {
  clockId = setInterval(() => { now.value = Date.now(); }, 100);
  document.addEventListener('keydown', keyHandler);
  systemTheme.addEventListener('change', systemThemeChanged);
  if (api) {
    removeTheme = api.onTheme?.(applyTheme);
    removeUpdate = api.onUpdate?.(receiveUpdate);
    removeShutdown = api.onShutdown?.(beginShutdown);
    removeBackendError = api.onBackendError?.(error => {
      startupError.value = error.message;
      loading.value = true;
    });
    removeState = api.onState(state => {
      initializingStates?.add(state.profileId);
      if (state.status !== 'awaiting-trust') hostRequests.value = hostRequests.value.filter(request => request.profileId !== state.profileId);
      const previous = states[state.profileId];
      if (previous?.status !== state.status && (previous?.status === 'connected' || state.status === 'connected')) {
        connectionGenerations[state.profileId] = (connectionGenerations[state.profileId] || 0) + 1;
        clearPriorities(state.profileId);
      }
      if (state.status !== 'connected') {
        if (jobCancellation.value?.profileId === state.profileId && !busy.cancelJob) jobCancellation.value = null;
        if (tmpCleanup.value?.profileId === state.profileId && !busy.cleanupTmp) tmpCleanup.value = null;
      }
      if (state.slurm?.status === 'ready' && jobCancellation.value?.profileId === state.profileId && !busy.cancelJob && !state.slurm.jobs.some(job => job.id === jobCancellation.value?.preview.jobId)) {
        jobCancellation.value = null;
        notify('该作业已不在队列中，请刷新作业列表。', true);
      }
      states[state.profileId] = state;
      const requestedScope = pendingSlurmScopes[state.profileId];
      if (requestedScope && (state.status !== 'connected' || state.slurm?.availability === 'unavailable' || state.slurm?.scope === requestedScope && state.slurm.status !== 'loading')) delete pendingSlurmScopes[state.profileId];
    });
    removeHost = api.onHostKey(queueHostKey);
  }
  void initialize();
});
onUnmounted(() => { disposed = true; clearInterval(clockId); removeState?.(); removeHost?.(); removeTheme?.(); removeShutdown?.(); removeBackendError?.(); removeUpdate?.(); systemTheme.removeEventListener('change', systemThemeChanged); document.removeEventListener('keydown', keyHandler); });
</script>

<template>
  <main v-if="loading" class="startup-screen" data-testid="startup-screen" :aria-busy="!startupError" :inert="Boolean(shutdown)">
    <section class="startup-panel" :role="startupError ? 'alert' : 'status'" aria-live="polite">
      <div class="startup-mark"><Icon name="shield" :size="37" /></div>
      <h1>Lion <span>SSH</span></h1>
      <template v-if="startupError"><h2>暂时无法打开工作台</h2><p class="startup-error">{{ startupError }}</p><button class="button primary" data-testid="startup-retry" @click="initialize()"><Icon name="refresh" :size="16" />重新加载</button></template>
      <template v-else><span class="startup-spinner" aria-hidden="true" /><h2>正在打开工作台</h2><p>正在读取本机配置…</p></template>
    </section>
    <span class="startup-caption">让连接，始终在线</span>
  </main>
  <div v-else class="app-shell" :class="{ 'has-modal': anyModal }" :inert="anyModal || Boolean(shutdown)" :aria-busy="Boolean(shutdown)">
    <aside class="sidebar" aria-label="设备导航">
      <div class="brand">
        <div class="brand-mark">
          <svg viewBox="0 0 40 40" fill="none" aria-hidden="true"><path d="m9 10 5-4 6 2 6-2 5 4 3 11-6 12-8 3-8-3-6-12 3-11Z" fill="currentColor" opacity=".18"/><path d="m12 13 4 1 4-3 4 3 4-1 2 9-4 7-6 3-6-3-4-7 2-9Z" stroke="currentColor" stroke-width="1.8"/><path d="m14 20 3 1m9-1-3 1m-5 3h4l-2 3-2-3Zm2 3v2" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
        </div>
        <div><strong>Lion <span>SSH</span></strong><small>让连接，始终在线</small></div>
      </div>
      <div class="workspace-nav"><Icon name="server" :size="18" /><span>连接工作台</span><span class="nav-pill">{{ profiles.length }}</span></div>
      <div class="sidebar-section-title"><span>我的设备</span><button class="icon-button subtle" title="添加设备" aria-label="添加设备" @click="openDevice()"><Icon name="plus" :size="18" /></button></div>
      <div class="server-list">
        <button v-for="profile in profiles" :key="profile.id" class="server-item" :class="{ selected: selectedId === profile.id }" @click="selectProfile(profile)">
          <span class="server-item-icon"><Icon name="server" :size="19" /></span>
          <span class="server-item-text"><strong>{{ profile.name }}</strong><small>{{ profile.host }}</small><span class="server-item-status"><i class="status-dot" :class="statusClass(getState(profile).status)" />{{ statusLabel(getState(profile).status) }}<span v-if="sampleIsFresh(profile, getState(profile))" class="sidebar-gpu">{{ getState(profile).gpu.initialConfirming ? '确认空闲中' : getState(profile).gpu.idleCount + ' 张空闲' }}</span></span></span>
          <Icon v-if="selectedId === profile.id" name="chevron" :size="15" />
        </button>
        <div v-if="!profiles.length && !loading" class="sidebar-empty">还没有设备<br />添加后即可管理连接</div>
      </div>
      <button class="add-device-sidebar" @click="openDevice()"><Icon name="plus" :size="17" />添加设备</button>
      <div class="sidebar-bottom">
        <div class="theme-control" data-testid="theme-select" role="group" aria-label="界面主题"><button v-for="option in themeOptions" :key="option.value" :data-testid="`theme-${option.value}`" :class="{ active: theme.preference === option.value }" :aria-pressed="theme.preference === option.value" :title="option.title" :aria-label="option.title" :disabled="busy.theme" @click="changeTheme(option.value)"><Icon :name="option.icon" :size="13" /><span>{{ option.label }}</span></button></div>
        <div class="sidebar-footer">
          <span><i class="status-dot positive" />{{ onlineCount }} 台在线</span>
          <div class="version-control">
            <span>v{{ version }}</span>
            <button v-if="updatePending" class="update-button" data-testid="install-update" :disabled="updating || update?.status === 'checking'" :aria-label="`${update?.manualOnly ? '下载' : '更新到'} v${update?.version}`" :title="update?.error || (update?.manualOnly ? '打开 GitHub Releases 下载新版本' : `下载并更新到 v${update?.version}，安装前会安全断开连接`)" @click="installUpdate">
              <Icon :name="updating ? 'refresh' : 'download'" :size="12" :class="{ spinning: updating }" /><span>{{ updating ? update?.status === 'installing' ? '安装中' : updateProgress : update?.manualOnly ? '下载' : '更新' }}</span>
            </button>
          </div>
        </div>
      </div>
    </aside>

    <main class="workspace">
      <header class="page-header"><div><div class="eyebrow">YOUR CONNECTIONS, AT A GLANCE</div><h1>连接工作台<span class="title-dot">.</span></h1></div><div class="header-actions"><div class="wall-clock"><Icon name="clock" :size="15" />{{ timeText(now) }}</div><button data-testid="add-device" class="button secondary" @click="openDevice()"><Icon name="plus" :size="17" />添加设备</button></div></header>
      <div v-if="preview" class="preview-banner"><Icon name="info" :size="17" /><span>浏览器界面预览 · 此处未连接服务器，连接操作请使用桌面应用。</span></div>

      <template v-if="selected && currentState">
        <section class="device-card card">
          <div class="device-identity"><div class="device-avatar"><Icon name="server" :size="27" /></div><div class="device-title"><div class="device-title-line"><h2>{{ selected.name }}</h2><span class="status-badge" :class="statusClass(currentState.status)"><i class="status-dot" :class="statusClass(currentState.status)" />{{ statusLabel(currentState.status) }}</span></div><div class="device-address">{{ selected.username }}@{{ selected.host }}<span>:{{ selected.port }}</span><span class="divider">/</span><Icon :name="selected.authType === 'key' ? 'key' : 'lock'" :size="13" />{{ selected.authType === 'key' ? '密钥认证' : '密码认证' }}</div></div></div>
          <div class="device-actions"><button class="icon-button" data-testid="edit-device" title="编辑设备" aria-label="编辑设备" @click="openDevice(selected)"><Icon name="edit" :size="18" /></button><button class="icon-button danger-hover" title="移除设备" aria-label="移除设备" @click="deleting = { kind: 'device', id: selected.id, name: selected.name }"><Icon name="trash" :size="18" /></button><span class="action-divider" /><button v-if="connectionActive" class="button secondary disconnect-button" :disabled="busy[`disconnect:${selected.id}`] || currentState.status === 'disconnecting'" @click="disconnect(selected)"><Icon name="power" :size="16" />{{ currentState.status === 'disconnecting' ? '正在关闭' : '断开连接' }}</button><button v-else data-testid="connect-button" class="button primary" :disabled="busy[`connect:${selected.id}`]" @click="requestConnection(selected)"><Icon name="link" :size="17" />连接设备</button></div>
          <div v-if="!showGpuCard" data-testid="gpu-detect-note" class="gpu-detect-note" role="status"><Icon :name="gpuDetecting ? 'refresh' : gpuAvailability === 'absent' ? 'info' : 'chip'" :size="14" :class="{ spinning: gpuDetecting }" /><span>{{ gpuDetectionText }}</span><small v-if="gpuDetectionDetail" :title="gpuDetectionDetail">{{ gpuDetectionDetail }}</small><span v-if="gpuAvailability === 'unavailable' && connected && selected.gpu.enabled" class="gpu-recheck-schedule">每 60 秒重新检测</span><button v-if="connected" data-testid="gpu-recheck" class="text-button" :disabled="busy.refresh || gpuDetecting" @click="action('refresh', () => api!.refreshGpu(selected!.id))">重新检测</button></div>
          <div v-if="slurmDetectionError" class="slurm-detect-note" data-testid="slurm-detect-note" role="status"><Icon name="info" :size="14" /><span>作业监测待确认</span><small :title="slurmDetectionError">{{ slurmDetectionError }}</small><button v-if="connected" class="text-button" :disabled="busy[`slurmRefresh:${selected.id}`]" @click="action(`slurmRefresh:${selected!.id}`, () => api!.refreshSlurm(selected!.id))">重新检测</button></div>
          <div v-if="currentState.error" class="inline-error"><Icon name="alert" :size="16" /><span>{{ currentState.error }}</span><button v-if="selected.authType === 'key' && !connectionActive" class="text-button" @click="credentialProfile = selected; credentialPassphrase = ''; credentialPassword = ''; credentialRemember = false; focusFirst()">填写密钥口令</button></div>
        </section>

        <section v-if="showGpuCard" data-testid="gpu-card" class="gpu-card card">
          <div class="section-heading"><div class="heading-label"><span class="section-icon purple"><Icon name="chip" :size="20" /></span><h2>显卡监测</h2><span class="light-tag">NVIDIA</span></div><div class="monitor-controls"><span class="switch-label">自动检测</span><button class="switch" :class="{ on: selected.gpu.enabled }" role="switch" :aria-checked="selected.gpu.enabled" aria-label="自动检测显卡" :disabled="busy.gpuSettings" @click="applyGpu({ enabled: !selected.gpu.enabled })"><span /></button><span class="control-divider" /><label class="interval-control">每 <input data-testid="gpu-interval" v-model.number="pollInterval" type="number" min="2" max="300" step="1" aria-label="显卡检测间隔秒数" :disabled="busy.gpuSettings" @change="applyGpu({ intervalSec: Math.round(pollInterval) })" /> 秒</label><button class="icon-button" data-testid="gpu-refresh" title="立即刷新显卡状态" aria-label="立即刷新显卡状态" :disabled="!connected || busy.refresh || ['loading', 'polling'].includes(gpu?.status || '')" @click="action('refresh', () => api!.refreshGpu(selected!.id))"><Icon name="refresh" :size="18" :class="{ spinning: ['loading', 'polling'].includes(gpu?.status || '') }" /></button></div></div>
          <div class="gpu-summary-row"><div class="gpu-count"><span class="gpu-count-label">{{ hasGpuSample ? (gpuFresh ? gpu?.initialConfirming ? '正在确认空闲' : '可用算力' : '上次采样') : '等待采样' }}</span><div class="gpu-number"><strong>{{ hasGpuSample && !gpu?.initialConfirming ? gpu?.idleCount : '—' }}</strong><span>/ {{ hasGpuSample ? gpu?.total : '—' }} <small>张空闲</small></span></div><p v-if="gpuSampleNotice">{{ gpuSampleNotice }}</p></div><div class="gpu-pulse-chart" aria-hidden="true"><div class="pulse-grid"/><svg viewBox="0 0 290 62" preserveAspectRatio="none"><path d="M0 42H39l9-8 14 9 20-4 12-20 13 38 13-26 13 5h34l9-8 13 14 12-5h69" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg><span>{{ connected && selected.gpu.enabled ? '自动监测已开启' : '监测待命' }}</span></div><div class="poll-clock"><span class="clock-circle gpu-progress-ring" :class="{ paused: !gpuTimerActive, sampling: gpuTimerSampling }" data-testid="gpu-progress-ring" :data-progress="gpuTimerProgress.toFixed(4)" :data-sampling="gpuTimerSampling" aria-hidden="true">
                <svg width="44" height="44" viewBox="0 0 44 44"><circle class="gpu-progress-track" cx="22" cy="22" r="18" pathLength="100" /><circle class="gpu-progress-value" cx="22" cy="22" r="18" pathLength="100" transform="rotate(-90 22 22)" :stroke-dasharray="gpuTimerSampling ? '25 100' : `${gpuTimerProgress * 100} 100`" :stroke-linecap="gpuTimerSampling || gpuTimerProgress > 0 ? 'round' : 'butt'" /></svg>
                <Icon class="gpu-progress-center" name="clock" :size="19" />
              </span><div><span class="clock-label">下一次检测</span><strong data-testid="gpu-countdown">{{ countdown }}</strong><small>更新于 {{ timeText(gpu?.lastUpdated) }}</small></div></div></div>
          <div v-if="gpu?.error" class="gpu-warning"><Icon name="alert" :size="16" />{{ gpu.error }}<span v-if="hasGpuSample"> · 当前展示上次结果</span></div>
          <div class="gpu-bottom"><div class="gpu-rule-note"><i class="status-dot" :class="gpuFresh ? 'positive' : 'neutral'" /><span>{{ gpuFresh ? '采样正常' : hasGpuSample ? '结果待更新' : '尚无显卡数据' }}</span><span class="rule-divider">·</span><button class="text-button muted" @click="showGpuRules = !showGpuRules">空闲判定规则<Icon name="down" :size="12" :class="{ rotated: showGpuRules }" /></button></div><button class="text-button" :disabled="!gpu?.cards.length" @click="expandedGpu = !expandedGpu">{{ expandedGpu ? '收起显卡详情' : '查看显卡详情' }}<Icon name="down" :size="14" :class="{ rotated: expandedGpu }" /></button></div>
          <form v-if="showGpuRules" class="gpu-rules" @submit.prevent="applyGpu(gpuRules)"><label>利用率 ≤<span><input v-model.number="gpuRules.idleUtil" type="number" min="0" max="100" required /> %</span></label><label>已用显存 ≤<span><input v-model.number="gpuRules.idleMemoryMiB" type="number" min="0" max="131072" required /> MiB</span></label><label>连续确认<span><input v-model.number="gpuRules.stableSamples" type="number" min="1" max="10" required /> 次</span></label><div class="rule-help">同时要求没有计算进程。显示进程的少量占用允许存在。</div><button class="button secondary small" type="submit" :disabled="busy.gpuSettings">保存规则</button></form>
          <div v-if="expandedGpu && gpu?.cards.length" class="gpu-detail-grid"><div v-for="card in gpu.cards" :key="card.uuid || card.index" class="gpu-detail" :class="{ idle: card.idle && gpuFresh }"><div class="gpu-detail-heading"><span>GPU {{ card.index }}</span><span class="mini-badge" :class="card.idle && gpuFresh ? 'positive' : card.idle ? 'neutral' : 'pending'">{{ cardLabel(card) }}</span></div><strong>{{ card.name }}</strong><div class="gpu-metrics"><span>利用率 <b>{{ numberText(card.utilization) }}%</b></span><span>显存 <b>{{ numberText(card.memoryUsedMiB) }} MiB</b></span></div><div class="memory-meter"><span :style="{ width: `${memoryPercent(card)}%` }" /></div><small>{{ processCount(card) }} 个计算进程 · 总显存 {{ memoryTotal(card) }} GiB</small></div></div>
        </section>

        <section class="storage-card card" data-testid="storage-card" :aria-busy="storageLoading">
          <div class="section-heading"><div class="heading-label"><span class="section-icon blue"><Icon name="folder" :size="20" /></span><h2>我的空间</h2><span class="light-tag">{{ selected.username }}</span></div><div class="storage-controls"><button class="button secondary small" data-testid="tmp-cleanup" :disabled="!connected || busy[`prepareCleanup:${selected.id}`] || busy.cleanupTmp" @click="requestTmpCleanup()"><Icon name="trash" :size="14" />{{ busy[`prepareCleanup:${selected.id}`] ? '正在统计' : '清理我的 /tmp' }}</button><button class="icon-button" data-testid="storage-refresh" aria-label="立即刷新空间占用" title="立即刷新空间占用" :disabled="!connected || storageLoading" @click="action(`storageRefresh:${selected!.id}`, () => api!.refreshStorage(selected!.id))"><Icon name="refresh" :size="18" :class="{ spinning: storageLoading }" /></button></div></div>
          <div class="storage-metrics"><div><span class="storage-path" :title="storage?.homePath || `/home/${selected.username}`">{{ storage?.homePath || `/home/${selected.username}` }}</span><strong data-testid="storage-home">{{ bytesText(storage?.homeBytes) }}</strong><small>个人目录</small></div><div><span class="storage-path">/tmp</span><strong data-testid="storage-tmp">{{ bytesText(storage?.tmpBytes) }}</strong><small>仅统计属于我的项目</small></div><div class="storage-total"><span class="storage-path">合计</span><strong data-testid="storage-total">{{ bytesText(storage?.totalBytes) }}</strong><small>个人目录 + 我的临时文件</small></div></div>
          <div v-if="storageWarning" class="storage-warning" role="status"><Icon name="info" :size="14" /><span>{{ storageWarning }}</span></div>
          <div v-if="storage?.warnings?.length" class="storage-warnings"><span v-for="warning in storage.warnings" :key="warning">{{ warning }}</span></div>
          <div class="storage-footer"><span><i v-if="storageLoading" class="status-dot pending" />{{ storageLoading ? '正在统计空间占用…' : connected ? `每 ${Math.round((storage?.intervalSec || 300) / 60)} 分钟自动更新` : '等待连接' }}</span><time v-if="storage?.lastUpdated">更新于 {{ timeText(storage.lastUpdated) }}</time></div>
        </section>

        <section v-if="showSlurmCard" class="slurm-card card" data-testid="slurm-card" :aria-busy="slurmLoading">
          <div class="section-heading">
            <div class="heading-label"><span class="section-icon blue"><Icon name="activity" :size="20" /></span><h2>Slurm 作业</h2><span class="count-tag">{{ slurmLoading ? '—' : slurmJobs.length }}</span></div>
            <div class="slurm-controls">
              <div class="slurm-scope" role="group" aria-label="作业查看范围">
                <button data-testid="slurm-scope-mine" :class="{ active: slurmScope === 'mine' }" :aria-pressed="slurmScope === 'mine'" :disabled="!connected || busy[`slurmScope:${selected.id}`]" @click="changeSlurmScope('mine')">个人</button>
                <button data-testid="slurm-scope-all" :class="{ active: slurmScope === 'all' }" :aria-pressed="slurmScope === 'all'" :disabled="!connected || busy[`slurmScope:${selected.id}`]" @click="changeSlurmScope('all')">全部</button>
              </div>
              <button class="icon-button" data-testid="slurm-refresh" title="立即刷新作业" aria-label="立即刷新作业" :disabled="!connected || slurmLoading || busy[`slurmRefresh:${selected.id}`]" @click="action(`slurmRefresh:${selected!.id}`, () => api!.refreshSlurm(selected!.id))"><Icon name="refresh" :size="18" :class="{ spinning: slurmLoading }" /></button>
            </div>
          </div>
          <div v-if="slurmWarning" class="slurm-warning" role="status"><Icon name="info" :size="15" /><span>{{ slurmWarning }}</span></div>
          <div v-if="slurmLoading" class="slurm-empty" data-testid="slurm-loading" role="status"><Icon name="refresh" :size="19" class="spinning" /><span>正在读取{{ slurmScope === 'mine' ? '个人' : '全部' }}作业…</span></div>
          <div v-else-if="slurmJobs.length" class="slurm-table-wrap" tabindex="0" role="region" aria-label="Slurm 作业列表，可横向滚动">
            <table class="slurm-table" data-testid="slurm-table">
              <thead><tr><th scope="col">作业编号</th><th scope="col">分区</th><th scope="col">作业名称</th><th scope="col">用户</th><th scope="col">状态</th><th scope="col">已运行时间</th><th scope="col">节点数</th><th scope="col">节点列表</th><th scope="col">原因</th><th scope="col">权重</th><th scope="col" class="align-right">操作</th></tr></thead>
              <tbody><tr v-for="(job, index) in slurmPageRows" :key="`${job.cluster || ''}:${job.id}:${index}`">
                <td class="slurm-job-id"><span :title="job.id">{{ job.id }}</span><small v-if="job.cluster" :title="job.cluster">{{ job.cluster }}</small></td>
                <td><span class="slurm-cell-text" :title="job.partition">{{ job.partition || '—' }}</span></td>
                <td><span class="slurm-cell-text slurm-job-name" :title="job.name">{{ job.name || '—' }}</span></td>
                <td><span class="slurm-cell-text" :title="job.user">{{ job.user || '—' }}</span></td>
                <td><span class="slurm-job-state" :class="job.stateTone" :title="slurmStateTitle(job)"><i class="status-dot" :class="job.stateTone" />{{ job.stateLabel }}</span></td>
                <td class="slurm-elapsed">{{ job.elapsed || '—' }}</td>
                <td class="slurm-node-count">{{ numberText(job.nodes) }}</td>
                <td><span class="slurm-cell-text" :title="job.nodeList">{{ job.nodeList || '—' }}</span></td>
                <td class="slurm-job-reason"><span :title="slurmReasonTitle(job)">{{ job.reasonLabel || '—' }}</span></td>
                <td class="slurm-priority"><button v-if="priorities[priorityKey(job)]?.value !== undefined" class="text-button priority-value" :data-testid="`job-priority-${job.id}`" :title="priorities[priorityKey(job)]?.values?.length ? `各作业权重：${priorities[priorityKey(job)]?.values?.join('、')}；点击重新读取` : '点击重新读取权重'" :disabled="!connected || busy[`priority:${priorityKey(job)}`]" @click="requestPriority(job)">{{ priorities[priorityKey(job)]?.value === null ? priorities[priorityKey(job)]?.values?.length ? '多值' : '—' : priorities[priorityKey(job)]?.value?.toLocaleString() }}</button><button v-else class="text-button" :data-testid="`job-priority-query-${job.id}`" :title="priorities[priorityKey(job)]?.error || '读取该作业的调度权重'" :disabled="!connected || busy[`priority:${priorityKey(job)}`]" @click="requestPriority(job)">{{ busy[`priority:${priorityKey(job)}`] ? '读取中' : priorities[priorityKey(job)]?.error ? '重试' : '查看' }}</button></td>
                <td class="align-right"><button v-if="ownsJob(job)" class="icon-button danger-hover" :data-testid="`job-cancel-${job.id}`" :title="`删除自己的作业 ${job.id}`" :aria-label="`删除自己的作业 ${job.id}`" :disabled="!connected || busy[`prepareCancel:${priorityKey(job)}`] || busy.cancelJob" @click="requestJobCancellation(job)"><Icon :name="busy[`prepareCancel:${priorityKey(job)}`] ? 'refresh' : 'trash'" :size="16" :class="{ spinning: busy[`prepareCancel:${priorityKey(job)}`] }" /></button><span v-else class="slurm-no-action">—</span></td>
              </tr></tbody>
            </table>
          </div>
          <div v-else class="slurm-empty" data-testid="slurm-empty"><Icon :name="slurm?.error ? 'info' : 'check'" :size="19" /><span>{{ slurm?.error ? '暂时无法读取作业' : slurmScope === 'mine' ? '该用户暂无作业' : '当前暂无可见作业' }}</span></div>
          <div class="slurm-footer"><span>{{ slurmScope === 'mine' ? `当前用户 ${selected.username}` : '服务器允许查看的全部作业' }}<span class="slurm-footer-divider">·</span>{{ connected ? `每 ${slurm?.intervalSec || 30} 秒自动更新` : '等待连接' }}</span><div class="slurm-footer-actions"><div v-if="slurmJobs.length > slurmPageSize" class="slurm-pagination" data-testid="slurm-pagination" role="group" aria-label="作业列表分页"><button class="icon-button previous" data-testid="slurm-prev-page" title="上一页作业" aria-label="上一页作业" :disabled="slurmPage <= 1" @click="slurmPage--"><Icon name="chevron" :size="14" /></button><span data-testid="slurm-page-number" aria-live="polite">{{ slurmPage }} / {{ slurmPageCount }} 页</span><button class="icon-button" data-testid="slurm-next-page" title="下一页作业" aria-label="下一页作业" :disabled="slurmPage >= slurmPageCount" @click="slurmPage++"><Icon name="chevron" :size="14" /></button></div><time v-if="slurm?.lastUpdated">更新于 {{ timeText(slurm.lastUpdated) }}</time></div></div>
        </section>

        <section class="tunnel-card card"><div class="section-heading"><div class="heading-label"><span class="section-icon blue"><Icon name="exchange" :size="20" /></span><h2>端口隧道</h2><span class="count-tag">{{ selected.tunnels.length }}</span></div><button data-testid="add-tunnel" class="button secondary small" @click="openTunnel()"><Icon name="plus" :size="15" />添加隧道</button></div>
          <div v-if="selected.tunnels.length" class="tunnel-table-wrap"><table class="tunnel-table"><thead><tr><th>名称 / 类型</th><th>端口映射</th><th>状态</th><th class="align-right">操作</th></tr></thead><tbody><tr v-for="tunnel in selected.tunnels" :key="tunnel.id"><td><strong class="tunnel-name">{{ tunnel.name }}</strong><span class="direction-label" :class="tunnel.direction"><Icon name="arrow" :size="11" />{{ tunnel.direction === 'local' ? '正向隧道' : '反向隧道' }}</span></td><td class="mapping-cell"><div class="mapping-route"><span><small>{{ tunnel.direction === 'local' ? '本机' : '服务器' }}</small><strong>{{ tunnel.bindPort }}</strong></span><Icon name="arrow" :size="17" /><span><small>{{ tunnel.direction === 'local' ? '远端' : '本机侧' }}</small><strong>{{ tunnel.targetPort }}</strong></span></div><span class="mapping-address" :title="`${tunnel.bindHost} → ${tunnel.targetHost}`">{{ tunnel.bindHost }} → {{ tunnel.targetHost }}</span></td><td><div class="tunnel-status" :class="!tunnel.enabled ? 'neutral' : statusClass(tunnelState(tunnel)?.status)"><i class="status-dot" :class="!tunnel.enabled ? 'neutral' : statusClass(tunnelState(tunnel)?.status)" />{{ tunnelStatus(tunnel) }}</div><small class="tunnel-error" :title="tunnelDetail(tunnel)">{{ tunnelDetail(tunnel) }}</small></td><td><div class="tunnel-actions"><button class="switch small-switch" :class="{ on: tunnel.enabled }" role="switch" :aria-checked="tunnel.enabled" :aria-label="`${tunnel.enabled ? '暂停' : '启用'}隧道 ${tunnel.name}`" :disabled="busy[`tunnel:${tunnel.id}`]" @click="toggleTunnel(tunnel)"><span /></button><button class="icon-button" :title="`重启隧道 ${tunnel.name}`" :aria-label="`重启隧道 ${tunnel.name}`" :disabled="!connected || !tunnel.enabled || busy[`restart:${tunnel.id}`]" @click="action(`restart:${tunnel.id}`, () => api!.restartTunnel(selected!.id, tunnel.id))"><Icon name="refresh" :size="16" /></button><button class="icon-button" :title="`编辑隧道 ${tunnel.name}`" :aria-label="`编辑隧道 ${tunnel.name}`" @click="openTunnel(tunnel)"><Icon name="edit" :size="16" /></button><button class="icon-button danger-hover" :title="`移除隧道 ${tunnel.name}`" :aria-label="`移除隧道 ${tunnel.name}`" @click="deleting = { kind: 'tunnel', id: tunnel.id, name: tunnel.name }"><Icon name="trash" :size="16" /></button></div></td></tr></tbody></table></div>
          <div v-else class="tunnel-empty"><span class="empty-tunnel-icon"><Icon name="exchange" :size="25" /></span><div><strong>为这台设备添加第一条隧道</strong><p>正向访问远端服务，或反向连接本机服务。</p></div><button class="text-button" @click="openTunnel()">添加隧道<Icon name="arrow" :size="15" /></button></div>
          <div class="tunnel-footer"><Icon name="shield" :size="14" /><span>异常时先关闭旧转发，再恢复连接。远端端口未释放时会自动等待重试。</span></div>
        </section>

        <section class="events-card card"><div class="section-heading"><div class="heading-label"><span class="section-icon gray"><Icon name="activity" :size="19" /></span><h2>系统日志</h2><span class="light-tag">{{ selected.name }}</span></div></div><div v-if="recentEvents.length" class="event-list" role="log" aria-label="系统日志"><div v-for="event in recentEvents" :key="event.id" class="event-line"><time>{{ timeText(event.time) }}</time><span class="event-level" :class="['error', 'warn', 'warning'].includes(event.level) ? event.level === 'error' ? 'negative' : 'pending' : 'positive'"><Icon :name="event.level === 'error' ? 'alert' : ['warn', 'warning'].includes(event.level) ? 'info' : 'check'" :size="13" /></span><span>{{ event.message }}</span></div></div><div v-else class="event-empty"><i class="status-dot neutral" />暂无事件，连接设备后将在这里记录状态变化。</div></section>
      </template>
      <section v-else class="welcome-empty card"><div class="welcome-icon"><Icon name="server" :size="40" /></div><span class="eyebrow">A QUIET PLACE FOR YOUR SERVERS</span><h2>从你的第一台设备开始</h2><p>安全连接多台服务器，管理隧道，随时了解空闲算力。</p><button class="button primary" @click="openDevice()"><Icon name="plus" :size="17" />添加设备</button><div class="welcome-features"><span><Icon name="shield" :size="16" />密码与密钥认证</span><span><Icon name="exchange" :size="16" />双向隧道与自动恢复</span><span><Icon name="chip" :size="16" />定时监测显卡</span></div></section>
      <footer class="workspace-footer"><span>Lion SSH</span></footer>
    </main>
  </div>

  <Transition name="drawer"><div v-if="drawer" class="drawer-backdrop" @click.self="closeDrawer()"><section class="drawer" role="dialog" aria-modal="true" :aria-labelledby="drawer === 'device' ? 'device-form-title' : 'tunnel-form-title'"><div class="drawer-header"><div><span class="eyebrow">{{ drawer === 'device' ? 'DEVICE SETTINGS' : 'TUNNEL SETTINGS' }}</span><h2 :id="drawer === 'device' ? 'device-form-title' : 'tunnel-form-title'">{{ editing ? '编辑' : '添加' }}{{ drawer === 'device' ? '设备' : '隧道' }}</h2></div><button class="icon-button" aria-label="关闭设置" @click="closeDrawer()"><Icon name="close" /></button></div>
      <form v-if="drawer === 'device'" class="drawer-form" @submit.prevent="saveDevice"><div class="form-body"><div class="form-section-title"><Icon name="server" :size="16" />连接信息</div><label class="field">设备名称<input v-model="deviceDraft.name" placeholder="例如：训练服务器" maxlength="80" required /></label><div class="form-row host-row"><label class="field">服务器地址<input v-model="deviceDraft.host" placeholder="IP 地址或主机名" maxlength="255" required /></label><label class="field">端口<input v-model.number="deviceDraft.port" type="number" min="1" max="65535" required /></label></div><label class="field">用户名<input v-model="deviceDraft.username" placeholder="例如：user" maxlength="128" required autocomplete="off" /></label><div class="form-section-title"><Icon name="shield" :size="16" />认证方式</div><div class="segmented-control"><button type="button" :class="{ active: deviceDraft.authType === 'key' }" @click="deviceDraft.authType = 'key'"><Icon name="key" :size="16" />SSH 密钥</button><button type="button" :class="{ active: deviceDraft.authType === 'password' }" @click="deviceDraft.authType = 'password'"><Icon name="lock" :size="16" />密码</button></div><template v-if="deviceDraft.authType === 'key'"><label class="field">私钥文件<div class="input-with-button"><input v-model="deviceDraft.keyPath" placeholder="选择本机私钥文件" required /><button type="button" class="icon-button" title="选择私钥文件" aria-label="选择私钥文件" @click="chooseKey()"><Icon name="folder" :size="18" /></button></div><small>支持 OpenSSH 私钥，文件内容仅用于建立连接。</small></label><label class="field">密钥口令<span class="field-optional">可选</span><input v-model="devicePassphrase" type="password" :placeholder="editing ? '留空使用已有口令或未加密密钥' : '私钥未加密时留空'" autocomplete="new-password" /></label></template><label v-else class="field">连接密码<div class="input-with-button"><input v-model="devicePassword" :type="passwordVisible ? 'text' : 'password'" :placeholder="editing ? '留空保留已有密码' : '可稍后在连接时输入'" autocomplete="new-password" /><button type="button" class="icon-button" :aria-label="passwordVisible ? '隐藏密码' : '显示密码'" @click="passwordVisible = !passwordVisible"><Icon name="eye" :size="17" /></button></div></label><label class="checkbox-row"><input v-model="rememberSecret" type="checkbox" /><span>记住{{ deviceDraft.authType === 'password' ? '密码' : '密钥口令' }}<small>使用系统加密存储；不勾选则仅在本次运行中使用。</small></span></label><div class="form-section-title"><Icon name="activity" :size="16" />连接与监测</div><label class="checkbox-row"><input v-model="deviceDraft.autoReconnect" type="checkbox" /><span>断线后自动重连<small>保留配置，关闭旧会话后按递增间隔恢复。</small></span></label><label class="checkbox-row"><input v-model="deviceDraft.gpu.enabled" type="checkbox" /><span>自动监测 NVIDIA 显卡<small>只执行 nvidia-smi 读取显卡状态。</small></span></label><label class="field inline-field">检测间隔<div><input v-model.number="deviceDraft.gpu.intervalSec" type="number" min="2" max="300" required /><span>秒</span></div></label><div class="form-note"><Icon name="info" :size="17" /><span>首次连接需要确认服务器指纹。连接后，隧道和显卡监测会随设备独立运行。</span></div><div v-if="formError" class="form-error" role="alert">{{ formError }}</div></div><div class="drawer-footer"><button type="button" class="button secondary" @click="closeDrawer()">取消</button><button type="submit" data-testid="device-save" class="button primary" :disabled="busy.save">{{ busy.save ? '正在保存' : '保存设备' }}<Icon name="check" :size="16" /></button></div></form>
      <form v-else class="drawer-form" @submit.prevent="saveTunnel"><div class="form-body"><div class="form-context"><Icon name="server" :size="16" />{{ selected?.name }}<span>{{ selected?.host }}</span></div><label class="field">隧道名称<span class="field-optional">可选</span><input v-model="tunnelDraft.name" placeholder="例如：TensorBoard" maxlength="80" /></label><label class="field">转发方向</label><div class="segmented-control"><button type="button" :class="{ active: tunnelDraft.direction === 'local' }" @click="tunnelDraft.direction = 'local'"><Icon name="arrow" :size="16" />正向隧道</button><button type="button" :class="{ active: tunnelDraft.direction === 'remote' }" @click="tunnelDraft.direction = 'remote'"><Icon name="exchange" :size="16" />反向隧道</button></div><div class="direction-explanation"><span>{{ tunnelDraft.direction === 'local' ? '本机端口' : '服务器端口' }}</span><Icon name="arrow" :size="16" /><span>SSH</span><Icon name="arrow" :size="16" /><span>{{ tunnelDraft.direction === 'local' ? '远端服务' : '本机侧服务' }}</span></div><div class="form-section-title"><Icon name="link" :size="16" />{{ tunnelDraft.direction === 'local' ? '本机监听' : '服务器监听' }}</div><div class="form-row host-row"><label class="field">监听地址<input v-model="tunnelDraft.bindHost" placeholder="127.0.0.1" required maxlength="255" /></label><label class="field">监听端口<input v-model.number="tunnelDraft.bindPort" type="number" min="1" max="65535" required /></label></div><div class="form-section-title"><Icon name="arrow" :size="16" />{{ tunnelDraft.direction === 'local' ? '远端目标' : '本机侧目标' }}</div><div class="form-row host-row"><label class="field">目标地址<input v-model="tunnelDraft.targetHost" placeholder="127.0.0.1" required maxlength="255" /></label><label class="field">目标端口<input v-model.number="tunnelDraft.targetPort" type="number" min="1" max="65535" required /></label></div><label class="field inline-field">连通性检测间隔<div><input v-model.number="tunnelDraft.healthIntervalSec" type="number" min="5" max="300" required /><span>秒</span></div></label><label class="checkbox-row"><input v-model="tunnelDraft.enabled" type="checkbox" /><span>启用这条隧道<small>设备连接后自动建立；手动暂停后不会自动重启。</small></span></label><div class="form-note"><Icon name="info" :size="17" /><span>{{ tunnelDraft.direction === 'remote' ? '远端监听地址能否对外访问取决于服务器的转发权限。无法探测完整路径时，状态会显示未验证。' : '目标地址从服务器一侧访问。目标服务未启动时会提示不可用，并继续检测。' }}</span></div><div v-if="formError" class="form-error" role="alert">{{ formError }}</div></div><div class="drawer-footer"><button type="button" class="button secondary" @click="closeDrawer()">取消</button><button type="submit" data-testid="tunnel-save" class="button primary" :disabled="busy.save">{{ busy.save ? '正在保存' : '保存隧道' }}<Icon name="check" :size="16" /></button></div></form>
    </section></div></Transition>

  <div v-if="credentialProfile" class="modal-backdrop" @click.self="credentialProfile = null"><form class="modal credential-modal" role="dialog" aria-modal="true" aria-labelledby="credential-title" @submit.prevent="submitCredentials"><div class="modal-top"><span class="modal-icon blue"><Icon name="key" :size="25" /></span><button class="icon-button" type="button" aria-label="关闭认证" @click="credentialProfile = null"><Icon name="close" :size="19" /></button></div><h2 id="credential-title">连接 {{ credentialProfile.name }}</h2><p>{{ credentialProfile.username }}@{{ credentialProfile.host }}:{{ credentialProfile.port }}</p><label v-if="credentialProfile.authType === 'password'" class="field">连接密码<input v-model="credentialPassword" type="password" placeholder="输入密码，或留空使用已记住的密码" autocomplete="current-password" /></label><label v-else class="field">密钥口令<input v-model="credentialPassphrase" type="password" placeholder="未加密私钥可留空" autocomplete="current-password" /></label><label class="checkbox-row"><input v-model="credentialRemember" type="checkbox" /><span>使用系统加密记住凭据</span></label><div class="modal-actions"><button type="button" class="button secondary" @click="credentialProfile = null">取消</button><button type="submit" class="button primary"><Icon name="link" :size="16" />连接</button></div></form></div>
  <div v-if="deleting" class="modal-backdrop" @click.self="!busy.delete && (deleting = null)"><section class="modal confirm-modal" role="alertdialog" aria-modal="true" aria-labelledby="delete-title"><span class="modal-icon red"><Icon name="trash" :size="25" /></span><h2 id="delete-title">移除{{ deleting.kind === 'device' ? '设备' : '隧道' }}？</h2><p>「{{ deleting.name }}」的{{ deleting.kind === 'device' ? '连接会先关闭，然后移除设备配置。' : '转发会先停止，然后移除隧道配置。' }}</p><div class="modal-actions"><button class="button secondary" :disabled="busy.delete" @click="deleting = null">取消</button><button class="button danger" :disabled="busy.delete" @click="deleteConfirmed()">{{ busy.delete ? '正在移除' : '确认移除' }}</button></div></section></div>
  <div v-if="jobCancellation" class="modal-backdrop" @click.self="!busy.cancelJob && (jobCancellation = null)"><section class="modal confirm-modal" data-testid="job-cancel-confirmation" role="alertdialog" aria-modal="true" aria-labelledby="job-cancel-title"><span class="modal-icon red"><Icon name="trash" :size="25" /></span><h2 id="job-cancel-title">删除自己的作业？</h2><p>将取消排队中的作业，或终止正在运行的作业。</p><dl class="confirmation-details"><div><dt>服务器</dt><dd>{{ jobCancellation.profileName }}</dd></div><div><dt>作业编号</dt><dd>{{ jobCancellation.preview.jobId }}</dd></div><div><dt>作业名称</dt><dd>{{ jobCancellation.jobName || '—' }}</dd></div><div><dt>所属用户</dt><dd>{{ jobCancellation.preview.owner }}</dd></div></dl><div v-if="confirmationExpired" class="confirmation-warning" role="status">确认已过期，请取消后重新检查作业。</div><div class="modal-actions"><button class="button secondary" data-testid="job-cancel-dismiss" :disabled="busy.cancelJob" @click="jobCancellation = null">取消</button><button class="button danger" data-testid="job-cancel-approve" :disabled="busy.cancelJob || confirmationExpired" @click="cancelJobConfirmed()">{{ busy.cancelJob ? '正在删除' : '确认删除作业' }}</button></div></section></div>
  <div v-if="tmpCleanup" class="modal-backdrop" @click.self="!busy.cleanupTmp && (tmpCleanup = null)"><section class="modal confirm-modal cleanup-modal" data-testid="tmp-cleanup-confirmation" role="alertdialog" aria-modal="true" aria-labelledby="tmp-cleanup-title"><span class="modal-icon red"><Icon name="trash" :size="25" /></span><h2 id="tmp-cleanup-title">清理自己的临时文件？</h2><p>将删除 /tmp 内属于你的文件及可移除的空目录，可能影响正在使用这些文件的程序。此操作无法撤销。</p><dl class="confirmation-details"><div><dt>服务器</dt><dd>{{ tmpCleanup.profileName }}</dd></div><div><dt>目录</dt><dd>{{ tmpCleanup.preview.root }}</dd></div><div><dt>所属用户</dt><dd>{{ tmpCleanup.preview.username }}（UID {{ tmpCleanup.preview.uid }}）</dd></div><div><dt>文件及链接</dt><dd>{{ tmpCleanup.preview.fileCount.toLocaleString() }} 项<small>另清理可移除的本人空目录</small></dd></div><div><dt>我的空间占用</dt><dd>{{ bytesText(tmpCleanup.preview.bytes) }}</dd></div></dl><div v-if="tmpCleanup.preview.warnings?.length" class="confirmation-warning"><div v-for="warning in tmpCleanup.preview.warnings" :key="warning">{{ warning }}</div></div><div v-if="confirmationExpired" class="confirmation-warning" role="status">确认已过期，请取消后重新统计文件。</div><div class="modal-actions"><button class="button secondary" data-testid="tmp-cleanup-dismiss" :disabled="busy.cleanupTmp" @click="tmpCleanup = null">取消</button><button class="button danger" data-testid="tmp-cleanup-approve" :disabled="busy.cleanupTmp || confirmationExpired || !tmpCleanup.preview.fileCount" @click="cleanupTmpConfirmed()">{{ busy.cleanupTmp ? '正在清理' : '确认清理 /tmp' }}</button></div></section></div>
  <div v-if="hostRequest" class="modal-backdrop host-backdrop"><section class="modal host-modal" role="alertdialog" aria-modal="true" aria-labelledby="host-title"><span class="modal-icon" :class="hostRequest.changed ? 'red' : 'blue'"><Icon :name="hostRequest.changed ? 'alert' : 'shield'" :size="28" /></span><h2 id="host-title">{{ hostRequest.changed ? '服务器指纹已变化' : '确认服务器身份' }}</h2><p>{{ hostRequest.changed ? '当前服务器密钥与此前记录不同。请核对来源，确认服务器是否更换过密钥。' : '首次连接这台设备，请与服务器管理员提供的指纹核对。确认后会记住此服务器身份。' }}</p><div class="fingerprint-box"><span>{{ hostRequest.host }}:{{ hostRequest.port }}<small>{{ hostRequest.keyType }}</small></span><code>{{ hostRequest.fingerprint }}</code></div><div class="modal-actions"><button class="button secondary" :disabled="busy.trust" @click="trustHost(false)">取消连接</button><button class="button" :class="hostRequest.changed ? 'danger' : 'primary'" data-testid="host-accept" :disabled="busy.trust" @click="trustHost(true)">确认并信任<Icon name="check" :size="16" /></button></div></section></div>
  <div v-if="shutdown" class="shutdown-backdrop" data-testid="shutdown-overlay"><section class="shutdown-panel" role="status" aria-live="assertive"><span class="shutdown-spinner" aria-hidden="true" /><h2>正在安全退出</h2><p>{{ shutdown.message }}</p><small>连接关闭后，软件会自动退出</small></section></div>
  <div class="toast-stack" :inert="Boolean(shutdown)" role="status" aria-live="polite"><TransitionGroup name="toast"><div v-for="notice in notices" :key="notice.id" class="toast" :class="{ error: notice.error }"><Icon :name="notice.error ? 'alert' : 'check'" :size="18" /><span>{{ notice.message }}</span><button class="icon-button" aria-label="关闭通知" @click="notices = notices.filter(n => n.id !== notice.id)"><Icon name="close" :size="15" /></button></div></TransitionGroup></div>
</template>
