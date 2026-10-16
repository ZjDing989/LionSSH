export interface GpuSettings {
  enabled: boolean;
  intervalSec: number;
  idleUtil: number;
  idleMemoryMiB: number;
  stableSamples: number;
}

export interface Tunnel {
  id: string;
  name: string;
  direction: 'local' | 'remote';
  bindHost: string;
  bindPort: number;
  targetHost: string;
  targetPort: number;
  enabled: boolean;
  healthIntervalSec: number;
}

export interface Profile {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authType: 'key' | 'password';
  keyPath: string;
  autoReconnect: boolean;
  hasSavedSecret?: boolean;
  gpu: GpuSettings;
  tunnels: Tunnel[];
}

export interface GpuCard {
  index: number;
  uuid: string;
  name: string;
  memoryTotalMiB: number | null;
  memoryUsedMiB: number | null;
  utilization: number | null;
  computeProcesses: number | { pid?: number; name?: string; memoryUsedMiB?: number | null }[];
  idleCandidate?: boolean;
  idle: boolean;
  stableCount: number;
}

export type SlurmScope = 'mine' | 'all';
export interface SlurmJob {
  id: string;
  partition: string;
  name: string;
  user: string;
  stateCode: string;
  stateLabel: string;
  stateTone: 'positive' | 'pending' | 'negative' | 'neutral';
  elapsed: string;
  nodes: number | null;
  nodeList: string;
  reasonCode: string;
  reasonLabel: string;
  cluster?: string;
}
export interface SlurmState {
  availability: 'unknown' | 'available' | 'unavailable';
  status: 'unknown' | 'loading' | 'ready' | 'error';
  scope: SlurmScope;
  jobs: SlurmJob[];
  lastUpdated?: number | string | null;
  nextPollAt?: number | string | null;
  intervalSec: number;
  error?: string | null;
  availabilityReason?: string | null;
}

export interface StorageState {
  status: 'unknown' | 'loading' | 'ready' | 'error';
  homePath: string;
  homeBytes: number | null;
  tmpBytes: number | null;
  totalBytes: number | null;
  lastUpdated?: number | string | null;
  nextPollAt?: number | string | null;
  intervalSec: number;
  error?: string | null;
  warnings?: string[];
  partial?: boolean;
}
export interface JobCancelPreview {
  jobId: string;
  owner: string;
  priority?: number;
  token: string;
  expiresAt: number;
}
export interface TmpCleanupPreview {
  token: string;
  expiresAt: number;
  username: string;
  uid: number;
  root: '/tmp';
  fileCount: number;
  bytes: number | null;
  warnings?: string[];
}

export interface CodexState {
  status: 'unknown' | 'checking' | 'ready' | 'installing' | 'restarting' | 'error';
  installed: boolean | null;
  currentVersion: string | null;
  latestVersion: string | null;
  updateAvailable: boolean;
  installMethod: 'npm' | 'native' | 'unknown' | null;
  executablePath: string | null;
  canInstall: boolean;
  unsupportedReason: string | null;
  lastChecked: number | null;
  lastResult: string | null;
  error: string | null;
  restartStatus: 'not-running' | 'restarted' | 'already-current' | 'manual' | 'failed' | null;
  restartMessage: string | null;
  log: string[];
}

export interface TerminalSession {
  id: string;
  profileId: string;
  title: string;
  defaultTitleNumber?: number | null;
  status: 'opening' | 'running' | 'exited' | 'error';
  exitCode: number | null;
  error: string | null;
  cols: number;
  rows: number;
}
export interface TerminalRead {
  data: string;
  cursor: number;
  startCursor: number;
  session: TerminalSession;
}

export interface ServerState {
  profileId: string;
  status: string;
  error?: string;
  gpu: {
    status: string;
    availability?: 'unknown' | 'available' | 'absent' | 'unavailable';
    availabilityReason?: string | null;
    availabilityCheckedAt?: number | string | null;
    cards: GpuCard[];
    idleCount: number;
    total: number;
    lastUpdated?: number | string | null;
    nextPollAt?: number | string | null;
    intervalSec: number;
    initialConfirming?: boolean;
    error?: string;
  };
  slurm?: SlurmState;
  storage?: StorageState;
  codex?: CodexState;
  tunnels: { id: string; status: string; error?: string; lastChecked?: number | string; healthVerified?: boolean; healthStatus?: 'pending' | 'tcp-ok' | 'probe-restricted' | 'unverified' | 'target-unavailable'; healthDetail?: string }[];
  events: { id: string; time: number | string; level: string; message: string }[];
}

