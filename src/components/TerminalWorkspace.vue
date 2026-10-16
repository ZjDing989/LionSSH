<script setup lang="ts">
import { computed, nextTick, reactive, ref, watch } from 'vue';
import Icon from './Icon.vue';
import TerminalSessionPane from './TerminalSessionPane.vue';
import type { LionApi, Profile, ServerState, TerminalSession } from '../types';
import { message, t } from '../i18n';

const props = defineProps<{
  visible: boolean;
  api: LionApi | undefined;
  profileId: string;
  profiles: Profile[];
  states: Record<string, ServerState>;
  disabled: boolean;
}>();
defineEmits<{ back: []; connect: [] }>();

interface TerminalGroup {
  sessions: TerminalSession[];
  activeId: string;
  loaded: boolean;
  loading: boolean;
  loadFailed: boolean;
  defaultAttempted: boolean;
  opening: boolean;
  error: string;
}
const groups = reactive<Record<string, TerminalGroup>>({});
const closing = reactive<Record<string, boolean>>({});
const renaming = reactive<Record<string, boolean>>({});
const titleOverrides: Record<string, string> = {};
const editing = ref<{ id: string; profileId: string; title: string } | null>(null);
const titleInput = ref<HTMLInputElement[] | HTMLInputElement | null>(null);
const workspaceRoot = ref<HTMLElement | null>(null);
const profile = computed(() => props.profiles.find(item => item.id === props.profileId));
const group = computed(() => groups[props.profileId]);
const connected = computed(() => props.states[props.profileId]?.status === 'connected');
const connectionActive = computed(() => ['connecting', 'connected', 'reconnecting', 'disconnecting'].includes(props.states[props.profileId]?.status || ''));
const available = computed(() => Boolean(props.api?.listTerminals && props.api.openTerminal));
const canOpen = computed(() => Boolean(profile.value && connected.value && available.value && !props.disabled && !group.value?.opening && !group.value?.loading));
const allSessions = computed(() => Object.values(groups).flatMap(item => item.sessions));

