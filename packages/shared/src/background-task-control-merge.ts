import type { ESCodeBackgroundTaskControlItem } from "./background-task-controls.js";

export function mergeESCodeBackgroundTaskControlItems(
  current: readonly ESCodeBackgroundTaskControlItem[],
  updates: readonly ESCodeBackgroundTaskControlItem[],
): ESCodeBackgroundTaskControlItem[] {
  const jobsById = new Map(current.map((job) => [job.jobId, job] as const));
  for (const job of updates) {
    jobsById.set(job.jobId, job);
  }
  return Array.from(jobsById.values());
}
