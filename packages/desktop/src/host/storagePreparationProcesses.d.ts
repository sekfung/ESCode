import { type DatabaseMigrationFacts, type DatabaseStartupState } from "@escode/shared";
import { resolveDefaultESCodeAgentCommand } from "@escode/services/storage-startup";
type Phase = NonNullable<DatabaseStartupState["databasePhase"]>;
export declare function prepareHostStorage(path: string, report: (phase: Phase, migration?: DatabaseMigrationFacts) => void, signal: AbortSignal): Promise<void>;
/** Host 拥有存储准备的 Worker/原生子进程，两种入口共享协议和结束判定。 */
export declare function prepareSessionStorage(options: {
    cwd: string;
    env?: Record<string, string>;
    signal: AbortSignal;
    report: (phase: Phase, details?: {
        databaseId: string;
        migration?: DatabaseMigrationFacts;
    }) => void;
    preparedPaths?: Set<string>;
    resolveCommand?: typeof resolveDefaultESCodeAgentCommand;
    observePath: (path: string) => Promise<void>;
}): Promise<void>;
export {};
//# sourceMappingURL=storagePreparationProcesses.d.ts.map