function cleanError(error: unknown) {
  return (error instanceof Error ? error.message : String(error)).replace(/^Error:\s*/, '');
}
function getGroup(profileId: string) {
  if (!groups[profileId]) groups[profileId] = { sessions: [], activeId: '', loaded: false, loading: false, loadFailed: false, defaultAttempted: false, opening: false, error: '' };
  return groups[profileId];
}
function isConnected(profileId: string) { return props.states[profileId]?.status === 'connected'; }
function groupExists(profileId: string, target: TerminalGroup) { return groups[profileId] === target && props.profiles.some(item => item.id === profileId); }
function statusLabel(session: TerminalSession) {
  if (session.status === 'error') return t('连接异常');
  if (session.status === 'exited') return session.exitCode === null ? t('已结束') : t('已结束 ({code})', { code: session.exitCode });
  if (!isConnected(session.profileId)) return t('已断开');
  return session.status === 'opening' ? t('正在连接') : t('运行中');
}
function sessionTitle(session: TerminalSession) {
  return session.defaultTitleNumber != null ? t('终端 {number}', { number: session.defaultTitleNumber }) : session.title;
}
function statusTone(session: TerminalSession) {
  return session.status === 'error' ? 'negative' : session.status === 'exited' || !isConnected(session.profileId) ? 'neutral' : session.status === 'opening' ? 'pending' : 'positive';
}
async function loadGroup(profileId: string) {
  const target = getGroup(profileId);
  if (target.loaded || target.loading) return;
  if (!props.api?.listTerminals) {
    target.error = '当前界面未连接支持终端的桌面后台。';
    target.loadFailed = true;
    return;
  }
  target.loading = true;
  target.loadFailed = false;
  target.error = '';
  try {
    const sessions = await props.api.listTerminals(profileId);
    if (!groupExists(profileId, target)) return;
    target.sessions = sessions;
    target.activeId = target.sessions[0]?.id || '';
    target.loaded = true;
    if (target.sessions.length) target.defaultAttempted = true;
  } catch (error) {
    target.error = cleanError(error);
    target.loadFailed = true;
  } finally { target.loading = false; }
}
async function visitGroup(profileId: string) {
  if (!props.profiles.some(item => item.id === profileId)) return;
  const target = getGroup(profileId);
  if (target.loadFailed) return;
  await loadGroup(profileId);
  if (groupExists(profileId, target) && target.loaded && !target.defaultAttempted && props.visible && props.profileId === profileId && isConnected(profileId) && !props.disabled) {
    target.defaultAttempted = true;
    await createTerminal(profileId);
  }
}
async function createTerminal(profileId = props.profileId) {
  if (!props.profiles.some(item => item.id === profileId)) return;
  const target = getGroup(profileId);
  if (!props.api?.openTerminal || !isConnected(profileId) || props.disabled || target.opening || target.loading) return;
  target.defaultAttempted = true;
  if (!target.loaded) {
    await loadGroup(profileId);
    if (!target.loaded || !groupExists(profileId, target)) return;
  }
  target.opening = true;
  target.error = '';
  try {
    const session = await props.api.openTerminal(profileId, 100, 28);
    if (!groupExists(profileId, target)) return;
    target.sessions.push(session);
    target.activeId = session.id;
  } catch (error) { target.error = cleanError(error); }
  finally { target.opening = false; }
}
async function retryLoad() {
  const target = getGroup(props.profileId);
  target.loadFailed = false;
  await visitGroup(props.profileId);
}
async function activate(session: TerminalSession) {
  const target = groups[session.profileId];
  if (!target) return;
  target.activeId = session.id;
  await nextTick();
  if (!props.visible || editing.value || props.profileId !== session.profileId) return;
  workspaceRoot.value?.querySelector<HTMLTextAreaElement>(`.terminal-surface[data-terminal-id="${CSS.escape(session.id)}"] .xterm-helper-textarea`)?.focus();
}
async function closeTerminal(session: TerminalSession) {
  if (!props.api?.closeTerminal || closing[session.id] || props.disabled) return;
  const target = groups[session.profileId];
  if (!target) return;
  closing[session.id] = true;
  target.error = '';
  if (editing.value?.id === session.id) editing.value = null;
  try {
    await props.api.closeTerminal(session.profileId, session.id);
    const index = target.sessions.findIndex(item => item.id === session.id);
    target.sessions = target.sessions.filter(item => item.id !== session.id);
    if (target.activeId === session.id) target.activeId = target.sessions[Math.min(index, target.sessions.length - 1)]?.id || '';
    delete titleOverrides[session.id];
  } catch (error) { target.error = cleanError(error); }
  finally { delete closing[session.id]; }
}
async function editTitle(session: TerminalSession) {
  if (closing[session.id] || renaming[session.id] || props.disabled) return;
  editing.value = { id: session.id, profileId: session.profileId, title: sessionTitle(session) };
  await nextTick();
  const input = Array.isArray(titleInput.value) ? titleInput.value[0] : titleInput.value;
  input?.focus();
  input?.select();
}
async function saveTitle() {
  const draft = editing.value;
  editing.value = null;
  if (!draft || !props.api?.renameTerminal) return;
  const target = groups[draft.profileId];
  if (!target) return;
  const current = target.sessions.find(item => item.id === draft.id);
  const title = draft.title.trim();
  if (!current || !title || title === sessionTitle(current)) return;
  const previousTitle = current.title;
  titleOverrides[draft.id] = title;
  current.title = title;
  const previousDefaultTitleNumber = current.defaultTitleNumber;
  current.defaultTitleNumber = null;
  renaming[draft.id] = true;
  try { updateSession(await props.api.renameTerminal(draft.profileId, draft.id, title), false); }
  catch (error) { delete titleOverrides[draft.id]; current.title = previousTitle; current.defaultTitleNumber = previousDefaultTitleNumber; target.error = cleanError(error); }
  finally { delete renaming[draft.id]; }
}
function updateSession(session: TerminalSession, confirmTitle = true) {
  const target = groups[session.profileId];
  if (!target) return;
  const index = target.sessions.findIndex(item => item.id === session.id);
  if (index < 0) return;
  if (confirmTitle && session.title === titleOverrides[session.id]) delete titleOverrides[session.id];
  target.sessions[index] = { ...session, title: titleOverrides[session.id] || session.title, defaultTitleNumber: titleOverrides[session.id] ? null : session.defaultTitleNumber };
}
function reportError(profileId: string, error: string) {
  if (groups[profileId]) groups[profileId].error = error;
}
watch(() => [props.visible, props.profileId, props.states[props.profileId]?.status, props.disabled] as const, ([visible, profileId]) => {
  if (visible && profileId && !props.disabled) void visitGroup(profileId);
  if (editing.value && editing.value.profileId !== profileId) void saveTitle();
}, { immediate: true });
watch(() => props.profiles.map(item => item.id), ids => {
  for (const id of Object.keys(groups)) if (!ids.includes(id)) delete groups[id];
});
</script>

