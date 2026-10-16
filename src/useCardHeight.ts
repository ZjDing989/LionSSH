import { computed, onUnmounted, ref, watch } from 'vue';

/** Persist an optional whole-card height; untouched cards keep their natural size. */
export function useCardHeight(storageKey: string, minHeight = 280, maxHeight = 2400) {
  const clamp = (value: number) => Math.round(Math.max(minHeight, Math.min(maxHeight, value)));
  function loadHeight(): number | null {
    try {
      const saved = localStorage.getItem(storageKey);
      const value = Number(saved);
      return saved && Number.isFinite(value) && value > 0 ? clamp(value) : null;
    } catch { return null; }
  }

  const cardElement = ref<HTMLElement | null>(null);
  const cardHeight = ref(loadHeight());
  const measuredHeight = ref(minHeight);
  const currentHeight = computed(() => cardHeight.value ?? measuredHeight.value);
  const resizing = ref(false);
  let observer: ResizeObserver | undefined;
  let frame = 0;
  let drag: { pointerId: number; handle: HTMLElement; startY: number; startHeight: number; lastY: number; moved: boolean } | undefined;

  function saveHeight() {
    try {
      if (cardHeight.value === null) localStorage.removeItem(storageKey);
      else localStorage.setItem(storageKey, String(cardHeight.value));
    } catch { /* Resizing still works when browser storage is unavailable. */ }
  }
  function updateDragHeight() {
    if (!drag) return;
    const delta = drag.lastY + window.scrollY - drag.startY;
    // Focusing or clicking the grip alone must not resize an automatic card.
    if (!drag.moved && Math.abs(delta) < 2) return;
    drag.moved = true;
    cardHeight.value = clamp(drag.startHeight + delta);
  }
  function scrollWhileDragging() {
    if (!drag) return;
    // Keep the grip usable when growing a card beyond the visible window.
    const edge = 40;
    let step = 0;
    if (drag.moved && drag.lastY > innerHeight - edge && currentHeight.value < maxHeight)
      step = Math.min(18, Math.ceil((drag.lastY - innerHeight + edge) / 2));
    else if (drag.moved && drag.lastY < edge && currentHeight.value > minHeight)
      step = -Math.min(18, Math.ceil((edge - drag.lastY) / 2));
    if (step) {
      window.scrollBy(0, step);
      updateDragHeight();
    }
    frame = requestAnimationFrame(scrollWhileDragging);
  }
  function finishResize(event?: PointerEvent) {
    if (!drag || event && event.pointerId !== drag.pointerId) return;
    const previous = drag;
    drag = undefined;
    resizing.value = false;
    cancelAnimationFrame(frame);
    window.removeEventListener('scroll', updateDragHeight);
    window.removeEventListener('blur', blur);
    document.documentElement.classList.remove('resizing-card');
    if (previous.handle.hasPointerCapture(previous.pointerId)) previous.handle.releasePointerCapture(previous.pointerId);
    saveHeight();
  }
  function blur() { finishResize(); }
  function startResize(event: PointerEvent) {
    if (event.button !== 0 || !event.isPrimary || !cardElement.value || drag) return;
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    handle.focus({ preventScroll: true });
    handle.setPointerCapture(event.pointerId);
    const height = cardElement.value.getBoundingClientRect().height;
    drag = { pointerId: event.pointerId, handle, startY: event.clientY + window.scrollY, startHeight: height, lastY: event.clientY, moved: false };
    resizing.value = true;
    document.documentElement.classList.add('resizing-card');
    window.addEventListener('scroll', updateDragHeight, { passive: true });
    window.addEventListener('blur', blur);
    frame = requestAnimationFrame(scrollWhileDragging);
  }
  function moveResize(event: PointerEvent) {
    if (!drag || event.pointerId !== drag.pointerId) return;
    drag.lastY = event.clientY;
    updateDragHeight();
  }
  function resetHeight() {
    finishResize();
    cardHeight.value = null;
    saveHeight();
  }
  function resizeKey(event: KeyboardEvent) {
    const step = event.shiftKey ? 80 : 24;
    let value: number;
    if (event.key === 'ArrowDown') value = currentHeight.value + step;
    else if (event.key === 'ArrowUp') value = currentHeight.value - step;
    else if (event.key === 'Home') value = minHeight;
    else if (event.key === 'End') value = maxHeight;
    else return;
    event.preventDefault();
    cardHeight.value = clamp(value);
    saveHeight();
  }

  watch(cardElement, element => {
    finishResize();
    observer?.disconnect();
    if (!element) return;
    const measure = () => { measuredHeight.value = clamp(element.getBoundingClientRect().height); };
    measure();
    observer = new ResizeObserver(measure);
    observer.observe(element);
  }, { flush: 'post' });
  onUnmounted(() => { finishResize(); observer?.disconnect(); });

  return { cardElement, cardHeight, currentHeight, resizing, minHeight, maxHeight, startResize, moveResize, finishResize, resizeKey, resetHeight };
}
