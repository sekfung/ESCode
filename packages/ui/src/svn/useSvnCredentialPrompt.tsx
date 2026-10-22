import { useCallback, useRef, useState, type FormEvent, type ReactNode } from "react";
import type { SvnCredentials, SvnResult } from "@escode/shared";
import { Button } from "@/components/ui/button.js";
import { Checkbox } from "@/components/ui/checkbox.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog.js";
import { Input } from "@/components/ui/input.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";

interface PendingPrompt {
  resolve: (credentials: SvnCredentials | null) => void;
  repositoryLabel?: string;
}

/**
 * 认证失败时收集凭据并**只重试一次**（spec：docs/svn/architecture.md「写操作与凭据重试」）。
 * 重试由 UI 发起，SvnService 保持无状态；取消时以 cancelled 结束，不循环弹窗。
 * 密码只在本次调用的闭包里存在，对话框关闭即清空，不进入任何 store。
 */
export function useSvnCredentialPrompt(repositoryLabel?: string): {
  runWithCredentialRetry: <T extends object>(
    operation: (credentials?: SvnCredentials) => Promise<SvnResult<T>>,
  ) => Promise<SvnResult<T>>;
  credentialDialog: ReactNode;
} {
  const { intl } = useESCodeIntl();
  const [pending, setPending] = useState<PendingPrompt | null>(null);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [storeInSvnCache, setStoreInSvnCache] = useState(true);
  const pendingRef = useRef<PendingPrompt | null>(null);

  const close = useCallback((credentials: SvnCredentials | null) => {
    pendingRef.current?.resolve(credentials);
    pendingRef.current = null;
    setPending(null);
    setPassword("");
  }, []);

  const askForCredentials = useCallback(
    () =>
      new Promise<SvnCredentials | null>((resolve) => {
        const prompt = { resolve, repositoryLabel };
        pendingRef.current = prompt;
        setPending(prompt);
      }),
    [repositoryLabel],
  );

  const runWithCredentialRetry = useCallback(
    async <T extends object>(
      operation: (credentials?: SvnCredentials) => Promise<SvnResult<T>>,
    ): Promise<SvnResult<T>> => {
      const first = await operation();
      if (first.ok || first.issue !== "auth-required") {
        return first;
      }
      const credentials = await askForCredentials();
      if (!credentials) {
        return { ok: false, issue: "cancelled", message: "cancelled" };
      }
      return await operation(credentials);
    },
    [askForCredentials],
  );

  const credentialDialog = (
    <Dialog
      open={pending !== null}
      onOpenChange={(open) => {
        if (!open) {
          close(null);
        }
      }}
    >
      <DialogContent className="max-w-md gap-0 overflow-hidden rounded-2xl p-0">
        <DialogHeader className="gap-2 px-6 py-5 pb-0">
          <DialogTitle className="text-lg font-medium text-foreground">
            {intl.formatMessage({ id: "svn.credentials.title" })}
          </DialogTitle>
          <DialogDescription className="break-all text-ui-base leading-6 text-foreground-subtle">
            {pending?.repositoryLabel ?? intl.formatMessage({ id: "svn.credentials.description" })}
          </DialogDescription>
        </DialogHeader>
        <form
          className="space-y-4 px-6 py-6"
          onSubmit={(event: FormEvent<HTMLFormElement>) => {
            event.preventDefault();
            if (!username.trim()) {
              return;
            }
            close({ username: username.trim(), password, storeInSvnCache });
          }}
        >
          <div className="space-y-2">
            <label
              htmlFor="svn-credentials-username"
              className="inline-flex text-ui-base font-medium text-foreground-subtle"
            >
              {intl.formatMessage({ id: "svn.credentials.username" })}
            </label>
            <Input
              id="svn-credentials-username"
              autoFocus
              autoComplete="username"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
            />
          </div>
          <div className="space-y-2">
            <label
              htmlFor="svn-credentials-password"
              className="inline-flex text-ui-base font-medium text-foreground-subtle"
            >
              {intl.formatMessage({ id: "svn.credentials.password" })}
            </label>
            <Input
              id="svn-credentials-password"
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </div>
          <label className="flex items-center gap-2 text-ui-base text-foreground">
            <Checkbox
              checked={storeInSvnCache}
              onCheckedChange={(checked) => setStoreInSvnCache(checked === true)}
            />
            {intl.formatMessage({ id: "svn.credentials.remember" })}
          </label>
          <DialogFooter className="gap-2 pt-2">
            <Button type="button" variant="secondary" size="lg" onClick={() => close(null)}>
              {intl.formatMessage({ id: "common.cancel" })}
            </Button>
            <Button type="submit" size="lg" disabled={!username.trim()}>
              {intl.formatMessage({ id: "svn.credentials.submit" })}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );

  return { runWithCredentialRetry, credentialDialog };
}
