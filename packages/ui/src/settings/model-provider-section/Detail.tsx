import {
  getProviderFormApiKeyManagementUrl,
  type ProviderSettingsFormProvider,
} from "@/lib/providerSettingsFormTypes.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { ModelProviderNavItem } from "./constants.js";
import { InlineEditableProviderCard } from "./InlineEditableProviderCard.js";
import { Loader2Icon } from "lucide-react";
import type { ModelConnectivityResult } from "@escode/shared";
import type { SavePersonalModelDraftInput } from "@escode/provider";
import { useProviderSettingsView } from "@/hooks/useProviderSettingsView.js";
import type { ProviderSettingsView } from "@escode/services";

function ModelProviderLoadingCard({ loadingLabel }: { loadingLabel: string }) {
  return (
    <div className="flex flex-col gap-2 rounded-xl border border-border bg-surface p-3">
      <div className="flex items-center gap-2 text-ui-base text-foreground-subtle">
        <Loader2Icon className="size-4 animate-spin" />
        <span>{loadingLabel}</span>
      </div>
    </div>
  );
}

export function ModelProviderSectionDetail({
  selectedNavItem,
  onSave,
  onAddPersonalModel,
  onSavePersonalModelDraft,
  onSetPersonalModelEnabled,
  onDeletePersonalModel,
  onDelete,
  onReorderProviderModels,
  onTestModel,
  onOpenApiKeyUrl,
  providerSettingsView: providerSettingsViewOverride,
}: {
  selectedNavItem: ModelProviderNavItem | null;
  onSave: (config: ProviderSettingsFormProvider) => void | Promise<void>;
  onAddPersonalModel?: (
    providerId: string,
    modelId: string,
    config: ProviderSettingsFormProvider["models"][number]["personalConfig"],
    useRecommendedConfig?: boolean,
  ) => Promise<unknown>;
  onSavePersonalModelDraft?: (input: SavePersonalModelDraftInput) => Promise<unknown>;
  onSetPersonalModelEnabled?: (
    providerId: string,
    modelId: string,
    enabled: boolean,
  ) => Promise<unknown>;
  onDeletePersonalModel?: (providerId: string, modelId: string) => Promise<unknown>;
  onDelete: (provider: ProviderSettingsFormProvider) => Promise<void>;
  onReorderProviderModels?: (providerId: string, modelIds: string[]) => Promise<void>;
  onTestModel: (providerId: string, modelId: string) => Promise<ModelConnectivityResult>;
  onOpenApiKeyUrl: (url: string) => void;
  providerSettingsView?: ProviderSettingsView | null;
}) {
  const { intl } = useESCodeIntl();
  const loadingLabel = intl.formatMessage({ id: "common.loading" });
  const rootProviderSettingsRead = useProviderSettingsView();
  const rootProviderSettingsView =
    rootProviderSettingsRead.state.status === "ready" ? rootProviderSettingsRead.state.view : null;
  const providerSettingsView = providerSettingsViewOverride ?? rootProviderSettingsView;
  // 账号分支曾漏传删除回调，出现只删 UI 不写盘。所有详情共用同一套模型操作装配。
  const modelEditingProps = {
    onAddPersonalModel,
    onSavePersonalModelDraft,
    onSetPersonalModelEnabled,
    onDeletePersonalModel,
    settingsRevision: providerSettingsView?.revision,
  };

  if (!selectedNavItem) {
    return <ModelProviderLoadingCard loadingLabel={loadingLabel} />;
  }

  if (!selectedNavItem.provider) {
    return <ModelProviderLoadingCard loadingLabel={loadingLabel} />;
  }

  const customProvider = selectedNavItem.provider;
  const customApiKeyUrl = customProvider.templateId
    ? getProviderFormApiKeyManagementUrl(customProvider)
    : undefined;
  return (
    // 仅展示预设模板声明的入口，不根据地址猜测自定义 Provider 的 Key 控制台。
    <InlineEditableProviderCard
      provider={customProvider}
      onSave={onSave}
      {...modelEditingProps}
      onDelete={() => onDelete(customProvider)}
      onReorderModelIds={
        onReorderProviderModels
          ? (modelIds) => onReorderProviderModels(customProvider.providerId, modelIds)
          : undefined
      }
      onTestModel={onTestModel}
      presetApiKeyUrl={customApiKeyUrl}
      readOnlyEndpoints={false}
      nameEditable
      onOpenPresetApiKey={
        customApiKeyUrl
          ? () => {
              onOpenApiKeyUrl(customApiKeyUrl);
            }
          : undefined
      }
    />
  );
}
