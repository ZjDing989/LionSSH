<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import type { LionApi, TerminalSession } from '../types';

const props = defineProps<{
  api: LionApi | undefined;
  session: TerminalSession;
  active: boolean;
  connected: boolean;
  disabled: boolean;
}>();
const emit = defineEmits<{ session: [session: TerminalSession]; error: [error: string] }>();
const surface = ref<HTMLDivElement | null>(null);
const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');
const writable = computed(() => props.connected && props.session.status === 'running' && !props.disabled);
const ended = computed(() => props.session.status === 'exited' || props.session.status === 'error');
const notice = computed(() => props.session.status === 'error' ? props.session.error || '终端连接异常，可新建终端重试。' : props.session.status === 'exited' ? props.session.exitCode === null ? '终端会话已结束。' : `终端会话已结束，退出码 ${props.session.exitCode}。` : !props.connected ? '连接已断开，终端内容已保留。重新连接后可新建终端。' : props.session.status === 'opening' ? '正在连接终端…' : '');

let terminal: Terminal | undefined;
let fitAddon: FitAddon | undefined;
let resizeObserver: ResizeObserver | undefined;
let pollTimer: ReturnType<typeof setTimeout> | undefined;
let resizeTimer: ReturnType<typeof setTimeout> | undefined;
let cursor = 0;
let disposed = false;
let reading = false;
let drained = false;
let pollFailures = 0;
let pendingBytes = 0;
let writes = Promise.resolve();
let resizeInFlight = false;
let resizePending: { cols: number; rows: number } | undefined;
let lastSize = { cols: props.session.cols, rows: props.session.rows };
let clipboardGeneration = 0;
const pendingRenders = new Set<() => void>();

