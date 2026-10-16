import { createUuid } from "@escode/shared";
import type { ProviderSettingsFormProvider } from "@/lib/providerSettingsFormTypes.js";

export function generateId(): string {
  return createUuid();
}

export type ModelProviderNavItem = {
  key: string;
  type: "custom";
  label: string;
  provider: ProviderSettingsFormProvider;
  statusActive: boolean;
};

export type ModelProviderNavGroupId = "custom";

export interface ModelProviderNavGroup {
  id: ModelProviderNavGroupId;
  title: string;
  items: ModelProviderNavItem[];
}
