<script setup lang="ts">
withDefaults(defineProps<{ name: string; size?: number }>(), { size: 20 });
const paths: Record<string, string[]> = {
  plus: ['M12 5v14M5 12h14'],
  close: ['m6 6 12 12M18 6 6 18'],
  server: ['M5 3h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2ZM5 13h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4a2 2 0 0 1 2-2Z', 'M7 7h.01M7 17h.01M11 7h6M11 17h6'],
  chip: ['M7 4h10a3 3 0 0 1 3 3v10a3 3 0 0 1-3 3H7a3 3 0 0 1-3-3V7a3 3 0 0 1 3-3ZM9 9h6v6H9z', 'M8 1v3M16 1v3M8 20v3M16 20v3M1 8h3M1 16h3M20 8h3M20 16h3'],
  shield: ['m12 3 8 3v6c0 5-5 8-8 9-3-1-8-4-8-9V6l8-3Z', 'm8 12 3 3 5-6'],
  chevron: ['m9 5 7 7-7 7'],
  down: ['m6 9 6 6 6-6'],
  download: ['M12 3v12m-5-5 5 5 5-5', 'M4 16v5h16v-5'],
  refresh: ['M20 7v5h-5M4 17v-5h5', 'M6.5 6.5A8 8 0 0 1 20 12M4 12a8 8 0 0 0 13.5 5.5'],
  play: ['m9 5 11 7-11 7V5Z'],
  stop: ['M6 6h12v12H6z'],
  power: ['M12 2v10', 'M6 5a9 9 0 1 0 12 0'],
  edit: ['m16 3 5 5-12 12-6 1 1-6L16 3Z', 'm13 6 5 5'],
  trash: ['M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7'],
  key: ['M15 3a6 6 0 1 1-3 11L5 21H2v-3l7-7a6 6 0 0 1 6-8Z', 'M16 7h.01'],
  lock: ['M5 10h14v11H5zM8 10V6a4 4 0 0 1 8 0v4'],
  folder: ['M3 7V4h7l2 3h9v13H3V7Z'],
  clock: ['M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Z', 'M12 7v5l3 2'],
  arrow: ['M4 12h16m-6-6 6 6-6 6'],
  exchange: ['M3 7h17m-4-4 4 4-4 4M21 17H4m4-4-4 4 4 4'],
  activity: ['M2 12h5l3-8 4 16 3-8h5'],
  check: ['m5 12 4 4L19 6'],
  alert: ['m12 3 10 18H2L12 3Z', 'M12 9v5M12 17h.01'],
  link: ['M10 13a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-2 2M14 11a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l2-2'],
  eye: ['M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12Z', 'M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z'],
  info: ['M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Z', 'M12 11v6M12 7h.01'],
  sun: ['M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Z', 'M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5 19 19M5 19l1.5-1.5M17.5 6.5 19 5'],
  moon: ['M20.7 14A9 9 0 0 1 10 3.3 9 9 0 1 0 20.7 14Z'],
  monitor: ['M3 4h18v13H3zM8 21h8M12 17v4'],
};
</script>

<template>
  <svg :width="size" :height="size" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path v-for="(path, index) in paths[name] || paths.server" :key="index" :d="path" />
  </svg>
</template>