<template>
  <section ref="workspaceRoot" class="terminal-page" data-testid="terminal-page">
    <div class="tool-page-toolbar"><button class="button secondary small" data-testid="terminal-back" @click="$emit('back')"><Icon name="back" :size="16" />{{ t('返回工作台') }}</button><span>{{ t('小工具 / 终端') }}</span></div>
    <header class="page-header terminal-page-header"><h1>{{ t('终端') }}<span class="title-dot">.</span></h1><div v-if="profile" class="terminal-server-label"><Icon name="server" :size="16" /><strong>{{ profile.name }}</strong><span>{{ profile.username }}@{{ profile.host }}:{{ profile.port }}</span></div></header>
    <div v-if="profile && !connected" class="terminal-connection-note" data-testid="terminal-connection-note" role="status"><Icon name="info" :size="16" /><span>{{ t('服务器尚未连接，已有终端会保留。') }}</span><button v-if="!connectionActive" class="button secondary small" :disabled="disabled" @click="$emit('connect')"><Icon name="link" :size="14" />{{ t('连接设备') }}</button><span v-else>{{ t(props.states[profileId]?.status === 'disconnecting' ? '正在断开连接' : '正在连接') }}</span></div>
    <div v-if="group?.error" class="terminal-error" data-testid="terminal-error" role="alert"><Icon name="alert" :size="16" /><span>{{ message(group.error) }}</span><button v-if="group.loadFailed && available" class="text-button" data-testid="terminal-retry" @click="retryLoad">{{ t('重试') }}</button><button v-else class="icon-button" :aria-label="t('关闭终端提示')" @click="group.error = ''"><Icon name="close" :size="15" /></button></div>
    <div class="terminal-workspace card">
      <div class="terminal-tabbar" role="tablist" :aria-label="t('服务器终端')">
        <div v-for="session in group?.sessions || []" :key="session.id" class="terminal-tab" :class="{ active: group?.activeId === session.id, closing: closing[session.id] }" data-testid="terminal-tab" :data-terminal-id="session.id" role="tab" :aria-selected="group?.activeId === session.id" :aria-controls="`terminal-pane-${session.id}`" :tabindex="group?.activeId === session.id ? 0 : -1" :title="t('{title} · {status}；双击修改备注', { title: sessionTitle(session), status: statusLabel(session) })" @click="activate(session)" @dblclick="editTitle(session)" @keydown.enter.prevent="activate(session)" @keydown.space.prevent="activate(session)">
          <i class="status-dot" :class="statusTone(session)" />
          <input v-if="editing?.id === session.id" ref="titleInput" v-model="editing.title" class="terminal-title-input" data-testid="terminal-title-input" :aria-label="t('终端备注')" maxlength="80" @click.stop @dblclick.stop @keydown.stop @keydown.enter.prevent="saveTitle" @keydown.esc.prevent="editing = null" @blur="saveTitle" />
          <span v-else class="terminal-tab-title">{{ sessionTitle(session) }}</span>
          <button class="terminal-tab-close" data-testid="terminal-close" :data-terminal-id="session.id" :aria-label="t('关闭终端 {title}', { title: sessionTitle(session) })" :disabled="closing[session.id] || disabled" :title="t('关闭终端')" @click.stop="closeTerminal(session)" @dblclick.stop @keydown.stop><Icon :name="closing[session.id] ? 'refresh' : 'close'" :size="13" :class="{ spinning: closing[session.id] }" /></button>
        </div>
        <button class="terminal-add" data-testid="terminal-add" :disabled="!canOpen" :aria-label="t(group?.opening ? '正在新建终端' : '新建终端')" :title="t(connected ? '在当前服务器新建终端' : '连接服务器后可新建终端')" @click="createTerminal()"><Icon :name="group?.opening || group?.loading ? 'refresh' : 'plus'" :size="18" :class="{ spinning: group?.opening || group?.loading }" /></button>
      </div>
      <div class="terminal-panels">
        <TerminalSessionPane v-for="session in allSessions" :id="`terminal-pane-${session.id}`" :key="session.id" v-show="session.profileId === profileId && groups[session.profileId]?.activeId === session.id" :api="api" :session="session" :active="visible && session.profileId === profileId && groups[session.profileId]?.activeId === session.id" :connected="isConnected(session.profileId)" :disabled="disabled || Boolean(closing[session.id])" @session="updateSession" @error="reportError(session.profileId, $event)" />
        <div v-if="!group?.sessions.length" class="terminal-empty" data-testid="terminal-empty"><Icon name="terminal" :size="37" /><h2>{{ t(!profile ? '先选择一台服务器' : group?.loading || group?.opening ? '正在打开终端' : '还没有打开的终端') }}</h2><button v-if="profile" class="button secondary" :disabled="!canOpen" @click="createTerminal()"><Icon name="plus" :size="16" />{{ t('新建终端') }}</button></div>
      </div>
    </div>
  </section>
</template>
