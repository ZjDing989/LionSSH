'use strict';

// Representative output only; this is never fetched from a remote machine.
const cards = Array.from({ length: 8 }, (_, index) => ({
  index,
  name: 'NVIDIA GeForce RTX 4090',
  memoryUsedMiB: [18000, 12000, 19, 19, 19, 8000, 19, 19][index],
  memoryTotalMiB: 24564,
  utilization: [90, 70, 0, 0, 0, 0, 0, 0][index],
}));
const processes = [
  ...cards.map((card) => ({ index: card.index, pid: 1000 + card.index, type: 'G', name: '/usr/lib/xorg/Xorg', memory: 4 })),
  { index: 0, pid: 2000, type: 'C', name: 'python', memory: 17996 },
  { index: 1, pid: 2001, type: 'C+G', name: 'python', memory: 11996 },
  // Even 0% utilization and 19 MiB memory must remain busy with an MPS job.
  { index: 2, pid: 2002, type: 'M', name: 'nvidia-cuda-mps-server', memory: 15 },
  { index: 5, pid: 2005, type: 'C', name: 'python', memory: 7996 },
];

function makeSummary(options = {}) {
  const inputCards = options.cards || cards;
  const inputProcesses = options.processes || processes;
  const lines = [
    'Wed Oct 14 12:00:00 2026',
    '+-----------------------------------------------------------------------------------------+',
    '| NVIDIA-SMI 580.95.05              Driver Version: 580.95.05      CUDA Version: 13.0      |',
    '|-----------------------------------------+------------------------+----------------------|',
    '| GPU  Name                 Persistence-M | Bus-Id          Disp.A | Volatile Uncorr. ECC |',
    '| Fan  Temp   Perf          Pwr:Usage/Cap  |           Memory-Usage | GPU-Util  Compute M. |',
    '|                                         |                        |               MIG M. |',
    '|=========================================+========================+======================|',
  ];
  for (const card of inputCards) {
    const used = card.memoryUsedMiB === null ? 'N/A' : `${card.memoryUsedMiB}MiB`;
    const total = card.memoryTotalMiB === null ? 'N/A' : `${card.memoryTotalMiB}MiB`;
    const utilization = card.utilization === null ? 'N/A' : `${card.utilization}%`;
    lines.push(`| ${String(card.index).padStart(3)}  ${card.name}       Off |   00000000:18:00.0 Off |                  Off |`);
    lines.push(`|  0%   32C    P8             26W /  450W | ${used.padStart(10)} / ${total.padStart(8)} | ${utilization.padStart(6)}      Default |`);
    lines.push('|                                         |                        |                  N/A |');
    lines.push('+-----------------------------------------+------------------------+----------------------+');
  }
  if (options.includeProcesses !== false) {
    lines.push('');
    lines.push('+-----------------------------------------------------------------------------------------+');
    lines.push('| Processes:                                                                              |');
    lines.push('|  GPU   GI   CI        PID   Type   Process name                              GPU Memory |');
    lines.push('|        ID   ID                                                               Usage      |');
    lines.push('|=========================================================================================|');
    if (inputProcesses.length === 0) lines.push('|  No running processes found                                                             |');
    for (const item of inputProcesses) {
      lines.push(`| ${String(item.index).padStart(4)}   N/A  N/A ${String(item.pid).padStart(10)} ${item.type.padStart(6)}   ${item.name.padEnd(40)} ${String(item.memory).padStart(5)}MiB |`);
    }
    if (!options.truncatedProcesses) lines.push('+-----------------------------------------------------------------------------------------+');
  }
  return `${lines.join('\n')}\n`;
}

module.exports = { makeSummary, cards, processes, summaryFixture: makeSummary() };