export interface HostKeyRequest {
  profileId: string;
  fingerprint: string;
  keyType: string;
  host: string;
  port: number;
  changed: boolean;
}

export interface Secrets { password?: string; passphrase?: string; remember?: boolean }

export type ThemePreference = 'light' | 'dark' | 'system';
export interface ThemeState { preference: ThemePreference; resolved: 'light' | 'dark' }
export type LanguagePreference = 'system' | 'zh' | 'en';
export interface LanguageState { preference: LanguagePreference; resolved: 'zh' | 'en' }
export interface ShutdownNotice { phase: 'closing'; message: string }

export interface UpdateState {
  status: string;
  currentVersion: string;
  version: string | null;
  notes: string | null;
  publishedAt: string | null;
  error: string | null;
  downloaded: number;
  total: number | null;
  lastChecked: number | null;
  nextCheckAt: number | null;
  portable: boolean;
  configured: boolean;
  manualOnly?: boolean;
  releaseUrl?: string;
}

export interface LionApi {
  readonly initialTheme?: ThemeState;
  readonly initialLanguage?: LanguageState;
  getInitial(): Promise<{ profiles: Profile[]; states: ServerState[]; version: string; theme?: ThemeState; language?: LanguageState; shutdown?: ShutdownNotice | null; pendingHostKeys?: HostKeyRequest[]; update?: UpdateState }>;
  getUpdateState?(): Promise<UpdateState>;
  checkUpdate?(): Promise<UpdateState>;
  installUpdate?(): Promise<void>;
  openUpdateDownload?(): Promise<void>;
  onUpdate?(callback: (state: UpdateState) => void): () => void;
  setTheme(preference: ThemePreference): Promise<ThemeState>;
  setLanguage?(preference: LanguagePreference): Promise<LanguageState>;
  onLanguage?(callback: (language: LanguageState) => void): () => void;
  saveProfile(profile: Profile, secrets?: Secrets): Promise<Profile>;
  deleteProfile(id: string): Promise<void>;
  connect(id: string, secrets?: Secrets): Promise<void>;
  disconnect(id: string): Promise<void>;
  refreshGpu(id: string): Promise<void>;
  refreshSlurm(id: string): Promise<void>;
  setSlurmScope(id: string, scope: SlurmScope): Promise<void>;
  prepareJobCancel(id: string, jobId: string): Promise<JobCancelPreview>;
  cancelJob(id: string, jobId: string, token: string): Promise<{ jobId: string; cancelled: true }>;
  getJobPriority(id: string, jobId: string): Promise<{ jobId: string; priority: number | null; priorities?: number[] }>;
  refreshStorage(id: string): Promise<void>;
  refreshCodex(id: string): Promise<CodexState>;
  installCodex(id: string): Promise<CodexState>;
  listTerminals(profileId: string): Promise<TerminalSession[]>;
  openTerminal(profileId: string, cols: number, rows: number): Promise<TerminalSession>;
  readTerminal(profileId: string, id: string, cursor: number): Promise<TerminalRead>;
  writeTerminal(profileId: string, id: string, data: string): Promise<void>;
  resizeTerminal(profileId: string, id: string, cols: number, rows: number): Promise<void>;
  renameTerminal(profileId: string, id: string, title: string): Promise<TerminalSession>;
  closeTerminal(profileId: string, id: string): Promise<void>;
  prepareTmpCleanup(id: string): Promise<TmpCleanupPreview>;
  cleanupTmp(id: string, token: string): Promise<{ deletedCount: number | null; cleanupStatus: 'complete' | 'partial' | 'unknown'; warnings?: string[] }>;
  updateGpu(id: string, gpu: GpuSettings): Promise<void>;
  setTunnel(id: string, tunnelId: string, enabled: boolean): Promise<void>;
  restartTunnel(id: string, tunnelId: string): Promise<void>;
  pickKey(): Promise<string | null>;
  trustHost(id: string, accepted: boolean, fingerprint: string): Promise<void>;
  onState(callback: (state: ServerState) => void): () => void;
  onHostKey(callback: (request: HostKeyRequest) => void): () => void;
  onTheme(callback: (theme: ThemeState) => void): () => void;
  onShutdown(callback: (notice: ShutdownNotice) => void): () => void;
  onBackendError?(callback: (error: { message: string }) => void): () => void;
}

declare global {
  interface Window { lion?: LionApi }
}
