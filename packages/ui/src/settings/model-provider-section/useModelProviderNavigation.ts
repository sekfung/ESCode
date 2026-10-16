import { useEffect, useMemo } from "react";
import type { ProviderSettingsFormProvider } from "@/lib/providerSettingsFormTypes.js";
import { getProviderFormLabel } from "@/lib/providerSettingsFormTypes.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { ModelProviderNavGroup } from "@/settings/model-provider-section/constants.js";
import { createCustomProviderNodeKey } from "@/settings/model-provider-section/utils.js";
import {
  sortModelProvidersForDisplay,
  type ProviderOrderView,
} from "@/lib/modelProviderOrdering.js";

interface UseModelProviderNavigationOptions {
  modelProviders: ProviderSettingsFormProvider[];
  modelProvidersLoading?: boolean;
  displayOrder?: ProviderOrderView;
  selectedNodeKey: string | null;
  setSelectedNodeKey: (key: string | null) => void;
  intl: ReturnType<typeof useESCodeIntl>["intl"];
}

export function useModelProviderNavigation({
  modelProviders,
  displayOrder,
  selectedNodeKey,
  setSelectedNodeKey,
  intl,
}: UseModelProviderNavigationOptions) {
  const customProviders = useMemo(() => {
    const allCustomProviders = modelProviders.filter(
      (provider) => provider.config.group === "standard-personal",
    );
    // 这里复用模型菜单的展示排序，确保设置页和聊天框供应商顺序一致。
    return sortModelProvidersForDisplay(allCustomProviders, displayOrder);
  }, [displayOrder, modelProviders]);

  const navigationGroups = useMemo<ModelProviderNavGroup[]>(() => {
    return [
      {
        id: "custom",
        title: intl.formatMessage({ id: "settings.modelProvider.customTitle" }),
        items: customProviders.map((provider) => ({
          key: createCustomProviderNodeKey(provider.providerId),
          type: "custom" as const,
          label: getProviderFormLabel(provider),
          provider,
          statusActive: provider.executable === true,
        })),
      },
    ];
  }, [customProviders, intl]);

  const navigationItems = useMemo(
    () => navigationGroups.flatMap((group) => group.items),
    [navigationGroups],
  );

  const navigationItemByKey = useMemo(
    () => new Map(navigationItems.map((item) => [item.key, item])),
    [navigationItems],
  );

  const selectedNavItem = selectedNodeKey
    ? (navigationItemByKey.get(selectedNodeKey) ?? null)
    : null;

  const fallbackNodeKey = navigationItems[0]?.key ?? null;
  useEffect(() => {
    const hasSelectedNode = selectedNodeKey ? navigationItemByKey.has(selectedNodeKey) : false;
    if (hasSelectedNode) {
      return;
    }

    if (selectedNodeKey !== fallbackNodeKey) {
      setSelectedNodeKey(fallbackNodeKey);
    }
  }, [fallbackNodeKey, selectedNodeKey, setSelectedNodeKey, navigationItemByKey]);

  return {
    navigationGroups,
    navigationItems,
    selectedNavItem,
    navigationUnavailable: false,
  };
}