function cleanError(error: unknown) { return (error instanceof Error ? error.message : String(error)).replace(/^Error:\s*/, ''); }
function applyCursorMotion() {
  if (terminal) terminal.options.cursorBlink = !reducedMotion.matches;
}
function copySelection() {
  const target = terminal;
  const text = target?.getSelection();
  if (!target || !text || disposed || !props.active || props.disabled) return;
  if (!navigator.clipboard?.writeText) { emit('error', '当前环境无法访问剪贴板，请使用复制快捷键。'); return; }
  const generation = clipboardGeneration;
  // Start in the user event so the webview retains clipboard user activation.
  void navigator.clipboard.writeText(text).then(() => {
    if (!disposed && terminal === target && generation === clipboardGeneration && target.getSelection() === text) target.clearSelection();
  }).catch(error => { if (!disposed && generation === clipboardGeneration) emit('error', `复制失败：${cleanError(error)}`); });
}
function pasteClipboard() {
  const target = terminal;
  if (!target || disposed || !props.active || !writable.value) return;
  if (!navigator.clipboard?.readText) { emit('error', '当前环境无法访问剪贴板，请使用粘贴快捷键。'); return; }
  const generation = clipboardGeneration;
  void navigator.clipboard.readText().then(text => {
    if (!disposed && terminal === target && generation === clipboardGeneration && props.active && writable.value && text) {
      target.focus();
      target.paste(text);
    }
  }).catch(error => { if (!disposed && generation === clipboardGeneration) emit('error', `粘贴失败：${cleanError(error)}`); });
}
function terminalContextMenu(event: MouseEvent) {
  event.preventDefault();
  // Own this gesture before xterm moves its hidden textarea or sends a mouse report.
  event.stopPropagation();
  if (!event.isTrusted || disposed || !props.active || props.disabled) return;
  if (terminal?.hasSelection()) copySelection();
  else pasteClipboard();
}
function terminalRightButton(event: MouseEvent) {
  if (event.button !== 2) return;
  event.preventDefault();
  event.stopPropagation();
}
function encodeBytes(bytes: Uint8Array) {
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  return btoa(binary);
}
function decodeBytes(data: string) { return Uint8Array.from(atob(data), char => char.charCodeAt(0)); }
function sendBytes(bytes: Uint8Array) {
  if (!props.api?.writeTerminal || !writable.value || disposed || !bytes.length) return;
  if (pendingBytes + bytes.length > 1024 * 1024) { emit('error', '终端输入较多，请等待服务器处理后重试。'); return; }
  pendingBytes += bytes.length;
  // Keep keystrokes and multiline paste in exactly the order they were entered.
  for (let offset = 0; offset < bytes.length; offset += 32768) {
    const part = bytes.slice(offset, offset + 32768);
    writes = writes.then(async () => {
      if (!disposed && writable.value) await props.api!.writeTerminal(props.session.profileId, props.session.id, encodeBytes(part));
    }).catch(error => { if (!disposed && !props.disabled) emit('error', cleanError(error)); })
      .finally(() => { pendingBytes -= part.length; });
  }
}
function schedulePoll(delay = props.active ? 100 : 500) {
  clearTimeout(pollTimer);
  if (!disposed && !drained && !props.disabled) pollTimer = setTimeout(() => void poll(), delay);
}
async function renderBytes(data: Uint8Array) {
  if (!terminal || disposed || !data.length) return;
  await new Promise<void>(resolve => {
    const finish = () => { pendingRenders.delete(finish); resolve(); };
    pendingRenders.add(finish);
    terminal!.write(data, finish);
  });
}
async function poll() {
  if (disposed || drained || reading || props.disabled || !props.api?.readTerminal) return;
  reading = true;
  let immediate = false;
  try {
    const result = await props.api.readTerminal(props.session.profileId, props.session.id, cursor);
    if (disposed) return;
    if (result.startCursor > cursor) await renderBytes(new TextEncoder().encode('\r\n\x1b[2m[较早的输出已超出缓冲范围]\x1b[0m\r\n'));
    const data = decodeBytes(result.data);
    await renderBytes(data);
    if (disposed) return;
    cursor = result.cursor;
    emit('session', result.session);
    pollFailures = 0;
    drained = ['exited', 'error'].includes(result.session.status) && data.length === 0;
    immediate = data.length >= 65536;
  } catch (error) {
    pollFailures += 1;
    if (!disposed && !props.disabled && pollFailures === 1) emit('error', cleanError(error));
  } finally {
    reading = false;
    schedulePoll(pollFailures ? Math.min(5000, pollFailures * 1000) : immediate ? 0 : props.active ? 100 : 500);
  }
}
async function flushResize() {
  if (resizeInFlight || disposed || !props.api?.resizeTerminal) return;
  resizeInFlight = true;
  try {
    while (resizePending && !disposed) {
      const size = resizePending;
      resizePending = undefined;
      if (!writable.value || size.cols === lastSize.cols && size.rows === lastSize.rows) continue;
      try {
        await props.api.resizeTerminal(props.session.profileId, props.session.id, size.cols, size.rows);
        lastSize = size;
      } catch (error) { if (!disposed && writable.value) emit('error', cleanError(error)); }
    }
  } finally { resizeInFlight = false; }
}
function fit() {
  if (disposed || !props.active || !surface.value?.offsetWidth || !surface.value.offsetHeight || !terminal || !fitAddon) return;
  fitAddon.fit();
  resizePending = { cols: terminal.cols, rows: terminal.rows };
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => void flushResize(), 100);
}
async function activate() {
  await nextTick();
  if (props.active && !disposed) {
    fit();
    terminal?.focus();
    if (!reading) schedulePoll(0);
  }
}
onMounted(() => {
  if (!surface.value) return;
  terminal = new Terminal({
    cols: props.session.cols || 100, rows: props.session.rows || 28,
    fontFamily: '"Cascadia Mono", "Cascadia Code", Consolas, "SFMono-Regular", Menlo, monospace',
    fontSize: 14, lineHeight: 1.2, cursorBlink: !reducedMotion.matches, cursorStyle: 'block', cursorInactiveStyle: 'outline', scrollback: 5000,
    theme: { background: '#080c12', foreground: '#e5eaf2', cursor: '#b4d0ff', cursorAccent: '#080c12', selectionBackground: '#3e608c99', black: '#1b2330', red: '#ed8796', green: '#8cd4ae', yellow: '#e9c58d', blue: '#85acf4', magenta: '#c7a0ed', cyan: '#8bd7df', white: '#d5dce8', brightBlack: '#7b879b', brightRed: '#f7a7b2', brightGreen: '#b0e7c9', brightYellow: '#f4dca9', brightBlue: '#b4ccff', brightMagenta: '#dbbdf5', brightCyan: '#b3e9ee', brightWhite: '#ffffff' },
    disableStdin: !writable.value,
    rightClickSelectsWord: false,
    // Escape sequences may render links but never open URLs or a local program.
    linkHandler: { activate: () => {} },
  });
  fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  terminal.open(surface.value);
  reducedMotion.addEventListener('change', applyCursorMotion);
  terminal.onData(data => sendBytes(new TextEncoder().encode(data)));
  terminal.onBinary(data => sendBytes(Uint8Array.from(data, char => char.charCodeAt(0))));
  terminal.attachCustomKeyEventHandler(event => {
    if (event.type !== 'keydown' || !event.ctrlKey || !event.shiftKey || event.altKey || event.metaKey) return true;
    if (event.code === 'KeyC') {
      copySelection();
      event.preventDefault();
      return false;
    }
    if (event.code === 'KeyV') {
      pasteClipboard();
      event.preventDefault();
      return false;
    }
    return true;
  });
  resizeObserver = new ResizeObserver(fit);
  resizeObserver.observe(surface.value);
  void activate();
  schedulePoll(0);
});
watch(() => props.active, active => { if (active) void activate(); else schedulePoll(); });
watch(writable, value => { if (terminal) terminal.options.disableStdin = !value; if (value) fit(); });
watch(() => props.disabled, disabled => { if (disabled) clearTimeout(pollTimer); else schedulePoll(0); });
watch(() => [props.active, writable.value, props.disabled] as const, () => { clipboardGeneration += 1; }, { flush: 'sync' });
onUnmounted(() => {
  disposed = true;
  clipboardGeneration += 1;
  clearTimeout(pollTimer);
  clearTimeout(resizeTimer);
  resizeObserver?.disconnect();
  reducedMotion.removeEventListener('change', applyCursorMotion);
  for (const finish of pendingRenders) finish();
  terminal?.dispose();
  terminal = undefined;
});
</script>

<template>
  <div class="terminal-session-panel" role="tabpanel" :data-terminal-id="session.id">
    <div ref="surface" class="terminal-surface" data-testid="terminal-surface" :data-terminal-id="session.id" :aria-label="`终端 ${session.title}`" @contextmenu.capture="terminalContextMenu" @mousedown.capture="terminalRightButton" @mouseup.capture="terminalRightButton" />
    <div v-if="notice" class="terminal-session-notice" :class="{ error: session.status === 'error', ended }" role="status">{{ notice }}</div>
  </div>
</template>
