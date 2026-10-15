'use strict';

// squeue's ordinary output is right aligned; the final combined node/reason
// field is the only variable-width field. No custom remote formatting is used.
const widths = [18, 9, 8, 8, 2, 10, 6];
function row(values) {
  return values.map((value, index) => index < widths.length ? String(value).padStart(widths[index]) : value).join(' ');
}
const HEADER = row(['JOBID', 'PARTITION', 'NAME', 'USER', 'ST', 'TIME', 'NODES', 'NODELIST(REASON)']);
const runningRow = row(['4201', 'gpu', 'train', 'tester', 'R', '1:02:03', 2, 'gpu[01-02]']);
const pendingRow = row(['4202', 'gpu', 'next', 'tester', 'PD', '0:00', 1, '(Resources)']);
const allRow = row(['4203', 'batch', 'other', 'another', 'CG', '2-01:02:03', 1, 'cpu09']);
const mineQueue = `${HEADER}\n${runningRow}\n${pendingRow}\n`;
const allQueue = `${mineQueue}${allRow}\n`;

module.exports = { HEADER, row, runningRow, pendingRow, allRow, mineQueue, allQueue };
