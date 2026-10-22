import type {
  SvnBranchListRequest,
  SvnBranchListResult,
  SvnChangesResult,
  SvnCommitRequest,
  SvnCommitResult,
  SvnDiffRequest,
  SvnDiffResult,
  SvnGenerateCommitMessageRequest,
  SvnGenerateCommitMessageResult,
  SvnLogRequest,
  SvnLogResult,
  SvnMergeRequest,
  SvnMergeResult,
  SvnRemoteStatusRequest,
  SvnRemoteStatusResult,
  SvnRequest,
  SvnResolveRequest,
  SvnResult,
  SvnRevertRequest,
  SvnSetBinaryPathRequest,
  SvnSwitchRequest,
  SvnSwitchResult,
  SvnUpdateRequest,
  SvnUpdateResult,
  SvnWorkingCopyInfo,
} from "@escode/shared";
import { ServiceChannels } from "@escode/shared";
import { createServiceDescriptor } from "../descriptors.js";

/**
 * SVN 工作副本服务（spec：docs/svn/architecture.md）。
 * 无状态：事实来源是工作副本本身；同一 workspace 身份 key 的写操作在 Host 内串行。
 * 可预期的业务失败以 `{ ok: false, issue }` 返回，不抛异常。
 */
export interface ISvnService {
  getWorkingCopyInfo(params: SvnRequest): Promise<SvnWorkingCopyInfo>;
  getChanges(params: SvnRequest): Promise<SvnResult<SvnChangesResult>>;
  getDiff(params: SvnDiffRequest): Promise<SvnResult<SvnDiffResult>>;
  getLog(params: SvnLogRequest): Promise<SvnResult<SvnLogResult>>;
  getRemoteStatus(params: SvnRemoteStatusRequest): Promise<SvnResult<SvnRemoteStatusResult>>;
  listBranches(params: SvnBranchListRequest): Promise<SvnResult<SvnBranchListResult>>;
  commit(params: SvnCommitRequest): Promise<SvnResult<SvnCommitResult>>;
  update(params: SvnUpdateRequest): Promise<SvnResult<SvnUpdateResult>>;
  revert(params: SvnRevertRequest): Promise<SvnResult>;
  resolve(params: SvnResolveRequest): Promise<SvnResult>;
  switchBranch(params: SvnSwitchRequest): Promise<SvnResult<SvnSwitchResult>>;
  merge(params: SvnMergeRequest): Promise<SvnResult<SvnMergeResult>>;
  cleanup(params: SvnRequest): Promise<SvnResult>;
  setBinaryPath(params: SvnSetBinaryPathRequest): Promise<void>;
  generateCommitMessage(
    params: SvnGenerateCommitMessageRequest,
  ): Promise<SvnResult<SvnGenerateCommitMessageResult>>;
}

export const ISvnService = createServiceDescriptor<ISvnService>(ServiceChannels.Svn);
