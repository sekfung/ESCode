import {
  collectVisibleESCodeBackgroundTaskControlItems,
  getESCodeBackgroundTaskControlItemElapsedMs,
  isActiveESCodeBackgroundTaskControlItem,
  parseESCodeBackgroundTaskControlItems,
  type ESCodeBackgroundTaskControlItem,
  type ESCodeBackgroundTaskControlStatus,
} from "./background-task-controls.js";

export type ESCodeBackgroundBashJobStatus = ESCodeBackgroundTaskControlStatus;
export type ESCodeBackgroundBashJob = ESCodeBackgroundTaskControlItem & {
  taskKind: "bash";
};

export function parseESCodeBackgroundBashJobs(value: unknown): ESCodeBackgroundBashJob[] {
  return parseESCodeBackgroundTaskControlItems(value).filter(isBackgroundBashJob);
}

export function isActiveESCodeBackgroundBashJob(job: ESCodeBackgroundBashJob): boolean {
  return isActiveESCodeBackgroundTaskControlItem(job);
}

export function getESCodeBackgroundBashJobElapsedMs(
  job: ESCodeBackgroundBashJob,
  now = Date.now(),
): number {
  return getESCodeBackgroundTaskControlItemElapsedMs(job, now);
}

export function collectVisibleESCodeBackgroundBashJobs(
  jobs: readonly ESCodeBackgroundBashJob[],
  now = Date.now(),
  thresholdMs = 30_000,
): Array<ESCodeBackgroundBashJob & { elapsedMs: number }> {
  return collectVisibleESCodeBackgroundTaskControlItems(jobs, now, thresholdMs) as Array<
    ESCodeBackgroundBashJob & { elapsedMs: number }
  >;
}

function isBackgroundBashJob(job: ESCodeBackgroundTaskControlItem): job is ESCodeBackgroundBashJob {
  return job.taskKind === "bash";
}
