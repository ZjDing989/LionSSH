import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { LionApi } from './types';

/** The web preview may inject a mock; desktop requests always use Tauri IPC. */
function createTauriApi(): LionApi {
  const subscriptions = new Set<() => Promise<void>>();

  function subscribe<T>(channel: string, callback: (payload: T) => void): () => void {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    let registration: Promise<void> | undefined;
    const ensureListening = () => {
      if (!registration) {
        registration = listen<T>(channel, event => { if (!disposed) callback(event.payload); })
          .then(remove => { if (disposed) remove(); else unlisten = remove; })
          .catch(error => { registration = undefined; throw error; });
      }
      return registration;
    };
    subscriptions.add(ensureListening);
    // getInitial awaits registrations and reports failures through the startup UI.
    void ensureListening().catch(() => {});
    return () => {
      disposed = true;
      subscriptions.delete(ensureListening);
      unlisten?.();
      unlisten = undefined;
    };
  }

  const request = <T>(method: string, ...args: unknown[]) => {
    // JSON arrays turn undefined into null; omit absent optional credentials.
    while (args.length && args[args.length - 1] === undefined) args.pop();
    return invoke<T>('lion_request', { method, args });
  };
  return {
    getInitial: async () => {
      // Establish every event listener before the snapshot can start SSH work.
      await Promise.all([...subscriptions].map(ensureListening => ensureListening()));
      return request<Awaited<ReturnType<LionApi['getInitial']>>>('initial');
    },
    getUpdateState: () => request('get-update-state'),
    checkUpdate: () => request('check-update'),
    installUpdate: () => request('install-update'),
    openUpdateDownload: () => request('open-update-download'),
    onUpdate: callback => subscribe('lion:update', callback),
    setTheme: preference => request('set-theme', preference),
    saveProfile: (profile, secrets) => request('save-profile', profile, secrets),
    deleteProfile: id => request('delete-profile', id),
    connect: (id, secrets) => request('connect', id, secrets),
    disconnect: id => request('disconnect', id),
    refreshGpu: id => request('refresh-gpu', id),
    refreshSlurm: id => request('refresh-slurm', id),
    refreshStorage: id => request('refresh-storage', id),
    refreshCodex: id => request('refresh-codex', id),
    installCodex: id => request('install-codex', id),
    listTerminals: id => request('terminal-list', id),
    openTerminal: (id, cols, rows) => request('terminal-open', id, cols, rows),
    readTerminal: (id, terminalId, cursor) => request('terminal-read', id, terminalId, cursor),
    writeTerminal: (id, terminalId, data) => request('terminal-write', id, terminalId, data),
    resizeTerminal: (id, terminalId, cols, rows) => request('terminal-resize', id, terminalId, cols, rows),
    renameTerminal: (id, terminalId, title) => request('terminal-rename', id, terminalId, title),
    closeTerminal: (id, terminalId) => request('terminal-close', id, terminalId),
    prepareJobCancel: (id, jobId) => request('prepare-job-cancel', id, jobId),
    cancelJob: (id, jobId, token) => request('cancel-job', id, jobId, token),
    getJobPriority: (id, jobId) => request('get-job-priority', id, jobId),
    prepareTmpCleanup: id => request('prepare-tmp-cleanup', id),
    cleanupTmp: (id, token) => request('cleanup-tmp', id, token),
    setSlurmScope: (id, scope) => request('set-slurm-scope', id, scope),
    updateGpu: (id, gpu) => request('update-gpu', id, gpu),
    setTunnel: (id, tunnelId, enabled) => request('set-tunnel', id, tunnelId, enabled),
    restartTunnel: (id, tunnelId) => request('restart-tunnel', id, tunnelId),
    pickKey: () => request('pick-key'),
    trustHost: (id, accepted, fingerprint) => request('trust-host', id, accepted, fingerprint),
    onState: callback => subscribe('lion:state', callback),
    onHostKey: callback => subscribe('lion:host-key', callback),
    onTheme: callback => subscribe('lion:theme', callback),
    onShutdown: callback => subscribe('lion:shutdown', callback),
    onBackendError: callback => subscribe('lion:backend-error', callback),
  };
}

export const api = window.lion ?? (isTauri() ? createTauriApi() : undefined);
