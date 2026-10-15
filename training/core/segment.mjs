// Deterministic task segmentation for observed steps without explicit
// teach/finish markers. Explicit markers always win (the caller passes them as
// `boundaries`). Application switching alone is never a boundary: work moves
// between apps inside one task. Boundaries come from long idle gaps, from a
// switch that coincides with a shorter idle gap, and from explicit markers.
export const DEFAULTS = {
  idleNs: 5n * 60n * 1000000000n, // 5 minutes of nothing ends a task
  switchIdleNs: 90n * 1000000000n, // app switch after 90 s idle starts one
};

export function segmentSteps(
  steps,
  { boundaries = [], idleNs = DEFAULTS.idleNs, switchIdleNs = DEFAULTS.switchIdleNs } = {},
) {
  if (!steps.length) return [];
  const marks = boundaries.map((b) => BigInt(b)).sort((a, b) => (a < b ? -1 : 1));
  const groups = [[steps[0]]];
  for (let i = 1; i < steps.length; i += 1) {
    const previous = steps[i - 1];
    const step = steps[i];
    const t0 = BigInt(previous.action.monotonic_ns);
    const t1 = BigInt(step.action.monotonic_ns);
    const gap = t1 - t0;
    const marked = marks.some((mark) => mark > t0 && mark <= t1);
    const app0 = previous.action.target?.app;
    const app1 = step.action.target?.app;
    const switched = app0 && app1 && app0 !== app1;
    if (marked || gap >= idleNs || (switched && gap >= switchIdleNs)) groups.push([step]);
    else groups.at(-1).push(step);
  }
  return groups;
}
